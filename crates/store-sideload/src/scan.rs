//! "Add a folder": the games inside a folder the user picked.
//!
//! Each subfolder is one game, run by its most likely program (named like
//! the folder, near the top, the largest); programs, desktop entries
//! (`.desktop`), Windows shortcuts (`.lnk`) and app bundles (`.app`) right
//! in the folder are a game each. Installers, uninstallers, redistributables,
//! crash handlers and helper processes are never offered, and folders of
//! runtimes and prerequisites are not searched. Only reads.

use std::path::{Path, PathBuf};

use crate::app::SideloadPlatform;

/// Other programs offered per game folder.
const MAX_OTHERS: usize = 12;
/// How deep a game's folder is searched for its program.
const MAX_DEPTH: usize = 4;
/// Entries read per folder before giving up on it (a folder of assets).
const MAX_ENTRIES: usize = 4000;

/// What a found game came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoundKind {
    /// A program in a game's folder, or right in the picked folder.
    Program,
    /// A desktop entry (`.desktop`).
    DesktopEntry,
    /// A Windows shortcut (`.lnk`).
    Shortcut,
    /// A macOS app bundle.
    AppBundle,
}

/// One game found in the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundGame {
    pub title: String,
    pub executable: PathBuf,
    pub platform: SideloadPlatform,
    /// Arguments from a desktop entry or shortcut (shell-style quoting).
    pub args: String,
    /// The game's folder, when it has one of its own.
    pub install_folder: Option<PathBuf>,
    /// A local icon or picture named by a desktop entry.
    pub icon: Option<PathBuf>,
    pub kind: FoundKind,
    /// The other programs of the game's folder, most likely first, for the
    /// user to start the game with instead (a launcher, a 32-bit build).
    pub others: Vec<(PathBuf, SideloadPlatform)>,
}

/// The games in `root`, sorted by title. Reads the disk: call it off the
/// async runtime.
pub fn scan_folder(root: &Path) -> Vec<FoundGame> {
    let mut found = Vec::new();
    let mut loose = Vec::new();
    for entry in read_dir(root) {
        let path = entry.path();
        let name = file_name(&path);
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let lower = name.to_lowercase();
        if kind.is_dir() && lower.ends_with(".app") && cfg!(target_os = "macos") {
            if !is_ignored_program(&lower) {
                found.push(app_bundle(&path));
            }
        } else if kind.is_dir() {
            if !is_ignored_dir(&lower)
                && let Some(game) = game_in_folder(&path)
            {
                found.push(game);
            }
        } else if lower.ends_with(".desktop") {
            found.extend(desktop_entry(&path));
        } else if lower.ends_with(".lnk") {
            found.extend(shortcut(&path));
        } else if let Some(platform) = program_platform(&path)
            && !is_ignored_program(&lower)
        {
            loose.push((path, platform));
        }
    }
    // One program right in a folder of no games is that folder's game.
    if found.is_empty()
        && let [(path, platform)] = loose.as_slice()
    {
        found.push(FoundGame {
            title: pretty_title(&file_name(root)),
            executable: path.clone(),
            platform: *platform,
            args: String::new(),
            install_folder: Some(root.to_path_buf()),
            icon: None,
            kind: FoundKind::Program,
            others: Vec::new(),
        });
    } else {
        found.extend(loose.into_iter().map(|(path, platform)| FoundGame {
            title: pretty_title(&stem(&path)),
            executable: path,
            platform,
            args: String::new(),
            install_folder: None,
            icon: None,
            kind: FoundKind::Program,
            others: Vec::new(),
        }));
    }
    // A program named by a desktop entry or shortcut is that entry's game,
    // with its name and arguments.
    found.sort_by_key(|g| g.kind == FoundKind::Program);
    let mut seen = std::collections::HashSet::new();
    found.retain(|g| seen.insert(g.executable.clone()));
    found.sort_by_key(|g| g.title.to_lowercase());
    found
}

/// The game one file stands for, when it is a program, a desktop entry, a
/// Windows shortcut or an app bundle (a file dropped on the library). Other
/// files (ROMs, documents) are `None`. Reads the disk: call it off the
/// async runtime.
pub fn game_of_file(path: &Path) -> Option<FoundGame> {
    let lower = file_name(path).to_lowercase();
    if lower.ends_with(".app") && cfg!(target_os = "macos") && path.is_dir() {
        return Some(app_bundle(path));
    }
    if !path.is_file() {
        return None;
    }
    if lower.ends_with(".desktop") {
        return desktop_entry(path);
    }
    if lower.ends_with(".lnk") {
        return shortcut(path);
    }
    // Batch files start a game too when the user picks one, and on Windows
    // so do the scripts it runs through an interpreter.
    let script = |exts: &[&str]| exts.iter().any(|e| lower.ends_with(e));
    let windows_script = script(&[".bat", ".cmd"]) || (cfg!(windows) && script(&[".ps1", ".vbs"]));
    let platform =
        program_platform(path).or_else(|| windows_script.then_some(SideloadPlatform::Windows))?;
    Some(FoundGame {
        title: pretty_title(&stem(path)),
        executable: path.to_path_buf(),
        platform,
        args: String::new(),
        install_folder: None,
        icon: None,
        kind: FoundKind::Program,
        others: Vec::new(),
    })
}

/// Structural child folder names inside a game root that must never be
/// mistaken for a single-child archive wrapper directory.
const NON_WRAPPER_DIRS: &[&str] = &[
    "bin", "binaries", "win64", "win32", "x64", "x86", "x86_64", "linux", "linux64", "macos",
    "osx", "data",
];

fn is_archive_metadata_entry(lower_name: &str) -> bool {
    matches!(lower_name, "__macosx" | ".ds_store" | "thumbs.db")
}

/// Unwrap nested single-child wrapper folders (e.g. `<dest>/<GameFolder>/`)
/// produced by archives that package their files under a top-level directory.
fn unwrap_single_child_dir(root: &Path) -> PathBuf {
    let mut dir = root.to_path_buf();
    for _ in 0..4 {
        let mut only_subdir: Option<PathBuf> = None;
        let mut count = 0usize;
        for entry in read_dir(&dir) {
            let path = entry.path();
            let lower = file_name(&path).to_lowercase();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() || is_archive_metadata_entry(&lower) {
                continue;
            }
            count += 1;
            if count > 1 {
                break;
            }
            let is_app_bundle = cfg!(target_os = "macos") && lower.ends_with(".app");
            if kind.is_dir()
                && !is_app_bundle
                && !is_ignored_dir(&lower)
                && !NON_WRAPPER_DIRS.contains(&lower.as_str())
            {
                only_subdir = Some(path);
            }
        }
        if count == 1
            && let Some(next) = only_subdir
        {
            dir = next;
            continue;
        }
        break;
    }
    dir
}

/// Scan an extracted download directory (or a single downloaded executable)
/// and return the ONE primary game it contains, if any.
///
/// Unlike [`scan_folder`], which treats each subfolder as a separate game,
/// this unwraps single-child archive wrapper directories (`<dest>/<Game>/`)
/// and delegates to `game_in_folder` so multi-executable engines (such as
/// Unreal Engine shipping binaries next to a root stub) resolve to one
/// [`FoundGame`] with alternates listed in [`FoundGame::others`].
pub fn scan_extracted_game(root: &Path) -> Option<FoundGame> {
    if !root.exists() {
        return None;
    }
    if root.is_file()
        || (cfg!(target_os = "macos")
            && root.is_dir()
            && file_name(root).to_lowercase().ends_with(".app"))
    {
        let lower = file_name(root).to_lowercase();
        if is_ignored_program(&lower) {
            return None;
        }
        return game_of_file(root);
    }
    if !root.is_dir() {
        return None;
    }
    let dir = unwrap_single_child_dir(root);
    if let Some(game) = game_in_folder(&dir) {
        return Some(game);
    }
    let mut files = Vec::new();
    for entry in read_dir(&dir) {
        let path = entry.path();
        let lower = file_name(&path).to_lowercase();
        if is_archive_metadata_entry(&lower) {
            continue;
        }
        if !is_ignored_program(&lower) {
            files.push(path);
        }
    }
    let [only] = files.as_slice() else {
        return None;
    };
    game_of_file(only)
}

