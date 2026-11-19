//! The L1 view's data: the Logos blockchain node run by the `logosctl` daemon (module
//! `blockchain_module`), and the wallets its keystore holds. Everything comes from running
//! `logosctl --json …`; nothing here starts, stops or changes the node.
//!
//! logosctl can't say which config the node was started with, so that path is a prog-tui setting
//! (`Saved::l1_config`). Its directory's `keystore.yaml` names the wallet addresses: only its
//! `public_keys:` block is read.
//!
//! Besides reading, jobs act: starting or stopping the node (daemon, module, node), bridging L1
//! funds into a LEZ account, L1 transfers, switching mining and auto-claim on the running node, and
//! rewriting the config's `pow:` section, which the node reads when it starts.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context as _, anyhow, bail};
use serde_json::Value;
use tokio::process::Command;

use crate::jobs::{self, Ctx, Msg, Planned};

/// The module that runs the node.
pub const MODULE: &str = "blockchain_module";

/// How long prog-tui waits for one logosctl command. A slower one is left to finish on its own:
/// killing a client mid-call can crash the daemon (it segfaulted in libQt6RemoteObjects when its
/// clients were killed, logosctl 1.0.0), so prog-tui never kills one.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A logosctl client: in a session of its own, so a terminal hang-up or Ctrl-C in prog-tui never
/// reaches it, and never killed, not even when prog-tui stops waiting for it.
fn client(program: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(program);
    command
        .arg("--json")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(false);
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

/// What the daemon reported about itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Daemon {
    pub status: String,
    pub pid: Option<u64>,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModuleRow {
    pub name: String,
    pub status: String,
    pub version: String,
    pub uptime_seconds: Option<u64>,
    pub cpu_percent: Option<f64>,
    pub memory_mb: Option<f64>,
}

/// The node's own reports. Each is one call, kept apart so one failing doesn't hide the rest.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    /// `get_cryptarchia_info`: `height`, `slot`, `lib_slot`, `lib`, `tip`, `mode`.
    pub chain: Option<Value>,
    /// `get_network_info`: `n_peers`, `n_connections`, …
    pub network: Option<Value>,
    /// `get_time_info`: `current_epoch`, `current_slot`, `genesis_time`, …
    pub time: Option<Value>,
    /// `pow_status`: `is_mining`, `are_rewards_enabled`, `auto_claim`.
    pub pow: Option<Value>,
    /// `wallet_get_claimable_vouchers`: `total_claimable`, `reward_amount`, `vouchers`.
    pub vouchers: Option<Value>,
    /// `pow_claimable_rewards`: `claimable_tickets`, `slots_until_expiry`.
    pub rewards: Option<Value>,
    /// The first error among those calls, to show when they all fail.
    pub error: Option<String>,
}

/// The node's state as its log tells it: prog-tui reads this before calling the node at all,
/// since a node that isn't `Online` yet doesn't answer calls (each fails after the daemon's 20s
/// timeout), and calls piling up on it have crashed the daemon.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodeLog {
    /// The latest `entering <phase> phase`, e.g. `ProlongedBootstrapPeriod`.
    pub phase: Option<String>,
    /// When it entered that phase, as logged (`2026-11-19 09:33:04`).
    pub since: Option<String>,
    /// `Chain is now Online.` came after the node last started.
    pub online: bool,
    /// The node gave up and shut itself down (its initial block download failed), with why.
    pub shut_down: Option<String>,
    /// The slot of the latest block it received.
    pub slot: Option<u64>,
    /// When it received that block.
    pub slot_at: Option<String>,
}

impl NodeLog {
    /// The node was started in this daemon and hasn't shut itself down since.
    pub fn started(&self) -> bool {
        self.phase.is_some() && self.shut_down.is_none()
    }

    /// When `ProlongedBootstrapPeriod` should end: it lasts an hour (as observed: 20:02:07 →
    /// 21:02:12, 10:24:14 → 11:24:14).
    pub fn online_expected(&self) -> Option<String> {
        if self.online || self.phase.as_deref() != Some("ProlongedBootstrapPeriod") {
            return None;
        }
        let since = self.since.as_deref()?;
        let (date, time) = since.split_once(' ')?;
        let mut parts = time.split(':').map(|part| part.parse::<u32>().ok());
        let (h, m) = (parts.next()??, parts.next()??);
        let _ = date;
        Some(format!("{:02}:{m:02}", (h + 1) % 24))
    }
}

/// `[2026-11-19 09:33:04.709] …` → `2026-11-19 09:33:04`.
fn log_time(line: &str) -> Option<String> {
    let stamp = line.strip_prefix('[')?.split(']').next()?;
    Some(stamp.split('.').next()?.to_owned())
}

/// Reads the node's phase from the daemon's log (one file per daemon start). The node logs
/// through the daemon, so this needs no call to it.
pub fn parse_node_log(text: &str) -> NodeLog {
    let mut log = NodeLog::default();
    for line in text.lines() {
        if !line.contains("[blockchain_module]") {
            continue;
        }
        if let Some(rest) = line.split("entering ").nth(1)
            && let Some(phase) = rest.strip_suffix(" phase").or_else(|| rest.split(" phase").next())
            && !phase.contains(' ')
        {
            if phase == "AwaitingGenesisTime" {
                // A (re)start: what came before belongs to the last run.
                log = NodeLog::default();
            }
            log.phase = Some(phase.to_owned());
            log.since = log_time(line);
            if phase == "Following" {
                log.online = true;
            }
        } else if line.contains("Chain is now Online") {
            log.online = true;
        } else if line.contains("Initiating graceful shutdown") {
            let why = line
                .split("ERROR")
                .nth(1)
                .and_then(|rest| rest.split(": ").nth(1))
                .unwrap_or("it shut itself down")
                .trim();
            log.shut_down = Some(why.to_owned());
            log.online = false;
        } else if line.contains("Received new block")
            && let Some(slot) = line.split("\"slot\":").nth(1)
        {
            let digits: String = slot.chars().take_while(char::is_ascii_digit).collect();
            log.slot = digits.parse().ok();
            log.slot_at = log_time(line);
        }
    }
    log
}

/// The current daemon's log: `<session>/logs/daemon.log`, a link to this start's file.
pub fn read_node_log() -> Option<NodeLog> {
    let text = std::fs::read(session_dir().join("logs/daemon.log")).ok()?;
    Some(parse_node_log(&String::from_utf8_lossy(&text)))
}

#[derive(Clone, Debug, PartialEq)]
pub struct L1Wallet {
    /// Hex, as `wallet_get_known_addresses` lists it.
    pub address: String,
    /// The key's name in the keystore's `public_keys`, e.g. `PoWClaim`.
    pub name: Option<String>,
    /// The balance as the node reports it (a whole number), or why there's none. The node answers
    /// `Unknown wallet address.` for the keystore's keys that don't hold funds.
    pub balance: Result<String, String>,
}

impl L1Wallet {
    /// A key the wallet doesn't track funds for (blend, network, stake keys …).
    pub fn holds_no_funds(&self) -> bool {
        matches!(&self.balance, Err(error) if error.contains("Unknown wallet address"))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct L1Status {
    /// `logosctl` on PATH; `None` and nothing else is tried.
    pub logosctl: Option<PathBuf>,
    /// The session directory logosctl uses: `$LOGOSCTL_CONFIG_DIR`, else `~/.logosctl`.
    pub config_dir: PathBuf,
    /// The daemon, or why it couldn't be asked.
    pub daemon: Option<Result<Daemon, String>>,
    pub modules: Vec<ModuleRow>,
    /// Set when `blockchain_module` is loaded.
    pub node: Option<Node>,
    /// The node's phase, from the daemon's log.
    pub node_log: Option<NodeLog>,
    pub wallets: Vec<L1Wallet>,
    /// Why the wallets couldn't be listed.
    pub wallets_error: Option<String>,
    /// The config prog-tui was told the node runs with, if it exists.
    pub config: Option<PathBuf>,
    /// That config's `pow:` section, which the node reads when it starts.
    pub pow_config: Option<Result<PowSettings, String>>,
    /// Set once a refresh has come back.
    pub checked: bool,
}

impl L1Status {
    /// The daemon answered and says it's running.
    pub fn running(&self) -> bool {
        matches!(&self.daemon, Some(Ok(daemon)) if daemon.status == "running")
    }

    pub fn module_loaded(&self) -> bool {
        self.modules
            .iter()
            .any(|module| module.name == MODULE && module.status == "loaded")
    }

    /// The balances that are numbers, added up.
    pub fn total(&self) -> Option<u128> {
        let mut total = None;
        for wallet in &self.wallets {
            if let Ok(balance) = &wallet.balance
                && let Ok(amount) = balance.parse::<u128>()
            {
                total = Some(total.unwrap_or(0) + amount);
            }
        }
        total
    }
}

pub fn logosctl_path() -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join("logosctl"))
        .find(|path| path.is_file())
}

pub fn session_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("LOGOSCTL_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    std::env::var_os("HOME")
        .map_or_else(|| PathBuf::from("~"), PathBuf::from)
        .join(".logosctl")
}

/// The node config Basecamp's blockchain UI last used (`userConfigPath` in
/// `~/.config/Logos/BlockchainUI.conf`), as a default for `Saved::l1_config`.
pub fn basecamp_config() -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let text = std::fs::read_to_string(Path::new(&home).join(".config/Logos/BlockchainUI.conf")).ok()?;
    text.lines()
        .find_map(|line| line.trim().strip_prefix("userConfigPath="))
        .map(|path| path.trim().to_owned())
        .filter(|path| !path.is_empty())
}

