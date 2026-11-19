//! Which chain prog-tui works against, picked when it starts: the local standalone sequencer it
//! runs itself, or a sequencer of a network listed in `networks.json` (a devnet, say).
//!
//! `networks.json`, next to this crate's `Cargo.toml`:
//!
//! ```json
//! {
//!   "networks": [
//!     {
//!       "name": "devnet",
//!       "channel_id": "0101…01",
//!       "poll_timeout": "5s",
//!       "sequencers": ["127.0.0.1:3040", "10.0.0.7", "http://seq.example:3041"]
//!     }
//!   ]
//! }
//! ```
//!
//! Each sequencer is one choice in the selector. An address without a port gets the sequencer's
//! default RPC port, 3040; one without a scheme gets `http://`. `channel_id` is optional: a client
//! doesn't need it (the wallet only talks to the sequencer's RPC), but when it's given, prog-tui
//! checks it against the sequencer's `getChannelId`, so a wrong address shows up at once.
//! `poll_timeout` is optional too: the wallet's `seq_poll_timeout` on this network (see
//! [`Network::Remote::poll_timeout`]).

use std::{fs, path::Path};

use anyhow::Context as _;
use serde::Deserialize;

/// The sequencer's RPC port when an address doesn't give one.
const DEFAULT_RPC_PORT: u16 = 3040;

/// The file's name, in this crate's directory.
pub const NETWORKS_FILE: &str = "networks.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Network {
    /// LEZ's standalone sequencer on 127.0.0.1, which prog-tui starts, stops and resets.
    Local,
    /// A sequencer someone else runs: prog-tui only talks to its RPC.
    Remote {
        /// The network's name in `networks.json`, e.g. `devnet`.
        name: String,
        /// The sequencer's RPC, e.g. `http://10.0.0.7:3040`.
        url: String,
        /// The channel id the sequencer should report, as hex, when the file gives one.
        channel_id: Option<String>,
        /// The wallet's `seq_poll_timeout` here (`5s`, `500ms`, `1m`): how long it sleeps between
        /// looks for a sent transaction, up to `seq_tx_poll_max_blocks` looks. Set from this on
        /// every start; `None` is LEZ's debug config's value, which suits 1s blocks.
        poll_timeout: Option<String>,
    },
}

