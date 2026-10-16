//! Background work. A job (build, deploy, call, walkthrough…) runs one at a time, because the
//! wallet's `storage.json` isn't safe with concurrent writers. Read-only refreshes run beside it.
//! Everything reports back to the UI as [`Msg`]s.

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, anyhow, bail};
use tokio::{
    io::{AsyncBufReadExt as _, AsyncRead, AsyncWriteExt as _, BufReader},
    process::Command,
    sync::mpsc::{UnboundedSender, unbounded_channel},
};

use crate::{
    lez::{Lez, RepoInfo, repo_info},
    rpc::{AccountView, Rpc, describe_bytes, unhex},
    sequencer::{self, SeqStatus, StopTarget},
    state::Deployment,
    userprogs,
    wallet::{
        WalletAccount, is_noise, parse_account_list, parse_new_account, parse_prog_cli,
        parse_program_ids,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    Pending,
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub repo: RepoInfo,
    pub seq: SeqStatus,
}

pub enum Msg {
    Log(String),
    JobDone {
        name: String,
        log: PathBuf,
        result: Result<(), String>,
    },
    Status(Status),
    Accounts(Vec<WalletAccount>),
    /// Account views by id; `None` means not on chain.
    Views(BTreeMap<String, Option<AccountView>>),
    SetPayer(String),
    SetAccount(String),
    Deployed(Deployment),
    /// The chain and wallet were wiped.
    ChainReset,
    Checklist(Vec<String>),
    Step(usize, StepState),
}

/// What a job gets: the repo, the RPC, a channel to the UI, and its own log file.
pub struct Ctx {
    tx: UnboundedSender<Msg>,
    pub lez: Lez,
    pub state_dir: PathBuf,
    pub rpc: Rpc,
    pub password: String,
    log: Option<fs::File>,
    pub log_path: PathBuf,
}

pub struct CmdOut {
    pub ok: bool,
    pub out: String,
}

impl Ctx {
    pub fn new(
        tx: UnboundedSender<Msg>,
        lez: Lez,
        state_dir: PathBuf,
        rpc: Rpc,
        password: String,
        job: &str,
    ) -> Self {
        let logs = state_dir.join("logs");
        let _ = fs::create_dir_all(&logs);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let slug: String = job
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let log_path = logs.join(format!("{stamp}-{slug}.log"));
        Self {
            tx,
            lez,
            state_dir,
            rpc,
            password,
            log: fs::File::create(&log_path).ok(),
            log_path,
        }
    }

    pub fn send(&self, msg: Msg) {
        let _ = self.tx.send(msg);
    }

    /// To the job's log file, and to the log pane unless it's noise.
    pub fn log(&mut self, line: impl Into<String>) {
        self.write(line.into(), true);
    }

    fn write(&mut self, line: String, show: bool) {
        if let Some(file) = &mut self.log {
            let _ = writeln!(file, "{line}");
        }
        if show && !is_noise(&line) {
            self.send(Msg::Log(line));
        }
    }

    /// A command in the LEZ repo, with the wallet home set and the wallet's transaction dumps
    /// suppressed.
    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.lez.dir)
            .env("LEE_WALLET_HOME_DIR", self.lez.wallet_home())
            .env("SUPPRESS_VERBOSE_PRINTS", "1")
            .env("RISC0_DEV_MODE", "1");
        // A new session has no controlling terminal, so tools that write to /dev/tty (the risc0
        // guest build does) can't draw over the TUI.
        // SAFETY: setsid(2) is async-signal-safe, so it may run between fork and exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command
    }

    pub fn wallet(&self, args: &[&str]) -> Command {
        let mut command = self.command(self.lez.release_bin("wallet"));
        command.args(args);
        command
    }

    pub fn prog_cli(&self, payer: &str, args: &[String]) -> Command {
        let mut command = self.command(self.lez.release_bin("prog_cli"));
        command.arg("--payer").arg(payer).args(args);
        command
    }

    /// Runs `command`, streaming its stdout and stderr into the log, and returns all of it.
    pub async fn run(
        &mut self,
        label: &str,
        command: Command,
        stdin: Option<String>,
    ) -> anyhow::Result<CmdOut> {
        self.run_inner(label, command, stdin, true).await
    }

    /// Like [`Self::run`], but fails when the command does.
    pub async fn run_ok(
        &mut self,
        label: &str,
        command: Command,
        stdin: Option<String>,
    ) -> anyhow::Result<String> {
        let output = self.run(label, command, stdin).await?;
        if !output.ok {
            bail!("`{label}` failed");
        }
        Ok(output.out)
    }

    async fn run_inner(
        &mut self,
        label: &str,
        mut command: Command,
        stdin: Option<String>,
        show: bool,
    ) -> anyhow::Result<CmdOut> {
        self.write(format!("$ {label}"), show);
        command
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .with_context(|| format!("couldn't start `{label}`"))?;
        if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
            pipe.write_all(input.as_bytes()).await?;
        }
        let (line_tx, mut lines) = unbounded_channel();
        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(forward_lines(stdout, line_tx.clone()));
        }
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(forward_lines(stderr, line_tx));
        }
        let mut out = String::new();
        while let Some(line) = lines.recv().await {
            out.push_str(&line);
            out.push('\n');
            self.write(line, show);
        }
        let status = child.wait().await?;
        if !status.success() {
            self.write(format!("`{label}` exited with {status}"), show);
        }
        Ok(CmdOut {
            ok: status.success(),
            out,
        })
    }
}

