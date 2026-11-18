//! Steam's own per-game settings Irori changes: the Steam Play compatibility
//! tool (the Proton build) Steam runs a game with, from the tools installed
//! ([`tools`]) into Steam's `config/config.vdf` ([`mapping`]), and the
//! game's launch options in the account's `localconfig.vdf`
//! ([`launch_options`]), with the rules for changing them.
//!
//! Steam reads both files when it starts and writes them back when it
//! exits, so a change made while it runs would be lost and could clobber
//! Steam's own. The files are therefore written only while Steam is closed;
//! closing it is the user's call ([`close_steam`]), never in Steam's Game
//! Mode. Before each write the previous file is kept as
//! `<file>.irori-backup`, and the new one replaces it atomically.

mod launch_options;
mod mapping;
mod tools;

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use irori_core::host;

pub use launch_options::{
    COMMAND, variable, with_dll_overrides, with_variable, without_dll_overrides,
};
pub use tools::{CompatTool, ToolSource};

/// What a game's launch options in Steam show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchOptionsState {
    /// The line Steam starts the game with; `None` when there is none.
    pub line: Option<String>,
    /// Steam runs now, so the line can't be written.
    pub steam_running: bool,
    /// Irori may close Steam (not in Steam's Game Mode).
    pub can_close_steam: bool,
}

/// What the compatibility tool choice of one game shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatToolState {
    /// The installed tools, in the order to offer them.
    pub tools: Vec<CompatTool>,
    /// The tool Steam maps the game to; `None` for Steam's default.
    pub current: Option<String>,
    /// Steam runs now, so the choice can't be written.
    pub steam_running: bool,
    /// Irori may close Steam: false where Steam is the session itself
    /// (the Steam Deck's Game Mode, Big Picture sessions).
    pub can_close_steam: bool,
}

/// The choice can't be saved while Steam runs. Returned in the error chain
/// of [`set_compat_tool`], so callers can offer to close Steam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SteamRunning;

impl fmt::Display for SteamRunning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Steam is running. Close Steam to change its settings for the game.")
    }
}

impl std::error::Error for SteamRunning {}

fn config_file(steam_dir: &Path) -> PathBuf {
    steam_dir.join("config").join("config.vdf")
}

fn backup_file(steam_dir: &Path) -> PathBuf {
    steam_dir.join("config").join("config.vdf.irori-backup")
}

/// The tools and the current choice of `app_id`.
pub(crate) async fn state(steam_dir: &Path, app_id: &str) -> Result<CompatToolState> {
    let dir = steam_dir.to_path_buf();
    let id = app_id.to_string();
    let (tools, current) = tokio::task::spawn_blocking(move || {
        let current = std::fs::read_to_string(config_file(&dir))
            .ok()
            .and_then(|text| mapping::compat_tool(&text, &id));
        (tools::installed_tools(&dir), current)
    })
    .await?;
    Ok(CompatToolState {
        tools,
        current,
        steam_running: steam_running().await?,
        can_close_steam: !irori_core::platform::in_steam_gamepad_ui(),
    })
}

/// Map `app_id` to `tool`, or back to Steam's default for `None`. Fails
/// with [`SteamRunning`] while Steam runs.
pub(crate) async fn set_compat_tool(
    steam_dir: &Path,
    app_id: &str,
    tool: Option<&str>,
) -> Result<()> {
    if steam_running().await? {
        return Err(SteamRunning.into());
    }
    let dir = steam_dir.to_path_buf();
    let id = app_id.to_string();
    let tool = tool.map(str::to_string);
    tokio::task::spawn_blocking(move || write_mapping(&dir, &id, tool.as_deref())).await??;
    Ok(())
}

/// Write the mapping into `steam_dir`'s settings file, keeping the previous
/// file as the backup. `false` when the file already said so. Blocking.
fn write_mapping(steam_dir: &Path, app_id: &str, tool: Option<&str>) -> Result<bool> {
    let changed = edit_file(&config_file(steam_dir), &backup_file(steam_dir), |text| {
        mapping::set_compat_tool(text, app_id, tool)
    })?;
    if !changed {
        return Ok(false);
    }
    tracing::info!(
        "Steam app {app_id} now uses {}",
        tool.unwrap_or("Steam's default compatibility tool")
    );
    Ok(true)
}

