//! Moving files and folders to the system's trash, for cleanups whose items
//! may hold something the user wants back (a prefix's saves, a folder in the
//! games folder).
//!
//! - Linux: the freedesktop.org trash: the home trash when the item is on
//!   the same disk, else the disk's own `.Trash/<uid>` or `.Trash-<uid>`, with
//!   a `.trashinfo` file so file managers can put it back.
//! - macOS: `~/.Trash` on the startup disk, else the volume's
//!   `.Trashes/<uid>`. Moving is a rename, so nothing is copied.
//! - Windows: the Recycle Bin through the shell (`SHFileOperationW` with
//!   undo allowed). A drive without a Recycle Bin deletes the item instead;
//!   Windows doesn't say which happened.
//!
//! Elsewhere there is no trash and [`trash`] answers `Ok(false)`.

use std::path::Path;

use anyhow::Result;

/// Move `path` to the trash. `Ok(false)` means this system has no trash for
/// it (another disk without one); the caller decides whether to delete.
pub fn trash(path: &Path) -> Result<bool> {
    imp::trash(path)
}

#[cfg(target_os = "linux")]
mod imp {
    use std::io::Write as _;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    use std::path::{Path, PathBuf};

    use anyhow::{Context as _, Result};

    /// The home trash: `$XDG_DATA_HOME/Trash`, or the host's inside the
    /// Flatpak (whose `XDG_DATA_HOME` is the sandbox's own data folder).
    fn home_trash() -> Option<PathBuf> {
        if irori_core::host::is_flatpak() {
            return dirs::home_dir().map(|h| h.join(".local/share/Trash"));
        }
        dirs::data_dir().map(|d| d.join("Trash"))
    }

    fn uid() -> u32 {
        // SAFETY: `getuid` takes nothing and can't fail.
        unsafe { libc_getuid() }
    }

    unsafe extern "C" {
        #[link_name = "getuid"]
        fn libc_getuid() -> u32;
    }

    /// The topmost folder above `path` on the same disk.
    fn top_dir(path: &Path, dev: u64) -> PathBuf {
        let mut top = path.to_path_buf();
        for ancestor in path.ancestors().skip(1) {
            match std::fs::metadata(ancestor) {
                Ok(meta) if meta.dev() == dev => top = ancestor.to_path_buf(),
                _ => break,
            }
        }
        top
    }

    /// A disk's own trash for this user: `.Trash/<uid>` when `.Trash` is a
    /// real sticky folder (the spec refuses others), else `.Trash-<uid>`.
    fn disk_trash(top: &Path) -> Option<PathBuf> {
        let shared = top.join(".Trash");
        if let Ok(meta) = std::fs::symlink_metadata(&shared)
            && meta.is_dir()
            && meta.permissions().mode() & 0o1000 != 0
        {
            let own = shared.join(uid().to_string());
            if std::fs::create_dir_all(&own).is_ok() {
                return Some(own);
            }
        }
        let own = top.join(format!(".Trash-{}", uid()));
        if !own.is_dir() {
            std::fs::create_dir(&own).ok()?;
            std::fs::set_permissions(&own, std::fs::Permissions::from_mode(0o700)).ok()?;
        }
        Some(own)
    }

    pub(super) fn trash(path: &Path) -> Result<bool> {
        trash_into(path, home_trash())
    }

    /// [`trash`] with the home trash given, so tests use their own.
    pub(super) fn trash_into(path: &Path, home: Option<PathBuf>) -> Result<bool> {
        let path = std::path::absolute(path).context("finding the item to trash")?;
        let meta = std::fs::symlink_metadata(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let dev = meta.dev();
        let home = home.filter(|t| {
            std::fs::create_dir_all(t).is_ok() && std::fs::metadata(t).is_ok_and(|m| m.dev() == dev)
        });
        let (trash, info_base) = match home {
            Some(trash) => (trash, None),
            None => {
                let top = top_dir(&path, dev);
                match disk_trash(&top) {
                    Some(trash) => (trash, Some(top)),
                    None => return Ok(false),
                }
            }
        };
        let files = trash.join("files");
        let info = trash.join("info");
        std::fs::create_dir_all(&files)
            .and_then(|()| std::fs::create_dir_all(&info))
            .with_context(|| format!("opening the trash at {}", trash.display()))?;
        // Paths in a disk's trash are relative to the disk's top folder.
        let recorded = match &info_base {
            Some(top) => path.strip_prefix(top).unwrap_or(&path).to_path_buf(),
            None => path.clone(),
        };
        let stem = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "item".into());
        let date = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
        let body = format!(
            "[Trash Info]\nPath={}\nDeletionDate={date}\n",
            super::encode_path(&recorded.to_string_lossy())
        );
        for n in 1..1000 {
            let name = super::numbered(&stem, n, ".");
            let info_file = info.join(format!("{name}.trashinfo"));
            // The info file is created first and exclusively: it reserves
            // the name, as the spec asks.
            let Ok(mut file) = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&info_file)
            else {
                continue;
            };
            file.write_all(body.as_bytes())
                .with_context(|| format!("writing {}", info_file.display()))?;
            drop(file);
            let target = files.join(&name);
            if target.exists() {
                let _ = std::fs::remove_file(&info_file);
                continue;
            }
            return match std::fs::rename(&path, &target) {
                Ok(()) => Ok(true),
                Err(e) => {
                    let _ = std::fs::remove_file(&info_file);
                    Err(e).with_context(|| format!("moving {} to the trash", path.display()))
                }
            };
        }
        anyhow::bail!("the trash already holds too many items named {stem}")
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::os::unix::fs::MetadataExt as _;
    use std::path::{Path, PathBuf};

