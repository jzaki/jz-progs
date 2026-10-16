//! The LEZ checkout: where things are, and what state the repo is in.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Where `cargo build -p example_program_deployment_methods` puts the example guests.
const GUEST_DIR: &str = "target/riscv-guest/example_program_deployment_methods/example_program_deployment_programs/riscv32im-risc0-zkvm-elf/release";

pub const DEFAULT_SEQUENCER_URL: &str = "http://127.0.0.1:3040";

#[derive(Clone, Debug)]
pub struct Lez {
    pub dir: PathBuf,
}

impl Lez {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn present(&self) -> bool {
        self.dir.join("Justfile").is_file() && self.dir.join("lez/wallet").is_dir()
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.dir.join(GUEST_DIR)
    }

    pub fn guest_bin(&self, name: &str) -> PathBuf {
        self.bin_dir().join(format!("{name}.bin"))
    }

    /// `just run-wallet`'s home; the runners and `prog_cli` read the same wallet.
    pub fn wallet_home(&self) -> PathBuf {
        self.dir.join("lez/wallet/configs/debug")
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

    pub fn prog_cli_src(&self) -> PathBuf {
        self.dir
            .join("examples/program_deployment/src/bin/prog_cli.rs")
    }

    /// The built example guests, by name, sorted.
    pub fn guests(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.bin_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                (path.extension()? == "bin")
                    .then(|| path.file_stem()?.to_str().map(str::to_owned))
                    .flatten()
            })
            .collect();
        names.sort();
        names
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
    pub has_prog_cli_src: bool,
    pub wallet_built: bool,
    pub prog_cli_built: bool,
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
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
        has_prog_cli_src: lez.prog_cli_src().is_file(),
        wallet_built: lez.release_bin("wallet").is_file(),
        prog_cli_built: lez.release_bin("prog_cli").is_file(),
    }
}
