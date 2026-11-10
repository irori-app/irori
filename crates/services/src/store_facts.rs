//! Facts learned from store pages, PCGamingWiki and HowLongToBeat: age
//! ratings, game modes and main-story times. Steam's and GOG's store pages and the wiki page are read only
//! when a game's page opens; what they say is kept here, in
//! `<cache>/store-facts.json`, so the library can filter by it afterwards
//! without asking anyone again in the background. Facts a store's own
//! library data carries (Amazon's and Epic's ratings) need nothing from
//! here, and the user's own game modes (`GameOverrides::game_modes`) win
//! over everything learned.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use irori_core::{AgeRating, GameId, GameInfo, GameMode, Paths, age_rating, game_mode, json_store};
use irori_extras::wiki::{self, StoreIds, WikiInfo};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::{Backend, HltbTimes};

/// What is known of one game.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Learned {
    /// From the store page.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub age_ratings: Vec<AgeRating>,
    /// From the store page.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "game_mode::lenient"
    )]
    pub game_modes: Vec<GameMode>,
    /// From PCGamingWiki, used when the store says nothing.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "game_mode::lenient"
    )]
    pub wiki_modes: Vec<GameMode>,
    /// HowLongToBeat's main-story time in hours, for smart categories.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hltb_main_hours: Option<u32>,
}

pub struct StoreFactsService {
    /// Facts by game key (`runner:app`).
    known: RwLock<HashMap<String, Learned>>,
    /// The main-story times of `known` alone, so rules read one number per
    /// game instead of a whole wiki entry.
    hltb: RwLock<HltbTimes>,
}

fn hltb_of(known: &HashMap<String, Learned>) -> HashMap<String, u32> {
    known
        .iter()
        .filter_map(|(key, l)| Some((key.clone(), l.hltb_main_hours?)))
        .collect()
}

fn file() -> PathBuf {
    Paths::get().cache.join("store-facts.json")
}

/// Where the first version kept age ratings alone (by game key).
fn legacy_file() -> PathBuf {
    Paths::get().cache.join("age-ratings.json")
}

fn load_known() -> HashMap<String, Learned> {
    if let Ok(Some(known)) = json_store::read(&file()) {
        return known;
    }
    let legacy: HashMap<String, Vec<AgeRating>> = json_store::read_or_default(&legacy_file());
    legacy
        .into_iter()
        .map(|(key, age_ratings)| {
            let learned = Learned {
                age_ratings,
                ..Default::default()
            };
            (key, learned)
        })
        .collect()
}

impl StoreFactsService {
    pub fn load() -> Self {
        let known = load_known();
        let hltb = HltbTimes {
            revision: 0,
            hours: Arc::new(hltb_of(&known)),
        };
        Self {
            known: RwLock::new(known),
            hltb: RwLock::new(hltb),
        }
    }

    /// The HowLongToBeat main-story times known so far.
    pub fn hltb_times(&self) -> HltbTimes {
        self.hltb.read().clone()
    }

    /// Add what was learned to a library entry: ratings of boards its own
    /// data doesn't name, and game modes when its data has none (the store
    /// page's, else the wiki's).
    pub fn complete(&self, game: &mut GameInfo) {
        let known = self.known.read();
        let Some(learned) = known.get(&game.id().key()) else {
            return;
        };
        age_rating::merge(&mut game.age_ratings, &learned.age_ratings);
        if game.game_modes.is_empty() {
            game.game_modes = if learned.game_modes.is_empty() {
                learned.wiki_modes.clone()
            } else {
                learned.game_modes.clone()
            };
        }
    }

    /// Change what is known of one game with `f`. Returns whether anything
    /// changed (the library should be rebuilt).
    fn learn(&self, key: String, f: impl FnOnce(&mut Learned)) -> bool {
        let snapshot = {
            let mut known = self.known.write();
            let before = known.get(&key).cloned().unwrap_or_default();
            let mut after = before.clone();
            f(&mut after);
            if after == before {
                return false;
            }
            let hltb_changed = after.hltb_main_hours != before.hltb_main_hours;
            known.insert(key, after);
            if hltb_changed {
                let mut hltb = self.hltb.write();
                *hltb = HltbTimes {
                    revision: hltb.revision + 1,
                    hours: Arc::new(hltb_of(&known)),
                };
            }
            known.clone()
        };
        if let Err(e) = json_store::write(&file(), &snapshot) {
            tracing::warn!("saving facts learned from store pages: {e:#}");
        }
        true
    }

