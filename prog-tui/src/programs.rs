//! The lez-programs checkout: its programs (built through their methods crates, called with
//! `spel` and their IDLs), and the `jz/` guest package prog-tui keeps in it for
//! `jz-progs/example_progs`.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::{lez::git, state::Origin, userprogs};

/// Where the lez-programs checkout sits relative to this crate.
pub const DEFAULT_PROGRAMS_DIR: &str = "../../lez-programs";

/// risc0-build's output directory under `target/riscv-guest/<methods>/<guest>/`.
const RISCV_RELEASE: &str = "riscv32im-risc0-zkvm-elf/release";

/// The jz guest package, relative to the lez-programs checkout. At the top level, not under
/// `programs/`, so lez-programs' own guest globs (`programs/*/methods/guest`) never pick it up.
const JZ_DIR: &str = "jz";

/// The jz package's files: `(path under jz/, contents)`.
const JZ_TEMPLATE: [(&str, &str); 4] = [
    ("methods/Cargo.toml", include_str!("../templates/jz/methods/Cargo.toml")),
    ("methods/build.rs", include_str!("../templates/jz/methods/build.rs")),
    ("methods/src/lib.rs", include_str!("../templates/jz/methods/lib.rs")),
    ("methods/guest/Cargo.toml", include_str!("../templates/jz/methods/guest/Cargo.toml")),
];

/// One of lez-programs' ported programs, e.g. token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    /// `programs/<name>`.
    pub name: String,
    /// Its methods package, e.g. `token-methods`: building it builds the guest.
    pub methods: String,
    /// Its guest package, e.g. `token-guest`.
    pub guest: String,
    /// The guest binary's name, e.g. `token`.
    pub bin: String,
}

/// A guest prog-tui can deploy: one of lez-programs' programs, or a built jz guest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guest {
    pub name: String,
    pub origin: Origin,
    pub bin: PathBuf,
}

impl Guest {
    pub fn built(&self) -> bool {
        self.bin.is_file()
    }
}

#[derive(Clone, Debug)]
pub struct Programs {
    pub dir: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct ProgramsInfo {
    pub present: bool,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub dirty: bool,
}

impl Programs {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn present(&self) -> bool {
        self.dir.join("Cargo.toml").is_file() && self.dir.join("programs").is_dir()
    }

    /// The programs in the workspace: each `programs/<name>/methods` member that isn't commented
    /// out. The others aren't ported to the actor model yet, though `target/` may still hold
    /// their old builds.
    pub fn programs(&self) -> Vec<Program> {
        let Ok(manifest) = fs::read_to_string(self.dir.join("Cargo.toml")) else {
            return Vec::new();
        };
        let mut programs: Vec<Program> = workspace_methods(&manifest)
            .into_iter()
            .filter_map(|name| {
                let methods_dir = self.dir.join("programs").join(&name).join("methods");
                let methods = package_name(&fs::read_to_string(methods_dir.join("Cargo.toml")).ok()?)?;
                let guest_manifest = fs::read_to_string(methods_dir.join("guest/Cargo.toml")).ok()?;
                let guest = package_name(&guest_manifest)?;
                let bin = bin_name(&guest_manifest).unwrap_or_else(|| name.clone());
                Some(Program { name, methods, guest, bin })
            })
            .collect();
        programs.sort_by(|a, b| a.name.cmp(&b.name));
        programs
    }

    /// What the Guests pane lists: lez-programs' programs, built or not, then the built jz guests.
    pub fn guests(&self) -> Vec<Guest> {
        let mut guests: Vec<Guest> = self
            .programs()
            .into_iter()
            .map(|program| Guest {
                bin: self.bin(&program),
                origin: Origin::Program(program.name.clone()),
                name: program.name,
            })
            .collect();
        guests.extend(self.jz_guests().into_iter().map(|name| Guest {
            bin: self.jz_bin(&name),
            origin: Origin::Jz,
            name,
        }));
        guests
    }

    pub fn bin(&self, program: &Program) -> PathBuf {
        self.dir
            .join("target/riscv-guest")
            .join(&program.methods)
            .join(&program.guest)
            .join(RISCV_RELEASE)
            .join(format!("{}.bin", program.bin))
    }