/// Runs `logosctl --json <args>` and parses what it prints. A non-zero exit with a JSON error
/// (`{"status":"error","message":…}`) comes back as that message.
async fn logosctl(program: &Path, args: &[&str]) -> Result<Value, String> {
    let output = tokio::time::timeout(TIMEOUT, client(program, args).output())
        .await
        .map_err(|_| format!("logosctl {} took over {}s", args.join(" "), TIMEOUT.as_secs()))?
        .map_err(|error| format!("couldn't run logosctl: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    match serde_json::from_str::<Value>(stdout.trim()) {
        Ok(value) => match top_level_error(&value) {
            Some(error) => Err(error),
            None => Ok(value),
        },
        Err(_) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let text = if stderr.trim().is_empty() { stdout.trim() } else { stderr.trim() };
            Err(first_line(text).unwrap_or("no output").to_owned())
        }
    }
}

fn first_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

/// logosctl's own error object: `{"code":…,"message":…,"status":"error"}`.
fn top_level_error(value: &Value) -> Option<String> {
    if value.get("status").and_then(Value::as_str) != Some("error") {
        return None;
    }
    Some(
        value
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| value.get("code").and_then(Value::as_str))
            .unwrap_or("logosctl reported an error")
            .to_owned(),
    )
}

/// A module method's result: `{"result":{"success","error","value"}}`. A `value` that is a string
/// of JSON (most of the node's reports) is parsed; any other string stays a string.
pub fn call_value(reply: &Value) -> Result<Value, String> {
    let result = reply.get("result").ok_or("no result in logosctl's reply")?;
    let success = result.get("success").and_then(Value::as_bool).unwrap_or(false);
    if !success {
        return Err(result
            .get("error")
            .and_then(Value::as_str)
            .filter(|error| !error.is_empty())
            .unwrap_or("the call failed, with no reason given")
            .to_owned());
    }
    let value = result.get("value").cloned().unwrap_or(Value::Null);
    Ok(match &value {
        Value::String(text) => serde_json::from_str(text).unwrap_or(value),
        _ => value,
    })
}

/// A text argument for `logosctl call`: `str:<text>`. logosctl otherwise guesses types, so `1` (or a
/// hex string that happens to be all digits) would go as a number, and the module, whose
/// parameters here are all text, fails it with no reason given.
pub fn text_arg(text: &str) -> String {
    format!("str:{text}")
}

async fn call(program: &Path, method: &str, args: &[&str]) -> Result<Value, String> {
    let texts: Vec<String> = args.iter().map(|arg| text_arg(arg)).collect();
    let mut all = vec!["call", MODULE, method];
    all.extend(texts.iter().map(String::as_str));
    call_value(&logosctl(program, &all).await?)
}

fn parse_status(value: &Value) -> (Daemon, Vec<ModuleRow>) {
    let daemon = value.get("daemon").cloned().unwrap_or(Value::Null);
    let daemon = Daemon {
        status: daemon
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_owned(),
        pid: daemon.get("pid").and_then(Value::as_u64),
        version: daemon.get("version").and_then(Value::as_str).map(str::to_owned),
    };
    let mut modules: Vec<ModuleRow> = value
        .get("modules")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|module| ModuleRow {
            name: text(module, "name"),
            status: text(module, "status"),
            version: text(module, "version"),
            uptime_seconds: module.get("uptime_seconds").and_then(Value::as_u64),
            cpu_percent: None,
            memory_mb: None,
        })
        .collect();
    // The node first, then the rest by name.
    modules.sort_by(|a, b| (a.name != MODULE, &a.name).cmp(&(b.name != MODULE, &b.name)));
    (daemon, modules)
}

/// Adds `module stats`' `cpu_percent` and `memory_mb` to the rows.
fn merge_stats(modules: &mut [ModuleRow], stats: &Value) {
    for stat in stats.as_array().into_iter().flatten() {
        let name = text(stat, "name");
        if let Some(module) = modules.iter_mut().find(|module| module.name == name) {
            module.cpu_percent = stat.get("cpu_percent").and_then(Value::as_f64);
            module.memory_mb = stat.get("memory_mb").and_then(Value::as_f64);
        }
    }
}

fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
}

/// Hex → name from a keystore's `public_keys:` block:
///
/// ```yaml
/// public_keys:
///   PoWClaim: 82da…
/// secret_keys:
///   …
/// ```
///
/// It stops at the next top-level key, so nothing after `public_keys` (the secret keys) is kept.
pub fn keystore_names(keystore: &str) -> BTreeMap<String, String> {
    let mut names = BTreeMap::new();
    let mut inside = false;
    for line in keystore.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let top_level = !line.starts_with([' ', '\t']);
        if top_level {
            if inside {
                break;
            }
            inside = line.trim_end() == "public_keys:";
            continue;
        }
        if inside && let Some((name, hex)) = line.trim().split_once(':') {
            let hex = hex.trim().trim_matches(['"', '\'']).to_ascii_lowercase();
            if !hex.is_empty() {
                names.insert(hex, name.trim().to_owned());
            }
        }
    }
    names
}

pub fn names_beside(config: Option<&Path>) -> BTreeMap<String, String> {
    config
        .and_then(Path::parent)
        .and_then(|dir| std::fs::read_to_string(dir.join("keystore.yaml")).ok())
        .map(|text| keystore_names(&text))
        .unwrap_or_default()
}

/// Asks logosctl about the daemon, the node and its wallets, one call at a time. With `full` false,
/// only the daemon and its modules (`logosctl status`): no call reaches the node.
///
/// The node is asked `get_cryptarchia_info` first, and nothing more when that fails: a node that is
/// still syncing answers slowly, and logosctl 1.0.0's daemon has segfaulted while calls to a
/// syncing node were pending, so prog-tui keeps them to one.
pub async fn gather(config: Option<PathBuf>, full: bool) -> L1Status {
    let mut status = L1Status {
        logosctl: logosctl_path(),
        config_dir: session_dir(),
        config: config.filter(|path| path.is_file()),
        checked: true,
        ..L1Status::default()
    };
    status.pow_config = status.config.as_deref().map(read_pow_config);
    let Some(program) = status.logosctl.clone() else {
        return status;
    };
    match logosctl(&program, &["status"]).await {
        Ok(value) => {
            let (daemon, modules) = parse_status(&value);
            status.daemon = Some(Ok(daemon));
            status.modules = modules;
        }
        Err(error) => {
            status.daemon = Some(Err(error));
            return status;
        }
    }
    if !status.running() {
        return status;
    }
    status.node_log = tokio::task::spawn_blocking(read_node_log).await.ok().flatten();
    if !full {
        return status;
    }
    // Only an Online node answers; until the log says so, nothing is asked of it.
    let online = status.node_log.as_ref().is_some_and(|log| log.online);
    if !online {
        return status;
    }

    // One call at a time: concurrent calls slow a bootstrapping node down to timeouts.
    if let Ok(stats) = logosctl(&program, &["module", "stats"]).await {
        merge_stats(&mut status.modules, &stats);
    }
    let mut node = None;
    let mut addresses = None;
    if status.module_loaded() {
        let mut found = Node::default();
        match call(&program, "get_cryptarchia_info", &[]).await {
            Ok(chain) => found.chain = Some(chain),
            Err(error) => {
                found.error = Some(format!("get_cryptarchia_info: {error}"));
                status.node = Some(found);
                return status;
            }
        }
        for method in [
            "get_network_info",
            "get_time_info",
            "pow_status",
            "pow_claimable_rewards",
            "wallet_get_claimable_vouchers",
        ] {
            let result = call(&program, method, &[]).await;
            let slot = match method {
                "get_network_info" => &mut found.network,
                "get_time_info" => &mut found.time,
                "pow_status" => &mut found.pow,
                "pow_claimable_rewards" => &mut found.rewards,
                _ => &mut found.vouchers,
            };
            match result {
                Ok(value) => *slot = Some(value),
                Err(error) => {
                    found.error.get_or_insert(format!("{method}: {error}"));
                }
            }
        }
        node = Some(found);
        addresses = Some(call(&program, "wallet_get_known_addresses", &[]).await);
    }
    status.node = node;

    match addresses {
        None => {}
        Some(Err(error)) => status.wallets_error = Some(error),
        Some(Ok(list)) => {
            let names = names_beside(status.config.as_deref());
            let addresses: Vec<String> = list
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            let mut found = Vec::with_capacity(addresses.len());
            for address in &addresses {
                found.push(call(&program, "wallet_get_balance", &[address]).await.map(|value| {
                    match value {
                        Value::String(text) => text,
                        other => other.to_string(),
                    }
                }));
            }
            status.wallets = addresses
                .into_iter()
                .zip(found)
                .map(|(address, balance)| L1Wallet {
                    name: names.get(&address.to_ascii_lowercase()).cloned(),
                    address,
                    balance,
                })
                .collect();
        }
    }
    status
}

// ---- Starting and stopping ----

/// The node is up: the module answers `get_cryptarchia_info`.
async fn node_started(program: &Path) -> bool {
    call(program, "get_cryptarchia_info", &[]).await.is_ok()
}

async fn daemon_running(program: &Path) -> bool {
    logosctl(program, &["status"])
        .await
        .is_ok_and(|value| parse_status(&value).0.status == "running")
}

async fn module_loaded(program: &Path) -> bool {
    logosctl(program, &["status"]).await.is_ok_and(|value| {
        parse_status(&value)
            .1
            .iter()
            .any(|module| module.name == MODULE && module.status == "loaded")
    })
}

