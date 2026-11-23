//! Bethesda games (Oblivion, Fallout 3 and New Vegas, Skyrim, Fallout 4,
//! Starfield): putting a plugin (`.esp`, `.esm`, `.esl`) in `Data` isn't
//! enough, the game loads only the plugins `Plugins.txt` names, and some
//! games ignore loose files older than their archives unless an ini setting
//! says otherwise.
//!
//! While mods are applied, [`apply`] writes `Plugins.txt` (in the game's
//! local application data folder) with the enabled mods' plugins after the
//! ones already listed, and sets the ini keys loose files need. Before it
//! writes anything it records what it will do (`bethesda.json` in the
//! game's mod folder), so [`restore`] puts the user's own `Plugins.txt`
//! back and resets each ini key it changed, also after a crash. A key the
//! game or the user changed meanwhile is theirs and stays.
//!
//! The older games (Oblivion, Fallout 3 and New Vegas, Skyrim) order
//! plugins by their files' modified times, which Irori leaves alone: there
//! the mods' plugins are turned on, in whatever order their files give.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::json_store;
use serde::{Deserialize, Serialize};

/// Suffix of a file set aside while Irori's own stands in for it.
const BACKUP_SUFFIX: &str = ".irori-original";

/// A Bethesda game Irori writes plugin lists for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Game {
    Oblivion,
    Fallout3,
    FalloutNv,
    Skyrim,
    SkyrimSe,
    SkyrimVr,
    Fallout4,
    Fallout4Vr,
    Starfield,
}

/// What differs between the games.
struct Spec {
    /// The game's program, at the game folder's root.
    program: &'static str,
    /// Folders under the local application data folder holding
    /// `Plugins.txt`, the usual one first (store editions add a suffix).
    local_dirs: &'static [&'static str],
    /// Folder under `Documents/My Games` holding the ini.
    my_games: &'static str,
    /// The ini Irori sets keys in, and the keys (`[Archive]`).
    ini: Option<(&'static str, &'static [(&'static str, &'static str)])>,
    /// `Plugins.txt` marks active plugins with `*` and lists inactive ones.
    starred: bool,
    /// The game's own masters, which the game loads first by itself
    /// (starred games) or which the list must start with (the others).
    masters: &'static [&'static str],
    /// The script extender's loader and its name.
    extender: (&'static str, &'static str),
}

const INVALIDATE: &[(&str, &str)] = &[("bInvalidateOlderFiles", "1")];
const INVALIDATE_FINAL: &[(&str, &str)] = &[
    ("bInvalidateOlderFiles", "1"),
    ("sResourceDataDirsFinal", ""),
];

const SKYRIM_SE_MASTERS: &[&str] = &[
    "Skyrim.esm",
    "Update.esm",
    "Dawnguard.esm",
    "HearthFires.esm",
    "Dragonborn.esm",
];
const FALLOUT4_MASTERS: &[&str] = &[
    "Fallout4.esm",
    "DLCRobot.esm",
    "DLCworkshop01.esm",
    "DLCCoast.esm",
    "DLCworkshop02.esm",
    "DLCworkshop03.esm",
    "DLCNukaWorld.esm",
    "DLCUltraHighResolution.esm",
];

impl Game {
    const ALL: [Game; 9] = [
        // VR first: their folders may hold the flat game's program too.
        Game::SkyrimVr,
        Game::Fallout4Vr,
        Game::SkyrimSe,
        Game::Fallout4,
        Game::Starfield,
        Game::Skyrim,
        Game::FalloutNv,
        Game::Fallout3,
        Game::Oblivion,
    ];

