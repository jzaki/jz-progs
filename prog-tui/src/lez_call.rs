//! A program's messages, from `lez-call` (`../lez-call`).
//!
//! `lez-call iface` reads a guest's source: the messages its handler takes, their params in Borsh
//! order, hints from what each one checks, and its `PdaSeed` constants. It's quick (no chain), so
//! the UI runs it straight away. Calls go through `lez-call`, `lez-dryrun` and the wallet, as jobs.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use serde::Deserialize;

use crate::{
    lez::Lez,
    tools::{LEZ_CALL, LEZ_DRYRUN},
    wallet::parse_json_line,
};

#[derive(Clone, Debug, Deserialize)]
pub struct Interface {
    /// `"enum Message"`, `"struct Ping"`, a type such as `"Vec<u8>"`, or `"raw"`.
    pub message: String,
    pub methods: Vec<Method>,
    #[serde(default)]
    pub seeds: Vec<Seed>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Method {
    pub name: String,
    pub index: Option<u8>,
    pub fields: String,
    pub params: Vec<Param>,
    pub doc: Option<String>,
    pub hints: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Param {
    pub name: String,
    pub ty: String,
    pub hint: String,
}

/// A `PdaSeed` constant in the source: `--to pda:<name>` names its PDA.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Seed {
    pub name: String,
    /// `0x…`, 32 bytes.
    pub bytes: String,
}

impl Interface {
    /// Whether the handler takes one message type, not an enum: a call gives only its values.
    pub fn single(&self) -> bool {
        !self.message.starts_with("enum ")
    }
}

impl Method {
    /// What the picker lists: index, name, fields, and hints.
    pub fn label(&self) -> String {
        let index = self
            .index
            .map_or_else(|| "  ".to_owned(), |index| format!("{index:>2}"));
        let hints = if self.hints.is_empty() {
            String::new()
        } else {
            format!("   [{}]", self.hints.join(", "))
        };
        format!("{index}  {}{}{hints}", self.name, self.fields)
    }

    /// Sent by another program, not a transaction: a direct call is refused.
    pub fn internal(&self) -> bool {
        self.hints.iter().any(|hint| hint == "internal")
    }

    /// The source checks `is_authorized`: the receiver signs.
    pub fn signed(&self) -> bool {
        self.hints.iter().any(|hint| hint == "signed")
    }
}

/// `lez-call iface <source>` as text, for `v`.
pub fn text(lez: &Lez, password: &str, source: &Path) -> anyhow::Result<Vec<String>> {
    let out = run(lez, password, &["iface".as_ref(), source.as_os_str()])?;
    Ok(out.lines().map(str::to_owned).collect())
}

/// `lez-call --json iface <source>`, for `c`.
pub fn interface(lez: &Lez, password: &str, source: &Path) -> anyhow::Result<Interface> {
    let out = run(lez, password, &["--json".as_ref(), "iface".as_ref(), source.as_os_str()])?;
    let value = parse_json_line(&out, "LEZCALL")
        .ok_or_else(|| anyhow::anyhow!("lez-call printed no interface"))?;
    Ok(serde_json::from_value(value)?)
}

/// The guest a deployed program runs, by the image id in its header: `lez-dryrun source-of`.
pub fn guest_of(lez: &Lez, program_id: &str) -> anyhow::Result<String> {
    let tool = LEZ_DRYRUN
        .path()
        .ok_or_else(|| anyhow::anyhow!("finding a deployment's guest needs lez-dryrun (key b)"))?;
    let output = Command::new(tool)
        .args(["--rpc", &lez.sequencer_url(), "source-of", program_id, "--bin-dir"])
        .arg(lez.bin_dir())
        .output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    anyhow::ensure!(output.status.success(), "{}", stderr.trim().trim_start_matches("Error: "));
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The source of the guest named `guest`, in LEZ's guest package.
pub fn source_of(lez: &Lez, guest: &str) -> PathBuf {
    lez.guest_src_dir().join(format!("{guest}.rs"))
}

fn run(lez: &Lez, password: &str, args: &[&std::ffi::OsStr]) -> anyhow::Result<String> {
    let tool = LEZ_CALL
        .path()
        .ok_or_else(|| anyhow::anyhow!("lez-call isn't built (key b) or on PATH"))?;
    let output = Command::new(tool)
        .args(args)
        .current_dir(&lez.dir)
        .env("LEZ_WALLET", lez.release_bin("wallet"))
        .env("LEE_WALLET_HOME_DIR", lez.wallet_home())
        .env("LEE_WALLET_PASSWORD", password)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{}", stderr.trim().trim_start_matches("Error: "));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lez_calls_interface() {
        let line = r##"LEZCALL {"handler":"handle_message","message":"enum Message","methods":[{"doc":"To your seat, signed by you.","fields":"(Vec<u8>)","hints":["signed"],"index":0,"name":"Open","params":[{"hint":"text, or 0x… hex","name":"#0","ty":"Vec<u8>"}]},{"doc":null,"fields":"(Vec<u8>)","hints":["internal"],"index":1,"name":"Init","params":[{"hint":"text, or 0x… hex","name":"#0","ty":"Vec<u8>"}]}],"seeds":[{"name":"SEAT","bytes":"0x05"}],"source":"x.rs"}"##;
        let interface: Interface =
            serde_json::from_value(parse_json_line(line, "LEZCALL").unwrap()).unwrap();
        assert!(!interface.single());
        assert_eq!(interface.methods[0].label(), " 0  Open(Vec<u8>)   [signed]");
        assert!(interface.methods[0].signed());
        assert!(interface.methods[1].internal());
        assert_eq!(interface.methods[0].params[0].name, "#0");
        assert_eq!(interface.seeds[0].bytes, "0x05");
    }
}