async fn forward_lines(pipe: impl AsyncRead + Unpin, tx: UnboundedSender<String>) {
    let mut lines = BufReader::new(pipe).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if tx.send(sequencer::strip_ansi(&line)).is_err() {
            break;
        }
    }
}

// ---- Read-only refreshes ----

pub async fn gather_status(lez: Lez, state_dir: PathBuf, rpc: Rpc) -> Status {
    let url = rpc.url.clone();
    let blocking = tokio::task::spawn_blocking(move || {
        let port_open = sequencer::port_open(&url);
        let seq = SeqStatus {
            port_open,
            managed: sequencer::managed_pgid(&state_dir),
            listener: port_open.then(|| sequencer::listener_pid(&url)).flatten(),
            ..SeqStatus::default()
        };
        (repo_info(&lez), seq)
    })
    .await;
    let (repo, mut seq) = blocking.unwrap_or_default();
    if seq.port_open && rpc.healthy().await {
        seq.healthy = true;
        seq.block = rpc.last_block().await;
    }
    Status { repo, seq }
}

/// `wallet account list`, without logging.
pub async fn list_accounts(lez: &Lez) -> anyhow::Result<Vec<WalletAccount>> {
    let output = Command::new(lez.release_bin("wallet"))
        .arg("account")
        .arg("list")
        .current_dir(&lez.dir)
        .env("LEE_WALLET_HOME_DIR", lez.wallet_home())
        .stdin(Stdio::null())
        .output()
        .await?;
    Ok(parse_account_list(&String::from_utf8_lossy(&output.stdout)))
}

pub async fn fetch_views(rpc: &Rpc, ids: Vec<String>) -> BTreeMap<String, Option<AccountView>> {
    let mut views = BTreeMap::new();
    for id in ids {
        if let Ok(view) = rpc.account(&id).await {
            views.insert(id, view);
        }
    }
    views
}

// ---- Job steps ----