/// A job's logosctl command: shown as `$ label`, its output in the job's log. Like every
/// logosctl client here it's never killed (see [`TIMEOUT`]); after `wait` prog-tui stops waiting.
/// Returns whether it succeeded, and its stdout.
async fn run(
    ctx: &mut Ctx,
    label: &str,
    program: &Path,
    args: &[&str],
    wait: Duration,
) -> anyhow::Result<(bool, String)> {
    ctx.log(format!("$ {label}"));
    let output = tokio::time::timeout(wait, client(program, args).output())
        .await
        .map_err(|_| anyhow!("`{label}` took over {}s (left running)", wait.as_secs()))?
        .with_context(|| format!("couldn't run `{label}`"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    for line in stdout.lines().chain(String::from_utf8_lossy(&output.stderr).lines()) {
        if !line.trim().is_empty() {
            ctx.log(line.to_owned());
        }
    }
    Ok((output.status.success(), stdout))
}

/// Starts what isn't running yet: the logosctl daemon (detached), then `blockchain_module`, then
/// the node with `config` and the deployment the module ships with.
pub async fn start(ctx: &mut Ctx, config: Option<PathBuf>) -> anyhow::Result<()> {
    let program = logosctl_path().ok_or_else(|| anyhow!("logosctl isn't on PATH"))?;
    if daemon_running(&program).await {
        ctx.log("L1: the logosctl daemon is already running");
    } else {
        // The detached daemon keeps whatever it inherits open, so its output goes to a file:
        // a pipe would never close, and the job would never end.
        let log = ctx.state_dir.join("logs").join("l1-daemon.log");
        let file = std::fs::File::create(&log).context("creating the daemon's log")?;
        let mut start = client(&program, &["daemon", "start", "--detach"]);
        start.stdout(file.try_clone()?).stderr(file);
        ctx.log(format!("$ logosctl daemon start --detach   (output in {})", log.display()));
        let status = tokio::time::timeout(Duration::from_secs(60), start.status())
            .await
            .map_err(|_| anyhow!("logosctl daemon start --detach took over 60s"))??;
        if !status.success() {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            bail!("logosctl daemon start failed ({status}): {}", first_line(&text).unwrap_or(""));
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while !daemon_running(&program).await {
            if tokio::time::Instant::now() > deadline {
                bail!("the daemon started but doesn't answer `logosctl status`");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        ctx.log("✓ L1: daemon running");
    }

    if module_loaded(&program).await {
        ctx.log(format!("L1: {MODULE} is already loaded"));
    } else {
        let label = format!("logosctl module load {MODULE}");
        let (ok, _) = run(ctx, &label, &program, &["module", "load", MODULE], TIMEOUT).await?;
        if !ok || !module_loaded(&program).await {
            bail!(
                "{MODULE} didn't load. Is it installed in this session? \
                 logosctl package install ./blockchain_module-<version>.lgx"
            );
        }
    }

    // The log says whether the node was started; asking the node fails while it bootstraps.
    match read_node_log() {
        Some(log) if log.started() => {
            ctx.log(format!(
                "L1: the node is already running (phase {}{})",
                log.phase.as_deref().unwrap_or("?"),
                log.online_expected().map(|at| format!(", Online about {at}")).unwrap_or_default()
            ));
            return Ok(());
        }
        Some(log) if log.shut_down.is_some() => {
            ctx.log(format!(
                "L1: the node had shut itself down ({}); starting it again",
                log.shut_down.as_deref().unwrap_or("")
            ));
        }
        _ if node_started(&program).await => {
            ctx.log("L1: the node is already running");
            return Ok(());
        }
        _ => {}
    }
    let config = config
        .filter(|path| path.is_file())
        .ok_or_else(|| anyhow!("no L1 config file: set it with C on the L1 view"))?;
    let config_text = config.display().to_string();
    let (config_arg, deployment) = (text_arg(&config_text), text_arg(""));
    let label = format!("logosctl call {MODULE} start {config_arg} {deployment}");
    let args = ["call", MODULE, "start", config_arg.as_str(), deployment.as_str()];
    let (_, out) = run(ctx, &label, &program, &args, Duration::from_secs(120)).await?;
    let reply: Value = serde_json::from_str(out.trim())
        .map_err(|_| anyhow!("logosctl's reply isn't JSON: {}", first_line(&out).unwrap_or("")))?;
    if let Some(error) = top_level_error(&reply) {
        bail!("the node didn't start: {error}");
    }
    call_value(&reply).map_err(|error| anyhow!("the node didn't start: {error}"))?;
    // A bootstrapping node answers slowly, so the job ends here; the L1 view shows it syncing.
    ctx.log("✓ L1: node started; it syncs from its peers (mode Bootstrapping, then Online)");
    Ok(())
}

/// Stops the node (`blockchain_module stop`), then the logosctl daemon.
pub async fn stop(ctx: &mut Ctx) -> anyhow::Result<()> {
    let program = logosctl_path().ok_or_else(|| anyhow!("logosctl isn't on PATH"))?;
    if !daemon_running(&program).await {
        ctx.log("L1: the daemon isn't running");
        return Ok(());
    }
    if module_loaded(&program).await {
        let label = format!("logosctl call {MODULE} stop");
        run(ctx, &label, &program, &["call", MODULE, "stop"], Duration::from_secs(120)).await?;
    }
    run(ctx, "logosctl daemon stop", &program, &["daemon", "stop"], Duration::from_secs(60)).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while daemon_running(&program).await {
        if tokio::time::Instant::now() > deadline {
            bail!("the daemon still answers 30s after `logosctl daemon stop`");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    ctx.log("✓ L1: node and daemon stopped");
    Ok(())
}

// ---- Bridging L1 funds into LEZ ----

/// How a deposit names its LEZ recipient (its `metadata`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataForm {
    /// `borsh(DepositMetadata { recipient: Identified(Key(pk)) })`, `01 00 ‖ pk`: what LEZ
    /// v0.4.0-rc3's bridge (`bridge_core::deposit`) parses.
    Key,
    /// The recipient's 32-byte account id, as the Logos Blockchain app's guide describes for the
    /// LEZ testnet. LEZ v0.4.0-rc3's bridge rejects this form.
    Account,
}

impl MetadataForm {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "" | "key" => Ok(Self::Key),
            "account" | "id" => Ok(Self::Account),
            other => bail!("metadata is `key` or `account`, not {other:?}"),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Key => "key (01 00 ‖ pk, LEZ v0.4)",
            Self::Account => "account (the 32-byte account id)",
        }
    }
}

/// A bridge deposit as asked for: `amount` of the L1 address `from`'s funds to the LEZ account
/// `to`, through the network's channel.
#[derive(Clone, Debug)]
pub struct BridgeRequest {
    /// The L1 address (hex) that pays: the deposited note, the fees and the change are its.
    pub from: String,
    /// The LEZ account credited, base58.
    pub to: String,
    pub amount: u64,
    /// The LEZ network's channel on the L1, hex.
    pub channel: String,
    /// The most each transaction may pay in fees (a transfer paid about 10,000 on the devnet).
    pub max_fee: u64,
    pub metadata: MetadataForm,
}

/// A planned deposit. It deposits one whole note worth exactly `amount`, with
/// `channel_deposit_with_notes`, which (unlike `channel_deposit`) pays a fee: a deposit without
/// one never got into a block.
#[derive(Clone, Debug)]
pub struct BridgeCall {
    pub request: BridgeRequest,
    /// The LEZ account's public key, hex.
    pub pk: String,
    /// The deposit's metadata, hex.
    pub metadata: String,
    /// A note of `from` worth exactly `amount`, when there is one; otherwise sending splits one
    /// off first, with a transfer from `from` to itself.
    pub note: Option<String>,
}

impl BridgeCall {
    /// `channel_deposit_with_notes(channel, [note], metadata, change, [funding], max_fee, tip)`:
    /// `from` funds the fee from its other notes and takes the change.
    pub fn deposit_args(&self, note: &str) -> Vec<String> {
        let request = &self.request;
        vec![
            "call".into(),
            MODULE.into(),
            "channel_deposit_with_notes".into(),
            text_arg(&request.channel),
            format!("json:[\"{note}\"]"),
            text_arg(&self.metadata),
            text_arg(&request.from),
            format!("json:[\"{}\"]", request.from),
            text_arg(&request.max_fee.to_string()),
            // No tip: the current one.
            text_arg(""),
        ]
    }

    /// The transfer that makes a note of exactly `amount`: `from` to itself.
    pub fn split(&self) -> TransferCall {
        let from = &self.request.from;
        TransferCall {
            from: from.clone(),
            to: from.clone(),
            amount: self.request.amount,
            change: from.clone(),
        }
    }
}

/// `logosctl <args>` as a shell would need it typed.
fn shell_line(args: Vec<String>) -> String {
    let args: Vec<String> = args
        .into_iter()
        .map(|arg| if arg.contains([' ', '"', '\'']) { format!("'{arg}'") } else { arg })
        .collect();
    format!("logosctl {}", args.join(" "))
}

fn is_hex32(text: &str) -> bool {
    text.len() == 64 && text.chars().all(|c| c.is_ascii_hexdigit())
}

/// A deposit's metadata for a key-held LEZ account, as LEZ's bridge reads it
/// (`bridge_core::DepositMetadata`): tag 1 (`Identified`), evidence tag 0 (`Key`), the public key.
pub fn deposit_metadata(pk: &str) -> anyhow::Result<String> {
    let pk = pk.trim().to_ascii_lowercase();
    if !is_hex32(&pk) {
        bail!("a LEZ public key is 32 bytes of hex, not {pk:?}");
    }
    Ok(format!("0100{pk}"))
}

/// The other form: the LEZ account id's 32 bytes, as hex.
pub fn account_metadata(account: &str) -> anyhow::Result<String> {
    let bytes = bs58::decode(account.trim())
        .into_vec()
        .map_err(|_| anyhow!("{account:?} isn't a base58 account id"))?;
    if bytes.len() != 32 {
        bail!("{account:?} is {} bytes, not 32", bytes.len());
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The `pk <hex>` line of `wallet account get --keys`.
pub fn parse_pk(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("pk "))
        .map(|pk| pk.trim().to_ascii_lowercase())
}

/// `wallet_get_notes`' `{"notes":[{"id","value"}],"tip"}` as `(id, value)` pairs.
pub fn parse_notes(value: &Value) -> Vec<(String, u64)> {
    value
        .get("notes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|note| {
            let id = note.get("id")?.as_str()?.to_owned();
            let value = match note.get("value")? {
                Value::String(text) => text.parse().ok()?,
                other => other.as_u64()?,
            };
            Some((id, value))
        })
        .collect()
}

async fn notes(program: &Path, address: &str) -> Result<Vec<(String, u64)>, String> {
    Ok(parse_notes(&call(program, "wallet_get_notes", &[address, ""]).await?))
}

/// Runs a module call for a job and returns its value, or fails with the module's reason.
async fn job_call(ctx: &mut Ctx, program: &Path, args: Vec<String>) -> anyhow::Result<Value> {
    let label = shell_line(args.clone());
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (_, out) = run(ctx, &label, program, &args, Duration::from_secs(120)).await?;
    let reply: Value = serde_json::from_str(out.trim())
        .map_err(|_| anyhow!("logosctl's reply isn't JSON: {}", first_line(&out).unwrap_or("")))?;
    if let Some(error) = top_level_error(&reply) {
        bail!("{error}");
    }
    call_value(&reply).map_err(|error| anyhow!("{}: {error}", args.get(2).copied().unwrap_or("call")))
}

/// Checks a deposit, works out its metadata and the note it deposits, then shows the plan;
/// Enter on it sends.
pub async fn plan_bridge(ctx: &mut Ctx, request: BridgeRequest) -> anyhow::Result<()> {
    let program = logosctl_path().ok_or_else(|| anyhow!("logosctl isn't on PATH"))?;
    let channel = request.channel.trim().trim_start_matches("0x").to_ascii_lowercase();
    if !is_hex32(&channel) {
        bail!("the channel id is 32 bytes of hex, not {:?}", request.channel);
    }
    let from = request.from.trim().trim_start_matches("0x").to_ascii_lowercase();
    if !is_hex32(&from) {
        bail!("the L1 address is 32 bytes of hex, not {:?}", request.from);
    }
    let request = BridgeRequest { from, channel, ..request };

    let keys = ctx.wallet(&["account", "get", "--keys", "-a", &format!("Public/{}", request.to)]);
    let output = ctx.run_quiet("wallet account get --keys", keys).await?;
    let pk = parse_pk(&output.out).ok_or_else(|| {
        anyhow!(
            "the LEZ wallet gave no public key for {}: is it a public account in this wallet?",
            request.to
        )
    })?;
    let metadata = match request.metadata {
        MetadataForm::Key => deposit_metadata(&pk)?,
        MetadataForm::Account => account_metadata(&request.to)?,
    };

    let notes = notes(&program, &request.from)
        .await
        .map_err(|error| anyhow!("the L1 address {}: {error}", jobs::short(&request.from)))?;
    let total: u64 = notes.iter().map(|(_, value)| value).sum();
    let note = notes
        .iter()
        .find(|(_, value)| *value == request.amount)
        .map(|(id, _)| id.clone());
    let lez_balance = ctx.rpc.account(&request.to).await?.map(|view| view.balance());
    let channel_state = call(&program, "get_channel_state", &[&request.channel]).await;

    let call = BridgeCall { request, pk, metadata, note };
    let request = &call.request;
    let mut lines = vec![
        format!("from L1     {}  ({} in {} note(s))", request.from, total, notes.len()),
        format!(
            "to LEZ      Public/{}  (balance {})",
            request.to,
            lez_balance.map_or("not on chain yet".to_owned(), |b| b.to_string())
        ),
        format!("  its pk    {}", call.pk),
        format!("amount      {}", request.amount),
        format!("max fee     {} per transaction", request.max_fee),
        format!("channel     {}  ({})", request.channel, ctx.network.label()),
        format!("metadata    {}  ({})", call.metadata, request.metadata.name()),
        String::new(),
    ];
    match &channel_state {
        Ok(_) => lines.push("channel on L1: found".to_owned()),
        Err(error) => lines.push(format!("✗ channel on L1: {error}")),
    }
    // The fee comes from `from`'s other notes, never the deposited one.
    let (steps, enough) = match &call.note {
        Some(note) => {
            let others = total - request.amount;
            lines.push(format!("1. deposit note {} (exactly {})", jobs::short(note), request.amount));
            lines.push(format!("   $ {}", shell_line(call.deposit_args(note))));
            (1, others >= request.max_fee)
        }
        None => {
            lines.push(format!("1. no note of exactly {}: split one off, `from` to itself", request.amount));
            lines.push(format!("   $ {}", call.split().command_line()));
            lines.push("   then wait for it to land in a block (a few minutes)".to_owned());
            lines.push("2. deposit that note, the fee paid from the change".to_owned());
            lines.push(format!("   $ {}", shell_line(call.deposit_args("<the new note>"))));
            (2, total >= request.amount + 2 * request.max_fee)
        }
    };
    if !enough {
        lines.push(format!(
            "✗ {} can't cover {} plus up to {} in fees beside the deposited note",
            jobs::short(&request.from),
            request.amount,
            request.max_fee * steps
        ));
    }
    lines.push(String::new());
    lines.push(
        "Enter sends, then waits for the deposit to land in an L1 block. LEZ credits it shortly \
         after (under 30s in practice)."
            .to_owned(),
    );
    ctx.send(Msg::Plan {
        title: format!("bridge {} → {}", request.amount, jobs::short(&request.to)),
        lines,
        call: enough.then(|| Planned::Bridge(call.clone())),
    });
    Ok(())
}

/// Polls `from`'s notes every 10s, up to `wait`, until `done` holds for them.
async fn wait_for_notes(
    program: &Path,
    from: &str,
    wait: Duration,
    done: impl Fn(&[(String, u64)]) -> bool,
) -> Option<Vec<(String, u64)>> {
    let deadline = tokio::time::Instant::now() + wait;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_secs(10)).await;
        if let Ok(notes) = notes(program, from).await
            && done(&notes)
        {
            return Some(notes);
        }
    }
    None
}

/// Sends the deposit: splits off a note of exactly the amount first when there's none, waits
/// for that to land, deposits it, then waits for the deposit to land.
pub async fn bridge(ctx: &mut Ctx, call: &BridgeCall) -> anyhow::Result<()> {
    let program = logosctl_path().ok_or_else(|| anyhow!("logosctl isn't on PATH"))?;
    let (from, amount) = (call.request.from.clone(), call.request.amount);
    let note = match &call.note {
        Some(note) => note.clone(),
        None => {
            let before: Vec<String> = notes(&program, &from)
                .await
                .map_err(|error| anyhow!("{error}"))?
                .into_iter()
                .map(|(id, _)| id)
                .collect();
            ctx.log(format!("splitting off a note of exactly {amount}: a transfer from {} to itself", jobs::short(&from)));
            let sent = job_call(ctx, &program, call.split().args()).await?;
            ctx.log(format!("✓ split submitted: {sent}; waiting for it to land in a block"));
            let notes = wait_for_notes(&program, &from, Duration::from_secs(600), |notes| {
                notes.iter().any(|(id, value)| *value == amount && !before.contains(id))
            })
            .await
            .ok_or_else(|| {
                anyhow!("the split didn't land within 10 minutes; once it does, b again deposits its note")
            })?;
            let (note, _) = notes
                .into_iter()
                .find(|(id, value)| *value == amount && !before.contains(id))
                .expect("found above");
            ctx.log(format!("✓ split in a block: note {} holds {amount}", jobs::short(&note)));
            note
        }
    };

    let sent = job_call(ctx, &program, call.deposit_args(&note)).await?;
    ctx.log(format!("✓ deposit submitted on L1: {sent}"));
    ctx.log("waiting for it to land in an L1 block (its note is spent then)".to_owned());
    let landed = wait_for_notes(&program, &from, Duration::from_secs(600), |notes| {
        !notes.iter().any(|(id, _)| *id == note)
    })
    .await;
    match landed {
        Some(_) => ctx.log(format!(
            "✓ deposit in an L1 block. LEZ credits Public/{} shortly after (under 30s in practice)",
            jobs::short(&call.request.to)
        )),
        None => ctx.log(
            "not in a block within 10 minutes: it waits in the node's mempool (keep the node running)",
        ),
    }
    Ok(())
}

// ---- Transfers between L1 addresses ----

/// An L1 transfer: `wallet_transfer_funds(change, [from], to, amount, tip)`.
#[derive(Clone, Debug)]
pub struct TransferCall {
    /// The address that pays (hex); the module signs with its key from the keystore.
    pub from: String,
    pub to: String,
    pub amount: u64,
    /// Where the rest of the spent notes goes back to (hex), usually `from`.
    pub change: String,
}

impl TransferCall {
    pub fn args(&self) -> Vec<String> {
        vec![
            "call".into(),
            MODULE.into(),
            "wallet_transfer_funds".into(),
            text_arg(&self.change),
            // The senders are a list: logosctl's `json:` form.
            format!("json:[\"{}\"]", self.from),
            text_arg(&self.to),
            text_arg(&self.amount.to_string()),
            // No tip: the current one.
            text_arg(""),
        ]
    }

    pub fn command_line(&self) -> String {
        shell_line(self.args())
    }
}

/// An address as typed: hex (an optional `0x`), or the name of a key in the keystore beside the
/// config (`PoWClaim`), matched without regard to case.
pub fn resolve_address(text: &str, names: &BTreeMap<String, String>) -> anyhow::Result<String> {
    let text = text.trim();
    let hex = text.trim_start_matches("0x").to_ascii_lowercase();
    if is_hex32(&hex) {
        return Ok(hex);
    }
    names
        .iter()
        .find(|(_, name)| name.eq_ignore_ascii_case(text))
        .map(|(hex, _)| hex.clone())
        .ok_or_else(|| anyhow!("{text:?} is neither 32 bytes of hex nor a key name in the keystore"))
}

/// A balance as the module reports it, or why there's none (`Unknown wallet address.` for keys the
/// wallet tracks no funds for).
async fn balance_text(program: &Path, address: &str) -> Result<String, String> {
    call(program, "wallet_get_balance", &[address]).await.map(|value| match value {
        Value::String(text) => text,
        other => other.to_string(),
    })
}

/// Checks a transfer and shows its plan; Enter on it sends.
pub async fn plan_transfer(ctx: &mut Ctx, call: TransferCall, config: Option<PathBuf>) -> anyhow::Result<()> {
    let program = logosctl_path().ok_or_else(|| anyhow!("logosctl isn't on PATH"))?;
    let names = names_beside(config.as_deref());
    let call = TransferCall {
        from: resolve_address(&call.from, &names)?,
        to: resolve_address(&call.to, &names)?,
        change: resolve_address(&call.change, &names)?,
        ..call
    };
    if call.from == call.to {
        bail!("from and to are the same address");
    }
    let label = |hex: &str| names.get(hex).map(|name| format!(" [{name}]")).unwrap_or_default();
    let from_balance = balance_text(&program, &call.from).await;
    let to_balance = balance_text(&program, &call.to).await;
    let shown = |balance: &Result<String, String>| match balance {
        Ok(balance) => format!("balance {balance}"),
        Err(error) => error.clone(),
    };
    let mut lines = vec![
        format!("from    {}{}  ({})", call.from, label(&call.from), shown(&from_balance)),
        format!("to      {}{}  ({})", call.to, label(&call.to), shown(&to_balance)),
        format!("amount  {}", call.amount),
        format!("change  {}{}", call.change, label(&call.change)),
        String::new(),
    ];
    let enough = from_balance
        .as_ref()
        .is_ok_and(|b| b.parse::<u128>().is_ok_and(|b| b >= u128::from(call.amount)));
    if !enough {
        lines.push(format!("✗ from can't pay {}: {}", call.amount, shown(&from_balance)));
    }
    if to_balance.is_err() {
        lines.push(
            "The node's wallet tracks no funds for `to`, so its balance won't show here; the \
             sender's will drop once the transfer is in a block."
                .to_owned(),
        );
    }
    lines.push(
        "A note already spent by a pending transaction (an unconfirmed deposit, say) can't be \
         spent again: the two conflict and only one gets in."
            .to_owned(),
    );
    lines.push(String::new());
    lines.push(format!("$ {}", call.command_line()));
    lines.push(String::new());
    lines.push("Enter sends, then watches both balances for up to 3 minutes.".to_owned());
    ctx.send(Msg::Plan {
        title: format!("L1 transfer {} → {}", call.amount, jobs::short(&call.to)),
        lines,
        call: enough.then(|| Planned::L1Transfer(call.clone())),
    });
    Ok(())
}

/// Sends an L1 transfer, then watches the balances until one changes.
pub async fn transfer(ctx: &mut Ctx, call: &TransferCall) -> anyhow::Result<()> {
    let program = logosctl_path().ok_or_else(|| anyhow!("logosctl isn't on PATH"))?;
    let before = (
        balance_text(&program, &call.from).await,
        balance_text(&program, &call.to).await,
    );
    let args = call.args();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (_, out) = run(ctx, &call.command_line(), &program, &args, Duration::from_secs(120)).await?;
    let reply: Value = serde_json::from_str(out.trim())
        .map_err(|_| anyhow!("logosctl's reply isn't JSON: {}", first_line(&out).unwrap_or("")))?;
    if let Some(error) = top_level_error(&reply) {
        bail!("{error}");
    }
    let sent = call_value(&reply).map_err(|error| anyhow!("wallet_transfer_funds: {error}"))?;
    ctx.log(format!("✓ transfer submitted on L1: {sent}"));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;
        let now = (
            balance_text(&program, &call.from).await,
            balance_text(&program, &call.to).await,
        );
        if now != before {
            let show = |b: &Result<String, String>| b.clone().unwrap_or_else(|_| "-".to_owned());
            ctx.log(format!(
                "✓ in a block: from {} → {}, to {} → {}",
                show(&before.0),
                show(&now.0),
                show(&before.1),
                show(&now.1)
            ));
            return Ok(());
        }
        if tokio::time::Instant::now() > deadline {
            ctx.log(
                "not in a block within 3 minutes: it waits in the node's mempool (keep the node \
                 running); the L1 view shows the balances when it lands.",
            );
            return Ok(());
        }
    }
}

// ---- Mining and auto-claim (PoW) ----
//
// The running node takes four switches: `pow_start_mining`, `pow_stop_mining`,
// `pow_start_auto_claim`, `pow_stop_auto_claim`. None is kept: every start begins with mining off,
// and auto-claim armed exactly when the config names claim targets. Auto-claim pays, each tick, the
// target furthest below its threshold, and stops mining once every target has reached its own.
//
// The settings live in the config's `pow:` section, read when the node starts. The module's
// `pow_configure` writes it, but (0.3.5-rc.1) checks targets against `wallet.known_keys` in the
// same file, which Basecamp's `user_config.yaml` doesn't hold, so it refuses every target there.
// prog-tui rewrites the section itself, leaving the rest of the file as it was, and checks the
// targets against the keys the running node's wallet tracks: a target it doesn't track stops the
// node from starting.

/// How often auto-claim tries: `pow.auto_claim.tick`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimTick {
    Seconds(u64),
    Slots(u64),
}

