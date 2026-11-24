//! Snapshots of a game's save folders, taken before a cloud sync could
//! replace them (a download, an automatic or a forced sync), for every store
//! with cloud saves, so a sync that went the wrong way can be undone.
//!
//! Each snapshot is a copy of the game's save locations in
//! `<data>/save-snapshots/<runner>/<app>/<time>/`, with a `snapshot.json`
//! naming them and a SHA-256 of their contents. A sync that finds the saves
//! as the newest snapshot holds them takes none, and only the newest
//! `iroriSaveSnapshots` are kept (0 takes none). Restoring copies a
//! snapshot's files back over the save folders after snapshotting what is
//! there now, so a restore is undone the same way; it never deletes a file.
//! All of it reads and writes files: run it on a blocking thread.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::{GameId, Paths, json_store};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Saves larger than this aren't copied: a snapshot is for save games, and
/// a save folder this large is most likely a whole game folder by mistake.
const MAX_BYTES: u64 = 1024 * 1024 * 1024;

const MANIFEST: &str = "snapshot.json";

/// One save location in a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotLocation {
    /// GOG's location name; empty for a store's single save folder.
    pub name: String,
    /// Where the saves were (and go back to).
    pub path: String,
    /// The folder inside the snapshot holding them.
    pub folder: String,
}

/// A snapshot of a game's saves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveSnapshot {
    /// The snapshot's folder name, which identifies it.
    #[serde(skip)]
    pub id: String,
    /// RFC 3339.
    pub created: String,
    /// What it was taken before: `download`, `auto`, `force-download`,
    /// `force-upload` or `restore`.
    pub reason: String,
    pub bytes: u64,
    pub files: u64,
    /// SHA-256 of the locations' contents (paths and bytes).
    pub hash: String,
    pub locations: Vec<SnapshotLocation>,
}

/// The save locations a store's sync takes: one folder, or GOG's
/// `name=path` lines.
pub fn locations(saves: &str) -> Vec<(String, PathBuf)> {
    let lines: Vec<&str> = saves
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let named = lines.len() > 1 || lines.first().is_some_and(|l| is_named(l));
    lines
        .into_iter()
        .map(|line| match line.split_once('=').filter(|_| named) {
            Some((name, path)) => (
                name.trim().to_string(),
                irori_core::paths::expand_home(path.trim()),
            ),
            None => (String::new(), irori_core::paths::expand_home(line)),
        })
        .collect()
}

/// Whether a line is GOG's `name=path` rather than a path holding `=`.
fn is_named(line: &str) -> bool {
    line.split_once('=').is_some_and(|(name, path)| {
        !name.is_empty()
            && !name.contains(['/', '\\'])
            && (path.starts_with(['/', '~', '\\']) || path.get(1..3) == Some(":\\"))
    })
}

/// Where a game's snapshots are kept.
pub fn folder(id: &GameId) -> PathBuf {
    Paths::get()
        .data
        .join("save-snapshots")
        .join(id.runner.id())
        .join(irori_core::paths::sanitize(&id.app_name))
}

/// A game's snapshots, newest first.
pub fn list(id: &GameId) -> Vec<SaveSnapshot> {
    list_in(&folder(id))
}

fn list_in(dir: &Path) -> Vec<SaveSnapshot> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<SaveSnapshot> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let mut snapshot: SaveSnapshot =
                json_store::read(&e.path().join(MANIFEST)).ok().flatten()?;
            snapshot.id = e.file_name().to_string_lossy().into_owned();
            Some(snapshot)
        })
        .collect();
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

/// Every file under the locations, with the location it is in and its path
/// inside it, sorted so the hash doesn't depend on the order the disk lists.
fn files(locations: &[(String, PathBuf)]) -> Vec<(usize, PathBuf, PathBuf)> {
    let mut out = Vec::new();
    for (ix, (_, root)) in locations.iter().enumerate() {
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                // Links are left alone: they may point anywhere.
                if kind.is_dir() {
                    stack.push(entry.path());
                } else if kind.is_file()
                    && let Ok(rel) = entry.path().strip_prefix(root)
                {
                    out.push((ix, rel.to_path_buf(), entry.path()));
                }
            }
        }
    }
    out.sort();
    out
}