fn read_dir(dir: &Path) -> impl Iterator<Item = std::fs::DirEntry> + use<> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .take(MAX_ENTRIES)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The platform a file's program is for, when it is one.
fn program_platform(path: &Path) -> Option<SideloadPlatform> {
    let lower = file_name(path).to_lowercase();
    if lower.ends_with(".exe") {
        return Some(SideloadPlatform::Windows);
    }
    if cfg!(windows) {
        return None;
    }
    let native = if cfg!(target_os = "macos") {
        SideloadPlatform::Mac
    } else {
        SideloadPlatform::Linux
    };
    let known_ext = [".sh", ".x86_64", ".x86", ".appimage"]
        .iter()
        .any(|e| lower.ends_with(e));
    // Executable files without an extension are native programs; libraries,
    // scripts of other tools and data are not.
    let bare = !lower.contains('.')
        && path.is_file()
        && irori_core::util::is_executable(path)
        && is_binary(path);
    (known_ext || bare).then_some(native)
}

/// Whether a file starts like a native program (ELF or Mach-O), not a text
/// file marked executable.
fn is_binary(path: &Path) -> bool {
    use std::io::Read as _;
    let mut magic = [0u8; 4];
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    if file.read_exact(&mut magic).is_err() {
        return false;
    }
    magic == *b"\x7fELF"
        || matches!(
            u32::from_be_bytes(magic),
            0xfeedface | 0xfeedfacf | 0xcefaedfe | 0xcffaedfe | 0xcafebabe
        )
}

/// Programs that are never the game: installers and uninstallers,
/// redistributables, crash handlers and helper processes.
fn is_ignored_program(lower_name: &str) -> bool {
    const PARTS: &[&str] = &[
        "unins",
        "setup",
        "install",
        "redist",
        "vcredist",
        "vc_redist",
        "directx",
        "dxwebsetup",
        "dotnet",
        "ndp4",
        "crash",
        "bugreport",
        "errorreport",
        "prereq",
        "physx",
        "oalinst",
        "easyanticheat",
        "beservice",
        "battleye",
        "helper",
        "webhelper",
        "subprocess",
        "browsersubprocess",
        "qtwebengine",
        "cefprocess",
        "awesomium",
        "notification",
        "updater",
        "patcher",
        "dxsetup",
        "quicksfv",
    ];
    const WHOLE: &[&str] = &[
        "python", "pythonw", "java", "javaw", "7z", "unrar", "unzip", "zsync", "dosbox", "scummvm",
        "wine", "sfv", "md5sum",
    ];
    let stem = lower_name
        .rsplit_once('.')
        .map_or(lower_name, |(s, _)| s)
        .trim_end_matches("64")
        .trim_end_matches("32")
        .trim_end_matches(['_', '-', ' ']);
    PARTS.iter().any(|p| lower_name.contains(p)) || WHOLE.contains(&stem)
}

/// Folders of runtimes, prerequisites and engine tools, never searched.
fn is_ignored_dir(lower_name: &str) -> bool {
    const DIRS: &[&str] = &[
        "_commonredist",
        "commonredist",
        "redist",
        "redistributables",
        "__installer",
        "installer",
        "installers",
        "directx",
        "dotnet",
        "vcredist",
        "support",
        "prerequisites",
        "_redist",
        "easyanticheat",
        "battleye",
        "monobleedingedge",
        "jre",
        "jdk",
        "java",
        "python",
        "crashreporter",
        "crashpad",
        "thirdparty",
        "engine",
        "__macosx",
        "dosbox",
        "scummvm",
        "manual",
        "docs",
        "soundtrack",
        "ost",
        "extras",
        "mods",
        "saves",
        "screenshots",
    ];
    lower_name.starts_with('.') || DIRS.contains(&lower_name)
}

/// The most likely program of the game in `dir`.
fn game_in_folder(dir: &Path) -> Option<FoundGame> {
    let folder = file_name(dir);
    let mut candidates = Vec::new();
    collect_programs(dir, 0, &mut candidates);
    let wanted = squash(&folder);
    let mut ranked: Vec<_> = candidates
        .into_iter()
        .map(|(path, platform, depth)| {
            let name = squash(&stem(&path));
            let similar = if name == wanted {
                3
            } else if !name.is_empty() && (wanted.contains(&name) || name.contains(&wanted)) {
                2
            } else if wanted.chars().take(3).eq(name.chars().take(3)) && name.len() >= 3 {
                1
            } else {
                0
            };
            // A launcher or settings tool is a fallback, never the first pick.
            let tool = ["launcher", "config", "settings", "editor", "server"]
                .iter()
                .any(|t| name.contains(t) && !wanted.contains(t));
            // 64-bit builds before 32-bit ones; native builds where both exist.
            let bits = path.to_string_lossy().to_lowercase().contains("64");
            let native = platform != SideloadPlatform::Windows;
            let size = std::fs::metadata(&path).map_or(0, |m| m.len()) / (1 << 20);
            let rank = (!tool, similar, std::cmp::Reverse(depth), native, bits, size);
            (rank, path, platform)
        })
        .collect();
    // Best first. Of equal ranks the one read last leads, as it always has.
    ranked.reverse();
    ranked.sort_by_key(|a| std::cmp::Reverse(a.0));
    let mut ranked = ranked
        .into_iter()
        .map(|(_, path, platform)| (path, platform));
    let (path, platform) = ranked.next()?;
    let others = ranked.take(MAX_OTHERS).collect();
    Some(FoundGame {
        title: pretty_title(&folder),
        executable: path,
        platform,
        args: String::new(),
        install_folder: Some(dir.to_path_buf()),
        icon: None,
        kind: FoundKind::Program,
        others,
    })
}

fn collect_programs(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, SideloadPlatform, usize)>) {
    if depth > MAX_DEPTH {
        return;
    }
    for entry in read_dir(dir) {
        let path = entry.path();
        let lower = file_name(&path).to_lowercase();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if lower.ends_with(".app") && cfg!(target_os = "macos") {
                if !is_ignored_program(&lower) {
                    out.push((path, SideloadPlatform::Mac, depth));
                }
            } else if !is_ignored_dir(&lower) {
                collect_programs(&path, depth + 1, out);
            }
        } else if let Some(platform) = program_platform(&path)
            && !is_ignored_program(&lower)
        {
            out.push((path, platform, depth));
        }
    }
}

fn app_bundle(path: &Path) -> FoundGame {
    FoundGame {
        title: pretty_title(&stem(path)),
        executable: path.to_path_buf(),
        platform: SideloadPlatform::Mac,
        args: String::new(),
        install_folder: None,
        icon: None,
        kind: FoundKind::AppBundle,
        others: Vec::new(),
    }
}

