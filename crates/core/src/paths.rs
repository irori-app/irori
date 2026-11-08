//! Filesystem layout. Everything lives under XDG directories; `IRORI_HOME`
//! relocates all of it (used by tests), and so does a `portable` file beside
//! `irori.exe` (the portable Windows zip).

use std::path::{Path, PathBuf};

use once_cell::sync::OnceCell;

use crate::runner::{GameId, Runner};

static PATHS: OnceCell<Paths> = OnceCell::new();

/// The file beside `irori.exe` that keeps everything in the executable's folder.
pub const PORTABLE_MARKER: &str = "portable";

#[derive(Debug, Clone)]
pub struct Paths {
    /// `~/.config/irori`: settings and store credentials.
    pub config: PathBuf,
    /// `~/.local/share/irori`: tools, Wine builds, bundled programs, logs.
    pub data: PathBuf,
    /// `~/.cache/irori`: images, store API caches.
    pub cache: PathBuf,
}

impl Paths {
    /// Process-wide paths, resolved once.
    pub fn get() -> &'static Paths {
        PATHS.get_or_init(Paths::resolve)
    }

    /// Install explicit paths (tests). Returns false when already initialised.
    pub fn init(paths: Paths) -> bool {
        PATHS.set(paths).is_ok()
    }

    pub fn under(root: &Path) -> Paths {
        Paths {
            config: root.join("config"),
            data: root.join("data"),
            cache: root.join("cache"),
        }
    }

    fn resolve() -> Paths {
        if let Some(root) = std::env::var_os("IRORI_HOME") {
            return Paths::under(Path::new(&root));
        }
        if let Some(root) = portable_root() {
            return Paths::under(&root);
        }
        let home = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
        let paths = Paths {
            config: dirs::config_dir()
                .unwrap_or_else(|| home.join(".config"))
                .join("irori"),
            data: dirs::data_local_dir()
                .unwrap_or_else(|| home.join(".local/share"))
                .join("irori"),
            cache: dirs::cache_dir()
                .unwrap_or_else(|| home.join(".cache"))
                .join("irori"),
        };
        paths.adopt_legacy_dirs();
        paths
    }

    /// Irori was called Ember before its first release. Move Ember's folders
    /// over once, so settings, logins and installs survive the rename. Another
    /// program's `ember` folder (the WorldForge client has one) is left alone:
    /// only a config folder holding Ember's own `settings.json` and `state`
    /// counts.
    fn adopt_legacy_dirs(&self) {
        let legacy = |dir: &Path| dir.with_file_name("ember");
        let config = legacy(&self.config);
        if self.config.exists()
            || !config.join("settings.json").is_file()
            || !config.join("state").is_dir()
        {
            return;
        }
        for dir in [&self.config, &self.data, &self.cache] {
            let old = legacy(dir);
            if dir.exists() || !old.is_dir() {
                continue;
            }
            if let Err(e) = std::fs::rename(&old, dir) {
                tracing::warn!("could not move {} to {}: {e}", old.display(), dir.display());
            }
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    /// Sparse per-game settings file.
    pub fn game_settings_file(&self, id: &GameId) -> PathBuf {
        self.config
            .join("games")
            .join(id.runner.id())
            .join(format!("{}.json", sanitize(&id.app_name)))
    }

    /// Launcher-owned state files (download queue, playtime, favourites…).
    pub fn state_dir(&self) -> PathBuf {
        self.config.join("state")
    }

    /// A store's credentials, install records and other state.
    pub fn store_config_dir(&self, runner: Runner) -> PathBuf {
        self.config.join("stores").join(runner.id())
    }

    /// Cached store API responses.
    pub fn store_cache_dir(&self, runner: Runner) -> PathBuf {
        self.cache.join("stores").join(runner.id())
    }

    /// The store window's cookies and site data for a store's website, kept
    /// apart from Irori's own store login.
    pub fn store_window_dir(&self, runner: Runner) -> PathBuf {
        self.data.join("store-window").join(runner.id())
    }

    /// Credentials and installs of a store account other than the first
    /// (see `irori_services::accounts`).
    pub fn account_config_dir(&self, runner: Runner, slot: &str) -> PathBuf {
        self.store_config_dir(runner)
            .join("accounts")
            .join(sanitize(slot))
    }

    /// Caches of a store account other than the first. Kept apart from
    /// [`Paths::store_cache_dir`], which stores clear when logging out.
    pub fn account_cache_dir(&self, runner: Runner, slot: &str) -> PathBuf {
        self.cache
            .join("accounts")
            .join(runner.id())
            .join(sanitize(slot))
    }

    pub fn images_dir(&self) -> PathBuf {
        self.cache.join("images")
    }

    /// Where the user's screenshots are, by folder (rebuilt when missing).
    pub fn screenshot_index_file(&self) -> PathBuf {
        self.cache.join("screenshots.json")
    }

    pub fn tools_dir(&self) -> PathBuf {
        self.data.join("tools")
    }

    /// Downloaded Wine builds.
    pub fn wine_dir(&self) -> PathBuf {
        self.tools_dir().join("wine")
    }

    /// Downloaded Proton builds.
    pub fn proton_dir(&self) -> PathBuf {
        self.tools_dir().join("proton")
    }

    /// Programs Irori places in Wine prefixes (`EpicGamesLauncher.exe`,
    /// `GalaxyCommunication.exe`).
    pub fn bin_dir(&self) -> PathBuf {
        self.data.join("bin")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.data.join("logs")
    }

    pub fn game_log_file(&self, id: &GameId) -> PathBuf {
        self.logs_dir().join("games").join(format!(
            "{}-{}.log",
            id.runner.id(),
            sanitize(&id.app_name)
        ))
    }

    /// Output of installs, updates and repairs of a game.
    pub fn operation_log_file(&self, id: &GameId) -> PathBuf {
        self.logs_dir().join("games").join(format!(
            "{}-{}.install.log",
            id.runner.id(),
            sanitize(&id.app_name)
        ))
    }

    pub fn launcher_log_file(&self) -> PathBuf {
        self.logs_dir().join("irori.log")
    }
}

/// The folder of a portable Irori: `irori.exe`'s own, when a [`PORTABLE_MARKER`]
/// file sits beside it. Only on Windows, where the zip is unpacked anywhere; a
/// Unix `bin` folder may hold an unrelated program called `portable`.
pub fn portable_root() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    portable_root_of(&exe)
}