impl Default for ClaimTick {
    /// The node's default.
    fn default() -> Self {
        Self::Seconds(10)
    }
}

impl ClaimTick {
    /// `10`, `10s`, `10 seconds`, `5 slots`.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let text = text.trim().to_ascii_lowercase();
        let digits = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
        let (number, unit) = text.split_at(digits);
        let value: u64 = number
            .parse()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| anyhow!("claim every {text:?}: a whole number above 0, then s or slots"))?;
        match unit.trim() {
            "" | "s" | "sec" | "secs" | "second" | "seconds" => Ok(Self::Seconds(value)),
            "slot" | "slots" => Ok(Self::Slots(value)),
            other => bail!("claim every {text:?}: unknown unit {other:?} (s or slots)"),
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::Seconds(value) => format!("{value}s"),
            Self::Slots(1) => "1 slot".to_owned(),
            Self::Slots(value) => format!("{value} slots"),
        }
    }

    fn unit(self) -> (&'static str, u64) {
        match self {
            Self::Seconds(value) => ("seconds", value),
            Self::Slots(value) => ("slots", value),
        }
    }
}

/// One auto-claim target: a key the node's wallet tracks, and the balance it should reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimTarget {
    /// Lowercase hex.
    pub public_key: String,
    pub threshold: u64,
}