    fn spec(self) -> Spec {
        match self {
            Game::Oblivion => Spec {
                program: "Oblivion.exe",
                local_dirs: &["Oblivion"],
                my_games: "Oblivion",
                ini: Some(("Oblivion.ini", INVALIDATE)),
                starred: false,
                masters: &["Oblivion.esm"],
                extender: ("obse_loader.exe", "OBSE"),
            },
            Game::Fallout3 => Spec {
                program: "Fallout3.exe",
                local_dirs: &["Fallout3"],
                my_games: "Fallout3",
                ini: Some(("Fallout.ini", INVALIDATE)),
                starred: false,
                masters: &["Fallout3.esm"],
                extender: ("fose_loader.exe", "FOSE"),
            },
            Game::FalloutNv => Spec {
                program: "FalloutNV.exe",
                local_dirs: &["FalloutNV"],
                my_games: "FalloutNV",
                ini: Some(("Fallout.ini", INVALIDATE)),
                starred: false,
                masters: &["FalloutNV.esm"],
                extender: ("nvse_loader.exe", "NVSE"),
            },
            Game::Skyrim => Spec {
                program: "TESV.exe",
                local_dirs: &["Skyrim"],
                my_games: "Skyrim",
                ini: Some(("Skyrim.ini", INVALIDATE)),
                starred: false,
                masters: &["Skyrim.esm", "Update.esm"],
                extender: ("skse_loader.exe", "SKSE"),
            },
            Game::SkyrimSe => Spec {
                program: "SkyrimSE.exe",
                local_dirs: &[
                    "Skyrim Special Edition",
                    "Skyrim Special Edition GOG",
                    "Skyrim Special Edition EPIC",
                    "Skyrim Special Edition MS",
                ],
                my_games: "Skyrim Special Edition",
                ini: None,
                starred: true,
                masters: SKYRIM_SE_MASTERS,
                extender: ("skse64_loader.exe", "SKSE"),
            },
            Game::SkyrimVr => Spec {
                program: "SkyrimVR.exe",
                local_dirs: &["Skyrim VR"],
                my_games: "Skyrim VR",
                ini: None,
                starred: true,
                masters: SKYRIM_SE_MASTERS,
                extender: ("sksevr_loader.exe", "SKSEVR"),
            },
            Game::Fallout4 => Spec {
                program: "Fallout4.exe",
                local_dirs: &["Fallout4", "Fallout4 MS"],
                my_games: "Fallout4",
                ini: Some(("Fallout4Custom.ini", INVALIDATE_FINAL)),
                starred: true,
                masters: FALLOUT4_MASTERS,
                extender: ("f4se_loader.exe", "F4SE"),
            },
            Game::Fallout4Vr => Spec {
                program: "Fallout4VR.exe",
                local_dirs: &["Fallout4VR"],
                my_games: "Fallout4VR",
                ini: Some(("Fallout4Custom.ini", INVALIDATE_FINAL)),
                starred: true,
                masters: FALLOUT4_MASTERS,
                extender: ("f4sevr_loader.exe", "F4SEVR"),
            },
            Game::Starfield => Spec {
                program: "Starfield.exe",
                local_dirs: &["Starfield"],
                my_games: "Starfield",
                ini: Some(("StarfieldCustom.ini", INVALIDATE_FINAL)),
                starred: true,
                masters: &[
                    "Starfield.esm",
                    "Constellation.esm",
                    "OldMars.esm",
                    "BlueprintShips-Starfield.esm",
                    "SFBGS003.esm",
                    "SFBGS004.esm",
                    "SFBGS006.esm",
                    "SFBGS007.esm",
                    "SFBGS008.esm",
                ],
                extender: ("sfse_loader.exe", "SFSE"),
            },
        }
    }

    /// The script extender's loader program (`skse64_loader.exe`).
    pub fn extender_loader(self) -> &'static str {
        self.spec().extender.0
    }

    /// The script extender's name (`SKSE`).
    pub fn extender_name(self) -> &'static str {
        self.spec().extender.1
    }
}

/// The Bethesda game installed in `game_dir`, by its program.
pub fn detect(game_dir: &Path) -> Option<Game> {
    let names: Vec<String> = std::fs::read_dir(game_dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_lowercase())
        .collect();
    Game::ALL
        .into_iter()
        .find(|g| names.contains(&g.spec().program.to_lowercase()))
}