fn portable_root_of(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    dir.join(PORTABLE_MARKER)
        .is_file()
        .then(|| dir.to_path_buf())
}

/// Whether this Irori keeps its files beside its executable, so it must leave
/// the system alone: no registry entries, Start menu shortcuts or URL handler.
pub fn is_portable() -> bool {
    static PORTABLE: OnceCell<bool> = OnceCell::new();
    *PORTABLE.get_or_init(|| portable_root().is_some())
}

/// Replace characters that are unsafe in file names. The result is always a
/// plain name: never empty, `.` or `..`, which would lead to another folder.
pub fn sanitize(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c => c,
        })
        .collect();
    if clean.chars().all(|c| matches!(c, '.' | ' ')) {
        "_".into()
    } else {
        clean
    }
}

/// Expand a leading `~` to the home directory.
pub fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

/// Heroic's config directory, for import: the native or the Flatpak one.
/// When both exist, the one Heroic wrote to last, so a native folder left
/// behind after a switch to the Flatpak doesn't hide the Flatpak's games.
pub fn heroic_config_dir() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let candidates: Vec<PathBuf> = [
        dirs::config_dir().map(|c| c.join("heroic")),
        Some(home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic")),
    ]
    .into_iter()
    .flatten()
    .collect();
    pick_heroic_dir(&candidates)
}