/// The config's `pow:` section. A missing field is the node's default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PowSettings {
    /// Ticket-search threads; `None` (`null`) is one per CPU.
    pub max_threads: Option<u64>,
    /// Search attempts in flight per block.
    pub max_tickets_per_block: u64,
    pub tick: ClaimTick,
    /// Empty: auto-claim stays off.
    pub targets: Vec<ClaimTarget>,
}

impl Default for PowSettings {
    fn default() -> Self {
        Self {
            max_threads: Some(1),
            max_tickets_per_block: 1,
            tick: ClaimTick::default(),
            targets: Vec::new(),
        }
    }
}

impl PowSettings {
    /// `threads 1 · 1 ticket/block · claim every 10s · no targets`.
    pub fn summary(&self, names: &BTreeMap<String, String>) -> String {
        let threads = self.max_threads.map_or("auto".to_owned(), |n| n.to_string());
        let targets = if self.targets.is_empty() {
            "no claim targets (auto-claim off)".to_owned()
        } else {
            self.targets
                .iter()
                .map(|target| format!("{} ≥ {}", key_label(&target.public_key, names), target.threshold))
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!(
            "threads {threads} · {} ticket/block · claim every {} · {targets}",
            self.max_tickets_per_block,
            self.tick.label()
        )
    }

    /// The `pow:` section as YAML, in the layout Basecamp writes.
    pub fn yaml(&self) -> String {
        let threads = self.max_threads.map_or("null".to_owned(), |n| n.to_string());
        let (unit, value) = self.tick.unit();
        let mut yaml = format!(
            "pow:\n  mining:\n    max_threads: {threads}\n    max_tickets_per_block: {}\n  auto_claim:\n    \
             tick:\n      unit: {unit}\n      value: {value}\n",
            self.max_tickets_per_block
        );
        if self.targets.is_empty() {
            yaml.push_str("    targets: []\n");
        } else {
            yaml.push_str("    targets:\n");
            for target in &self.targets {
                yaml.push_str(&format!(
                    "    - public_key: {}\n      threshold: {}\n",
                    target.public_key, target.threshold
                ));
            }
        }
        yaml
    }

    /// The `targets` form field's text: `PoWClaim=1000000, …`.
    pub fn targets_text(&self, names: &BTreeMap<String, String>) -> String {
        self.targets
            .iter()
            .map(|target| format!("{}={}", names.get(&target.public_key).unwrap_or(&target.public_key), target.threshold))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A key's keystore name, else its short hex.
fn key_label(hex: &str, names: &BTreeMap<String, String>) -> String {
    names.get(hex).cloned().unwrap_or_else(|| jobs::short(hex))
}

/// A YAML number, or a number in a string.
fn yaml_u64(value: &serde_yaml::Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.trim().parse().ok())
}

/// The `pow:` section of a config's text.
pub fn parse_pow_settings(text: &str) -> anyhow::Result<PowSettings> {
    let document: serde_yaml::Value = serde_yaml::from_str(text).context("the config isn't YAML")?;
    let mut settings = PowSettings::default();
    let Some(pow) = document.get("pow").filter(|pow| !pow.is_null()) else {
        return Ok(settings);
    };
    if let Some(mining) = pow.get("mining") {
        match mining.get("max_threads") {
            None => {}
            Some(serde_yaml::Value::Null) => settings.max_threads = None,
            Some(value) => {
                settings.max_threads =
                    Some(yaml_u64(value).ok_or_else(|| anyhow!("pow.mining.max_threads isn't a number"))?);
            }
        }
        if let Some(value) = mining.get("max_tickets_per_block") {
            settings.max_tickets_per_block = yaml_u64(value)
                .ok_or_else(|| anyhow!("pow.mining.max_tickets_per_block isn't a number"))?;
        }
    }
    if let Some(claim) = pow.get("auto_claim") {
        if let Some(tick) = claim.get("tick") {
            let value = tick
                .get("value")
                .and_then(yaml_u64)
                .ok_or_else(|| anyhow!("pow.auto_claim.tick.value isn't a number"))?;
            settings.tick = match tick.get("unit").and_then(serde_yaml::Value::as_str) {
                Some("slots") => ClaimTick::Slots(value),
                Some("seconds") | None => ClaimTick::Seconds(value),
                Some(other) => bail!("pow.auto_claim.tick.unit {other:?}"),
            };
        }
        if let Some(targets) = claim.get("targets").and_then(serde_yaml::Value::as_sequence) {
            for target in targets {
                let public_key = target
                    .get("public_key")
                    .and_then(serde_yaml::Value::as_str)
                    .ok_or_else(|| anyhow!("a claim target without a public_key"))?
                    .trim_start_matches("0x")
                    .to_ascii_lowercase();
                let threshold = target
                    .get("threshold")
                    .and_then(yaml_u64)
                    .ok_or_else(|| anyhow!("claim target {public_key}: no threshold"))?;
                settings.targets.push(ClaimTarget { public_key, threshold });
            }
        }
    }
    Ok(settings)
}

fn read_pow_config(config: &Path) -> Result<PowSettings, String> {
    let text = std::fs::read_to_string(config).map_err(|error| format!("reading {}: {error}", config.display()))?;
    parse_pow_settings(&text).map_err(|error| format!("{error:#}"))
}

/// `text` with its top-level `pow:` section swapped for `block`, every other line untouched; the
/// section is added at the end when there is none. The section runs from the `pow:` line to the
/// last indented line before the next top-level key (comments and blank lines after it stay).
pub fn replace_pow_block(text: &str, block: &str) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let Some(start) = lines.iter().position(|line| line.starts_with("pow:")) else {
        let mut out = text.to_owned();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(block);
        return out;
    };
    let inline = lines[start]["pow:".len()..].trim();
    let mut end = start + 1;
    if inline.is_empty() || inline.starts_with('#') {
        for (index, line) in lines.iter().enumerate().skip(start + 1) {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if line.starts_with([' ', '\t']) {
                end = index + 1;
            } else {
                break;
            }
        }
    }
    let mut out: String = lines[..start].concat();
    out.push_str(block);
    out.push_str(&lines[end..].concat());
    out
}

/// What the PoW form asks for, as typed.
#[derive(Clone, Debug)]
pub struct PowForm {
    pub threads: String,
    pub tickets: String,
    pub tick: String,
    /// `PoWClaim=1000000, <hex>=500`; empty: none.
    pub targets: String,
}

/// A planned rewrite of the config's `pow:` section.
#[derive(Clone, Debug)]
pub struct PowWrite {
    pub config: PathBuf,
    /// The file as planned against: the write refuses when it changed since.
    pub before: String,
    pub after: String,
}

impl PowWrite {
    pub fn backup(&self) -> PathBuf {
        PathBuf::from(format!("{}.bak", self.config.display()))
    }
}

/// The settings a form asks for. Keys are hex or keystore names.
pub fn pow_settings_from(form: &PowForm, names: &BTreeMap<String, String>) -> anyhow::Result<PowSettings> {
    let whole = |what: &str, text: &str| -> anyhow::Result<u64> {
        text.trim()
            .replace('_', "")
            .parse::<u64>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| anyhow!("{what} must be a whole number above 0, not {text:?}"))
    };
    let threads = form.threads.trim();
    let max_threads = if threads.eq_ignore_ascii_case("auto") {
        None
    } else {
        Some(whole("threads", threads)?)
    };
    let mut targets: Vec<ClaimTarget> = Vec::new();
    for entry in form.targets.split([',', '\n']).map(str::trim).filter(|entry| !entry.is_empty()) {
        let (key, threshold) = entry
            .split_once('=')
            .ok_or_else(|| anyhow!("claim target {entry:?}: write it key=threshold, e.g. PoWClaim=1000000"))?;
        let public_key = resolve_address(key, names)?;
        if targets.iter().any(|target| target.public_key == public_key) {
            bail!("{} is a claim target twice", key.trim());
        }
        targets.push(ClaimTarget {
            public_key,
            threshold: whole(&format!("{}'s threshold", key.trim()), threshold)?,
        });
    }
    Ok(PowSettings {
        max_threads,
        max_tickets_per_block: whole("tickets per block", &form.tickets)?,
        tick: ClaimTick::parse(&form.tick)?,
        targets,
    })
}

