//! eXoScummVM launch helpers: translating eXo's `platform.txt` codes into
//! ScummVM's `--platform=<code>` flag.

use std::ffi::OsString;
use std::path::Path;

use irori_core::process::CommandSpec;

/// Maximum `platform.txt` size read from disk.
const MAX_PLATFORM_FILE_BYTES: u64 = 4096;

/// Read `platform.txt` inside `config_dir` (if present) and translate its eXo
/// platform tag to a ScummVM `--platform` code.
pub fn detect_platform_code(config_dir: &Path) -> Option<&'static str> {
    let path = super::find_file_ci(config_dir, "platform.txt")?;
    let meta = std::fs::metadata(&path).ok()?;
    if meta.len() > MAX_PLATFORM_FILE_BYTES {
        return None;
    }
    let raw = std::fs::read_to_string(&path).ok()?;
    let first = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    map_platform_code(first)
}

/// Translate an eXoScummVM platform code into the value accepted by ScummVM's
/// `--platform` flag. Unknown values return `None` so ScummVM still auto-detects
/// from the game's data files instead of rejecting the command line.
pub fn map_platform_code(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "dos" | "pc" => Some("pc"),
        "amiga" => Some("amiga"),
        "atari" | "st" | "atari-st" | "atarist" => Some("atari"),
        "c64" => Some("c64"),
        "nes" => Some("nes"),
        "sp48" | "spectrum" | "zx" | "zxspectrum" => Some("zx"),
        "mac" | "macintosh" => Some("macintosh"),
        "win" | "windows" => Some("windows"),
        "fmtowns" | "towns" => Some("fmtowns"),
        "pc98" => Some("pc98"),
        "pce" | "pcengine" => Some("pce"),
        "segacd" | "sega" => Some("segacd"),
        "3do" => Some("3do"),
        "cdi" => Some("cdi"),
        "apple2" => Some("apple2"),
        "apple2gs" | "2gs" => Some("2gs"),
        "psx" | "playstation" => Some("playstation"),
        "linux" => Some("linux"),
        "os2" => Some("os2"),
        other => {
            tracing::warn!("eXoScummVM: unrecognized platform '{other}' in platform.txt");
            None
        }
    }
}

/// Insert `--platform=<code>` right before `--auto-detect` on a ScummVM command.
pub fn inject_platform_arg(command: &mut CommandSpec, platform: &str) {
    let flag = OsString::from(format!("--platform={platform}"));
    if let Some(pos) = command
        .args
        .iter()
        .position(|a| a.to_str() == Some("--auto-detect"))
    {
        command.args.insert(pos, flag);
    } else {
        command.args.push(flag);
    }
}
