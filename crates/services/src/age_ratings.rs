//! Age ratings learned from store pages. Steam's and GOG's ratings come
//! with a game's store page, which is read only when its page opens; they
//! are kept here so the library can filter by them afterwards, without
//! asking the stores again in the background. Ratings a store's own library
//! data carries (Amazon, Epic) need nothing from here.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context as _, Result};
use irori_core::{AgeRating, GameId, GameInfo, Paths, age_rating, json_store};
use parking_lot::RwLock;

use crate::Backend;

pub struct AgeRatingsService {
    /// Ratings by game key (`runner:app`).
    known: RwLock<HashMap<String, Vec<AgeRating>>>,
}

fn file() -> PathBuf {
    Paths::get().cache.join("age-ratings.json")
}

impl AgeRatingsService {
    pub fn load() -> Self {
        Self {
            known: RwLock::new(json_store::read_or_default(&file())),
        }
    }

    /// Add the learned ratings of boards the game's own data doesn't name.
    pub fn complete(&self, game: &mut GameInfo) {
        if let Some(learned) = self.known.read().get(&game.id().key()) {
            age_rating::merge(&mut game.age_ratings, learned);
        }
    }

    /// Remember a game's ratings. Returns whether they changed (the library
    /// should be rebuilt). An empty answer keeps what was known: a store
    /// page that couldn't be read says nothing about the game's ratings.
    fn learn(&self, key: String, ratings: &[AgeRating]) -> bool {
        if ratings.is_empty() {
            return false;
        }
        let snapshot = {
            let mut known = self.known.write();
            if known.get(&key).map(Vec::as_slice) == Some(ratings) {
                return false;
            }
            known.insert(key, ratings.to_vec());
            known.clone()
        };
        if let Err(e) = json_store::write(&file(), &snapshot) {
            tracing::warn!("saving age ratings: {e:#}");
        }
        true
    }
}

impl Backend {
    /// A game's store-page details. Age ratings in them are kept, so the
    /// library knows them from then on.
    pub async fn extra_info(&self, id: &GameId) -> Result<irori_core::ExtraInfo> {
        let store = self
            .store(id.runner)
            .with_context(|| format!("{} is not available", id.runner.store_name()))?;
        let info = store.extra_info(&id.app_name).await?;
        if self.age_ratings.learn(id.key(), &info.age_ratings) {
            self.rebuild_library();
        }
        Ok(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::{RatingSystem, Runner};

    #[test]
    fn learned_ratings_fill_in_boards_the_store_data_lacks() {
        let service = AgeRatingsService {
            known: RwLock::new(HashMap::new()),
        };
        let pegi = AgeRating::new(RatingSystem::Pegi, "16", Vec::new(), None).unwrap();
        let esrb = AgeRating::new(RatingSystem::Esrb, "t", Vec::new(), None).unwrap();
        let key = "steam:220".to_string();
        assert!(!service.learn(key.clone(), &[]));
        service
            .known
            .write()
            .insert(key.clone(), vec![pegi.clone(), esrb.clone()]);
        assert!(!service.learn(key.clone(), &[pegi.clone(), esrb.clone()]));

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
}
