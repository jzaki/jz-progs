//! The two CLIs prog-tui runs besides LEZ's own `wallet`: `lez-call` (a guest's interface, its
//! messages encoded) and `lez-dryrun` (a call planned with local dry runs). Each is used from its
//! checkout's release build, else from `PATH`.

use std::path::{Path, PathBuf};

pub struct Tool {
    pub name: &'static str,
    /// Its checkout, relative to this crate.
    dir: &'static str,
}

/// `../lez-call`, next to jz-progs.
pub const LEZ_CALL: Tool = Tool {
    name: "lez-call",
    dir: "../../lez-call",
};

/// `jz-progs/lez-dryrun`.
pub const LEZ_DRYRUN: Tool = Tool {
    name: "lez-dryrun",
    dir: "../lez-dryrun",
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
