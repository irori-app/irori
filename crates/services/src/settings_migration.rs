//! One-time rewrite of per-game settings written before variables, wrappers
//! and programs merged with the defaults (`irori_core::merged_lists`).
//!
//! Such a list replaced the defaults' whole, so a default entry it left out
//! was out for that game. Read with the merge it would come back, changing
//! what the game starts with. Each such list gets removals of the default
//! entries it left out, so every game reads exactly what it read before. The
//! rewrite runs once, when settings load, and is recorded in
//! `state/migrations.json`; backups written before the merge are rewritten
//! the same way when restored.

use std::path::Path;

use irori_core::json_store;
use irori_core::merged_lists::{MERGED_LISTS, from_whole_list};
use irori_core::settings::{GameSettings, GameSettingsOverrides};
use serde::{Deserialize, Serialize};

/// Migrations already done in this config folder.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Done {
    merged_game_lists: bool,
    /// Migrations a newer Irori recorded, kept as they are.
    #[serde(flatten)]
    other: serde_json::Map<String, serde_json::Value>,
}

/// Rewrite the game settings under `games` (`<runner>/<app>.json`) for the
/// merge, against `defaults`, unless `record` says it was done.
pub fn migrate_game_lists(games: &Path, record: &Path, defaults: &GameSettings) {
    let mut done: Done = json_store::read_or_default(record);
    if done.merged_game_lists {
        return;
    }
    let mut failed = false;
    for runner in std::fs::read_dir(games).into_iter().flatten().flatten() {
        for file in std::fs::read_dir(runner.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = file.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Ok(Some(mut overrides)) = json_store::read::<GameSettingsOverrides>(&path) else {
                continue;
            };
            if keep_whole_lists(&mut overrides, defaults)
                && let Err(e) = json_store::write(&path, &overrides)
            {
                tracing::error!("keeping the lists of {}: {e:#}", path.display());
                failed = true;
            }
        }
    }
    // A file that couldn't be written is tried again at the next start.
    if !failed {
        done.merged_game_lists = true;
        if let Err(e) = json_store::write(record, &done) {
            tracing::error!("recording the settings migration: {e:#}");
        }
    }
}

/// Rewrite the lists of `overrides`, written when they replaced the
/// defaults' whole, so that merged over `defaults` they read the same.
/// Returns whether anything changed.
pub fn keep_whole_lists(overrides: &mut GameSettingsOverrides, defaults: &GameSettings) -> bool {
    let base = serde_json::to_value(defaults).unwrap_or_default();
    let mut changed = false;
    for (key, field) in MERGED_LISTS {
        let Some(old) = overrides.0.get(*key) else {
            continue;
        };
        if let Some(kept) = from_whole_list(base.get(*key), old, field) {
            overrides.set(key, kept);
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::EnvVar;
    use serde_json::json;

    #[test]
    fn older_game_lists_keep_what_they_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let games = dir.path().join("games");
        let record = dir.path().join("state/migrations.json");
        let var = |k: &str, v: &str| EnvVar {
            key: k.into(),
            value: v.into(),
        };
        let defaults = GameSettings {
            environment_options: vec![var("DXVK_HUD", "1"), var("FOO", "2")],
            ..Default::default()
        };
        let file = games.join("legendary/game.json");
        // Written before the merge: the game dropped DXVK_HUD on purpose.
        json_store::write(
            &file,
            &json!({"enviromentOptions": [
                {"key": "FOO", "value": "2"},
                {"key": "OWN", "value": "x"},
            ], "showFps": true}),
        )
        .unwrap();
        let read = || -> GameSettingsOverrides { json_store::read(&file).unwrap().unwrap() };
        let before = vec![var("FOO", "2"), var("OWN", "x")];

        migrate_game_lists(&games, &record, &defaults);
        let once = read();
        assert_eq!(once.apply(&defaults).environment_options, before);
        assert!(once.apply(&defaults).show_fps);

        // A second load changes nothing, even with a list in the new form.
        let mut newer = once.clone();
        newer.set(
            "wrapperOptions",
            json!([{"exe": "gamemoderun", "args": ""}]),
        );
        json_store::write(&file, &newer).unwrap();
        migrate_game_lists(&games, &record, &defaults);
        assert_eq!(read(), newer);
        assert_eq!(read().apply(&defaults).environment_options, before);
    }

    #[test]
    fn lists_that_copied_the_defaults_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let games = dir.path().join("games");
        let file = games.join("gog/1.json");
        let list = json!({"enviromentOptions": [{"key": "A", "value": "1"}]});
        json_store::write(&file, &list).unwrap();
        let defaults = GameSettings {
            environment_options: vec![EnvVar {
                key: "A".into(),
                value: "0".into(),
            }],
            ..Default::default()
        };
        migrate_game_lists(&games, &dir.path().join("m.json"), &defaults);
        let after: serde_json::Value = json_store::read(&file).unwrap().unwrap();
        assert_eq!(after, list);
    }
}
