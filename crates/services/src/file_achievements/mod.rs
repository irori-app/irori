//! Achievements that games keep in their own files: the unlocks Steam API
//! emulators (Goldberg, gbe_fork, CODEX and RUNE) record for DRM-free and
//! offline copies, read only and only with `iroriFileAchievements` on.
//!
//! It applies to games Irori starts from their own program ([`applies_to`]:
//! never Steam's, the ROM library's or third-party launchers'). Their
//! Steam app id and files are found by [`discover`]; names, icons and
//! rarity come from Steam's public schema (cached a day, a stale copy
//! standing in offline), else from the game's `steam_settings`, else the
//! unlocks name themselves. [`Backend::achievements`] asks this source
//! before the store, so the answer reaches `state/achievements.json`, the
//! stats page and unlock notices like any other.

pub mod discover;
pub mod formats;

use std::path::{Path, PathBuf};

use anyhow::Result;
use irori_core::achievement_cache::AchievementCache;
use irori_core::util::rfc3339_from_unix;
use irori_core::{Achievement, GameId, HostPlatform, InstallPlatform, Paths, Runner};
use irori_steam_api::achievements::{icon_url, steam_language};
use irori_steam_api::{Client, GameAchievement};
use once_cell::sync::Lazy;

use crate::Backend;
use discover::{GameFiles, HostDirs};
use formats::Unlocks;

/// The per-game setting holding the app id the user chose.
pub const APP_ID_KEY: &str = "iroriSteamAppId";

/// How long Steam's schema of a game is used without asking again.
const SCHEMA_MAX_AGE_SECS: u64 = 24 * 60 * 60;

static CLIENT: Lazy<Option<Client>> = Lazy::new(|| match Client::new() {
    Ok(c) => Some(c),
    Err(e) => {
        tracing::warn!("Steam client for achievements: {e:#}");
        None
    }
});

static SCHEMAS: Lazy<AchievementCache> = Lazy::new(|| {
    AchievementCache::new(
        Paths::get().cache.join("file-achievements.json"),
        SCHEMA_MAX_AGE_SECS,
    )
});

/// Whether games of `runner` can have achievements in their files: those
/// Irori starts from their own program.
pub fn applies_to(runner: Runner) -> bool {
    matches!(
        runner,
        Runner::Sideload
            | Runner::Gog
            | Runner::Itch
            | Runner::Legendary
            | Runner::Nile
            | Runner::Zoom
    )
}

/// Steam's definitions as achievements without unlocks.
pub fn from_schema(app_id: u32, defs: &[GameAchievement]) -> Vec<Achievement> {
    defs.iter()
        .map(|d| Achievement {
            id: d.internal_name.clone(),
            key: d.internal_name.clone(),
            visible: !d.hidden,
            name: d.localized_name.clone(),
            description: d.localized_desc.clone(),
            image_unlocked: icon_url(app_id, &d.icon),
            image_locked: icon_url(app_id, &d.icon_gray),
            rarity: d.player_percent_unlocked,
            ..Default::default()
        })
        .collect()
}

/// `definitions` with the emulator's unlocks applied.
pub fn apply_unlocks(mut definitions: Vec<Achievement>, unlocks: &Unlocks) -> Vec<Achievement> {
    for a in &mut definitions {
        a.unlocked_at = None;
        a.progress = None;
        let Some(u) = discover::unlock_for(unlocks, &a.id) else {
            continue;
        };
        if u.earned {
            a.unlocked_at = Some(
                rfc3339_from_unix(u.time as i64)
                    .filter(|_| u.time > 0)
                    .unwrap_or_default(),
            );
        } else {
            a.progress = u.progress;
        }
    }
    definitions
}

/// The executable Irori starts for a game: the chosen one, else the
/// store's, relative to the install folder unless absolute.
fn executable(install_dir: &Path, chosen: &str, store: &str) -> Option<PathBuf> {
    let exe = if chosen.trim().is_empty() {
        store
    } else {
        chosen
    };
    let exe = exe.trim();
    if exe.is_empty() {
        return None;
    }
    let path = Path::new(exe);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        install_dir.join(path)
    })
}

impl Backend {
    /// A game's achievements from its files, when the setting is on, the
    /// game is one Irori starts itself and its files name a Steam app id
    /// along with an emulator's saves or definitions (or the user chose
    /// the app id). `None` leaves the answer to the store.
    pub(crate) async fn file_achievements(
        &self,
        id: &GameId,
        language: &str,
    ) -> Option<Result<Vec<Achievement>>> {
        if !self.settings.app().file_achievements || !applies_to(id.runner) {
            return None;
        }
        let game = self.game(id)?;
        if !game.is_installed
            || game.is_dlc
            || game.browser_url.is_some()
            || game.third_party_managed_app.is_some()
        {
            return None;
        }
        let install_dir = game.install_path()?;
        let settings = self.settings.game(id);
        let store_exe = game
            .install
            .as_ref()
            .map(|i| i.executable.clone())
            .unwrap_or_default();
        let platform = game
            .installed_platform()
            .unwrap_or(InstallPlatform::Windows);
        let host = HostPlatform::current();
        let prefix = (host.needs_wine(platform))
            .then(|| irori_runtime::prefix_path(&self.settings.app(), &settings, &game));
        let files = GameFiles {
            exe: executable(&install_dir, &settings.target_exe, &store_exe),
            install_dir,
            variables: settings
                .environment_options
                .iter()
                .map(|v| (v.key.clone(), v.value.clone()))
                .collect(),
            chosen_app_id: formats::app_id(&settings.steam_app_id),
            prefix: None,
            windows: platform == InstallPlatform::Windows,
        };
        let steam_lang = steam_language(language);
        let found = tokio::task::spawn_blocking(move || {
            let files = GameFiles {
                prefix: prefix
                    .as_deref()
                    .and_then(crate::ludusavi::wine_prefix_root),
                ..files
            };
            discover::discover(&files, &HostDirs::current(), steam_lang)
        })
        .await
        .ok()??;
        let chosen = found.source == Some(discover::AppIdSource::Chosen);
        if !chosen && !found.emulator_files {
            return None;
        }
        Some(self.file_answer(found, steam_lang).await)
    }

