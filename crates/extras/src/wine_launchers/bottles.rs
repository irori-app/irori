//! Bottles: each bottle is a Wine prefix with a `bottle.yml` naming its
//! Wine build ("runner"), its environment variables and the programs the
//! user added to it (`External_Programs`). Each program becomes an added
//! Windows game in that prefix with that build. Programs Bottles finds in a
//! bottle's Start menu are left to the caller, which reads shortcuts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::Deserialize;

use super::{LutrisPlan, SkipReason, planned, xdg};
use crate::lutris_import::ImportKind;

const FLATPAK_ID: &str = "com.usebottles.bottles";

/// The folders of one Bottles installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BottlesDirs {
    pub flatpak: bool,
    /// Where the bottles are: `<data>/bottles/bottles`, or the custom
    /// folder set in Bottles' preferences.
    pub bottles_dir: PathBuf,
    /// Bottles' own Wine builds: `<data>/bottles/runners`.
    pub runners_dir: PathBuf,
}

impl BottlesDirs {
    /// Every Bottles installation of the current user with a bottles
    /// folder: the native one, then the Flatpak.
    pub fn discover() -> Vec<BottlesDirs> {
        let Some(home) = dirs::home_dir() else {
            return Vec::new();
        };
        Self::under(&home, &xdg(&home, "XDG_DATA_HOME", ".local/share"))
    }

    /// The installations under `home`, the native one in `data`.
    pub fn under(home: &Path, data: &Path) -> Vec<BottlesDirs> {
        [
            (false, data.join("bottles")),
            (
                true,
                home.join(".var/app").join(FLATPAK_ID).join("data/bottles"),
            ),
        ]
        .into_iter()
        .filter_map(|(flatpak, root)| {
            let custom = std::fs::read_to_string(root.join("data.yml"))
                .ok()
                .and_then(|text| serde_yaml_ng::from_str::<DataFile>(&text).ok())
                .and_then(|d| d.custom_bottles_path)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute() && p.is_dir());
            let bottles_dir = custom.unwrap_or_else(|| root.join("bottles"));
            bottles_dir.is_dir().then(|| BottlesDirs {
                flatpak,
                bottles_dir,
                runners_dir: root.join("runners"),
            })
        })
        .collect()
    }
}

#[derive(Debug, Default, Deserialize)]
struct DataFile {
    #[serde(default)]
    custom_bottles_path: Option<String>,
}

/// The parts of `bottle.yml` Irori reads.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BottleFile {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Path")]
    path: String,
    #[serde(rename = "Custom_Path")]
    custom_path: bool,
    #[serde(rename = "Runner")]
    runner: String,
    #[serde(rename = "Arch")]
    arch: String,
    #[serde(rename = "Environment_Variables")]
    environment_variables: BTreeMap<String, serde_yaml_ng::Value>,
    #[serde(rename = "External_Programs")]
    external_programs: BTreeMap<String, Program>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Program {
    name: String,
    executable: String,
    path: String,
    folder: String,
    arguments: String,
    removed: bool,
}

/// One bottle as found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bottle {
    pub name: String,
    /// The prefix.
    pub path: PathBuf,
    /// Bottles' name of its Wine build (`soda-9.0-1`, `sys-wine-9.0`).
    pub runner: String,
}

/// Everything the Bottles installation `dirs` would import, and its bottles
/// (whose Start menus the caller may read for more games).
pub fn plan(dirs: &BottlesDirs) -> Result<(LutrisPlan, Vec<Bottle>)> {
    let entries = std::fs::read_dir(&dirs.bottles_dir)
        .with_context(|| format!("reading {}", dirs.bottles_dir.display()))?;
    let mut out = LutrisPlan::default();
    let mut bottles = Vec::new();
    let mut folders: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    folders.sort();
    for folder in folders {
        let file = folder.join("bottle.yml");
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let bottle: BottleFile = match serde_yaml_ng::from_str(&text) {
            Ok(b) => b,
            Err(e) => {
                out.warnings.push(format!("{}: {e}", file.display()));
                continue;
            }
        };
        let prefix = if bottle.custom_path && Path::new(&bottle.path).is_absolute() {
            PathBuf::from(&bottle.path)
        } else {
            folder.clone()
        };
        let name = if bottle.name.is_empty() {
            folder
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            bottle.name.clone()
        };
        let env: BTreeMap<String, String> = bottle
            .environment_variables
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), yaml_text(v)?)))
            .collect();
        let wine_version = (!bottle.runner.is_empty()).then(|| bottle.runner.clone());
        for (id, program) in &bottle.external_programs {
            if program.removed {
                continue;
            }
            let title = if program.name.is_empty() {
                Path::new(&program.executable)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            } else {
                program.name.clone()
            };
            let exe = PathBuf::from(&program.path);
            let kind = if program.path.is_empty() {
                ImportKind::Skipped {
                    reason: SkipReason::MissingEntryPoint,
                }
            } else if !exe.is_file() {
                ImportKind::Skipped {
                    reason: SkipReason::FileMissing(exe),
                }
            } else {
                ImportKind::Windows {
                    exe,
                    args: Some(program.arguments.clone()).filter(|a| !a.trim().is_empty()),
                    working_dir: Some(PathBuf::from(&program.folder))
                        .filter(|f| !program.folder.is_empty() && f.is_dir()),
                    prefix: Some(prefix.clone()),
                    wine_version: wine_version.clone(),
                    arch: Some(bottle.arch.clone()).filter(|a| !a.is_empty()),
                    env: env.clone(),
                }
            };
            out.games
                .push(planned(title, format!("{name}/{id}"), "bottles", kind));
        }
        bottles.push(Bottle {
            name,
            path: prefix,
            runner: bottle.runner,
        });
    }
    Ok((out, bottles))
}

