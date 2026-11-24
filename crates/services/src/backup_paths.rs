//! Paths in a backup made elsewhere: on another computer, for another user,
//! on another system, or by Irori's Flatpak where this one is native (and the
//! other way round).
//!
//! A backup records where the computer that made it kept the home folder and
//! Irori's own folders ([`BackupOrigin`]). On restore those roots become
//! this computer's ([`automatic_remaps`]), the user can add folders that
//! moved (an old games drive), and every path in the restored settings is
//! rewritten through them ([`remap_value`]). Paths that still belong to
//! another system afterwards (`/home/…` on Windows, `C:\…` elsewhere) are
//! left out, so the defaults apply instead of a path that can't exist here.
//! Everything here is pure; the restore pass in `backup.rs` checks the disk.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Where the computer that made a backup kept things.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupOrigin {
    /// `linux`, `macos` or `windows`.
    pub os: String,
    pub home: String,
    pub config: String,
    pub data: String,
    pub cache: String,
    /// Made by Irori's Flatpak, whose folders sit in `~/.var/app`.
    pub flatpak: bool,
}

impl BackupOrigin {
    /// This computer, as a backup made now records it.
    pub fn here() -> Self {
        let paths = irori_core::Paths::get();
        let text = |p: &std::path::Path| p.to_string_lossy().into_owned();
        Self {
            os: std::env::consts::OS.to_string(),
            home: dirs::home_dir().as_deref().map(text).unwrap_or_default(),
            config: text(&paths.config),
            data: text(&paths.data),
            cache: text(&paths.cache),
            flatpak: irori_core::host::is_flatpak(),
        }
    }

    fn windows(&self) -> bool {
        self.os == "windows"
    }
}

/// One folder that moved: every path under `from` is now under `to`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remap {
    pub from: String,
    pub to: String,
}

impl Remap {
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
        }
    }
}

/// The roots that moved between the computer that made a backup and this
/// one, most specific first: Irori's own folders, then the home folder.
/// A Flatpak's folders and a native install's differ only in these roots,
/// so the same rules convert between them.
pub fn automatic_remaps(origin: &BackupOrigin, here: &BackupOrigin) -> Vec<Remap> {
    let mut out: Vec<Remap> = [
        (&origin.config, &here.config),
        (&origin.data, &here.data),
        (&origin.cache, &here.cache),
        (&origin.home, &here.home),
    ]
    .into_iter()
    .filter(|(from, to)| !from.trim().is_empty() && !to.trim().is_empty())
    .filter(|(from, to)| !same_path(from, to, origin.windows()))
    .map(|(from, to)| Remap::new(from.as_str(), to.as_str()))
    .collect();
    // Longest first, so a folder inside another is matched before it.
    out.sort_by_key(|r| std::cmp::Reverse(r.from.len()));
    out
}

fn same_path(a: &str, b: &str, windows: bool) -> bool {
    let (a, b) = (trim_end(a), trim_end(b));
    if windows {
        a.replace('\\', "/")
            .eq_ignore_ascii_case(&b.replace('\\', "/"))
    } else {
        a == b
    }
}

fn trim_end(path: &str) -> &str {
    let trimmed = path.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() { path } else { trimmed }
}

/// `path` with the first matching remap applied, its separators turned
/// into this system's (`windows`). A path under no remap stays as written.
pub fn remap_path(path: &str, remaps: &[Remap], windows: bool) -> String {
    for remap in remaps {
        if let Some(rest) = under(path, &remap.from) {
            let to = trim_end(&remap.to);
            if rest.is_empty() {
                return to.to_string();
            }
            let sep = if windows { '\\' } else { '/' };
            let rest: String = rest
                .trim_start_matches(['/', '\\'])
                .chars()
                .map(|c| if c == '/' || c == '\\' { sep } else { c })
                .collect();
            return format!("{to}{sep}{rest}");
        }
    }
    path.to_string()
}

