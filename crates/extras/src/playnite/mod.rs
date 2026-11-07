//! Plan an import of a Playnite library (Windows): its installed games,
//! favourites, categories, playtime, last played dates, covers and
//! backgrounds.
//!
//! This module only reads. [`PlayniteDirs::discover`] finds Playnite through
//! the program its `playnite://` link opens (an installed or a portable
//! copy) and the library folder its `config.json` names. [`plan`] reads
//! `games.db` and `categories.db` there with a small LiteDB 4 reader
//! ([`litedb`]) and returns a [`PlaynitePlan`] saying what each game becomes
//! in Irori and, for every game that is not imported, why. Playnite's files
//! are never written; while Playnite runs it holds its library for itself,
//! and the import asks the user to close it.
//!
//! Games of stores Irori lists itself (Steam, Epic Games, GOG, Amazon,
//! itch.io, Xbox, Battle.net, EA, Ubisoft, Rockstar) are matched to them by
//! id; every other installed game is played through Playnite
//! (`playnite://playnite/start/<id>`), followed by its install folder.

pub mod bson;
pub mod litedb;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result};
use irori_core::Runner;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use bson::{Bson, Document, Guid};
use litedb::LiteDb;

/// Where one Playnite keeps its program and library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayniteDirs {
    /// The folder of `Playnite.DesktopApp.exe` (`{PlayniteDir}`).
    pub program_dir: PathBuf,
    /// The library folder: `games.db`, `categories.db`, `files/`.
    pub library_dir: PathBuf,
}

impl PlayniteDirs {
    /// The Playnite whose program the `playnite://` link opens, on Windows.
    /// `None` elsewhere, or without Playnite.
    pub fn discover() -> Option<Self> {
        let program = registered_program()?;
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from)?;
        let dirs = Self::from_program_dir(program.parent()?, &appdata);
        dirs.library_dir.join("games.db").is_file().then_some(dirs)
    }

    /// The library of the Playnite installed in `program_dir`, with Windows'
    /// roaming application data folder at `appdata`. A copy without its
    /// uninstaller is portable and keeps its configuration beside itself;
    /// `DatabasePath` in `config.json` moves the library.
    pub fn from_program_dir(program_dir: &Path, appdata: &Path) -> Self {
        let portable = !program_dir.join("unins000.exe").is_file();
        let config_root = if portable {
            program_dir.to_path_buf()
        } else {
            appdata.join("Playnite")
        };
        let configured = std::fs::read(config_root.join("config.json"))
            .ok()
            .and_then(|raw| {
                let raw = raw.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&raw);
                serde_json::from_slice::<serde_json::Value>(raw).ok()
            })
            .and_then(|v| v.get("DatabasePath")?.as_str().map(str::to_string))
            .filter(|p| !p.trim().is_empty());
        let library_dir = match configured {
            Some(path) => expand_database_path(&path, program_dir, appdata),
            None if portable => program_dir.join("library"),
            None => appdata.join("Playnite").join("library"),
        };
        Self {
            program_dir: program_dir.to_path_buf(),
            library_dir,
        }
    }
}

/// Whether Playnite is installed on this computer (Windows only).
pub fn is_installed() -> bool {
    cfg!(windows) && PlayniteDirs::discover().is_some()
}

/// Playnite's `DatabasePath` with its variables expanded, as Playnite
/// expands it: `{PlayniteDir}`, `%AppData%`, and relative paths against the
/// program's folder (where Playnite runs).
fn expand_database_path(path: &str, program_dir: &Path, appdata: &Path) -> PathBuf {
    let program = program_dir.to_string_lossy();
    let mut text = replace_ignore_case(path.trim(), "{PlayniteDir}", &program);
    text = replace_ignore_case(&text, "%AppData%", &appdata.to_string_lossy());
    if is_absolute_windows(&text) || Path::new(&text).is_absolute() {
        native(&text)
    } else {
        program_dir.join(native(&text))
    }
}

fn replace_ignore_case(text: &str, from: &str, to: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let needle = from.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = lower[at..].find(&needle) {
        out.push_str(&text[at..at + found]);
        out.push_str(to);
        at += found + needle.len();
    }
    out.push_str(&text[at..]);
    out
}

/// `C:\…`, `C:/…` or `\\server\…`.
fn is_absolute_windows(text: &str) -> bool {
    let b = text.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/'))
        || text.starts_with("\\\\")
}

/// A Windows path in this system's separators (tests run elsewhere).
fn native(text: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(text)
    } else {
        PathBuf::from(text.replace('\\', "/"))
    }
}