/// A desktop entry's game: its name, program and arguments. Entries that
/// start another launcher's game (Steam, Lutris, Heroic, itch) are left to
/// those launchers' own imports.
fn desktop_entry(path: &Path) -> Option<FoundGame> {
    let text = std::fs::read_to_string(path).ok()?;
    let entry = parse_desktop_entry(&text)?;
    let words = irori_core::process::split_args(&entry.exec);
    // `env VAR=value program …` keeps the program.
    let mut words = words
        .into_iter()
        .skip_while(|w| w == "env" || (w.contains('=') && !w.starts_with('-')))
        .filter(|w| !matches!(w.as_str(), "%f" | "%F" | "%u" | "%U" | "%i" | "%c" | "%k"));
    let program = words.next()?;
    let args: Vec<String> = words.collect();
    let launcher_link = ["steam://", "lutris:", "heroic://", "itch://", "irori://"];
    if launcher_link
        .iter()
        .any(|l| args.iter().any(|a| a.starts_with(l)) || program.starts_with(l))
    {
        return None;
    }
    let executable = if Path::new(&program).is_absolute() {
        PathBuf::from(&program)
    } else {
        which::which(&program).ok()?
    };
    if !executable.exists() {
        return None;
    }
    let platform = if program_platform(&executable) == Some(SideloadPlatform::Windows) {
        SideloadPlatform::Windows
    } else if cfg!(target_os = "macos") {
        SideloadPlatform::Mac
    } else {
        SideloadPlatform::Linux
    };
    Some(FoundGame {
        title: entry.name,
        executable,
        platform,
        args: args
            .iter()
            .map(|a| irori_core::process::shell_quote(a))
            .collect::<Vec<_>>()
            .join(" "),
        install_folder: entry.path.map(PathBuf::from),
        icon: entry
            .icon
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && p.is_file()),
        kind: FoundKind::DesktopEntry,
        others: Vec::new(),
    })
}

/// The fields of a desktop entry Irori uses.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DesktopEntry {
    name: String,
    exec: String,
    path: Option<String>,
    icon: Option<String>,
}

/// The `[Desktop Entry]` group of an application entry that is shown;
/// `None` for links, hidden entries and entries without a program.
fn parse_desktop_entry(text: &str) -> Option<DesktopEntry> {
    let mut in_group = false;
    let (mut name, mut exec, mut path, mut icon) = (None, None, None, None);
    let (mut kind, mut hidden) = (None, false);
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().to_string();
        match key.trim() {
            "Type" => kind = Some(value),
            "Name" => name = Some(value),
            "Exec" => exec = Some(value),
            "Path" if !value.is_empty() => path = Some(value),
            "Icon" if !value.is_empty() => icon = Some(value),
            "NoDisplay" | "Hidden" if value.eq_ignore_ascii_case("true") => hidden = true,
            _ => {}
        }
    }
    if hidden || kind.as_deref() != Some("Application") {
        return None;
    }
    Some(DesktopEntry {
        name: name.filter(|n| !n.is_empty())?,
        exec: exec.filter(|e| !e.is_empty())?,
        path,
        icon,
    })
}

/// A Windows shortcut's game. In a Wine prefix (a Start menu folder), the
/// target's drive is the prefix's.
fn shortcut(path: &Path) -> Option<FoundGame> {
    let bytes = std::fs::read(path).ok()?;
    let link = parse_lnk(&bytes)?;
    let executable = windows_path(&link.target, path)?;
    let lower = file_name(&executable).to_lowercase();
    if !lower.ends_with(".exe") || is_ignored_program(&lower) || !executable.is_file() {
        return None;
    }
    let title = stem(path);
    if is_ignored_program(&title.to_lowercase()) {
        return None;
    }
    Some(FoundGame {
        title,
        platform: SideloadPlatform::Windows,
        args: link.arguments.unwrap_or_default(),
        install_folder: link
            .working_dir
            .and_then(|d| windows_path(&d, path))
            .filter(|d| d.is_dir()),
        executable,
        icon: None,
        kind: FoundKind::Shortcut,
        others: Vec::new(),
    })
}

/// A Windows path as a path on this computer: unchanged on Windows; inside
/// the Wine prefix holding `near` elsewhere.
fn windows_path(target: &str, near: &Path) -> Option<PathBuf> {
    if cfg!(windows) {
        return Some(PathBuf::from(target));
    }
    let normalized = target.replace('/', "\\");
    let (drive, rest) = normalized.split_once(":\\")?;
    let drive = drive.to_lowercase();
    let prefix = near
        .ancestors()
        .find(|a| a.join("drive_c").is_dir() && a.join("dosdevices").is_dir())?;
    let root = if drive == "c" {
        prefix.join("drive_c")
    } else {
        let dev = prefix.join("dosdevices").join(format!("{drive}:"));
        std::fs::canonicalize(&dev).unwrap_or(dev)
    };
    let mut current = root;
    for part in rest.split('\\').filter(|s| !s.is_empty()) {
        let direct = current.join(part);
        if direct.exists() {
            current = direct;
        } else if let Some(matched) = read_dir(&current)
            .map(|e| e.path())
            .find(|p| file_name(p).eq_ignore_ascii_case(part))
        {
            current = matched;
        } else {
            current = direct;
        }
    }
    Some(current)
}

/// What Irori reads from a shortcut.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct LnkTarget {
    target: String,
    arguments: Option<String>,
    working_dir: Option<String>,
}

/// The target, arguments and working folder of a Shell Link (`.lnk`, see
/// MS-SHLLINK): the local base path and common path suffix of its link info,
/// then its string data.
fn parse_lnk(b: &[u8]) -> Option<LnkTarget> {
    const HAS_ID_LIST: u32 = 0x1;
    const HAS_LINK_INFO: u32 = 0x2;
    const HAS_NAME: u32 = 0x4;
    const HAS_RELATIVE_PATH: u32 = 0x8;
    const HAS_WORKING_DIR: u32 = 0x10;
    const HAS_ARGUMENTS: u32 = 0x20;
    const IS_UNICODE: u32 = 0x80;
    let u16_at = |at: usize| Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?));
    let c_string = |at: usize| -> Option<String> {
        let tail = b.get(at..)?;
        let end = tail.iter().position(|c| *c == 0)?;
        Some(tail[..end].iter().map(|c| char::from(*c)).collect())
    };
    let wide_string = |at: usize| -> Option<String> {
        let mut units = Vec::new();
        let mut i = at;
        loop {
            let unit = u16_at(i)?;
            if unit == 0 {
                break;
            }
            units.push(unit);
            i += 2;
        }
        Some(String::from_utf16_lossy(&units))
    };
    if u32_at(0)? != 0x4c {
        return None;
    }
    let flags = u32_at(0x14)?;
    let mut at = 0x4c;
    if flags & HAS_ID_LIST != 0 {
        at += 2 + usize::from(u16_at(at)?);
    }
    let mut target = None;
    if flags & HAS_LINK_INFO != 0 {
        let info = at;
        let size = usize::try_from(u32_at(info)?).ok()?;
        let header = u32_at(info + 4)?;
        let info_flags = u32_at(info + 8)?;
        let suffix = if header >= 0x24 {
            let offset = usize::try_from(u32_at(info + 0x20)?).ok()?;
            wide_string(info + offset)
        } else {
            let offset = usize::try_from(u32_at(info + 0x18)?).ok()?;
            c_string(info + offset)
        }
        .unwrap_or_default();
        if info_flags & 0x1 != 0 {
            let base = if header >= 0x24 {
                let offset = usize::try_from(u32_at(info + 0x1c)?).ok()?;
                wide_string(info + offset)
            } else {
                let offset = usize::try_from(u32_at(info + 0x10)?).ok()?;
                c_string(info + offset)
            }?;
            target = Some(format!("{base}{suffix}"));
        }
        at = info + size;
    }
    let unicode = flags & IS_UNICODE != 0;
    let mut read_string = |present: bool| -> Option<Option<String>> {
        if !present {
            return Some(None);
        }
        let count = usize::from(u16_at(at)?);
        at += 2;
        let text = if unicode {
            let raw = b.get(at..at + count * 2)?;
            at += count * 2;
            let units: Vec<u16> = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            let raw = b.get(at..at + count)?;
            at += count;
            raw.iter().map(|c| char::from(*c)).collect()
        };
        Some(Some(text.trim_matches('\0').trim().to_string()))
    };
    let _name = read_string(flags & HAS_NAME != 0)?;
    let relative = read_string(flags & HAS_RELATIVE_PATH != 0)?;
    let working_dir = read_string(flags & HAS_WORKING_DIR != 0)?;
    let arguments = read_string(flags & HAS_ARGUMENTS != 0)?;
    let target = target.or(relative)?;
    Some(LnkTarget {
        target,
        arguments: arguments.filter(|a| !a.is_empty()),
        working_dir: working_dir.filter(|d| !d.is_empty()),
    })
}

