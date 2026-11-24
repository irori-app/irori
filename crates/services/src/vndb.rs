//! VNDB for the library (`iroriVndb`, off by default): the visual novel
//! of each game, which fills what store pages and IGDB leave out
//! ([`Backend::extra_info`]) and offers its cover in the art picker.
//!
//! Nothing reaches VNDB while the setting is off; a game's novel is asked
//! only when its page (or the art picker, or "Change VNDB match") wants
//! it, then cached on disk (`irori_extras::vndb::novel_for`). Any game may
//! match while the setting is on: only a title VNDB knows does. The
//! user's choice (`iroriVndbId`) wins over the title search.

use std::sync::Arc;

use anyhow::{Result, bail};
use irori_core::{ExtraInfo, GameId, GameInfo};
use irori_extras::vndb::{self, Client, Lookup, VndbNovel};

use crate::Backend;

/// The one VNDB client, so every request keeps to its spacing.
pub(crate) struct VndbState {
    client: parking_lot::Mutex<Arc<Client>>,
}

impl Default for VndbState {
    fn default() -> Self {
        Self {
            client: parking_lot::Mutex::new(Arc::new(Client::new())),
        }
    }
}

impl Backend {
    /// Talk to another server than VNDB's (tests).
    #[cfg(test)]
    pub(crate) fn set_vndb_api(&self, api: &str) {
        *self.vndb.client.lock() = Arc::new(Client::with_api(api, std::time::Duration::ZERO));
    }

    fn vndb_client(&self) -> Option<Arc<Client>> {
        self.settings
            .app()
            .vndb
            .then(|| self.vndb.client.lock().clone())
    }

    fn vndb_lookup(&self, game: &GameInfo) -> Lookup {
        let chosen = self.settings.game(&game.id()).vndb_id;
        Lookup {
            runner: game.runner(),
            app_name: game.app_name.clone(),
            title: game.title.clone(),
            release_year: game
                .release_date
                .as_deref()
                .and_then(|d| d.get(..4))
                .and_then(|y| y.parse().ok()),
            chosen: Some(chosen.trim().to_string()).filter(|c| !c.is_empty()),
        }
    }

    /// VNDB's novel of `game`, while the setting is on: `Ok(None)` when
    /// it's off or VNDB has no such novel.
    pub async fn vndb_novel(&self, game: &GameInfo) -> Result<Option<VndbNovel>> {
        let Some(client) = self.vndb_client() else {
            return Ok(None);
        };
        vndb::novel_for(&client, &vndb::cache_dir(), &self.vndb_lookup(game)).await
    }

    /// VNDB's novels named like `query` (the art picker, "Change VNDB
    /// match").
    pub async fn search_vndb(&self, query: &str) -> Result<Vec<VndbNovel>> {
        let Some(client) = self.vndb_client() else {
            bail!("Turn on VNDB in Settings first");
        };
        client.search(query).await
    }

    /// Use VNDB novel `vndb_id` (`v17`) for `id`; `Some("none")` for none,
    /// `None` to find it by title again.
    pub fn set_vndb_match(&self, id: &GameId, vndb_id: Option<String>) {
        self.settings
            .set_game_value(id, "iroriVndbId", vndb_id.map(serde_json::Value::String));
    }

    /// Fill `info`'s gaps with VNDB's novel of `id`'s game. VNDB being
    /// unreachable never fails the store's page. Developers are added only
    /// when the library names none.
    pub(crate) async fn fill_from_vndb(&self, id: &GameId, info: &mut ExtraInfo) {
        if !self.settings.app().vndb {
            return;
        }
        let Some(game) = self.library().get(id).cloned() else {
            return;
        };
        match self.vndb_novel(&game).await {
            Ok(Some(mut found)) => {
                if game
                    .developer
                    .as_deref()
                    .is_some_and(|d| !d.trim().is_empty())
                {
                    found.developers.clear();
                }
                vndb::fill_extra(info, &found);
            }
            Ok(None) => {}
            Err(e) => tracing::debug!("VNDB lookup for {}: {e:#}", id.key()),
        }
    }
}