/// The program of the `playnite://` link: the registry's
/// `HKCU\Software\Classes\Playnite\shell\open\command`, which Playnite
/// writes as `"<folder>\Playnite.DesktopApp.exe" --uridata "%1"`.
#[cfg(windows)]
fn registered_program() -> Option<PathBuf> {
    let key = windows_registry::CURRENT_USER
        .open(r"Software\Classes\Playnite\shell\open\command")
        .ok()?;
    let command = key.get_string("").ok()?;
    command_program(&command).filter(|p| p.is_file())
}

#[cfg(not(windows))]
fn registered_program() -> Option<PathBuf> {
    None
}

/// The program a registry command line starts: its first word, quoted or
/// not.
pub fn command_program(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    let program = match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => command
            .find(" --")
            .map_or(command, |at| &command[..at])
            .trim(),
    };
    (!program.is_empty()).then(|| PathBuf::from(program))
}

/// A store Irori lists itself, as one of Playnite's library plugins.
struct StorePlugin {
    plugin: &'static str,
    runner: Runner,
    /// The store's name, for "log in to …" and the skip reason.
    name: &'static str,
    /// Irori's app name for Playnite's `GameId`.
    app_name: fn(&str) -> String,
    /// The store needs an account to list games (its games are pointed at it
    /// when Irori doesn't have them); otherwise Irori lists what this
    /// computer has installed.
    account: bool,
}

/// Playnite's library plugins for stores Irori lists itself, by plugin id
/// (from the plugins' sources in JosefNemec/PlayniteExtensions; EA's from
/// its last version there, before the plugin moved out).
const STORE_PLUGINS: [StorePlugin; 10] = [
    StorePlugin {
        plugin: "cb91dfc9-b977-43bf-8e70-55f46e410fab",
        runner: Runner::Steam,
        name: "Steam",
        app_name: plain,
        account: false,
    },
    StorePlugin {
        plugin: "00000002-dbd1-46c6-b5d0-b1ba559d10e4",
        runner: Runner::Legendary,
        name: "Epic Games",
        app_name: plain,
        account: true,
    },
    StorePlugin {
        plugin: "aebe8b7c-6dc3-4a66-af31-e7375c6b5e9e",
        runner: Runner::Gog,
        name: "GOG",
        app_name: plain,
        account: true,
    },
    StorePlugin {
        plugin: "402674cd-4af6-4886-b6ec-0e695bfa0688",
        runner: Runner::Nile,
        name: "Amazon Games",
        app_name: plain,
        account: true,
    },
    StorePlugin {
        plugin: "00000001-ebb2-4eec-abcb-7c89937a42bb",
        runner: Runner::Itch,
        name: "itch.io",
        app_name: plain,
        account: true,
    },
    StorePlugin {
        plugin: "7e4fbb5e-2ae3-48d4-8ba0-6b30e7a4e287",
        runner: Runner::Launchers,
        name: "Xbox",
        app_name: |id| format!("xbox:{id}"),
        account: false,
    },
    StorePlugin {
        plugin: "e3c26a3d-d695-4cb7-a769-5ff7612c7edd",
        runner: Runner::Launchers,
        name: "Battle.net",
        // Playnite names products by Blizzard's codes in their own case
        // (`Pro`, `WoW`); Irori in lower case.
        app_name: |id| format!("battlenet:{}", id.to_ascii_lowercase()),
        account: false,
    },
    StorePlugin {
        plugin: "85dd7072-2f20-4e76-a007-41035e390724",
        runner: Runner::Launchers,
        name: "EA app",
        app_name: |id| format!("ea:{id}"),
        account: false,
    },
    StorePlugin {
        plugin: "c2f038e5-8b92-4877-91f1-da9094155fc5",
        runner: Runner::Launchers,
        name: "Ubisoft Connect",
        app_name: |id| format!("ubisoft:{id}"),
        account: false,
    },
    StorePlugin {
        plugin: "88409022-088a-4de8-805a-fdbac291f00a",
        runner: Runner::Launchers,
        name: "Rockstar Games Launcher",
        app_name: |id| format!("rockstar:{}", id.to_ascii_lowercase()),
        account: false,
    },
];

fn plain(id: &str) -> String {
    id.to_string()
}

/// Everything one Playnite library would import.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlaynitePlan {
    /// Every game in the library, including skipped ones.
    pub games: Vec<PlayniteGame>,
    /// Files that could not be read; the plan is still usable.
    pub warnings: Vec<String>,
}

/// One Playnite game and what it becomes in Irori.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayniteGame {
    /// Playnite's id for the game (a Guid, lower case).
    pub id: String,
    pub name: String,
    pub kind: PlayniteKind,
    pub installed: bool,
    pub hidden: bool,
    pub favourite: bool,
    /// Names of the game's categories.
    pub categories: Vec<String>,
    /// Playnite's playtime (seconds) in whole minutes, rounded.
    pub playtime_minutes: u64,
    /// RFC 3339, UTC; `None` when never played.
    pub last_played: Option<String>,
    /// The cover and background: `file://` addresses of files in the
    /// library, or the web addresses Playnite keeps.
    pub cover: Option<String>,
    pub background: Option<String>,
    /// The folder the game is installed in, with Playnite's variables
    /// expanded, when Playnite knows one.
    pub install_dir: Option<PathBuf>,
}