/// Whether `name` is a plugin file.
pub fn is_plugin(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".esm", ".esp", ".esl"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

/// The plugins in the game's data folder that mod files provide, in load
/// order: by mod, and in a mod masters (`.esm`), then light (`.esl`), then
/// the rest, each by name. `mods` holds each enabled mod's files (relative
/// to the game folder) in load order; `data` is the data folder's name.
pub fn mod_plugins(mods: &[Vec<String>], data: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for files in mods {
        let mut plugins: Vec<&str> = files
            .iter()
            .filter_map(|f| {
                let (dir, name) = f.rsplit_once('/')?;
                (dir.eq_ignore_ascii_case(data) && is_plugin(name)).then_some(name)
            })
            .collect();
        let rank = |n: &str| {
            let lower = n.to_ascii_lowercase();
            if lower.ends_with(".esm") {
                0
            } else if lower.ends_with(".esl") {
                1
            } else {
                2
            }
        };
        plugins.sort_by_key(|n| (rank(n), n.to_ascii_lowercase()));
        for name in plugins {
            // A later mod providing the same plugin wins its place.
            out.retain(|p| !p.eq_ignore_ascii_case(name));
            out.push(name.to_string());
        }
    }
    out
}

/// `Plugins.txt` with `mods` (plugins of the enabled mods, in load order)
/// active after what `existing` lists, which keeps its order and states.
/// `present` tells whether a plugin is in the data folder: the older games'
/// own masters are listed first when present.
pub fn plugins_txt(
    game: Game,
    existing: Option<&str>,
    mods: &[String],
    present: impl Fn(&str) -> bool,
) -> String {
    let spec = game.spec();
    let is_mod = |name: &str| mods.iter().any(|m| m.eq_ignore_ascii_case(name));
    let mut entries: Vec<(String, bool)> = Vec::new();
    for line in existing.unwrap_or_default().lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, active) = match line.strip_prefix('*') {
            Some(name) => (name.trim(), true),
            None => (line, !spec.starred),
        };
        if is_plugin(name)
            && !is_mod(name)
            && !entries.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            entries.push((name.to_string(), active));
        }
    }
    if !spec.starred {
        let masters: Vec<(String, bool)> = spec
            .masters
            .iter()
            .filter(|m| present(m))
            .filter(|m| !entries.iter().any(|(n, _)| n.eq_ignore_ascii_case(m)))
            .map(|m| (m.to_string(), true))
            .collect();
        entries.splice(0..0, masters);
    }
    entries.extend(mods.iter().map(|m| (m.clone(), true)));
    let mut out = String::from(
        "# This file is used by the game to keep track of your downloaded content.\r\n# Irori wrote it while mods are applied and puts yours back afterwards.\r\n",
    );
    for (name, active) in entries {
        match (spec.starred, active) {
            (true, true) => out.push_str(&format!("*{name}\r\n")),
            (true, false) => out.push_str(&format!("{name}\r\n")),
            (false, true) => out.push_str(&format!("{name}\r\n")),
            (false, false) => {}
        }
    }
    out
}

/// The value of `key` in `[section]` of ini text, without letter case.
pub fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_section = name.trim().eq_ignore_ascii_case(section);
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=')
            && k.trim().eq_ignore_ascii_case(key)
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

