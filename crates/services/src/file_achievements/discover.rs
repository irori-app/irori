//! Finding a game's Steam app id and the files its Steam API emulator
//! keeps achievements in. Blocking file reads only; callers run it on the
//! blocking pool.
//!
//! The emulator's folder is wherever its `steam_api(64).dll` or
//! `libsteam_api.so` sits (or a `steam_settings` folder, which sits beside
//! it), found in the install folder a few levels deep. Saves are then
//! looked for in:
//!
//! * the portable folder: gbe_fork's `local_save_path` from
//!   `steam_settings/configs.user.ini`, or Goldberg's `local_save.txt`
//!   beside the library, both relative to the library's folder;
//! * the emulators' global folders (`GSE Saves`, `Goldberg SteamEmu Saves`
//!   or the name `saves_folder_name` gives): `%APPDATA%` for Windows games
//!   (in their Wine prefix: every `drive_c/users/<user>/AppData/Roaming`),
//!   `$XDG_DATA_HOME` or `~/.local/share` for native ones;
//! * for Windows games, CODEX's and RUNE's `Steam/CODEX` and `Steam/RUNE`
//!   under Public Documents, and `Steam/CODEX` under `%APPDATA%`.
//!
//! Each holds `<appid>/achievements.json` (Goldberg) or
//! `<appid>/achievements.ini` (CODEX, RUNE). Every file found adds to the
//! unlocks.

use std::path::{Path, PathBuf};

use irori_core::Achievement;

use super::formats::{self, LocalUnlock, Unlocks};

/// Global save folder names of Goldberg (`Goldberg SteamEmu Saves`) and
/// gbe_fork (`GSE Saves`).
const SAVE_FOLDERS: [&str; 2] = ["GSE Saves", "Goldberg SteamEmu Saves"];
/// Files that mark a Steam API library's folder.
const LIBRARIES: [&str; 4] = [
    "steam_api.dll",
    "steam_api64.dll",
    "libsteam_api.so",
    "libsteam_api.dylib",
];
/// How deep under the install folder libraries are looked for.
const MAX_DEPTH: usize = 4;
/// Folder entries looked at before giving up, so a huge install stays quick.
const MAX_ENTRIES: usize = 20_000;

/// What is known of a game before looking at its files.
#[derive(Debug, Clone, Default)]
pub struct GameFiles {
    pub install_dir: PathBuf,
    /// The program Irori starts, when known.
    pub exe: Option<PathBuf>,
    /// The game's environment variables (`enviromentOptions`).
    pub variables: Vec<(String, String)>,
    /// The app id the user chose.
    pub chosen_app_id: Option<u32>,
    /// The Wine prefix holding `drive_c`, for a Windows game run through
    /// Wine or Proton.
    pub prefix: Option<PathBuf>,
    /// A Windows build (in a prefix, or on Windows itself).
    pub windows: bool,
}

/// This computer's folders the emulators save to, for games that don't run
/// in a prefix.
#[derive(Debug, Clone, Default)]
pub struct HostDirs {
    /// `%APPDATA%` (Windows).
    pub appdata: Option<PathBuf>,
    /// Public Documents (Windows).
    pub public_documents: Option<PathBuf>,
    /// `$XDG_DATA_HOME`, else `~/.local/share` (every other system).
    pub data_home: Option<PathBuf>,
}

impl HostDirs {
    pub fn current() -> Self {
        if cfg!(windows) {
            HostDirs {
                appdata: dirs::config_dir(),
                public_documents: std::env::var_os("PUBLIC")
                    .map(|p| PathBuf::from(p).join("Documents")),
                data_home: None,
            }
        } else {
            let xdg = std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute());
            HostDirs {
                appdata: None,
                public_documents: None,
                data_home: xdg.or_else(|| dirs::home_dir().map(|h| h.join(".local").join("share"))),
            }
        }
    }
}

/// Where an app id came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppIdSource {
    Chosen,
    File,
    Variable,
}

