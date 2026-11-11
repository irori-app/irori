//! Launcher shortcuts for games: freedesktop entries on the desktop and in
//! the application menu, Steam non-Steam-game shortcuts, and the icons they
//! use. Every shortcut starts the game with `irori --launch <runner>/<app>`
//! (Steam's with `--wait`, so Steam follows the whole session); the
//! `irori://launch/<runner>/<app>` URL scheme is registered alongside.
//!
//! [`ShortcutTargets`], [`add`] and [`remove`] are the contract used by
//! `irori-services`.

#[cfg(target_os = "linux")]
pub mod desktop;
#[cfg(any(target_os = "macos", windows))]
#[path = "desktop_native.rs"]
pub mod desktop;
pub mod icon;
pub mod steam;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, bail};
use irori_core::{AppSettings, GameId, GameInfo};

use crate::util;

/// Which shortcut targets to create.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShortcutTargets {
    pub desktop: bool,
    pub app_menu: bool,
    pub steam: bool,
}

impl ShortcutTargets {
    pub fn from_settings(app: &AppSettings) -> Self {
        Self {
            desktop: app.add_desktop_shortcuts,
            app_menu: app.add_start_menu_shortcuts,
            steam: app.add_steam_shortcuts,
        }
    }
}

/// How shortcuts start Irori: the program plus fixed leading arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherCommand {
    pub program: String,
    /// Arguments before Irori's own (`run <app-id>` under Flatpak).
    pub prefix_args: Vec<String>,
    /// Working directory for Steam's `StartDir`.
    pub working_dir: PathBuf,
}

impl LauncherCommand {
    /// Detect how this process was started: Flatpak (`flatpak run $FLATPAK_ID`),
    /// AppImage (`$APPIMAGE`), else the current executable (falling back to
    /// `irori` on `PATH`).
    pub fn detect() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        if let Some(id) = util::env_nonempty("FLATPAK_ID") {
            return Self {
                program: "flatpak".into(),
                prefix_args: vec!["run".into(), id],
                working_dir: home,
            };
        }
        let exe = util::env_nonempty("APPIMAGE")
            .map(PathBuf::from)
            .or_else(|| std::env::current_exe().ok())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().to_lowercase().contains("irori"))
            });
        match exe {
            Some(exe) => Self {
                working_dir: exe.parent().map(PathBuf::from).unwrap_or(home),
                program: exe.to_string_lossy().into_owned(),
                prefix_args: vec![],
            },
            None => Self {
                program: "irori".into(),
                prefix_args: vec![],
                working_dir: home,
            },
        }
    }

    /// Full argument list (program first) that launches `id`.
    pub fn launch_args(&self, id: &GameId) -> Vec<String> {
        let mut v = Vec::with_capacity(self.prefix_args.len() + 3);
        v.push(self.program.clone());
        v.extend(self.prefix_args.iter().cloned());
        v.push("--launch".into());
        v.push(launch_target(id));
        v
    }
}

/// `<runner>/<app_name>`, the argument of `--launch`.
pub fn launch_target(id: &GameId) -> String {
    format!("{}/{}", id.runner.id(), id.app_name)
}

/// What [`add_with_report`] did.
#[derive(Debug, Clone, Default)]
pub struct ShortcutReport {
    pub desktop: Option<PathBuf>,
    pub app_menu: Option<PathBuf>,
    pub icon: Option<PathBuf>,
    /// Present when a Steam shortcut was requested.
    pub steam: Option<steam::SteamReport>,
    /// Steam was running, so its shortcuts could not be changed. Show the
    /// user "Close Steam and try again".
    pub steam_running: bool,
    /// Non-fatal problems, already logged.
    pub errors: Vec<String>,
}

/// Create launcher shortcuts that start the game through Irori.
///
/// Fails when any requested target could not be created (including Steam
/// being running); the targets that worked are kept. Use
/// [`add_with_report`] for the details.
pub async fn add(game: &GameInfo, app: &AppSettings, targets: ShortcutTargets) -> Result<()> {
    let report = add_with_report(game, app, targets).await?;
    if report.steam_running {
        bail!(
            "Steam is running. Close Steam and try again to add {} to Steam.",
            game.title
        );
    }
    if !report.errors.is_empty() {
        bail!("{}", report.errors.join("; "));
    }
    Ok(())
}

/// Like [`add`], returning what was created. DLCs are skipped.
pub async fn add_with_report(
    game: &GameInfo,
    app: &AppSettings,
    targets: ShortcutTargets,
) -> Result<ShortcutReport> {
    let mut report = ShortcutReport::default();
    if is_dlc(game) {
        return Ok(report);
    }
    let launcher = LauncherCommand::detect();
    // A portable Irori leaves the system as it found it: only the shortcut the
    // user put on the desktop points at it.
    let portable = irori_core::paths::is_portable();
    let targets = ShortcutTargets {
        app_menu: targets.app_menu && !portable,
        ..targets
    };

    if targets.desktop || targets.app_menu {
        if !portable && let Err(e) = desktop::register_url_scheme(&launcher).await {
            report
                .errors
                .push(format!("registering irori:// links: {e:#}"));
        }
        match icon::ensure_icon(game).await {
            Ok(p) => report.icon = p,
            Err(e) => report
                .errors
                .push(format!("icon for {}: {e:#}", game.title)),
        }
        match desktop::create(
            game,
            &launcher,
            report.icon.as_deref(),
            targets.desktop,
            targets.app_menu,
        )
        .await
        {
            Ok(created) => {
                report.desktop = created.desktop;
                report.app_menu = created.menu;
            }
            Err(e) => report
                .errors
                .push(format!("shortcuts for {}: {e:#}", game.title)),
        }
    }

    if targets.steam {
        let steam_id = crate::wiki::steam_app_id(game).await;
        match steam::add(game, app, steam_id.as_deref()).await {
            Ok(r) => {
                report.steam_running = r.steam_running && !r.already_present;
                report.errors.extend(r.errors.iter().cloned());
                report.steam = Some(r);
            }
            Err(e) => report.errors.push(format!("{e:#}")),
        }
    }

    for e in &report.errors {
        tracing::warn!("{e}");
    }
    Ok(report)
}

