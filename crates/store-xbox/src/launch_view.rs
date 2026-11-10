//! The launch view: a private folder that mirrors an installed game for one
//! session, with every encrypted program and library decrypted into it and
//! everything else linked to the install folder.
//!
//! Microsoft Store packages keep their executables encrypted on disk, as on
//! Windows, where the filesystem decrypts them on read. Xodus hands Wine the
//! decrypted images through memory file descriptors and a patched loader;
//! Irori writes them as real files into a folder only the user can read
//! (mode 0700) beside the game, so any Wine with `xgameruntime.dll` runs
//! them, and removes the folder when the session ends. Package paths are
//! resolved without letter case, never through `..` or links leaving the
//! install folder.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};

use irori_msixvc::xvd::{SegmentFile, XvdFile};

/// Name prefix of launch views (beside the install folder or in `/tmp`).
const VIEW_PREFIX: &str = ".irori-xbox-launch-";
/// Held while a session uses the view; a sweep keeps locked views.
const LOCK_FILE: &str = ".irori-xbox-view.lock";

/// A game's launch view; dropping it removes the folder.
pub struct LaunchView {
    dir: tempfile::TempDir,
    _lock: File,
    entry: PathBuf,
}

impl LaunchView {
    /// The program to start.
    pub fn entry(&self) -> &Path {
        &self.entry
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }
}

/// A package-relative path, refusing parent steps, roots and drive letters.
pub fn checked_relative_path(name: &str) -> io::Result<PathBuf> {
    let mut relative = PathBuf::new();
    for component in name.split(['\\', '/']).filter(|c| !c.is_empty()) {
        if component == "." || component == ".." || component.contains(':') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsafe package path: {name}"),
            ));
        }
        relative.push(component);
    }
    if relative.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "empty package path",
        ));
    }
    Ok(relative)
}

/// The file named `name` (package spelling, any letter case) under `root`,
/// relative to it. An exact match wins; several case-insensitive matches,
/// a link leading out of `root` or a non-file are refused.
pub fn resolve_package_file(root: &Path, name: &str) -> io::Result<PathBuf> {
    let relative = checked_relative_path(name)?;
    let base = root.canonicalize()?;
    let mut current = base.clone();
    for component in relative.components() {
        let component = component.as_os_str();
        let entries = std::fs::read_dir(&current)?.collect::<io::Result<Vec<_>>>()?;
        let exact = entries.iter().find(|e| e.file_name() == component);
        let matches: Vec<_> = match exact {
            Some(exact) => vec![exact],
            None => entries
                .iter()
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .zip(component.to_str())
                        .is_some_and(|(actual, wanted)| actual.eq_ignore_ascii_case(wanted))
                })
                .collect(),
        };
        if matches.len() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{name} is missing or has ambiguous letter casing"),
            ));
        }
        current.push(matches[0].file_name());
        if !current.canonicalize()?.starts_with(&base) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "package path leaves the install folder",
            ));
        }
    }
    if !current.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "package image is not a file",
        ));
    }
    current
        .strip_prefix(&base)
        .map(Path::to_path_buf)
        .map_err(|_| io::Error::other("package path leaves the install folder"))
}