/// What a game's files hold.
#[derive(Debug, Clone, Default)]
pub struct Found {
    pub app_id: u32,
    pub source: Option<AppIdSource>,
    pub unlocks: Unlocks,
    /// The definitions in the game's `steam_settings/achievements.json`.
    pub definitions: Vec<Achievement>,
    /// Save or definition files were found.
    pub emulator_files: bool,
}

/// Folders holding a Steam API library or `steam_settings`: the program's
/// folder and the install folder first, then what lies under it.
pub fn emulator_dirs(game: &GameFiles) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let add = |d: PathBuf, dirs: &mut Vec<PathBuf>| {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    };
    if let Some(parent) = game.exe.as_deref().and_then(Path::parent) {
        add(parent.to_path_buf(), &mut dirs);
    }
    add(game.install_dir.clone(), &mut dirs);
    let mut seen = 0usize;
    let mut stack = vec![(game.install_dir.clone(), 0usize)];
    let mut found = Vec::new();
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut marked = false;
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                break;
            }
            // `file_type` doesn't follow links, so a link can't lead the
            // walk out of the game's folder.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if kind.is_dir() {
                if name.eq_ignore_ascii_case("steam_settings") {
                    marked = true;
                } else if depth < MAX_DEPTH {
                    stack.push((entry.path(), depth + 1));
                }
            } else if LIBRARIES.iter().any(|l| name.eq_ignore_ascii_case(l)) {
                marked = true;
            }
        }
        if marked {
            found.push(dir);
        }
        if seen > MAX_ENTRIES {
            break;
        }
    }
    found.sort();
    for d in found {
        add(d, &mut dirs);
    }
    dirs
}

fn read(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|b| formats::decode(&b))
}

/// A child of `dir` named `name` in any letter case (Windows games don't
/// care, and their files may arrive spelled either way on Linux).
fn child(dir: &Path, name: &str) -> PathBuf {
    let exact = dir.join(name);
    if exact.exists() {
        return exact;
    }
    std::fs::read_dir(dir)
        .ok()
        .and_then(|entries| {
            entries
                .flatten()
                .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
                .map(|e| e.path())
        })
        .unwrap_or(exact)
}

/// The game's app id, first match wins: chosen by hand,
/// `steam_settings/steam_appid.txt`, `steam_appid.txt` beside the program
/// or in the install folder (or beside the library), then `SteamAppId` or
/// `SteamGameId` in the game's variables.
pub fn app_id(game: &GameFiles, dirs: &[PathBuf]) -> Option<(u32, AppIdSource)> {
    if let Some(id) = game.chosen_app_id.filter(|id| *id > 0) {
        return Some((id, AppIdSource::Chosen));
    }
    let from_file = |path: PathBuf| read(&path).as_deref().and_then(formats::app_id);
    for dir in dirs {
        if let Some(id) = from_file(child(&child(dir, "steam_settings"), "steam_appid.txt")) {
            return Some((id, AppIdSource::File));
        }
    }
    // `dirs` starts with the program's folder and the install folder.
    for dir in dirs {
        if let Some(id) = from_file(child(dir, "steam_appid.txt")) {
            return Some((id, AppIdSource::File));
        }
    }
    for key in ["SteamAppId", "SteamGameId"] {
        if let Some(id) = game
            .variables
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| formats::app_id(v))
        {
            return Some((id, AppIdSource::Variable));
        }
    }
    None
}

/// The user folders of a Wine prefix (`drive_c/users/*`).
fn prefix_users(prefix: &Path) -> Vec<PathBuf> {
    let users = child(&child(prefix, "drive_c"), "users");
    let mut list: Vec<PathBuf> = std::fs::read_dir(&users)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir() || t.is_symlink()))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    list.sort();
    list
}