/// Ini text with `key` in `[section]` set to `value` (`None` removes the
/// key), the rest kept as it is. A missing section is added at the end.
pub fn set_ini_value(text: &str, section: &str, key: &str, value: Option<&str>) -> String {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out: Vec<String> = Vec::new();
    let mut in_section = false;
    let mut found_section = false;
    let mut done = false;
    let lines: Vec<&str> = text.lines().collect();
    for (ix, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let header = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']'));
        if let Some(name) = header {
            if in_section && !done {
                if let Some(v) = value {
                    // Before the blank lines that end the section.
                    let at = out
                        .iter()
                        .rposition(|l| !l.trim().is_empty())
                        .map_or(out.len(), |p| p + 1);
                    out.insert(at, format!("{key}={v}"));
                }
                done = true;
            }
            in_section = name.trim().eq_ignore_ascii_case(section);
            found_section |= in_section;
        } else if in_section
            && !done
            && trimmed
                .split_once('=')
                .is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key))
        {
            if let Some(v) = value {
                let name = trimmed.split_once('=').map_or(key, |(k, _)| k.trim());
                out.push(format!("{name}={v}"));
            }
            done = true;
            continue;
        }
        out.push(line.to_string());
        if ix + 1 == lines.len() && in_section && !done {
            if let Some(v) = value {
                out.push(format!("{key}={v}"));
            }
            done = true;
        }
    }
    if !found_section && let Some(v) = value {
        if out.last().is_some_and(|l| !l.trim().is_empty()) {
            out.push(String::new());
        }
        out.push(format!("[{section}]"));
        out.push(format!("{key}={v}"));
    }
    let mut text = out.join(newline);
    text.push_str(newline);
    text
}

/// Where a game keeps its lists and settings for the user who plays it:
/// on Windows the user's own folders, under Wine those of the prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Places {
    /// `%LOCALAPPDATA%`.
    pub local_app_data: PathBuf,
    /// The Documents folder (holding `My Games`).
    pub documents: PathBuf,
}

impl Places {
    /// The user folders inside a Wine prefix (the folder holding
    /// `drive_c`): Proton's `steamuser`, else the one user that isn't
    /// `Public`. Older prefixes spell the folders the Windows XP way.
    pub fn in_prefix(prefix: &Path) -> Option<Places> {
        let users = prefix.join("drive_c").join("users");
        let mut names: Vec<String> = std::fs::read_dir(&users)
            .ok()?
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.eq_ignore_ascii_case("public"))
            .collect();
        names.sort();
        let user = names
            .iter()
            .find(|n| n.as_str() == "steamuser")
            .or_else(|| names.first())?;
        let home = users.join(user);
        let local = [
            home.join("AppData").join("Local"),
            home.join("Local Settings").join("Application Data"),
        ];
        let docs = [home.join("Documents"), home.join("My Documents")];
        let pick = |options: &[PathBuf]| {
            options
                .iter()
                .find(|p| p.is_dir())
                .unwrap_or(&options[0])
                .clone()
        };
        Some(Places {
            local_app_data: pick(&local),
            documents: pick(&docs),
        })
    }
}

/// What [`apply`] changed, saved before it changes anything.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    #[serde(default)]
    pub files: Vec<WrittenFile>,
    #[serde(default)]
    pub ini: Vec<IniChange>,
}

/// A file Irori wrote in place of the user's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WrittenFile {
    pub path: PathBuf,
    /// The user's file was set aside beside it.
    pub backed_up: bool,
}

/// One ini key Irori set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IniChange {
    pub path: PathBuf,
    pub section: String,
    pub key: String,
    /// The value before, `None` when the key wasn't there.
    pub previous: Option<String>,
    pub value: String,
    /// The ini didn't exist before.
    #[serde(default)]
    pub created: bool,
}

/// What [`apply`] did, for the game's log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    pub plugins_txt: Option<PathBuf>,
    pub plugins: usize,
    pub ini_keys: usize,
}

fn backup_of(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(BACKUP_SUFFIX);
    path.with_file_name(name)
}

/// The folder of `Plugins.txt`: the one of the game's folders that exists,
/// else the one whose name ends with the store's edition suffix
/// (`GOG`, `EPIC`), else the usual one.
fn local_dir(game: Game, local_app_data: &Path, edition: Option<&str>) -> PathBuf {
    let spec = game.spec();
    let existing = spec
        .local_dirs
        .iter()
        .find(|d| local_app_data.join(d).is_dir());
    let by_edition = edition.and_then(|suffix| {
        spec.local_dirs
            .iter()
            .find(|d| d.ends_with(&format!(" {suffix}")))
    });
    local_app_data.join(existing.or(by_edition).unwrap_or(&spec.local_dirs[0]))
}