    /// The definitions of `found`'s game (see the module docs) with its
    /// unlocks.
    async fn file_answer(
        &self,
        found: discover::Found,
        steam_lang: &str,
    ) -> Result<Vec<Achievement>> {
        let app = found.app_id.to_string();
        let cached = SCHEMAS.get(&app, steam_lang).await;
        let schema = match cached.as_ref().filter(|c| c.fresh) {
            Some(c) => Some(c.list.clone()),
            None => match self.fetch_schema(found.app_id, steam_lang).await {
                Ok(list) => {
                    SCHEMAS.put(&app, steam_lang, &list).await;
                    Some(list)
                }
                Err(e) => {
                    tracing::debug!("Steam achievements of app {app}: {e:#}");
                    cached.map(|c| c.list)
                }
            },
        };
        let definitions = match schema {
            Some(list) if !list.is_empty() => list,
            _ if !found.definitions.is_empty() => found.definitions,
            _ => discover::bare_definitions(&found.unlocks),
        };
        Ok(apply_unlocks(definitions, &found.unlocks))
    }

    async fn fetch_schema(&self, app_id: u32, steam_lang: &str) -> Result<Vec<Achievement>> {
        let client = CLIENT
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Irori couldn't set up its connection to Steam"))?;
        if !self.is_online() {
            anyhow::bail!("offline");
        }
        let defs = client.game_achievements(None, app_id, steam_lang).await?;
        Ok(from_schema(app_id, &defs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use formats::LocalUnlock;

    #[test]
    fn only_games_irori_starts_itself() {
        for r in [
            Runner::Sideload,
            Runner::Gog,
            Runner::Itch,
            Runner::Legendary,
            Runner::Nile,
            Runner::Zoom,
        ] {
            assert!(applies_to(r), "{r:?}");
        }
        for r in [Runner::Steam, Runner::Emulation, Runner::Launchers] {
            assert!(!applies_to(r), "{r:?}");
        }
    }

    #[test]
    fn merges_the_schema_with_the_unlocks() {
        let defs = vec![
            GameAchievement {
                internal_name: "ACH_WIN".into(),
                localized_name: "Winner".into(),
                icon: "a.jpg".into(),
                icon_gray: "b.jpg".into(),
                player_percent_unlocked: 12.5,
                ..Default::default()
            },
            GameAchievement {
                internal_name: "ACH_GRIND".into(),
                localized_name: "Grinder".into(),
                hidden: true,
                ..Default::default()
            },
            GameAchievement {
                internal_name: "ACH_NONE".into(),
                ..Default::default()
            },
        ];
        let unlocks = Unlocks::from([
            (
                "ach_win".to_string(),
                LocalUnlock {
                    earned: true,
                    time: 1_700_000_000,
                    progress: None,
                },
            ),
            (
                "ACH_GRIND".to_string(),
                LocalUnlock {
                    earned: false,
                    time: 0,
                    progress: Some(0.3),
                },
            ),
            (
                "ACH_UNKNOWN".to_string(),
                LocalUnlock {
                    earned: true,
                    ..Default::default()
                },
            ),
        ]);
        let list = apply_unlocks(from_schema(480, &defs), &unlocks);
        assert_eq!(list.len(), 3);
        assert_eq!(
            list[0].unlocked_at.as_deref(),
            rfc3339_from_unix(1_700_000_000).as_deref()
        );
        assert_eq!(list[0].image_unlocked, icon_url(480, "a.jpg"));
        assert_eq!(list[0].rarity, 12.5);
        assert!(!list[1].is_unlocked());
        assert!(!list[1].visible);
        assert_eq!(list[1].progress, Some(0.3));
        assert!(!list[2].is_unlocked());

        // Without a schema the unlocks name themselves; an unknown time is
        // an unlock without a date.
        let bare = apply_unlocks(discover::bare_definitions(&unlocks), &unlocks);
        assert_eq!(bare.len(), 3);
        let unknown = bare.iter().find(|a| a.id == "ACH_UNKNOWN").unwrap();
        assert_eq!(unknown.unlocked_at.as_deref(), Some(""));
    }

    #[test]
    fn resolves_the_executable() {
        let dir = Path::new("/games/x");
        assert_eq!(
            executable(dir, "", "bin/game.exe"),
            Some(dir.join("bin/game.exe"))
        );
        let chosen = if cfg!(windows) {
            "C:\\other\\run.exe"
        } else {
            "/other/run.exe"
        };
        assert_eq!(
            executable(dir, chosen, "bin/game.exe"),
            Some(PathBuf::from(chosen))
        );
        assert_eq!(executable(dir, " ", ""), None);
    }
}
