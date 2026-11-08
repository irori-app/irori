//! Importing a Playnite library (Windows): reads it with
//! [`irori_extras::playnite`] (read-only) and applies the plan. Games of
//! stores Irori lists itself are matched to them, by id or else by install
//! folder; every other installed game becomes an added game that Playnite
//! starts (`playnite://playnite/start/<id>`), with its cover and background.
//! Favourites, hidden store games, categories, playtime and last played
//! dates come along. Every game that is not imported is reported with the
//! reason.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use irori_core::{GameId, GameInfo, Runner, Store as _};
use irori_extras::playnite::{self, PlayniteDirs, PlayniteGame, PlayniteKind, PlayniteSkip};
use irori_services::Backend;
use irori_sideload::{SideloadApp, SideloadPlatform, SideloadStore};
use irori_ui::hooks::{LutrisSkip, LutrisSummary};

/// Import the Playnite whose program the `playnite://` link opens.
pub async fn import(
    backend: &Backend,
    sideload: Option<Arc<SideloadStore>>,
) -> Result<LutrisSummary> {
    anyhow::ensure!(
        cfg!(windows),
        "Playnite runs on Windows; there is no Playnite library to import here"
    );
    let dirs = tokio::task::spawn_blocking(PlayniteDirs::discover)
        .await
        .context("looking for Playnite stopped")?
        .context("No Playnite library was found on this computer")?;
    let plan = playnite::plan_async(dirs).await?;
    let mut summary = LutrisSummary {
        launcher: Some("Playnite".into()),
        ..Default::default()
    };
    summary.warnings.extend(plan.warnings);
    let library = backend.library();
    for game in plan.games {
        summary.found += 1;
        apply_game(
            backend,
            sideload.as_deref(),
            &library.games,
            game,
            &mut summary,
        )
        .await;
    }
    backend.refresh_all(false).await;
    Ok(summary)
}

async fn apply_game(
    backend: &Backend,
    sideload: Option<&SideloadStore>,
    library: &[Arc<GameInfo>],
    game: PlayniteGame,
    summary: &mut LutrisSummary,
) {
    let skip = |summary: &mut LutrisSummary, reason: &str, detail: Option<String>| {
        summary.skipped.push(LutrisSkip {
            title: game.name.clone(),
            reason: reason.to_string(),
            detail,
        });
    };
    let id = match &game.kind {
        PlayniteKind::Store {
            runner,
            app_name,
            store,
            account,
        } => {
            let id = GameId::new(*runner, app_name.as_str());
            let found = library
                .iter()
                .any(|g| g.runner() == *runner && g.app_name == *app_name)
                .then_some(id)
                .or_else(|| {
                    game.install_dir
                        .as_deref()
                        .and_then(|dir| same_folder(library, dir))
                });
            match found {
                Some(id) => {
                    summary.matched += 1;
                    id
                }
                None if !game.installed => {
                    skip(summary, PlayniteSkip::NotInstalled.id(), None);
                    return;
                }
                None if *account => {
                    summary.store_games.push((game.name.clone(), store.clone()));
                    return;
                }
                None => {
                    skip(summary, "storeGameMissing", Some(store.clone()));
                    return;
                }
            }
        }
        PlayniteKind::Playnite { link, install_dir } => {
            // A game another store of Irori's already has in this folder
            // (one Playnite added by hand) is matched, not added twice.
            if let Some(id) = same_folder(library, install_dir) {
                summary.matched += 1;
                apply_user_data(backend, &id, &game, summary);
                return;
            }
            let Some(store) = sideload else {
                skip(summary, "unavailable", None);
                return;
            };
            match add_app(store, &game, link, install_dir).await {
                Ok(id) => {
                    summary.imported += 1;
                    id
                }
                Err(e) => {
                    skip(summary, "failed", Some(format!("{e:#}")));
                    return;
                }
            }
        }
        PlayniteKind::Skipped { reason } => {
            let detail = match reason {
                PlayniteSkip::FolderMissing(p) => Some(p.display().to_string()),
                _ => None,
            };
            skip(summary, reason.id(), detail);
            return;
        }
    };
    apply_user_data(backend, &id, &game, summary);
}