impl Network {
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }

    /// A short, filesystem-safe key: `local`, or `devnet-10.0.0.7-3040`. It names the network's
    /// state directory and wallet home, so each network keeps its own accounts and deployments.
    pub fn key(&self) -> String {
        match self {
            Self::Local => "local".to_owned(),
            Self::Remote { name, url, .. } => {
                let host = url
                    .split_once("://")
                    .map_or(url.as_str(), |(_, rest)| rest)
                    .trim_end_matches('/');
                format!("{name}-{host}")
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '-' })
                    .collect()
            }
        }
    }

    /// What the selector and the status bar show.
    pub fn label(&self) -> String {
        match self {
            Self::Local => "local standalone".to_owned(),
            Self::Remote { name, url, .. } => format!("{name} · {url}"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct NetworksFile {
    #[serde(default)]
    networks: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    name: String,
    #[serde(default)]
    channel_id: Option<String>,
    #[serde(default)]
    poll_timeout: Option<String>,
    sequencers: Vec<String>,
}

/// The selector's choices: local first, then one per sequencer in `networks.json`. A missing file
/// gives only local; a broken one is an error, so a typo doesn't silently drop the devnet.
pub fn choices(crate_dir: &Path) -> anyhow::Result<Vec<Network>> {
    let path = crate_dir.join(NETWORKS_FILE);
    let mut choices = vec![Network::Local];
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(choices);
    };
    choices.extend(parse(&text).with_context(|| format!("reading {}", path.display()))?);
    Ok(choices)
}

fn parse(text: &str) -> anyhow::Result<Vec<Network>> {
    let file: NetworksFile = serde_json::from_str(text)?;
    let mut networks = Vec::new();
    for entry in file.networks {
        anyhow::ensure!(!entry.name.trim().is_empty(), "a network has no name");
        let channel_id = entry
            .channel_id
            .map(|id| id.trim().trim_start_matches("0x").to_ascii_lowercase())
            .filter(|id| !id.is_empty());
        if let Some(id) = &channel_id {
            anyhow::ensure!(
                id.len() == 64 && id.chars().all(|c| c.is_ascii_hexdigit()),
                "{}: channel_id must be 32 bytes of hex, not {id:?}",
                entry.name
            );
        }
        let poll_timeout = entry
            .poll_timeout
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty());
        if let Some(text) = &poll_timeout {
            anyhow::ensure!(
                is_duration(text),
                "{}: poll_timeout must be a duration such as 5s, 500ms or 1m, not {text:?}",
                entry.name
            );
        }
        anyhow::ensure!(!entry.sequencers.is_empty(), "{}: no sequencers listed", entry.name);
        for address in &entry.sequencers {
            networks.push(Network::Remote {
                name: entry.name.trim().to_owned(),
                url: rpc_url(address)?,
                channel_id: channel_id.clone(),
                poll_timeout: poll_timeout.clone(),
            });
        }
    }
    Ok(networks)
}

/// A duration the wallet's `humantime` parsing takes: numbers each followed by a unit, e.g. `5s`,
/// `500ms`, `1m 30s`. A light check, so a typo fails here rather than in every wallet command.
fn is_duration(text: &str) -> bool {
    const UNITS: [&str; 8] = ["ms", "s", "sec", "secs", "m", "min", "mins", "h"];
    text.split_whitespace().all(|part| {
        let digits = part.find(|c: char| !c.is_ascii_digit()).unwrap_or(part.len());
        digits > 0 && UNITS.contains(&&part[digits..])
    }) && !text.trim().is_empty()
}

/// `10.0.0.7` → `http://10.0.0.7:3040`; `10.0.0.7:3041` → `http://10.0.0.7:3041`; a URL as given,
/// with the default port added when it has none.
pub fn rpc_url(address: &str) -> anyhow::Result<String> {
    let address = address.trim().trim_end_matches('/');
    anyhow::ensure!(!address.is_empty(), "an empty sequencer address");
    let (scheme, rest) = address.split_once("://").unwrap_or(("http", address));
    anyhow::ensure!(
        scheme == "http" || scheme == "https",
        "{address}: only http:// and https:// sequencers"
    );
    let has_port = match rest.rsplit_once(':') {
        // `[::1]:3040` has a port; a bare IPv6 address in brackets doesn't.
        Some((_, port)) => port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty(),
        None => false,
    };
    Ok(if has_port {
        format!("{scheme}://{rest}")
    } else {
        format!("{scheme}://{rest}:{DEFAULT_RPC_PORT}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_addresses() {
        assert_eq!(rpc_url("10.0.0.7").unwrap(), "http://10.0.0.7:3040");
        assert_eq!(rpc_url("10.0.0.7:3041").unwrap(), "http://10.0.0.7:3041");
        assert_eq!(rpc_url("http://seq.example").unwrap(), "http://seq.example:3040");
        assert_eq!(rpc_url("https://seq.example:443/").unwrap(), "https://seq.example:443");
        assert!(rpc_url("ws://x").is_err());
        assert!(rpc_url(" ").is_err());
    }

    #[test]
    fn expands_each_sequencer_into_a_choice() {
        let networks = parse(
            r#"{"networks": [{"name": "devnet", "channel_id": "0x01010101010101010101010101010101010101010101010101010101010101AB",
                "sequencers": ["127.0.0.1:3040", "127.0.0.1:3041"]}]}"#,
        )
        .unwrap();
        assert_eq!(networks.len(), 2);
        let Network::Remote { url, channel_id, .. } = &networks[1] else {
            panic!("a remote network");
        };
        assert_eq!(url, "http://127.0.0.1:3041");
        assert_eq!(channel_id.as_deref().map(|id| &id[60..]), Some("01ab"));
        assert_eq!(networks[1].key(), "devnet-127.0.0.1-3041");
        assert_eq!(networks[1].label(), "devnet · http://127.0.0.1:3041");
    }

    #[test]
    fn reads_the_poll_timeout() {
        let poll = |json: &str| match &parse(json).unwrap()[0] {
            Network::Remote { poll_timeout, .. } => poll_timeout.clone(),
            Network::Local => unreachable!(),
        };
        assert_eq!(poll(r#"{"networks": [{"name": "d", "poll_timeout": " 5s ", "sequencers": ["x"]}]}"#).as_deref(), Some("5s"));
        assert_eq!(poll(r#"{"networks": [{"name": "d", "poll_timeout": "1m 30s", "sequencers": ["x"]}]}"#).as_deref(), Some("1m 30s"));
        assert_eq!(poll(r#"{"networks": [{"name": "d", "poll_timeout": "", "sequencers": ["x"]}]}"#), None);
        assert_eq!(poll(r#"{"networks": [{"name": "d", "sequencers": ["x"]}]}"#), None);
    }

    #[test]
    fn refuses_a_bad_file() {
        assert!(parse("{").is_err());
        assert!(parse(r#"{"networks": [{"name": "d", "sequencers": []}]}"#).is_err());
        assert!(parse(r#"{"networks": [{"name": "d", "channel_id": "12", "sequencers": ["x"]}]}"#).is_err());
        assert!(parse(r#"{"networks": [{"name": "d", "poll_timeout": "5", "sequencers": ["x"]}]}"#).is_err());
        assert!(parse(r#"{"networks": [{"name": "d", "poll_timeout": "five s", "sequencers": ["x"]}]}"#).is_err());
        // No channel id is fine.
        assert_eq!(parse(r#"{"networks": [{"name": "d", "sequencers": ["x"]}]}"#).unwrap().len(), 1);
    }

    #[test]
    fn local_is_always_first() {
        let dir = std::env::temp_dir().join(format!("prog-tui-net-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(choices(&dir).unwrap(), [Network::Local]);
        fs::write(dir.join(NETWORKS_FILE), r#"{"networks": [{"name": "d", "sequencers": ["x"]}]}"#).unwrap();
        assert_eq!(choices(&dir).unwrap()[0], Network::Local);
        assert_eq!(choices(&dir).unwrap().len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }
}
