//! Scanning ROM folders into library entries. Blocking; run on the blocking
//! pool.
//!
//! A file's system comes from the folder it sits in, its extension, or its
//! header ([`irori_engines::systems::detect`]). Files a `.cue` sheet or an
//! `.m3u` playlist lists belong to that sheet; discs of one game become one
//! entry (with a playlist written for engines that switch discs). DOS,
//! ScummVM and PlayStation 3, 4 and 5 games are folders, the PlayStation
//! ones named by their own title files; PlayStation 3 games may also be disc
//! images, named by the title file inside. Detection results are cached by path, size and
//! modification time, so a rescan of a 10,000-file library only lists
//! folders. Nothing here touches the network.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use irori_engines::systems::{self, System};
use serde::{Deserialize, Serialize};
use sha1::{Digest as _, Sha1};

use crate::names::{self, RomName};

/// Folder levels searched below a ROM folder.
const MAX_DEPTH: usize = 8;

/// One game in the library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RomEntry {
    pub app_name: String,
    pub system: String,
    pub title: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// What the engine opens: a ROM, a `.cue`/`.m3u`, a DOS program, or a
    /// game folder.
    pub path: PathBuf,
    /// The discs of a multi-disc game, in order (empty otherwise).
    #[serde(default)]
    pub discs: Vec<PathBuf>,
    /// Bytes on disk (all discs).
    #[serde(default)]
    pub size: u64,
    /// The file name the title came from, for thumbnails.
    #[serde(default)]
    pub stem: String,
    /// The ROM folder the game was found in.
    pub root: PathBuf,
    /// Metadata for entries discovered inside a local eXo collection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exo: Option<crate::exo::ExoEntryMeta>,
}

/// The version of the detection rules. A file detected under older rules is
/// looked at again, so a newer Irori recognises what an older one didn't
/// (1: disc images by their marker files; 2: Dreamcast, PS Vita and Switch,
/// and the tracks a `.gdi` lists; 3: DOOM's WADs aren't Wii games; 4: DOOM
/// standalone IWADs; 5: local eXo collections).
pub const DETECTION_RULES: u32 = 5;

/// What a file was detected as last time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    pub size: u64,
    pub mtime: u64,
    pub system: Option<String>,
    /// The [`DETECTION_RULES`] it was detected under (0 before they had a
    /// version).
    #[serde(default)]
    pub rules: u32,
}

pub type ScanCache = HashMap<PathBuf, Seen>;

#[derive(Debug, Default)]
pub struct Scanned {
    pub entries: Vec<RomEntry>,
    pub cache: ScanCache,
    /// Game-like files no system claims.
    pub unrecognized: usize,
}

/// A stable id for the game at `source` (its first disc or folder).
pub fn app_name_of(system: &str, source: &Path) -> String {
    let mut h = Sha1::new();
    h.update(system.as_bytes());
    h.update([0]);
    h.update(source.to_string_lossy().as_bytes());
    hex::encode(&h.finalize()[..8])
}

/// Scan `roots`, reusing `previous` detections. Playlists of multi-disc
/// games are written into `playlists`. `progress` is called with the number
/// of games found so far.
pub fn scan(
    roots: &[PathBuf],
    previous: &ScanCache,
    playlists: &Path,
    progress: &dyn Fn(usize),
) -> Scanned {
    let mut out = Scanned::default();
    for root in roots {
        if crate::exo::is_exo_root(root) {
            out.entries.extend(crate::exo::scan_root(root, root));
            progress(out.entries.len());
            continue;
        }
        let hint = root
            .file_name()
            .and_then(|n| systems::by_folder(&n.to_string_lossy()));
        let mut stack = vec![(root.clone(), hint, 0usize)];
        while let Some((dir, hint, depth)) = stack.pop() {
            scan_dir(
                &dir, root, hint, depth, previous, playlists, &mut out, &mut stack,
            );
            progress(out.entries.len());
        }
    }
    out.entries.sort_by_key(|e| e.title.to_lowercase());
    out
}

struct Candidate {
    system: &'static System,
    path: PathBuf,
    size: u64,
    name: RomName,
    stem: String,
}

