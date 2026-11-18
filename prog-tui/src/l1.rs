//! The L1 view's data: the Logos blockchain node run by the `logosctl` daemon (module
//! `blockchain_module`), and the wallets its keystore holds. Everything comes from running
//! `logosctl --json …`; nothing here starts, stops or changes the node.
//!
//! logosctl can't say which config the node was started with, so that path is a prog-tui setting
//! (`Saved::l1_config`). Its directory's `keystore.yaml` names the wallet addresses: only its
//! `public_keys:` block is read.
//!
//! Besides reading, two jobs act: starting or stopping the node (daemon, module, node), and
//! bridging L1 funds into a LEZ account with the module's `channel_deposit`.

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
    /// The first error among those calls, to show when they all fail.
    pub error: Option<String>,
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
    pub wallets: Vec<L1Wallet>,
    /// Why the wallets couldn't be listed.
    pub wallets_error: Option<String>,
    /// The config prog-tui was told the node runs with, if it exists.
    pub config: Option<PathBuf>,
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

fn names_beside(config: Option<&Path>) -> BTreeMap<String, String> {
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
    if !status.running() || !full {
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
            "wallet_get_claimable_vouchers",
        ] {
            let result = call(&program, method, &[]).await;
            let slot = match method {
                "get_network_info" => &mut found.network,
                "get_time_info" => &mut found.time,
                "pow_status" => &mut found.pow,
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

    if node_started(&program).await {
        ctx.log("L1: the node is already running");
        return Ok(());
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

/// A bridge deposit as asked for: L1 funds from `from` to the LEZ account `to`, through the
/// network's channel.
#[derive(Clone, Debug)]
pub struct BridgeRequest {
    /// The L1 address (a public key, hex) that pays.
    pub from: String,
    /// The LEZ account credited, base58.
    pub to: String,
    pub amount: u64,
    /// The LEZ network's channel on the L1, hex.
    pub channel: String,
}

/// A planned deposit: `channel_deposit(channel, from, amount, metadata, tip)`.
#[derive(Clone, Debug)]
pub struct BridgeCall {
    pub request: BridgeRequest,
    /// The LEZ account's public key, hex.
    pub pk: String,
    /// `borsh(DepositMetadata { recipient: Identified(Key(pk)) })`, hex.
    pub metadata: String,
}

impl BridgeCall {
    pub fn args(&self) -> Vec<String> {
        let request = &self.request;
        vec![
            "call".into(),
            MODULE.into(),
            "channel_deposit".into(),
            text_arg(&request.channel),
            text_arg(&request.from),
            // The module's `amount` is text; a bare `1` would go as a number.
            text_arg(&request.amount.to_string()),
            text_arg(&self.metadata),
            // No tip.
            text_arg(""),
        ]
    }

    pub fn command_line(&self) -> String {
        let args: Vec<String> = self
            .args()
            .into_iter()
            .map(|arg| if arg.contains([' ', '"', '\'']) { format!("'{arg}'") } else { arg })
            .collect();
        format!("logosctl {}", args.join(" "))
    }
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

/// The `pk <hex>` line of `wallet account get --keys`.
pub fn parse_pk(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("pk "))
        .map(|pk| pk.trim().to_ascii_lowercase())
}

/// Checks a deposit and works out its metadata from the LEZ account's public key, then shows the
/// plan; Enter on it sends.
pub async fn plan_bridge(ctx: &mut Ctx, request: BridgeRequest) -> anyhow::Result<()> {
    let program = logosctl_path().ok_or_else(|| anyhow!("logosctl isn't on PATH"))?;
    let channel = request.channel.trim().trim_start_matches("0x").to_ascii_lowercase();
    if !is_hex32(&channel) {
        bail!("the channel id is 32 bytes of hex, not {:?}", request.channel);
    }
    let from = request.from.trim().to_ascii_lowercase();
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
    let metadata = deposit_metadata(&pk)?;

    let l1_balance = call(&program, "wallet_get_balance", &[&request.from])
        .await
        .map_err(|error| anyhow!("the L1 address {}: {error}", jobs::short(&request.from)))?;
    let l1_balance = match l1_balance {
        Value::String(text) => text,
        other => other.to_string(),
    };
    let lez_balance = ctx.rpc.account(&request.to).await?.map(|view| view.balance());
    let channel_state = call(&program, "get_channel_state", &[&request.channel]).await;

    let call = BridgeCall { request, pk, metadata };
    let request = &call.request;
    let mut lines = vec![
        format!("from L1     {}  (balance {l1_balance})", request.from),
        format!("to LEZ      Public/{}  (balance {})", request.to, lez_balance.map_or("not on chain yet".to_owned(), |b| b.to_string())),
        format!("  its pk    {}", call.pk),
        format!("amount      {}", request.amount),
        format!("channel     {}  ({})", request.channel, ctx.network.label()),
        format!("metadata    {}  (Identified, Key, pk)", call.metadata),
        String::new(),
    ];
    match &channel_state {
        Ok(state) => {
            let text = state.to_string();
            let shown: String = text.chars().take(200).collect();
            lines.push(format!("channel on L1: {shown}{}", if text.len() > 200 { "…" } else { "" }));
        }
        Err(error) => lines.push(format!("✗ channel on L1: {error}")),
    }
    let enough = l1_balance.parse::<u128>().is_ok_and(|b| b >= u128::from(request.amount));
    if !enough {
        lines.push(format!("✗ the L1 address holds {l1_balance}, less than {}", request.amount));
    }
    lines.push(String::new());
    lines.push(format!("$ {}", call.command_line()));
    lines.push(String::new());
    lines.push(
        "The sequencer credits the LEZ account once it sees the deposit on L1. Enter sends, then \
         watches the LEZ balance for up to 3 minutes."
            .to_owned(),
    );
    ctx.send(Msg::Plan {
        title: format!("bridge {} → {}", request.amount, jobs::short(&request.to)),
        lines,
        call: enough.then(|| Planned::Bridge(call.clone())),
    });
    Ok(())
}

/// Sends the deposit, then watches the LEZ account's balance.
pub async fn bridge(ctx: &mut Ctx, call: &BridgeCall) -> anyhow::Result<()> {
    let program = logosctl_path().ok_or_else(|| anyhow!("logosctl isn't on PATH"))?;
    let to = call.request.to.clone();
    let before = ctx.rpc.account(&to).await?.map_or(0, |view| view.balance());
    let args = call.args();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (_, out) = run(ctx, &call.command_line(), &program, &args, Duration::from_secs(120)).await?;
    let reply: Value = serde_json::from_str(out.trim())
        .map_err(|_| anyhow!("logosctl's reply isn't JSON: {}", first_line(&out).unwrap_or("")))?;
    if let Some(error) = top_level_error(&reply) {
        bail!("{error}");
    }
    let sent = call_value(&reply).map_err(|error| anyhow!("channel_deposit: {error}"))?;
    ctx.log(format!("✓ deposit submitted on L1: {sent}"));

    ctx.log(format!("watching Public/{} (balance {before})", jobs::short(&to)));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    loop {
        let now = ctx.rpc.account(&to).await?.map_or(0, |view| view.balance());
        if now > before {
            ctx.log(format!("✓ credited on LEZ: {before} → {now} (+{})", now - before));
            return Ok(());
        }
        if tokio::time::Instant::now() > deadline {
            ctx.log(
                "not credited within 3 minutes. The sequencer mints a deposit once it sees it on \
                 L1, at the latest when L1 finalises it; the Accounts pane will show the balance.",
            );
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
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

        let call = BridgeCall {
            request: BridgeRequest {
                from: "82da".into(),
                to: "2bMv".into(),
                amount: 5,
                channel: "d616".into(),
            },
            pk: pk.into(),
            metadata: deposit_metadata(pk).unwrap(),
        };
        assert_eq!(call.args()[2..6], ["channel_deposit", "str:d616", "str:82da", "str:5"], "text, not numbers");
        assert_eq!(call.args()[6], format!("str:0100{pk}"));
        assert!(call.command_line().ends_with(" str:"), "no tip");
        assert!(call.command_line().contains(" str:5 "));
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
}