/// The installed library game, other than an added one, whose folder is
/// `dir` (letter case and separators aside, as Windows compares them).
fn same_folder(library: &[Arc<GameInfo>], dir: &Path) -> Option<GameId> {
    let key = folder_key(&dir.to_string_lossy())?;
    library
        .iter()
        .filter(|g| g.is_installed && g.runner() != Runner::Sideload)
        .find(|g| {
            g.install
                .as_ref()
                .and_then(|i| folder_key(&i.install_path))
                .is_some_and(|k| k == key)
        })
        .map(|g| g.id())
}

fn folder_key(path: &str) -> Option<String> {
    let key = path
        .trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase();
    // A drive's root alone names no game.
    (key.len() > 3).then_some(key)
}

/// Add the game, or find the one an earlier import added for the same link.
async fn add_app(
    store: &SideloadStore,
    game: &PlayniteGame,
    link: &str,
    install_dir: &Path,
) -> Result<GameId> {
    if let Some(existing) = store.games().into_iter().find(|g| {
        store
            .app(&g.app_name)
            .is_some_and(|a| a.launch_link.as_deref() == Some(link))
    }) {
        return Ok(existing.id());
    }
    store
        .add_app(SideloadApp {
            title: game.name.clone(),
            platform: SideloadPlatform::Windows,
            install_folder: Some(install_dir.to_path_buf()),
            launch_link: Some(link.to_string()),
            cover_art: game.cover.clone(),
            background_art: game.background.clone(),
            ..Default::default()
        })
        .await
}

fn apply_user_data(
    backend: &Backend,
    id: &GameId,
    game: &PlayniteGame,
    summary: &mut LutrisSummary,
) {
    let key = id.key();
    if game.favourite {
        summary.favourites += 1;
    }
    summary.categories += game.categories.len();
    if game.playtime_minutes > 0 {
        summary.playtime_games += 1;
    }
    backend.userdata.update(Some(id), |d| {
        if game.favourite {
            d.favourites.insert(key.clone());
        }
        if game.hidden {
            d.hidden.insert(key.clone());
        }
        for category in &game.categories {
            d.categories
                .entry(category.clone())
                .or_default()
                .insert(key.clone());
        }
        if game.playtime_minutes > 0 || game.last_played.is_some() {
            let rec = d.playtime.entry(key.clone()).or_default();
            // Keep the larger total: playing in both launchers isn't counted twice.
            rec.total_minutes = rec.total_minutes.max(game.playtime_minutes);
            if let Some(last) = &game.last_played
                && rec.last_played.as_deref().is_none_or(|l| l < last.as_str())
            {
                rec.last_played = Some(last.clone());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::{InstallPlatform, InstalledInfo};

    fn installed(runner: Runner, app: &str, path: &str) -> Arc<GameInfo> {
        Arc::new(GameInfo {
            runner: Some(runner),
            app_name: app.into(),
            is_installed: true,
            install: Some(InstalledInfo {
                install_path: path.into(),
                platform: InstallPlatform::Windows,
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    #[test]
    fn games_match_by_their_folder() {
        let library = vec![
            installed(Runner::Sideload, "mine", r"C:\Games\Hades"),
            installed(
                Runner::Launchers,
                "battlenet:pro",
                r"C:\Program Files (x86)\Overwatch\",
            ),
            installed(Runner::Gog, "1", "C:\\"),
        ];
        assert_eq!(
            same_folder(&library, Path::new(r"c:/program files (x86)/overwatch")),
            Some(GameId::new(Runner::Launchers, "battlenet:pro"))
        );
        // Added games and drive roots don't count.
        assert_eq!(same_folder(&library, Path::new(r"C:\Games\Hades")), None);
        assert_eq!(same_folder(&library, Path::new("C:/")), None);
    }
}