#[allow(clippy::too_many_arguments)] // one call site; a struct would only rename them
fn scan_dir(
    dir: &Path,
    root: &Path,
    hint: Option<&'static System>,
    depth: usize,
    previous: &ScanCache,
    playlists: &Path,
    out: &mut Scanned,
    stack: &mut Vec<(PathBuf, Option<&'static System>, usize)>,
) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            dirs.push(entry.path());
        } else if meta.is_file() {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            files.push((entry.path(), meta.len(), mtime));
        }
    }
    files.sort();
    dirs.sort();

    // Folder games: each subfolder of a DOS or ScummVM folder is one game.
    if let Some(system) = hint.filter(|s| s.folder_games) {
        for game in &dirs {
            folder_games(system, game, dir, root, 0, &mut out.entries);
        }
        // Disc images of a system whose games are otherwise folders
        // (PlayStation 3 `.iso` files) are games of their own; nothing
        // else here is.
        if !system.disc_files.is_empty() {
            files.retain(|(path, _, _)| system.takes_disc_image(path));
            if !files.is_empty() {
                scan_files(files, root, hint, previous, playlists, out);
            }
            return;
        }
        if system.id == "dos" {
            for (path, size, _) in &files {
                if is_dos_program(path) {
                    let stem = stem_of(path);
                    let name = names::parse(&stem);
                    out.entries.push(RomEntry {
                        app_name: app_name_of(&system.id, path),
                        system: system.id.clone(),
                        title: name.title,
                        tags: name.tags,
                        path: path.clone(),
                        discs: Vec::new(),
                        size: *size,
                        stem,
                        root: root.to_path_buf(),
                        exo: None,
                    });
                }
            }
        }
        return;
    }

    // A folder holding a disc's marker file (`PS3_DISC.SFB`) is a game of
    // that disc's system wherever it sits; its insides are not searched.
    // Likewise, a local eXo collection root is scanned through its LaunchBox
    // catalog instead of walking `Images/` and `!dos/` as loose ROMs.
    let mut rest = Vec::with_capacity(dirs.len());
    for sub in dirs {
        if crate::exo::is_exo_root(&sub) {
            out.entries.extend(crate::exo::scan_root(&sub, root));
            continue;
        }
        match marked_disc_folder(&sub) {
            Some(system) => {
                if let Some(entry) = folder_game(system, &sub, dir, root) {
                    out.entries.push(entry);
                }
            }
            None => rest.push(sub),
        }
    }
    scan_files(files, root, hint, previous, playlists, out);

    if depth < MAX_DEPTH {
        for sub in rest.into_iter().rev() {
            let named = sub
                .file_name()
                .and_then(|n| systems::by_folder(&n.to_string_lossy()));
            stack.push((sub, named.or(hint), depth + 1));
        }
    }
}

/// The folder-games system whose disc marker file sits in `folder`.
fn marked_disc_folder(folder: &Path) -> Option<&'static System> {
    systems::all().iter().find(|s| {
        s.folder_games
            && !s.folder_main.is_empty()
            && s.disc_files.iter().any(|m| folder.join(m).is_file())
    })
}