/// Every `achievements.json` and `achievements.ini` that may hold `app`'s
/// unlocks, in the order they are read.
pub fn save_files(game: &GameFiles, host: &HostDirs, dirs: &[PathBuf], app: u32) -> Vec<PathBuf> {
    let app = app.to_string();
    let mut goldberg_roots: Vec<PathBuf> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for dir in dirs {
        let settings = child(dir, "steam_settings");
        if let Some(text) = read(&child(&settings, "configs.user.ini")) {
            let saves = formats::save_settings(&text);
            if let Some(path) = saves.local_save_path {
                goldberg_roots.push(dir.join(path));
            }
            names.extend(saves.saves_folder_name);
        }
        if let Some(text) = read(&child(dir, "local_save.txt")) {
            // Goldberg takes the file's text as a folder beside the library;
            // an empty file means the library's own folder.
            let name = text.lines().next().unwrap_or_default().trim();
            goldberg_roots.push(if name.is_empty() {
                dir.clone()
            } else {
                dir.join(name)
            });
        }
    }
    names.extend(SAVE_FOLDERS.iter().map(|s| s.to_string()));
    names.dedup();

    let mut appdata: Vec<PathBuf> = Vec::new();
    let mut public_documents: Vec<PathBuf> = Vec::new();
    let mut native_data: Vec<PathBuf> = Vec::new();
    match &game.prefix {
        Some(prefix) => {
            for user in prefix_users(prefix) {
                appdata.push(child(&child(&user, "AppData"), "Roaming"));
                appdata.push(child(&user, "Application Data"));
                public_documents.push(child(&user, "Documents"));
            }
        }
        None if game.windows => {
            appdata.extend(host.appdata.clone());
            public_documents.extend(host.public_documents.clone());
        }
        None => native_data.extend(host.data_home.clone()),
    }
    for base in appdata.iter().chain(&native_data) {
        for name in &names {
            goldberg_roots.push(base.join(name));
        }
    }

    let mut files: Vec<PathBuf> = goldberg_roots
        .iter()
        .map(|root| child(&child(root, &app), "achievements.json"))
        .collect();
    if game.windows {
        let mut codex_roots: Vec<PathBuf> = Vec::new();
        for docs in &public_documents {
            let steam = child(docs, "Steam");
            codex_roots.push(child(&steam, "CODEX"));
            codex_roots.push(child(&steam, "RUNE"));
        }
        for base in &appdata {
            codex_roots.push(child(&child(base, "Steam"), "CODEX"));
        }
        files.extend(
            codex_roots
                .iter()
                .map(|root| child(&child(root, &app), "achievements.ini")),
        );
    }
    let mut unique = Vec::new();
    for f in files {
        if !unique.contains(&f) {
            unique.push(f);
        }
    }
    unique
}

/// Read every unlock in `files` that exists.
pub fn read_unlocks(files: &[PathBuf]) -> (Unlocks, bool) {
    let mut unlocks = Unlocks::new();
    let mut any = false;
    for file in files {
        let Some(text) = read(file) else {
            continue;
        };
        let is_ini = file
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("ini"));
        let found = if is_ini {
            formats::codex_unlocks(&text)
        } else {
            match formats::goldberg_unlocks(&text) {
                Ok(found) => found,
                Err(e) => {
                    tracing::debug!("reading {}: {e:#}", file.display());
                    continue;
                }
            }
        };
        any = true;
        for (name, unlock) in found {
            let merged = match unlocks.get(&name) {
                Some(old) => old.merge(unlock),
                None => unlock,
            };
            unlocks.insert(name, merged);
        }
    }
    (unlocks, any)
}

/// The definitions the first `steam_settings/achievements.json` holds.
pub fn read_definitions(dirs: &[PathBuf], steam_language: &str) -> Vec<Achievement> {
    for dir in dirs {
        let settings = child(dir, "steam_settings");
        let Some(text) = read(&child(&settings, "achievements.json")) else {
            continue;
        };
        match formats::goldberg_definitions(&text, steam_language, &settings) {
            Ok(defs) if !defs.is_empty() => return defs,
            Ok(_) => {}
            Err(e) => tracing::debug!("reading {}: {e:#}", settings.display()),
        }
    }
    Vec::new()
}

