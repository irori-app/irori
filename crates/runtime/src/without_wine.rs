//! Windows builds whose program is one of this system's, which start
//! without Wine, and AppImages started where FUSE can't mount them.
//!
//! A store sells the Windows build, but a game's program may be a native
//! replacement the user put in the game folder and chose (OpenJK for Jedi
//! Academy, fallout2-ce): its header says so, and it starts directly. A
//! program Irori can't recognise starts directly only when the game's
//! settings say so (`iroriNoWine`); a Windows program never does.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use irori_core::program_kind::{self, ProgramKind};
use irori_core::{GameInfo, GameSettings};

/// The program a launch starts, as far as the settings and the install
/// record tell: the game's own program setting (absolute, or inside the
/// install folder), else the store's recorded program. `None` when neither
/// is known.
pub fn launch_program(settings: &GameSettings, game: &GameInfo) -> Option<PathBuf> {
    let install = game
        .install
        .as_ref()
        .map(|i| PathBuf::from(&i.install_path))
        .filter(|p| !p.as_os_str().is_empty());
    let in_install = |rel: &str| -> Option<PathBuf> {
        let rel = rel.trim().replace('\\', "/");
        if rel.is_empty() {
            return None;
        }
        let path = irori_core::paths::expand_home(&rel);
        if path.is_absolute() {
            Some(path)
        } else {
            install.as_ref().map(|dir| dir.join(path))
        }
    };
    in_install(&settings.target_exe).or_else(|| {
        game.install
            .as_ref()
            .and_then(|i| in_install(&i.executable))
    })
}

/// Whether a Windows build starts without Wine: always for a program of
/// this system, for an unrecognised one only when `forced`. A Windows
/// program set to start without Wine is a mistake the user must hear of.
pub fn decide(forced: bool, kind: ProgramKind, program: Option<&Path>) -> Result<bool> {
    match (kind, forced) {
        (ProgramKind::Native, _) => Ok(true),
        (ProgramKind::Windows, true) => bail!(
            "{} is a Windows program and can't start without Wine. Turn off \u{201c}Don't use \
             Wine\u{201d}, or choose the game's native program in its settings.",
            program
                .and_then(Path::file_name)
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "The game's program".into())
        ),
        (_, forced) => Ok(forced),
    }
}

/// [`decide`] for a game about to start (reads the program's first bytes on
/// a blocking thread).
pub async fn runs_without_wine(settings: &GameSettings, game: &GameInfo) -> Result<bool> {
    let program = launch_program(settings, game);
    let kind = match program.clone() {
        Some(p) => tokio::task::spawn_blocking(move || program_kind::of_file(&p))
            .await
            .unwrap_or(ProgramKind::Unknown),
        None => ProgramKind::Unknown,
    };
    let starts = decide(settings.no_wine, kind, program.as_deref())?;
    if starts && let Some(p) = &program {
        make_executable(p);
    }
    Ok(starts)
}

/// A native program unpacked from a Windows build's archive may lack its
/// executable bit.
fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path)
            && meta.is_file()
            && meta.permissions().mode() & 0o111 == 0
        {
            let mut perms = meta.permissions();
            perms.set_mode(perms.mode() | 0o755);
            if let Err(e) = std::fs::set_permissions(path, perms) {
                tracing::warn!("making {} executable: {e}", path.display());
            }
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// `APPIMAGE_EXTRACT_AND_RUN=1` for an AppImage program where FUSE can't
/// mount it (inside Irori's Flatpak, or without `/dev/fuse` and
/// `fusermount`): the AppImage then unpacks itself to a temporary folder and
/// runs from there, which is slower to start but works.
pub fn appimage_env(program: Option<&Path>, fuse_usable: bool) -> Option<(String, String)> {
    let name = program?.file_name()?.to_string_lossy();
    (program_kind::is_appimage(&name) && !fuse_usable)
        .then(|| ("APPIMAGE_EXTRACT_AND_RUN".to_string(), "1".to_string()))
}

/// Whether an AppImage can mount itself here. Blocking (looks at files).
pub fn fuse_usable() -> bool {
    cfg!(target_os = "linux")
        && !crate::locations::is_flatpak()
        && Path::new("/dev/fuse").exists()
        && (which::which("fusermount3").is_ok() || which::which("fusermount").is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::{InstallPlatform, InstalledInfo};

    fn game(install: &str, exe: &str) -> GameInfo {
        GameInfo {
            title: "Jedi Academy".into(),
            install: Some(InstalledInfo {
                install_path: install.into(),
                executable: exe.into(),
                platform: InstallPlatform::Windows,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn the_program_is_the_setting_else_the_store_record() {
        let g = game("/games/JA", "GameData\\jasp.exe");
        let mut s = GameSettings::default();
        assert_eq!(
            launch_program(&s, &g),
            Some(PathBuf::from("/games/JA/GameData/jasp.exe"))
        );
        s.target_exe = "GameData/openjk_sp.x86_64".into();
        assert_eq!(
            launch_program(&s, &g),
            Some(PathBuf::from("/games/JA/GameData/openjk_sp.x86_64"))
        );
        s.target_exe = "/opt/openjk/openjk".into();
        assert_eq!(
            launch_program(&s, &g),
            Some(PathBuf::from("/opt/openjk/openjk"))
        );
        assert_eq!(
            launch_program(&GameSettings::default(), &game("", "")),
            None
        );
    }

    #[test]
    fn only_native_or_forced_programs_skip_wine() {
        assert!(decide(false, ProgramKind::Native, None).unwrap());
        assert!(!decide(false, ProgramKind::Unknown, None).unwrap());
        assert!(!decide(false, ProgramKind::Windows, None).unwrap());
        assert!(decide(true, ProgramKind::Unknown, None).unwrap());
        let e = decide(true, ProgramKind::Windows, Some(Path::new("/g/jasp.exe"))).unwrap_err();
        assert!(e.to_string().contains("jasp.exe"));
    }

    #[test]
    fn appimages_unpack_only_without_fuse() {
        let app = Path::new("/g/OpenJK-x86_64.AppImage");
        assert_eq!(
            appimage_env(Some(app), false),
            Some(("APPIMAGE_EXTRACT_AND_RUN".into(), "1".into()))
        );
        assert_eq!(appimage_env(Some(app), true), None);
        assert_eq!(appimage_env(Some(Path::new("/g/openjk")), false), None);
        assert_eq!(appimage_env(None, false), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_native_program_in_a_windows_build_starts_directly() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("openjk");
        std::fs::write(&native, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&native, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::write(dir.path().join("jasp.exe"), b"MZ\x90\x00").unwrap();
        let g = game(&dir.path().display().to_string(), "jasp.exe");
        let mut s = GameSettings::default();
        assert!(!runs_without_wine(&s, &g).await.unwrap());
        s.target_exe = "openjk".into();
        assert!(runs_without_wine(&s, &g).await.unwrap());
        let mode = std::fs::metadata(&native).unwrap().permissions().mode();
        assert_ne!(mode & 0o111, 0, "made executable");
        // Forcing the Windows program out of Wine is refused.
        s.target_exe = String::new();
        s.no_wine = true;
        assert!(runs_without_wine(&s, &g).await.is_err());
    }
}
