//! The usual places ROMs already sit: EmuDeck's `Emulation/roms`,
//! RetroDECK's `retrodeck/roms` and plain `ROMs` folders, in the home
//! folder and at the top of other disks. Each holds a folder per system,
//! named the way ES-DE names them (`psx`, `snes`, `n3ds`), which
//! [`systems::by_folder`](crate::systems::by_folder) reads.
//!
//! Finding them only reads: a few known paths, the mount points one or two
//! levels down, and each system folder until its first game. EmuDeck makes
//! a folder for every system with only its notes inside, so a folder
//! counts only once it holds a file the system plays. Nothing is added
//! here; the UI offers the result and adds what the user keeps.

use std::path::{Path, PathBuf};

use crate::systems::{self, System};

/// Entries read per folder before giving up on finding a game in it.
const MAX_ENTRIES: usize = 2000;
/// Disks looked at under each mount folder.
const MAX_MOUNTS: usize = 32;

/// Who laid a ROM folder out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    EmuDeck,
    RetroDeck,
    /// A `ROMs` folder of the user's own.
    Folder,
}

/// One ROM folder found, with its systems that hold games.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomSource {
    pub root: PathBuf,
    pub kind: SourceKind,
    /// Systems Irori knows, with the folder of their games, in folder order.
    pub systems: Vec<(String, PathBuf)>,
    /// Folders holding games of systems Irori doesn't know, by their name.
    pub unknown: Vec<String>,
}

/// Where ROM folders are looked for under `base` (the home folder or the
/// top of a disk), and who lays each out.
fn layouts(base: &Path) -> [(PathBuf, SourceKind); 5] {
    [
        (base.join("Emulation").join("roms"), SourceKind::EmuDeck),
        (base.join("retrodeck").join("roms"), SourceKind::RetroDeck),
        (base.join("ROMs"), SourceKind::Folder),
        (base.join("roms"), SourceKind::Folder),
        (base.join("Roms"), SourceKind::Folder),
    ]
}

/// The tops of other disks: removable and mounted drives (Linux's
/// `/run/media`, `/media`, `/mnt`, macOS's `/Volumes`) or the other drive
/// letters (Windows).
pub fn mount_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(windows) {
        for letter in b'D'..=b'Z' {
            let drive = PathBuf::from(format!("{}:\\", letter as char));
            if drive.is_dir() {
                out.push(drive);
            }
        }
    } else if cfg!(target_os = "macos") {
        out.extend(children(Path::new("/Volumes")));
    } else {
        // `/run/media/<user>/<disk>` on most desktops, `/run/media/<disk>`
        // on older SteamOS, `/media/<user>/<disk>` on Debian's family.
        for parent in ["/run/media", "/media"] {
            for child in children(Path::new(parent)) {
                out.extend(children(&child));
                out.push(child);
            }
        }
        out.extend(children(Path::new("/mnt")));
    }
    out
}

/// The folders in `dir`, a few at most, without following links out of it.
fn children(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .take(MAX_MOUNTS)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect()
}

/// The ROM folders under the home folder `home` and the disks `mounts`,
/// each once (case-insensitive disks name `ROMs` and `roms` alike).
pub fn find(home: &Path, mounts: &[PathBuf]) -> Vec<RomSource> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for base in std::iter::once(home).chain(mounts.iter().map(PathBuf::as_path)) {
        for (root, kind) in layouts(base) {
            if !root.is_dir() {
                continue;
            }
            let id = identity(&root);
            if seen.contains(&id) {
                continue;
            }
            seen.push(id);
            if let Some(source) = read_source(&root, kind) {
                out.push(source);
            }
        }
    }
    out
}