/// Letters and digits only, lower case, for comparing names.
fn squash(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A folder or file name as a title: separators become spaces, and
/// "HollowKnight" or "hollow_knight" reads "Hollow Knight".
pub fn pretty_title(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    let mut prev: Option<char> = None;
    for c in name.chars() {
        let c = if matches!(c, '_' | '.') { ' ' } else { c };
        if let Some(p) = prev
            && p.is_lowercase()
            && c.is_uppercase()
        {
            out.push(' ');
        }
        out.push(c);
        prev = Some(c);
    }
    let words: Vec<String> = out
        .split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) if w.chars().all(|c| !c.is_uppercase()) => {
                    first.to_uppercase().chain(chars).collect()
                }
                _ => w.to_string(),
            }
        })
        .collect();
    words.join(" ")
}

/// One executable candidate found after running an installer in a prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledCandidate {
    pub executable: PathBuf,
    pub args: String,
    pub install_folder: Option<PathBuf>,
    pub label: Option<String>,
    pub from_shortcut: bool,
}

/// Snapshot of executables, shortcuts and registry install locations present
/// in a prefix before running an installer.
#[derive(Debug, Default, Clone)]
pub struct PrefixSnapshot {
    shortcuts: std::collections::HashSet<PathBuf>,
    executables: std::collections::HashSet<PathBuf>,
    reg_dirs: std::collections::HashSet<PathBuf>,
}

/// Result of scanning a prefix after an installer finishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectionOutcome {
    /// No game executable was found.
    None,
    /// Exactly one game executable was found.
    Picked(InstalledCandidate),
    /// Multiple plausible game executables were found, ordered best-first.
    Ambiguous(Vec<InstalledCandidate>),
}

/// How deep inside a game container folder (`Games`, `Program Files`, …)
/// post-install scanning looks for `.exe` files.
const INSTALL_SCAN_DEPTH: usize = 6;

impl PrefixSnapshot {
    /// Record the shortcuts, executables and registry install directories
    /// currently in `prefix` (and `extra_dir` when given). Reads the disk:
    /// call it off the async runtime.
    pub fn capture(prefix: &Path, extra_dir: Option<&Path>) -> Self {
        let mut shortcuts = std::collections::HashSet::new();
        let mut executables = std::collections::HashSet::new();
        let mut reg_dirs = std::collections::HashSet::new();
        if let Some(root) = wine_root(prefix) {
            for lnk in prefix_shortcut_files(&root) {
                shortcuts.insert(canonical_or_self(&lnk));
                if let Some(found) = shortcut(&lnk) {
                    executables.insert(canonical_or_self(&found.executable));
                }
            }
            for dir in reg_install_dirs(&root) {
                let canon = canonical_or_self(&dir);
                if reg_dirs.insert(canon) {
                    let mut exes = Vec::new();
                    collect_windows_exes(&dir, &dir, 0, &mut exes);
                    for (exe, _, _) in exes {
                        executables.insert(canonical_or_self(&exe));
                    }
                }
            }
            for (exe, _, _) in drive_c_executables(&root.join("drive_c")) {
                executables.insert(canonical_or_self(&exe));
            }
        }
        if let Some(dir) = extra_dir.filter(|d| d.is_dir()) {
            let mut exes = Vec::new();
            collect_windows_exes(dir, dir, 0, &mut exes);
            for (exe, _, _) in exes {
                executables.insert(canonical_or_self(&exe));
            }
        }
        Self {
            shortcuts,
            executables,
            reg_dirs,
        }
    }
}

/// Find the game executable(s) installed into `prefix` (or referenced by its
/// shortcuts / registry / `extra_dir`) compared against `before`.
///
/// Works even when the user unchecked Desktop and Start Menu shortcuts in
/// the installer by walking `drive_c` directly and filtering out Wine's own
/// system files, uninstallers, redistributables and crash handlers.
pub fn detect_installed_executables(
    prefix: &Path,
    title: &str,
    installer: &Path,
    extra_dir: Option<&Path>,
    before: &PrefixSnapshot,
) -> DetectionOutcome {
    let mut candidates = collect_installed_candidates(prefix, installer, extra_dir, before);
    // If the user re-ran an installer in a prefix where the game was already
    // installed before the snapshot, fall back to scanning without the
    // snapshot filter so the installed game is still found.
    if candidates.is_empty() && (!before.executables.is_empty() || !before.shortcuts.is_empty()) {
        candidates =
            collect_installed_candidates(prefix, installer, extra_dir, &PrefixSnapshot::default());
    }
    rank_and_decide(candidates, title)
}

#[derive(Debug, Clone)]
struct RawCandidate {
    candidate: InstalledCandidate,
    depth: usize,
}

fn collect_installed_candidates(
    prefix: &Path,
    installer: &Path,
    extra_dir: Option<&Path>,
    before: &PrefixSnapshot,
) -> Vec<RawCandidate> {
    let installer_canon = canonical_or_self(installer);
    let mut by_exe: Vec<(PathBuf, RawCandidate)> = Vec::new();
    let mut upsert = |raw: RawCandidate| {
        let key = canonical_or_self(&raw.candidate.executable);
        if key == installer_canon || before.executables.contains(&key) {
            return;
        }
        if let Some((_, existing)) = by_exe.iter_mut().find(|(k, _)| *k == key) {
            if raw.candidate.from_shortcut && !existing.candidate.from_shortcut {
                existing.candidate.from_shortcut = true;
                if !raw.candidate.args.is_empty() {
                    existing.candidate.args = raw.candidate.args;
                }
                if raw.candidate.label.is_some() {
                    existing.candidate.label = raw.candidate.label;
                }
                if raw.candidate.install_folder.is_some() {
                    existing.candidate.install_folder = raw.candidate.install_folder;
                }
            }
            existing.depth = existing.depth.min(raw.depth);
        } else {
            by_exe.push((key, raw));
        }
    };

    if let Some(root) = wine_root(prefix) {
        let drive_c = root.join("drive_c");
        for (exe, install_folder, depth) in drive_c_executables(&drive_c) {
            upsert(RawCandidate {
                candidate: InstalledCandidate {
                    executable: exe,
                    args: String::new(),
                    install_folder: Some(install_folder),
                    label: None,
                    from_shortcut: false,
                },
                depth,
            });
        }
        for dir in reg_install_dirs(&root) {
            let canon = canonical_or_self(&dir);
            if before.reg_dirs.contains(&canon) {
                continue;
            }
            let mut exes = Vec::new();
            collect_windows_exes(&dir, &dir, 0, &mut exes);
            for (exe, folder, depth) in exes {
                upsert(RawCandidate {
                    candidate: InstalledCandidate {
                        executable: exe,
                        args: String::new(),
                        install_folder: Some(folder),
                        label: None,
                        from_shortcut: false,
                    },
                    depth,
                });
            }
        }
        for lnk in prefix_shortcut_files(&root) {
            let lnk_canon = canonical_or_self(&lnk);
            if before.shortcuts.contains(&lnk_canon) {
                continue;
            }
            if let Some(found) = shortcut(&lnk) {
                let folder = found
                    .install_folder
                    .or_else(|| found.executable.parent().map(Path::to_path_buf));
                let depth = folder
                    .as_deref()
                    .and_then(|f| found.executable.strip_prefix(f).ok())
                    .map_or(0, |rel| rel.components().count().saturating_sub(1));
                upsert(RawCandidate {
                    candidate: InstalledCandidate {
                        executable: found.executable,
                        args: found.args,
                        install_folder: folder,
                        label: Some(found.title).filter(|t| !t.is_empty()),
                        from_shortcut: true,
                    },
                    depth,
                });
            }
        }
    }

    if let Some(dir) = extra_dir.filter(|d| d.is_dir()) {
        let mut exes = Vec::new();
        collect_windows_exes(dir, dir, 0, &mut exes);
        for (exe, folder, depth) in exes {
            upsert(RawCandidate {
                candidate: InstalledCandidate {
                    executable: exe,
                    args: String::new(),
                    install_folder: Some(folder),
                    label: None,
                    from_shortcut: false,
                },
                depth,
            });
        }
    }

    by_exe.into_iter().map(|(_, raw)| raw).collect()
}