/// Write `Plugins.txt` with `mods` active and set the ini keys loose files
/// need, recording everything in `record_file` first. `data_dir` is the
/// game's data folder; `edition` the store's suffix for the local folder.
pub fn apply(
    game: Game,
    data_dir: &Path,
    places: &Places,
    edition: Option<&str>,
    mods: &[String],
    record_file: &Path,
) -> Result<Applied> {
    if record_file.exists() {
        bail!("the plugin list is already written");
    }
    let spec = game.spec();
    let plugins_file = local_dir(game, &places.local_app_data, edition).join("Plugins.txt");
    let existing = match std::fs::read_to_string(&plugins_file) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        // Plugins.txt is written in the system's code page.
        Err(_) => std::fs::read(&plugins_file)
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned()),
    };
    let present = |name: &str| {
        data_dir.join(name).is_file() || {
            std::fs::read_dir(data_dir).is_ok_and(|d| {
                d.flatten()
                    .any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
            })
        }
    };
    let text = plugins_txt(game, existing.as_deref(), mods, present);
    let backup = backup_of(&plugins_file);
    if existing.is_some() && backup.exists() {
        bail!("{} is in the way; move it and try again", backup.display());
    }

    let mut record = Record {
        files: vec![WrittenFile {
            path: plugins_file.clone(),
            backed_up: existing.is_some(),
        }],
        ini: Vec::new(),
    };
    let mut ini_text: Option<(PathBuf, String)> = None;
    if let Some((ini, keys)) = spec.ini {
        let path = places
            .documents
            .join("My Games")
            .join(spec.my_games)
            .join(ini);
        let before = std::fs::read_to_string(&path).ok();
        let mut text = before.clone().unwrap_or_default();
        for (key, value) in keys {
            let previous = ini_value(&text, "Archive", key);
            if previous.as_deref() == Some(*value) {
                continue;
            }
            text = set_ini_value(&text, "Archive", key, Some(value));
            record.ini.push(IniChange {
                path: path.clone(),
                section: "Archive".into(),
                key: key.to_string(),
                previous,
                value: value.to_string(),
                created: before.is_none(),
            });
        }
        if !record.ini.is_empty() {
            ini_text = Some((path, text));
        }
    }
    json_store::write(record_file, &record)?;

    if let Some(parent) = plugins_file.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    if existing.is_some() {
        std::fs::rename(&plugins_file, &backup)
            .with_context(|| format!("setting {} aside", plugins_file.display()))?;
    }
    json_store::write_bytes(&plugins_file, text.as_bytes())?;
    let ini_keys = record.ini.len();
    if let Some((path, text)) = ini_text {
        json_store::write_bytes(&path, text.as_bytes())?;
    }
    Ok(Applied {
        plugins_txt: Some(plugins_file),
        plugins: mods.len(),
        ini_keys,
    })
}

