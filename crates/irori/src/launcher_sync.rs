//! Keeping other launchers' libraries in sync (`iroriLauncherImports`):
//! Lutris, Bottles and Faugus Launcher are read again at start, every
//! quarter of an hour and as soon as a switch is turned on, and only the
//! games no import or sync has seen before are imported
//! ([`crate::lutris`] applies them). A game the user took out of Irori
//! stays out, and settings changed in Irori are never overwritten. A game
//! the launcher uninstalled shows as not installed by itself, its playtime
//! kept: an added game is installed while its program is there. The
//! launchers' files are only read.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use irori_core::{AppSettings, Event, LauncherImports, Notice, NoticeLevel, Paths, json_store};
use irori_extras::lutris_import::{ImportKind, LutrisPlan, PlannedGame};
use irori_services::Backend;
use serde::{Deserialize, Serialize};

/// How often the launchers are read again.
const INTERVAL: Duration = Duration::from_secs(15 * 60);
/// The first read waits for the app to settle after its start.
const FIRST_READ: Duration = Duration::from_secs(30);

/// The games each launcher listed when it was last read, by launcher.
#[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Seen {
    games: BTreeMap<String, BTreeSet<String>>,
}

/// Serializes reading and writing the file.
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn seen_path() -> std::path::PathBuf {
    Paths::get().state_dir().join("launcher-imports.json")
}

/// How a launcher's game is known between reads: its row id (Lutris) and
/// its slug (Lutris's slug, a bottle's program, Faugus's game id).
fn game_key(game: &PlannedGame) -> String {
    format!("{}:{}", game.lutris_id, game.slug)
}

/// Note the games of `plan` in `seen`, and return the plan: whole, or with
/// `only_new` only the games neither seen before nor already in Irori's
/// library (`in_library`). A game already there (imported before syncs
/// were recorded, or added by hand) is noted as seen and never imported
/// again, so what the user changed about it in Irori stays. Games skipped
/// (not installed in the launcher yet, a file missing) aren't noted, so a
/// later read imports them once they can be.
fn select(
    seen: &mut BTreeSet<String>,
    mut plan: LutrisPlan,
    only_new: bool,
    in_library: &dyn Fn(&PlannedGame) -> bool,
) -> LutrisPlan {
    let before = seen.clone();
    let countable = |g: &PlannedGame| !matches!(g.kind, ImportKind::Skipped { .. });
    seen.extend(plan.games.iter().filter(|g| countable(g)).map(game_key));
    if only_new {
        plan.games
            .retain(|g| !before.contains(&game_key(g)) && !(countable(g) && in_library(g)));
    }
    plan
}

/// [`select`] against the record of `launcher` in
/// `state/launcher-imports.json`, which it updates.
pub async fn take_unseen(
    launcher: &str,
    plan: LutrisPlan,
    only_new: bool,
    in_library: &(dyn Fn(&PlannedGame) -> bool + Sync),
) -> LutrisPlan {
    let _guard = LOCK.lock().await;
    let path = seen_path();
    let mut state: Seen = tokio::task::spawn_blocking({
        let path = path.clone();
        move || json_store::read_or_default(&path)
    })
    .await
    .unwrap_or_default();
    let seen = state.games.entry(launcher.to_string()).or_default();
    let before = seen.clone();
    let plan = select(seen, plan, only_new, in_library);
    if *seen != before {
        let result = tokio::task::spawn_blocking(move || json_store::write(&path, &state)).await;
        if let Ok(Err(e)) = result {
            tracing::warn!("recording {launcher}'s games: {e:#}");
        }
    }
    plan
}

/// Read every launcher that is switched on, now and then.
pub fn start(backend: Backend) {
    let b = backend.clone();
    backend.spawn(async move {
        let mut events = b.subscribe();
        let mut on = b.settings.app().launcher_imports;
        let mut wait = FIRST_READ;
        loop {
            let deadline = tokio::time::Instant::now() + wait;
            // Wake early when a switch is turned on.
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => break,
                    event = events.recv() => match event {
                        Ok(Event::SettingsChanged) => {
                            let now = b.settings.app().launcher_imports;
                            let switched_on = turned_on(on, now);
                            on = now;
                            if switched_on {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                        _ => {}
                    },
                }
            }
            let app = b.settings.app();
            on = app.launcher_imports;
            if on.any() {
                sync(&b, &app).await;
            }
            wait = INTERVAL;
        }
    });
}

/// Whether a launcher was switched on between `was` and `now`.
fn turned_on(was: LauncherImports, now: LauncherImports) -> bool {
    (now.lutris && !was.lutris) || (now.bottles && !was.bottles) || (now.faugus && !was.faugus)
}

