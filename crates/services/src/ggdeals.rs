//! Optional user-supplied GG.deals API key for regional Official vs Keyshops
//! pricing and historical lows, kept in `<config>/ggdeals.json` via
//! `irori_secrets` and never stored in `settings.json` or logged.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use irori_core::Paths;
use serde::{Deserialize, Serialize};

use crate::Backend;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GgDealsSecret {
    api_key: String,
}

#[derive(Default)]
pub(crate) struct GgDealsState {
    /// `None` until loaded from disk; then the trimmed key (if any).
    api_key: tokio::sync::Mutex<Option<Option<String>>>,
}

fn secret_file() -> PathBuf {
    Paths::get().config.join("ggdeals.json")
}

impl Backend {
    /// Load and return the saved GG.deals API key, if any.
    pub async fn ggdeals_api_key(&self) -> Option<String> {
        let mut guard = self.ggdeals.api_key.lock().await;
        if guard.is_none() {
            let loaded =
                tokio::task::spawn_blocking(
                    || irori_secrets::load::<GgDealsSecret>(&secret_file()),
                )
                .await
                .ok()
                .flatten()
                .map(|s| s.api_key.trim().to_string())
                .filter(|k| !k.is_empty());
            *guard = Some(loaded);
        }
        guard.clone().flatten()
    }

    /// Whether a GG.deals API key is currently saved.
    pub async fn ggdeals_key_saved(&self) -> bool {
        self.ggdeals_api_key().await.is_some()
    }

    /// Save a personal GG.deals API key, or with `None`/empty remove the saved one.
    pub async fn set_ggdeals_api_key(&self, key: Option<String>) -> Result<()> {
        let cleaned = key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
        let next = match cleaned {
            Some(api_key) => {
                let to_save = GgDealsSecret {
                    api_key: api_key.clone(),
                };
                tokio::task::spawn_blocking(move || {
                    irori_secrets::write_json(&secret_file(), &to_save)
                })
                .await
                .context("saving GG.deals API key stopped")?
                .context("Could not save the GG.deals API key")?;
                Some(api_key)
            }
            None => {
                tokio::task::spawn_blocking(|| match std::fs::remove_file(secret_file()) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                    _ => Ok(()),
                })
                .await
                .context("removing GG.deals API key stopped")?
                .context("Could not remove the GG.deals API key")?;
                None
            }
        };
        *self.ggdeals.api_key.lock().await = Some(next);
        Ok(())
    }
}
