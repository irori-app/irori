//! File helpers shared by installs and deployment: walking a mod's files,
//! matching the game folder's letter case, moving files across folders.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};

/// Regular files under `dir` as `/`-separated relative paths with their
/// sizes, sorted. Links, special files and overlay deletion markers are
/// skipped: a mod is plain files.
pub(crate) fn walk_files(dir: &Path) -> Result<Vec<(String, u64)>> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, u64)>) -> Result<()> {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
        };
        for entry in entries {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_dir() {
                walk(root, &path, out)?;
            } else if kind.is_file()
                && !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(crate::overlay::WHITEOUT_PREFIX)
            {
                let rel = path.strip_prefix(root).unwrap_or(&path);
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                out.push((rel_string(rel), size));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    out.sort();
    Ok(out)
}

/// `rel` with `/` separators on every platform.
pub(crate) fn rel_string(rel: &Path) -> String {
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
}

/// `root` joined with a `/`-separated relative path.
pub(crate) fn join_rel(root: &Path, rel: &str) -> PathBuf {
    let mut out = root.to_path_buf();
    for part in rel.split('/').filter(|p| !p.is_empty()) {
        out.push(part);
    }
    out
}

/// `rel` spelled the way the game folder already spells its folders and
/// files. Windows games don't care about letter case, but Linux does: a mod
/// shipping `bepinex/Plugins` must land in the game's `BepInEx/plugins`, not
/// beside it. [`CaseAligner`] spells many paths, listing each folder once.
pub fn align_case(game_dir: &Path, rel: &str) -> String {
    CaseAligner::new(game_dir).align(rel)
}

/// Spells paths the way the game folder does, listing each folder once.
///
/// The spelling comes from the folder's listing, never from whether a path
/// opens: on a case-insensitive file system (NTFS, APFS) `bepinex` opens the
/// game's `BepInEx`, and keeping the mod's spelling would rename the game's
/// own files when a replaced one is put back.
pub struct CaseAligner {
    game_dir: PathBuf,
    /// Folder → (exact names, lower-cased name → first name with it); `None`
    /// for a folder that can't be listed.
    folders: HashMap<PathBuf, Option<Names>>,
}

struct Names {
    exact: HashSet<String>,
    lower: HashMap<String, String>,
}

impl CaseAligner {
    pub fn new(game_dir: &Path) -> Self {
        Self {
            game_dir: game_dir.to_path_buf(),
            folders: HashMap::new(),
        }
    }

    /// `rel` with every part the game folder has spelled as it spells it;
    /// parts below the first one it lacks stay as they are.
    pub fn align(&mut self, rel: &str) -> String {
        let mut dir = self.game_dir.clone();
        let mut out: Vec<String> = Vec::new();
        let mut exists = true;
        for part in rel.split('/').filter(|p| !p.is_empty()) {
            let mut name = part.to_string();
            if exists {
                match self.spelling(&dir, part) {
                    Some(found) => {
                        name = found;
                        dir.push(&name);
                    }
                    None => exists = false,
                }
            }
            out.push(name);
        }
        out.join("/")
    }

    fn spelling(&mut self, dir: &Path, name: &str) -> Option<String> {
        let names = self
            .folders
            .entry(dir.to_path_buf())
            .or_insert_with(|| list_names(dir))
            .as_ref()?;
        if names.exact.contains(name) {
            return Some(name.to_string());
        }
        names.lower.get(&name.to_lowercase()).cloned()
    }
}

fn list_names(dir: &Path) -> Option<Names> {
    let mut names = Names {
        exact: HashSet::new(),
        lower: HashMap::new(),
    };
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        names
            .lower
            .entry(name.to_lowercase())
            .or_insert_with(|| name.clone());
        names.exact.insert(name);
    }
    Some(names)
}

/// Suffix of a copy being written, renamed into place once complete.
pub(crate) const PARTIAL_SUFFIX: &str = ".irori-partial";

/// How long a file in use may keep Irori waiting.
pub(crate) const LOCK_BUDGET: Duration = Duration::from_secs(5);