/// Rewrite one of Steam's settings files with `edit`, keeping the previous
/// file as `backup`. `false` when the edit changed nothing. Blocking.
fn edit_file(
    file: &Path,
    backup: &Path,
    edit: impl FnOnce(&str) -> Result<String>,
) -> Result<bool> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            bail!("Steam has no settings file yet. Start Steam once, close it and try again.")
        }
        Err(e) => return Err(e).with_context(|| format!("Couldn't read {}", file.display())),
    };
    let edited = edit(&text)?;
    if edited == text {
        return Ok(false);
    }
    irori_core::json_store::write_bytes(backup, text.as_bytes())
        .with_context(|| format!("Couldn't keep a copy of {}", file.display()))?;
    irori_core::json_store::write_bytes(file, edited.as_bytes())
        .with_context(|| format!("Couldn't save {}", file.display()))?;
    Ok(true)
}

/// The account's `localconfig.vdf`: the signed-in account's (`steamid`),
/// else the user Steam remembers as the latest here.
fn localconfig_file(steam_dir: &Path, steamid: Option<&str>) -> Result<PathBuf> {
    crate::hidden::user_dir(steam_dir, steamid)
        .map(|dir| dir.join("config").join("localconfig.vdf"))
        .context("Steam has no account on this computer yet. Sign in to Steam once and try again.")
}

/// The launch options of `app_id` and whether they can be written now.
pub(crate) async fn launch_options_state(
    steam_dir: &Path,
    steamid: Option<String>,
    app_id: &str,
) -> Result<LaunchOptionsState> {
    let dir = steam_dir.to_path_buf();
    let id = app_id.to_string();
    let line = tokio::task::spawn_blocking(move || -> Result<Option<String>> {
        let file = localconfig_file(&dir, steamid.as_deref())?;
        Ok(std::fs::read_to_string(file)
            .ok()
            .and_then(|text| launch_options::launch_options(&text, &id)))
    })
    .await??;
    Ok(LaunchOptionsState {
        line,
        steam_running: steam_running().await?,
        can_close_steam: !irori_core::platform::in_steam_gamepad_ui(),
    })
}

/// Set the launch options of `app_id` to `line`, or clear them for `None`.
/// Fails with [`SteamRunning`] while Steam runs.
pub(crate) async fn set_launch_options(
    steam_dir: &Path,
    steamid: Option<String>,
    app_id: &str,
    line: Option<String>,
) -> Result<()> {
    if steam_running().await? {
        return Err(SteamRunning.into());
    }
    let dir = steam_dir.to_path_buf();
    let id = app_id.to_string();
    tokio::task::spawn_blocking(move || {
        write_launch_options(
            &localconfig_file(&dir, steamid.as_deref())?,
            &id,
            line.as_deref(),
        )
    })
    .await??;
    Ok(())
}

/// Write the launch options into `file`, keeping the previous file as
/// `localconfig.vdf.irori-backup`. Blocking.
fn write_launch_options(file: &Path, app_id: &str, line: Option<&str>) -> Result<bool> {
    let backup = file.with_file_name("localconfig.vdf.irori-backup");
    let changed = edit_file(file, &backup, |text| {
        launch_options::set_launch_options(text, app_id, line)
    })?;
    if changed {
        // The line may hold anything the user typed; only say it changed.
        tracing::info!("Steam app {app_id}: launch options changed");
    }
    Ok(changed)
}

/// How long Steam gets to close.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(30);

/// Ask Steam to close (`steam -shutdown`, which also reaches Flatpak Steam)
/// and wait until it has, so its settings file is written and free.
pub(crate) async fn close_steam(steam_dir: &Path) -> Result<()> {
    if irori_core::platform::in_steam_gamepad_ui() {
        bail!("Steam runs this session, so Irori can't close it.");
    }
    if !steam_running().await? {
        return Ok(());
    }
    let command = match crate::client::steam_program(steam_dir).await {
        Some(steam) => steam.arg("-shutdown"),
        None => crate::client::open_url("steam://exit"),
    };
    irori_core::process::run(&command, Default::default())
        .await
        .context("Steam couldn't be asked to close")?;
    let deadline = tokio::time::Instant::now() + CLOSE_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if !steam_running().await? {
            return Ok(());
        }
    }
    bail!("Steam didn't close. Close it yourself and try again.")
}