    /// Remember what a store page says. An empty part keeps what was
    /// known: a page that couldn't be read says nothing about the game.
    fn learn_store_page(&self, key: String, ratings: &[AgeRating], modes: &[GameMode]) -> bool {
        if ratings.is_empty() && modes.is_empty() {
            return false;
        }
        self.learn(key, |l| {
            if !ratings.is_empty() {
                l.age_ratings = ratings.to_vec();
            }
            if !modes.is_empty() {
                l.game_modes = game_mode::normalized(modes.to_vec());
            }
        })
    }

    /// Remember HowLongToBeat's main-story time: `None` keeps what was known
    /// (the site may just have been out of reach), unless the user chose
    /// no match (`cleared`).
    fn learn_hltb(&self, key: String, hours: Option<u32>, cleared: bool) -> bool {
        let hours = hours.filter(|h| *h > 0);
        if hours.is_none() && !cleared {
            return false;
        }
        self.learn(key, |l| l.hltb_main_hours = hours)
    }

    /// Remember the game modes PCGamingWiki names.
    fn learn_wiki(&self, key: String, modes: &[GameMode]) -> bool {
        if modes.is_empty() {
            return false;
        }
        self.learn(key, |l| {
            l.wiki_modes = game_mode::normalized(modes.to_vec())
        })
    }
}

impl Backend {
    /// A game's store-page details. Age ratings and game modes in them are
    /// kept, so the library knows them from then on.
    pub async fn extra_info(&self, id: &GameId) -> Result<irori_core::ExtraInfo> {
        let store = self
            .store(id.runner)
            .with_context(|| format!("{} is not available", id.runner.store_name()))?;
        let info = store.extra_info(&id.app_name).await?;
        if self
            .store_facts
            .learn_store_page(id.key(), &info.age_ratings, &info.game_modes)
        {
            self.rebuild_library();
        }
        Ok(info)
    }

    /// What PCGamingWiki, HowLongToBeat, ProtonDB and the others say about a
    /// game ([`irori_extras::wiki::game_info`], cached for a month). The
    /// wiki's game modes are kept, for games whose store names none.
    pub async fn wiki_info(&self, game: &GameInfo) -> Result<Arc<WikiInfo>> {
        let mut ids = StoreIds::of(game);
        let chosen = self.settings.game(&game.id()).hltb_id;
        ids.hltb_id = Some(chosen.trim().to_owned()).filter(|c| !c.is_empty());
        let info = wiki::game_info(&game.title, &ids).await?;
        let modes = info
            .pcgamingwiki
            .as_ref()
            .map(|p| p.game_modes.as_slice())
            .unwrap_or_default();
        let learned_modes = self.store_facts.learn_wiki(game.id().key(), modes);
        let main_story = info.howlongtobeat.as_ref().map(|h| h.main_story);
        let no_match = chosen.trim() == "none";
        let learned_hltb = self
            .store_facts
            .learn_hltb(game.id().key(), main_story, no_match);
        if learned_modes || learned_hltb {
            self.rebuild_library();
        }
        Ok(info)
    }

    /// HowLongToBeat main-story times known so far, for smart categories.
    pub fn hltb_times(&self) -> crate::HltbTimes {
        self.store_facts.hltb_times()
    }

    /// HowLongToBeat's games named like `query`, for "Change match".
    pub async fn search_hltb(&self, query: &str) -> Result<Vec<wiki::HowLongToBeat>> {
        wiki::hltb::search(query).await
    }

    /// Use HowLongToBeat game `hltb_id` for `id`'s times; `Some("none")`
    /// for none, `None` to find one again on its own.
    pub fn set_hltb_match(&self, id: &GameId, hltb_id: Option<String>) {
        self.settings
            .set_game_value(id, "iroriHltbId", hltb_id.map(serde_json::Value::String));
    }