/// What follows `root` in `path` when `path` is `root` or inside it.
/// Windows roots match without letter case and with either separator.
fn under<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    let root = trim_end(root);
    if root.is_empty() || path.len() < root.len() {
        return None;
    }
    let (head, rest) = path.split_at_checked(root.len())?;
    let windows_root = root.contains('\\') || looks_like_drive(root);
    let matches = if windows_root {
        head.replace('\\', "/")
            .eq_ignore_ascii_case(&root.replace('\\', "/"))
    } else {
        head == root
    };
    (matches && (rest.is_empty() || rest.starts_with(['/', '\\']))).then_some(rest)
}

fn looks_like_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Whether `path` is an absolute path of another system than this one
/// (`windows` says which this is): such a path can never exist here.
pub fn foreign_path(path: &str, windows: bool) -> bool {
    let path = path.trim();
    if windows {
        path.starts_with('/')
    } else {
        looks_like_drive(path) && path[2..].starts_with(['\\', '/']) || path.starts_with("\\\\")
    }
}

/// Every string in `value` rewritten through `remaps`.
pub fn remap_value(value: &mut Value, remaps: &[Remap], windows: bool) {
    match value {
        Value::String(s) => {
            let next = remap_path(s, remaps, windows);
            if next != *s {
                *s = next;
            }
        }
        Value::Array(items) => {
            for item in items {
                remap_value(item, remaps, windows);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                remap_value(item, remaps, windows);
            }
        }
        _ => {}
    }
}

/// App settings that name a folder or program of this computer.
pub const APP_PATH_KEYS: &[&str] = &[
    "defaultInstallPath",
    "defaultWinePrefixDir",
    "defaultSteamPath",
    "egsLinkedPath",
    "iroriExtrasPath",
    "iroriCrossoverBottlesPath",
    "iroriCompatToolsPath",
    "iroriBiosFolder",
    "iroriLudusaviPath",
    "iroriLudusaviBackupPath",
];

/// App settings that list folders of this computer.
pub const APP_PATH_LISTS: &[&str] = &["customWinePaths", "iroriRomFolders"];

/// Game settings (the defaults and each game's) that name a path.
pub const GAME_PATH_KEYS: &[&str] = &[
    "winePrefix",
    "targetExe",
    "savesPath",
    "beforeLaunchScriptPath",
    "afterLaunchScriptPath",
];