/// Whether the Steam client runs, on the host when Irori is a Flatpak.
async fn steam_running() -> Result<bool> {
    if cfg!(target_os = "linux") && host::is_flatpak() {
        let processes = host::processes(None)
            .await
            .context("Couldn't tell whether Steam is running")?;
        return Ok(processes
            .iter()
            .any(|p| p.args().first().is_some_and(|a| is_steam_program(a))));
    }
    tokio::task::spawn_blocking(|| {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
        );
        sys.processes().values().any(|p| {
            is_steam_program(&p.name().to_string_lossy())
                || p.exe()
                    .and_then(Path::file_name)
                    .is_some_and(|n| is_steam_program(&n.to_string_lossy()))
        })
    })
    .await
    .context("Couldn't tell whether Steam is running")
}

/// Whether a program (a path or a process name) is the Steam client
/// itself, not one of its helpers (`steamwebhelper`, `steam.sh`).
fn is_steam_program(program: &str) -> bool {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    matches!(
        name.to_ascii_lowercase().as_str(),
        "steam" | "steam.exe" | "steam_osx"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_the_client_from_its_helpers() {
        assert!(is_steam_program("steam"));
        assert!(is_steam_program(
            "/home/u/.local/share/Steam/ubuntu12_32/steam"
        ));
        assert!(is_steam_program(r"C:\Program Files (x86)\Steam\steam.exe"));
        assert!(is_steam_program("steam_osx"));
        assert!(!is_steam_program("steamwebhelper"));
        assert!(!is_steam_program("/usr/bin/steam.sh"));
        assert!(!is_steam_program("/usr/lib/steam/bin_steam.sh"));
        assert!(!is_steam_program("irori"));
    }

    #[test]
    fn writes_with_a_backup_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(write_mapping(dir.path(), "620", Some("proton_9")).is_err());
        std::fs::create_dir_all(dir.path().join("config")).unwrap();
        let original = "\"InstallConfigStore\"\n{\n\t\"Music\"\t\t\"1\"\n}\n";
        std::fs::write(config_file(dir.path()), original).unwrap();

        assert!(write_mapping(dir.path(), "620", Some("proton_9")).unwrap());
        let written = std::fs::read_to_string(config_file(dir.path())).unwrap();
        assert_eq!(
            mapping::compat_tool(&written, "620").as_deref(),
            Some("proton_9")
        );
        assert_eq!(
            std::fs::read_to_string(backup_file(dir.path())).unwrap(),
            original
        );
        // The same choice again writes nothing.
        assert!(!write_mapping(dir.path(), "620", Some("proton_9")).unwrap());
        assert!(write_mapping(dir.path(), "620", None).unwrap());
        let reset = std::fs::read_to_string(config_file(dir.path())).unwrap();
        assert_eq!(mapping::compat_tool(&reset, "620"), None);
        assert!(reset.contains("\"Music\""));
        assert_eq!(
            std::fs::read_to_string(backup_file(dir.path())).unwrap(),
            written
        );
        let files = std::fs::read_dir(dir.path().join("config"))
            .unwrap()
            .count();
        assert_eq!(files, 2, "only the file and its backup");
    }

    #[test]
    fn launch_options_are_written_with_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("localconfig.vdf");
        assert!(write_launch_options(&file, "620", Some("-novid")).is_err());
        let original = "\"UserLocalConfigStore\"\n{\n\t\"friends\"\n\t{\n\t}\n}\n";
        std::fs::write(&file, original).unwrap();
        assert!(write_launch_options(&file, "620", Some("-novid")).unwrap());
        let written = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            launch_options::launch_options(&written, "620").as_deref(),
            Some("-novid")
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("localconfig.vdf.irori-backup")).unwrap(),
            original
        );
        assert!(!write_launch_options(&file, "620", Some("-novid")).unwrap());
        assert!(write_launch_options(&file, "620", None).unwrap());
    }

    #[test]
    fn finds_the_accounts_settings_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(localconfig_file(dir.path(), None).is_err());
        let user = dir.path().join("userdata/1234/config");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(user.join("localconfig.vdf"), "").unwrap();
        assert_eq!(
            localconfig_file(dir.path(), None).unwrap(),
            user.join("localconfig.vdf")
        );
        // The signed-in account's own folder only.
        assert!(localconfig_file(dir.path(), Some("76561197960265729")).is_err());
        assert_eq!(
            localconfig_file(dir.path(), Some("76561197960266962")).unwrap(),
            user.join("localconfig.vdf")
        );
    }

    #[test]
    fn running_steam_is_its_own_error() {
        let e: anyhow::Error = anyhow::Error::from(SteamRunning).context("saving");
        assert!(e.downcast_ref::<SteamRunning>().is_some());
    }
}
