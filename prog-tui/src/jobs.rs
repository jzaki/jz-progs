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
    io::{AsyncBufReadExt as _, AsyncRead, BufReader},
    process::Command,
    sync::mpsc::{UnboundedSender, unbounded_channel},
};

use crate::{
    guest_bin,
    lez::{Lez, RepoInfo, repo_info},
    lez_call::{self, Seed},
    rpc::{AccountView, Rpc, describe_bytes, unhex},
    sequencer::{self, SeqStatus, StopTarget},
    state::Deployment,
    tools::{LEZ_CALL, LEZ_DRYRUN, Tool},
    userprogs,
    wallet::{
        WalletAccount, is_noise, parse_account_list, parse_json_line, parse_new_account,
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
    /// Show this account's state in the Accounts pane: `(id, note)`.
    Watch(String, String),
    /// A call's plan to show: its title and text, and the call, when it can be sent.
    Plan {
        title: String,
        lines: Vec<String>,
        call: Option<PlannedCall>,
    },
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

    /// A command in the LEZ repo, with the wallet home and password set and the wallet's
    /// transaction dumps suppressed.
    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.lez.dir)
            .env("LEE_WALLET_HOME_DIR", self.lez.wallet_home())
            .env("LEE_WALLET_PASSWORD", &self.password)
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

    /// A `lez-call` or `lez-dryrun` command, with `LEZ_WALLET` naming LEZ's wallet.
    pub fn tool(&self, tool: &Tool, args: &[String]) -> anyhow::Result<Command> {
        let path = tool
            .path()
            .ok_or_else(|| anyhow!("{} isn't built (key b) or on PATH", tool.name))?;
        let mut command = self.command(path);
        command
            .env("LEZ_WALLET", self.lez.release_bin("wallet"))
            .args(args);
        Ok(command)
    }

    /// Runs `command`, streaming its stdout and stderr into the log, and returns all of it.
    pub async fn run(
        &mut self,
        label: &str,
        command: Command,
    ) -> anyhow::Result<CmdOut> {
        self.run_inner(label, command, true).await
    }

    /// Like [`Self::run`], but into the job's log file only, not the log pane.
    pub async fn run_quiet(&mut self, label: &str, command: Command) -> anyhow::Result<CmdOut> {
        self.run_inner(label, command, false).await
    }

    /// Like [`Self::run`], but fails when the command does.
    pub async fn run_ok(
        &mut self,
        label: &str,
        command: Command,
    ) -> anyhow::Result<String> {
        let output = self.run(label, command).await?;
        if !output.ok {
            bail!("`{label}` failed");
        }
        Ok(output.out)
    }

    async fn run_inner(
        &mut self,
        label: &str,
        mut command: Command,
        show: bool,
    ) -> anyhow::Result<CmdOut> {
        self.write(format!("$ {label}"), show);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .with_context(|| format!("couldn't start `{label}`"))?;
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
pub async fn list_accounts(lez: &Lez, password: &str) -> anyhow::Result<Vec<WalletAccount>> {
    let output = Command::new(lez.release_bin("wallet"))
        .arg("account")
        .arg("list")
        .current_dir(&lez.dir)
        .env("LEE_WALLET_HOME_DIR", lez.wallet_home())
        .env("LEE_WALLET_PASSWORD", password)
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
    )
    .await?;
    for tool in [LEZ_CALL, LEZ_DRYRUN] {
        let checkout = tool.checkout();
        if !tool.has_checkout() {
            ctx.log(format!("no {} checkout at {}: not building it", tool.name, checkout.display()));
            continue;
        }
        let mut command = ctx.command("cargo");
        command.args(["build", "--release"]).current_dir(&checkout);
        ctx.run_ok(
            &format!("cargo build --release  (in {})", checkout.display()),
            command,
        )
        .await?;
    }
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
    let output = ctx.run("just clean", command).await?;
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
    // The wallet reads the new storage's password from `LEE_WALLET_PASSWORD`, set by `command`.
    let command = ctx.wallet(&["check-health"]);
    ctx.run_ok("wallet check-health", command).await?;
    let mut command = ctx.command("just");
    command.arg("wallet-import-test-accounts");
    ctx.run_ok("just wallet-import-test-accounts", command)
        .await?;
    Ok(())
}

pub async fn new_account(ctx: &mut Ctx, label: &str) -> anyhow::Result<String> {
    let mut args = vec!["account", "new", "public"];
    if !label.is_empty() {
        args.extend(["--label", label]);
    }
    let command = ctx.wallet(&args);
    let output = ctx.run_ok("wallet account new public", command).await?;
    parse_new_account(&output).ok_or_else(|| anyhow!("no account id in the wallet's output"))
}

/// The first imported account with a balance: one of the genesis test accounts.
pub async fn choose_payer(ctx: &mut Ctx) -> anyhow::Result<String> {
    let accounts = list_accounts(&ctx.lez, &ctx.password).await?;
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

/// Deploys a built guest through `program_loader`: a fresh header account and one fresh segment
/// account per chunk of its code, then `wallet program-loader deploy`, paid by `payer` (the new
/// accounts can't pay for their own claim). The header's id is the program's id.
pub async fn deploy(ctx: &mut Ctx, guest: &str, payer: &str) -> anyhow::Result<Deployment> {
    let bin = ctx.lez.guest_bin(guest);
    if !bin.is_file() {
        bail!("{} doesn't exist; build first (key b)", bin.display());
    }
    let segments = guest_bin::segment_count(&fs::read(&bin)?)
        .ok_or_else(|| anyhow!("{} isn't a risc0 program binary", bin.display()))?;
    let header = new_account(ctx, "").await?;
    let mut args = vec![
        "program-loader".to_owned(),
        "deploy".to_owned(),
        "--elf".to_owned(),
        bin.display().to_string(),
        "--header".to_owned(),
        format!("Public/{header}"),
        "--segments".to_owned(),
    ];
    for _ in 0..segments {
        args.push(format!("Public/{}", new_account(ctx, "").await?));
    }
    args.extend(["--payer".to_owned(), format!("Public/{payer}")]);
    let command = ctx.wallet(&args.iter().map(String::as_str).collect::<Vec<_>>());
    let output = ctx
        .run_ok(&format!("wallet program-loader deploy {guest}"), command)
        .await?;
    if !output.contains(&format!("Header account id: {header}")) {
        bail!("the wallet didn't report deploying {guest} at {header}");
    }
    let program_id = header;
    ctx.log(format!("deployed {guest} → {program_id}"));
    let deployment = Deployment {
        name: guest.to_owned(),
        program_id,
        stale: false,
    };
    ctx.send(Msg::Deployed(deployment.clone()));
    Ok(deployment)
}

/// One call to make, as the form gives it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CallRequest {
    pub payer: String,
    pub program_id: String,
    /// The program's name, for reading.
    pub name: String,
    /// The guest's source, for `lez-call`.
    pub source: PathBuf,
    pub to: String,
    /// `Some(true)`: `to` signs; `Some(false)`: it never does; `None`: the dry run finds out.
    pub sign: Option<bool>,
    /// The source hints that the message is signed.
    pub hinted_signed: bool,
    pub actors: Vec<String>,
    /// Seeds to try for a new PDA, from the form.
    pub pda_seeds: Vec<String>,
    /// The source's `PdaSeed` constants.
    pub seeds: Vec<Seed>,
    /// The message's name; `None` when the program takes one message type.
    pub method: Option<String>,
    /// `name=value` for each param.
    pub values: Vec<String>,
}

/// A call ready to send: the wallet command, and what its dry run says it changes.
#[derive(Clone, Debug)]
pub struct PlannedCall {
    pub name: String,
    /// `program call …`: the words after `wallet`.
    pub wallet_args: Vec<String>,
    /// Each actor state the dry run changed, as `(account, program, after)`. Empty without a
    /// dry run.
    pub changes: Vec<(String, String, Vec<u8>)>,
    /// The PDAs the call declares, to watch.
    pub pdas: Vec<String>,
}

/// What `prepare` found: the plan's text, and the call when it can be sent.
pub struct Prepared {
    pub lines: Vec<String>,
    pub call: Option<PlannedCall>,
}

/// Encodes the message with `lez-call`, then plans the call with `lez-dryrun` against the chain:
/// its actors, signers and evidence, and what it changes. Without `lez-dryrun`, the call is sent
/// as given, with `to` signing when asked or hinted.
pub async fn prepare(ctx: &mut Ctx, request: &CallRequest) -> anyhow::Result<Prepared> {
    let accounts = list_accounts(&ctx.lez, &ctx.password).await?;
    let public: Vec<&WalletAccount> = accounts.iter().filter(|account| account.public).collect();
    let labels: Vec<String> = public
        .iter()
        .filter_map(|account| Some(format!("{}={}", account.label.as_ref()?, account.id)))
        .collect();

    // `lez-call call --print` encodes the message and gives the wallet command for the call as
    // given: what's sent when there's no dry run.
    let signs_unplanned = request.sign == Some(true)
        || (request.sign.is_none() && request.hinted_signed && !request.to.starts_with("pda:"));
    let to = if signs_unplanned && !request.to.ends_with(":sign") {
        format!("{}:sign", request.to)
    } else {
        request.to.clone()
    };
    let mut args = vec![
        "--json".to_owned(),
        "call".to_owned(),
        request.source.display().to_string(),
        "--print".to_owned(),
        "--program".to_owned(),
        request.program_id.clone(),
        "--to".to_owned(),
        to,
        "--payer".to_owned(),
        format!("Public/{}", request.payer),
    ];
    for actor in &request.actors {
        args.extend(["--actor".to_owned(), actor.clone()]);
    }
    for label in &labels {
        args.extend(["--label".to_owned(), label.clone()]);
    }
    args.extend(request.method.iter().cloned());
    args.extend(request.values.iter().cloned());
    let command = ctx.tool(&LEZ_CALL, &args)?;
    let output = ctx.run_quiet(&format!("lez-call {}", args.join(" ")), command).await?;
    let encoded = parse_json_line(&output.out, "LEZCALL").filter(|_| output.ok).ok_or_else(|| {
        let error = output.out.lines().rev().find(|line| line.starts_with("Error"));
        anyhow!("{}", error.unwrap_or("lez-call couldn't encode the message"))
    })?;
    let message_hex = encoded["message_hex"].as_str().unwrap_or_default().to_owned();
    let strings = |value: &serde_json::Value| -> Vec<String> {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|item| Some(item.as_str()?.to_owned()))
            .collect()
    };

    if LEZ_DRYRUN.path().is_none() {
        let command = encoded["command"].as_str().unwrap_or_default().to_owned();
        let lines = vec![
            format!("message {message_hex}"),
            String::new(),
            "no dry run: lez-dryrun isn't built (key b), so this is sent as given, unchecked".to_owned(),
            String::new(),
            "same call by hand:".to_owned(),
            format!("  {command}"),
        ];
        let call = PlannedCall {
            name: request.name.clone(),
            wallet_args: strings(&encoded["wallet_args"]),
            changes: Vec::new(),
            pdas: Vec::new(),
        };
        return Ok(Prepared {
            lines,
            call: Some(call),
        });
    }

    let mut args = vec![
        "--json".to_owned(),
        "--rpc".to_owned(),
        ctx.rpc.url.clone(),
        "plan".to_owned(),
        "--payer".to_owned(),
        request.payer.clone(),
        "--program".to_owned(),
        request.program_id.clone(),
        "--program-name".to_owned(),
        request.name.clone(),
        "--message-hex".to_owned(),
        message_hex,
        "--to".to_owned(),
        request.to.clone(),
        "--budget".to_owned(),
        ctx.lez.wallet_gas_limit().to_string(),
    ];
    let mut flag = |name: &str, value: String| args.extend([name.to_owned(), value]);
    for actor in &request.actors {
        flag("--actor", actor.clone());
    }
    for account in &public {
        flag("--signer", account.id.clone());
    }
    for label in &labels {
        flag("--label", label.clone());
    }
    for seed in &request.seeds {
        flag("--seed", format!("{}={}", seed.name, seed.bytes));
    }
    for seed in &request.pda_seeds {
        flag("--seed", seed.clone());
    }
    for mentioned in strings(&encoded["mentioned"]) {
        flag("--mention", mentioned);
    }
    match request.sign {
        Some(true) => args.push("--sign".to_owned()),
        Some(false) => args.push("--no-sign".to_owned()),
        None => {}
    }
    if request.hinted_signed {
        args.push("--hinted-signed".to_owned());
    }
    let command = ctx.tool(&LEZ_DRYRUN, &args)?;
    let output = ctx.run_quiet(&format!("lez-dryrun {}", args.join(" ")), command).await?;
    let Some(result) = parse_json_line(&output.out, "LEZDRYRUN") else {
        let error = output.out.lines().rev().find(|line| line.starts_with("Error"));
        bail!("{}", error.unwrap_or("lez-dryrun printed no plan"));
    };
    let plan = &result["plan"];
    let mut lines = strings(&result["lines"]);
    if lines.is_empty() {
        // A lez-dryrun built before its JSON carried the text: say so, rather than show nothing.
        lines = vec![
            "lez-dryrun printed no plan text: rebuild it (key b)".to_owned(),
            String::new(),
            plan["error"].as_str().map_or_else(
                || "the dry run worked".to_owned(),
                |error| format!("✗ {error}"),
            ),
            String::new(),
            "same call by hand:".to_owned(),
            format!("  {}", plan["command"].as_str().unwrap_or_default()),
        ];
    }
    if plan["ok"] != true {
        return Ok(Prepared { lines, call: None });
    }
    let changes = plan["changes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|change| {
            Some((
                change["account"].as_str()?.to_owned(),
                change["program"].as_str()?.to_owned(),
                unhex(change["after"].as_str()?)?,
            ))
        })
        .collect();
    let pdas = plan["actors"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|actor| actor["admission"] == "pda")
        .filter_map(|actor| Some(actor["account"].as_str()?.to_owned()))
        .collect();
    let call = PlannedCall {
        name: request.name.clone(),
        wallet_args: strings(&plan["wallet_args"]),
        changes,
        pdas,
    };
    Ok(Prepared {
        lines,
        call: Some(call),
    })
}

