//! Settings that make a launcher behave under Wine, written into a prefix
//! before Irori first runs the launcher's installer there:
//!
//! - Battle.net (`%APPDATA%\Battle.net\Battle.net.config`):
//!   - `Client.HardwareAcceleration = "true"`, paired with `--in-process-gpu`
//!     on launch so the CEF UI stays responsive under Wine/Proton without blank
//!     windows.
//!   - `Client.GameLaunchWindowBehavior = "2"` (default if unset), so
//!     Battle.net exits completely when starting a game, freeing RAM/GPU and
//!     avoiding focus conflicts.
//!   - `Client.DefaultStartupScreen = "1"` (default if unset), so it opens on
//!     the last viewed game page.
//!   - `Client.SingleInstance = "true"` (default if unset).
//!   - `Client.GameSearch.BackgroundSearch = "true"` (default if unset), so it
//!     detects games already present on disk.
//!   - `Client.Install.CreateDesktopShortcut = "false"` (default if unset), to
//!     avoid creating broken Linux desktop shortcuts without Wine/umu flags.
//! - Ubisoft Connect without its in-game overlay (`overlay.enabled` in
//!   `%LOCALAPPDATA%\Ubisoft Game Launcher\settings.yaml`), which hooks the
//!   game's renderer and crashes or freezes games under Wine.
//!
//! Only those keys change; existing Battle.net user choices for the optional
//! defaults and every unrelated setting in a file that is already there stay
//! as written. The EA app and the Rockstar Games Launcher need nothing.

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

/// `Battle.net.config` with Wine-friendly defaults, or `None` when nothing
/// changes (or the file is not JSON Irori can safely rewrite).
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

    let mut changed = false;
    // Battle.net writes every value as a string. HardwareAcceleration is
    // paired with `--in-process-gpu` on Linux launches, so upgrade older
    // prefixes where Irori previously wrote "false".
    let on = Value::String("true".into());
    if client.get("HardwareAcceleration") != Some(&on) {
        client.insert("HardwareAcceleration".into(), on);
        changed = true;
    }

    // Populate user-overridable defaults only when absent.
    for (key, value) in [
        ("GameLaunchWindowBehavior", "2"),
        ("DefaultStartupScreen", "1"),
        ("SingleInstance", "true"),
    ] {
        if !client.contains_key(key) {
            client.insert(key.into(), Value::String(value.into()));
            changed = true;
        }
    }
    for (section, key, value) in [
        ("GameSearch", "BackgroundSearch", "true"),
        ("Install", "CreateDesktopShortcut", "false"),
    ] {
        match client.get_mut(section) {
            Some(Value::Object(map)) => {
                if !map.contains_key(key) {
                    map.insert(key.into(), Value::String(value.into()));
                    changed = true;
                }
            }
            None => {
                let mut map = Map::new();
                map.insert(key.into(), Value::String(value.into()));
                client.insert(section.into(), Value::Object(map));
                changed = true;
            }
            // Leave unexpected non-object values untouched.
            Some(_) => {}
        }
    }

    if !changed {
        return None;
    }
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
    fn battlenet_writes_defaults_and_keeps_user_overrides() {
        let fresh = battlenet_config(None).unwrap();
        let v: serde_json::Value = serde_json::from_str(&fresh).unwrap();
        assert_eq!(v["Client"]["HardwareAcceleration"], "true");
        assert_eq!(v["Client"]["GameLaunchWindowBehavior"], "2");
        assert_eq!(v["Client"]["DefaultStartupScreen"], "1");
        assert_eq!(v["Client"]["SingleInstance"], "true");
        assert_eq!(v["Client"]["GameSearch"]["BackgroundSearch"], "true");
        assert_eq!(v["Client"]["Install"]["CreateDesktopShortcut"], "false");

        assert_eq!(battlenet_config(Some(&fresh)), None, "idempotent");

        let existing = r#"{
            "Client": {
                "HardwareAcceleration": "false",
                "GameLaunchWindowBehavior": "0",
                "DefaultStartupScreen": "0",
                "SingleInstance": "false",
                "GameSearch": { "BackgroundSearch": "false" },
                "Install": { "CreateDesktopShortcut": "true", "Custom": "kept" },
                "Sound": { "Enabled": "false" }
            },
            "Games": { "x": 1 }
        }"#;
        let updated = battlenet_config(Some(existing)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&updated).unwrap();
        assert_eq!(v["Client"]["HardwareAcceleration"], "true");
        assert_eq!(v["Client"]["GameLaunchWindowBehavior"], "0");
        assert_eq!(v["Client"]["DefaultStartupScreen"], "0");
        assert_eq!(v["Client"]["SingleInstance"], "false");
        assert_eq!(v["Client"]["GameSearch"]["BackgroundSearch"], "false");
        assert_eq!(v["Client"]["Install"]["CreateDesktopShortcut"], "true");
        assert_eq!(v["Client"]["Install"]["Custom"], "kept");
        assert_eq!(v["Client"]["Sound"]["Enabled"], "false");
        assert_eq!(v["Games"]["x"], 1);
        assert_eq!(
            battlenet_config(Some(&updated)),
            None,
            "second pass is idempotent"
        );

        assert_eq!(battlenet_config(Some("not json")), None, "left alone");
        assert_eq!(battlenet_config(Some("[1]")), None, "left alone");
        assert_eq!(
            battlenet_config(Some(r#"{"Client": 1}"#)),
            None,
            "non-object Client left alone"
        );
        let non_object_sections = r#"{
            "Client": {
                "HardwareAcceleration": "true",
                "GameLaunchWindowBehavior": "2",
                "DefaultStartupScreen": "1",
                "SingleInstance": "true",
                "GameSearch": "unexpected",
                "Install": 42
            }
        }"#;
        assert_eq!(
            battlenet_config(Some(non_object_sections)),
            None,
            "non-object nested sections left alone"
        );
        let from_whitespace = battlenet_config(Some("   \n\t")).unwrap();
        let ws: serde_json::Value = serde_json::from_str(&from_whitespace).unwrap();
        assert_eq!(ws["Client"]["HardwareAcceleration"], "true");
        assert_eq!(ws["Client"]["GameSearch"]["BackgroundSearch"], "true");
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
