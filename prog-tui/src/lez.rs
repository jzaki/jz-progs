//! The LEZ checkout: the sequencer, the wallet and its home, and what state the repo is in. The
//! programs prog-tui builds are in lez-programs ([`crate::programs`]).

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::tools::{LEZ_CALL, LEZ_DRYRUN, SPEL};

pub const DEFAULT_SEQUENCER_URL: &str = "http://127.0.0.1:3040";

#[derive(Clone, Debug)]
pub struct Lez {
    pub dir: PathBuf,
    /// A wallet home of prog-tui's own, for a remote network; `None` is LEZ's debug home.
    wallet_home: Option<PathBuf>,
}

impl Lez {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            wallet_home: None,
        }
    }

    /// The same checkout, with its wallet at `home` instead of LEZ's debug home. A remote network
    /// keeps its own wallet, so its accounts and nonces never mix with the local chain's.
    pub fn with_wallet_home(mut self, home: PathBuf) -> Self {
        self.wallet_home = Some(home);
        self
    }

    pub fn present(&self) -> bool {
        self.dir.join("Justfile").is_file() && self.dir.join("lez/wallet").is_dir()
    }

    /// The wallet every command uses (`LEE_WALLET_HOME_DIR`): `just run-wallet`'s debug home for
    /// the local chain, or the network's own. `lez-call` and `spel` read the same wallet.
    pub fn wallet_home(&self) -> PathBuf {
        self.wallet_home
            .clone()
            .unwrap_or_else(|| self.debug_wallet_home())
    }

    /// LEZ's debug wallet home, `lez/wallet/configs/debug`: the local chain's, and the template
    /// for a remote network's `wallet_config.json`.
    pub fn debug_wallet_home(&self) -> PathBuf {
        self.dir.join("lez/wallet/configs/debug")
    }

    /// Makes `home` a wallet home for the sequencer at `url`: LEZ's debug `wallet_config.json`
    /// with `sequencer_addr` set to `url`. Keeps the rest of an existing config, so a hand edit
    /// (a shorter `seq_poll_timeout`, say) survives. Never touches `storage.json`.
    pub fn prepare_wallet_home(&self, home: &Path, url: &str) -> anyhow::Result<()> {
        fs::create_dir_all(home)?;
        let path = home.join("wallet_config.json");
        let template = if path.is_file() {
            path.clone()
        } else {
            self.debug_wallet_home().join("wallet_config.json")
        };
        let text = fs::read_to_string(&template)
            .map_err(|error| anyhow::anyhow!("reading {}: {error}", template.display()))?;
        let mut config: serde_json::Value = serde_json::from_str(&text)?;
        config["sequencer"]["sequencer_addr"] = serde_json::Value::String(url.to_owned());
        let updated = serde_json::to_string_pretty(&config)?;
        if fs::read_to_string(&path).ok().as_deref() != Some(updated.as_str()) {
            fs::write(&path, updated)?;
        }
        Ok(())
    }

    pub fn storage_json(&self) -> PathBuf {
        self.wallet_home().join("storage.json")
    }

    pub fn sequencer_home(&self) -> PathBuf {
        self.dir.join("lez/sequencer/service/sequencer_home")
    }

    pub fn release_bin(&self, name: &str) -> PathBuf {
        self.dir.join("target/release").join(name)
    }

    /// The execution gas limit the wallet declares on every transaction: `gas_limit` in
    /// `wallet_config.json`, or the wallet's `DEFAULT_GAS_LIMIT`.
    pub fn wallet_gas_limit(&self) -> u64 {
        const DEFAULT_GAS_LIMIT: u64 = 2_000_000;
        fs::read_to_string(self.wallet_home().join("wallet_config.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|config| config["gas_limit"].as_u64())
            .unwrap_or(DEFAULT_GAS_LIMIT)
    }

    /// The wallet's `sequencer.sequencer_addr`, which is where the sequencer listens.
    pub fn sequencer_url(&self) -> String {
        fs::read_to_string(self.wallet_home().join("wallet_config.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|config| {
                config["sequencer"]["sequencer_addr"]
                    .as_str()
                    .map(|url| url.trim().trim_end_matches('/').to_owned())
            })
            .unwrap_or_else(|| DEFAULT_SEQUENCER_URL.to_owned())
    }
}

#[derive(Clone, Debug, Default)]
pub struct RepoInfo {
    pub present: bool,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub dirty: bool,
    pub wallet_built: bool,
    /// `lez-call` is built (or on PATH); without it, calls can't be made.
    pub lez_call: bool,
    pub lez_call_checkout: bool,
    /// `lez-dryrun` is built (or on PATH); without it, calls are sent unplanned.
    pub lez_dryrun: bool,
    pub lez_dryrun_checkout: bool,
    /// `spel` is built (or on PATH); without it, lez-programs' programs can't be called.
    pub spel: bool,
    pub spel_checkout: bool,
}

pub fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Blocking: runs `git`.
pub fn repo_info(lez: &Lez) -> RepoInfo {
    if !lez.present() {
        return RepoInfo::default();
    }
    RepoInfo {
        present: true,
        branch: git(&lez.dir, &["branch", "--show-current"]).filter(|branch| !branch.is_empty()),
        head: git(&lez.dir, &["rev-parse", "--short", "HEAD"]),
        dirty: git(&lez.dir, &["status", "--porcelain"]).is_some_and(|status| !status.is_empty()),
        wallet_built: lez.release_bin("wallet").is_file(),
        lez_call: LEZ_CALL.path().is_some(),
        lez_call_checkout: LEZ_CALL.has_checkout(),
        lez_dryrun: LEZ_DRYRUN.path().is_some(),
        lez_dryrun_checkout: LEZ_DRYRUN.has_checkout(),
        spel: SPEL.path().is_some(),
        spel_checkout: SPEL.has_checkout(),
    }
}