/// Plans `request`; the plan goes to the UI, to confirm before [`call`] sends it.
pub async fn plan(ctx: &mut Ctx, title: String, request: CallRequest) -> anyhow::Result<()> {
    let prepared = prepare(ctx, &request).await?;
    let ok = prepared.call.is_some();
    ctx.send(Msg::Plan {
        title,
        lines: prepared.lines,
        call: prepared.call,
    });
    if !ok {
        bail!("the dry run failed: nothing was sent");
    }
    Ok(())
}

/// Sends a planned call with `wallet program call`, then checks each state its dry run said it
/// changes against the chain. Watches the PDAs it declared, as `PDA of <name>`. On a revert,
/// shows why, from the sequencer's log.
pub async fn call(ctx: &mut Ctx, call: &PlannedCall) -> anyhow::Result<()> {
    let log_offset = sequencer::log_len(&ctx.state_dir);
    let managed = sequencer::managed_pgid(&ctx.state_dir).is_some();
    let command = ctx.wallet(&call.wallet_args.iter().map(String::as_str).collect::<Vec<_>>());
    ctx.run_ok(&format!("wallet program call ({})", call.name), command)
        .await?;
    for pda in &call.pdas {
        ctx.send(Msg::Watch(pda.clone(), format!("PDA of {}", call.name)));
    }
    if call.changes.is_empty() {
        ctx.log("no dry run said what this changes, so there's nothing to check");
        return Ok(());
    }
    let mut matched = true;
    for (account, program, after) in &call.changes {
        let actual = ctx
            .rpc
            .account(account)
            .await?
            .and_then(|view| view.actor_states.get(program).cloned())
            .unwrap_or_default();
        let ok = actual == *after;
        matched &= ok;
        ctx.log(format!(
            "{} {}@{}: now {}{}",
            if ok { "✓" } else { "✗" },
            short(account),
            short(program),
            describe_bytes(&actual),
            if ok {
                " (as the dry run said)".to_owned()
            } else {
                format!(": expected {}", describe_bytes(after))
            }
        ));
    }
    if !matched {
        report_revert(ctx, log_offset, managed).await;
        bail!("the chain doesn't match the dry run: the call probably reverted");
    }
    Ok(())
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
    let accounts = list_accounts(&ctx.lez, &ctx.password).await?;
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
    let output = ctx.run_ok(&args.join(" "), command).await?;

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
            "the runner printed {} program id(s) for {} guest(s): not registering them. Rebuild (key b) if the runner is older than its PROG_CLI lines",
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
    "Build wallet, runners, guests, lez-call",
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
    if LEZ_CALL.path().is_some() {
        let deployment = step!(ctx, 4, deploy(ctx, "hello_world", &payer).await);
        step!(ctx, 5, async {
            let program = deployment.program_id.clone();
            let request = CallRequest {
                payer: payer.clone(),
                program_id: program.clone(),
                name: deployment.name.clone(),
                source: lez_call::source_of(&ctx.lez, "hello_world"),
                to: account.clone(),
                values: vec![format!("#0={greeting}")],
                ..CallRequest::default()
            };
            let prepared = prepare(ctx, &request).await?;
            for line in &prepared.lines {
                ctx.log(line.clone());
            }
            let planned = prepared
                .call
                .ok_or_else(|| anyhow!("the dry run failed: nothing was sent"))?;
            call(ctx, &planned).await?;
            let state = ctx
                .rpc
                .account(&account)
                .await?
                .and_then(|view| view.actor_states.get(&program).cloned())
                .unwrap_or_default();
            check_greeting(&state, greeting)
        }
        .await);
    } else {
        // Without lez-call, the stock runner deploys and calls in one go.
        ctx.log("lez-call isn't built: using run_hello_world, which deploys and calls");
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
