//! Steam's user tags for Steam games ("Puzzle", "Co-op", "Great
//! Soundtrack"), which the library filters and searches by (`tag:`).
//!
//! Each app's tag ids and the tags' names come from two keyless store
//! endpoints and are kept in `<cache>/steam-tags.json`: an app's tags for a
//! month (they settle once a game has players), the names a week per
//! language. Only apps without a fresh answer are asked for, a hundred at a
//! time, after a library rebuild while online, so a library that knows its
//! tags asks nothing.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use irori_core::{GameInfo, Paths, Runner, json_store, util};
use irori_steam_api::Client;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

const APP_FRESH_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const NAMES_FRESH_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct AppTags {
    ids: Vec<u32>,
    /// Unix time of the answer.
    fetched: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Names {
    names: HashMap<u32, String>,
    fetched: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Cache {
    /// By app id.
    apps: HashMap<u32, AppTags>,
    /// By Steam language name.
    names: HashMap<String, Names>,
}

pub struct SteamTagsService {
    cache: RwLock<Cache>,
    busy: AtomicBool,
}

fn file() -> PathBuf {
    Paths::get().cache.join("steam-tags.json")
}

/// The app id of a Steam game.
fn app_id(game: &GameInfo) -> Option<u32> {
    (game.runner() == Runner::Steam)
        .then(|| game.app_name.parse().ok())
        .flatten()
}

impl SteamTagsService {
    pub fn load() -> Self {
        Self {
            cache: RwLock::new(json_store::read_or_default(&file())),
            busy: AtomicBool::new(false),
        }
    }

    /// Fill in a Steam game's tags, named in `language`, from what is
    /// known. Tags without a name in that language fall back to English.
    pub fn complete(&self, game: &mut GameInfo, language: &str) {
        let Some(id) = app_id(game) else {
            return;
        };
        let cache = self.cache.read();
        let Some(app) = cache.apps.get(&id) else {
            return;
        };
        let local = cache.names.get(language).map(|n| &n.names);
        let english = cache.names.get("english").map(|n| &n.names);
        game.store_tags = app
            .ids
            .iter()
            .filter_map(|tag| {
                local
                    .and_then(|n| n.get(tag))
                    .or_else(|| english.and_then(|n| n.get(tag)))
                    .cloned()
            })
            .collect();
    }

    /// Steam games without a fresh answer.
    fn missing(&self, games: &[std::sync::Arc<GameInfo>], now: u64) -> Vec<u32> {
        let cache = self.cache.read();
        let mut ids: Vec<u32> = games
            .iter()
            .filter_map(|g| app_id(g))
            .filter(|id| {
                cache
                    .apps
                    .get(id)
                    .is_none_or(|a| now.saturating_sub(a.fetched) >= APP_FRESH_FOR.as_secs())
            })
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    fn names_stale(&self, language: &str, now: u64) -> bool {
        self.cache
            .read()
            .names
            .get(language)
            .is_none_or(|n| now.saturating_sub(n.fetched) >= NAMES_FRESH_FOR.as_secs())
    }

    /// Ask Steam for the tags of games without a fresh answer, and for the
    /// names in `language` (and English, the fallback) when they are old.
    /// Returns whether anything was learned. One round runs at a time.
    pub async fn refresh(&self, games: &[std::sync::Arc<GameInfo>], language: &str) -> bool {
        let now = util::unix_now();
        let missing = self.missing(games, now);
        let mut languages = vec![language.to_owned()];
        if language != "english" {
            languages.push("english".into());
        }
        languages.retain(|l| self.names_stale(l, now));
        let has_steam = games.iter().any(|g| app_id(g).is_some());
        if !has_steam
            || (missing.is_empty() && languages.is_empty())
            || self.busy.swap(true, Ordering::AcqRel)
        {
            return false;
        }
        let learned = self.ask(&missing, &languages, now).await;
        if learned {
            let snapshot = self.cache.read().clone();
            let path = file();
            let written =
                tokio::task::spawn_blocking(move || json_store::write(&path, &snapshot)).await;
            if let Ok(Err(e)) | Err(e) = written.map_err(anyhow::Error::from) {
                tracing::warn!("saving Steam tags: {e:#}");
            }
        }
        self.busy.store(false, Ordering::Release);
        learned
    }

    async fn ask(&self, missing: &[u32], languages: &[String], now: u64) -> bool {
        let client = match Client::new() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Steam tags: {e:#}");
                return false;
            }
        };
        let mut learned = false;
        for language in languages {
            match client.tag_names(language).await {
                Ok(names) => {
                    self.cache.write().names.insert(
                        language.clone(),
                        Names {
                            names,
                            fetched: now,
                        },
                    );
                    learned = true;
                }
                Err(e) => tracing::info!("Steam tag names ({language}): {e:#}"),
            }
        }
        if !missing.is_empty() {
            match client.app_tags(missing).await {
                Ok(found) => {
                    let mut cache = self.cache.write();
                    // An app the store doesn't know is an answer too, so it
                    // isn't asked for again before the month is out.
                    for id in missing {
                        let ids = found.get(id).cloned().unwrap_or_default();
                        cache.apps.insert(*id, AppTags { ids, fetched: now });
                    }
                    learned = true;
                }
                Err(e) => tracing::info!("Steam tags: {e:#}"),
            }
        }
        learned
    }

    #[cfg(test)]
    fn seed(&self, app: u32, ids: &[u32], language: &str, names: &[(u32, &str)]) {
        let mut cache = self.cache.write();
        cache.apps.insert(
            app,
            AppTags {
                ids: ids.to_vec(),
                fetched: util::unix_now(),
            },
        );
        cache.names.insert(
            language.into(),
            Names {
                names: names.iter().map(|(k, v)| (*k, (*v).to_owned())).collect(),
                fetched: util::unix_now(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn steam(app: &str) -> GameInfo {
        GameInfo {
            runner: Some(Runner::Steam),
            app_name: app.into(),
            ..Default::default()
        }
    }

    fn service() -> SteamTagsService {
        SteamTagsService {
            cache: RwLock::new(Cache::default()),
            busy: AtomicBool::new(false),
        }
    }

    #[test]
    fn steam_games_get_named_tags_in_order_with_english_fallback() {
        let tags = service();
        tags.seed(
            620,
            &[2, 1, 3],
            "english",
            &[(1, "Puzzle"), (2, "Co-op"), (3, "Funny")],
        );
        tags.cache.write().names.insert(
            "german".into(),
            Names {
                names: [(1, "Rätsel".to_owned())].into(),
                fetched: util::unix_now(),
            },
        );
        let mut game = steam("620");
        tags.complete(&mut game, "german");
        assert_eq!(game.store_tags, ["Co-op", "Rätsel", "Funny"]);

        let mut other = GameInfo {
            runner: Some(Runner::Gog),
            app_name: "620".into(),
            ..Default::default()
        };
        tags.complete(&mut other, "english");
        assert!(other.store_tags.is_empty(), "only Steam games");
    }

    #[test]
    fn only_steam_games_without_a_fresh_answer_are_asked_for() {
        let tags = service();
        tags.seed(620, &[1], "english", &[(1, "Puzzle")]);
        let now = util::unix_now();
        let games = vec![
            Arc::new(steam("620")),
            Arc::new(steam("400")),
            Arc::new(steam("400")),
            Arc::new(steam("not a number")),
            Arc::new(GameInfo {
                runner: Some(Runner::Gog),
                app_name: "1".into(),
                ..Default::default()
            }),
        ];
        assert_eq!(tags.missing(&games, now), [400]);
        assert_eq!(
            tags.missing(&games, now + APP_FRESH_FOR.as_secs()),
            [400, 620]
        );
        assert!(!tags.names_stale("english", now));
        assert!(tags.names_stale("german", now));
    }
}
