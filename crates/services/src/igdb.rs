//! IGDB for the library (`iroriIgdb`, off by default): the user's Twitch
//! application, kept in `<config>/igdb.json` through `irori_secrets`, and
//! the record of each game, which fills what store pages leave out
//! ([`Backend::extra_info`]) and offers art in the art picker.
//!
//! Nothing reaches IGDB while the setting is off or no application is
//! saved; a game's record is asked only when its page (or the art picker)
//! wants it, then cached on disk (`irori_extras::igdb::game_for`). The
//! user's choices win: a match chosen with "Change match" (`iroriIgdbId`),
//! and the title, art and modes set in the Modify dialog, which are
//! overrides applied over everything a store or IGDB says.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use irori_core::{ExtraInfo, GameId, GameInfo, Paths};
use irori_extras::igdb::{self, Client, Credentials, IgdbGame, Lookup, REQUEST_SPACING};

use crate::Backend;

/// The kept Twitch application and the client made from it.
#[derive(Default)]
pub(crate) struct IgdbState {
    /// `None` until read from disk; then the client, if credentials exist.
    client: tokio::sync::Mutex<Option<Option<Arc<Client>>>>,
    /// Twitch's and IGDB's addresses (a test server's in tests).
    endpoints: parking_lot::Mutex<igdb::Endpoints>,
}

/// What the settings panel shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IgdbStatus {
    /// `iroriIgdb` is on.
    pub enabled: bool,
    /// The saved application's client ID (not a secret), if one is saved.
    pub client_id: Option<String>,
}

fn credentials_file() -> PathBuf {
    Paths::get().config.join("igdb.json")
}

impl Backend {
    fn new_igdb_client(&self, credentials: Credentials) -> Arc<Client> {
        let endpoints = self.igdb.endpoints.lock().clone();
        Arc::new(Client::with_endpoints(
            credentials,
            endpoints,
            REQUEST_SPACING,
        ))
    }

    /// Talk to other servers than Twitch's and IGDB's (tests).
    #[cfg(test)]
    pub(crate) fn set_igdb_endpoints(&self, endpoints: igdb::Endpoints) {
        *self.igdb.endpoints.lock() = endpoints;
    }

    /// The client for the saved application, read from disk once.
    async fn igdb_client(&self) -> Option<Arc<Client>> {
        let mut held = self.igdb.client.lock().await;
        if held.is_none() {
            let saved = tokio::task::spawn_blocking(|| {
                irori_secrets::load::<Credentials>(&credentials_file())
            })
            .await
            .ok()
            .flatten();
            *held = Some(saved.map(|c| self.new_igdb_client(c)));
        }
        held.clone().flatten()
    }

    /// The client while `iroriIgdb` is on, else `None`.
    async fn igdb_enabled_client(&self) -> Option<Arc<Client>> {
        if !self.settings.app().igdb {
            return None;
        }
        self.igdb_client().await
    }

    pub async fn igdb_status(&self) -> IgdbStatus {
        IgdbStatus {
            enabled: self.settings.app().igdb,
            client_id: self
                .igdb_client()
                .await
                .map(|c| c.credentials().client_id.clone()),
        }
    }

    /// Save a Twitch application after Twitch accepted it, or with `None`
    /// forget the saved one.
    pub async fn set_igdb_credentials(&self, credentials: Option<Credentials>) -> Result<()> {
        let client = match credentials {
            Some(c) => {
                let client = self.new_igdb_client(c.clone());
                client.verify().await?;
                tokio::task::spawn_blocking(move || {
                    irori_secrets::write_json(&credentials_file(), &c)
                })
                .await
                .context("saving the IGDB application stopped")?
                .context("Could not save the IGDB application")?;
                Some(client)
            }
            None => {
                tokio::task::spawn_blocking(|| match std::fs::remove_file(credentials_file()) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                })
                .await
                .context("forgetting the IGDB application stopped")?
                .context("Could not remove the IGDB application")?;
                None
            }
        };
        *self.igdb.client.lock().await = Some(client);
        Ok(())
    }

    fn igdb_lookup(&self, game: &GameInfo) -> Lookup {
        let chosen = self.settings.game(&game.id()).igdb_id;
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

    /// IGDB's record of `game`, while the setting is on: `Ok(None)` when
    /// it's off, no application is saved, or IGDB has no such game.
    pub async fn igdb_game(&self, game: &GameInfo) -> Result<Option<IgdbGame>> {
        let Some(client) = self.igdb_enabled_client().await else {
            return Ok(None);
        };
        igdb::game_for(&client, &igdb::cache_dir(), &self.igdb_lookup(game)).await
    }

    /// IGDB's games named like `query` (the art picker, "Change match").
    pub async fn search_igdb(&self, query: &str) -> Result<Vec<IgdbGame>> {
        let client = self
            .igdb_enabled_client()
            .await
            .context("Turn on IGDB and save a Twitch application in Settings first")?;
        client.search(query).await
    }

    /// Use IGDB game `igdb_id` for `id`; `Some("none")` for none, `None`
    /// to find it by title again.
    pub fn set_igdb_match(&self, id: &GameId, igdb_id: Option<String>) {
        self.settings
            .set_game_value(id, "iroriIgdbId", igdb_id.map(serde_json::Value::String));
    }

    /// Fill `info`'s gaps with IGDB's record of `id`'s game. IGDB being
    /// unreachable never fails the store's page.
    pub(crate) async fn fill_from_igdb(&self, id: &GameId, info: &mut ExtraInfo) {
        if !self.settings.app().igdb {
            return;
        }
        let Some(game) = self.library().get(id).cloned() else {
            return;
        };
        match self.igdb_game(&game).await {
            Ok(Some(found)) => {
                igdb::fill_extra(info, &found);
            }
            Ok(None) => {}
            Err(e) => tracing::debug!("IGDB lookup for {}: {e:#}", id.key()),
        }
    }
}
