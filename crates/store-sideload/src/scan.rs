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

fn read_dir(dir: &Path) -> impl Iterator<Item = std::fs::DirEntry> {
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
    ];
    const WHOLE: &[&str] = &[
        "python", "pythonw", "java", "javaw", "7z", "unrar", "unzip", "zsync", "dosbox", "scummvm",
        "wine",
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
    let (path, platform, _) = candidates
        .into_iter()
        .max_by_key(|(path, platform, depth)| {
            let name = squash(&stem(path));
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
            let native = *platform != SideloadPlatform::Windows;
            let size = std::fs::metadata(path).map_or(0, |m| m.len()) / (1 << 20);
            (
                !tool,
                similar,
                std::cmp::Reverse(*depth),
                native,
                bits,
                size,
            )
        })?;
    Some(FoundGame {
        title: pretty_title(&folder),
        executable: path,
        platform,
        args: String::new(),
        install_folder: Some(dir.to_path_buf()),
        icon: None,
        kind: FoundKind::Program,
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
    })
}

/// A Windows path as a path on this computer: unchanged on Windows; inside
/// the Wine prefix holding `near` elsewhere.
fn windows_path(target: &str, near: &Path) -> Option<PathBuf> {
    if cfg!(windows) {
        return Some(PathBuf::from(target));
    }
    let (drive, rest) = target.split_once(":\\")?;
    let drive = drive.to_lowercase();
    let prefix = near
        .ancestors()
        .find(|a| a.join("drive_c").is_dir() && a.join("dosdevices").is_dir())?;
    let root = if drive == "c" {
        prefix.join("drive_c")
    } else {
        prefix.join("dosdevices").join(format!("{drive}:"))
    };
    Some(rest.split('\\').fold(root, |p, part| p.join(part)))
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
        Some(Some(text))
    };
    let _name = read_string(flags & HAS_NAME != 0)?;
    let relative = read_string(flags & HAS_RELATIVE_PATH != 0)?;
    let working_dir = read_string(flags & HAS_WORKING_DIR != 0)?;
    let arguments = read_string(flags & HAS_ARGUMENTS != 0)?;
    let target = target.or(relative)?;
    Some(LnkTarget {
        target,
        arguments: arguments.filter(|a| !a.trim().is_empty()),
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
}