/// Whether `e` says another program has the file open: Windows' sharing
/// and lock violations (a game that hasn't quite exited, an antivirus
/// scan, an indexer), or a busy file elsewhere. Worth waiting for, unlike
/// a missing file or a permission.
pub(crate) fn is_locked(e: &std::io::Error) -> bool {
    match e.raw_os_error() {
        // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
        Some(32 | 33) if cfg!(windows) => true,
        // EBUSY, ETXTBSY.
        Some(16 | 26) if cfg!(unix) => true,
        _ => false,
    }
}

/// Run `op` again while it fails because the file is in use, waiting a
/// little longer each time, until `budget` is spent.
pub(crate) fn retry_locked<T>(
    budget: Duration,
    mut op: impl FnMut() -> std::io::Result<T>,
) -> std::io::Result<T> {
    let start = Instant::now();
    let mut wait = Duration::from_millis(50);
    loop {
        match op() {
            Err(e) if is_locked(&e) && start.elapsed() + wait <= budget => {
                std::thread::sleep(wait);
                wait = (wait * 2).min(Duration::from_secs(1));
            }
            result => return result,
        }
    }
}

/// Copy `from` to `to` so that `to` is either the whole file or untouched:
/// the copy goes to a `.irori-partial` sibling first and is renamed into
/// place, so a copy cut short (a full disk, a crash) never leaves half a
/// file where a whole one is expected.
pub(crate) fn copy_whole(from: &Path, to: &Path) -> Result<()> {
    let mut name = to.file_name().unwrap_or_default().to_os_string();
    name.push(PARTIAL_SUFFIX);
    let partial = to.with_file_name(name);
    let copied = retry_locked(LOCK_BUDGET, || std::fs::copy(from, &partial))
        .with_context(|| format!("copying {} to {}", from.display(), to.display()))
        .and_then(|_| {
            retry_locked(LOCK_BUDGET, || std::fs::rename(&partial, to))
                .with_context(|| format!("putting {} in place", to.display()))
        });
    if copied.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    copied
}

/// Move the file `from` to `to`, creating `to`'s folder and replacing a file
/// there. Renames when both are on one file system, else copies (whole, see
/// [`copy_whole`]) and removes the source. Waits a while for files in use.
pub(crate) fn move_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    match retry_locked(LOCK_BUDGET, || std::fs::rename(from, to)) {
        Ok(()) => return Ok(()),
        Err(e) if is_locked(&e) => {
            return Err(e).with_context(|| format!("{} is in use", from.display()));
        }
        Err(_) => {}
    }
    copy_whole(from, to)?;
    retry_locked(LOCK_BUDGET, || std::fs::remove_file(from))
        .with_context(|| format!("removing {}", from.display()))
}

/// Nanoseconds since the epoch of `path`'s last change, when known.
pub(crate) fn modified_ns(meta: &std::fs::Metadata) -> Option<u64> {
    meta.modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_nanos()).ok())
}

/// Whether `a` and `b` are the same file on disk (hard links of each other).
#[cfg(unix)]
pub(crate) fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    a.dev() == b.dev() && a.ino() == b.ino()
}

/// Windows exposes no file id through std's stable API; hard links share
/// their metadata, so equal size and change time is the best sign.
#[cfg(not(unix))]
pub(crate) fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    a.len() == b.len() && modified_ns(a) == modified_ns(b)
}

/// The path under `root` matching the `/`- or `\`-separated `rel` without
/// letter case, when it exists. Archives made on Linux may hold both
/// `Textures` and `textures`; every spelling is tried. `None` for paths
/// that leave `root`.
pub(crate) fn find_path_ci(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.replace('\\', "/");
    let clean = irori_core::archive_guard::relative_entry_path(Path::new(&rel))?;
    let parts: Vec<String> = clean
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    fn search(dir: &Path, parts: &[String]) -> Option<PathBuf> {
        let Some((first, rest)) = parts.split_first() else {
            return Some(dir.to_path_buf());
        };
        let exact = dir.join(first);
        if exact.symlink_metadata().is_ok()
            && let Some(found) = search(&exact, rest)
        {
            return Some(found);
        }
        let lower = first.to_lowercase();
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != first && n.to_lowercase() == lower)
            .collect();
        names.sort();
        names.iter().find_map(|n| search(&dir.join(n), rest))
    }
    search(root, &parts)
}
