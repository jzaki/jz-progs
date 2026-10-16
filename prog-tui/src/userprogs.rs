//! Your own guest programs, in `jz-progs/example_progs/`.
//!
//! They build with LEZ's own example guest package, so they get its dependencies (`lee_core`,
//! `borsh`, `risc0-zkvm`), lockfile and toolchain with nothing new to install. Each
//! `example_progs/<name>.rs` is symlinked into that package's `src/bin/` as `jz_<name>.rs`, which
//! makes it a guest named `jz_<name>`. The links are untracked files in the LEZ checkout.

use std::{
    fs, io,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, bail};

/// Marks the guests that come from example_progs, and keeps them apart from LEZ's own.
pub const PREFIX: &str = "jz_";

/// `jz-progs/example_progs`, next to this crate.
pub fn example_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../example_progs");
    fs::canonicalize(&dir).unwrap_or(dir)
}

/// A path as typed: absolute, `~/…`, or relative to `example_progs`.
pub fn resolve(text: &str) -> PathBuf {
    let text = text.trim();
    match (text.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ if Path::new(text).is_absolute() => PathBuf::from(text),
        _ => example_dir().join(text),
    }
}

/// The guest name for a source file: `jz_<stem>`. The stem must be a plain binary name.
pub fn guest_name(file: &Path) -> anyhow::Result<String> {
    if file.extension().and_then(|ext| ext.to_str()) != Some("rs") {
        bail!("{} isn't a .rs file", file.display());
    }
    let stem = file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    if stem.is_empty()
        || !stem
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        bail!("{stem:?} must be lowercase letters, digits and underscores, to be a guest name");
    }
    Ok(format!("{PREFIX}{stem}"))
}

/// What [`link`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Linked {
    Created,
    Replaced,
    Unchanged,
}

/// Symlinks `file` into `guest_src` as `jz_<stem>.rs`. Returns the guest name. Never replaces a
/// regular file, only an earlier link.
pub fn link(file: &Path, guest_src: &Path) -> anyhow::Result<(String, Linked)> {
    let name = guest_name(file)?;
    let target = fs::canonicalize(file).with_context(|| format!("no file at {}", file.display()))?;
    let dest = guest_src.join(format!("{name}.rs"));
    let linked = match fs::symlink_metadata(&dest) {
        Ok(meta) if meta.file_type().is_symlink() => {
            if fs::read_link(&dest)? == target {
                return Ok((name, Linked::Unchanged));
            }
            fs::remove_file(&dest)?;
            Linked::Replaced
        }
        Ok(_) => bail!(
            "{} is a regular file in LEZ, not a link prog-tui made; not touching it",
            dest.display()
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Linked::Created,
        Err(error) => return Err(error.into()),
    };
    symlink(&target, &dest)
        .with_context(|| format!("linking {} to {}", dest.display(), target.display()))?;
    Ok((name, linked))
}

/// Every `.rs` in `dir`, sorted.
pub fn sources(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rs") && path.is_file())
        .collect();
    files.sort();
    files
}

/// Removes `jz_*.rs` links in `guest_src` whose source is gone. Returns their names. Only ever
/// removes symlinks.
pub fn prune(guest_src: &Path) -> Vec<String> {
    let mut removed = Vec::new();
    for entry in fs::read_dir(guest_src).into_iter().flatten().flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let ours = file_name.starts_with(PREFIX)
            && Path::new(file_name).extension().is_some_and(|ext| ext == "rs");
        let is_link = fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink());
        let dangling = fs::metadata(&path).is_err();
        if ours && is_link && dangling && fs::remove_file(&path).is_ok() {
            removed.push(file_name.trim_end_matches(".rs").to_owned());
        }
    }
    removed.sort();
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("prog-tui-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("progs")).unwrap();
        fs::create_dir_all(dir.join("bin")).unwrap();
        dir
    }

    #[test]
    fn names_guests() {
        assert_eq!(guest_name(Path::new("/x/hello_world.rs")).unwrap(), "jz_hello_world");
        assert!(guest_name(Path::new("/x/Hello.rs")).is_err());
        assert!(guest_name(Path::new("/x/my-prog.rs")).is_err());
        assert!(guest_name(Path::new("/x/hello_world.txt")).is_err());
    }

    #[test]
    fn links_replaces_and_refuses() {
        let dir = scratch("link");
        let (progs, bin) = (dir.join("progs"), dir.join("bin"));
        fs::write(progs.join("hello.rs"), "fn main() {}").unwrap();

        assert_eq!(
            link(&progs.join("hello.rs"), &bin).unwrap(),
            ("jz_hello".to_owned(), Linked::Created)
        );
        assert_eq!(fs::read_to_string(bin.join("jz_hello.rs")).unwrap(), "fn main() {}");
        assert_eq!(link(&progs.join("hello.rs"), &bin).unwrap().1, Linked::Unchanged);

        // Same name from somewhere else: the old link is replaced.
        fs::create_dir_all(dir.join("other")).unwrap();
        fs::write(dir.join("other/hello.rs"), "// other").unwrap();
        assert_eq!(link(&dir.join("other/hello.rs"), &bin).unwrap().1, Linked::Replaced);
        assert_eq!(fs::read_to_string(bin.join("jz_hello.rs")).unwrap(), "// other");

        // A regular file with that name is never touched.
        fs::write(bin.join("jz_real.rs"), "// LEZ's").unwrap();
        fs::write(progs.join("real.rs"), "// mine").unwrap();
        assert!(link(&progs.join("real.rs"), &bin).is_err());
        assert_eq!(fs::read_to_string(bin.join("jz_real.rs")).unwrap(), "// LEZ's");

        assert!(link(&progs.join("missing.rs"), &bin).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn prunes_only_dangling_links_of_ours() {
        let dir = scratch("prune");
        let (progs, bin) = (dir.join("progs"), dir.join("bin"));
        fs::write(progs.join("keep.rs"), "").unwrap();
        fs::write(progs.join("gone.rs"), "").unwrap();
        link(&progs.join("keep.rs"), &bin).unwrap();
        link(&progs.join("gone.rs"), &bin).unwrap();
        fs::remove_file(progs.join("gone.rs")).unwrap();
        // Not ours: a dangling link without the prefix, and a regular jz_ file.
        symlink(dir.join("nowhere.rs"), bin.join("other.rs")).unwrap();
        fs::write(bin.join("jz_file.rs"), "").unwrap();

        assert_eq!(prune(&bin), ["jz_gone"]);
        assert!(bin.join("jz_keep.rs").exists());
        assert!(fs::symlink_metadata(bin.join("other.rs")).is_ok());
        assert!(bin.join("jz_file.rs").exists());
        assert_eq!(sources(&progs), [progs.join("keep.rs")]);
        let _ = fs::remove_dir_all(&dir);
    }
}