/// What tells two names of one folder apart from two folders: the device
/// and inode on Unix, where case-insensitive disks (macOS, FAT cards) give
/// `ROMs` and `roms` one folder; the resolved path in lower case elsewhere.
#[cfg(unix)]
fn identity(path: &Path) -> String {
    use std::os::unix::fs::MetadataExt as _;
    match std::fs::metadata(path) {
        Ok(m) => format!("{}:{}", m.dev(), m.ino()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(not(unix))]
fn identity(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
        .to_lowercase()
}

/// The systems in a ROM folder that hold games; `None` when none do.
pub fn read_source(root: &Path, kind: SourceKind) -> Option<RomSource> {
    let mut folders = children_sorted(root);
    folders.retain(|f| !file_name(f).starts_with('.'));
    let mut systems = Vec::new();
    let mut unknown = Vec::new();
    for folder in folders {
        let name = file_name(&folder);
        match systems::by_folder(&name) {
            Some(system) => {
                if has_games(&folder, Some(system))
                    && !systems.iter().any(|(id, _)| id == &system.id)
                {
                    systems.push((system.id.clone(), folder));
                }
            }
            None => {
                if has_games(&folder, None) {
                    unknown.push(name);
                }
            }
        }
    }
    (!systems.is_empty() || !unknown.is_empty()).then(|| RomSource {
        root: root.to_path_buf(),
        kind,
        systems,
        unknown,
    })
}

fn children_sorted(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .take(MAX_ENTRIES)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    out.sort();
    out
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Files front ends and EmuDeck leave in system folders: notes, lists and
/// pictures, never games.
fn is_note(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.starts_with('.')
        || matches!(
            lower.rsplit_once('.').map(|(_, ext)| ext),
            Some(
                "txt"
                    | "md"
                    | "xml"
                    | "json"
                    | "ini"
                    | "cfg"
                    | "nfo"
                    | "png"
                    | "jpg"
                    | "jpeg"
                    | "webp"
                    | "gif"
                    | "pdf"
                    | "html"
                    | "db"
            )
        )
}

/// Whether `dir` holds a game of `system` (one of its files, or a game's
/// folder for systems whose games are folders), or, for a system Irori
/// doesn't know, any file that isn't a note. Looks two levels deep and
/// stops at the first game.
fn has_games(dir: &Path, system: Option<&System>) -> bool {
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    let mut read = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            read += 1;
            if read > MAX_ENTRIES {
                return false;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if system.is_some_and(|s| s.folder_games) {
                    return true;
                }
                if depth < 1 {
                    stack.push((entry.path(), depth + 1));
                }
                continue;
            }
            let plays = match system {
                Some(system) => {
                    let ext = name
                        .rsplit_once('.')
                        .map(|(_, e)| e.to_lowercase())
                        .unwrap_or_default();
                    system.extensions.contains(&ext) || matches!(ext.as_str(), "zip" | "7z")
                }
                None => !is_note(&name),
            };
            if plays {
                return true;
            }
        }
    }
    false
}

/// What adding the found folders would do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// System folders to add, with their system: games Irori can play.
    pub add: Vec<(String, PathBuf)>,
    /// System folders the library already scans.
    pub already: usize,
    /// Names of systems with games but no emulator Irori can use here:
    /// known systems by name, unknown ones by their folder's name.
    pub unplayable: Vec<String>,
}