/// Leave out of `map` the path settings that belong to another system, so
/// the defaults apply. Returns the keys left out.
pub fn drop_foreign(map: &mut Map<String, Value>, game: bool, windows: bool) -> Vec<String> {
    let mut dropped = Vec::new();
    let mut keys: Vec<&str> = GAME_PATH_KEYS.to_vec();
    if !game {
        keys.extend(APP_PATH_KEYS);
    }
    for key in keys {
        if map
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|p| foreign_path(p, windows))
        {
            map.remove(key);
            dropped.push(key.to_string());
        }
    }
    let wine_foreign = map
        .get("wineVersion")
        .and_then(|w| w.get("bin"))
        .and_then(Value::as_str)
        .is_some_and(|p| foreign_path(p, windows));
    if wine_foreign {
        map.remove("wineVersion");
        dropped.push("wineVersion".to_string());
    }
    if !game {
        for key in APP_PATH_LISTS {
            if let Some(Value::Array(items)) = map.get_mut(*key) {
                let before = items.len();
                items.retain(|v| !v.as_str().is_some_and(|p| foreign_path(p, windows)));
                if items.len() != before {
                    dropped.push((*key).to_string());
                }
            }
        }
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux(home: &str) -> BackupOrigin {
        BackupOrigin {
            os: "linux".into(),
            home: home.into(),
            config: format!("{home}/.config/irori"),
            data: format!("{home}/.local/share/irori"),
            cache: format!("{home}/.cache/irori"),
            flatpak: false,
        }
    }

    #[test]
    fn another_user_and_flatpak_roots_move() {
        let flatpak = BackupOrigin {
            os: "linux".into(),
            home: "/home/ana".into(),
            config: "/home/ana/.var/app/io.irori.Irori/config/irori".into(),
            data: "/home/ana/.var/app/io.irori.Irori/data/irori".into(),
            cache: "/home/ana/.var/app/io.irori.Irori/cache/irori".into(),
            flatpak: true,
        };
        let here = linux("/home/bo");
        let remaps = automatic_remaps(&flatpak, &here);
        assert_eq!(
            remap_path(
                "/home/ana/.var/app/io.irori.Irori/data/irori/prefixes/Hades",
                &remaps,
                false
            ),
            "/home/bo/.local/share/irori/prefixes/Hades"
        );
        assert_eq!(
            remap_path("/home/ana/Games/Irori/Hades", &remaps, false),
            "/home/bo/Games/Irori/Hades"
        );
        assert_eq!(remap_path("/home/ana", &remaps, false), "/home/bo");
        // A folder that only starts with the same letters isn't inside.
        assert_eq!(
            remap_path("/home/anabel/x", &remaps, false),
            "/home/anabel/x"
        );
        assert_eq!(remap_path("/mnt/games/x", &remaps, false), "/mnt/games/x");
        // The same computer moves nothing.
        assert!(automatic_remaps(&here, &here).is_empty());
    }

    #[test]
    fn windows_paths_move_without_case_and_with_its_separators() {
        let windows = BackupOrigin {
            os: "windows".into(),
            home: r"C:\Users\Ana".into(),
            config: r"C:\Users\Ana\AppData\Roaming\irori".into(),
            data: r"C:\Users\Ana\AppData\Local\irori".into(),
            cache: r"C:\Users\Ana\AppData\Local\irori\cache".into(),
            flatpak: false,
        };
        let here = BackupOrigin {
            os: "windows".into(),
            home: r"D:\Users\bo".into(),
            config: r"D:\Users\bo\AppData\Roaming\irori".into(),
            data: r"D:\Users\bo\AppData\Local\irori".into(),
            cache: r"D:\Users\bo\AppData\Local\irori\cache".into(),
            flatpak: false,
        };
        let remaps = automatic_remaps(&windows, &here);
        assert_eq!(
            remap_path(r"c:\users\ana\Games\Hades", &remaps, true),
            r"D:\Users\bo\Games\Hades"
        );
        assert_eq!(
            remap_path(r"C:\Users\Ana\AppData\Local\irori\cache\art", &remaps, true),
            r"D:\Users\bo\AppData\Local\irori\cache\art",
            "the most specific root wins"
        );
    }

    #[test]
    fn a_backup_from_windows_restored_on_linux() {
        let origin = BackupOrigin {
            os: "windows".into(),
            home: r"C:\Users\Ana".into(),
            ..Default::default()
        };
        let remaps = automatic_remaps(&origin, &linux("/home/ana"));
        let mut settings = serde_json::json!({
            "defaultInstallPath": r"C:\Users\Ana\Games\Irori",
            "defaultSteamPath": r"C:\Program Files (x86)\Steam",
            "iroriRomFolders": [r"C:\Users\Ana\ROMs", r"E:\ROMs", "/srv/roms"],
            "launcherArgs": "-windowed",
            "wineVersion": {"bin": r"C:\wine\bin\wine.exe", "name": "Wine"},
        });
        remap_value(&mut settings, &remaps, false);
        assert_eq!(settings["defaultInstallPath"], "/home/ana/Games/Irori");
        let map = settings.as_object_mut().unwrap();
        let dropped = drop_foreign(map, false, false);
        assert_eq!(
            dropped,
            ["defaultSteamPath", "wineVersion", "iroriRomFolders"]
        );
        assert_eq!(
            settings["iroriRomFolders"],
            serde_json::json!(["/home/ana/ROMs", "/srv/roms"])
        );
        assert_eq!(settings["launcherArgs"], "-windowed");
    }

    #[test]
    fn foreign_paths() {
        assert!(foreign_path("/home/ana/x", true));
        assert!(!foreign_path(r"C:\Games", true));
        assert!(foreign_path(r"C:\Games", false));
        assert!(foreign_path("D:/Games", false));
        assert!(foreign_path(r"\\nas\games", false));
        assert!(!foreign_path("/home/ana", false));
        assert!(!foreign_path("C:", false), "not a path");
        assert!(!foreign_path("", false));
    }
}