/// SHA-256 of the files' places and bytes, with their count and size.
fn fingerprint(files: &[(usize, PathBuf, PathBuf)]) -> Result<(String, u64)> {
    let mut hash = Sha256::new();
    let mut bytes = 0;
    let mut buf = vec![0u8; 64 * 1024];
    for (ix, rel, path) in files {
        hash.update(ix.to_le_bytes());
        hash.update(rel.to_string_lossy().replace('\\', "/").as_bytes());
        hash.update([0]);
        let mut file =
            std::fs::File::open(path).with_context(|| format!("Cannot read {}", path.display()))?;
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            bytes += n as u64;
            if bytes > MAX_BYTES {
                bail!("The saves are larger than a snapshot takes");
            }
            hash.update(&buf[..n]);
        }
        hash.update([0]);
    }
    Ok((hex::encode(hash.finalize()), bytes))
}

/// Snapshot `saves` (as the store's sync takes them) of `id` before a sync
/// for `reason`, unless the newest snapshot holds the same saves, then keep
/// the newest `keep`. Returns the snapshot taken.
pub fn take(id: &GameId, saves: &str, reason: &str, keep: u32) -> Result<Option<SaveSnapshot>> {
    take_in(&folder(id), saves, reason, keep)
}

fn take_in(dir: &Path, saves: &str, reason: &str, keep: u32) -> Result<Option<SaveSnapshot>> {
    if keep == 0 {
        return Ok(None);
    }
    let locations = locations(saves);
    let files = files(&locations);
    if files.is_empty() {
        return Ok(None);
    }
    let (hash, bytes) = fingerprint(&files)?;
    if list_in(dir).first().is_some_and(|s| s.hash == hash) {
        return Ok(None);
    }
    let created = irori_core::util::now_rfc3339();
    // Sortable as text and unique: the time to the microsecond (fixed
    // width), then a number in the unlikely case of two at once.
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%.6fZ").to_string();
    let mut id = stamp.clone();
    let mut n = 1;
    while dir.join(&id).exists() {
        n += 1;
        id = format!("{stamp}-{n}");
    }
    let partial = dir.join(format!(".{id}.partial"));
    let _ = std::fs::remove_dir_all(&partial);
    let snapshot = SaveSnapshot {
        id: id.clone(),
        created,
        reason: reason.to_string(),
        bytes,
        files: files.len() as u64,
        hash,
        locations: locations
            .iter()
            .enumerate()
            .map(|(ix, (name, path))| SnapshotLocation {
                name: name.clone(),
                path: path.to_string_lossy().into_owned(),
                folder: ix.to_string(),
            })
            .collect(),
    };
    for (ix, rel, path) in &files {
        let to = partial.join(ix.to_string()).join(rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Cannot create {}", parent.display()))?;
        }
        std::fs::copy(path, &to).with_context(|| format!("Cannot copy {}", path.display()))?;
    }
    json_store::write(&partial.join(MANIFEST), &snapshot)?;
    std::fs::rename(&partial, dir.join(&id)).context("Cannot keep the snapshot")?;
    prune(dir, keep);
    Ok(Some(snapshot))
}

/// Keep the newest `keep` snapshots.
fn prune(dir: &Path, keep: u32) {
    for old in list_in(dir).into_iter().skip(keep as usize) {
        let path = dir.join(&old.id);
        if let Err(e) = std::fs::remove_dir_all(&path) {
            tracing::warn!("removing save snapshot {}: {e}", path.display());
        }
    }
}

/// Copy snapshot `snapshot_id` of `id` back over its save folders, after
/// snapshotting what is there now (kept beside it as `restore`). Files the
/// snapshot doesn't hold stay. Returns how many files were put back.
pub fn restore(id: &GameId, snapshot_id: &str, keep: u32) -> Result<u64> {
    restore_in(&folder(id), snapshot_id, keep)
}