/// The folders to offer from `sources`: those of systems some emulator
/// plays here (`playable`), not already inside (or holding) one of the
/// library's ROM folders `existing`.
pub fn plan(sources: &[RomSource], existing: &[PathBuf], playable: impl Fn(&str) -> bool) -> Plan {
    let mut plan = Plan::default();
    let covered = |folder: &Path| {
        existing
            .iter()
            .any(|e| folder.starts_with(e) || e.starts_with(folder))
    };
    for source in sources {
        for (system, folder) in &source.systems {
            if !playable(system) {
                let name = systems::get(system).map_or(system.clone(), |s| s.name.clone());
                if !plan.unplayable.contains(&name) {
                    plan.unplayable.push(name);
                }
            } else if covered(folder) {
                plan.already += 1;
            } else if !plan.add.iter().any(|(_, f)| f == folder) {
                plan.add.push((system.clone(), folder.clone()));
            }
        }
        for name in &source.unknown {
            if !plan.unplayable.contains(name) {
                plan.unplayable.push(name.clone());
            }
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn finds_emudeck_retrodeck_and_plain_rom_folders_with_games() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let disk = dir.path().join("media/SD");
        // EmuDeck: a folder per system, most with only its notes.
        write(&home.join("Emulation/roms/psx/Crash (USA).chd"));
        write(&home.join("Emulation/roms/snes/systeminfo.txt"));
        write(&home.join("Emulation/roms/snes/metadata.txt"));
        write(&home.join("Emulation/roms/gba/Advance Wars.zip"));
        write(&home.join("Emulation/roms/dreamcast/Shenmue.gdi"));
        write(&home.join("Emulation/roms/saturn/systeminfo.txt"));
        // DOS games are folders.
        write(&home.join("Emulation/roms/dos/KEEN/KEEN1.EXE"));
        // RetroDECK on a card.
        write(&disk.join("retrodeck/roms/n64/Mario.z64"));
        // Nothing playable.
        std::fs::create_dir_all(home.join("ROMs/nes")).unwrap();

        let found = find(&home, std::slice::from_ref(&disk));
        assert_eq!(found.len(), 2, "{found:?}");
        let emudeck = &found[0];
        assert_eq!(emudeck.kind, SourceKind::EmuDeck);
        let ids: Vec<&str> = emudeck.systems.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(ids, ["dos", "gba", "psx"]);
        assert_eq!(emudeck.unknown, ["dreamcast"]);
        assert_eq!(found[1].kind, SourceKind::RetroDeck);
        assert_eq!(found[1].systems[0].0, "n64");
        assert_eq!(found[1].systems[0].1, disk.join("retrodeck/roms/n64"));
    }

    #[test]
    fn es_de_folder_names_name_their_systems() {
        for (folder, id) in [
            ("n3ds", "3ds"),
            ("gc", "gamecube"),
            ("megadrive", "genesis"),
            ("mastersystem", "sms"),
            ("pcengine", "pce"),
            ("megacd", "segacd"),
            ("mame", "arcade"),
        ] {
            assert_eq!(systems::by_folder(folder).map(|s| s.id.as_str()), Some(id));
        }
    }

    #[test]
    fn games_one_level_down_count_and_notes_never_do() {
        let dir = tempfile::tempdir().unwrap();
        let snes = systems::get("snes").unwrap();
        write(&dir.path().join("snes/USA/Zelda.sfc"));
        assert!(has_games(&dir.path().join("snes"), Some(snes)));
        write(&dir.path().join("other/readme.txt"));
        write(&dir.path().join("other/cover.png"));
        assert!(!has_games(&dir.path().join("other"), None));
        assert!(!has_games(&dir.path().join("other"), Some(snes)));
    }

    #[test]
    fn the_plan_adds_what_plays_and_names_what_doesnt() {
        let roms = PathBuf::from("/home/me/Emulation/roms");
        let sources = vec![RomSource {
            root: roms.clone(),
            kind: SourceKind::EmuDeck,
            systems: vec![
                ("psx".into(), roms.join("psx")),
                ("snes".into(), roms.join("snes")),
                ("ps5".into(), roms.join("ps5")),
            ],
            unknown: vec!["switch".into()],
        }];
        let playable = |s: &str| s != "ps5";
        let plan1 = plan(&sources, &[], playable);
        assert_eq!(
            plan1.add,
            [
                ("psx".to_string(), roms.join("psx")),
                ("snes".to_string(), roms.join("snes")),
            ]
        );
        assert_eq!(plan1.unplayable, ["PlayStation 5", "switch"]);

        // Folders added before stay as they are, whether the system's
        // folder or the whole root was added.
        let plan2 = plan(&sources, &[roms.join("snes")], playable);
        assert_eq!(plan2.add.len(), 1);
        assert_eq!(plan2.already, 1);
        let plan3 = plan(&sources, std::slice::from_ref(&roms), playable);
        assert!(plan3.add.is_empty());
        assert_eq!(plan3.already, 2);
    }
}