/// The games among the files of one folder.
fn scan_files(
    files: Vec<(PathBuf, u64, u64)>,
    root: &Path,
    hint: Option<&'static System>,
    previous: &ScanCache,
    playlists: &Path,
    out: &mut Scanned,
) {
    // Files a cue sheet or playlist lists are not games of their own.
    let mut owned: HashSet<PathBuf> = HashSet::new();
    for (path, _, _) in &files {
        match extension(path).as_deref() {
            Some("cue") => owned.extend(cue_tracks(path)),
            Some("gdi") => owned.extend(gdi_tracks(path)),
            Some("m3u") => owned.extend(playlist_entries(path)),
            _ => {}
        }
    }

    let mut candidates = Vec::new();
    for (path, size, mtime) in files {
        if owned.contains(&path) {
            continue;
        }
        let system = match previous.get(&path) {
            Some(seen)
                if seen.size == size && seen.mtime == mtime && seen.rules == DETECTION_RULES =>
            {
                seen.system.as_deref().and_then(systems::get)
            }
            _ => systems::detect(&path, hint),
        };
        out.cache.insert(
            path.clone(),
            Seen {
                size,
                mtime,
                system: system.map(|s| s.id.clone()),
                rules: DETECTION_RULES,
            },
        );
        let Some(system) = system else {
            if is_game_like(&path) {
                out.unrecognized += 1;
            }
            continue;
        };
        // A track `.bin` beside nothing that lists it, or a DOS program
        // outside a DOS folder, is not a game; a disc image of a system
        // whose games are otherwise folders is.
        if system.folder_games && !system.takes_disc_image(&path) {
            continue;
        }
        let (name, stem) = image_name(system, &path);
        candidates.push(Candidate {
            system,
            name,
            stem,
            path,
            size,
        });
    }

    // Group the discs of one game.
    let mut groups: BTreeMap<(String, String), Vec<Candidate>> = BTreeMap::new();
    for c in candidates {
        let key = if c.system.multi_disc && c.name.disc.is_some() {
            (c.system.id.clone(), c.name.group.clone())
        } else {
            (c.system.id.clone(), format!("\0{}", c.path.display()))
        };
        groups.entry(key).or_default().push(c);
    }
    for (_, mut discs) in groups {
        discs.sort_by_key(|c| c.name.disc.unwrap_or(0));
        let first = &discs[0];
        let system = first.system;
        let app_name = app_name_of(&system.id, &first.path);
        let (path, disc_paths) = if discs.len() > 1 {
            let disc_paths: Vec<PathBuf> = discs.iter().map(|c| c.path.clone()).collect();
            let path = if system.disc_images {
                write_playlist(playlists, &app_name, &disc_paths).unwrap_or(first.path.clone())
            } else {
                first.path.clone()
            };
            (path, disc_paths)
        } else {
            (first.path.clone(), Vec::new())
        };
        out.entries.push(RomEntry {
            app_name,
            system: system.id.clone(),
            title: first.name.title.clone(),
            tags: first.name.tags.clone(),
            path,
            discs: disc_paths,
            size: discs.iter().map(|c| c.size).sum(),
            stem: first.stem.clone(),
            root: root.to_path_buf(),
            exo: None,
        });
    }
}

/// The name of the game file at `path`, and the stem thumbnails go by: the
/// file's name, or for a disc image the title file inside names the game
/// ([`systems::image_title`]), as it does for folder games.
fn image_name(system: &System, path: &Path) -> (RomName, String) {
    let mut stem = stem_of(path);
    let mut name = names::parse(&stem);
    if system.takes_disc_image(path)
        && !system.folder_title.is_empty()
        && let Some(title) = systems::image_title(system, path)
    {
        if !name.title.eq_ignore_ascii_case(&title) {
            stem.clone_from(&title);
        }
        name.title = title;
    }
    (name, stem)
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
}

/// Files that look like a game a user would expect to see, for the
/// "not recognised" count.
fn is_game_like(path: &Path) -> bool {
    matches!(
        extension(path).as_deref(),
        Some("zip" | "7z" | "iso" | "bin" | "rom" | "img" | "chd" | "cue")
    )
}

fn is_dos_program(path: &Path) -> bool {
    matches!(extension(path).as_deref(), Some("exe" | "com" | "bat"))
}

/// Programs that set a DOS game up rather than start it.
fn is_dos_helper(path: &Path) -> bool {
    let stem = stem_of(path).to_lowercase();
    [
        "setup", "install", "config", "uninst", "readme", "setsound", "sound",
    ]
    .iter()
    .any(|w| stem.starts_with(w))
}

/// Folder levels searched inside a folder of a system's folder for games
/// whose program sits deeper (`PS3/Demon's Souls/BLUS30443/PS3_GAME/…`).
const NESTED_DEPTH: usize = 3;

/// The games in `folder`, a folder inside the system folder `top`: the
/// folder itself when it is one, else, for systems whose games hold a known
/// program ([`System::folder_main`]), the games in its subfolders. A game's
/// insides are never searched.
fn folder_games(
    system: &System,
    folder: &Path,
    top: &Path,
    root: &Path,
    depth: usize,
    out: &mut Vec<RomEntry>,
) {
    if let Some(entry) = folder_game(system, folder, top, root) {
        // A patch or add-on beside the games (RPCS3's `dev_hdd0/game`) is
        // not a game.
        if !systems::folder_is_game_data(system, folder) {
            out.push(entry);
        }
        return;
    }
    if system.folder_main.is_empty() || depth >= NESTED_DEPTH {
        return;
    }
    let Ok(read) = std::fs::read_dir(folder) else {
        return;
    };
    let mut subs = Vec::new();
    let mut images = Vec::new();
    for e in read.flatten() {
        if e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() {
            subs.push(e.path());
        } else if kind.is_file() && system.takes_disc_image(&e.path()) {
            images.push((e.path(), e.metadata().map_or(0, |m| m.len())));
        }
    }
    subs.sort();
    images.sort();
    // Disc images in a folder of the system's folder (`PS3/Discs/…`).
    for (path, size) in images {
        let system = systems::disc_system(&path).unwrap_or(system);
        let (name, stem) = image_name(system, &path);
        out.push(RomEntry {
            app_name: app_name_of(&system.id, &path),
            system: system.id.clone(),
            title: name.title,
            tags: name.tags,
            path,
            discs: Vec::new(),
            size,
            stem,
            root: root.to_path_buf(),
            exo: None,
        });
    }
    for sub in subs {
        folder_games(system, &sub, top, root, depth + 1, out);
    }
}

