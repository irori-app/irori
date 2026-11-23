//! Undoing a restore: before Ludusavi writes, the files it will replace are
//! copied to `<data>/save-restore-undo/<game>/<time>/`, with a list of every
//! file the restore writes. Undo puts the copies back and removes the files
//! the restore created. A restore stopped part-way keeps its copy, so it can
//! still be undone. The newest [`KEEP`] copies of a game are kept.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

/// Copies kept per game.
pub const KEEP: usize = 3;
const LIST: &str = "undo.json";

/// One file a restore writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    path: PathBuf,
    /// The copy of what was there, inside the undo folder; `None` when the
    /// restore created the file.
    saved: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct List {
    /// RFC 3339 time of the restore.
    when: String,
    /// The backup restored.
    backup: String,
    entries: Vec<Entry>,
}

/// The undo folders of a game, newest first.
fn copies(game_dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(game_dir)
        .map(|e| {
            e.flatten()
                .map(|e| e.path())
                .filter(|p| p.join(LIST).is_file())
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    dirs.reverse();
    dirs
}

/// Copy what `targets` hold now into a new undo folder under `game_dir`,
/// named `stamp` (sortable: `20261118T101500Z`). Older copies past
/// [`KEEP`] are removed.
pub fn save(
    game_dir: &Path,
    stamp: &str,
    when: &str,
    backup: &str,
    targets: &[PathBuf],
) -> Result<PathBuf> {
    let dir = game_dir.join(stamp);
    let files = dir.join("files");
    std::fs::create_dir_all(&files)
        .with_context(|| format!("Cannot create {}", files.display()))?;
    let mut list = List {
        when: when.to_string(),
        backup: backup.to_string(),
        entries: Vec::new(),
    };
    for (i, path) in targets.iter().enumerate() {
        let saved = if path.is_file() {
            let name = i.to_string();
            std::fs::copy(path, files.join(&name))
                .with_context(|| format!("Cannot keep a copy of {}", path.display()))?;
            Some(name)
        } else {
            None
        };
        list.entries.push(Entry {
            path: path.clone(),
            saved,
        });
    }
    irori_core::json_store::write(&dir.join(LIST), &list)?;
    for old in copies(game_dir).into_iter().skip(KEEP) {
        if let Err(e) = std::fs::remove_dir_all(&old) {
            tracing::warn!("removing {}: {e}", old.display());
        }
    }
    Ok(dir)
}

/// The newest restore of a game that can be undone: its time and backup.
pub fn latest(game_dir: &Path) -> Option<(String, String)> {
    let dir = copies(game_dir).into_iter().next()?;
    let list: List = irori_core::json_store::read(&dir.join(LIST)).ok()??;
    Some((list.when, list.backup))
}

/// Undo the newest restore: put the replaced files back, remove the ones
/// it created, then forget the copy. Returns how many files changed.
pub fn undo(game_dir: &Path) -> Result<usize> {
    let dir = copies(game_dir)
        .into_iter()
        .next()
        .context("There is no restore to undo")?;
    let list: List = irori_core::json_store::read(&dir.join(LIST))?
        .context("The copy kept before the restore is missing")?;
    let mut changed = 0;
    for entry in &list.entries {
        match &entry.saved {
            Some(name) => {
                if let Some(parent) = entry.path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(dir.join("files").join(name), &entry.path)
                    .with_context(|| format!("Cannot put back {}", entry.path.display()))?;
                changed += 1;
            }
            None => match std::fs::remove_file(&entry.path) {
                Ok(()) => changed += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("Cannot remove {}", entry.path.display()));
                }
            },
        }
    }
    std::fs::remove_dir_all(&dir).with_context(|| format!("Cannot remove {}", dir.display()))?;
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn puts_replaced_files_back_and_removes_created_ones() {
        let dir = tempfile::tempdir().unwrap();
        let undo_dir = dir.path().join("undo");
        let saves = dir.path().join("saves");
        std::fs::create_dir_all(&saves).unwrap();
        let old = saves.join("slot1.sav");
        let new = saves.join("deep/slot2.sav");
        std::fs::write(&old, b"mine").unwrap();
        assert_eq!(latest(&undo_dir), None);

        save(
            &undo_dir,
            "20261118T100000Z",
            "2026-11-18T10:00:00Z",
            "backup-1",
            &[old.clone(), new.clone()],
        )
        .unwrap();
        // The restore writes.
        std::fs::write(&old, b"restored").unwrap();
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        std::fs::write(&new, b"restored").unwrap();
        assert_eq!(
            latest(&undo_dir),
            Some(("2026-11-18T10:00:00Z".into(), "backup-1".into()))
        );

        assert_eq!(undo(&undo_dir).unwrap(), 2);
        assert_eq!(std::fs::read(&old).unwrap(), b"mine");
        assert!(!new.exists());
        assert_eq!(latest(&undo_dir), None);
        assert!(undo(&undo_dir).is_err());
    }

    #[test]
    fn keeps_the_newest_copies() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            save(
                dir.path(),
                &format!("2026111{i}T000000Z"),
                &format!("t{i}"),
                "b",
                &[],
            )
            .unwrap();
        }
        assert_eq!(copies(dir.path()).len(), KEEP);
        assert_eq!(latest(dir.path()).unwrap().0, "t4");
    }
}
