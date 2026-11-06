//! Windows files opened with Irori from the file manager (a double-clicked
//! `.exe`, `.msi`, `.bat`, `.cmd` or `.reg`): which games can open them,
//! opening one in a game's Wine prefix, and making Irori the program that
//! opens them.

use std::path::Path;

use anyhow::{Context as _, Result, ensure};
use irori_core::{GameId, GameInfo, HostPlatform, InstallPlatform, Runner};

use crate::Backend;

/// Whether a Windows file can open in this game's Wine prefix: an installed
/// Windows build that Irori runs through Wine itself. Steam's games live in
/// Steam's own Proton prefixes and a third-party launcher's in the
/// launcher's, so neither is offered. Reads no files, so screens may ask
/// while they draw.
pub fn opens_windows_files(game: &GameInfo) -> bool {
    opens_windows_files_on(HostPlatform::current(), game)
}

fn opens_windows_files_on(host: HostPlatform, game: &GameInfo) -> bool {
    let runner = game.runner();
    game.is_installed
        && !game.is_dlc
        && game.browser_url.is_none()
        && runner.manages_installs()
        && runner != Runner::Emulation
        && game.installed_platform() == Some(InstallPlatform::Windows)
        && host.needs_wine(InstallPlatform::Windows)
}

/// Whether Irori can make itself the program that opens Windows files.
/// macOS offers Irori through the app bundle (Finder's "Open With"), and
/// Windows runs the files itself.
pub fn can_open_windows_files_by_default() -> bool {
    cfg!(target_os = "linux")
}

impl Backend {
    /// Open a Windows file in the game's Wine prefix, from the file's own
    /// folder: run a program, installer or batch file, or import a registry
    /// file. Returns once it has started.
    pub async fn open_windows_file(&self, id: &GameId, file: &Path) -> Result<()> {
        let argv = irori_runtime::windows_file::command(file)
            .context("Irori opens only .exe, .msi, .bat, .cmd and .reg files")?;
        ensure!(
            tokio::fs::metadata(file).await.is_ok_and(|m| m.is_file()),
            "{} was not found",
            file.display()
        );
        let game = self.game(id).context("unknown game")?;
        ensure!(
            opens_windows_files(&game),
            "{} doesn't run through Wine",
            game.title
        );
        self.run_in_prefix(id, argv).await.map(|_| ())
    }

    /// Make Irori the program that opens Windows programs, installers, batch
    /// and registry files for this user, so a double click in the file
    /// manager opens them here. See [`can_open_windows_files_by_default`].
    pub async fn open_windows_files_by_default(&self) -> Result<()> {
        let launcher = irori_extras::shortcuts::LauncherCommand::detect();
        irori_extras::shortcuts::desktop::register_windows_files(&launcher).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::{InstalledInfo, Runner};

    fn game(runner: Runner, platform: Option<InstallPlatform>) -> GameInfo {
        GameInfo {
            app_name: "app".into(),
            runner: Some(runner),
            title: "Game".into(),
            is_installed: platform.is_some(),
            install: platform.map(|platform| InstalledInfo {
                platform,
                install_path: "/games/game".into(),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn windows_builds_run_by_irori_open_windows_files() {
        let opens = |g: &GameInfo| opens_windows_files_on(HostPlatform::Linux, g);
        let windows = Some(InstallPlatform::Windows);
        for runner in [
            Runner::Legendary,
            Runner::Gog,
            Runner::Nile,
            Runner::Sideload,
        ] {
            assert!(opens(&game(runner, windows)), "{runner:?}");
        }
        assert!(opens_windows_files_on(
            HostPlatform::Mac,
            &game(Runner::Gog, windows)
        ));
        // Not installed, native, DLC, a web page, or in someone else's prefix.
        assert!(!opens(&game(Runner::Gog, None)));
        assert!(!opens(&game(Runner::Gog, Some(InstallPlatform::Linux))));
        assert!(!opens(&GameInfo {
            is_dlc: true,
            ..game(Runner::Gog, windows)
        }));
        assert!(!opens(&GameInfo {
            browser_url: Some("https://example.com".into()),
            ..game(Runner::Sideload, windows)
        }));
        assert!(!opens(&game(Runner::Steam, windows)));
        assert!(!opens(&game(Runner::Launchers, windows)));
        assert!(!opens(&game(Runner::Emulation, windows)));
        // Windows runs them itself.
        assert!(!opens_windows_files_on(
            HostPlatform::Windows,
            &game(Runner::Gog, windows)
        ));
    }
}