/// What a Playnite game becomes in Irori.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayniteKind {
    /// An added game Playnite starts: Irori opens `link` and follows the
    /// processes running from `install_dir`.
    Playnite { link: String, install_dir: PathBuf },
    /// A game of a store Irori lists itself, matched by `app_name` (the
    /// caller also matches by install folder).
    Store {
        runner: Runner,
        app_name: String,
        /// The store's name ("Battle.net").
        store: String,
        /// The store lists games only with an account.
        account: bool,
    },
    /// Not imported.
    Skipped { reason: PlayniteSkip },
}

/// Why a Playnite game is not imported. [`PlayniteSkip::id`] is a stable
/// key for its translated message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayniteSkip {
    /// Playnite lists it but it is not installed.
    NotInstalled,
    /// Hidden in Playnite.
    Hidden,
    /// Playnite knows no folder for it, so its session couldn't be followed.
    NoInstallFolder,
    /// Its install folder is gone.
    FolderMissing(PathBuf),
}

impl PlayniteSkip {
    /// Stable id, for translation keys and logs.
    pub fn id(&self) -> &'static str {
        match self {
            PlayniteSkip::NotInstalled => "playniteNotInstalled",
            PlayniteSkip::Hidden => "playniteHidden",
            PlayniteSkip::NoInstallFolder => "noInstallFolder",
            PlayniteSkip::FolderMissing(_) => "folderMissing",
        }
    }
}

/// Plan the import of one Playnite library. Reads files directly; call it
/// off the async runtime.
pub fn plan(dirs: &PlayniteDirs) -> Result<PlaynitePlan> {
    let games_db = dirs.library_dir.join("games.db");
    anyhow::ensure!(
        games_db.is_file(),
        "No Playnite library found at {}",
        dirs.library_dir.display()
    );
    let db = LiteDb::open(&games_db)?;
    let collection = db
        .collection_names()
        .find(|n| n.eq_ignore_ascii_case("Game"))
        .unwrap_or("Game")
        .to_string();
    let documents = db.documents(&collection)?;

    let mut out = PlaynitePlan::default();
    let categories = match read_names(&dirs.library_dir.join("categories.db"), "Category") {
        Ok(names) => names,
        Err(e) => {
            out.warnings.push(format!("categories.db: {e:#}"));
            HashMap::new()
        }
    };
    for doc in &documents {
        if let Some(game) = plan_game(dirs, doc, &categories) {
            out.games.push(game);
        }
    }
    Ok(out)
}

/// The names of a collection's items by id (categories, …). A missing file
/// has none.
fn read_names(path: &Path, collection: &str) -> Result<HashMap<Guid, String>> {
    if !path.is_file() {
        return Ok(HashMap::new());
    }
    let db = LiteDb::open(path)?;
    let mut names = HashMap::new();
    let found = db
        .collection_names()
        .find(|n| n.eq_ignore_ascii_case(collection))
        .map(str::to_string);
    let Some(found) = found else {
        return Ok(names);
    };
    for doc in db.documents(&found)? {
        if let (Some(id), Some(name)) = (doc.get("_id").and_then(Bson::as_guid), doc.str("Name")) {
            let name = name.trim();
            if !name.is_empty() {
                names.insert(id, name.to_string());
            }
        }
    }
    Ok(names)
}