    /// Its spel IDL (v2), as lez-programs' `make idl` writes it.
    pub fn idl(&self, name: &str) -> PathBuf {
        self.dir.join("artifacts").join(format!("{name}-idl.json"))
    }

    pub fn jz_dir(&self) -> PathBuf {
        self.dir.join(JZ_DIR)
    }

    pub fn jz_manifest(&self) -> PathBuf {
        self.jz_dir().join("methods/Cargo.toml")
    }

    /// The jz guest package's `src/bin/`: each `jz_<name>.rs` there is a link to
    /// `example_progs/<name>.rs`.
    pub fn jz_src_dir(&self) -> PathBuf {
        self.jz_dir().join("methods/guest/src/bin")
    }

    /// Where the jz guests' `.bin`s land. The package builds with `--target-dir` lez-programs'
    /// `target/`, so it shares that build cache.
    pub fn jz_bin_dir(&self) -> PathBuf {
        self.dir
            .join("target/riscv-guest/jz-methods/jz-guest")
            .join(RISCV_RELEASE)
    }

    pub fn jz_bin(&self, name: &str) -> PathBuf {
        self.jz_bin_dir().join(format!("{name}.bin"))
    }

    /// Where the jz guests' spel IDLs go: `jz/idl/<guest>.json`, from `spel generate-idl`.
    pub fn jz_idl_dir(&self) -> PathBuf {
        self.jz_dir().join("idl")
    }

    pub fn jz_idl(&self, name: &str) -> PathBuf {
        self.jz_idl_dir().join(format!("{name}.json"))
    }

    /// The crate `spel generate-idl` reads for a jz guest: `jz/idl/src/<guest>/`, whose
    /// `src/lib.rs` is a link to the guest's source. spel reads a crate, not a single file.
    pub fn jz_idl_crate(&self, name: &str) -> PathBuf {
        self.jz_idl_dir().join("src").join(name)
    }

    /// The built jz guests, by name, sorted: `jz_*.bin` whose source is still linked.
    pub fn jz_guests(&self) -> Vec<String> {
        let src = self.jz_src_dir();
        let mut names: Vec<String> = fs::read_dir(self.jz_bin_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let name = path.file_stem()?.to_str()?;
                (path.extension()? == "bin"
                    && name.starts_with(userprogs::PREFIX)
                    && src.join(format!("{name}.rs")).exists())
                .then(|| name.to_owned())
            })
            .collect();
        names.sort();
        names
    }

    /// Writes the jz package's files where they are missing or differ from the templates.
    /// Returns whether anything was written. Never touches `src/bin/`, which holds the links.
    pub fn write_jz_package(&self) -> io::Result<bool> {
        let mut wrote = false;
        for (path, contents) in JZ_TEMPLATE {
            let dest = self.jz_dir().join(path);
            if fs::read_to_string(&dest).is_ok_and(|current| current == contents) {
                continue;
            }
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&dest, contents)?;
            wrote = true;
        }
        fs::create_dir_all(self.jz_src_dir())?;
        Ok(wrote)
    }
}

/// The state type to give `spel generate-idl --state` for a guest's source: the one type named
/// `State` or ending in `State` (a `struct`, an `enum`, or a `type` alias, which gives its
/// target, since spel doesn't follow aliases there). `None` when there are none, or several (a
/// program whose actors hold different states), or the alias's target isn't a type name.
pub fn state_type(source: &str) -> Option<String> {
    let mut found = Vec::new();
    for line in source.lines().map(str::trim) {
        let line = line.strip_prefix("pub ").unwrap_or(line);
        let (keyword, rest) = match line.split_once(' ') {
            Some((keyword @ ("struct" | "enum" | "type"), rest)) => (keyword, rest),
            _ => continue,
        };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.ends_with("State") {
            continue;
        }
        if keyword != "type" {
            found.push(name);
            continue;
        }
        let target = rest
            .split_once('=')
            .map(|(_, target)| target.trim().trim_end_matches(';').trim())
            .unwrap_or_default();
        let target = target.rsplit("::").next().unwrap_or_default();
        let is_type_name = target.starts_with(|c: char| c.is_ascii_uppercase())
            && target.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        found.push(if is_type_name { target.to_owned() } else { String::new() });
    }
    match found.as_slice() {
        [one] if !one.is_empty() => Some(one.clone()),
        _ => None,
    }
}