/// The Steam app IDs (from wiki data) of `games`, which pick official
/// Steam artwork; DLCs are left out.
async fn with_steam_ids(games: &[Arc<GameInfo>]) -> Vec<(&GameInfo, Option<String>)> {
    let mut out = Vec::with_capacity(games.len());
    for game in games.iter().map(|g| &**g).filter(|g| !is_dlc(g)) {
        out.push((game, crate::wiki::steam_app_id(game).await));
    }
    out
}

fn steam_games<'a>(list: &'a [(&'a GameInfo, Option<String>)]) -> Vec<steam::SteamGame<'a>> {
    list.iter()
        .map(|(game, id)| steam::SteamGame {
            game,
            steam_app_id: id.as_deref(),
        })
        .collect()
}

/// Add games to Steam in one pass, installed or not; DLCs are skipped.
/// Returns how many shortcuts were added. Fails when Steam is running or
/// no Steam user could be changed.
pub async fn add_to_steam(games: &[Arc<GameInfo>], app: &AppSettings) -> Result<usize> {
    let ids = with_steam_ids(games).await;
    let list = steam_games(&ids);
    let report = steam::add_many(&list, app).await?;
    if report.steam_running {
        bail!("Steam is running. Close Steam and try again to add games to Steam.");
    }
    if report.users_changed == 0 && !report.already_present {
        bail!("Adding games to Steam failed: {}", report.errors.join("; "));
    }
    for e in &report.errors {
        tracing::warn!("{e}");
    }
    Ok(report.added)
}

/// Bring Irori's Steam shortcuts in step with `games` (DLCs skipped): add
/// the missing ones, update Irori's own fields of the others and fill in
/// their missing art. Irori's shortcuts of games `gone` picks are removed.
/// Fails when Steam is running.
pub async fn sync_steam(
    games: &[Arc<GameInfo>],
    gone: Option<&steam::Gone<'_>>,
    app: &AppSettings,
) -> Result<steam::SyncReport> {
    let ids = with_steam_ids(games).await;
    let list = steam_games(&ids);
    let report = steam::sync(&list, gone, app).await?;
    if report.steam_running {
        bail!("Steam is running. Close Steam and try again to change its shortcuts.");
    }
    Ok(report)
}

/// Remove Irori's Steam shortcuts of `ids`, or every one of Irori's with
/// `None`, with their art. Other programs' shortcuts stay. Fails when Steam
/// is running and there is something to remove.
pub async fn remove_from_steam(
    ids: Option<&HashSet<GameId>>,
    app: &AppSettings,
) -> Result<steam::SyncReport> {
    let report = steam::remove_games(ids, app).await?;
    if report.steam_running {
        bail!("Steam is running. Close Steam and try again to remove games from Steam.");
    }
    Ok(report)
}

/// Remove every shortcut Irori created for the game.
pub async fn remove(game: &GameInfo, app: &AppSettings) -> Result<()> {
    let report = remove_with_report(game, app).await?;
    if report.steam_running {
        bail!(
            "Steam is running. Close Steam and try again to remove {} from Steam.",
            game.title
        );
    }
    if !report.errors.is_empty() {
        bail!("{}", report.errors.join("; "));
    }
    Ok(())
}

/// What [`remove_with_report`] did.
#[derive(Debug, Clone, Default)]
pub struct RemoveReport {
    pub removed_files: Vec<PathBuf>,
    pub steam: steam::SteamReport,
    /// The game has a Steam shortcut that could not be removed because Steam
    /// is running.
    pub steam_running: bool,
    pub errors: Vec<String>,
}

/// Like [`remove`], returning what was removed. DLCs are skipped.
pub async fn remove_with_report(game: &GameInfo, app: &AppSettings) -> Result<RemoveReport> {
    let mut report = RemoveReport::default();
    if is_dlc(game) {
        return Ok(report);
    }
    match desktop::remove(game).await {
        Ok(files) => report.removed_files = files,
        Err(e) => report.errors.push(format!("{e:#}")),
    }
    match steam::remove(game, app).await {
        Ok(r) => {
            report.steam_running = r.steam_running;
            report.errors.extend(r.errors.iter().cloned());
            report.steam = r;
        }
        Err(e) => report.errors.push(format!("{e:#}")),
    }
    icon::remove_icon(game).await;
    Ok(report)
}

/// Whether a desktop or application-menu shortcut exists for the game.
pub async fn exists(game: &GameInfo) -> bool {
    desktop::exists(game).await
}

fn is_dlc(game: &GameInfo) -> bool {
    game.is_dlc || game.install.as_ref().is_some_and(|i| i.is_dlc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;

    #[test]
    fn launch_args() {
        let l = LauncherCommand {
            program: "flatpak".into(),
            prefix_args: vec!["run".into(), "x.Irori".into()],
            working_dir: "/".into(),
        };
        assert_eq!(
            l.launch_args(&GameId::new(Runner::Zoom, "42")),
            ["flatpak", "run", "x.Irori", "--launch", "zoom/42"]
        );
    }
}
