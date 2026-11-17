//! The CLIs prog-tui runs besides LEZ's own `wallet`: `lez-call` (a guest's interface, its
//! messages encoded), `lez-dryrun` (a call planned with local dry runs) and `spel` (lez-programs'
//! programs, called from their IDLs). Each is used from its checkout's release build, else from
//! `PATH`.

use std::path::{Path, PathBuf};

pub struct Tool {
    pub name: &'static str,
    /// Its checkout, relative to this crate.
    dir: &'static str,
    /// What `cargo build --release` builds in the checkout: everything, or these args.
    pub build_args: &'static [&'static str],
}

/// `../lez-prog-call`, next to jz-progs.
pub const LEZ_CALL: Tool = Tool {
    name: "lez-call",
    dir: "../../lez-prog-call",
    build_args: &[],
};

/// `jz-progs/lez-dryrun`.
pub const LEZ_DRYRUN: Tool = Tool {
    name: "lez-dryrun",
    dir: "../lez-dryrun",
    build_args: &[],
};

/// `../spel`, next to jz-progs, on the branch that targets lez-programs' LEZ
/// (`refactor/lez-v0.4.x`). Only its CLI is built.
pub const SPEL: Tool = Tool {
    name: "spel",
    dir: "../../spel",
    build_args: &["--bin", "spel"],
};

impl Tool {
    pub fn checkout(&self) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(self.dir);
        dir.canonicalize().unwrap_or(dir)
    }

    pub fn has_checkout(&self) -> bool {
        self.checkout().join("Cargo.toml").is_file()
    }

    /// Its build in the checkout, else the one on `PATH`.
    pub fn path(&self) -> Option<PathBuf> {
        let built = self.checkout().join("target/release").join(self.name);
        if built.is_file() {
            return Some(built);
        }
        let paths = std::env::var_os("PATH")?;
        std::env::split_paths(&paths)
            .map(|dir| dir.join(self.name))
            .find(|path| path.is_file())
    }
}