fn rank_and_decide(mut candidates: Vec<RawCandidate>, title: &str) -> DetectionOutcome {
    if candidates.is_empty() {
        return DetectionOutcome::None;
    }
    let wanted = squash(title);
    let score = |raw: &RawCandidate| {
        let c = &raw.candidate;
        let exe_stem = stem(&c.executable);
        let name = squash(&exe_stem);
        let label = c.label.as_deref().map(squash).unwrap_or_default();
        let folder_name = c
            .install_folder
            .as_deref()
            .map(|f| squash(&file_name(f)))
            .unwrap_or_default();
        let sim = |target: &str, candidate: &str| -> u8 {
            if target.is_empty() || candidate.is_empty() {
                0
            } else if candidate == target {
                3
            } else if target.contains(candidate) || candidate.contains(target) {
                2
            } else if target.len() >= 3
                && candidate.len() >= 3
                && target.chars().take(3).eq(candidate.chars().take(3))
            {
                1
            } else {
                0
            }
        };
        let title_sim = sim(&wanted, &name)
            .max(sim(&wanted, &label))
            .max(sim(&folder_name, &name));
        let tool = [
            "launcher",
            "config",
            "settings",
            "editor",
            "server",
            "benchmark",
            "handler",
            "reporter",
            "browser",
        ]
        .iter()
        .any(|t| name.contains(t) && !wanted.contains(t));
        let bits = c.executable.to_string_lossy().to_lowercase().contains("64");
        let size = std::fs::metadata(&c.executable).map_or(0, |m| m.len()) / (1 << 20);
        (
            !tool,
            title_sim,
            c.from_shortcut,
            std::cmp::Reverse(raw.depth),
            bits,
            size,
        )
    };
    // Sort highest score first; tie-break alphabetically by path for determinism.
    candidates.sort_by(|a, b| {
        score(b)
            .cmp(&score(a))
            .then_with(|| a.candidate.executable.cmp(&b.candidate.executable))
    });
    let mut out: Vec<InstalledCandidate> = candidates.into_iter().map(|r| r.candidate).collect();
    if out.len() == 1 {
        DetectionOutcome::Picked(out.remove(0))
    } else {
        DetectionOutcome::Ambiguous(out)
    }
}

fn canonical_or_self(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The folder inside `prefix` that holds `drive_c` and the registry (`prefix`
/// itself for Wine and umu where `pfx` is a symlink to `.`, or `pfx` for
/// direct Proton prefixes).
fn wine_root(prefix: &Path) -> Option<PathBuf> {
    if prefix.join("drive_c").is_dir() {
        return Some(prefix.to_path_buf());
    }
    let pfx = prefix.join("pfx");
    if pfx.join("drive_c").is_dir() {
        Some(pfx)
    } else {
        None
    }
}

/// Every `.lnk` file in the prefix's Start Menu and Desktop folders.
fn prefix_shortcut_files(wine_root: &Path) -> Vec<PathBuf> {
    let drive = wine_root.join("drive_c");
    let mut roots = vec![
        drive.join("ProgramData/Microsoft/Windows/Start Menu/Programs"),
        drive.join("users/Public/Desktop"),
    ];
    for entry in read_dir(&drive.join("users")) {
        let user = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            roots.push(user.join("Desktop"));
            roots.push(user.join("AppData/Roaming/Microsoft/Windows/Start Menu/Programs"));
            roots.push(user.join("Start Menu/Programs"));
        }
    }
    let mut lnks = Vec::new();
    for root in roots {
        collect_lnks(&root, 0, &mut lnks);
    }
    lnks
}

fn collect_lnks(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 3 || !dir.is_dir() {
        return;
    }
    for entry in read_dir(dir) {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect_lnks(&path, depth + 1, out);
        } else if kind.is_file() && file_name(&path).to_lowercase().ends_with(".lnk") {
            out.push(path);
        }
    }
}

/// Folders recorded by Inno Setup, NSIS or MSI under `Uninstall` registry
/// keys (`InstallLocation` and `Inno Setup: App Path`), resolved to host
/// paths. Captures games installed outside `drive_c` (via `Z:\`) even when
/// the user unchecked Desktop and Start Menu shortcuts.
fn reg_install_dirs(wine_root: &Path) -> Vec<PathBuf> {
    const KEYS: &[&str] = &[
        "\"InstallLocation\"=\"",
        "\"Inno Setup: App Path\"=\"",
        "\"InstallPath\"=\"",
    ];
    let mut dirs = Vec::new();
    for file in ["system.reg", "user.reg"] {
        let path = wine_root.join(file);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in text.lines().map(str::trim) {
            let Some(rest) = KEYS.iter().find_map(|k| line.strip_prefix(k)) else {
                continue;
            };
            let Some(raw) = rest.strip_suffix('"') else {
                continue;
            };
            let unescaped = raw.replace("\\\\", "\\");
            if unescaped.is_empty() {
                continue;
            }
            if let Some(resolved) = windows_path(&unescaped, &path)
                && resolved.is_dir()
                && !is_wine_system_path(&resolved, &wine_root.join("drive_c"))
            {
                dirs.push(resolved);
            }
        }
    }
    dirs
}

fn is_wine_system_path(path: &Path, drive_c: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(drive_c) else {
        return false;
    };
    let mut comps = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase());
    let Some(first) = comps.next() else {
        return true;
    };
    if matches!(
        first.as_str(),
        "windows" | "programdata" | "openxr" | "vrclient"
    ) {
        return true;
    }
    if matches!(first.as_str(), "program files" | "program files (x86)")
        && let Some(second) = comps.next()
    {
        return is_wine_builtin_program_dir(&second);
    }
    false
}

/// Built-in folders inside `Program Files` and `Program Files (x86)` that
/// `wineboot --init` creates in every prefix.
fn is_wine_builtin_program_dir(lower_name: &str) -> bool {
    matches!(
        lower_name,
        "common files"
            | "internet explorer"
            | "windows media player"
            | "windows mail"
            | "windows nt"
            | "windows defender"
            | "windows photo viewer"
            | "windows portable devices"
            | "windows sidebar"
            | "msbuild"
            | "reference assemblies"
            | "wine"
    )
}