/// Plans the config's new `pow:` section: the lines to show, and the write, or `None` when the
/// section already says this. `known` is the node's wallet keys (`wallet_get_known_addresses`),
/// `None` when the node isn't Online to ask: then only targets the config already has are allowed.
pub fn plan_pow(
    config: &Path,
    form: &PowForm,
    known: Option<&[String]>,
) -> anyhow::Result<(Vec<String>, Option<PowWrite>)> {
    let before = std::fs::read_to_string(config).with_context(|| format!("reading {}", config.display()))?;
    let current = parse_pow_settings(&before)?;
    let names = names_beside(Some(config));
    let settings = pow_settings_from(form, &names)?;
    for target in &settings.targets {
        let tracked = match known {
            Some(known) => known.iter().any(|key| key.eq_ignore_ascii_case(&target.public_key)),
            None if current.targets.iter().any(|old| old.public_key == target.public_key) => true,
            None => bail!(
                "the node isn't Online to list the keys its wallet tracks, so a new claim target \
                 can't be checked (an untracked one stops the node from starting)"
            ),
        };
        if !tracked {
            bail!(
                "{} isn't a key the node's wallet tracks (L1 wallets pane): the node would refuse to start",
                key_label(&target.public_key, &names)
            );
        }
    }
    let after = replace_pow_block(&before, &settings.yaml());
    // The new file must read back as these settings, with everything else as it was.
    let check: serde_yaml::Value = serde_yaml::from_str(&after).context("the rewritten config isn't YAML")?;
    let old: serde_yaml::Value = serde_yaml::from_str(&before).context("the config isn't YAML")?;
    let without_pow = |value: &serde_yaml::Value| {
        let mut value = value.clone();
        if let Some(map) = value.as_mapping_mut() {
            map.remove("pow");
        }
        value
    };
    if parse_pow_settings(&after)? != settings || without_pow(&check) != without_pow(&old) {
        bail!("rewriting the pow section would change more than it (unusual layout?); edit {} by hand", config.display());
    }
    let mut lines = vec![
        format!("config  {}", config.display()),
        String::new(),
        format!("now     {}", current.summary(&names)),
        format!("after   {}", settings.summary(&names)),
        String::new(),
    ];
    if settings == current {
        return Ok((lines, None));
    }
    lines.push("The pow section becomes:".to_owned());
    lines.extend(settings.yaml().lines().map(|line| format!("  {line}")));
    lines.push(String::new());
    if !settings.targets.is_empty() {
        lines.push(
            "Auto-claim pays, each tick, the target furthest below its threshold; once all have \
             reached theirs it stops, and stops mining with it."
                .to_owned(),
        );
    }
    let write = PowWrite { config: config.to_owned(), before, after };
    lines.push(
        "The node reads this when it starts: it applies once s stops the node and s starts it again \
         (Online again soon after a short stop, after about an hour's bootstrap after a long one)."
            .to_owned(),
    );
    lines.push(String::new());
    lines.push(format!("Enter writes the file; the old one is kept as {}.", write.backup().display()));
    Ok((lines, Some(write)))
}

/// Writes a planned `pow:` section: the old file to `<config>.bak`, the new one through a rename.
pub async fn write_pow(ctx: &mut Ctx, write: &PowWrite) -> anyhow::Result<()> {
    let now = std::fs::read_to_string(&write.config).with_context(|| format!("reading {}", write.config.display()))?;
    if now != write.before {
        bail!("{} changed since the plan: p plans again", write.config.display());
    }
    let backup = write.backup();
    std::fs::write(&backup, &now).with_context(|| format!("writing {}", backup.display()))?;
    let tmp = PathBuf::from(format!("{}.prog-tui.tmp", write.config.display()));
    std::fs::write(&tmp, &write.after).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::set_permissions(&tmp, std::fs::metadata(&write.config)?.permissions())?;
    std::fs::rename(&tmp, &write.config).with_context(|| format!("replacing {}", write.config.display()))?;
    ctx.log(format!("✓ wrote the pow section of {} (the old file: {})", write.config.display(), backup.display()));
    ctx.log("it applies when the node next starts: s stops it, s starts it");
    Ok(())
}

/// A switch on the running node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowSwitch {
    StartMining,
    StopMining,
    StartAutoClaim,
    StopAutoClaim,
}

impl PowSwitch {
    pub fn method(self) -> &'static str {
        match self {
            Self::StartMining => "pow_start_mining",
            Self::StopMining => "pow_stop_mining",
            Self::StartAutoClaim => "pow_start_auto_claim",
            Self::StopAutoClaim => "pow_stop_auto_claim",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::StartMining => "start mining",
            Self::StopMining => "stop mining",
            Self::StartAutoClaim => "arm auto-claim",
            Self::StopAutoClaim => "disarm auto-claim",
        }
    }
}