/// The folder whose name is the game's in the game folder `folder`: the
/// folder itself, unless it is part of the game's layout (a `PS3_GAME`
/// folder on its own), then the nearest folder above it that isn't, below
/// the system folder `top`. When every folder up to `top` is a layout
/// folder, the game folder's own name is all there is.
fn naming_folder<'a>(system: &System, folder: &'a Path, top: &Path) -> &'a Path {
    folder
        .ancestors()
        .take_while(|dir| *dir != top && dir.starts_with(top))
        .find(|dir| {
            dir.file_name()
                .is_some_and(|n| !system.is_layout_folder(&n.to_string_lossy()))
        })
        .unwrap_or(folder)
}

/// A game folder (DOS, ScummVM, PlayStation 3, 4 and 5), found inside the
/// system folder `top`. Its title is the one its own files give it
/// ([`systems::folder_title`]), else its folder's name.
fn folder_game(system: &System, folder: &Path, top: &Path, root: &Path) -> Option<RomEntry> {
    let path = if system.id == "dos" {
        dos_program(folder)?
    } else if !system.folder_main.is_empty() {
        // Not a game folder unless the program engines open is inside.
        system
            .folder_main
            .iter()
            .map(|rel| folder.join(rel))
            .find(|p| p.is_file())?
    } else {
        // ScummVM finds the game itself; an empty folder is not a game.
        std::fs::read_dir(folder).ok()?.next()?.ok()?;
        folder.to_path_buf()
    };
    let named_by = naming_folder(system, folder, top);
    let mut stem = named_by.file_name()?.to_string_lossy().into_owned();
    let mut name = names::parse(&stem);
    if let Some(title) = systems::folder_title(system, folder) {
        // Thumbnails are looked up by name: a folder named like the game
        // (with its region) says more than the title, a title id less.
        if !name.title.eq_ignore_ascii_case(&title) {
            stem.clone_from(&title);
        }
        name.title = title;
    }
    Some(RomEntry {
        app_name: app_name_of(&system.id, folder),
        system: system.id.clone(),
        title: name.title,
        tags: name.tags,
        path,
        discs: Vec::new(),
        size: 0,
        stem,
        root: root.to_path_buf(),
        exo: None,
    })
}

/// The program that starts the DOS game in `folder`: its DOSBox config if it
/// has one, else the one program that isn't an installer, preferring one
/// named like the folder.
pub fn dos_program(folder: &Path) -> Option<PathBuf> {
    fn programs(dir: &Path) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|r| r.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        out.sort();
        out
    }
    let top = programs(folder);
    if let Some(conf) = top.iter().find(|p| extension(p).as_deref() == Some("conf")) {
        return Some(conf.clone());
    }
    let wanted = folder
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let pick = |list: &[PathBuf]| -> Option<PathBuf> {
        let runnable: Vec<&PathBuf> = list
            .iter()
            .filter(|p| is_dos_program(p) && !is_dos_helper(p))
            .collect();
        runnable
            .iter()
            .find(|p| {
                let s = stem_of(p).to_lowercase();
                !s.is_empty() && (wanted.starts_with(&s) || s.starts_with(&wanted))
            })
            .or_else(|| {
                runnable
                    .iter()
                    .find(|p| extension(p).as_deref() != Some("bat"))
            })
            .or(runnable.first())
            .map(|p| (*p).clone())
    };
    pick(&top).or_else(|| {
        top.iter()
            .filter(|p| p.is_dir())
            .find_map(|sub| pick(&programs(sub)))
    })
}

/// Track files a cue sheet lists.
fn cue_tracks(cue: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(cue) else {
        return Vec::new();
    };
    let dir = cue.parent().unwrap_or(Path::new("."));
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("FILE ")?;
            let name = match rest.strip_prefix('"') {
                Some(r) => r.split('"').next()?,
                None => rest.split_whitespace().next()?,
            };
            Some(dir.join(name))
        })
        .collect()
}