/// Every non-system game executable inside `drive_c`, paired with its
/// inferred game root folder and depth below that folder.
fn drive_c_executables(drive_c: &Path) -> Vec<(PathBuf, PathBuf, usize)> {
    let mut out = Vec::new();
    for entry in read_dir(drive_c) {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() || kind.is_symlink() {
            continue;
        }
        let lower = file_name(&path).to_lowercase();
        if matches!(
            lower.as_str(),
            "windows" | "programdata" | "openxr" | "vrclient"
        ) || is_ignored_dir(&lower)
        {
            continue;
        }
        if matches!(lower.as_str(), "program files" | "program files (x86)") {
            for sub in read_dir(&path) {
                let sub_path = sub.path();
                let Ok(sub_kind) = sub.file_type() else {
                    continue;
                };
                if !sub_kind.is_dir() || sub_kind.is_symlink() {
                    continue;
                }
                let sub_lower = file_name(&sub_path).to_lowercase();
                if is_wine_builtin_program_dir(&sub_lower) || is_ignored_dir(&sub_lower) {
                    continue;
                }
                collect_container_game_exes(&sub_path, &mut out);
            }
        } else if matches!(lower.as_str(), "games" | "gog games") {
            for sub in read_dir(&path) {
                let sub_path = sub.path();
                let Ok(sub_kind) = sub.file_type() else {
                    continue;
                };
                if sub_kind.is_dir() && !sub_kind.is_symlink() {
                    let sub_lower = file_name(&sub_path).to_lowercase();
                    if !is_ignored_dir(&sub_lower) {
                        collect_container_game_exes(&sub_path, &mut out);
                    }
                } else if sub_kind.is_file()
                    && sub_path
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
                    && !is_ignored_program(&file_name(&sub_path).to_lowercase())
                {
                    out.push((sub_path, path.clone(), 0));
                }
            }
        } else if lower == "users" {
            collect_user_executables(&path, &mut out);
        } else {
            collect_container_game_exes(&path, &mut out);
        }
    }
    out
}

/// Collect executables inside a top-level folder under `Program Files` or
/// `Games`. When the folder is a publisher directory containing a single
/// game subfolder and no direct `.exe`s (`Program Files/Publisher/Game`),
/// the inner folder becomes the `install_folder`.
fn collect_container_game_exes(dir: &Path, out: &mut Vec<(PathBuf, PathBuf, usize)>) {
    let mut subdirs = Vec::new();
    let mut has_direct_exe = false;
    for entry in read_dir(dir) {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        let lower = file_name(&path).to_lowercase();
        if kind.is_dir() && !is_ignored_dir(&lower) {
            subdirs.push(path);
        } else if kind.is_file() && lower.ends_with(".exe") && !is_ignored_program(&lower) {
            has_direct_exe = true;
        }
    }
    let root = if !has_direct_exe && subdirs.len() == 1 {
        &subdirs[0]
    } else {
        dir
    };
    collect_windows_exes(root, root, 0, out);
}

fn collect_user_executables(users_dir: &Path, out: &mut Vec<(PathBuf, PathBuf, usize)>) {
    const IGNORED_APPDATA: &[&str] = &[
        "temp",
        "microsoft",
        "connecteddevicesplatform",
        "d3dscache",
        "fontconfig",
        "packages",
        "publishers",
        "virtualstore",
        "wine",
        "cef",
        "crashdumps",
    ];
    for user in read_dir(users_dir) {
        let user_path = user.path();
        if !user.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let programs = user_path.join("AppData/Local/Programs");
        for app in read_dir(&programs) {
            let app_path = app.path();
            if app.file_type().is_ok_and(|t| t.is_dir()) {
                let lower = file_name(&app_path).to_lowercase();
                if !is_ignored_dir(&lower) {
                    collect_windows_exes(&app_path, &app_path, 0, out);
                }
            }
        }
        let local = user_path.join("AppData/Local");
        for app in read_dir(&local) {
            let app_path = app.path();
            if app.file_type().is_ok_and(|t| t.is_dir()) {
                let lower = file_name(&app_path).to_lowercase();
                if lower != "programs"
                    && !IGNORED_APPDATA.contains(&lower.as_str())
                    && !is_ignored_dir(&lower)
                {
                    collect_windows_exes(&app_path, &app_path, 0, out);
                }
            }
        }
    }
}