/// Undo what `record_file` records and remove it. Safe to run again after a
/// failure at any step.
pub fn restore(record_file: &Path) -> Result<()> {
    let Some(record) = json_store::read::<Record>(record_file)? else {
        return Ok(());
    };
    for file in &record.files {
        let backup = backup_of(&file.path);
        if file.backed_up && backup.exists() {
            std::fs::rename(&backup, &file.path)
                .with_context(|| format!("restoring {}", file.path.display()))?;
        } else if !file.backed_up {
            match std::fs::remove_file(&file.path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(e).with_context(|| format!("removing {}", file.path.display()));
                }
                _ => {}
            }
        }
    }
    let mut paths: Vec<&Path> = record.ini.iter().map(|c| c.path.as_path()).collect();
    paths.dedup();
    for path in paths {
        let Ok(mut text) = std::fs::read_to_string(path) else {
            continue;
        };
        let changes: Vec<&IniChange> = record.ini.iter().filter(|c| c.path == path).collect();
        for change in &changes {
            // A value someone set meanwhile is theirs.
            if ini_value(&text, &change.section, &change.key).as_deref() == Some(&change.value) {
                text = set_ini_value(
                    &text,
                    &change.section,
                    &change.key,
                    change.previous.as_deref(),
                );
            }
        }
        let empty = text
            .lines()
            .all(|l| l.trim().is_empty() || l.trim().eq_ignore_ascii_case("[archive]"));
        if changes.iter().all(|c| c.created) && empty {
            let _ = std::fs::remove_file(path);
        } else {
            json_store::write_bytes(path, text.as_bytes())?;
        }
    }
    std::fs::remove_file(record_file).with_context(|| format!("removing {}", record_file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn games_are_told_by_their_program() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(detect(tmp.path()), None);
        std::fs::write(tmp.path().join("skyrimse.exe"), b"").unwrap();
        assert_eq!(detect(tmp.path()), Some(Game::SkyrimSe));
        std::fs::write(tmp.path().join("SkyrimVR.exe"), b"").unwrap();
        assert_eq!(detect(tmp.path()), Some(Game::SkyrimVr));
        assert_eq!(Game::SkyrimSe.extender_loader(), "skse64_loader.exe");
    }

    #[test]
    fn mod_plugins_follow_the_load_order() {
        let mods = vec![
            vec![
                "Data/b.esp".to_string(),
                "Data/A.esm".into(),
                "Data/meshes/x.nif".into(),
                "Data/c.esl".into(),
            ],
            vec!["Data/Sub/nested.esp".into(), "Data/B.ESP".into()],
            vec!["readme.esp".into()],
        ];
        assert_eq!(mod_plugins(&mods, "Data"), ["A.esm", "c.esl", "B.ESP"]);
    }

    #[test]
    fn starred_lists_keep_the_users_entries() {
        let existing = "# comment\r\n*Unofficial.esp\r\nOff.esp\r\n*Mine.esp\r\n";
        let text = plugins_txt(
            Game::SkyrimSe,
            Some(existing),
            &["mine.esp".into(), "New.esp".into()],
            |_| true,
        );
        let lines: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(
            lines,
            ["*Unofficial.esp", "Off.esp", "*mine.esp", "*New.esp"]
        );
        assert!(text.contains("\r\n"));
    }

    #[test]
    fn older_games_list_active_plugins_after_their_masters() {
        let text = plugins_txt(
            Game::Oblivion,
            Some("Mine.esp\n"),
            &["New.esp".into()],
            |name| name == "Oblivion.esm",
        );
        let lines: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
        assert_eq!(lines, ["Oblivion.esm", "Mine.esp", "New.esp"]);
    }

    #[test]
    fn ini_keys_are_set_and_removed_in_place() {
        let text = "[General]\r\nsLanguage=en\r\n\r\n[Archive]\r\nbInvalidateOlderFiles=0\r\n\r\n[Display]\r\nx=1\r\n";
        let set = set_ini_value(text, "archive", "binvalidateolderfiles", Some("1"));
        assert_eq!(
            ini_value(&set, "Archive", "bInvalidateOlderFiles").as_deref(),
            Some("1")
        );
        assert!(set.contains("bInvalidateOlderFiles=1\r\n"));
        let added = set_ini_value(&set, "Archive", "sResourceDataDirsFinal", Some(""));
        assert_eq!(
            ini_value(&added, "Archive", "sResourceDataDirsFinal").as_deref(),
            Some("")
        );
        assert_eq!(ini_value(&added, "Display", "x").as_deref(), Some("1"));
        let removed = set_ini_value(&added, "Archive", "sResourceDataDirsFinal", None);
        assert_eq!(removed, set);
        let fresh = set_ini_value("", "Archive", "bInvalidateOlderFiles", Some("1"));
        assert_eq!(fresh, "[Archive]\nbInvalidateOlderFiles=1\n");
        let last = set_ini_value("[Archive]\na=1", "Archive", "b", Some("2"));
        assert_eq!(last, "[Archive]\na=1\nb=2\n");
    }

    #[test]
    fn apply_and_restore_put_the_users_files_back() {
        let tmp = tempfile::tempdir().unwrap();
        let prefix = tmp.path().join("pfx");
        let user = prefix.join("drive_c/users/steamuser");
        std::fs::create_dir_all(prefix.join("drive_c/users/Public")).unwrap();
        std::fs::create_dir_all(user.join("AppData/Local/Fallout4")).unwrap();
        let ini_dir = user.join("Documents/My Games/Fallout4");
        std::fs::create_dir_all(&ini_dir).unwrap();
        let places = Places::in_prefix(&prefix).unwrap();
        assert_eq!(places.local_app_data, user.join("AppData/Local"));
        let plugins = user.join("AppData/Local/Fallout4/Plugins.txt");
        std::fs::write(&plugins, "*Mine.esp\r\n").unwrap();
        let ini = ini_dir.join("Fallout4Custom.ini");
        std::fs::write(&ini, "[Archive]\nbInvalidateOlderFiles=0\n").unwrap();
        let data = tmp.path().join("game/Data");
        std::fs::create_dir_all(&data).unwrap();
        let record = tmp.path().join("bethesda.json");

        let applied = apply(
            Game::Fallout4,
            &data,
            &places,
            None,
            &["Mod.esp".into()],
            &record,
        )
        .unwrap();
        assert_eq!(applied.ini_keys, 2);
        let written = std::fs::read_to_string(&plugins).unwrap();
        assert!(written.contains("*Mine.esp\r\n*Mod.esp\r\n"));
        let text = std::fs::read_to_string(&ini).unwrap();
        assert_eq!(
            ini_value(&text, "Archive", "bInvalidateOlderFiles").as_deref(),
            Some("1")
        );
        assert!(apply(Game::Fallout4, &data, &places, None, &[], &record).is_err());

        // The user changes one key while playing; that one stays theirs.
        let text = set_ini_value(
            &text,
            "Archive",
            "sResourceDataDirsFinal",
            Some("STRINGS\\"),
        );
        std::fs::write(&ini, text).unwrap();
        restore(&record).unwrap();
        assert_eq!(std::fs::read_to_string(&plugins).unwrap(), "*Mine.esp\r\n");
        let text = std::fs::read_to_string(&ini).unwrap();
        assert_eq!(
            ini_value(&text, "Archive", "bInvalidateOlderFiles").as_deref(),
            Some("0")
        );
        assert_eq!(
            ini_value(&text, "Archive", "sResourceDataDirsFinal").as_deref(),
            Some("STRINGS\\")
        );
        assert!(!record.exists());
        restore(&record).unwrap();
    }

    #[test]
    fn files_irori_created_go_again() {
        let tmp = tempfile::tempdir().unwrap();
        let places = Places {
            local_app_data: tmp.path().join("local"),
            documents: tmp.path().join("docs"),
        };
        let data = tmp.path().join("Data");
        std::fs::create_dir_all(&data).unwrap();
        let record = tmp.path().join("bethesda.json");
        apply(
            Game::SkyrimSe,
            &data,
            &places,
            Some("GOG"),
            &["A.esp".into()],
            &record,
        )
        .unwrap();
        let plugins = places
            .local_app_data
            .join("Skyrim Special Edition GOG/Plugins.txt");
        assert!(plugins.is_file());
        apply(
            Game::Starfield,
            &data,
            &places,
            None,
            &[],
            &tmp.path().join("sf.json"),
        )
        .unwrap();
        let ini = places
            .documents
            .join("My Games/Starfield/StarfieldCustom.ini");
        assert!(ini.is_file());
        restore(&record).unwrap();
        restore(&tmp.path().join("sf.json")).unwrap();
        assert!(!plugins.exists());
        assert!(!ini.exists());
    }
}