    /// A game's modes as stores and PCGamingWiki tell them, without the
    /// user's own choice: what the Modify dialog starts from.
    pub fn known_game_modes(&self, id: &GameId) -> Vec<GameMode> {
        let Some(game) = self.store(id.runner).and_then(|s| s.game(&id.app_name)) else {
            return Vec::new();
        };
        let mut game = (*game).clone();
        self.store_facts.complete(&mut game);
        game.game_modes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::{RatingSystem, Runner};

    fn service() -> StoreFactsService {
        // Learning writes the facts file.
        crate::tests::isolate_paths();
        StoreFactsService {
            known: RwLock::new(HashMap::new()),
            hltb: RwLock::new(HltbTimes::default()),
        }
    }

    #[test]
    fn main_story_times_are_kept_apart_and_counted_by_revision() {
        let service = service();
        let key = "gog:7".to_string();
        assert!(!service.learn_hltb(key.clone(), None, false));
        assert!(!service.learn_hltb(key.clone(), Some(0), false));
        assert!(service.learn_hltb(key.clone(), Some(12), false));
        let times = service.hltb_times();
        assert_eq!(times.revision, 1);
        assert_eq!(times.hours.get(&key), Some(&12));
        // Out of reach keeps the time; the same time again changes nothing.
        assert!(!service.learn_hltb(key.clone(), None, false));
        assert!(!service.learn_hltb(key.clone(), Some(12), false));
        assert_eq!(service.hltb_times().revision, 1);
        // Other facts leave the times' revision alone.
        assert!(service.learn_wiki(key.clone(), &[GameMode::Single]));
        assert_eq!(service.hltb_times().revision, 1);
        // Choosing no match forgets the time.
        assert!(service.learn_hltb(key.clone(), None, true));
        let times = service.hltb_times();
        assert_eq!(times.revision, 2);
        assert!(times.hours.is_empty());
    }

    #[test]
    fn learned_ratings_fill_in_boards_the_store_data_lacks() {
        let service = service();
        let pegi = AgeRating::new(RatingSystem::Pegi, "16", Vec::new(), None).unwrap();
        let esrb = AgeRating::new(RatingSystem::Esrb, "t", Vec::new(), None).unwrap();
        let key = "steam:220".to_string();
        assert!(!service.learn_store_page(key.clone(), &[], &[]));
        service.known.write().insert(
            key.clone(),
            Learned {
                age_ratings: vec![pegi.clone(), esrb.clone()],
                ..Default::default()
            },
        );
        assert!(!service.learn_store_page(key.clone(), &[pegi.clone(), esrb.clone()], &[]));

        let mut game = GameInfo {
            runner: Some(Runner::Steam),
            app_name: "220".into(),
            age_ratings: vec![AgeRating::new(RatingSystem::Pegi, "18", Vec::new(), None).unwrap()],
            ..Default::default()
        };
        service.complete(&mut game);
        let titles: Vec<String> = game.age_ratings.iter().map(AgeRating::title).collect();
        assert_eq!(titles, ["PEGI 18", "ESRB T"]);

        let mut other = GameInfo {
            runner: Some(Runner::Gog),
            app_name: "1".into(),
            ..Default::default()
        };
        service.complete(&mut other);
        assert!(other.age_ratings.is_empty());
    }

    #[test]
    fn store_page_modes_come_before_the_wiki_s() {
        let service = service();
        let key = "gog:1".to_string();
        let game = || GameInfo {
            runner: Some(Runner::Gog),
            app_name: "1".into(),
            ..Default::default()
        };
        assert!(service.learn_wiki(key.clone(), &[GameMode::Online, GameMode::Single]));
        assert!(!service.learn_wiki(key.clone(), &[GameMode::Single, GameMode::Online]));
        let mut g = game();
        service.complete(&mut g);
        assert_eq!(g.game_modes, [GameMode::Single, GameMode::Online]);

        // The store page's modes win, and an unreadable page keeps them.
        assert!(service.learn_store_page(key.clone(), &[], &[GameMode::Coop]));
        assert!(!service.learn_store_page(key.clone(), &[], &[]));
        let mut g = game();
        service.complete(&mut g);
        assert_eq!(g.game_modes, [GameMode::Coop]);

        // The store's own data wins over anything learned.
        let mut g = GameInfo {
            game_modes: vec![GameMode::Mmo],
            ..game()
        };
        service.complete(&mut g);
        assert_eq!(g.game_modes, [GameMode::Mmo]);

        // The file keeps both and reads back.
        let json = serde_json::to_string(&*service.known.read()).unwrap();
        let back: HashMap<String, Learned> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, *service.known.read());
    }
}