fn collect_windows_exes(
    dir: &Path,
    install_root: &Path,
    depth: usize,
    out: &mut Vec<(PathBuf, PathBuf, usize)>,
) {
    if depth > INSTALL_SCAN_DEPTH {
        return;
    }
    for entry in read_dir(dir) {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        let lower = file_name(&path).to_lowercase();
        if kind.is_dir() {
            if !is_ignored_dir(&lower) {
                collect_windows_exes(&path, install_root, depth + 1, out);
            }
        } else if kind.is_file() && lower.ends_with(".exe") && !is_ignored_program(&lower) {
            out.push((path, install_root.to_path_buf(), depth));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[cfg(unix)]
    fn native(path: &Path, size: usize) {
        use std::os::unix::fs::PermissionsExt as _;
        let mut bytes = b"\x7fELF".to_vec();
        bytes.resize(size, 0);
        write(path, &bytes);
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn a_dropped_file_is_the_game_it_starts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("Hollow_Knight.exe"), &[0; 16]);
        let game = game_of_file(&root.join("Hollow_Knight.exe")).unwrap();
        assert_eq!(game.title, "Hollow Knight");
        assert_eq!(game.platform, SideloadPlatform::Windows);
        assert_eq!(game.kind, FoundKind::Program);
        write(&root.join("start.bat"), b"@echo off");
        assert_eq!(
            game_of_file(&root.join("start.bat")).unwrap().platform,
            SideloadPlatform::Windows
        );
        write(&root.join("run.ps1"), b"Write-Host");
        assert_eq!(game_of_file(&root.join("run.ps1")).is_some(), cfg!(windows));
        let program = root.join("supertux2");
        write(&program, &[0; 16]);
        write(
            &root.join("supertux.desktop"),
            format!(
                "[Desktop Entry]\nType=Application\nName=SuperTux\nExec={} --fullscreen\n",
                program.display()
            )
            .as_bytes(),
        );
        let entry = game_of_file(&root.join("supertux.desktop")).unwrap();
        assert_eq!(entry.title, "SuperTux");
        assert_eq!(entry.executable, program);
        assert_eq!(entry.args, "--fullscreen");
        assert_eq!(entry.kind, FoundKind::DesktopEntry);
        // ROMs, documents and folders are not programs.
        write(&root.join("mario.sfc"), &[0; 16]);
        assert!(game_of_file(&root.join("mario.sfc")).is_none());
        write(&root.join("readme.txt"), b"hi");
        assert!(game_of_file(&root.join("readme.txt")).is_none());
        assert!(game_of_file(root).is_none());
        assert!(game_of_file(&root.join("gone.exe")).is_none());
    }

    #[test]
    fn titles_read_like_names() {
        assert_eq!(pretty_title("HollowKnight"), "Hollow Knight");
        assert_eq!(pretty_title("hollow_knight"), "Hollow Knight");
        assert_eq!(
            pretty_title("FTL Faster Than Light"),
            "FTL Faster Than Light"
        );
        assert_eq!(pretty_title("celeste.v1.4"), "Celeste V1 4");
    }

    #[test]
    fn skips_installers_redistributables_and_helpers() {
        for name in [
            "unins000.exe",
            "setup.exe",
            "vc_redist.x64.exe",
            "UnityCrashHandler64.exe",
            "CrashReportClient.exe",
            "UE4PrereqSetup_x64.exe",
            "dxwebsetup.exe",
            "EasyAntiCheat_Setup.exe",
            "QtWebEngineProcess.exe",
            "python.exe",
        ] {
            assert!(is_ignored_program(&name.to_lowercase()), "{name}");
        }
        for name in [
            "Celeste.exe",
            "hollow_knight.exe",
            "Game.x86_64",
            "Launcher.exe",
        ] {
            assert!(!is_ignored_program(&name.to_lowercase()), "{name}");
        }
    }

    #[test]
    fn finds_one_game_per_folder_with_its_main_program() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // A Windows game with installers and a launcher next to it.
        write(&root.join("Celeste/Celeste.exe"), &[0; 2048]);
        write(&root.join("Celeste/unins000.exe"), &[0; 4096]);
        write(&root.join("Celeste/Launcher.exe"), &[0; 8192]);
        write(
            &root.join("Celeste/_CommonRedist/vcredist/vc_redist.x64.exe"),
            &[0; 9],
        );
        // An Unreal-style layout: the program sits deeper than a tool.
        write(
            &root.join("Outer Wilds/Engine/Binaries/Win64/CrashReportClient.exe"),
            &[0; 9],
        );
        write(
            &root.join("Outer Wilds/OuterWilds/Binaries/Win64/OuterWilds.exe"),
            &[0; 4096],
        );
        write(&root.join("Outer Wilds/Setup.exe"), &[0; 9]);
        // Nothing to run.
        write(&root.join("Soundtrack Only/track01.flac"), &[0; 9]);
        // A program right in the picked folder.
        write(&root.join("Tool.exe"), &[0; 9]);

        let games = scan_folder(root);
        let titles: Vec<&str> = games.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["Celeste", "Outer Wilds", "Tool"]);
        assert!(games[0].executable.ends_with("Celeste/Celeste.exe"));
        assert_eq!(games[0].platform, SideloadPlatform::Windows);
        assert_eq!(
            games[0].install_folder.as_deref(),
            Some(&*root.join("Celeste"))
        );
        assert!(games[1].executable.ends_with("OuterWilds.exe"));
        assert_eq!(games[2].install_folder, None);
        // The folder's other programs stay on offer, the launcher first;
        // installers and redistributables don't.
        let others: Vec<String> = games[0]
            .others
            .iter()
            .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(others, ["Launcher.exe"]);
        assert!(games[2].others.is_empty());
    }

    #[test]
    fn a_game_folder_picked_directly_is_one_game() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Hollow_Knight");
        write(&root.join("hollow_knight.exe"), &[0; 9]);
        write(&root.join("UnityCrashHandler64.exe"), &[0; 9]);
        write(&root.join("hollow_knight_Data/level0"), &[0; 9]);
        let games = scan_folder(&root);
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].title, "Hollow Knight");
        assert_eq!(games[0].install_folder.as_deref(), Some(&*root));
    }

    #[cfg(unix)]
    #[test]
    fn reads_native_programs_and_desktop_entries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        native(&root.join("Native Game/game.x86_64"), 64);
        native(&root.join("Native Game/lib/libsteam_api.so"), 64);
        write(&root.join("Script Folder/readme.txt"), b"#!/bin/sh");
        let program = root.join("bin/supertux2");
        native(&program, 64);
        write(
            &root.join("supertux.desktop"),
            format!(
                "[Desktop Entry]\nType=Application\nName=SuperTux\nName[de]=SuperTux DE\nExec=env FOO=1 {} --fullscreen %U\nIcon=supertux\n",
                program.display()
            )
            .as_bytes(),
        );
        write(
            &root.join("steam-game.desktop"),
            b"[Desktop Entry]\nType=Application\nName=Portal\nExec=steam steam://rungameid/400\n",
        );
        write(
            &root.join("hidden.desktop"),
            b"[Desktop Entry]\nType=Application\nName=Hidden\nNoDisplay=true\nExec=/bin/true\n",
        );
        let games = scan_folder(root);
        let titles: Vec<&str> = games.iter().map(|g| g.title.as_str()).collect();
        // The desktop entry names `bin/supertux2`, so that folder is not a
        // game of its own.
        assert_eq!(titles, ["Native Game", "SuperTux"]);
        let tux = &games[1];
        assert_eq!(tux.executable, program);
        assert_eq!(tux.args, "--fullscreen");
        assert_eq!(tux.kind, FoundKind::DesktopEntry);
        assert!(games[0].executable.ends_with("game.x86_64"));
    }

    /// A minimal Unicode shell link to `C:\Games\Hades\Hades.exe` with
    /// arguments and a working folder.
    fn lnk_bytes() -> Vec<u8> {
        let mut b = vec![0u8; 0x4c];
        b[0..4].copy_from_slice(&0x4cu32.to_le_bytes());
        // HasLinkInfo | HasWorkingDir | HasArguments | IsUnicode
        b[0x14..0x18].copy_from_slice(&(0x2u32 | 0x10 | 0x20 | 0x80).to_le_bytes());
        let base = b"C:\\Games\\Hades\\Hades.exe\0";
        let header = 0x1cu32;
        let base_offset = header;
        let suffix_offset = base_offset + base.len() as u32;
        let size = suffix_offset + 1;
        let mut info = Vec::new();
        info.extend_from_slice(&size.to_le_bytes());
        info.extend_from_slice(&header.to_le_bytes());
        info.extend_from_slice(&1u32.to_le_bytes());
        info.extend_from_slice(&0u32.to_le_bytes()); // volume id
        info.extend_from_slice(&base_offset.to_le_bytes());
        info.extend_from_slice(&0u32.to_le_bytes()); // network
        info.extend_from_slice(&suffix_offset.to_le_bytes());
        info.extend_from_slice(base);
        info.push(0);
        b.extend(info);
        for s in ["C:\\Games\\Hades", "-dx12"] {
            let units: Vec<u16> = s.encode_utf16().collect();
            b.extend_from_slice(&(units.len() as u16).to_le_bytes());
            for u in units {
                b.extend_from_slice(&u.to_le_bytes());
            }
        }
        b
    }

    #[test]
    fn parses_shell_links() {
        let link = parse_lnk(&lnk_bytes()).unwrap();
        assert_eq!(link.target, "C:\\Games\\Hades\\Hades.exe");
        assert_eq!(link.arguments.as_deref(), Some("-dx12"));
        assert_eq!(link.working_dir.as_deref(), Some("C:\\Games\\Hades"));
        assert!(parse_lnk(b"not a link").is_none());
        assert!(parse_lnk(&lnk_bytes()[..0x50]).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn shortcuts_in_a_wine_prefix_point_into_it() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("pfx");
        std::fs::create_dir_all(prefix.join("dosdevices")).unwrap();
        write(&prefix.join("drive_c/Games/Hades/Hades.exe"), &[0; 9]);
        let menu = prefix.join("drive_c/users/me/Start Menu/Programs");
        write(&menu.join("Hades.lnk"), &lnk_bytes());
        let games = scan_folder(&menu);
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].title, "Hades");
        assert_eq!(
            games[0].executable,
            prefix.join("drive_c/Games/Hades/Hades.exe")
        );
        assert_eq!(games[0].args, "-dx12");
        assert_eq!(
            games[0].install_folder.as_deref(),
            Some(&*prefix.join("drive_c/Games/Hades"))
        );
    }

    #[test]
    fn detects_single_installed_repack_without_shortcuts_and_ignores_wine_system_files() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("prefix");
        let installer = dir.path().join("setup.exe");
        write(&installer, &[0; 16]);

        // Snapshot taken before prefix creation (empty prefix).
        let before = PrefixSnapshot::capture(&prefix, None);

        // `wineboot --init` creates Wine's system executables.
        let drive_c = prefix.join("pfx/drive_c");
        std::fs::create_dir_all(prefix.join("pfx/dosdevices")).unwrap();
        write(&drive_c.join("windows/explorer.exe"), &[0; 64]);
        write(&drive_c.join("windows/system32/wineboot.exe"), &[0; 64]);
        write(
            &drive_c.join("Program Files/Internet Explorer/iexplore.exe"),
            &[0; 64],
        );
        write(
            &drive_c.join("Program Files/Windows Media Player/wmplayer.exe"),
            &[0; 64],
        );
        write(
            &drive_c.join("Program Files/Windows NT/Accessories/wordpad.exe"),
            &[0; 64],
        );
        write(
            &drive_c.join("ProgramData/Package Cache/{123}/VC_redist.x64.exe"),
            &[0; 64],
        );

        // Repack installs into drive_c/Games/Hollow Knight without any .lnk shortcuts.
        let game_dir = drive_c.join("Games/Hollow Knight");
        let exe = game_dir.join("hollow_knight.exe");
        write(&exe, &[0; 4096]);
        write(&game_dir.join("UnityCrashHandler64.exe"), &[0; 1024]);
        write(&game_dir.join("unins000.exe"), &[0; 2048]);
        write(&game_dir.join("QuickSFV.exe"), &[0; 512]);
        write(
            &game_dir.join("_CommonRedist/vcredist/vc_redist.x64.exe"),
            &[0; 512],
        );

        let outcome =
            detect_installed_executables(&prefix, "Hollow Knight", &installer, None, &before);
        assert_eq!(
            outcome,
            DetectionOutcome::Picked(InstalledCandidate {
                executable: exe,
                args: String::new(),
                install_folder: Some(game_dir),
                label: None,
                from_shortcut: false,
            })
        );
    }

    #[test]
    fn asks_when_multiple_game_executables_are_installed_and_ranks_main_first() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("prefix");
        let installer = dir.path().join("setup.exe");
        write(&installer, &[0; 16]);
        let before = PrefixSnapshot::capture(&prefix, None);

        let game_dir = prefix.join("drive_c/Games/Skyrim Special Edition");
        std::fs::create_dir_all(prefix.join("dosdevices")).unwrap();
        let main_exe = game_dir.join("SkyrimSE.exe");
        let launcher_exe = game_dir.join("SkyrimSELauncher.exe");
        write(&main_exe, &[0; 4096]);
        write(&launcher_exe, &[0; 8192]);
        write(&game_dir.join("unins000.exe"), &[0; 1024]);

        let outcome = detect_installed_executables(
            &prefix,
            "Skyrim Special Edition",
            &installer,
            None,
            &before,
        );
        let DetectionOutcome::Ambiguous(candidates) = outcome else {
            panic!("expected Ambiguous, got {outcome:?}");
        };
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].executable, main_exe);
        assert_eq!(candidates[1].executable, launcher_exe);
    }

    #[test]
    fn ignores_games_already_present_in_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("prefix");
        let installer = dir.path().join("setup.exe");
        write(&installer, &[0; 16]);
        std::fs::create_dir_all(prefix.join("dosdevices")).unwrap();
        write(&prefix.join("drive_c/Games/OldGame/OldGame.exe"), &[0; 512]);

        let before = PrefixSnapshot::capture(&prefix, None);

        let new_exe = prefix.join("drive_c/Games/Celeste/Celeste.exe");
        write(&new_exe, &[0; 512]);

        let outcome = detect_installed_executables(&prefix, "Celeste", &installer, None, &before);
        match outcome {
            DetectionOutcome::Picked(c) => assert_eq!(c.executable, new_exe),
            other => panic!("expected Picked(Celeste.exe), got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn detects_external_repack_from_registry_install_location_without_shortcuts() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("prefix");
        let external = dir.path().join("external/Cyberpunk 2077");
        let installer = dir.path().join("setup.exe");
        write(&installer, &[0; 16]);
        std::fs::create_dir_all(prefix.join("drive_c")).unwrap();
        std::fs::create_dir_all(prefix.join("dosdevices")).unwrap();
        std::os::unix::fs::symlink("/", prefix.join("dosdevices/z:")).unwrap();

        let before = PrefixSnapshot::capture(&prefix, None);

        let exe = external.join("bin/x64/Cyberpunk2077.exe");
        write(&exe, &[0; 2048]);
        let win_ext = format!("Z:{}", external.to_string_lossy().replace('/', "\\\\"));
        write(
            &prefix.join("system.reg"),
            format!(
                "[Software\\\\Microsoft\\\\Windows\\\\CurrentVersion\\\\Uninstall\\\\Cyberpunk_is1]\n\"Inno Setup: App Path\"=\"{win_ext}\"\n"
            )
            .as_bytes(),
        );

        let outcome =
            detect_installed_executables(&prefix, "Cyberpunk 2077", &installer, None, &before);
        match outcome {
            DetectionOutcome::Picked(c) => {
                assert_eq!(c.executable, exe);
                assert_eq!(c.install_folder, Some(external));
            }
            other => panic!("expected Picked, got {other:?}"),
        }
    }

    #[test]
    fn scan_extracted_game_unwraps_wrapper_and_handles_unreal_multi_exe_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("extract_out");

        // Archive extracted into a single-child wrapper folder alongside macOS junk.
        let game_root = dest.join("MyGame");
        let stub_exe = game_root.join("MyGame.exe");
        let shipping_exe = game_root.join("MyGame/Binaries/Win64/MyGame-Win64-Shipping.exe");
        write(&stub_exe, &[0; 1024]);
        write(&shipping_exe, &[0; 8192]);
        write(
            &game_root.join("Engine/Binaries/Win64/CrashReportClient.exe"),
            &[0; 512],
        );
        write(&game_root.join("unins000.exe"), &[0; 2048]);
        write(&dest.join("__MACOSX/._MyGame"), &[0; 8]);

        let found = scan_extracted_game(&dest).expect("should detect one game");
        assert_eq!(found.title, "My Game");
        assert_eq!(found.executable, stub_exe);
        assert_eq!(found.install_folder.as_deref(), Some(game_root.as_path()));
        assert_eq!(found.platform, SideloadPlatform::Windows);
        assert_eq!(found.others.len(), 1);
        assert_eq!(found.others[0].0, shipping_exe);

        // Scanning the unwrapped game root directly yields the exact same single game.
        let direct = scan_extracted_game(&game_root).expect("should detect directly");
        assert_eq!(direct.executable, stub_exe);
        assert_eq!(direct.others.len(), 1);
        assert_eq!(direct.others[0].0, shipping_exe);
    }

    #[test]
    fn scan_extracted_game_handles_single_executable_and_ignores_non_games() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("SuperGame.exe");
        write(&exe, &[0; 64]);
        let found = scan_extracted_game(&exe).expect("single exe");
        assert_eq!(found.title, "Super Game");
        assert_eq!(found.executable, exe);

        // Standalone installer or non-game folder returns None.
        let setup = dir.path().join("setup.exe");
        write(&setup, &[0; 64]);
        assert_eq!(scan_extracted_game(&setup), None);

        let docs = dir.path().join("Manual");
        write(&docs.join("readme.pdf"), b"%PDF");
        assert_eq!(scan_extracted_game(&docs), None);

        // Does not unwrap a generic `bin/` child into title "Bin".
        let bin_game = dir.path().join("CoolGame");
        write(&bin_game.join("bin/CoolGame.exe"), &[0; 64]);
        let found_bin = scan_extracted_game(&bin_game).expect("bin child game");
        assert_eq!(found_bin.title, "Cool Game");
        assert_eq!(
            found_bin.install_folder.as_deref(),
            Some(bin_game.as_path())
        );
    }
}