/// Links every `example_progs/*.rs` into LEZ's guest package and drops links whose file is gone.
pub fn sync_example_progs(ctx: &mut Ctx) -> anyhow::Result<()> {
    let guest_src = ctx.lez.guest_src_dir();
    for file in userprogs::sources(&userprogs::example_dir()) {
        let (name, linked) = userprogs::link(&file, &guest_src)?;
        if linked != userprogs::Linked::Unchanged {
            ctx.log(format!("linked {} as guest {name}", file.display()));
        }
    }
    for name in userprogs::prune(&guest_src) {
        // The Guests list reads $BIN, so drop what the build left there too: the program
        // binary, the raw ELF and its dep-info file.
        let bin_dir = ctx.lez.bin_dir();
        for built in [format!("{name}.bin"), name.clone(), format!("{name}.d")] {
            let _ = std::fs::remove_file(bin_dir.join(built));
        }
        ctx.log(format!("removed {name}: its file is gone"));
    }
    Ok(())
}

pub async fn build(ctx: &mut Ctx) -> anyhow::Result<()> {
    sync_example_progs(ctx)?;
    let mut command = ctx.command("cargo");
    command.args([
        "build",
        "--release",
        "-p",
        "wallet",
        "-p",
        "program_deployment",
        "-p",
        "example_program_deployment_methods",
    ]);
    ctx.run_ok(
        "cargo build --release -p wallet -p program_deployment -p example_program_deployment_methods",
        command,
        None,
    )
    .await?;
    Ok(())
}

/// Links `file` into LEZ's guest package, builds the guests, and deploys it as `jz_<stem>`.
pub async fn build_and_deploy(ctx: &mut Ctx, file: &Path, payer: &str) -> anyhow::Result<()> {
    let (name, linked) = userprogs::link(file, &ctx.lez.guest_src_dir())?;
    ctx.log(format!(
        "{} {} → guest {name}",
        match linked {
            userprogs::Linked::Unchanged => "already linked:",
            _ => "linked",
        },
        file.display()
    ));
    let mut command = ctx.command("cargo");
    command.args(["build", "--release", "-p", "example_program_deployment_methods"]);
    ctx.run_ok(
        "cargo build --release -p example_program_deployment_methods",
        command,
        None,
    )
    .await?;
    deploy(ctx, &name, payer).await?;
    Ok(())
}

