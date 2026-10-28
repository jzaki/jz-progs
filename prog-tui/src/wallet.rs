//! Reading the output of the `wallet`, `lez-call` and `lez-dryrun` binaries, and of the runners.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletAccount {
    pub public: bool,
    pub id: String,
    pub label: Option<String>,
    /// The key-derivation path. Imported accounts have none.
    pub path: Option<String>,
}

/// Parses `wallet account list`, whose lines look like:
///
/// ```text
/// Public/2RHZ…                       (imported)
/// /0 Public/5QBN… [account]          (derived, labelled)
/// / Private/3L3Z…
/// ```
pub fn parse_account_list(output: &str) -> Vec<WalletAccount> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (path, rest) = if line.starts_with('/') {
                let (path, rest) = line.split_once(' ')?;
                (Some(path.to_owned()), rest.trim())
            } else {
                (None, line)
            };
            let (kind, after) = rest.split_once('/')?;
            let public = match kind {
                "Public" => true,
                "Private" => false,
                _ => return None,
            };
            let (id, label) = match after.split_once(' ') {
                Some((id, label)) => (id, Some(label.trim())),
                None => (after, None),
            };
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
                return None;
            }
            let label = label
                .and_then(|label| label.strip_prefix('[')?.strip_suffix(']'))
                .map(str::to_owned);
            Some(WalletAccount {
                public,
                id: id.to_owned(),
                label,
                path,
            })
        })
        .collect()
}

/// The id from `wallet account new public`, which prints
/// `Generated new account with account_id Public/<id> …`.
pub fn parse_new_account(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let id = line.split("account_id Public/").nth(1)?.split_whitespace().next()?;
        (!id.is_empty()).then(|| id.to_owned())
    })
}

/// The last `<prefix> {json}` line in `output`: `LEZCALL` from `lez-call --json`, `LEZDRYRUN`
/// from `lez-dryrun --json`.
pub fn parse_json_line(output: &str, prefix: &str) -> Option<serde_json::Value> {
    output
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(prefix)?.strip_prefix(' '))
        .and_then(|json| serde_json::from_str(json).ok())
}

/// Every program id a runner deployed, in order, from its `PROG_CLI {"program_id":…}` lines
/// (`program_deployment::deploy_program` prints them). A runner that deploys two programs
/// (caller, then callee) prints two.
pub fn parse_program_ids(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix("PROG_CLI "))
        .filter_map(|json| serde_json::from_str::<serde_json::Value>(json).ok())
        .filter_map(|value| value["program_id"].as_str().map(str::to_owned))
        .collect()
}

/// Cargo and build-script chatter that would bury the useful lines in the log pane.
pub fn is_noise(line: &str) -> bool {
    // `lez-call --json`'s and `lez-dryrun --json`'s results: the job reads them and logs what
    // they say.
    line.starts_with("LEZCALL ")
        || line.starts_with("LEZDRYRUN ")
        || line.contains("Generated module at")
        || line.contains("future version of Rust")
        || line.contains("--future-incompat-report")
        || line.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_account_list() {
        let output = "Public/2RHZhw9h534Zr3eq2RGhQete2Hh667foECzXPmSkGni2\n\
                      /0/0/0 Public/2ikSa5D5hFnCCVqKGGCjq3T6JGKyX8cjWUZ47P9x849S [seg0]\n\
                      / Private/3L3ZBrCNm28Jkn93bVGjJ2YcjDsUGRu2SN4CXHQWvCde\n\
                      🔑 Running wallet\n";
        let accounts = parse_account_list(output);
        assert_eq!(accounts.len(), 3);
        assert_eq!(
            accounts[0],
            WalletAccount {
                public: true,
                id: "2RHZhw9h534Zr3eq2RGhQete2Hh667foECzXPmSkGni2".into(),
                label: None,
                path: None,
            }
        );
        assert_eq!(accounts[1].label.as_deref(), Some("seg0"));
        assert_eq!(accounts[1].path.as_deref(), Some("/0/0/0"));
        assert!(!accounts[2].public);
    }

    #[test]
    fn parses_new_account() {
        let output = "Generated new account with account_id Public/5QBN968ZNHs7yh8usxr1Rbx1AAMtqyZ8wK2FdVk2EuHX at path /0\nWith pk 16…";
        assert_eq!(
            parse_new_account(output).as_deref(),
            Some("5QBN968ZNHs7yh8usxr1Rbx1AAMtqyZ8wK2FdVk2EuHX")
        );
    }

    #[test]
    fn parses_every_program_id_in_order() {
        let output = "PROG_CLI {\"program_id\":\"CALLER\"}\n\
                      Transaction hash is ab\n\
                      PROG_CLI {\"program_id\":\"CALLEE\"}\n\
                      PROG_CLI {\"tx\":\"x\",\"block\":3}\n";
        assert_eq!(parse_program_ids(output), ["CALLER", "CALLEE"]);
        assert!(parse_program_ids("nothing").is_empty());
    }

    #[test]
    fn parses_json_lines() {
        let output = "noise\nLEZDRYRUN {\"plan\":{\"ok\":true}}\nLEZCALL {\"message_hex\":\"0x01\"}\n";
        assert_eq!(parse_json_line(output, "LEZCALL").unwrap()["message_hex"], "0x01");
        assert_eq!(parse_json_line(output, "LEZDRYRUN").unwrap()["plan"]["ok"], true);
        assert!(parse_json_line("LEZCALLX {}", "LEZCALL").is_none());
        assert!(parse_json_line("nothing", "LEZCALL").is_none());
    }
}
