//! The size cap of the art cache on disk (`iroriArtCacheLimit`, in MiB; 0 =
//! no cap): the downloaded originals (`<cache>/images/orig`) and the
//! thumbnails and backdrops cut from them (`<cache>/images/thumbs`), all of
//! which the art store downloads or cuts again when they are shown.
//!
//! Files are grouped by the art they belong to (the digest of its address,
//! which starts every file name), and whole groups go, least recently used
//! first, until the cache fits. Art shown since Irori started is never
//! removed: the art store notes it ([`note_used`]) and each trim first dates
//! its files to now, so "recently used" survives restarts without touching a
//! file on the UI thread. The trim rides the backend's periodic tick, at most
//! every few hours; nothing runs for it otherwise.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use once_cell::sync::Lazy;
use parking_lot::Mutex;

use crate::Backend;

/// How often the cache is measured and trimmed.
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// How long after start the first trim waits, so it never competes with the
/// first screens' art.
const FIRST_AFTER: Duration = Duration::from_secs(10 * 60);

/// Digests of the art shown since Irori started.
static IN_USE: Lazy<Mutex<HashSet<String>>> = Lazy::new(Default::default);
/// When Irori started, then when the cache was last trimmed.
static LAST: Lazy<Mutex<Instant>> = Lazy::new(|| Mutex::new(Instant::now()));
/// Whether a trim has run since start.
static TRIMMED: Mutex<bool> = parking_lot::const_mutex(false);

/// Note that the art with this digest (`irori_core::paths::art_digest`) is
/// shown, so a trim keeps it. Cheap; called by the art store as art becomes
/// ready.
pub fn note_used(digest: &str) {
    let mut used = IN_USE.lock();
    if !used.contains(digest) {
        used.insert(digest.to_string());
    }
}

/// The art that belongs to one address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtGroup {
    pub digest: String,
    pub files: Vec<PathBuf>,
    pub bytes: u64,
    /// The latest a file of it was written or touched.
    pub used: SystemTime,
}

/// What a trim did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrimReport {
    pub before: u64,
    pub removed_bytes: u64,
    pub removed_groups: usize,
}

/// The digest a cache file belongs to: its name up to the first `-` or `.`
/// (`<digest>`, `<digest>-480.jpg`, `<digest>-ambient.rgb`). `None` for a
/// name that isn't one of the art store's (a `.tmp` sibling, a stray file).
fn digest_of(name: &str) -> Option<&str> {
    let digest = name.split(['-', '.']).next()?;
    (digest.len() == 24 && digest.bytes().all(|b| b.is_ascii_hexdigit())).then_some(digest)
}

/// The art store's files under `images` (its `orig` and `thumbs` folders),
/// grouped by the art they belong to. Reads the disk.
pub fn groups(images: &Path) -> Vec<ArtGroup> {
    let mut by_digest: HashMap<String, ArtGroup> = HashMap::new();
    for folder in ["orig", "thumbs"] {
        let Ok(entries) = std::fs::read_dir(images.join(folder)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(digest) = name.to_str().and_then(digest_of) else {
                continue;
            };
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let used = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let group = by_digest
                .entry(digest.to_string())
                .or_insert_with(|| ArtGroup {
                    digest: digest.to_string(),
                    files: Vec::new(),
                    bytes: 0,
                    used: SystemTime::UNIX_EPOCH,
                });
            group.files.push(entry.path());
            group.bytes += meta.len();
            group.used = group.used.max(used);
        }
    }
    by_digest.into_values().collect()
}

/// The groups to remove so the rest fits in `limit` bytes: least recently
/// used first, never one in `keep`. When the art in use alone is over the
/// limit, every other group goes and the cache stays over it.
pub fn plan(groups: &[ArtGroup], limit: u64, keep: &HashSet<String>) -> Vec<usize> {
    let mut total: u64 = groups.iter().map(|g| g.bytes).sum();
    if total <= limit {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..groups.len())
        .filter(|i| !keep.contains(&groups[*i].digest))
        .collect();
    order.sort_by(|a, b| {
        let (a, b) = (&groups[*a], &groups[*b]);
        a.used.cmp(&b.used).then_with(|| b.bytes.cmp(&a.bytes))
    });
    let mut out = Vec::new();
    for i in order {
        if total <= limit {
            break;
        }
        total -= groups[i].bytes;
        out.push(i);
    }
    out
}

/// Date the files of the art in `keep` to now, then remove the least
/// recently used art until the cache under `images` fits in `limit` bytes.
pub fn trim(images: &Path, limit: u64, keep: &HashSet<String>) -> TrimReport {
    let mut found = groups(images);
    let now = SystemTime::now();
    for group in found.iter_mut().filter(|g| keep.contains(&g.digest)) {
        for file in &group.files {
            let touched = std::fs::File::options()
                .write(true)
                .open(file)
                .and_then(|f| f.set_modified(now));
            if let Err(e) = touched {
                tracing::debug!("dating {}: {e}", file.display());
            }
        }
        group.used = now;
    }
    let mut report = TrimReport {
        before: found.iter().map(|g| g.bytes).sum(),
        ..Default::default()
    };
    for i in plan(&found, limit, keep) {
        let group = &found[i];
        for file in &group.files {
            match std::fs::remove_file(file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    tracing::debug!("removing {}: {e}", file.display())
                }
                _ => {}
            }
        }
        report.removed_bytes += group.bytes;
        report.removed_groups += 1;
    }
    report
}