async fn sync(backend: &Backend, app: &AppSettings) {
    use crate::stores::{EMULATION, SIDELOAD};
    let on = app.launcher_imports;
    let runs = [
        (on.lutris, crate::lutris::LUTRIS),
        (on.bottles, crate::lutris::BOTTLES),
        (on.faugus, crate::lutris::FAUGUS),
    ];
    for (enabled, launcher) in runs {
        if !enabled {
            continue;
        }
        let sideload = SIDELOAD.get().cloned();
        let result = match launcher {
            crate::lutris::LUTRIS => {
                crate::lutris::import_lutris(backend, sideload, EMULATION.get().cloned(), true)
                    .await
            }
            crate::lutris::BOTTLES => {
                crate::lutris::import_bottles_with(backend, sideload, true).await
            }
            _ => crate::lutris::import_faugus_with(backend, sideload, true).await,
        };
        match result {
            Ok(summary) if summary.imported > 0 => {
                let count = summary.imported.to_string();
                backend.bus.notice(
                    Notice::new(NoticeLevel::Info, format!("New games from {launcher}"))
                        .message(format!("Added to the library: {count}"))
                        .keys(
                            "shell.notice.launcher_sync_added",
                            Some("shell.notice.launcher_sync_added_message"),
                            &[("launcher", launcher), ("count", &count)],
                        ),
                );
            }
            Ok(_) => {}
            // The launcher may be gone; the next read tries again.
            Err(e) => tracing::debug!("reading {launcher} again: {e:#}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_extras::lutris_import::SkipReason;

    fn game(id: i64, slug: &str) -> PlannedGame {
        PlannedGame {
            lutris_id: id,
            name: slug.into(),
            slug: slug.into(),
            runner: "wine".into(),
            kind: ImportKind::Skipped {
                reason: SkipReason::NotInstalled,
            },
            favourite: false,
            hidden: false,
            categories: Vec::new(),
            playtime_minutes: 0,
            last_played: None,
            cover: None,
            commands: Default::default(),
        }
    }

    fn plan(games: Vec<PlannedGame>) -> LutrisPlan {
        LutrisPlan {
            games,
            warnings: Vec::new(),
        }
    }

    fn installed(id: i64, slug: &str, exe: &str) -> PlannedGame {
        PlannedGame {
            kind: ImportKind::Windows {
                exe: exe.into(),
                args: None,
                working_dir: None,
                prefix: None,
                wine_version: None,
                arch: None,
                env: Default::default(),
                dll_overrides: Vec::new(),
            },
            ..game(id, slug)
        }
    }

    #[test]
    fn only_games_not_seen_before_are_imported() {
        let mut seen = BTreeSet::from(["1:hades".to_string(), "0:bottle/x.exe".to_string()]);
        let none = |_: &PlannedGame| false;
        let left = select(
            &mut seen,
            plan(vec![
                installed(1, "hades", "/g/hades.exe"),
                installed(2, "celeste", "/g/celeste.exe"),
                installed(0, "bottle/x.exe", "/b/x.exe"),
            ]),
            true,
            &none,
        );
        assert_eq!(left.games.len(), 1);
        assert_eq!(left.games[0].slug, "celeste");
        assert!(seen.contains("2:celeste"));
    }

    /// A game imported before syncs were recorded, whose settings the user
    /// changed in Irori since: the first sync leaves it alone, and so does
    /// every later one, while a game new to Irori is imported.
    #[test]
    fn games_already_in_irori_keep_their_changes_through_the_first_sync() {
        let in_library = |g: &PlannedGame| matches!(&g.kind, ImportKind::Windows { exe, .. } if exe == std::path::Path::new("/g/hades.exe"));
        let lutris = || {
            plan(vec![
                installed(1, "hades", "/g/hades.exe"),
                installed(2, "celeste", "/g/celeste.exe"),
            ])
        };
        let mut seen = BTreeSet::new();
        let first = select(&mut seen, lutris(), true, &in_library);
        let imported: Vec<_> = first.games.iter().map(|g| g.slug.as_str()).collect();
        assert_eq!(
            imported,
            ["celeste"],
            "the imported game isn't applied again"
        );
        assert!(seen.contains("1:hades"), "and is noted as seen");
        let again = select(&mut seen, lutris(), true, &|_| false);
        assert!(again.games.is_empty());
        // A whole import still lists every game, and notes them.
        let mut fresh = BTreeSet::new();
        assert_eq!(
            select(&mut fresh, lutris(), false, &in_library).games.len(),
            2
        );
        assert_eq!(fresh.len(), 2);
    }

    #[test]
    fn skipped_games_are_tried_again() {
        let mut seen = BTreeSet::new();
        let left = select(&mut seen, plan(vec![game(3, "later")]), true, &|_| true);
        assert_eq!(
            left.games.len(),
            1,
            "skipped games are never counted as imported"
        );
        assert!(seen.is_empty());
    }

    #[test]
    fn a_switch_turned_on_reads_at_once() {
        let off = LauncherImports::default();
        let lutris = LauncherImports {
            lutris: true,
            ..off
        };
        assert!(turned_on(off, lutris));
        assert!(!turned_on(lutris, lutris));
        assert!(!turned_on(lutris, off));
        assert!(!off.any() && lutris.any());
    }
}