    use anyhow::{Context as _, Result};

    unsafe extern "C" {
        #[link_name = "getuid"]
        fn libc_getuid() -> u32;
    }

    /// The trash for an item on the disk `dev`: the home one on the
    /// startup disk, else the volume's own.
    fn trash_for(path: &Path, dev: u64) -> Option<PathBuf> {
        let home = dirs::home_dir()?.join(".Trash");
        if std::fs::metadata(&home).is_ok_and(|m| m.dev() == dev) {
            return Some(home);
        }
        let volume = path
            .ancestors()
            .find(|a| a.parent() == Some(Path::new("/Volumes")))?;
        // SAFETY: `getuid` takes nothing and can't fail.
        let uid = unsafe { libc_getuid() };
        let own = volume.join(".Trashes").join(uid.to_string());
        own.is_dir().then_some(own)
    }

    pub(super) fn trash(path: &Path) -> Result<bool> {
        let path = std::path::absolute(path).context("finding the item to trash")?;
        let meta = std::fs::symlink_metadata(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        match trash_for(&path, meta.dev()) {
            Some(trash) => move_into(&path, &trash).map(|()| true),
            None => Ok(false),
        }
    }

    /// Move `path` into the folder `trash` under a name nothing there has.
    pub(super) fn move_into(path: &Path, trash: &Path) -> Result<()> {
        let stem = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "item".into());
        for n in 1..1000 {
            let target = trash.join(super::numbered(&stem, n, " "));
            if std::fs::symlink_metadata(&target).is_ok() {
                continue;
            }
            return std::fs::rename(path, &target)
                .with_context(|| format!("moving {} to the trash", path.display()));
        }
        anyhow::bail!("the trash already holds too many items named {stem}")
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;

    use anyhow::{Context as _, Result};
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };

    pub(super) fn trash(path: &Path) -> Result<bool> {
        let path = std::path::absolute(path).context("finding the item to trash")?;
        // The shell takes a list of paths ended by an empty one.
        let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
        from.extend([0, 0]);
        let mut op = SHFILEOPSTRUCTW {
            hwnd: std::ptr::null_mut(),
            wFunc: FO_DELETE,
            pFrom: from.as_ptr(),
            pTo: std::ptr::null(),
            fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16,
            fAnyOperationsAborted: 0,
            hNameMappings: std::ptr::null_mut(),
            lpszProgressTitle: std::ptr::null(),
        };
        // SAFETY: `op` and the path it points at live across the call, and
        // the path list ends with two NULs as the API requires.
        let code = unsafe { SHFileOperationW(&mut op) };
        if code != 0 || op.fAnyOperationsAborted != 0 {
            anyhow::bail!(
                "Windows could not move {} to the Recycle Bin (error {code:#x})",
                path.display()
            );
        }
        Ok(true)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod imp {
    use std::path::Path;

    use anyhow::Result;

    pub(super) fn trash(_: &Path) -> Result<bool> {
        Ok(false)
    }
}

/// `stem` for the first try, then `stem<sep>2`, `stem<sep>3`… (keeping a
/// file's extension at the end), so an item never replaces another in the
/// trash.
#[cfg_attr(not(unix), allow(dead_code))] // Windows names its own.
fn numbered(stem: &str, n: u32, sep: &str) -> String {
    if n == 1 {
        return stem.to_string();
    }
    match stem.rsplit_once('.') {
        Some((base, ext)) if !base.is_empty() => format!("{base}{sep}{n}.{ext}"),
        _ => format!("{stem}{sep}{n}"),
    }
}

/// A path as a `.trashinfo` file records it: percent-encoded, keeping `/`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))] // Only Linux writes it.
fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_never_repeat_and_keep_the_extension() {
        assert_eq!(numbered("Hades", 1, "."), "Hades");
        assert_eq!(numbered("Hades", 3, "."), "Hades.3");
        assert_eq!(numbered("save.dat", 2, " "), "save 2.dat");
        assert_eq!(numbered(".hidden", 2, "."), ".hidden.2");
    }

    #[test]
    fn trash_info_paths_are_percent_encoded() {
        assert_eq!(
            encode_path("/home/u/Games/Baldur's Gate 3"),
            "/home/u/Games/Baldur%27s%20Gate%203"
        );
        assert_eq!(encode_path("/p/日"), "/p/%E6%97%A5");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_trashed_folder_can_be_put_back() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("Trash");
        let item = tmp.path().join("Old prefix");
        std::fs::create_dir_all(item.join("drive_c")).unwrap();
        assert!(imp::trash_into(&item, Some(home.clone())).unwrap());
        assert!(!item.exists());
        assert!(home.join("files/Old prefix/drive_c").is_dir());
        let info = std::fs::read_to_string(home.join("info/Old prefix.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath=/"));
        assert!(info.contains("Old%20prefix\nDeletionDate="));

        // A second item of the same name gets a name of its own.
        std::fs::create_dir_all(&item).unwrap();
        assert!(imp::trash_into(&item, Some(home.clone())).unwrap());
        assert!(home.join("files/Old prefix.2").is_dir());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_trashed_folder_keeps_a_name_of_its_own() {
        let tmp = tempfile::tempdir().unwrap();
        let trash = tmp.path().join(".Trash");
        std::fs::create_dir_all(trash.join("Old prefix")).unwrap();
        let item = tmp.path().join("Old prefix");
        std::fs::create_dir_all(item.join("drive_c")).unwrap();
        imp::move_into(&item, &trash).unwrap();
        assert!(!item.exists());
        assert!(trash.join("Old prefix 2/drive_c").is_dir());
    }
}
