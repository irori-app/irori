//! Faugus Launcher: `games.json` in its config folder lists each game with
//! its program, prefix, Proton build ("runner"), environment and arguments.
//! Windows games become added games in that prefix; "Linux-Native" games
//! become added native games.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde_json::Value;

use super::{LutrisPlan, SkipReason, env_words, planned, xdg};
use crate::lutris_import::ImportKind;

const FLATPAK_ID: &str = "io.github.Faugus.faugus-launcher";

/// Faugus's runner for games that are not run through Proton.
const NATIVE: &str = "Linux-Native";

/// Where the game lists of each Faugus Launcher installation are.
pub fn discover() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    games_files(&home, &xdg(&home, "XDG_CONFIG_HOME", ".config"))
}

/// The game lists under `home`, the native one in `config`.
pub fn games_files(home: &Path, config: &Path) -> Vec<PathBuf> {
    [
        config.join("faugus-launcher/games.json"),
        home.join(".var/app")
            .join(FLATPAK_ID)
            .join("config/faugus-launcher/games.json"),
    ]
    .into_iter()
    .filter(|p| p.is_file())
    .collect()
}

/// Text of a field that Faugus writes as a string (or leaves out).
fn text(game: &Value, key: &str) -> String {
    match game.get(key) {
        Some(Value::String(s)) => s.trim().to_string(),
        _ => String::new(),
    }
}

/// Everything one `games.json` would import.
pub fn plan(games_file: &Path) -> Result<LutrisPlan> {
    let raw = std::fs::read_to_string(games_file)
        .with_context(|| format!("reading {}", games_file.display()))?;
    let list: Vec<Value> = serde_json::from_str(&raw)
        .with_context(|| format!("{} is not a Faugus game list", games_file.display()))?;
    let mut out = LutrisPlan::default();
    for game in &list {
        let title = text(game, "title");
        let id = text(game, "gameid");
        if title.is_empty() && id.is_empty() {
            continue;
        }
        let path = text(game, "path");
        let runner = text(game, "runner");
        // "launch_arguments" holds variables (and sometimes a wrapper) put
        // before the game; "game_arguments" go after it.
        let launch = text(game, "launch_arguments");
        let mut env = env_words(&launch);
        for toggle in ["mangohud", "gamemode", "disable_hidraw"] {
            env.extend(env_words(&text(game, toggle)));
        }
        // Anything else there wraps the game (Faugus's own tools) and is not
        // the game's.
        let args = irori_core::process::split_args(&text(game, "game_arguments"));
        let args = (!args.is_empty()).then(|| {
            args.iter()
                .map(|a| irori_core::process::shell_quote(a))
                .collect::<Vec<_>>()
                .join(" ")
        });
        let exe = PathBuf::from(&path);
        let kind = if path.is_empty() {
            ImportKind::Skipped {
                reason: SkipReason::MissingEntryPoint,
            }
        } else if !exe.is_file() {
            ImportKind::Skipped {
                reason: SkipReason::FileMissing(exe),
            }
        } else if runner == NATIVE {
            ImportKind::Native {
                working_dir: exe.parent().map(Path::to_path_buf),
                exe,
                args,
                env,
            }
        } else if runner.eq_ignore_ascii_case("steam") {
            ImportKind::Skipped {
                reason: SkipReason::UnknownRunner(runner.clone()),
            }
        } else {
            let prefix = text(game, "prefix");
            ImportKind::Windows {
                working_dir: exe.parent().map(Path::to_path_buf),
                exe,
                args,
                prefix: (!prefix.is_empty()).then(|| PathBuf::from(prefix)),
                wine_version: (!runner.is_empty()).then(|| runner.clone()),
                arch: None,
                env,
            }
        };
        let mut planned = planned(
            if title.is_empty() { id.clone() } else { title },
            id,
            "faugus",
            kind,
        );
        planned.hidden = matches!(game.get("hidden"), Some(Value::Bool(true)));
        planned.favourite = matches!(game.get("favorite"), Some(Value::Bool(true)));
        planned.cover = Some(PathBuf::from(text(game, "banner")))
            .filter(|p| p.is_absolute() && p.is_file() && is_portrait(p));
        out.games.push(planned);
    }
    Ok(out)
}

/// Whether a picture is taller than wide (a cover rather than a banner).
fn is_portrait(path: &Path) -> bool {
    image::image_dimensions(path).is_ok_and(|(w, h)| h > w)
}

// Bottles and Faugus Launcher run on Linux only.
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn reads_the_game_list() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let config = home.join(".config");
        let exe = home.join("Games/Celeste/Celeste.exe");
        let native = home.join("Games/Tux/supertux");
        for p in [&exe, &native] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        let list = serde_json::json!([
            {
                "gameid": "celeste",
                "title": "Celeste",
                "path": exe,
                "prefix": home.join("Faugus/celeste"),
                "launch_arguments": "DXVK_ASYNC=1 PROTON_USE_WINED3D=0",
                "game_arguments": "-windowed \"two words\"",
                "mangohud": "MANGOHUD=1",
                "gamemode": "",
                "runner": "GE-Proton",
                "banner": ""
            },
            {
                "gameid": "tux",
                "title": "SuperTux",
                "path": native,
                "runner": "Linux-Native",
                "hidden": true
            },
            {
                "gameid": "gone",
                "title": "Gone",
                "path": "/nowhere/gone.exe",
                "runner": "GE-Proton"
            },
            {"gameid": "", "title": ""}
        ]);
        let file = config.join("faugus-launcher/games.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, list.to_string()).unwrap();

        assert_eq!(games_files(home, &config), std::slice::from_ref(&file));
        let plan = plan(&file).unwrap();
        assert_eq!(plan.games.len(), 3);
        match &plan.games[0].kind {
            ImportKind::Windows {
                exe: e,
                args,
                prefix,
                wine_version,
                env,
                ..
            } => {
                assert_eq!(e, &exe);
                assert_eq!(args.as_deref(), Some("-windowed 'two words'"));
                assert_eq!(prefix.as_deref(), Some(&*home.join("Faugus/celeste")));
                assert_eq!(wine_version.as_deref(), Some("GE-Proton"));
                assert_eq!(env.get("DXVK_ASYNC").map(String::as_str), Some("1"));
                assert_eq!(env.get("MANGOHUD").map(String::as_str), Some("1"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(plan.games[1].kind, ImportKind::Native { .. }));
        assert!(plan.games[1].hidden);
        assert!(matches!(
            plan.games[2].kind,
            ImportKind::Skipped {
                reason: SkipReason::FileMissing(_)
            }
        ));
        assert!(super::super::bottles::BottlesDirs::under(home, &config).is_empty());
    }

    #[test]
    fn refuses_a_file_that_is_not_a_game_list() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("games.json");
        std::fs::write(&file, "{\"not\": \"a list\"}").unwrap();
        assert!(plan(&file).is_err());
    }
}
