//! Where the runtime keeps and finds things on disk: Irori's tools
//! directories, Heroic's (reused read-only when Irori has nothing yet), Steam
//! and the host environment flags.

use std::path::{Path, PathBuf};

use irori_core::{AppSettings, Paths};

/// `<data>/tools`: DXVK, VKD3D, winetricks, runtimes, Wine and Proton builds.
pub fn tools_dir() -> PathBuf {
    Paths::get().tools_dir()
}

/// The folder the user chose for downloaded Wine and Proton builds, if any.
fn compat_tools_root(app: &AppSettings) -> Option<PathBuf> {
    let configured = app.compat_tools_path.trim();
    (!configured.is_empty()).then(|| irori_core::paths::expand_home(configured))
}

/// Where the Wine manager installs the Wine builds it downloads.
pub fn wine_downloads_dir(app: &AppSettings) -> PathBuf {
    compat_tools_root(app).map_or_else(|| Paths::get().wine_dir(), |root| root.join("wine"))
}

/// Where the Wine manager installs the Proton builds it downloads, unless
/// they go into Steam.
pub fn proton_downloads_dir(app: &AppSettings) -> PathBuf {
    compat_tools_root(app).map_or_else(|| Paths::get().proton_dir(), |root| root.join("proton"))
}

/// `<data>/tools/runtimes`: umu, EAC and BattlEye runtimes.
pub fn runtimes_dir() -> PathBuf {
    tools_dir().join("runtimes")
}

/// Where Irori installs umu-run when the system has none.
pub fn default_umu_path() -> PathBuf {
    runtimes_dir().join("umu").join("umu_run.py")
}

/// The winetricks script Irori downloads.
pub fn winetricks_path() -> PathBuf {
    tools_dir().join("winetricks")
}

/// Cached Wine/Proton release list (the Wine manager's catalogue and install
/// state).
pub fn releases_file() -> PathBuf {
    Paths::get().state_dir().join("wine-releases.json")
}

/// Heroic's config directory (`~/.config/heroic` or the Flatpak one), if any.
pub fn heroic_dir() -> Option<PathBuf> {
    irori_core::paths::heroic_config_dir().or_else(|| {
        let dir = dirs::config_dir()?.join("heroic");
        dir.join("tools").is_dir().then_some(dir)
    })
}

/// Heroic's tools directory, when it exists.
pub fn heroic_tools_dir() -> Option<PathBuf> {
    heroic_dir().map(|d| d.join("tools")).filter(|d| d.is_dir())
}

pub fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

/// Whether Irori runs inside a Flatpak sandbox.
pub fn is_flatpak() -> bool {
    std::env::var_os("FLATPAK_ID").is_some()
}

/// Steam Deck game mode (gamescope session): MangoHud and gamescope are
/// provided by the session and must not be stacked.
pub fn is_steam_deck_game_mode() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|v| v == "gamescope")
}

/// The user's real home as seen by the host (`~/.var/app/<id>` inside a
/// Flatpak). Used for `PROTON_LOG_DIR`.
///
/// Heroic derives this by removing `/data` from `$XDG_DATA_HOME`, which on a
/// native install with `XDG_DATA_HOME=~/.local/share` yields a wrong path; this
/// only applies the substitution inside a Flatpak.
pub fn flatpak_home() -> PathBuf {
    if is_flatpak()
        && let Ok(data) = std::env::var("XDG_DATA_HOME")
        && let Some(stripped) = data.strip_suffix("/data")
    {
        return PathBuf::from(stripped);
    }
    home()
}

/// CrossOver's bottles folder: the configured one (`~` expanded), else
/// CrossOver's default.
pub fn crossover_bottles_dir(app: &AppSettings) -> PathBuf {
    let configured = app.crossover_bottles_path.trim();
    if configured.is_empty() {
        home().join("Library/Application Support/CrossOver/Bottles")
    } else {
        irori_core::paths::expand_home(configured)
    }
}

/// The Steam installation: the configured path (quotes removed, `~`
/// expanded) when it holds a Steam library, else on Linux the native,
/// Flatpak or Snap Steam with one, else the platform's default.
///
/// The setting starts out as the platform's default folder, so a folder that
/// merely exists there must not hide a Flatpak Steam.
pub fn steam_path(app: &AppSettings) -> PathBuf {
    let configured = app.default_steam_path.replace('\'', "");
    if !configured.trim().is_empty() {
        let p = irori_core::paths::expand_home(configured.trim());
        if p.join("steamapps").is_dir() {
            return p;
        }
    }
    if !cfg!(target_os = "linux") {
        return irori_core::platform::steam_dir();
    }
    irori_core::platform::find_linux_steam(&home(), "steamapps")
        .unwrap_or_else(irori_core::platform::steam_dir)
}

/// Value for `STEAM_COMPAT_CLIENT_INSTALL_PATH`.
///
/// Heroic uses `<flatpakHome>/.steam/steam`, which is wrong whenever
/// `XDG_DATA_HOME` is customised; Irori points at the real Steam directory.
pub fn steam_client_install_path(app: &AppSettings) -> PathBuf {
    steam_path(app)
}

/// Pick the first existing path, else the first candidate.
pub fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|p| p.exists())
        .or_else(|| candidates.first())
        .cloned()
}

/// Canonical form for de-duplication (falls back to the input).
pub fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_steam_installed_as_a_snap() {
        let home = tempfile::tempdir().unwrap();
        let snap = home.path().join("snap/steam/common/.local/share/Steam");
        std::fs::create_dir_all(snap.join("steamapps")).unwrap();
        let found = irori_core::platform::find_linux_steam(home.path(), "steamapps");
        assert_eq!(found, Some(snap));
    }

    #[test]
    fn a_flatpak_steam_wins_over_an_empty_native_folder() {
        let home = tempfile::tempdir().unwrap();
        let native = home.path().join(".steam/steam");
        std::fs::create_dir_all(native.join("steamapps")).unwrap();
        let flatpak = home
            .path()
            .join(".var/app/com.valvesoftware.Steam/.local/share/Steam");
        std::fs::create_dir_all(flatpak.join("steamapps")).unwrap();
        std::fs::write(flatpak.join("steamapps/libraryfolders.vdf"), "").unwrap();
        let found = irori_core::platform::find_linux_steam(home.path(), "steamapps");
        assert_eq!(found, Some(flatpak));
    }
}
