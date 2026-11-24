//! Settings that make a launcher behave under Wine, written into a prefix
//! before Irori first runs the launcher's installer there:
//!
//! - Battle.net without hardware acceleration (`Client.HardwareAcceleration`
//!   in `%APPDATA%\Battle.net\Battle.net.config`), whose GPU-drawn window
//!   stays blank or flickers under many Wine builds.
//! - Ubisoft Connect without its in-game overlay (`overlay.enabled` in
//!   `%LOCALAPPDATA%\Ubisoft Game Launcher\settings.yaml`), which hooks the
//!   game's renderer and crashes or freezes games under Wine.
//!
//! Only those keys change; every other setting in a file that is already
//! there stays as written. The EA app and the Rockstar Games Launcher need
//! nothing.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::Launcher;

/// Battle.net's settings file, relative to a user's folder.
const BATTLENET_CONFIG: &str = "AppData/Roaming/Battle.net/Battle.net.config";
/// Ubisoft Connect's settings file, relative to a user's folder.
const UBISOFT_SETTINGS: &str = "AppData/Local/Ubisoft Game Launcher/settings.yaml";

/// Rewrites a settings file's text; `None` leaves it as it is.
type Edit = fn(Option<&str>) -> Option<String>;

/// Write `launcher`'s Wine-friendly settings for every user of `drive`
/// (`drive_c` of a prefix). Returns the files changed.
pub fn prepare(launcher: Launcher, drive: &Path) -> io::Result<Vec<PathBuf>> {
    let (relative, edit): (&str, Edit) = match launcher {
        Launcher::BattleNet => (BATTLENET_CONFIG, battlenet_config),
        Launcher::Ubisoft => (UBISOFT_SETTINGS, ubisoft_settings),
        Launcher::EaApp | Launcher::Rockstar => return Ok(Vec::new()),
    };
    let mut changed = Vec::new();
    for user in users(drive)? {
        let path = user.join(relative);
        let existing = match fs::read_to_string(&path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        let Some(text) = edit(existing.as_deref()) else {
            continue;
        };
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("irori-tmp");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &path)?;
        changed.push(path);
    }
    Ok(changed)
}

/// The users' folders of `drive`, without `Public`.
fn users(drive: &Path) -> io::Result<Vec<PathBuf>> {
    let mut found: Vec<PathBuf> = fs::read_dir(drive.join("users"))?
        .flatten()
        .filter(|u| !u.file_name().eq_ignore_ascii_case("Public"))
        .filter(|u| u.file_type().is_ok_and(|t| t.is_dir()))
        .map(|u| u.path())
        .collect();
    found.sort();
    Ok(found)
}

/// `Battle.net.config` with hardware acceleration off, or `None` when it
/// already is (or the file isn't JSON Irori can safely rewrite).
fn battlenet_config(existing: Option<&str>) -> Option<String> {
    use serde_json::{Map, Value};
    let mut root = match existing.map(str::trim).filter(|t| !t.is_empty()) {
        Some(text) => serde_json::from_str::<Value>(text).ok()?,
        None => Value::Object(Map::new()),
    };
    let client = root
        .as_object_mut()?
        .entry("Client")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()?;
    // Battle.net writes every value as a string.
    let off = Value::String("false".into());
    if client.get("HardwareAcceleration") == Some(&off) {
        return None;
    }
    client.insert("HardwareAcceleration".into(), off);
    serde_json::to_string_pretty(&root).ok()
}

/// `settings.yaml` with the overlay off, or `None` when it already is. A
/// line edit, so the rest of the file stays exactly as Ubisoft Connect
/// wrote it.
fn ubisoft_settings(existing: Option<&str>) -> Option<String> {
    let text = existing.unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let Some(start) = lines.iter().position(|l| l.trim_end() == "overlay:") else {
        let mut out = text.to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("overlay:\n  enabled: false\n");
        return Some(out);
    };
    let block_end = lines[start + 1..]
        .iter()
        .position(|l| !l.trim().is_empty() && !l.starts_with([' ', '\t']))
        .map_or(lines.len(), |i| start + 1 + i);
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    match (start + 1..block_end).find(|&i| lines[i].trim_start().starts_with("enabled:")) {
        Some(i) => {
            let value = lines[i].trim_start()["enabled:".len()..].trim();
            if value == "false" {
                return None;
            }
            let indent = &lines[i][..lines[i].len() - lines[i].trim_start().len()];
            out[i] = format!("{indent}enabled: false");
        }
        None => {
            let indent = (start + 1..block_end)
                .map(|i| lines[i])
                .find(|l| !l.trim().is_empty())
                .map_or("  ", |l| &l[..l.len() - l.trim_start().len()]);
            out.insert(start + 1, format!("{indent}enabled: false"));
        }
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    Some(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battlenet_keeps_its_other_settings() {
        let fresh = battlenet_config(None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&fresh).unwrap();
        assert_eq!(v["Client"]["HardwareAcceleration"], "false");

        let existing = r#"{"Client":{"HardwareAcceleration":"true","Sound":{"Enabled":"false"}},"Games":{"x":1}}"#;
        let v: serde_json::Value =
            serde_json::from_str(&battlenet_config(Some(existing)).unwrap()).unwrap();
        assert_eq!(v["Client"]["HardwareAcceleration"], "false");
        assert_eq!(v["Client"]["Sound"]["Enabled"], "false");
        assert_eq!(v["Games"]["x"], 1);

        assert_eq!(battlenet_config(Some(&fresh)), None, "already off");
        assert_eq!(battlenet_config(Some("not json")), None, "left alone");
        assert_eq!(battlenet_config(Some("[1]")), None, "left alone");
    }

    #[test]
    fn ubisoft_turns_only_the_overlay_off() {
        assert_eq!(
            ubisoft_settings(None).unwrap(),
            "overlay:\n  enabled: false\n"
        );
        let existing = "misc:\n  start: true\noverlay:\n    enabled: true\n    fps_enabled: false\nuser:\n  name: x";
        assert_eq!(
            ubisoft_settings(Some(existing)).unwrap(),
            "misc:\n  start: true\noverlay:\n    enabled: false\n    fps_enabled: false\nuser:\n  name: x\n"
        );
        let without = "overlay:\n  fps_enabled: true\nuser: {}\n";
        assert_eq!(
            ubisoft_settings(Some(without)).unwrap(),
            "overlay:\n  enabled: false\n  fps_enabled: true\nuser: {}\n"
        );
        let no_block = "user:\n  name: x";
        assert_eq!(
            ubisoft_settings(Some(no_block)).unwrap(),
            "user:\n  name: x\noverlay:\n  enabled: false\n"
        );
        assert_eq!(ubisoft_settings(Some("overlay:\n  enabled: false\n")), None);
    }

    #[test]
    fn writes_for_every_user_but_public() {
        let dir = tempfile::tempdir().unwrap();
        let drive = dir.path();
        for user in ["steamuser", "Public"] {
            fs::create_dir_all(drive.join("users").join(user)).unwrap();
        }
        let changed = prepare(Launcher::BattleNet, drive).unwrap();
        assert_eq!(
            changed,
            [drive.join("users/steamuser").join(BATTLENET_CONFIG)]
        );
        assert!(prepare(Launcher::BattleNet, drive).unwrap().is_empty());
        assert_eq!(prepare(Launcher::Ubisoft, drive).unwrap().len(), 1);
        assert!(prepare(Launcher::EaApp, drive).unwrap().is_empty());
        assert!(!drive.join("users/Public/AppData").exists());
    }
}
