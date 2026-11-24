//! Steam's user tags ("Puzzle", "Co-op", "Great Soundtrack") for Steam
//! games and games linked to a Steam app (`steam_link`), which the library
//! filters and searches by (`tag:`), and the game page shows.
//!
//! A game keeps its strongest tags only: at most `iroriSteamTagLimit` (5–20),
//! and none that fewer than a quarter as many players gave as the
//! strongest ([`kept`]), so a stray vote doesn't tag a game.
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

use irori_core::{GameInfo, Paths, json_store, util};
use irori_steam_api::Client;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

const APP_FRESH_FOR: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const NAMES_FRESH_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct AppTags {
    ids: Vec<u32>,
    /// The players who gave each of `ids`; empty in answers kept before
    /// votes were (and every tag then counts).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    votes: Vec<u64>,
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

impl SteamTagsService {
    pub fn load() -> Self {
        Self {
            cache: RwLock::new(json_store::read_or_default(&file())),
            busy: AtomicBool::new(false),
        }
    }

    /// Fill in the tags of a game whose Steam app is `app`, named in
    /// `language`, from what is known: at most `limit` (see [`kept`]). Tags
    /// without a name in that language fall back to English.
    pub fn complete(&self, game: &mut GameInfo, language: &str, app: Option<u32>, limit: u32) {
        let Some(id) = app else {
            return;
        };
        let cache = self.cache.read();
        let Some(app) = cache.apps.get(&id) else {
            return;
        };
        let local = cache.names.get(language).map(|n| &n.names);
        let english = cache.names.get("english").map(|n| &n.names);
        game.store_tags = kept(&app.ids, &app.votes, limit)
            .iter()
            .filter_map(|tag| {
                local
                    .and_then(|n| n.get(tag))
                    .or_else(|| english.and_then(|n| n.get(tag)))
                    .cloned()
            })
            .collect();
    }

    /// Apps without a fresh answer.
    fn missing(&self, apps: &[u32], now: u64) -> Vec<u32> {
        let cache = self.cache.read();
        let mut ids: Vec<u32> = apps
            .iter()
            .copied()
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

    /// Ask Steam for the tags of the library's Steam `apps` without a fresh
    /// answer, and for the names in `language` (and English, the fallback)
    /// when they are old. Returns whether anything was learned. One round
    /// runs at a time.
    pub async fn refresh(&self, apps: &[u32], language: &str) -> bool {
        let now = util::unix_now();
        let missing = self.missing(apps, now);
        let mut languages = vec![language.to_owned()];
        if language != "english" {
            languages.push("english".into());
        }
        languages.retain(|l| self.names_stale(l, now));
        if apps.is_empty()
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
        if irori_core::net::working_offline() {
            return false;
        }
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
                        let tags = found.get(id).map(Vec::as_slice).unwrap_or_default();
                        cache.apps.insert(
                            *id,
                            AppTags {
                                ids: tags.iter().map(|t| t.id).collect(),
                                votes: tags.iter().map(|t| t.votes).collect(),
                                fetched: now,
                            },
                        );
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
                votes: Vec::new(),
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

/// Share of the strongest tag's votes a tag needs to be kept, in percent.
const VOTE_FLOOR_PERCENT: u64 = 25;
/// The fewest and most tags a game keeps (`iroriSteamTagLimit`).
pub const TAG_LIMITS: std::ops::RangeInclusive<u32> = 5..=20;

/// The tags of `ids` (strongest first) a game keeps: at most `limit`
/// (held to [`TAG_LIMITS`]), and, when `votes` are known, only those with
/// at least a quarter of the strongest tag's votes.
pub(crate) fn kept(ids: &[u32], votes: &[u64], limit: u32) -> Vec<u32> {
    let limit = limit.clamp(*TAG_LIMITS.start(), *TAG_LIMITS.end()) as usize;
    let top = votes.iter().copied().max().unwrap_or(0);
    ids.iter()
        .enumerate()
        .filter(|(i, _)| {
            top == 0
                || votes
                    .get(*i)
                    .is_none_or(|v| v * 100 >= top * VOTE_FLOOR_PERCENT)
        })
        .map(|(_, id)| *id)
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;

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
        tags.complete(&mut game, "german", Some(620), 20);
        assert_eq!(game.store_tags, ["Co-op", "Rätsel", "Funny"]);

        // A game of another store gets the tags of the app it is linked to.
        let mut other = GameInfo {
            runner: Some(Runner::Gog),
            app_name: "1".into(),
            ..Default::default()
        };
        tags.complete(&mut other, "english", None, 20);
        assert!(other.store_tags.is_empty(), "not linked");
        tags.complete(&mut other, "english", Some(620), 20);
        assert_eq!(other.store_tags, ["Co-op", "Puzzle", "Funny"]);
    }

    #[test]
    fn only_apps_without_a_fresh_answer_are_asked_for() {
        let tags = service();
        tags.seed(620, &[1], "english", &[(1, "Puzzle")]);
        let now = util::unix_now();
        let apps = [620, 400, 400];
        assert_eq!(tags.missing(&apps, now), [400]);
        assert_eq!(
            tags.missing(&apps, now + APP_FRESH_FOR.as_secs()),
            [400, 620]
        );
        assert!(!tags.names_stale("english", now));
        assert!(tags.names_stale("german", now));
    }

    #[test]
    fn games_keep_their_strongest_tags_only() {
        let ids: Vec<u32> = (1..=30).collect();
        // Without votes, the limit alone, held to 5–20.
        assert_eq!(kept(&ids, &[], 8), (1..=8).collect::<Vec<_>>());
        assert_eq!(kept(&ids, &[], 0).len(), 5);
        assert_eq!(kept(&ids, &[], 99).len(), 20);
        // With votes, tags under a quarter of the strongest's go.
        let votes = [400, 300, 100, 99, 250];
        assert_eq!(kept(&ids[..5], &votes, 20), [1, 2, 3, 5]);
        assert_eq!(kept(&ids[..5], &votes, 5), [1, 2, 3, 5]);
    }
}
