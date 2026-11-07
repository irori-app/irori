//! The files Steam API emulators keep achievements in, parsed from text:
//!
//! * Goldberg and gbe_fork: `<saves>/<appid>/achievements.json`, an object
//!   of `{"<API name>": {"earned": bool, "earned_time": unix, "progress": n,
//!   "max_progress": n}}`, and the definitions a game ships in
//!   `steam_settings/achievements.json` (an array of `name`, `displayName`,
//!   `description`, `hidden`, `icon`, `icongray`).
//! * gbe_fork's `steam_settings/configs.user.ini`, whose `[user::saves]`
//!   section may move the saves (`local_save_path`) or rename their folder
//!   (`saves_folder_name`).
//! * CODEX and RUNE: `achievements.ini`, a section per achievement with
//!   `Achieved`, `UnlockTime`, `CurProgress` and `MaxProgress`.
//!
//! Nothing here touches the disk.

use std::collections::BTreeMap;
use std::path::Path;

use irori_core::Achievement;
use serde_json::Value;

/// What an emulator recorded of one achievement.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LocalUnlock {
    pub earned: bool,
    /// Unix time of the unlock; 0 when unknown.
    pub time: u64,
    /// How far towards it, `0.0..=1.0`, when the emulator tracks progress.
    pub progress: Option<f32>,
}

impl LocalUnlock {
    /// One record of the same achievement from two files: earned in
    /// either, at the earlier known time.
    pub fn merge(self, other: LocalUnlock) -> LocalUnlock {
        let time = match (self.earned, other.earned) {
            (true, true) => match (self.time, other.time) {
                (0, t) | (t, 0) => t,
                (a, b) => a.min(b),
            },
            (true, false) => self.time,
            (false, true) => other.time,
            (false, false) => 0,
        };
        LocalUnlock {
            earned: self.earned || other.earned,
            time,
            progress: match (self.progress, other.progress) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            },
        }
    }
}

/// Unlocks keyed by API name.
pub type Unlocks = BTreeMap<String, LocalUnlock>;

/// Text of a file that may be UTF-16 (some Windows tools write INI files
/// that way) or start with a byte order mark.
pub fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

fn as_u64(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::Number(n)) => n
            .as_u64()
            .or_else(|| n.as_f64().map(|f| f.max(0.0) as u64))
            .unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

fn as_f32(v: Option<&Value>) -> f32 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) as f32,
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn as_bool(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => matches!(s.trim(), "1" | "true" | "True" | "TRUE"),
        _ => false,
    }
}

/// `progress / max` when the emulator tracks progress.
fn progress(current: f32, max: f32) -> Option<f32> {
    (max > 0.0).then(|| (current / max).clamp(0.0, 1.0))
}

/// The player's unlocks in Goldberg's or gbe_fork's `achievements.json`.
pub fn goldberg_unlocks(text: &str) -> anyhow::Result<Unlocks> {
    let v: Value = serde_json::from_str(text)?;
    let Value::Object(map) = v else {
        anyhow::bail!("achievements.json is not an object");
    };
    Ok(map
        .into_iter()
        .filter(|(name, _)| !name.trim().is_empty())
        .map(|(name, entry)| {
            let earned = as_bool(entry.get("earned"));
            let unlock = LocalUnlock {
                earned,
                time: if earned {
                    as_u64(entry.get("earned_time"))
                } else {
                    0
                },
                progress: progress(
                    as_f32(entry.get("progress")),
                    as_f32(entry.get("max_progress")),
                ),
            };
            (name, unlock)
        })
        .collect())
}

/// An INI file's sections in order, each with its `key=value` pairs. Keys
/// keep their case; comments (`;`, `#`) and lines without `=` are skipped.
pub fn ini(text: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut sections: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            sections.push((name.trim().to_string(), Vec::new()));
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if sections.is_empty() {
            sections.push((String::new(), Vec::new()));
        }
        if let Some((_, pairs)) = sections.last_mut() {
            pairs.push((key.trim().to_string(), value.trim().to_string()));
        }
    }
    sections
}

fn get<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.as_str())
}

