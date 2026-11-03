//! Rockstar titles bought on the Epic Games Store (GTA V, Red Dead Redemption
//! 2, the Definitive Editions…) start through the Rockstar Games Launcher,
//! which must be installed in the game's own Wine prefix. Heroic launches
//! them like any Epic game and leaves that to the game. Irori checks for the
//! launcher at every launch under Wine, after Epic's prerequisites ran, and
//! returns [`LauncherMissing`] so the user can install it there.

use std::path::PathBuf;

use irori_core::{GameInfo, LauncherMissing, WineKind, WineLaunch};
use irori_launchers_data::{Launcher, rockstar};

use crate::state::real_prefix;

/// The missing launcher of `game` (installed with `executable`), when it is
/// a Rockstar title that needs one and the prefix of `wine` lacks it.
pub(crate) fn launcher_missing(
    game: &GameInfo,
    executable: &str,
    wine: &WineLaunch,
) -> Option<LauncherMissing> {
    let makers: Vec<&str> = [&game.developer, &game.publisher]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect();
    let title = rockstar::store_title(&game.title, executable, &makers)?;
    if !title.needs_launcher {
        return None;
    }
    let program = Launcher::Rockstar.exe_in_prefix(&prefix_root(wine));
    if program.is_file() {
        return None;
    }
    Some(LauncherMissing {
        launcher: Launcher::Rockstar.display_name().to_string(),
        installer_url: Launcher::Rockstar.installer_url().to_string(),
        program,
    })
}

/// The folder holding `drive_c`: Proton keeps it in `pfx`, also before the
/// prefix was made.
fn prefix_root(wine: &WineLaunch) -> PathBuf {
    real_prefix(&wine.prefix).unwrap_or_else(|| match wine.kind {
        WineKind::Proton => wine.prefix.join("pfx"),
        _ => wine.prefix.clone(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn wine(prefix: &Path, kind: WineKind) -> WineLaunch {
        WineLaunch {
            bin: "/usr/bin/wine".into(),
            build_bin: "/usr/bin/wine".into(),
            kind,
            prefix: prefix.to_path_buf(),
            uses_umu: false,
            proton_verb: "waitforexitandrun",
        }
    }

    fn game(title: &str, developer: &str) -> GameInfo {
        GameInfo {
            title: title.into(),
            developer: Some(developer.into()),
            ..Default::default()
        }
    }

    #[test]
    fn asks_for_the_launcher_until_it_is_in_the_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path().join("GTA");
        let gta = game("Grand Theft Auto V", "Rockstar North");

        let missing = launcher_missing(&gta, "PlayGTAV.exe", &wine(&prefix, WineKind::Wine))
            .expect("missing");
        assert_eq!(missing.launcher, "Rockstar Games Launcher");
        assert!(
            missing
                .installer_url
                .starts_with("https://gamedownloads.rockstargames.com/")
        );
        assert_eq!(
            missing.program,
            prefix.join("drive_c/Program Files/Rockstar Games/Launcher/Launcher.exe")
        );

        // A Proton prefix that doesn't exist yet gets it inside `pfx`.
        let proton = launcher_missing(&gta, "PlayGTAV.exe", &wine(&prefix, WineKind::Proton))
            .expect("missing");
        assert!(proton.program.starts_with(prefix.join("pfx/drive_c")));

        std::fs::create_dir_all(missing.program.parent().unwrap()).unwrap();
        std::fs::write(&missing.program, b"MZ").unwrap();
        std::fs::write(prefix.join("user.reg"), b"").unwrap();
        assert_eq!(
            launcher_missing(&gta, "PlayGTAV.exe", &wine(&prefix, WineKind::Wine)),
            None
        );
    }

    #[test]
    fn other_games_never_ask() {
        let tmp = tempfile::tempdir().unwrap();
        let w = wine(tmp.path(), WineKind::Wine);
        // Bully's store copies start without the launcher.
        assert_eq!(
            launcher_missing(&game("Bully: Scholarship Edition", "Rockstar"), "", &w),
            None
        );
        assert_eq!(
            launcher_missing(&game("Hades", "Supergiant Games"), "Hades.exe", &w),
            None
        );
        // Found by its program when Epic renames an edition.
        assert!(
            launcher_missing(
                &game("Red Dead Redemption 2: Story Mode", "Rockstar Games"),
                "RDR2.exe",
                &w
            )
            .is_some()
        );
    }
}