/// Everything a game's files say about its achievements. `None` without an
/// app id.
pub fn discover(game: &GameFiles, host: &HostDirs, steam_language: &str) -> Option<Found> {
    let dirs = emulator_dirs(game);
    let (app_id, source) = app_id(game, &dirs)?;
    let (unlocks, saves) = read_unlocks(&save_files(game, host, &dirs, app_id));
    let definitions = read_definitions(&dirs, steam_language);
    Some(Found {
        app_id,
        source: Some(source),
        emulator_files: saves || !definitions.is_empty(),
        unlocks,
        definitions,
    })
}

/// A record of unlocks with nothing else known: each name as written.
pub fn bare_definitions(unlocks: &Unlocks) -> Vec<Achievement> {
    unlocks
        .keys()
        .map(|id| Achievement {
            id: id.clone(),
            key: id.clone(),
            visible: true,
            name: id.clone(),
            ..Default::default()
        })
        .collect()
}

/// Unlock state for `id`, matching the emulator's spelling without case.
pub fn unlock_for<'a>(unlocks: &'a Unlocks, id: &str) -> Option<&'a LocalUnlock> {
    unlocks.get(id).or_else(|| {
        unlocks
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(id))
            .map(|(_, u)| u)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn game(root: &Path) -> GameFiles {
        GameFiles {
            install_dir: root.join("Game"),
            exe: Some(root.join("Game/bin/game.exe")),
            ..Default::default()
        }
    }

    #[test]
    fn finds_the_app_id_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut g = game(root);
        g.variables = vec![("SteamGameId".into(), "30".into())];
        write(&root.join("Game/bin/game.exe"), "");
        let dirs = emulator_dirs(&g);
        assert_eq!(app_id(&g, &dirs), Some((30, AppIdSource::Variable)));
        g.variables.insert(0, ("SteamAppId".into(), "20".into()));
        assert_eq!(app_id(&g, &dirs), Some((20, AppIdSource::Variable)));

        write(&root.join("Game/steam_appid.txt"), "10\n");
        assert_eq!(app_id(&g, &dirs), Some((10, AppIdSource::File)));
        write(&root.join("Game/bin/steam_appid.txt"), "11");
        assert_eq!(app_id(&g, &dirs), Some((11, AppIdSource::File)));

        // steam_settings beside a library deeper down beats both.
        write(&root.join("Game/bin/x64/steam_api64.dll"), "");
        write(
            &root.join("Game/bin/x64/steam_settings/steam_appid.txt"),
            "12",
        );
        let dirs = emulator_dirs(&g);
        assert!(dirs.contains(&root.join("Game/bin/x64")));
        assert_eq!(app_id(&g, &dirs), Some((12, AppIdSource::File)));

        g.chosen_app_id = Some(99);
        assert_eq!(app_id(&g, &dirs), Some((99, AppIdSource::Chosen)));
        g.chosen_app_id = Some(0);
        assert_eq!(app_id(&g, &dirs).map(|(id, _)| id), Some(12));
    }

    #[test]
    fn no_app_id_without_a_hint() {
        let tmp = tempfile::tempdir().unwrap();
        let g = game(tmp.path());
        write(&tmp.path().join("Game/bin/game.exe"), "");
        assert!(discover(&g, &HostDirs::default(), "english").is_none());
    }

    #[test]
    fn reads_saves_in_a_proton_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let prefix = root.join("prefix/pfx");
        let mut g = game(root);
        g.prefix = Some(prefix.clone());
        g.windows = true;
        write(&root.join("Game/bin/steam_appid.txt"), "480");
        write(
            &prefix.join("drive_c/users/steamuser/AppData/Roaming/GSE Saves/480/achievements.json"),
            r#"{"ACH_A": {"earned": true, "earned_time": 100}}"#,
        );
        write(
            &prefix.join(
                "drive_c/users/steamuser/AppData/Roaming/Goldberg SteamEmu Saves/480/achievements.json",
            ),
            r#"{"ACH_B": {"earned": true, "earned_time": 200}}"#,
        );
        write(
            &prefix.join("drive_c/users/Public/Documents/Steam/CODEX/480/achievements.ini"),
            "[ACH_C]\nAchieved=1\nUnlockTime=300\n",
        );
        // Another game's saves stay out.
        write(
            &prefix.join("drive_c/users/steamuser/AppData/Roaming/GSE Saves/481/achievements.json"),
            r#"{"ACH_X": {"earned": true}}"#,
        );
        let found = discover(&g, &HostDirs::default(), "english").unwrap();
        assert_eq!(found.app_id, 480);
        assert!(found.emulator_files);
        assert_eq!(
            found.unlocks.keys().collect::<Vec<_>>(),
            ["ACH_A", "ACH_B", "ACH_C"]
        );
        assert_eq!(found.unlocks["ACH_C"].time, 300);
    }

    #[test]
    fn reads_portable_and_native_saves() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let g = GameFiles {
            install_dir: root.join("Game"),
            exe: Some(root.join("Game/game.x86_64")),
            ..Default::default()
        };
        write(&root.join("Game/lib/libsteam_api.so"), "");
        write(
            &root.join("Game/lib/steam_settings/configs.user.ini"),
            "[user::saves]\nlocal_save_path=./saves\nsaves_folder_name=Custom\n",
        );
        write(&root.join("Game/lib/steam_settings/steam_appid.txt"), "70");
        write(
            &root.join("Game/lib/saves/70/achievements.json"),
            r#"{"ACH_PORTABLE": {"earned": true, "earned_time": 5}}"#,
        );
        let data = root.join("data");
        write(
            &data.join("Custom/70/achievements.json"),
            r#"{"ACH_CUSTOM": {"earned": true}}"#,
        );
        write(
            &data.join("Goldberg SteamEmu Saves/70/achievements.json"),
            r#"{"ACH_OLD": {"earned": false, "progress": 1, "max_progress": 2}}"#,
        );
        // CODEX only for Windows builds.
        let docs = root.join("docs");
        write(
            &docs.join("Steam/CODEX/70/achievements.ini"),
            "[ACH_CODEX]\nAchieved=1\n",
        );
        write(
            &root.join("Game/lib/steam_settings/achievements.json"),
            r#"[{"name": "ACH_PORTABLE", "displayName": "Portable"}]"#,
        );
        let host = HostDirs {
            data_home: Some(data),
            public_documents: Some(docs),
            appdata: None,
        };
        let found = discover(&g, &host, "english").unwrap();
        assert_eq!(found.app_id, 70);
        assert_eq!(
            found.unlocks.keys().collect::<Vec<_>>(),
            ["ACH_CUSTOM", "ACH_OLD", "ACH_PORTABLE"]
        );
        assert_eq!(found.unlocks["ACH_OLD"].progress, Some(0.5));
        assert_eq!(found.definitions[0].name, "Portable");
    }

    #[test]
    fn reads_goldberg_local_save_txt() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut g = game(root);
        g.windows = true;
        write(&root.join("Game/bin/steam_api.dll"), "");
        write(&root.join("Game/bin/steam_appid.txt"), "5");
        write(&root.join("Game/bin/local_save.txt"), "SAVE\n");
        write(
            &root.join("Game/bin/SAVE/5/achievements.json"),
            r#"{"ACH": {"earned": true, "earned_time": 9}}"#,
        );
        let found = discover(&g, &HostDirs::default(), "english").unwrap();
        assert!(found.unlocks["ACH"].earned);
        assert!(found.definitions.is_empty());
    }

    #[test]
    fn a_chosen_id_without_files_finds_nothing_to_read() {
        let tmp = tempfile::tempdir().unwrap();
        let mut g = game(tmp.path());
        g.chosen_app_id = Some(620);
        let found = discover(&g, &HostDirs::default(), "english").unwrap();
        assert_eq!(found.source, Some(AppIdSource::Chosen));
        assert!(!found.emulator_files);
        assert!(found.unlocks.is_empty());
    }
}
