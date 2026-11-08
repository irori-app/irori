//! The file that turns on the Steam client's remote debugging: an empty
//! `.cef-enable-remote-debugging` that Steam looks for when it starts. On
//! Linux and Windows it sits in Steam's folder; on macOS beside the client's
//! program inside `Steam.AppBundle`.
//!
//! Irori only removes the file when it created it: another program (Decky
//! Loader, a theme tool) may rely on one that was already there.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use irori_core::HostPlatform;

/// The file's name, as Steam looks for it.
pub(crate) const MARKER_NAME: &str = ".cef-enable-remote-debugging";

/// Where Steam on `platform` looks for the file, for the Steam folder
/// `steam_dir`.
pub(crate) fn marker_path_for(platform: HostPlatform, steam_dir: &Path) -> PathBuf {
    match platform {
        HostPlatform::Mac => steam_dir
            .join("Steam.AppBundle/Steam/Contents/MacOS")
            .join(MARKER_NAME),
        HostPlatform::Windows | HostPlatform::Linux => steam_dir.join(MARKER_NAME),
    }
}

/// [`marker_path_for`] on this computer.
pub(crate) fn marker_path(steam_dir: &Path) -> PathBuf {
    marker_path_for(HostPlatform::current(), steam_dir)
}

/// Create the file unless it exists. `true` when Irori created it, `false`
/// when it was already there. Blocking.
pub(crate) fn create(steam_dir: &Path) -> Result<bool> {
    let path = marker_path(steam_dir);
    if path.exists() {
        return Ok(false);
    }
    let folder = path.parent().unwrap_or(steam_dir);
    anyhow::ensure!(
        folder.is_dir(),
        "Irori couldn't find Steam's folder ({}). Start Steam once, or set the Steam path in Irori's settings.",
        folder.display()
    );
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map(|_| true)
        .or_else(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => Ok(false),
            _ => Err(e),
        })
        .with_context(|| format!("Couldn't create {}", path.display()))
}

/// Remove the file. Missing is fine. Blocking.
pub(crate) fn remove(steam_dir: &Path) -> Result<()> {
    let path = marker_path(steam_dir);
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("Couldn't remove {}", path.display()))
        }
        _ => Ok(()),
    }
}

/// Whether the file is there. Blocking.
pub(crate) fn exists(steam_dir: &Path) -> bool {
    marker_path(steam_dir).is_file()
}

/// Whether Millennium (a Steam client modding tool) looks installed for
/// `steam_dir`. Millennium can open Steam's debugging port itself, on a port
/// of its choosing, without the file. A hint for explaining failures only.
/// Blocking.
pub(crate) fn millennium_installed(steam_dir: &Path) -> bool {
    let home = irori_core::paths::expand_home("~/");
    millennium_files(HostPlatform::current(), steam_dir, &home)
        .iter()
        .any(|p| p.exists())
}

/// Files Millennium's installers leave, per platform.
fn millennium_files(platform: HostPlatform, steam_dir: &Path, home: &Path) -> Vec<PathBuf> {
    match platform {
        HostPlatform::Windows => vec![steam_dir.join("millennium.dll")],
        HostPlatform::Mac => vec![
            steam_dir.join("Steam.AppBundle/Steam/Contents/MacOS/libtier0_s.real.dylib"),
            home.join("Library/Application Support/Millennium"),
        ],
        HostPlatform::Linux => vec![
            home.join(".local/share/millennium"),
            PathBuf::from("/usr/lib/millennium"),
            PathBuf::from("/usr/share/millennium"),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sits_where_each_platform_looks() {
        let dir = Path::new("/s");
        assert_eq!(
            marker_path_for(HostPlatform::Linux, dir),
            Path::new("/s/.cef-enable-remote-debugging")
        );
        assert_eq!(
            marker_path_for(HostPlatform::Windows, dir),
            Path::new("/s/.cef-enable-remote-debugging")
        );
        assert_eq!(
            marker_path_for(HostPlatform::Mac, dir),
            Path::new("/s/Steam.AppBundle/Steam/Contents/MacOS/.cef-enable-remote-debugging")
        );
    }

    #[test]
    fn creates_once_and_removes() {
        let dir = tempfile::tempdir().unwrap();
        let steam = dir.path();
        std::fs::create_dir_all(marker_path(steam).parent().unwrap()).unwrap();
        assert!(!exists(steam));
        assert!(create(steam).unwrap(), "Irori made it");
        assert!(exists(steam));
        assert_eq!(std::fs::metadata(marker_path(steam)).unwrap().len(), 0);
        assert!(!create(steam).unwrap(), "already there");
        remove(steam).unwrap();
        assert!(!exists(steam));
        remove(steam).unwrap();
    }

    #[test]
    fn needs_steams_folder() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("no-steam");
        assert!(create(&missing).is_err());
        assert!(!missing.exists(), "nothing is created for a missing Steam");
    }

    #[test]
    fn knows_millenniums_files() {
        let dir = tempfile::tempdir().unwrap();
        let files = millennium_files(HostPlatform::Windows, dir.path(), dir.path());
        assert!(!files.iter().any(|p| p.exists()));
        std::fs::write(dir.path().join("millennium.dll"), b"").unwrap();
        assert!(files.iter().any(|p| p.exists()));
    }
}