/// The cap in bytes, `None` for no cap.
pub fn limit_bytes(mib: u32) -> Option<u64> {
    (mib > 0).then(|| u64::from(mib) * 1024 * 1024)
}

impl Backend {
    /// Trim the art cache to its cap, when one is set and a trim is due.
    /// Runs on the periodic tick.
    pub(crate) async fn trim_art_cache_when_due(&self) {
        let Some(limit) = limit_bytes(self.settings.app().art_cache_limit) else {
            return;
        };
        {
            let mut last = LAST.lock();
            let mut trimmed = TRIMMED.lock();
            let wait = if *trimmed { EVERY } else { FIRST_AFTER };
            if last.elapsed() < wait {
                return;
            }
            *last = Instant::now();
            *trimmed = true;
        }
        let keep = IN_USE.lock().clone();
        let images = irori_core::Paths::get().images_dir();
        let result = self
            .spawn_blocking(move || trim(&images, limit, &keep))
            .await;
        match result {
            Ok(report) if report.removed_groups > 0 => tracing::info!(
                "art cache: removed {} images ({} bytes) of {} bytes",
                report.removed_groups,
                report.removed_bytes,
                report.before
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!("trimming the art cache: {e:#}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbb";
    const C: &str = "cccccccccccccccccccccccc";

    fn write(path: &Path, bytes: usize, age_hours: u64) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![0u8; bytes]).unwrap();
        let when = SystemTime::now() - Duration::from_secs(age_hours * 3600);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    #[test]
    fn reads_the_art_stores_file_names() {
        assert_eq!(digest_of(A), Some(A));
        assert_eq!(digest_of(&format!("{A}-480.jpg")), Some(A));
        assert_eq!(digest_of(&format!("{A}-ambient.rgb")), Some(A));
        assert_eq!(digest_of(&format!("{A}.tmp")), Some(A));
        assert_eq!(digest_of("notes.txt"), None);
        assert_eq!(digest_of("zzzzzzzzzzzzzzzzzzzzzzzz-480.jpg"), None);
    }

    #[test]
    fn removes_the_least_recently_used_art_first_but_never_art_in_use() {
        let g = |digest: &str, bytes, hours: u64| ArtGroup {
            digest: digest.into(),
            files: Vec::new(),
            bytes,
            used: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 - hours * 3600),
        };
        let groups = [g(A, 100, 1), g(B, 100, 50), g(C, 100, 10)];
        let none = HashSet::new();
        assert!(plan(&groups, 300, &none).is_empty());
        // B is the oldest, then C.
        assert_eq!(plan(&groups, 200, &none), [1]);
        assert_eq!(plan(&groups, 150, &none), [1, 2]);
        // B is shown now: C goes instead, then A.
        let keep: HashSet<String> = [B.to_string()].into();
        assert_eq!(plan(&groups, 200, &keep), [2]);
        assert_eq!(plan(&groups, 0, &keep), [2, 0]);
        assert_eq!(limit_bytes(0), None);
        assert_eq!(limit_bytes(2), Some(2 * 1024 * 1024));
    }

    #[test]
    fn trims_whole_groups_on_disk_and_keeps_what_is_shown() {
        let dir = tempfile::tempdir().unwrap();
        let images = dir.path();
        write(&images.join("orig").join(A), 400, 100);
        write(
            &images.join("thumbs").join(format!("{A}-480.jpg")),
            100,
            100,
        );
        write(&images.join("orig").join(B), 400, 200);
        write(&images.join("thumbs").join(format!("{B}-480.jpg")), 100, 1);
        write(&images.join("orig").join(C), 400, 300);
        write(&images.join("thumbs").join("stray.txt"), 10, 500);

        // C is the oldest but shown now; B was used an hour ago. A goes.
        let keep: HashSet<String> = [C.to_string()].into();
        let report = trim(images, 1000, &keep);
        assert_eq!(report.before, 1400);
        assert_eq!((report.removed_groups, report.removed_bytes), (1, 500));
        assert!(!images.join("orig").join(A).exists());
        assert!(!images.join("thumbs").join(format!("{A}-480.jpg")).exists());
        assert!(images.join("orig").join(B).exists());
        assert!(images.join("orig").join(C).exists());
        assert!(images.join("thumbs").join("stray.txt").exists());
        // C was dated to now, so it is the newest from here on.
        let c = groups(images).into_iter().find(|g| g.digest == C).unwrap();
        assert!(c.used.elapsed().unwrap() < Duration::from_secs(60));

        // Under the cap, nothing goes.
        assert_eq!(trim(images, 10_000, &HashSet::new()).removed_groups, 0);
    }
}