/// Files Heroic changes as it is used: its settings and window state, the
/// stores' installs, logins and library caches, and added apps.
const HEROIC_ACTIVITY: &[&str] = &[
    "config.json",
    "store/config.json",
    "store_cache/gog_library.json",
    "store_cache/legendary_library.json",
    "store_cache/nile_library.json",
    "gog_store/installed.json",
    "legendaryConfig/legendary/installed.json",
    "legendaryConfig/legendary/user.json",
    "nile_config/nile/installed.json",
    "nile_config/nile/user.json",
    "sideload_apps/library.json",
];

/// The Heroic folder (one holding `config.json`) used last; the first one
/// listed on a tie.
fn pick_heroic_dir(candidates: &[PathBuf]) -> Option<PathBuf> {
    let last_used = |dir: &Path| {
        HEROIC_ACTIVITY
            .iter()
            .filter_map(|p| {
                std::fs::metadata(dir.join(p))
                    .and_then(|m| m.modified())
                    .ok()
            })
            .max()
    };
    let mut best: Option<(&PathBuf, Option<std::time::SystemTime>)> = None;
    for dir in candidates
        .iter()
        .filter(|p| p.join("config.json").is_file())
    {
        let used = last_used(dir);
        if best.as_ref().is_none_or(|(_, b)| used > *b) {
            best = Some((dir, used));
        }
    }
    best.map(|(dir, _)| dir.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_heroic_folder_used_last_wins() {
        use std::time::{Duration, SystemTime};
        let tmp = tempfile::tempdir().unwrap();
        let native = tmp.path().join("native");
        let flatpak = tmp.path().join("flatpak");
        let touch = |path: &Path, age_days: u64| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"{}").unwrap();
            let when = SystemTime::now() - Duration::from_secs(age_days * 86_400);
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(when)
                .unwrap();
        };
        let both = [native.clone(), flatpak.clone()];

        // Only the Flatpak folder is a Heroic folder.
        touch(&flatpak.join("config.json"), 1);
        assert_eq!(pick_heroic_dir(&both), Some(flatpak.clone()));

        // A stale native folder, left after the switch to the Flatpak.
        touch(&native.join("config.json"), 300);
        touch(&native.join("gog_store/installed.json"), 300);
        touch(&flatpak.join("gog_store/installed.json"), 1);
        assert_eq!(pick_heroic_dir(&both), Some(flatpak.clone()));

        // Back to the native one: its installs changed last.
        touch(&native.join("legendaryConfig/legendary/installed.json"), 0);
        assert_eq!(pick_heroic_dir(&both), Some(native.clone()));
        assert_eq!(pick_heroic_dir(&[]), None);
    }

    #[test]
    fn sanitized_names_stay_in_their_folder() {
        for name in ["", ".", "..", "...", " .. ", "  "] {
            assert_eq!(sanitize(name), "_", "{name:?}");
        }
        assert_eq!(sanitize("../a"), ".._a");
        assert_eq!(sanitize("Game: The Sequel"), "Game_ The Sequel");
        assert_eq!(sanitize(".hidden"), ".hidden");
    }

    #[test]
    fn portable_marker_keeps_everything_beside_the_executable() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("irori.exe");
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(portable_root_of(&exe), None);

        std::fs::create_dir(tmp.path().join(PORTABLE_MARKER)).unwrap();
        assert_eq!(portable_root_of(&exe), None, "a folder is not the marker");
        std::fs::remove_dir(tmp.path().join(PORTABLE_MARKER)).unwrap();

        std::fs::write(tmp.path().join(PORTABLE_MARKER), b"").unwrap();
        let root = portable_root_of(&exe).unwrap();
        assert_eq!(root, tmp.path());
        let paths = Paths::under(&root);
        assert_eq!(paths.config, tmp.path().join("config"));
        assert_eq!(paths.data, tmp.path().join("data"));
        assert_eq!(paths.cache, tmp.path().join("cache"));
    }
}