/// `pow_status`'s `is_mining` and `auto_claim.is_armed`.
pub fn pow_flags(pow: &Value) -> (Option<bool>, Option<bool>) {
    (
        pow.get("is_mining").and_then(Value::as_bool),
        pow.get("auto_claim").and_then(|claim| claim.get("is_armed")).and_then(Value::as_bool),
    )
}

/// Flips a switch on the running node, then reports what `pow_status` says.
pub async fn pow_switch(ctx: &mut Ctx, switch: PowSwitch) -> anyhow::Result<()> {
    let program = logosctl_path().ok_or_else(|| anyhow!("logosctl isn't on PATH"))?;
    let method = switch.method();
    let label = format!("logosctl call {MODULE} {method}");
    let (_, out) = run(ctx, &label, &program, &["call", MODULE, method], TIMEOUT).await?;
    let reply: Value = serde_json::from_str(out.trim())
        .map_err(|_| anyhow!("logosctl's reply isn't JSON: {}", first_line(&out).unwrap_or("")))?;
    if let Some(error) = top_level_error(&reply) {
        bail!("{error}");
    }
    call_value(&reply).map_err(|error| anyhow!("{method}: {error}"))?;
    let status = call(&program, "pow_status", &[]).await.map_err(|error| anyhow!("pow_status: {error}"))?;
    let (mining, armed) = pow_flags(&status);
    let show = |flag: Option<bool>, yes: &str, no: &str| match flag {
        Some(true) => yes.to_owned(),
        Some(false) => no.to_owned(),
        None => "?".to_owned(),
    };
    ctx.log(format!(
        "✓ L1: mining {} · auto-claim {}",
        show(mining, "on", "off"),
        show(armed, "armed", "off")
    ));
    if switch == PowSwitch::StartAutoClaim && armed == Some(false) {
        let no_targets = status
            .get("auto_claim")
            .and_then(|claim| claim.get("targets"))
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty);
        if no_targets {
            bail!(
                "auto-claim didn't arm: the node has no claim targets. p sets them in the config; \
                 the node reads them when it starts"
            );
        }
        bail!("auto-claim didn't arm (the node's log may say why)");
    }
    Ok(())
}