/// A YAML scalar as text.
fn yaml_text(value: &serde_yaml_ng::Value) -> Option<String> {
    match value {
        serde_yaml_ng::Value::String(s) => Some(s.clone()),
        serde_yaml_ng::Value::Number(n) => Some(n.to_string()),
        serde_yaml_ng::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

// Bottles and Faugus Launcher run on Linux only.
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn reads_bottles_and_their_programs() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let data = home.join(".local/share");
        let bottle = data.join("bottles/bottles/Gaming");
        let game = home.join("Games/Hades/Hades.exe");
        write(&game, "");
        write(
            &bottle.join("bottle.yml"),
            &format!(
                r#"
Arch: win64
Custom_Path: false
Environment_Variables:
  DXVK_ASYNC: "1"
  RETRIES: 3
External_Programs:
  a1:
    executable: Hades.exe
    name: Hades
    path: {game}
    folder: {folder}
    arguments: -dx11
    id: a1
  b2:
    executable: Gone.exe
    name: Gone Game
    path: /nowhere/Gone.exe
    id: b2
  c3:
    executable: Old.exe
    name: Old
    path: /nowhere/Old.exe
    removed: true
Name: Gaming
Path: Gaming
Runner: soda-9.0-1
Versioning: false
"#,
                game = game.display(),
                folder = game.parent().unwrap().display()
            ),
        );
        // A bottle file Bottles itself could not read is reported, not fatal.
        write(
            &data.join("bottles/bottles/Broken/bottle.yml"),
            "Name: [unclosed",
        );

        let installs = BottlesDirs::under(home, &data);
        assert_eq!(installs.len(), 1);
        let (plan, bottles) = plan(&installs[0]).unwrap();
        assert_eq!(bottles.len(), 1);
        assert_eq!(bottles[0].path, bottle);
        assert_eq!(plan.warnings.len(), 1);
        assert_eq!(plan.games.len(), 2);
        let hades = plan.games.iter().find(|g| g.name == "Hades").unwrap();
        match &hades.kind {
            ImportKind::Windows {
                exe,
                args,
                working_dir,
                prefix,
                wine_version,
                env,
                ..
            } => {
                assert_eq!(exe, &game);
                assert_eq!(args.as_deref(), Some("-dx11"));
                assert_eq!(working_dir.as_deref(), game.parent());
                assert_eq!(prefix.as_deref(), Some(&*bottle));
                assert_eq!(wine_version.as_deref(), Some("soda-9.0-1"));
                assert_eq!(env.get("DXVK_ASYNC").map(String::as_str), Some("1"));
                assert_eq!(env.get("RETRIES").map(String::as_str), Some("3"));
            }
            other => panic!("unexpected {other:?}"),
        }
        let gone = plan.games.iter().find(|g| g.name == "Gone Game").unwrap();
        assert_eq!(
            gone.kind,
            ImportKind::Skipped {
                reason: SkipReason::FileMissing("/nowhere/Gone.exe".into())
            }
        );
    }

    #[test]
    fn follows_a_custom_bottles_folder() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let data = home.join(".local/share");
        let custom = home.join("Elsewhere");
        std::fs::create_dir_all(&custom).unwrap();
        write(
            &data.join("bottles/data.yml"),
            &format!("custom_bottles_path: {}\n", custom.display()),
        );
        let installs = BottlesDirs::under(home, &data);
        assert_eq!(installs.len(), 1);
        assert_eq!(installs[0].bottles_dir, custom);
    }
}
