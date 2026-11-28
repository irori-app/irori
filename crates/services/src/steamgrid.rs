//! Personal SteamGridDB API key for artwork search, kept in
//! `<config>/steamgriddb.json` through `irori_secrets` and never persisted in
//! `settings.json` or logged.
//!
//! Legacy keys saved in `AppSettings::steam_grid_db_api_key` by earlier builds
//! are migrated transparently on first load and removed from `settings.json`.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use irori_core::{Event, Paths};
use irori_extras::steamgrid::ApiKey;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::Backend;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SteamGridSecret {
    api_key: String,
}

#[derive(Default)]
pub(crate) struct SteamGridState {
    /// Serialises secret file reads, migrations, and writes.
    io: tokio::sync::Mutex<()>,
    /// `None` until loaded from disk; then the validated key (if any).
    /// Readable without blocking on the GPUI UI thread.
    cached: RwLock<Option<Option<ApiKey>>>,
}

fn secret_file() -> PathBuf {
    Paths::get().config.join("steamgriddb.json")
}

impl Backend {
    /// Load and return the saved SteamGridDB API key, migrating any legacy key
    /// stored in `settings.json` into `irori_secrets`.
    pub async fn steamgrid_api_key(&self) -> Option<ApiKey> {
        if let Some(loaded) = self.steamgrid.cached.read().clone() {
            return loaded;
        }
        let _io = self.steamgrid.io.lock().await;
        if let Some(loaded) = self.steamgrid.cached.read().clone() {
            return loaded;
        }

        let from_vault =
            tokio::task::spawn_blocking(|| irori_secrets::load::<SteamGridSecret>(&secret_file()))
                .await
                .ok()
                .flatten()
                .and_then(|s| ApiKey::new(&s.api_key));

        let resolved = if let Some(key) = from_vault {
            // Clear any leftover legacy copy from settings.json.
            if !self.settings.app().steam_grid_db_api_key.is_empty() {
                self.settings
                    .update_app(|s| s.steam_grid_db_api_key.clear());
            }
            Some(key)
        } else {
            let legacy_raw = self.settings.app().steam_grid_db_api_key.trim().to_string();
            if let Some(key) = ApiKey::new(&legacy_raw) {
                let to_save = SteamGridSecret {
                    api_key: legacy_raw,
                };
                match tokio::task::spawn_blocking(move || {
                    irori_secrets::write_json(&secret_file(), &to_save)
                })
                .await
                {
                    Ok(Ok(())) => {
                        self.settings
                            .update_app(|s| s.steam_grid_db_api_key.clear());
                    }
                    Ok(Err(e)) => {
                        tracing::warn!("migrating SteamGridDB API key to secrets: {e:#}");
                    }
                    Err(e) => {
                        tracing::warn!("migrating SteamGridDB API key task stopped: {e:#}");
                    }
                }
                Some(key)
            } else {
                None
            }
        };

        *self.steamgrid.cached.write() = Some(resolved.clone());
        resolved
    }

    /// Preload the SteamGridDB key in the background only when a secret file
    /// or legacy key already exists, avoiding unnecessary keyring prompts on
    /// fresh setups.
    pub(crate) async fn preload_steamgrid_key(&self) {
        let has_legacy = !self.settings.app().steam_grid_db_api_key.trim().is_empty();
        let has_file = tokio::task::spawn_blocking(|| secret_file().exists())
            .await
            .unwrap_or(false);
        if has_legacy || has_file {
            let _ = self.steamgrid_api_key().await;
            self.bus.emit(Event::SettingsChanged);
        }
    }

    /// Non-blocking UI accessor for the SteamGridDB API key. Falls back to any
    /// unmigrated legacy `AppSettings` key until background preload completes.
    pub fn cached_steamgrid_api_key(&self) -> Option<ApiKey> {
        match self.steamgrid.cached.read().as_ref() {
            Some(loaded) => loaded.clone(),
            None => ApiKey::from_settings(&self.settings.app()),
        }
    }

    /// Whether a SteamGridDB API key is currently saved (non-blocking).
    pub fn steamgrid_key_saved_cached(&self) -> bool {
        self.cached_steamgrid_api_key().is_some()
    }

    /// Save a personal SteamGridDB API key in `irori_secrets`, or remove it
    /// when `key` is `None` or blank.
    pub async fn set_steamgrid_api_key(&self, key: Option<String>) -> Result<()> {
        let _io = self.steamgrid.io.lock().await;
        let cleaned = key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
        let next = match cleaned {
            Some(raw) => {
                let Some(validated) = ApiKey::new(&raw) else {
                    anyhow::bail!(
                        "Heroic's encrypted SteamGridDB key cannot be reused directly; paste the plain key from SteamGridDB"
                    );
                };
                let to_save = SteamGridSecret { api_key: raw };
                tokio::task::spawn_blocking(move || {
                    irori_secrets::write_json(&secret_file(), &to_save)
                })
                .await
                .context("saving SteamGridDB API key stopped")?
                .context("Could not save the SteamGridDB API key")?;
                Some(validated)
            }
            None => {
                tokio::task::spawn_blocking(|| match std::fs::remove_file(secret_file()) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                })
                .await
                .context("removing SteamGridDB API key stopped")?
                .context("Could not remove the SteamGridDB API key")?;
                None
            }
        };
        if !self.settings.app().steam_grid_db_api_key.is_empty() {
            self.settings
                .update_app(|s| s.steam_grid_db_api_key.clear());
        }
        *self.steamgrid.cached.write() = Some(next);
        self.bus.emit(Event::SettingsChanged);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn saves_migrates_and_clears_steamgrid_key_in_secrets() {
        crate::tests::isolate_paths();
        let backend = Backend::start_for_tests(|_, _| Vec::new()).unwrap();

        // Clean slate before this test (shared process Paths).
        backend.set_steamgrid_api_key(None).await.unwrap();
        assert!(!backend.steamgrid_key_saved_cached());

        // Saving writes only to secrets, never to settings.json.
        backend
            .set_steamgrid_api_key(Some("  sgdb-test-secret-123  ".into()))
            .await
            .unwrap();
        assert_eq!(
            backend.cached_steamgrid_api_key(),
            ApiKey::new("sgdb-test-secret-123")
        );
        assert!(backend.settings.app().steam_grid_db_api_key.is_empty());
        assert!(secret_file().is_file());

        // Heroic's safeStorage-encrypted key is rejected cleanly.
        assert!(
            backend
                .set_steamgrid_api_key(Some("sgdb:v1:encrypted".into()))
                .await
                .is_err()
        );

        // Removing clears the secret file.
        backend.set_steamgrid_api_key(None).await.unwrap();
        assert!(!secret_file().exists());
        assert!(!backend.steamgrid_key_saved_cached());

        // Legacy key in settings.json migrates automatically into secrets.
        backend
            .settings
            .update_app(|s| s.steam_grid_db_api_key = "legacy-sgdb-key".into());
        *backend.steamgrid.cached.write() = None;
        assert_eq!(
            backend.steamgrid_api_key().await,
            ApiKey::new("legacy-sgdb-key")
        );
        assert!(backend.settings.app().steam_grid_db_api_key.is_empty());
        assert!(secret_file().is_file());

        // Cleanup.
        backend.set_steamgrid_api_key(None).await.unwrap();
    }
}