/// The player's unlocks in a CODEX or RUNE `achievements.ini`.
pub fn codex_unlocks(text: &str) -> Unlocks {
    ini(text)
        .into_iter()
        // `[SteamAchievements]` only lists the names.
        .filter(|(name, _)| !name.is_empty() && !name.eq_ignore_ascii_case("SteamAchievements"))
        .map(|(name, pairs)| {
            let number =
                |key: &str| -> f32 { get(&pairs, key).and_then(|v| v.parse().ok()).unwrap_or(0.0) };
            let earned =
                get(&pairs, "Achieved").is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
            let unlock = LocalUnlock {
                earned,
                time: if earned {
                    get(&pairs, "UnlockTime")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0)
                } else {
                    0
                },
                progress: progress(number("CurProgress"), number("MaxProgress")),
            };
            (name, unlock)
        })
        .collect()
}

/// Where gbe_fork's `configs.user.ini` keeps the saves.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SaveSettings {
    /// `local_save_path`: absolute, or relative to the emulator's library.
    pub local_save_path: Option<String>,
    /// `saves_folder_name`, in place of `GSE Saves`.
    pub saves_folder_name: Option<String>,
}

/// The `[user::saves]` section of gbe_fork's `configs.user.ini`.
pub fn save_settings(text: &str) -> SaveSettings {
    let mut out = SaveSettings::default();
    for (name, pairs) in ini(text) {
        if !name.eq_ignore_ascii_case("user::saves") {
            continue;
        }
        let value = |key: &str| {
            get(&pairs, key)
                .map(str::to_string)
                .filter(|v| !v.is_empty())
        };
        out.local_save_path = value("local_save_path").or(out.local_save_path);
        out.saves_folder_name = value("saves_folder_name").or(out.saves_folder_name);
    }
    out
}

/// A text of the definitions: a plain string, or one per Steam language.
fn localized(v: Option<&Value>, steam_language: &str) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(map)) => map
            .get(steam_language)
            .or_else(|| map.get("english"))
            .or_else(|| map.values().find(|v| v.is_string()))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

/// The address of an icon `steam_settings` names: a file under it.
fn local_icon(settings_dir: &Path, icon: &str) -> String {
    let icon = icon.trim();
    if icon.is_empty() {
        return String::new();
    }
    if icon.starts_with("http://") || icon.starts_with("https://") {
        return icon.to_string();
    }
    format!("file://{}", settings_dir.join(icon).display())
}

/// The achievements a game's `steam_settings/achievements.json` defines,
/// in `steam_language` where it has that language. `settings_dir` is that
/// `steam_settings` folder, which icon paths are relative to.
pub fn goldberg_definitions(
    text: &str,
    steam_language: &str,
    settings_dir: &Path,
) -> anyhow::Result<Vec<Achievement>> {
    let v: Value = serde_json::from_str(text)?;
    let Value::Array(list) = v else {
        anyhow::bail!("steam_settings/achievements.json is not a list");
    };
    Ok(list
        .iter()
        .filter_map(|d| {
            let id = d.get("name")?.as_str()?.trim();
            if id.is_empty() {
                return None;
            }
            let name = localized(d.get("displayName"), steam_language);
            let gray = d.get("icongray").or_else(|| d.get("icon_gray"));
            Some(Achievement {
                id: id.to_string(),
                key: id.to_string(),
                visible: !as_bool(d.get("hidden")),
                name: if name.is_empty() {
                    id.to_string()
                } else {
                    name
                },
                description: localized(d.get("description"), steam_language),
                image_unlocked: local_icon(
                    settings_dir,
                    d.get("icon").and_then(Value::as_str).unwrap_or_default(),
                ),
                image_locked: local_icon(
                    settings_dir,
                    gray.and_then(Value::as_str).unwrap_or_default(),
                ),
                ..Default::default()
            })
        })
        .collect())
}