fn restore_in(dir: &Path, snapshot_id: &str, keep: u32) -> Result<u64> {
    if snapshot_id.is_empty() || snapshot_id.contains(['/', '\\']) || snapshot_id.starts_with('.') {
        bail!("That snapshot can’t be found");
    }
    let snapshot = list_in(dir)
        .into_iter()
        .find(|s| s.id == snapshot_id)
        .context("That snapshot can’t be found")?;
    let saves: Vec<String> = snapshot
        .locations
        .iter()
        .map(|l| {
            if l.name.is_empty() {
                l.path.clone()
            } else {
                format!("{}={}", l.name, l.path)
            }
        })
        .collect();
    // One more than the user keeps, so taking it never drops the snapshot
    // being restored.
    take_in(dir, &saves.join("\n"), "restore", keep.max(1) + 1)?;
    let root = dir.join(&snapshot.id);
    let mut restored = 0;
    for location in &snapshot.locations {
        let from = root.join(&location.folder);
        let to = PathBuf::from(&location.path);
        for (_, rel, path) in files(&[(String::new(), from)]) {
            let target = to.join(&rel);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Cannot create {}", parent.display()))?;
            }
            std::fs::copy(&path, &target)
                .with_context(|| format!("Cannot write {}", target.display()))?;
            restored += 1;
        }
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_locations_as_stores_take_them() {
        assert_eq!(
            locations("/saves/hades"),
            [(String::new(), PathBuf::from("/saves/hades"))]
        );
        assert_eq!(
            locations("saves=/a\nconfig=/b\n"),
            [
                ("saves".to_string(), PathBuf::from("/a")),
                ("config".to_string(), PathBuf::from("/b"))
            ]
        );
        assert_eq!(
            locations("/odd=folder"),
            [(String::new(), PathBuf::from("/odd=folder"))],
            "a path holding = is still one folder"
        );
        assert!(locations("  ").is_empty());
    }

    #[test]
    fn snapshots_skip_unchanged_saves_keep_the_newest_and_restore() {
        let tmp = tempfile::tempdir().unwrap();
        let saves = tmp.path().join("saves");
        let store = tmp.path().join("snapshots");
        std::fs::create_dir_all(saves.join("slot")).unwrap();
        std::fs::write(saves.join("slot/1.sav"), b"first").unwrap();
        let spec = saves.to_string_lossy().into_owned();

        assert!(
            take_in(&store, &spec, "download", 0).unwrap().is_none(),
            "off"
        );
        let first = take_in(&store, &spec, "download", 2).unwrap().unwrap();
        assert_eq!((first.files, first.bytes), (1, 5));
        assert!(
            take_in(&store, &spec, "auto", 2).unwrap().is_none(),
            "the same saves again"
        );

        std::fs::write(saves.join("slot/1.sav"), b"second").unwrap();
        take_in(&store, &spec, "download", 2).unwrap().unwrap();
        std::fs::write(saves.join("slot/1.sav"), b"third!").unwrap();
        std::fs::write(saves.join("new.sav"), b"x").unwrap();
        take_in(&store, &spec, "force-download", 2)
            .unwrap()
            .unwrap();
        let listed = list_in(&store);
        assert_eq!(listed.len(), 2, "only the newest two are kept");
        assert_eq!(listed[0].reason, "force-download");

        // The sync went wrong: back to "second".
        let second = &listed[1];
        std::fs::write(saves.join("slot/1.sav"), b"broken").unwrap();
        assert_eq!(restore_in(&store, &second.id, 2).unwrap(), 1);
        assert_eq!(std::fs::read(saves.join("slot/1.sav")).unwrap(), b"second");
        assert!(saves.join("new.sav").exists(), "nothing is deleted");
        let after = list_in(&store);
        assert_eq!(after[0].reason, "restore", "what was there is kept");
        assert!(after.iter().any(|s| s.id == second.id));

        assert!(restore_in(&store, "../x", 2).is_err());
        assert!(restore_in(&store, "missing", 2).is_err());
    }

    #[test]
    fn gog_locations_are_kept_apart() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("same.dat"), b"a").unwrap();
        std::fs::write(b.join("same.dat"), b"b").unwrap();
        let spec = format!("saves={}\nconfig={}", a.display(), b.display());
        let store = tmp.path().join("snapshots");
        let snap = take_in(&store, &spec, "download", 3).unwrap().unwrap();
        assert_eq!(snap.locations.len(), 2);
        std::fs::write(a.join("same.dat"), b"changed").unwrap();
        std::fs::write(b.join("same.dat"), b"changed").unwrap();
        restore_in(&store, &snap.id, 3).unwrap();
        assert_eq!(std::fs::read(a.join("same.dat")).unwrap(), b"a");
        assert_eq!(std::fs::read(b.join("same.dat")).unwrap(), b"b");
    }
}