/// Build the view of the game in `root` for starting `exe` (package path):
/// decrypt every encrypted file of `files` with `key` into it and link the
/// rest. Blocking work runs on Tokio's blocking pool.
pub async fn prepare(
    root: &Path,
    xvd: &XvdFile,
    files: &HashMap<String, SegmentFile>,
    key: [u8; 32],
    exe: &str,
) -> anyhow::Result<LaunchView> {
    use anyhow::Context as _;
    let entry = checked_relative_path(exe)?;
    let root = root.to_path_buf();
    // Resolve the on-disk names of the encrypted images first.
    let encrypted: Vec<(PathBuf, SegmentFile)> = {
        let root = root.clone();
        let list: Vec<(String, SegmentFile)> = files
            .iter()
            .filter(|(_, f)| f.keep_encrypted)
            .map(|(n, f)| (n.clone(), SegmentFile::clone(f)))
            .collect();
        tokio::task::spawn_blocking(move || -> io::Result<Vec<(PathBuf, SegmentFile)>> {
            list.into_iter()
                .map(|(name, file)| Ok((resolve_package_file(&root, &name)?, file)))
                .collect()
        })
        .await
        .context("a background task failed")??
    };
    let entry = {
        let root = root.clone();
        let wanted = entry.clone();
        tokio::task::spawn_blocking(move || resolve_package_file(&root, &wanted.to_string_lossy()))
            .await
            .context("a background task failed")?
            .with_context(|| {
                format!("The game's program {exe} is not installed. Verify the game's files.")
            })?
    };
    let decrypted: HashSet<PathBuf> = encrypted.iter().map(|(p, _)| p.clone()).collect();
    let view = {
        let (root, decrypted) = (root.clone(), decrypted.clone());
        tokio::task::spawn_blocking(move || create_view(&root, &decrypted))
            .await
            .context("a background task failed")??
    };
    for (relative, file) in &encrypted {
        let source = root.join(relative);
        let mut input = tokio::fs::File::open(&source)
            .await
            .with_context(|| format!("Could not open {}", relative.display()))?;
        let expected = file.stored_length()?;
        if input.metadata().await?.len() < expected {
            anyhow::bail!(
                "{} is incomplete. Verify the game's files to repair it.",
                relative.display()
            );
        }
        let target = view.dir.path().join(relative);
        let mut output = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .await
            .with_context(|| format!("Could not write {}", target.display()))?;
        xvd.mount_mem_fd(&mut input, &mut output, file, key, |_, _| {})
            .await
            .map_err(|e| anyhow::anyhow!("Could not decrypt {}: {e}", relative.display()))?;
        tokio::io::AsyncWriteExt::flush(&mut output).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).await?;
        }
    }
    let entry = view.dir.path().join(entry);
    Ok(LaunchView { entry, ..view })
}

/// An empty view with every directory that holds a decrypted file real and
/// everything else a link into `root`.
fn create_view(root: &Path, decrypted: &HashSet<PathBuf>) -> anyhow::Result<LaunchView> {
    let parent = root.parent().unwrap_or(root);
    sweep(parent);
    let dir = private_dir(parent).or_else(|_| {
        let tmp = std::env::temp_dir();
        sweep(&tmp);
        private_dir(&tmp)
    })?;
    let lock = File::create(dir.path().join(LOCK_FILE))?;
    lock.try_lock()
        .map_err(|_| io::Error::other("the launch view is in use"))?;
    let mut real_dirs = HashSet::new();
    for relative in decrypted {
        let mut parent = relative.parent();
        while let Some(p) = parent.filter(|p| !p.as_os_str().is_empty()) {
            real_dirs.insert(p.to_path_buf());
            parent = p.parent();
        }
    }
    populate(root, dir.path(), Path::new(""), &real_dirs, decrypted)?;
    Ok(LaunchView {
        entry: PathBuf::new(),
        dir,
        _lock: lock,
    })
}

/// A new folder only the user can use, in `parent`.
fn private_dir(parent: &Path) -> io::Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(VIEW_PREFIX);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    let dir = builder.tempdir_in(parent)?;
    // Some filesystems can't hold links (or private permissions); the view
    // then goes to the temporary folder instead.
    let probe = dir.path().join(".irori-link-probe");
    symlink(Path::new("."), &probe)?;
    std::fs::remove_file(probe)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let meta = std::fs::symlink_metadata(dir.path())?;
        if !meta.is_dir()
            || meta.permissions().mode() & 0o777 != 0o700
            || meta.uid() != nix::unistd::geteuid().as_raw()
        {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
    }
    Ok(dir)
}

fn populate(
    root: &Path,
    view: &Path,
    relative_dir: &Path,
    real_dirs: &HashSet<PathBuf>,
    decrypted: &HashSet<PathBuf>,
) -> io::Result<()> {
    for item in std::fs::read_dir(root.join(relative_dir))? {
        let item = item?;
        let name = item.file_name();
        if name.to_str().is_some_and(|n| n.starts_with(".irori-xbox")) {
            continue;
        }
        let relative = relative_dir.join(&name);
        let destination = view.join(&relative);
        if real_dirs.contains(&relative) {
            if !item.metadata()?.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("expected a folder: {}", relative.display()),
                ));
            }
            std::fs::create_dir(&destination)?;
            populate(root, view, &relative, real_dirs, decrypted)?;
        } else if !decrypted.contains(&relative) {
            symlink(&item.path(), &destination)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn symlink(_target: &Path, _link: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "launch views need symbolic links",
    ))
}