/// Blocking: runs `git`.
pub fn programs_info(programs: &Programs) -> ProgramsInfo {
    if !programs.present() {
        return ProgramsInfo::default();
    }
    ProgramsInfo {
        present: true,
        branch: git(&programs.dir, &["branch", "--show-current"]).filter(|branch| !branch.is_empty()),
        head: git(&programs.dir, &["rev-parse", "--short", "HEAD"]),
        dirty: git(&programs.dir, &["status", "--porcelain", "--untracked-files=no"])
            .is_some_and(|status| !status.is_empty()),
    }
}

/// The `<name>` of each `"programs/<name>/methods"` workspace member that isn't commented out.
fn workspace_methods(manifest: &str) -> Vec<String> {
    let Some(start) = manifest.find("members") else {
        return Vec::new();
    };
    let rest = &manifest[start..];
    let end = rest.find(']').unwrap_or(rest.len());
    rest[..end]
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let member = line.trim_end_matches(',').trim().trim_matches('"');
            member
                .strip_prefix("programs/")?
                .strip_suffix("/methods")
                .filter(|name| !name.contains('/'))
                .map(str::to_owned)
        })
        .collect()
}

/// The first `name = "…"` in `[package]`.
fn package_name(manifest: &str) -> Option<String> {
    field_in_section(manifest, "[package]", "name")
}

/// The first `name = "…"` in a `[[bin]]` section.
fn bin_name(manifest: &str) -> Option<String> {
    field_in_section(manifest, "[[bin]]", "name")
}

fn field_in_section(manifest: &str, section: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == section;
            continue;
        }
        if !inside {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() == key {
            return Some(value.trim().trim_matches('"').to_owned());
        }
    }
    None
}

/// `path` relative to `base` when it's under it, for shorter log lines.
pub fn relative<'a>(path: &'a Path, base: &Path) -> &'a Path {
    path.strip_prefix(base).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_ported_programs_only() {
        let manifest = r#"[workspace]
members = [
    "programs/token/core",
    "programs/token",
    "programs/token/methods",
    "programs/nft/methods",
    "tools/idl-gen",
    # Not yet ported.
    # "programs/amm/methods",
]
exclude = [
    "programs/token/methods/guest",
]
"#;
        assert_eq!(workspace_methods(manifest), ["token", "nft"]);
    }

    #[test]
    fn reads_package_and_bin_names() {
        let guest = "[package]\nname = \"token-guest\"\nversion = \"0.1.0\"\n\n[workspace]\n\n[[bin]]\nname = \"token\"\npath = \"src/bin/token.rs\"\n";
        assert_eq!(package_name(guest).as_deref(), Some("token-guest"));
        assert_eq!(bin_name(guest).as_deref(), Some("token"));
        assert_eq!(bin_name("[package]\nname = \"x\"\n"), None);
    }

    #[test]
    fn finds_the_one_state_type() {
        assert_eq!(state_type("type GateState = Timestamps;\nstruct Timestamps {}").as_deref(), Some("Timestamps"));
        assert_eq!(state_type("pub struct TokenState { a: u8 }").as_deref(), Some("TokenState"));
        assert_eq!(state_type("enum State {").as_deref(), Some("State"));
        // Two states, or one that's a primitive: no single type to name.
        assert_eq!(state_type("type TreasuryState = Treasury;\ntype BalanceState = u128;"), None);
        assert_eq!(state_type("type BalanceState = u128;"), None);
        assert_eq!(state_type("fn main() {}"), None);
        // `StateReply` isn't a state.
        assert_eq!(state_type("struct StateReply {}"), None);
    }

    #[test]
    fn writes_the_jz_package_once() {
        let dir = std::env::temp_dir().join(format!("prog-tui-jz-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let programs = Programs::new(dir.clone());
        assert!(programs.write_jz_package().unwrap());
        assert!(!programs.write_jz_package().unwrap());
        assert!(programs.jz_manifest().is_file());
        assert!(programs.jz_src_dir().is_dir());
        let _ = fs::remove_dir_all(&dir);
    }
}