fn plan_game(
    dirs: &PlayniteDirs,
    doc: &Document,
    categories: &HashMap<Guid, String>,
) -> Option<PlayniteGame> {
    let id = doc.get("_id").and_then(Bson::as_guid)?;
    let name = doc
        .str("Name")
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("Unnamed game")
        .to_string();
    let installed = doc.bool("IsInstalled");
    let hidden = doc.bool("Hidden");
    let install_dir = install_dir(dirs, doc);
    let plugin = doc.get("PluginId").and_then(Bson::as_guid);
    let game_id = doc.str("GameId").map(str::trim).filter(|g| !g.is_empty());
    let store = plugin.zip(game_id).and_then(|(plugin, game_id)| {
        let plugin = plugin.to_string();
        let store = STORE_PLUGINS.iter().find(|s| s.plugin == plugin)?;
        Some(PlayniteKind::Store {
            runner: store.runner,
            app_name: (store.app_name)(game_id),
            store: store.name.to_string(),
            account: store.account,
        })
    });
    // Store games are matched whether or not they are installed, so their
    // playtime comes along; the caller decides what happens to the others.
    let kind = match store {
        Some(store) => store,
        None if !installed => PlayniteKind::Skipped {
            reason: PlayniteSkip::NotInstalled,
        },
        None if hidden => PlayniteKind::Skipped {
            reason: PlayniteSkip::Hidden,
        },
        None => match &install_dir {
            None => PlayniteKind::Skipped {
                reason: PlayniteSkip::NoInstallFolder,
            },
            Some(dir) if !dir.is_dir() => PlayniteKind::Skipped {
                reason: PlayniteSkip::FolderMissing(dir.clone()),
            },
            Some(dir) => PlayniteKind::Playnite {
                link: format!("playnite://playnite/start/{id}"),
                install_dir: dir.clone(),
            },
        },
    };
    let files = dirs.library_dir.join("files");
    let art = |key: &str| doc.str(key).and_then(|v| art(&files, v));
    let mut names: Vec<String> = doc
        .get("CategoryIds")
        .and_then(Bson::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|c| categories.get(&c.as_guid()?).cloned())
        .collect();
    names.sort();
    names.dedup();
    Some(PlayniteGame {
        id: id.to_string(),
        name,
        kind,
        installed,
        hidden,
        favourite: doc.bool("Favorite"),
        categories: names,
        playtime_minutes: doc
            .get("Playtime")
            .and_then(Bson::as_i64)
            // .NET's ulong, stored as Int64.
            .map(|s| (s as u64).saturating_add(30) / 60)
            .unwrap_or(0),
        last_played: match doc.get("LastActivity") {
            Some(Bson::DateTime(ms)) => rfc3339(*ms),
            _ => None,
        },
        cover: art("CoverImage"),
        background: art("BackgroundImage"),
        install_dir,
    })
}

/// The game's folder: `InstallDirectory`, else the folder of its play
/// action's program or of its first ROM (emulated games often have no
/// install folder, but their emulator's command line names the ROM, which
/// is how the session is followed).
fn install_dir(dirs: &PlayniteDirs, doc: &Document) -> Option<PathBuf> {
    let installed = doc
        .str("InstallDirectory")
        .and_then(|d| expand_game_path(d, dirs));
    if installed.is_some() {
        return installed;
    }
    let action = doc
        .get("GameActions")
        .and_then(Bson::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(Bson::as_document)
        .filter(|a| {
            a.str("Type")
                .is_some_and(|t| t.eq_ignore_ascii_case("File"))
        })
        .find(|a| a.bool("IsPlayAction"))
        .and_then(|a| a.str("Path"))
        .and_then(|p| expand_game_path(p, dirs));
    let rom = || {
        doc.get("Roms")
            .and_then(Bson::as_array)
            .unwrap_or_default()
            .iter()
            .filter_map(Bson::as_document)
            .find_map(|r| expand_game_path(r.str("Path")?, dirs))
    };
    action
        .or_else(rom)
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .filter(|p| p.components().count() > 1)
}

/// A path from a game's data with Playnite's `{PlayniteDir}` expanded;
/// `None` unless it is then absolute (other variables, such as
/// `{InstallDir}` or `{EmulatorDir}`, name nothing Irori can follow).
fn expand_game_path(text: &str, dirs: &PlayniteDirs) -> Option<PathBuf> {
    let text = text.trim().trim_matches('"');
    if text.is_empty() {
        return None;
    }
    let text = replace_ignore_case(text, "{PlayniteDir}", &dirs.program_dir.to_string_lossy());
    if text.contains('{') {
        return None;
    }
    (is_absolute_windows(&text) || Path::new(&text).is_absolute()).then(|| native(&text))
}

/// A cover or background: a file in the library's `files` folder (Playnite
/// keeps those relative, as `<game id>\<file>`), an absolute file, or a web
/// address. Anything that climbs out of `files` is refused.
fn art(files: &Path, value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value.starts_with("http://") || value.starts_with("https://") {
        return url::Url::parse(value).ok().map(|u| u.to_string());
    }
    let path = if is_absolute_windows(value) || Path::new(value).is_absolute() {
        native(value)
    } else {
        let relative = native(value);
        if !relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
        {
            return None;
        }
        files.join(relative)
    };
    if !path.is_file() {
        return None;
    }
    url::Url::from_file_path(&path).ok().map(|u| u.to_string())
}

fn rfc3339(unix_ms: i64) -> Option<String> {
    OffsetDateTime::from_unix_timestamp(unix_ms.div_euclid(1000))
        .ok()?
        .format(&Rfc3339)
        .ok()
}

/// [`plan`] on a blocking thread.
pub async fn plan_async(dirs: PlayniteDirs) -> Result<PlaynitePlan> {
    tokio::task::spawn_blocking(move || plan(&dirs))
        .await
        .context("Playnite import task failed")?
}
