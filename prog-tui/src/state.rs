//! What prog-tui remembers between runs, and the `env.sh` it writes for your own shell.

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::lez::Lez;

/// Where the LEZ checkout sits relative to this crate, as the repos are laid out now.
pub const DEFAULT_LEZ_DIR: &str = "../../logos-execution-zone";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deployment {
    /// The guest's name, e.g. `hello_world`.
    pub name: String,
    /// The header account id, which is the program's address.
    pub program_id: String,
    /// The program is no longer on chain (the chain was reset).
    #[serde(default)]
    pub stale: bool,
}

/// An account outside the wallet whose state the Accounts pane shows, such as a program's PDA.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Watched {
    pub id: String,
    /// What it is, e.g. `PDA of jz_admin_seat`.
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Saved {
    /// As entered: absolute, `~/…`, or relative to this crate's directory.
    pub lez_dir: String,
    pub payer: Option<String>,
    pub account: Option<String>,
    /// The wallet storage's password, given to every wallet command as `LEE_WALLET_PASSWORD`.
    /// A non-empty one encrypts the storage.
    pub wallet_password: String,
    pub deployments: Vec<Deployment>,
    pub watched: Vec<Watched>,
}

impl Default for Saved {
    fn default() -> Self {
        Self {
            lez_dir: DEFAULT_LEZ_DIR.to_owned(),
            payer: None,
            account: None,
            wallet_password: "prog-tui".to_owned(),
            deployments: Vec::new(),
            watched: Vec::new(),
        }
    }
}

impl Saved {
    pub fn load(state_dir: &Path) -> Self {
        fs::read_to_string(state_dir.join("state.json"))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes `state.json` and `env.sh`.
    pub fn save(&self, state_dir: &Path, lez: &Lez) -> anyhow::Result<()> {
        fs::create_dir_all(state_dir)?;
        fs::write(
            state_dir.join("state.json"),
            serde_json::to_string_pretty(self)?,
        )?;
        fs::write(env_sh_path(state_dir), env_sh(self, lez))?;
        Ok(())
    }

    /// The most recent live deployment of `name`.
    pub fn latest(&self, name: &str) -> Option<&Deployment> {
        self.deployments
            .iter()
            .rev()
            .find(|deployment| deployment.name == name && !deployment.stale)
    }

    pub fn program_name(&self, program_id: &str) -> Option<&str> {
        self.deployments
            .iter()
            .find(|deployment| deployment.program_id == program_id)
            .map(|deployment| deployment.name.as_str())
    }

    /// Forgets everything that lived on the old chain.
    pub fn clear_chain(&mut self) {
        self.payer = None;
        self.account = None;
        self.deployments.clear();
        self.watched.clear();
    }

    /// Adds `id` to the watched accounts, or updates its note. Returns whether it was new.
    pub fn watch(&mut self, id: String, note: String) -> bool {
        match self.watched.iter_mut().find(|watched| watched.id == id) {
            Some(watched) => {
                watched.note = note;
                false
            }
            None => {
                self.watched.push(Watched { id, note });
                true
            }
        }
    }
}

pub fn env_sh_path(state_dir: &Path) -> PathBuf {
    state_dir.join("env.sh")
}

/// Resolves `lez_dir` against this crate's directory, expanding a leading `~/`.
pub fn resolve_lez_dir(crate_dir: &Path, lez_dir: &str) -> PathBuf {
    let path = match (lez_dir.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(lez_dir),
    };
    let path = if path.is_absolute() {
        path
    } else {
        crate_dir.join(path)
    };
    fs::canonicalize(&path).unwrap_or(path)
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// A script that exports the walkthrough's variables, for `source` in bash or zsh.
pub fn env_sh(saved: &Saved, lez: &Lez) -> String {
    let mut out = String::from(
        "# Written by prog-tui on every change. Load it into your shell with: source <this file>\n",
    );
    let mut export = |name: &str, value: &str| {
        let _ = writeln!(out, "export {name}={}", shell_quote(value));
    };
    export("LEZ_DIR", &lez.dir.display().to_string());
    export(
        "LEE_WALLET_HOME_DIR",
        &lez.wallet_home().display().to_string(),
    );
    export("LEE_WALLET_PASSWORD", &saved.wallet_password);
    export("BIN", &lez.bin_dir().display().to_string());
    if let Some(payer) = &saved.payer {
        export("PAYER", payer);
    }
    if let Some(account) = &saved.account {
        export("ACCOUNT", account);
    }
    // Later deployments of the same guest come last, so they win.
    for deployment in saved.deployments.iter().filter(|deployment| !deployment.stale) {
        export(
            &format!("PROGRAM_{}", deployment.name.to_uppercase()),
            &deployment.program_id,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_sh_exports_set_values_and_skips_stale() {
        let saved = Saved {
            payer: Some("PAY".into()),
            account: None,
            deployments: vec![
                Deployment {
                    name: "hello_world".into(),
                    program_id: "OLD".into(),
                    stale: true,
                },
                Deployment {
                    name: "hello_world".into(),
                    program_id: "NEW".into(),
                    stale: false,
                },
            ],
            ..Saved::default()
        };
        let lez = Lez::new(PathBuf::from("/x/it's"));
        let script = env_sh(&saved, &lez);
        assert!(script.contains("export LEZ_DIR='/x/it'\\''s'\n"));
        assert!(script.contains("export LEE_WALLET_PASSWORD='prog-tui'\n"));
        assert!(script.contains("export PAYER='PAY'\n"));
        assert!(!script.contains("ACCOUNT="));
        assert!(script.contains("export PROGRAM_HELLO_WORLD='NEW'\n"));
        assert!(!script.contains("OLD"));
    }

    #[test]
    fn watching_twice_updates_the_note_and_a_reset_forgets() {
        let mut saved = Saved::default();
        assert!(saved.watch("PDA".into(), "first".into()));
        assert!(!saved.watch("PDA".into(), "PDA of jz_admin_seat".into()));
        assert_eq!(
            saved.watched,
            vec![Watched {
                id: "PDA".into(),
                note: "PDA of jz_admin_seat".into()
            }]
        );
        saved.clear_chain();
        assert!(saved.watched.is_empty());
    }

    #[test]
    fn latest_skips_stale() {
        let mut saved = Saved::default();
        saved.deployments.push(Deployment {
            name: "a".into(),
            program_id: "1".into(),
            stale: false,
        });
        saved.deployments.push(Deployment {
            name: "a".into(),
            program_id: "2".into(),
            stale: true,
        });
        assert_eq!(saved.latest("a").map(|d| d.program_id.as_str()), Some("1"));
    }
}