/// Starts the sequencer and waits until it answers `checkHealth`.
pub async fn start_sequencer(ctx: &mut Ctx) -> anyhow::Result<()> {
    if ctx.rpc.healthy().await {
        ctx.log("a sequencer is already running and healthy");
        return Ok(());
    }
    if sequencer::port_open(&ctx.rpc.url) {
        bail!(
            "something already listens on {} but isn't a healthy sequencer",
            ctx.rpc.url
        );
    }
    let (lez, state_dir) = (ctx.lez.clone(), ctx.state_dir.clone());
    let pgid = tokio::task::spawn_blocking(move || sequencer::start(&lez, &state_dir)).await??;
    let log = sequencer::log_file(&ctx.state_dir);
    ctx.log(format!(
        "$ RISC0_DEV_MODE=1 just run-sequencer-standalone  (process group {pgid}, log {})",
        log.display()
    ));
    let started = Instant::now();
    let mut last_report = Instant::now();
    loop {
        if ctx.rpc.healthy().await {
            ctx.log(format!(
                "sequencer healthy after {}s",
                started.elapsed().as_secs()
            ));
            return Ok(());
        }
        if sequencer::managed_pgid(&ctx.state_dir).is_none() {
            bail!("the sequencer exited; see {}", log.display());
        }
        if started.elapsed() > Duration::from_mins(20) {
            bail!("the sequencer didn't become healthy within 20 minutes");
        }
        if last_report.elapsed() > Duration::from_secs(15) {
            last_report = Instant::now();
            let tail = sequencer::read_log_from(&ctx.state_dir, 0, 2048);
            let last = tail.lines().rev().find(|line| !line.trim().is_empty());
            ctx.log(format!(
                "waiting for the sequencer ({}s): {}",
                started.elapsed().as_secs(),
                last.map(sequencer::strip_ansi).unwrap_or_default()
            ));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

pub async fn stop_sequencer(ctx: &mut Ctx, target: StopTarget) -> anyhow::Result<()> {
    ctx.log(match target {
        StopTarget::Managed(pgid) => format!("stopping the sequencer (process group {pgid})"),
        StopTarget::External(pid) => format!("stopping the external sequencer (pid {pid})"),
    });
    let (url, state_dir) = (ctx.rpc.url.clone(), ctx.state_dir.clone());
    tokio::task::spawn_blocking(move || sequencer::stop(target, &url, &state_dir)).await??;
    ctx.log("sequencer stopped");
    Ok(())
}

/// Stop (if given), `just clean`, start: a fresh chain and an empty wallet.
pub async fn reset(ctx: &mut Ctx, stop: Option<StopTarget>) -> anyhow::Result<()> {
    if let Some(target) = stop {
        stop_sequencer(ctx, target).await?;
    }
    let mut command = ctx.command("just");
    command.arg("clean");
    let output = ctx.run("just clean", command, None).await?;
    if !output.ok {
        if ctx.lez.sequencer_home().exists() || ctx.lez.storage_json().exists() {
            bail!("`just clean` failed before removing the chain data");
        }
        ctx.log("(only the Docker step of `just clean` failed; chain data and wallet storage are gone)");
    }
    ctx.send(Msg::ChainReset);
    start_sequencer(ctx).await
}

/// `wallet check-health` (creates the wallet on first run), then imports the two test accounts.
pub async fn wallet_init(ctx: &mut Ctx) -> anyhow::Result<()> {
    let command = ctx.wallet(&["check-health"]);
    let password = format!("{}\n", ctx.password);
    ctx.run_ok("wallet check-health", command, Some(password))
        .await?;
    let mut command = ctx.command("just");
    command.arg("wallet-import-test-accounts");
    ctx.run_ok("just wallet-import-test-accounts", command, None)
        .await?;
    Ok(())
}

pub async fn new_account(ctx: &mut Ctx, label: &str) -> anyhow::Result<String> {
    let mut args = vec!["account", "new", "public"];
    if !label.is_empty() {
        args.extend(["--label", label]);
    }
    let command = ctx.wallet(&args);
    let output = ctx.run_ok("wallet account new public", command, None).await?;
    parse_new_account(&output).ok_or_else(|| anyhow!("no account id in the wallet's output"))
}

/// The first imported account with a balance: one of the genesis test accounts.
pub async fn choose_payer(ctx: &mut Ctx) -> anyhow::Result<String> {
    let accounts = list_accounts(&ctx.lez).await?;
    for account in accounts.iter().filter(|a| a.public && a.path.is_none()) {
        if ctx
            .rpc
            .account(&account.id)
            .await?
            .is_some_and(|view| view.balance() > 0)
        {
            return Ok(account.id.clone());
        }
    }
    bail!("no funded imported account in the wallet; initialise it first (key i)")
}

pub async fn deploy(ctx: &mut Ctx, guest: &str, payer: &str) -> anyhow::Result<Deployment> {
    let bin = ctx.lez.guest_bin(guest);
    if !bin.is_file() {
        bail!("{} doesn't exist; build first (key b)", bin.display());
    }
    let command = ctx.prog_cli(payer, &["deploy".to_owned(), bin.display().to_string()]);
    let output = ctx
        .run_ok(&format!("prog_cli deploy {guest}"), command, None)
        .await?;
    let program_id = parse_prog_cli(&output)
        .and_then(|result| result["program_id"].as_str().map(str::to_owned))
        .ok_or_else(|| anyhow!("prog_cli didn't print a program id"))?;
    ctx.log(format!("deployed {guest} → {program_id}"));
    let deployment = Deployment {
        name: guest.to_owned(),
        program_id,
        stale: false,
    };
    ctx.send(Msg::Deployed(deployment.clone()));
    Ok(deployment)
}

/// Sends one `prog_cli` call. On a revert, shows why, from the sequencer's log.
pub async fn call(ctx: &mut Ctx, payer: &str, args: &[String]) -> anyhow::Result<Vec<u8>> {
    let log_offset = sequencer::log_len(&ctx.state_dir);
    let managed = sequencer::managed_pgid(&ctx.state_dir).is_some();
    let command = ctx.prog_cli(payer, args);
    let label = format!("prog_cli {}", args.join(" "));
    let output = ctx.run(&label, command, None).await?;
    let result = parse_prog_cli(&output.out);
    let state = result
        .as_ref()
        .and_then(|result| unhex(result["state_hex"].as_str()?))
        .unwrap_or_default();
    if let Some(result) = &result {
        ctx.log(format!(
            "{} in block {}: state of {} at {} is now {}",
            if result["changed"] == true {
                "✓ changed"
            } else {
                "✗ unchanged"
            },
            result["block"],
            short(result["account"].as_str().unwrap_or("?")),
            short(result["program"].as_str().unwrap_or("?")),
            describe_bytes(&state),
        ));
    }
    if !output.ok {
        report_revert(ctx, log_offset, managed).await;
        bail!("the call didn't change the target state");
    }
    Ok(state)
}

/// Logs why a transaction reverted: the sequencer log lines written since `log_offset`. Only a
/// sequencer prog-tui started writes to a log it can read.
async fn report_revert(ctx: &mut Ctx, log_offset: u64, managed: bool) {
    if !managed {
        ctx.log("the revert reason is in the external sequencer's own log");
        return;
    }
    // Give the sequencer a moment to flush the block's log lines.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let log = sequencer::read_log_from(&ctx.state_dir, log_offset, 256 * 1024);
    let reasons = sequencer::revert_reasons(&log);
    if reasons.is_empty() {
        ctx.log("no revert reason found in the sequencer log");
    }
    for reason in reasons {
        ctx.log(format!("sequencer: {reason}"));
    }
}

// ---- Native token transfers ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferOutcome {
    /// The recipient got `amount`; the sender also paid `fee`.
    Moved { fee: u128 },
    /// Included and charged, but nothing was credited.
    Reverted { fee: u128 },
    /// Neither balance changed.
    Unchanged,
}

/// What a transfer did, judged from both balances before and after. The wallet says "included in
/// block" even when a transfer reverts, so this is the only way to tell.
pub fn transfer_outcome(
    sender: (u128, u128),
    recipient: (u128, u128),
    amount: u128,
) -> TransferOutcome {
    let (sender_before, sender_after) = sender;
    let (recipient_before, recipient_after) = recipient;
    let spent = sender_before.saturating_sub(sender_after);
    if recipient_after == recipient_before.saturating_add(amount) {
        TransferOutcome::Moved {
            fee: spent.saturating_sub(amount),
        }
    } else if spent > 0 {
        TransferOutcome::Reverted { fee: spent }
    } else {
        TransferOutcome::Unchanged
    }
}

/// A native transfer's serialized size, rounded up: 511 bytes measured, 544 when it also admits a
/// new recipient.
const TRANSFER_BYTES_ESTIMATE: u128 = 600;

/// The fee reserve the sequencer checks the payer for at submission (`fee_reserve` in
/// `lez/programs/fee/core/src/assess.rs`): `gas_limit·base_fee_exec + bytes·base_fee_stor + tip`,
/// with the wallet's tip of 0 and the byte count estimated.
pub fn fee_reserve_estimate(gas_limit: u64, base_fee_exec: u64, base_fee_stor: u64) -> u128 {
    u128::from(gas_limit) * u128::from(base_fee_exec)
        + TRANSFER_BYTES_ESTIMATE * u128::from(base_fee_stor)
}

async fn balance(rpc: &Rpc, id: &str) -> anyhow::Result<Option<u128>> {
    Ok(rpc.account(id).await?.map(|view| view.balance()))
}

/// Sends `amount` of the native token with `wallet auth-transfer send`, then checks both balances.
pub async fn transfer(ctx: &mut Ctx, from: &str, to: &str, amount: u128) -> anyhow::Result<()> {
    if from == to {
        bail!("from and to are the same account");
    }
    let accounts = list_accounts(&ctx.lez).await?;
    let in_wallet = |id: &str| accounts.iter().any(|a| a.public && a.id == id);
    if !in_wallet(from) {
        bail!("{} isn't a public account in this wallet, so it can't sign", short(from));
    }
    let sender_before = balance(&ctx.rpc, from).await?.unwrap_or(0);
    // The sender pays the fee, and the sequencer rejects a transaction whose payer can't cover
    // the fee reserve ("Incorrect fee"). The reserve is held before execution, so the amount
    // has to come on top of it.
    let (base_fee_exec, base_fee_stor) = ctx
        .rpc
        .base_fees()
        .await
        .ok_or_else(|| anyhow!("couldn't read the fee state (getFeeState)"))?;
    let reserve = fee_reserve_estimate(ctx.lez.wallet_gas_limit(), base_fee_exec, base_fee_stor);
    if sender_before < amount.saturating_add(reserve) {
        bail!(
            "{} holds {sender_before}; sending {amount} needs about {} ({amount} plus ~{reserve} \
             held for the fee, which the sender pays). Fund it first, e.g. from PAYER",
            short(from),
            amount.saturating_add(reserve)
        );
    }
    let recipient_before = balance(&ctx.rpc, to).await?;
    if recipient_before.is_none() && !in_wallet(to) {
        // The engine only runs an actor on an account that is on chain, signs, or comes with
        // evidence; the wallet can only give evidence for accounts it holds keys for.
        bail!(
            "{} isn't on chain and isn't in this wallet: an unused account needs its public key \
             (`wallet auth-transfer send --to-pk <pk>`)",
            short(to)
        );
    }
    let recipient_before = recipient_before.unwrap_or(0);

    let log_offset = sequencer::log_len(&ctx.state_dir);
    let managed = sequencer::managed_pgid(&ctx.state_dir).is_some();
    let amount_text = amount.to_string();
    let command = ctx.wallet(&[
        "auth-transfer",
        "send",
        "--from",
        &format!("Public/{from}"),
        "--to",
        &format!("Public/{to}"),
        "--amount",
        &amount_text,
    ]);
    ctx.run_ok(
        &format!("wallet auth-transfer send {amount} {} → {}", short(from), short(to)),
        command,
        None,
    )
    .await?;

    // The credit is an async message: give it up to a minute to land.
    let deadline = Instant::now() + Duration::from_mins(1);
    let (sender_after, recipient_after) = loop {
        let sender_after = balance(&ctx.rpc, from).await?.unwrap_or(0);
        let recipient_after = balance(&ctx.rpc, to).await?.unwrap_or(0);
        if recipient_after != recipient_before || Instant::now() > deadline {
            break (sender_after, recipient_after);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    };
    match transfer_outcome(
        (sender_before, sender_after),
        (recipient_before, recipient_after),
        amount,
    ) {
        TransferOutcome::Moved { fee } => {
            ctx.log(format!(
                "✓ moved {amount} from {} to {} (fee {fee}); {} now holds {recipient_after}",
                short(from),
                short(to),
                short(to)
            ));
            Ok(())
        }
        TransferOutcome::Reverted { fee } => {
            ctx.log(format!("✗ reverted: nothing credited, fee {fee} charged"));
            report_revert(ctx, log_offset, managed).await;
            bail!("the transfer reverted")
        }
        TransferOutcome::Unchanged => bail!("neither balance changed within a minute"),
    }
}

/// An account id as typed: trimmed, with an optional `Public/` prefix removed.
pub fn bare_account_id(text: &str) -> &str {
    let text = text.trim();
    text.strip_prefix("Public/").unwrap_or(text)
}

/// A positive whole amount.
pub fn parse_amount(text: &str) -> Option<u128> {
    text.trim().parse().ok().filter(|amount| *amount > 0)
}

/// The guests a runner deploys, in deploy order: the `.bin` arguments' names. The tail-call
/// runners take the caller's `.bin` first and deploy it first.
fn runner_guests(args: &[String]) -> Vec<String> {
    args.iter()
        .filter_map(|arg| Path::new(arg).file_name()?.to_str()?.strip_suffix(".bin"))
        .map(str::to_owned)
        .collect()
}

/// Runs a stock `run_*` runner, and registers the programs it deployed (it prints their ids).
pub async fn runner(ctx: &mut Ctx, args: &[String]) -> anyhow::Result<()> {
    let (name, rest) = args.split_first().ok_or_else(|| anyhow!("no runner"))?;
    let mut command = ctx.command(ctx.lez.release_bin(name));
    command.args(rest);
    let output = ctx.run_ok(&args.join(" "), command, None).await?;

    let guests = runner_guests(rest);
    let ids = parse_program_ids(&output);
    if guests.len() == ids.len() {
        for (guest, program_id) in guests.into_iter().zip(ids) {
            ctx.log(format!("deployed {guest} → {program_id}"));
            ctx.send(Msg::Deployed(Deployment {
                name: guest,
                program_id,
                stale: false,
            }));
        }
    } else {
        ctx.log(format!(
            "the runner printed {} program id(s) for {} guest(s): not registering them. Rebuild (key b) if the runner is older than prog_cli",
            ids.len(),
            guests.len()
        ));
    }
    wait_for_next_block(ctx).await;
    ctx.log("the runner doesn't check its call: look at the account's actor states");
    Ok(())
}

/// The stock runners send their last call and exit without waiting for it. A job that started
/// right after would read the payer's nonce before that call lands, and its first transaction
/// would fail with `Nonce mismatch`. Everything sent before now is in the next block.
async fn wait_for_next_block(ctx: &mut Ctx) {
    let Some(start) = ctx.rpc.last_block().await else {
        return;
    };
    ctx.log("waiting for the runner's last call to be included…");
    let deadline = Instant::now() + Duration::from_mins(1);
    while Instant::now() < deadline {
        if ctx.rpc.last_block().await.is_some_and(|block| block > start) {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    ctx.log("no new block within a minute: wait before the next job");
}

pub fn short(id: &str) -> String {
    if id.len() > 8 {
        format!("{}…", &id[..8])
    } else {
        id.to_owned()
    }
}

// ---- The walkthrough ----

pub const WALKTHROUGH: [&str; 6] = [
    "Build wallet, runners, prog_cli, guests",
    "Reset the chain, start the sequencer",
    "Initialise wallet, import test accounts",
    "Set PAYER, create ACCOUNT",
    "Deploy hello_world",
    "Call hello, check \"Hola mundo!\"",
];

macro_rules! step {
    ($ctx:expr, $index:expr, $body:expr) => {{
        $ctx.send(Msg::Step($index, StepState::Running));
        match $body {
            Ok(value) => {
                $ctx.send(Msg::Step($index, StepState::Done));
                value
            }
            Err(error) => {
                $ctx.send(Msg::Step($index, StepState::Failed));
                return Err(error);
            }
        }
    }};
}

/// WALKTHROUGH.md, steps 1–5, from a clean chain.
pub async fn walkthrough(ctx: &mut Ctx, stop: Option<StopTarget>) -> anyhow::Result<()> {
    ctx.send(Msg::Checklist(
        WALKTHROUGH.iter().map(|step| (*step).to_owned()).collect(),
    ));
    step!(ctx, 0, build(ctx).await);
    step!(ctx, 1, reset(ctx, stop).await);
    step!(ctx, 2, wallet_init(ctx).await);
    let (payer, account) = step!(ctx, 3, async {
        let payer = choose_payer(ctx).await?;
        ctx.send(Msg::SetPayer(payer.clone()));
        let account = new_account(ctx, "account").await?;
        ctx.send(Msg::SetAccount(account.clone()));
        ctx.log(format!("PAYER={payer} ACCOUNT={account}"));
        anyhow::Ok((payer, account))
    }
    .await);
    let greeting = "Hola mundo!";
    if ctx.lez.release_bin("prog_cli").is_file() {
        let deployment = step!(ctx, 4, deploy(ctx, "hello_world", &payer).await);
        step!(ctx, 5, async {
            let args = [
                "hello",
                "--program",
                &deployment.program_id,
                "--account",
                &account,
                "--text",
                greeting,
            ]
            .map(str::to_owned);
            let state = call(ctx, &payer, &args).await?;
            check_greeting(&state, greeting)
        }
        .await);
    } else {
        // Without prog_cli, the stock runner deploys and calls in one go.
        ctx.log("prog_cli isn't built: using run_hello_world, which deploys and calls");
        step!(ctx, 4, async {
            let bin = ctx.lez.guest_bin("hello_world").display().to_string();
            runner(ctx, &["run_hello_world".to_owned(), bin, account.clone(), payer.clone()]).await
        }
        .await);
        step!(ctx, 5, async {
            let view = ctx.rpc.account(&account).await?.unwrap_or_default();
            let state = view
                .actor_states
                .values()
                .find(|state| state.as_slice() == greeting.as_bytes())
                .cloned()
                .unwrap_or_default();
            check_greeting(&state, greeting)
        }
        .await);
    }
    Ok(())
}

fn check_greeting(state: &[u8], greeting: &str) -> anyhow::Result<()> {
    if state == greeting.as_bytes() {
        Ok(())
    } else {
        bail!(
            "expected {greeting:?}, found {}",
            describe_bytes(state)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_transfer_outcomes() {
        // The values from a real transfer of 1000: the sender lost 5088.
        assert_eq!(
            transfer_outcome((19_999_986_297_496, 19_999_986_292_408), (0, 1000), 1000),
            TransferOutcome::Moved { fee: 4088 }
        );
        assert_eq!(
            transfer_outcome((10_000, 5_912), (7, 7), 1000),
            TransferOutcome::Reverted { fee: 4088 }
        );
        assert_eq!(
            transfer_outcome((10_000, 10_000), (7, 7), 1000),
            TransferOutcome::Unchanged
        );
    }

    #[test]
    fn estimates_the_fee_reserve() {
        // The dev chain's opening state: both base fees 8, the wallet's default gas limit.
        assert_eq!(fee_reserve_estimate(2_000_000, 8, 8), 16_004_800);
        assert_eq!(fee_reserve_estimate(0, 8, 8), 4_800);
    }

    #[test]
    fn parses_transfer_inputs() {
        assert_eq!(bare_account_id(" Public/2RHZ "), "2RHZ");
        assert_eq!(bare_account_id("2RHZ"), "2RHZ");
        assert_eq!(parse_amount("1000"), Some(1000));
        assert_eq!(parse_amount(" 7 "), Some(7));
        assert_eq!(parse_amount("0"), None);
        assert_eq!(parse_amount("-5"), None);
        assert_eq!(parse_amount("ten"), None);
    }

    #[test]
    fn runner_guests_are_the_bin_arguments_in_order() {
        let args: Vec<String> = [
            "/lez/target/x/release/simple_tail_call.bin",
            "/lez/target/x/release/hello_world.bin",
            "8Ue4RiqZ",
            "2RHZhw9h",
        ]
        .map(str::to_owned)
        .to_vec();
        assert_eq!(runner_guests(&args), ["simple_tail_call", "hello_world"]);
        assert_eq!(
            runner_guests(&["/x/hello_world.bin".to_owned(), "ACC".to_owned()]),
            ["hello_world"]
        );
        assert!(runner_guests(&["ACC".to_owned()]).is_empty());
    }
}