/// The app id in a `steam_appid.txt`: its first number, when not 0.
pub fn app_id(text: &str) -> Option<u32> {
    let token = text.split_whitespace().next()?;
    token.parse().ok().filter(|id| *id > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_goldberg_unlocks() {
        let text = r#"{
            "ACH_WIN_ONE_GAME": {"earned": true, "earned_time": 1700000000},
            "ACH_WIN_100_GAMES": {"earned": false, "earned_time": 0,
                                   "progress": 25, "max_progress": 100},
            "ACH_TRAVEL_FAR": {"earned": 1, "earned_time": "1600000000"}
        }"#;
        let u = goldberg_unlocks(text).unwrap();
        assert_eq!(u.len(), 3);
        assert_eq!(
            u["ACH_WIN_ONE_GAME"],
            LocalUnlock {
                earned: true,
                time: 1_700_000_000,
                progress: None
            }
        );
        assert!(!u["ACH_WIN_100_GAMES"].earned);
        assert_eq!(u["ACH_WIN_100_GAMES"].progress, Some(0.25));
        assert_eq!(u["ACH_TRAVEL_FAR"].time, 1_600_000_000);
        assert!(goldberg_unlocks("[]").is_err());
        assert!(goldberg_unlocks("not json").is_err());
    }

    #[test]
    fn reads_codex_unlocks() {
        let text = "\u{feff}[SteamAchievements]\r\n00000=ACH_A\r\nCount=2\r\n\r\n\
                    [ACH_A]\r\nAchieved=1\r\nCurProgress=0\r\nMaxProgress=0\r\n\
                    UnlockTime=1650000000\r\n\r\n; a comment\r\n[ACH_B]\r\nAchieved=0\r\n\
                    CurProgress=3\r\nMaxProgress=4\r\nUnlockTime=0\r\n";
        let u = codex_unlocks(&decode(text.as_bytes()));
        assert_eq!(u.len(), 2);
        assert!(u["ACH_A"].earned);
        assert_eq!(u["ACH_A"].time, 1_650_000_000);
        assert_eq!(u["ACH_A"].progress, None);
        assert!(!u["ACH_B"].earned);
        assert_eq!(u["ACH_B"].progress, Some(0.75));
    }

    #[test]
    fn decodes_utf16_files() {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "[ACH_A]\nAchieved=1\n".encode_utf16() {
            bytes.extend(unit.to_le_bytes());
        }
        assert!(codex_unlocks(&decode(&bytes))["ACH_A"].earned);
    }

    #[test]
    fn reads_gbe_save_settings() {
        let text = "[user::general]\naccount_name=Player\n\n[user::saves]\n\
                    # where saves go\nlocal_save_path=./saves\nsaves_folder_name=\n";
        assert_eq!(
            save_settings(text),
            SaveSettings {
                local_save_path: Some("./saves".into()),
                saves_folder_name: None
            }
        );
        let renamed = save_settings("[user::saves]\nsaves_folder_name = My Saves \n");
        assert_eq!(renamed.saves_folder_name.as_deref(), Some("My Saves"));
        assert_eq!(save_settings("local_save_path=x"), SaveSettings::default());
    }

    #[test]
    fn reads_goldberg_definitions() {
        let text = r#"[
            {"name": "ACH_A", "displayName": {"english": "First", "german": "Erster"},
             "description": "Do it", "hidden": "0", "icon": "images/a.jpg",
             "icongray": "images/a_gray.jpg"},
            {"name": "ACH_B", "displayName": "Secret", "hidden": 1},
            {"displayName": "No name"},
            {"name": "ACH_C"}
        ]"#;
        let dir = Path::new("/games/x/steam_settings");
        let defs = goldberg_definitions(text, "german", dir).unwrap();
        assert_eq!(defs.len(), 3);
        assert_eq!(defs[0].name, "Erster");
        assert_eq!(defs[0].description, "Do it");
        assert!(defs[0].visible);
        assert_eq!(
            defs[0].image_unlocked,
            format!("file://{}", dir.join("images/a.jpg").display())
        );
        assert!(!defs[1].visible);
        assert_eq!(defs[1].image_locked, "");
        assert_eq!(defs[2].name, "ACH_C");
        let english = goldberg_definitions(text, "french", dir).unwrap();
        assert_eq!(english[0].name, "First");
        assert!(goldberg_definitions("{}", "english", dir).is_err());
    }

    #[test]
    fn reads_app_ids() {
        assert_eq!(app_id("480\n"), Some(480));
        assert_eq!(app_id("  1245620 "), Some(1_245_620));
        assert_eq!(app_id("0"), None);
        assert_eq!(app_id("abc"), None);
        assert_eq!(app_id(""), None);
    }

    #[test]
    fn merges_two_records() {
        let a = LocalUnlock {
            earned: true,
            time: 200,
            progress: None,
        };
        let b = LocalUnlock {
            earned: true,
            time: 100,
            progress: None,
        };
        let locked = LocalUnlock {
            earned: false,
            time: 0,
            progress: Some(0.5),
        };
        assert_eq!(a.merge(b).time, 100);
        assert_eq!(locked.merge(a).time, 200);
        assert!(locked.merge(a).earned);
        let unknown = LocalUnlock { time: 0, ..a };
        assert_eq!(unknown.merge(a).time, 200);
    }
}