/// `show_config <path>`: the node's config as the module reads it, secrets shown as `<secret>`.
pub async fn show_config(config: &Path) -> Result<String, String> {
    let program = logosctl_path().ok_or("logosctl isn't on PATH")?;
    let path = config.display().to_string();
    match call(&program, "show_config", &[&path]).await? {
        Value::String(text) => Ok(text),
        other => Ok(serde_json::to_string_pretty(&other).unwrap_or_default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = r#"{"daemon":{"pid":775538,"status":"running","version":"1.0.0"},"modules":[{"name":"package_manager","status":"loaded","uptime_seconds":311,"version":"1.0.0"},{"name":"blockchain_module","status":"loaded","uptime_seconds":270,"version":"0.3.5-rc.1"}],"modules_summary":{"crashed":0,"loaded":2,"not_loaded":0}}"#;

    #[test]
    fn reads_the_daemon_status() {
        let (daemon, mut modules) = parse_status(&serde_json::from_str(STATUS).unwrap());
        assert_eq!(daemon.status, "running");
        assert_eq!(daemon.pid, Some(775_538));
        assert_eq!(modules[0].name, MODULE, "the node comes first");
        assert_eq!(modules[0].uptime_seconds, Some(270));
        let stats = serde_json::json!([{"cpu_percent":0.5,"memory_mb":133.1,"name":"blockchain_module","pid":776896}]);
        merge_stats(&mut modules, &stats);
        assert_eq!(modules[0].memory_mb, Some(133.1));
        assert_eq!(modules[1].memory_mb, None);
    }

    #[test]
    fn unwraps_call_results() {
        let chain: Value = serde_json::from_str(
            r#"{"method":"get_cryptarchia_info","module":"blockchain_module","result":{"error":null,"success":true,"value":"{\"height\":21781,\"mode\":\"Online\"}"},"status":"ok"}"#,
        )
        .unwrap();
        assert_eq!(call_value(&chain).unwrap()["height"], 21781);

        let list: Value = serde_json::from_str(
            r#"{"result":{"error":null,"success":true,"value":["2dea","82da"]},"status":"ok"}"#,
        )
        .unwrap();
        assert_eq!(call_value(&list).unwrap()[1], "82da");

        // A string that isn't JSON stays a string ("10000000" parses as a number, so use text).
        let text: Value = serde_json::from_str(
            r#"{"result":{"error":null,"success":true,"value":"network:\n  port: 3000"},"status":"ok"}"#,
        )
        .unwrap();
        assert_eq!(call_value(&text).unwrap(), "network:\n  port: 3000");

        let unknown: Value = serde_json::from_str(
            r#"{"result":{"error":"Unknown wallet address.","success":false,"value":null},"status":"ok"}"#,
        )
        .unwrap();
        let wallet = L1Wallet { address: "2dea".into(), name: None, balance: call_value(&unknown).map(|v| v.to_string()) };
        assert!(wallet.holds_no_funds());

        let silent: Value =
            serde_json::from_str(r#"{"result":{"error":null,"success":false,"value":null}}"#).unwrap();
        assert!(call_value(&silent).is_err());
    }

    #[test]
    fn reports_logosctls_own_errors() {
        let value: Value = serde_json::from_str(
            r#"{"code":"DAEMON_NOT_RUNNING","message":"the daemon isn't running","status":"error"}"#,
        )
        .unwrap();
        assert_eq!(top_level_error(&value).as_deref(), Some("the daemon isn't running"));
        assert_eq!(top_level_error(&serde_json::from_str(STATUS).unwrap()), None);
    }

    #[test]
    fn names_addresses_from_public_keys_only() {
        let keystore = "public_keys:\n  PoWClaim: 82DA47\n  Stake: 'ae88'\nsecret_keys:\n  PoWClaim: deadbeef\n";
        let names = keystore_names(keystore);
        assert_eq!(names.get("82da47").map(String::as_str), Some("PoWClaim"));
        assert_eq!(names.get("ae88").map(String::as_str), Some("Stake"));
        assert_eq!(names.len(), 2, "nothing from secret_keys");
    }

    #[test]
    fn builds_deposit_metadata_like_lezs_bridge() {
        let pk = "1ea4a67bf2daadba6192c64a9324accb347050f1d4d29d8a28ceb3226bc85be4";
        // bridge_core's test: [1, 0] ++ key bytes.
        assert_eq!(deposit_metadata(pk).unwrap(), format!("0100{pk}"));
        assert!(deposit_metadata("12").is_err());
        assert_eq!(parse_pk(&format!("Balance 0, nonce 0\npk {pk}\n")).as_deref(), Some(pk));
        assert_eq!(parse_pk("Balance 0"), None);
        // The account form: the base58 id's 32 bytes.
        let account = account_metadata("2bMvhBNWxGYjSHegycMBvNNL4eaKm3dDiBUUApHL8dPy").unwrap();
        assert_eq!(account.len(), 64);
        assert!(account_metadata("abc").is_err());
        assert_eq!(MetadataForm::parse("").unwrap(), MetadataForm::Key);
        assert_eq!(MetadataForm::parse("Account").unwrap(), MetadataForm::Account);
        assert!(MetadataForm::parse("hex").is_err());

        let notes = parse_notes(&serde_json::json!({"notes": [{"id": "c6a7", "value": "9966490"}, {"id": "aa", "value": 5}], "tip": "10"}));
        assert_eq!(notes, [("c6a7".to_owned(), 9_966_490), ("aa".to_owned(), 5)]);

        let call = BridgeCall {
            request: BridgeRequest {
                from: "82da".into(),
                to: "2bMv".into(),
                amount: 5,
                channel: "d616".into(),
                max_fee: 50_000,
                metadata: MetadataForm::Key,
            },
            pk: pk.into(),
            metadata: deposit_metadata(pk).unwrap(),
            note: None,
        };
        assert_eq!(
            call.deposit_args("aa")[2..],
            [
                "channel_deposit_with_notes".to_owned(),
                "str:d616".to_owned(),
                "json:[\"aa\"]".to_owned(),
                format!("str:0100{pk}"),
                "str:82da".to_owned(),
                "json:[\"82da\"]".to_owned(),
                "str:50000".to_owned(),
                "str:".to_owned(),
            ]
        );
        let split = call.split();
        assert_eq!((split.from.as_str(), split.to.as_str(), split.amount), ("82da", "82da", 5));
    }

    #[test]
    fn reads_the_nodes_phase_from_the_daemon_log() {
        let started = "[2026-11-19 09:33:00.101] [out] [blockchain_module] INFO chain::service: entering AwaitingGenesisTime phase
[2026-11-19 09:33:03.347] [out] [blockchain_module] INFO chain::service: entering InitialBlockDownload phase
[2026-11-19 09:33:04.709] [out] [blockchain_module] INFO chain::service: entering ProlongedBootstrapPeriod phase
[2026-11-19 09:42:41.071] [info] [logos] [blockchain_module] Received new block: {\"header\":{\"slot\":841360,\"parent_block\":\"26\"}}
[2026-11-19 09:42:42.000] [out] [storage_module] entering Nothing phase
";
        let log = parse_node_log(started);
        assert_eq!(log.phase.as_deref(), Some("ProlongedBootstrapPeriod"));
        assert_eq!(log.since.as_deref(), Some("2026-11-19 09:33:04"));
        assert_eq!(log.slot, Some(841_360));
        assert!(log.started() && !log.online);
        assert_eq!(log.online_expected().as_deref(), Some("10:33"));

        let online = format!("{started}[2026-11-19 10:33:04.800] [out] [blockchain_module] INFO blend::service: Chain is now Online.\n");
        let log = parse_node_log(&online);
        assert!(log.online);
        assert_eq!(log.online_expected(), None);

        let failed = "[2026-11-19 09:18:10.000] [out] [blockchain_module] INFO chain::service: entering InitialBlockDownload phase
[2026-11-19 09:24:43.699] [out] [blockchain_module] 2026-11-19T09:24:43Z ERROR logos_blockchain::chain::network: Initial Block Download failed: AllPeersFailed(AllPeersFailed). Initiating graceful shutdown. Retry with different bootstrap peers
";
        let log = parse_node_log(failed);
        assert!(!log.started());
        assert!(log.shut_down.as_deref().unwrap().starts_with("Initial Block Download failed"));
    }

    #[test]
    fn builds_a_transfer_and_resolves_key_names() {
        let a = "82da47b18aa86d6b18e786145f3d154383a7aa80af0e821e715c29199571670a";
        let b = "9388f8d30cad0448ae2a145e0e3f0951b598ca06dfe48b23d3b2c49bd3a81e18";
        let names = keystore_names(&format!("public_keys:\n  PoWClaim: {a}\n  LeaderFunding: {b}\n"));
        assert_eq!(resolve_address("leaderfunding", &names).unwrap(), b);
        assert_eq!(resolve_address(&format!("0x{}", a.to_uppercase()), &names).unwrap(), a);
        assert!(resolve_address("Nobody", &names).is_err());

        let call = TransferCall { from: a.into(), to: b.into(), amount: 7, change: a.into() };
        assert_eq!(
            call.args()[2..],
            [
                "wallet_transfer_funds".to_owned(),
                format!("str:{a}"),
                format!("json:[\"{a}\"]"),
                format!("str:{b}"),
                "str:7".to_owned(),
                "str:".to_owned(),
            ]
        );
        assert!(call.command_line().contains(&format!(" 'json:[\"{a}\"]' ")));
    }

    #[test]
    fn totals_the_numeric_balances() {
        let wallet = |balance: Result<&str, &str>| L1Wallet {
            address: String::new(),
            name: None,
            balance: balance.map(str::to_owned).map_err(str::to_owned),
        };
        let status = L1Status {
            wallets: vec![wallet(Ok("10")), wallet(Err("Unknown wallet address.")), wallet(Ok("5"))],
            ..L1Status::default()
        };
        assert_eq!(status.total(), Some(15));
        assert_eq!(L1Status::default().total(), None);
    }

    /// Basecamp's `user_config.yaml` (0.3.5), shortened.
    const BASECAMP_CONFIG: &str = "version: 0.3.5\nnetwork:\n  backend:\n    initial_peers:\n    - /ip4/1.2.3.4/udp/50001/quic-v1\nstate:\n  base_folder: /x/state\npow:\n  mining:\n    max_threads: 1\n    max_tickets_per_block: 1\n  auto_claim:\n    tick:\n      unit: seconds\n      value: 10\n    targets: []\n";

    const KEY_A: &str = "82da47b18aa86d6b18e786145f3d154383a7aa80af0e821e715c29199571670a";
    const KEY_B: &str = "9388f8d30cad0448ae2a145e0e3f0951b598ca06dfe48b23d3b2c49bd3a81e18";

    #[test]
    fn parses_claim_ticks() {
        assert_eq!(ClaimTick::parse("10").unwrap(), ClaimTick::Seconds(10));
        assert_eq!(ClaimTick::parse("30s").unwrap(), ClaimTick::Seconds(30));
        assert_eq!(ClaimTick::parse("5 slots").unwrap(), ClaimTick::Slots(5));
        assert_eq!(ClaimTick::parse(" 1 Slot ").unwrap(), ClaimTick::Slots(1));
        assert!(ClaimTick::parse("0s").is_err());
        assert!(ClaimTick::parse("10m").is_err());
        assert!(ClaimTick::parse("s").is_err());
        assert_eq!(ClaimTick::Slots(5).label(), "5 slots");
        assert_eq!(ClaimTick::parse(&ClaimTick::Seconds(10).label()).unwrap(), ClaimTick::Seconds(10));
    }

    #[test]
    fn reads_the_pow_section() {
        assert_eq!(parse_pow_settings(BASECAMP_CONFIG).unwrap(), PowSettings::default());
        // No section: the node's defaults.
        assert_eq!(parse_pow_settings("version: 0.3.5\n").unwrap(), PowSettings::default());
        let text = format!(
            "pow:\n  mining:\n    max_threads: null\n  auto_claim:\n    tick: {{unit: slots, value: 3}}\n    targets:\n    - public_key: 0x{}\n      threshold: '500'\n",
            KEY_A.to_ascii_uppercase()
        );
        let settings = parse_pow_settings(&text).unwrap();
        assert_eq!(settings.max_threads, None);
        assert_eq!(settings.max_tickets_per_block, 1);
        assert_eq!(settings.tick, ClaimTick::Slots(3));
        assert_eq!(settings.targets, vec![ClaimTarget { public_key: KEY_A.to_owned(), threshold: 500 }]);
    }

    #[test]
    fn writes_settings_that_read_back() {
        let settings = PowSettings {
            max_threads: None,
            max_tickets_per_block: 2,
            tick: ClaimTick::Slots(4),
            targets: vec![
                ClaimTarget { public_key: KEY_A.to_owned(), threshold: 1_000_000 },
                ClaimTarget { public_key: KEY_B.to_owned(), threshold: 5 },
            ],
        };
        assert_eq!(parse_pow_settings(&settings.yaml()).unwrap(), settings);
        // The defaults write what Basecamp wrote, line for line.
        assert!(BASECAMP_CONFIG.ends_with(&PowSettings::default().yaml()));
    }

    #[test]
    fn replaces_only_the_pow_section() {
        let block = "pow:\n  new: 1\n";
        // At the end.
        let out = replace_pow_block(BASECAMP_CONFIG, block);
        assert_eq!(out, format!("{}{block}", &BASECAMP_CONFIG[..BASECAMP_CONFIG.find("pow:").unwrap()]));
        // In the middle: what follows, and the comment and blank line before it, stay.
        let text = "a: 1\npow:\n  mining:\n    max_threads: 1\n\n# next\nb:\n  c: 2\n";
        assert_eq!(replace_pow_block(text, block), "a: 1\npow:\n  new: 1\n\n# next\nb:\n  c: 2\n");
        // A comment inside the section goes with it.
        let text = "pow:\n  # threads\n  mining: {}\nb: 1\n";
        assert_eq!(replace_pow_block(text, block), "pow:\n  new: 1\nb: 1\n");
        // An inline value.
        assert_eq!(replace_pow_block("pow: {}\nb: 1\n", block), "pow:\n  new: 1\nb: 1\n");
        // None yet: added at the end.
        assert_eq!(replace_pow_block("a: 1", block), "a: 1\npow:\n  new: 1\n");
        // `power:` isn't `pow:`... but starts with it; a nested `pow:` isn't top-level.
        assert_eq!(replace_pow_block("x:\n  pow: 1\n", block), "x:\n  pow: 1\npow:\n  new: 1\n");
    }

    #[test]
    fn form_settings_resolve_names() {
        let names = keystore_names(&format!("public_keys:\n  PoWClaim: {KEY_A}\n  LeaderFunding: {KEY_B}\n"));
        let form = |threads: &str, targets: &str| PowForm {
            threads: threads.to_owned(),
            tickets: "1".to_owned(),
            tick: "10s".to_owned(),
            targets: targets.to_owned(),
        };
        let settings = pow_settings_from(&form("auto", "powclaim=1_000_000, leaderfunding = 5"), &names).unwrap();
        assert_eq!(settings.max_threads, None);
        assert_eq!(
            settings.targets,
            vec![
                ClaimTarget { public_key: KEY_A.to_owned(), threshold: 1_000_000 },
                ClaimTarget { public_key: KEY_B.to_owned(), threshold: 5 },
            ]
        );
        assert_eq!(settings.targets_text(&names), "PoWClaim=1000000, LeaderFunding=5");
        assert!(pow_settings_from(&form("2", ""), &names).unwrap().targets.is_empty());
        assert!(pow_settings_from(&form("0", ""), &names).is_err());
        assert!(pow_settings_from(&form("1", "PoWClaim"), &names).is_err());
        assert!(pow_settings_from(&form("1", "PoWClaim=1, PoWClaim=2"), &names).is_err());
        assert!(pow_settings_from(&form("1", "Nobody=1"), &names).is_err());
    }

    #[test]
    fn plans_a_pow_rewrite() {
        let dir = std::env::temp_dir().join(format!("prog-tui-pow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("user_config.yaml");
        std::fs::write(&config, BASECAMP_CONFIG).unwrap();
        std::fs::write(dir.join("keystore.yaml"), format!("public_keys:\n  PoWClaim: {KEY_A}\n")).unwrap();
        let form = |targets: &str| PowForm {
            threads: "1".to_owned(),
            tickets: "1".to_owned(),
            tick: "10s".to_owned(),
            targets: targets.to_owned(),
        };
        // Unchanged: nothing to write.
        assert!(plan_pow(&config, &form(""), None).unwrap().1.is_none());
        // A new target needs the node's keys to check against.
        assert!(plan_pow(&config, &form("PoWClaim=100"), None).is_err());
        assert!(plan_pow(&config, &form("PoWClaim=100"), Some(&[KEY_B.to_owned()])).is_err());
        let (lines, write) = plan_pow(&config, &form("PoWClaim=100"), Some(&[KEY_A.to_owned()])).unwrap();
        let write = write.unwrap();
        assert!(lines.iter().any(|line| line.contains("PoWClaim ≥ 100")));
        assert!(write.after.starts_with("version: 0.3.5\nnetwork:"));
        assert_eq!(parse_pow_settings(&write.after).unwrap().targets.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
