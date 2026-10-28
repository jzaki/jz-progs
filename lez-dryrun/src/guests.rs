//! Which local build a deployed program runs.
//!
//! A deployed program's header holds the `image_id` of its bytecode. Computing the image id of
//! each local build and matching it names the guest, and so its source, with no registry.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use lee_core::program::ProgramId;
use serde::{Deserialize, Serialize};

/// Image ids computed before, by guest name, kept next to the builds.
const CACHE: &str = "lez-dryrun-image-ids.json";

/// A directory of built guests (`<name>.bin`).
pub struct Guests {
    pub bin_dir: PathBuf,
}

#[derive(Clone, Deserialize, Serialize)]
struct Cached {
    size: u64,
    modified: u64,
    image_id: ProgramId,
}

impl Guests {
    /// The built guest whose image id is `image_id`.
    pub fn by_image_id(&self, image_id: ProgramId) -> anyhow::Result<Option<String>> {
        let cache_path = self.bin_dir.join(CACHE);
        let cache: BTreeMap<String, Cached> = fs::read_to_string(&cache_path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let mut fresh = BTreeMap::new();
        let mut found = None;
        for entry in fs::read_dir(&self.bin_dir)?.flatten() {
            let path = entry.path();
            let Some(name) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .filter(|_| path.extension().is_some_and(|ext| ext == "bin"))
            else {
                continue;
            };
            let metadata = entry.metadata()?;
            let modified = metadata
                .modified()?
                .duration_since(UNIX_EPOCH)
                .map_or(0, |since| since.as_secs());
            let cached = match cache.get(name) {
                Some(cached) if cached.size == metadata.len() && cached.modified == modified => {
                    cached.clone()
                }
                _ => Cached {
                    size: metadata.len(),
                    modified,
                    image_id: image_id_of(&path)?,
                },
            };
            if cached.image_id == image_id {
                found = Some(name.to_owned());
            }
            fresh.insert(name.to_owned(), cached);
        }
        // The cache only saves time; a build directory that can't take it still works.
        let _ignored = serde_json::to_string(&fresh).map(|text| fs::write(&cache_path, text));
        Ok(found)
    }
}

fn image_id_of(path: &Path) -> anyhow::Result<ProgramId> {
    let bytes = fs::read(path)?;
    let binary = risc0_binfmt::ProgramBinary::decode(&bytes)
        .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
    let digest = binary
        .compute_image_id()
        .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?;
    Ok(digest.into())
}
