//! How a game is received on Steam, for its page: the store's review summary
//! and how many people play it now, for any game with a Steam app (its own,
//! or the app it is linked to, [`crate::steam_link`]). Asked only when a
//! game's page shows it, and kept in memory for [`FRESH_FOR`], so a page
//! opened again reads no network and nothing runs in the background.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use irori_core::GameInfo;
use irori_steam_api::ReviewSummary;
use once_cell::sync::Lazy;
use parking_lot::Mutex;

use crate::Backend;

/// How long an answer is shown before it is asked again: the player count
/// changes by the minute, but a page left open needn't follow it.
pub const FRESH_FOR: Duration = Duration::from_secs(10 * 60);

/// What Steam says of a game's reception.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SteamReception {
    /// The Steam app the numbers are of.
    pub app_id: u32,
    /// The store's review summary; `None` without reviews.
    pub reviews: Option<ReviewSummary>,
    /// People playing now; `None` when Steam doesn't count the app.
    pub players: Option<u64>,
}

impl SteamReception {
    /// Whether there is anything to show.
    pub fn is_empty(&self) -> bool {
        self.reviews.as_ref().is_none_or(|r| r.total == 0) && self.players.is_none()
    }
}

/// Answers by app, with when they were asked.
static KNOWN: Lazy<Mutex<HashMap<u32, (Instant, SteamReception)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// The kept answer for `app`, while fresh.
fn kept(app: u32, now: Instant) -> Option<SteamReception> {
    KNOWN
        .lock()
        .get(&app)
        .filter(|(at, _)| now.duration_since(*at) < FRESH_FOR)
        .map(|(_, r)| r.clone())
}

impl Backend {
    /// How `game` is received on Steam: `Ok(None)` for a game without a
    /// Steam app (or a ROM). Either half failing leaves it out; both
    /// failing is an error (Steam out of reach).
    pub async fn steam_reception(&self, game: &GameInfo) -> Result<Option<SteamReception>> {
        if game.runner() == irori_core::Runner::Emulation {
            return Ok(None);
        }
        let Some(app) = self.steam_link(game).app_id() else {
            return Ok(None);
        };
        if let Some(found) = kept(app, Instant::now()) {
            return Ok(Some(found));
        }
        let client = self.steam_links.client()?;
        let (reviews, players) =
            futures::join!(client.review_summary(app), client.current_players(app));
        let reception = match (reviews, players) {
            (Err(e), Err(_)) => return Err(e),
            (reviews, players) => SteamReception {
                app_id: app,
                reviews: reviews.ok().flatten(),
                players: players.ok().flatten(),
            },
        };
        KNOWN
            .lock()
            .insert(app, (Instant::now(), reception.clone()));
        Ok(Some(reception))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_until_there_is_something_to_show() {
        let mut r = SteamReception {
            app_id: 620,
            ..Default::default()
        };
        assert!(r.is_empty());
        r.reviews = Some(ReviewSummary::default());
        assert!(r.is_empty());
        r.players = Some(0);
        assert!(!r.is_empty());
    }

    #[test]
    fn answers_are_kept_a_while() {
        let start = Instant::now();
        let r = SteamReception {
            app_id: 4_000_000_001,
            players: Some(5),
            ..Default::default()
        };
        KNOWN.lock().insert(r.app_id, (start, r.clone()));
        assert_eq!(kept(r.app_id, start), Some(r.clone()));
        assert_eq!(kept(r.app_id, start + FRESH_FOR), None);
    }
}
