//! What the download queue still has to write on each disk, against the space
//! free there, so the downloads page can say a disk won't hold the queue
//! before the download that fills it starts.

use std::path::{Path, PathBuf};

use irori_core::GameId;

use crate::Backend;
use crate::downloads::{DownloadItem, DownloadKind, DownloadState};

/// One disk the queue downloads to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueDisk {
    /// The first folder the queue installs to on this disk, to name it by.
    pub folder: PathBuf,
    /// Bytes the queue still downloads there.
    pub needed: u64,
    /// Bytes free there, when they could be read.
    pub free: Option<u64>,
}

impl QueueDisk {
    /// The disk holds less than the queue still downloads to it.
    pub fn is_short(&self) -> bool {
        self.free.is_some_and(|free| free < self.needed)
    }
}

/// The folder and the bytes each waiting or running install still downloads.
/// Updates (whose size stores tell only while they run), offline installers
/// and installs of unknown size need nothing that can be told. The running
/// item counts only what is left of it, from `percent_done`.
pub(crate) fn queue_needs(
    active: &[DownloadItem],
    percent_done: impl Fn(&GameId) -> Option<f32>,
) -> Vec<(PathBuf, u64)> {
    active
        .iter()
        .filter_map(|item| {
            let DownloadKind::Install(req) = &item.kind else {
                return None;
            };
            if item.download_size == 0 || req.installer.is_some() || req.path.as_os_str().is_empty()
            {
                return None;
            }
            let left = match item.state {
                DownloadState::Running | DownloadState::Paused => {
                    remaining(item.download_size, percent_done(&item.id))
                }
                _ => item.download_size,
            };
            Some((req.path.clone(), left))
        })
        .collect()
}

/// Bytes still to download of `total`, `percent` of it done.
fn remaining(total: u64, percent: Option<f32>) -> u64 {
    match percent {
        Some(p) if p > 0.0 => {
            let left = f64::from(100.0 - p.clamp(0.0, 100.0)) / 100.0;
            (total as f64 * left).round() as u64
        }
        _ => total,
    }
}

/// Add the needs up per disk, in the order the queue reaches each disk.
/// `disk_of` names the disk a folder is on (two folders on one disk share a
/// name) and the bytes free there; a folder whose disk can't be told is a
/// disk of its own with its free space unknown.
pub(crate) fn per_disk(
    needs: Vec<(PathBuf, u64)>,
    disk_of: impl Fn(&Path) -> Option<(String, Option<u64>)>,
) -> Vec<QueueDisk> {
    let mut disks: Vec<(String, QueueDisk)> = Vec::new();
    for (folder, bytes) in needs {
        let (key, free) = disk_of(&folder).unwrap_or_else(|| (folder.display().to_string(), None));
        match disks.iter_mut().find(|(k, _)| *k == key) {
            Some((_, disk)) => disk.needed = disk.needed.saturating_add(bytes),
            None => disks.push((
                key,
                QueueDisk {
                    folder,
                    needed: bytes,
                    free,
                },
            )),
        }
    }
    disks.into_iter().map(|(_, disk)| disk).collect()
}

/// The disk holding `path` (its nearest existing folder) and its free bytes.
fn this_disk(path: &Path) -> Option<(String, Option<u64>)> {
    let existing = path.ancestors().find(|p| p.is_dir())?;
    let free = fs4::available_space(existing).ok();
    Some((disk_key(existing)?, free))
}

/// What tells disks apart: the device on Unix, the drive or share elsewhere.
#[cfg(unix)]
fn disk_key(existing: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata(existing)
        .ok()
        .map(|m| m.dev().to_string())
}

#[cfg(not(unix))]
fn disk_key(existing: &Path) -> Option<String> {
    use std::path::Component;
    match existing.components().next()? {
        Component::Prefix(prefix) => {
            Some(prefix.as_os_str().to_string_lossy().to_ascii_uppercase())
        }
        _ => Some(existing.display().to_string()),
    }
}

impl Backend {
    /// What the queue still downloads to each disk, against the space free
    /// there. Reads the disks, so call it off the UI thread, and only when
    /// the queue changes.
    pub fn download_disks(&self) -> Vec<QueueDisk> {
        let active = self.downloads().active;
        let needs = queue_needs(&active, |id| {
            self.statuses
                .get(id)
                .and_then(|s| s.progress)
                .and_then(|p| p.percent)
        });
        per_disk(needs, this_disk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::{InstallRequest, Runner, UpdateRequest};

    fn install(app: &str, path: &str, size: u64, state: DownloadState) -> DownloadItem {
        DownloadItem {
            id: GameId::new(Runner::Gog, app),
            title: app.into(),
            kind: DownloadKind::Install(InstallRequest {
                path: PathBuf::from(path),
                ..Default::default()
            }),
            state,
            added_at: 0,
            started_at: None,
            finished_at: None,
            download_size: size,
            error: None,
            progress: None,
        }
    }

    #[test]
    fn needs_count_what_is_left_of_known_installs() {
        let mut update = install("u", "/games", 0, DownloadState::Queued);
        update.kind = DownloadKind::Update(UpdateRequest::default());
        let mut offline = install("o", "/games", 500, DownloadState::Queued);
        if let DownloadKind::Install(req) = &mut offline.kind {
            req.installer = Some(PathBuf::from("/setup.exe"));
        }
        let active = vec![
            install("a", "/games", 1000, DownloadState::Running),
            install("b", "/other", 300, DownloadState::Queued),
            install("c", "/games", 0, DownloadState::Queued),
            update,
            offline,
        ];
        let needs = queue_needs(&active, |id| (id.app_name.as_ref() == "a").then_some(25.0));
        assert_eq!(
            needs,
            vec![
                (PathBuf::from("/games"), 750),
                (PathBuf::from("/other"), 300),
            ]
        );
    }

    #[test]
    fn adds_up_per_disk_and_flags_short_ones() {
        let needs = vec![
            (PathBuf::from("/mnt/big/a"), 100),
            (PathBuf::from("/home/me/games"), 50),
            (PathBuf::from("/mnt/big/b"), 200),
            (PathBuf::from("/unknown"), 10),
        ];
        let disks = per_disk(needs, |p| {
            if p.starts_with("/mnt/big") {
                Some(("big".into(), Some(250)))
            } else if p.starts_with("/home") {
                Some(("home".into(), Some(1_000)))
            } else {
                None
            }
        });
        assert_eq!(disks.len(), 3);
        assert_eq!(disks[0].folder, PathBuf::from("/mnt/big/a"));
        assert_eq!(disks[0].needed, 300);
        assert!(disks[0].is_short(), "300 needed, 250 free");
        assert!(!disks[1].is_short());
        assert_eq!(disks[2].free, None);
        assert!(!disks[2].is_short(), "unknown free space is never short");
    }

    #[test]
    fn reads_a_real_folder() {
        let dir = tempfile::tempdir().unwrap();
        let (key, free) = this_disk(&dir.path().join("not/yet/made")).unwrap();
        assert!(!key.is_empty());
        assert!(free.is_some());
    }
}