/// Remove views no session holds any more (left by a crash) in `parent`.
pub fn sweep(parent: &Path) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(VIEW_PREFIX) && !name.starts_with(&VIEW_PREFIX[1..]) {
            continue;
        }
        let path = entry.path();
        if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let held = File::options()
            .write(true)
            .open(path.join(LOCK_FILE))
            .is_ok_and(|lock| lock.try_lock().is_err());
        if held {
            continue;
        }
        // Only the view itself goes: links are removed, never followed.
        if let Err(e) = remove_view(&path) {
            tracing::warn!(
                "Could not remove the old launch view {}: {e}",
                path.display()
            );
        }
    }
}

fn remove_view(path: &Path) -> io::Result<()> {
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            return Err(io::Error::other("unexpected path"));
        }
    }
    std::fs::remove_dir_all(path)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn resolves_case_and_refuses_escapes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("Bin")).unwrap();
        std::fs::write(root.path().join("Bin/Game.exe"), b"MZ").unwrap();
        assert_eq!(
            resolve_package_file(root.path(), "bin\\GAME.exe").unwrap(),
            Path::new("Bin/Game.exe")
        );
        for bad in ["..\\x.exe", "C:\\x", "", "Bin/missing.exe"] {
            assert!(resolve_package_file(root.path(), bad).is_err(), "{bad}");
        }
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("o.exe"), b"MZ").unwrap();
        std::os::unix::fs::symlink(outside.path().join("o.exe"), root.path().join("o.exe"))
            .unwrap();
        assert!(resolve_package_file(root.path(), "o.exe").is_err());
        std::fs::write(root.path().join("bin/game.exe"), b"MZ").ok();
        std::fs::create_dir_all(root.path().join("bin")).unwrap();
        std::fs::write(root.path().join("bin/Game.EXE"), b"MZ").unwrap();
        assert!(resolve_package_file(root.path(), "BIN/game.exe").is_err());
    }

    #[test]
    fn view_links_everything_but_decrypted_files_and_is_private() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("Game");
        std::fs::create_dir_all(root.join("Bin/Data")).unwrap();
        std::fs::write(root.join("Bin/Game.exe"), b"cipher").unwrap();
        std::fs::write(root.join("Bin/Data/a.pak"), b"data").unwrap();
        std::fs::write(root.join("readme.txt"), b"hi").unwrap();
        std::fs::write(root.join(".irori-xbox.msixvc"), b"meta").unwrap();
        let decrypted = HashSet::from([PathBuf::from("Bin/Game.exe")]);
        let view = create_view(&root, &decrypted).unwrap();
        let v = view.path().to_path_buf();
        assert!(v.starts_with(parent.path()));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&v).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(std::fs::symlink_metadata(v.join("Bin")).unwrap().is_dir());
        assert!(
            std::fs::symlink_metadata(v.join("Bin/Data"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            std::fs::symlink_metadata(v.join("readme.txt"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!v.join("Bin/Game.exe").exists());
        assert!(!v.join(".irori-xbox.msixvc").exists());
        // A held view survives a sweep; a dropped one is gone.
        sweep(parent.path());
        assert!(v.exists());
        drop(view);
        assert!(!v.exists());
    }

    #[test]
    fn sweep_removes_unheld_views_but_never_their_targets() {
        let parent = tempfile::tempdir().unwrap();
        let game = parent.path().join("Game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("keep.bin"), b"keep").unwrap();
        let stale = parent.path().join(format!("{VIEW_PREFIX}old"));
        std::fs::create_dir_all(&stale).unwrap();
        std::os::unix::fs::symlink(&game, stale.join("Game")).unwrap();
        sweep(parent.path());
        assert!(!stale.exists());
        assert_eq!(std::fs::read(game.join("keep.bin")).unwrap(), b"keep");
    }
}