/// Track files a Dreamcast `.gdi` lists: after the track count, one line per
/// track of `number start type sector-size file offset`, the file quoted when
/// its name holds spaces.
fn gdi_tracks(gdi: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(gdi) else {
        return Vec::new();
    };
    let dir = gdi.parent().unwrap_or(Path::new("."));
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let mut rest = line.trim();
            // Number, start sector, type and sector size come first.
            for _ in 0..4 {
                rest = rest.split_once(char::is_whitespace)?.1.trim_start();
            }
            let name = match rest.strip_prefix('"') {
                Some(r) => r.split('"').next()?,
                None => rest.split_whitespace().next()?,
            };
            (!name.is_empty()).then(|| dir.join(name))
        })
        .collect()
}

/// Files an `.m3u` playlist lists.
fn playlist_entries(m3u: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(m3u) else {
        return Vec::new();
    };
    let dir = m3u.parent().unwrap_or(Path::new("."));
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| dir.join(l))
        .collect()
}

fn write_playlist(dir: &Path, app_name: &str, discs: &[PathBuf]) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!("{app_name}.m3u"));
    let text: String = discs.iter().map(|d| format!("{}\n", d.display())).collect();
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        irori_core::json_store::write_bytes(&path, text.as_bytes()).ok()?;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn scans_dreamcast_vita_and_switch_games() {
        use irori_engines::sfo;
        let dir = tempfile::tempdir().unwrap();
        let roms = dir.path().join("roms");
        // A GD-ROM dump: the `.gdi` owns its tracks, quoted names included.
        let gdi = "3\n1 0 4 2352 track01.bin 0\n2 756 0 2352 \"track 02.raw\" 0\n\
                   3 45000 4 2352 track03.bin 0\n";
        touch(
            &roms.join("dreamcast/Shenmue (USA)/Shenmue (USA).gdi"),
            gdi.as_bytes(),
        );
        for track in ["track01.bin", "track 02.raw", "track03.bin"] {
            touch(&roms.join("dreamcast/Shenmue (USA)").join(track), b"x");
        }
        touch(&roms.join("Sega Dreamcast/Ikaruga (Japan).cdi"), b"x");
        // Vita3K's own app folder: each game is its title id's folder.
        touch(&roms.join("ux0/app/PCSE00000/eboot.bin"), b"x");
        touch(
            &roms.join("ux0/app/PCSE00000/sce_sys/param.sfo"),
            &sfo::build(&[("TITLE", "Gravity Rush"), ("TITLE_ID", "PCSE00000")]),
        );
        touch(&roms.join("switch/Some Game [0100000000010000].nsp"), b"x");
        touch(&roms.join("loose/Other Game.xci"), b"x");

        let found = scan(
            std::slice::from_ref(&roms),
            &ScanCache::new(),
            &dir.path().join("playlists"),
            &|_| {},
        );
        let mut got: Vec<(&str, &str, String)> = found
            .entries
            .iter()
            .map(|e| {
                let file = e.path.file_name().unwrap().to_string_lossy().into_owned();
                (e.system.as_str(), e.title.as_str(), file)
            })
            .collect();
        got.sort();
        assert_eq!(
            got,
            [
                ("dreamcast", "Ikaruga", "Ikaruga (Japan).cdi".to_string()),
                ("dreamcast", "Shenmue", "Shenmue (USA).gdi".to_string()),
                ("psvita", "Gravity Rush", "eboot.bin".to_string()),
                ("switch", "Other Game", "Other Game.xci".to_string()),
                (
                    "switch",
                    "Some Game",
                    "Some Game [0100000000010000].nsp".to_string()
                ),
            ]
        );
        assert_eq!(found.unrecognized, 0);
    }

    #[test]
    fn scans_systems_discs_and_folder_games() {
        let dir = tempfile::tempdir().unwrap();
        let roms = dir.path().join("roms");
        touch(&roms.join("SNES/Chrono Trigger (USA).sfc"), b"x");
        touch(&roms.join("SNES/notes.txt"), b"x");
        touch(&roms.join("loose/Metroid Fusion (USA).gba"), b"x");
        // A disc game with its tracks, and a two-disc game.
        touch(
            &roms.join("PS1/Vagrant Story (USA).cue"),
            b"FILE \"Vagrant Story (USA).bin\" BINARY\n",
        );
        touch(&roms.join("PS1/Vagrant Story (USA).bin"), b"x");
        touch(&roms.join("PS1/Final Fantasy VII (USA) (Disc 1).chd"), b"x");
        touch(&roms.join("PS1/Final Fantasy VII (USA) (Disc 2).chd"), b"x");
        // DOS folders and a ScummVM folder.
        touch(&roms.join("DOS/Commander Keen/KEEN1.EXE"), b"MZ");
        touch(&roms.join("DOS/Commander Keen/SETUP.EXE"), b"MZ");
        touch(&roms.join("ScummVM/Monkey Island/MONKEY.000"), b"x");
        touch(
            &roms.join("PS3/Demon's Souls/PS3_GAME/USRDIR/EBOOT.BIN"),
            b"x",
        );
        touch(&roms.join("PS3/Empty Folder/readme.txt"), b"x");
        // PS5 dumps are named by title id; `param.json` names the game.
        touch(&roms.join("PS5/PPSA00000/eboot.bin"), b"\x7fELF");
        touch(
            &roms.join("PS5/PPSA00000/sce_sys/param.json"),
            br#"{"localizedParameters": {"en-US": {"titleName": "Zone of Test"}}}"#,
        );
        touch(&roms.join("MAME/pacman.zip"), b"PK");
        touch(&roms.join("PS1/unknown.zip"), b"PK");
        touch(&roms.join("other/mystery.iso"), &[0u8; 64]);

        let playlists = dir.path().join("playlists");
        let found = scan(
            std::slice::from_ref(&roms),
            &ScanCache::new(),
            &playlists,
            &|_| {},
        );
        let titles: Vec<(&str, &str)> = found
            .entries
            .iter()
            .map(|e| (e.system.as_str(), e.title.as_str()))
            .collect();
        assert_eq!(
            titles,
            [
                ("snes", "Chrono Trigger"),
                ("dos", "Commander Keen"),
                ("ps3", "Demon's Souls"),
                ("psx", "Final Fantasy VII"),
                ("gba", "Metroid Fusion"),
                ("scummvm", "Monkey Island"),
                ("arcade", "pacman"),
                ("psx", "unknown"),
                ("psx", "Vagrant Story"),
                ("ps5", "Zone of Test"),
            ]
        );
        let ff7 = &found.entries[3];
        assert_eq!(ff7.discs.len(), 2);
        assert_eq!(ff7.path.extension().unwrap(), "m3u");
        let playlist = std::fs::read_to_string(&ff7.path).unwrap();
        assert!(playlist.contains("(Disc 1).chd\n") && playlist.contains("(Disc 2).chd\n"));
        assert!(found.entries[1].path.ends_with("KEEN1.EXE"));
        assert_eq!(found.entries[0].tags, ["USA"]);
        assert_eq!(found.unrecognized, 1);

        // A rescan reuses detections and keeps ids stable.
        let again = scan(&[roms], &found.cache, &playlists, &|_| {});
        let ids = |s: &Scanned| {
            s.entries
                .iter()
                .map(|e| e.app_name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&again), ids(&found));
    }

    #[test]
    fn names_playstation_folder_games_from_param_sfo() {
        use irori_engines::sfo;
        let dir = tempfile::tempdir().unwrap();
        let roms = dir.path().join("roms");
        // A disc dump named by its serial, its title in `PS3_GAME`.
        touch(&roms.join("PS3/BLUS30443/PS3_GAME/USRDIR/EBOOT.BIN"), b"x");
        touch(
            &roms.join("PS3/BLUS30443/PS3_GAME/PARAM.SFO"),
            &sfo::build(&[("TITLE", "Demon's Souls™"), ("TITLE_ID", "BLUS30443")]),
        );
        // A folder named like the game keeps its name for thumbnails.
        touch(&roms.join("PS3/Flower (USA)/USRDIR/EBOOT.BIN"), b"x");
        touch(
            &roms.join("PS3/Flower (USA)/PARAM.SFO"),
            &sfo::build(&[("TITLE", "Flower")]),
        );
        // A game one folder deeper than usual.
        touch(
            &roms.join("PS3/Collection/BCES00001/PS3_GAME/USRDIR/EBOOT.BIN"),
            b"x",
        );
        touch(
            &roms.join("PS3/Collection/BCES00001/PS3_GAME/PARAM.SFO"),
            &sfo::build(&[("TITLE", "Nested\nGame")]),
        );
        // A `PS3_GAME` folder on its own without a title file: nothing
        // better than its own name, never the system's folder.
        touch(&roms.join("PS3/PS3_GAME/USRDIR/EBOOT.BIN"), b"x");
        touch(&roms.join("PS4/CUSA00572/eboot.bin"), b"x");
        touch(
            &roms.join("PS4/CUSA00572/sce_sys/param.sfo"),
            &sfo::build(&[("TITLE", "Sample Quest")]),
        );
        // A damaged file leaves the folder's name.
        touch(&roms.join("PS4/CUSA00001/eboot.bin"), b"x");
        touch(&roms.join("PS4/CUSA00001/sce_sys/param.sfo"), b"\0PSF");

        let playlists = dir.path().join("playlists");
        let found = scan(
            std::slice::from_ref(&roms),
            &ScanCache::new(),
            &playlists,
            &|_| {},
        );
        let got: Vec<(&str, &str, &str)> = found
            .entries
            .iter()
            .map(|e| (e.system.as_str(), e.title.as_str(), e.stem.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("ps4", "CUSA00001", "CUSA00001"),
                ("ps3", "Demon's Souls", "Demon's Souls"),
                ("ps3", "Flower", "Flower (USA)"),
                ("ps3", "Nested Game", "Nested Game"),
                ("ps3", "PS3 GAME", "PS3_GAME"),
                ("ps4", "Sample Quest", "Sample Quest"),
            ]
        );
        let flower = &found.entries[2];
        assert_eq!(flower.tags, ["USA"]);
        let nested = &found.entries[3];
        assert!(nested.path.ends_with("BCES00001/PS3_GAME/USRDIR/EBOOT.BIN"));

        // A game already in the library takes its title on the next scan,
        // under the same id.
        let before = found.entries[0].app_name.clone();
        touch(
            &roms.join("PS4/CUSA00001/sce_sys/param.sfo"),
            &sfo::build(&[("TITLE", "Fixed Title")]),
        );
        let again = scan(&[roms], &found.cache, &playlists, &|_| {});
        let fixed = again.entries.iter().find(|e| e.app_name == before).unwrap();
        assert_eq!(fixed.title, "Fixed Title");
    }

    #[test]
    fn finds_playstation_3_disc_images_and_dumps() {
        use irori_engines::{iso9660, sfo};
        let dir = tempfile::tempdir().unwrap();
        let roms = dir.path().join("roms");
        let sfo_of = |title: &str| sfo::build(&[("TITLE", title), ("CATEGORY", "DG")]);
        let ps3_iso = |title: &str| {
            iso9660::build(&[
                ("PS3_DISC.SFB", b".SFB"),
                ("PS3_GAME/PARAM.SFO", &sfo_of(title)),
                ("PS3_GAME/USRDIR/EBOOT.BIN", b"SCE"),
            ])
        };
        // Images in the system's folder, one folder deeper, and among other
        // systems' games, all named by the title file inside.
        touch(&roms.join("PS3/BLUS30443.iso"), &ps3_iso("Demon's Souls"));
        touch(&roms.join("PS3/Discs/Flower (USA).iso"), &ps3_iso("Flower"));
        touch(&roms.join("Mixed/BCES00001.iso"), &ps3_iso("Mixed Disc"));
        // A PSP disc among them is a PSP game; a PS2 disc has no marker.
        touch(
            &roms.join("Mixed/Patapon.iso"),
            &iso9660::build(&[("UMD_DATA.BIN", b"x"), ("PSP_GAME/PARAM.SFO", b"x")]),
        );
        touch(
            &roms.join("Mixed/ps2.iso"),
            &iso9660::build(&[("SYSTEM.CNF", b"BOOT2 = cdrom0:SLUS_200.62;1")]),
        );
        // An extracted disc outside a PS3 folder, known by its marker file.
        touch(&roms.join("Games/BLES00001/PS3_DISC.SFB"), b".SFB");
        touch(
            &roms.join("Games/BLES00001/PS3_GAME/USRDIR/EBOOT.BIN"),
            b"x",
        );
        touch(
            &roms.join("Games/BLES00001/PS3_GAME/PARAM.SFO"),
            &sfo_of("Extracted Disc"),
        );
        // RPCS3's own folder: an installed game and a patch for a disc game.
        touch(&roms.join("dev_hdd0/game/NPUB30001/USRDIR/EBOOT.BIN"), b"x");
        touch(
            &roms.join("dev_hdd0/game/NPUB30001/PARAM.SFO"),
            &sfo::build(&[("TITLE", "Installed Game"), ("CATEGORY", "HG")]),
        );
        touch(&roms.join("dev_hdd0/game/BLUS30443/USRDIR/EBOOT.BIN"), b"x");
        touch(
            &roms.join("dev_hdd0/game/BLUS30443/PARAM.SFO"),
            &sfo::build(&[("TITLE", "Demon's Souls"), ("CATEGORY", "GD")]),
        );

        let playlists = dir.path().join("playlists");
        // A cache an older Irori wrote, which took the image for nothing.
        let mut old = ScanCache::new();
        let mixed = roms.join("Mixed/BCES00001.iso");
        let meta = std::fs::metadata(&mixed).unwrap();
        old.insert(
            mixed.clone(),
            Seen {
                size: meta.len(),
                mtime: meta
                    .modified()
                    .unwrap()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                system: None,
                rules: 0,
            },
        );
        let found = scan(std::slice::from_ref(&roms), &old, &playlists, &|_| {});
        let got: Vec<(&str, &str, PathBuf)> = found
            .entries
            .iter()
            .map(|e| {
                let path = e.path.strip_prefix(&roms).unwrap().to_path_buf();
                (e.system.as_str(), e.title.as_str(), path)
            })
            .collect();
        let want: Vec<(&str, &str, PathBuf)> = [
            ("ps3", "Demon's Souls", "PS3/BLUS30443.iso"),
            (
                "ps3",
                "Extracted Disc",
                "Games/BLES00001/PS3_GAME/USRDIR/EBOOT.BIN",
            ),
            ("ps3", "Flower", "PS3/Discs/Flower (USA).iso"),
            (
                "ps3",
                "Installed Game",
                "dev_hdd0/game/NPUB30001/USRDIR/EBOOT.BIN",
            ),
            ("ps3", "Mixed Disc", "Mixed/BCES00001.iso"),
            ("psp", "Patapon", "Mixed/Patapon.iso"),
        ]
        .iter()
        .map(|(s, t, p)| (*s, *t, p.split('/').collect::<PathBuf>()))
        .collect();
        assert_eq!(got, want);
        assert_eq!(found.entries[2].stem, "Flower (USA)");
        assert_eq!(found.entries[2].tags, ["USA"]);
        assert_eq!(found.unrecognized, 1);
        assert_eq!(
            found.cache[&mixed].system.as_deref(),
            Some("ps3"),
            "an old detection is redone"
        );
    }

    #[test]
    fn layout_folders_never_name_games() {
        let ps3 = systems::get("ps3").unwrap();
        let top = Path::new("/roms/PS3");
        let at = |p: &str| naming_folder(ps3, Path::new(p), top).to_path_buf();
        assert_eq!(at("/roms/PS3/BLUS30443"), Path::new("/roms/PS3/BLUS30443"));
        assert_eq!(
            at("/roms/PS3/Folklore/PS3_GAME"),
            Path::new("/roms/PS3/Folklore")
        );
        assert_eq!(
            at("/roms/PS3/Folklore/ps3_game/USRDIR"),
            Path::new("/roms/PS3/Folklore")
        );
        // The system's folder (or the ROM folder) is never a game's name.
        assert_eq!(at("/roms/PS3/PS3_GAME"), Path::new("/roms/PS3/PS3_GAME"));
    }

    #[test]
    fn picks_dos_programs() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("Doom");
        touch(&game.join("INSTALL.EXE"), b"MZ");
        touch(&game.join("DOOM.EXE"), b"MZ");
        touch(&game.join("GO.BAT"), b"x");
        assert!(dos_program(&game).unwrap().ends_with("DOOM.EXE"));
        let conf = dir.path().join("Quake");
        touch(&conf.join("QUAKE.EXE"), b"MZ");
        touch(&conf.join("dosbox.conf"), b"[autoexec]");
        assert!(dos_program(&conf).unwrap().ends_with("dosbox.conf"));
        let nested = dir.path().join("Duke");
        touch(&nested.join("DUKE3D/DUKE3D.EXE"), b"MZ");
        assert!(dos_program(&nested).unwrap().ends_with("DUKE3D/DUKE3D.EXE"));
    }
}
