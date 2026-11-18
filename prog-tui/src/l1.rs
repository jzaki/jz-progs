//! The L1 view's data: the Logos blockchain node run by the `logosctl` daemon (module
//! `blockchain_module`), and the wallets its keystore holds. Everything comes from running
//! `logosctl --json …`; nothing here starts, stops or changes the node.
//!
//! logosctl can't say which config the node was started with, so that path is a prog-tui setting
//! (`Saved::l1_config`). Its directory's `keystore.yaml` names the wallet addresses: only its
//! `public_keys:` block is read.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde_json::Value;
use tokio::{process::Command, task::JoinSet};

/// The module that runs the node.
pub const MODULE: &str = "blockchain_module";

/// How long one logosctl command may take before it counts as failed.
const TIMEOUT: Duration = Duration::from_secs(10);

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
    let output = tokio::time::timeout(
        TIMEOUT,
        Command::new(program)
            .arg("--json")
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
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

async fn call(program: &Path, method: &str, args: &[&str]) -> Result<Value, String> {
    let mut all = vec!["call", MODULE, method];
    all.extend_from_slice(args);
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

/// Asks logosctl about the daemon, the node and its wallets. The node's calls and the balances run
/// at once, since each logosctl run takes about half a second.
pub async fn gather(config: Option<PathBuf>) -> L1Status {
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
    if !status.running() {
        return status;
    }

    let mut tasks = JoinSet::new();
    let methods = [
        "get_cryptarchia_info",
        "get_network_info",
        "get_time_info",
        "pow_status",
        "wallet_get_claimable_vouchers",
    ];
    if status.module_loaded() {
        for method in methods {
            let program = program.clone();
            tasks.spawn(async move { (method.to_owned(), call(&program, method, &[]).await) });
        }
        let program = program.clone();
        tasks.spawn(async move {
            let addresses = call(&program, "wallet_get_known_addresses", &[]).await;
            ("wallet_get_known_addresses".to_owned(), addresses)
        });
    }
    {
        let program = program.clone();
        tasks.spawn(async move {
            ("stats".to_owned(), logosctl(&program, &["module", "stats"]).await)
        });
    }

    let mut node = status.module_loaded().then(Node::default);
    let mut addresses = None;
    while let Some(joined) = tasks.join_next().await {
        let Ok((what, result)) = joined else { continue };
        if what == "stats" {
            if let Ok(stats) = result {
                merge_stats(&mut status.modules, &stats);
            }
            continue;
        }
        if what == "wallet_get_known_addresses" {
            addresses = Some(result);
            continue;
        }
        let Some(node) = node.as_mut() else { continue };
        let slot = match what.as_str() {
            "get_cryptarchia_info" => &mut node.chain,
            "get_network_info" => &mut node.network,
            "get_time_info" => &mut node.time,
            "pow_status" => &mut node.pow,
            _ => &mut node.vouchers,
        };
        match result {
            Ok(value) => *slot = Some(value),
            Err(error) => {
                node.error.get_or_insert(format!("{what}: {error}"));
            }
        }
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
            let mut balances = JoinSet::new();
            for (index, address) in addresses.iter().enumerate() {
                let (program, address) = (program.clone(), address.clone());
                balances.spawn(async move {
                    (index, call(&program, "wallet_get_balance", &[&address]).await)
                });
            }
            let mut found = vec![Err("no answer".to_owned()); addresses.len()];
            while let Some(joined) = balances.join_next().await {
                if let Ok((index, result)) = joined {
                    found[index] = result.map(|value| match value {
                        Value::String(text) => text,
                        other => other.to_string(),
                    });
                }
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
