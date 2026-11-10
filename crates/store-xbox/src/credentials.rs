//! Where the Microsoft account and device credentials live: one file,
//! `credentials.json` in the store's config folder, written only through
//! [`irori_secrets`] (encrypted with the key kept in the system keyring).
//! Xodus keeps the same records as separate system keyring entries.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use irori_msstore_api::tokens::TokenManager;
use irori_msstore_api::tokens::backend::MemoryBackend;
use irori_msstore_api::tokens::store::{TokenBackend, TokenStoreError};
use parking_lot::Mutex;

/// The persistent tier of the token store: each Xodus record under its key,
/// base64 so any bytes survive JSON.
pub struct SecretsBackend {
    path: PathBuf,
    records: Mutex<Option<BTreeMap<String, String>>>,
}

impl SecretsBackend {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            records: Mutex::new(None),
        }
    }

    fn with_records<T>(
        &self,
        change: impl FnOnce(&mut BTreeMap<String, String>) -> (T, bool),
    ) -> Result<T, TokenStoreError> {
        let mut guard = self.records.lock();
        if guard.is_none() {
            let loaded: Option<BTreeMap<String, String>> = irori_secrets::read_json(&self.path)
                .map_err(|e| TokenStoreError::Storage(format!("{e:#}")))?;
            *guard = Some(loaded.unwrap_or_default());
        }
        let records = guard.as_mut().ok_or(TokenStoreError::NotFound)?;
        let (value, changed) = change(records);
        if changed {
            if records.is_empty() {
                match std::fs::remove_file(&self.path) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(TokenStoreError::Io(e)),
                }
            } else {
                irori_secrets::write_json(&self.path, &*records)
                    .map_err(|e| TokenStoreError::Storage(format!("{e:#}")))?;
            }
        }
        Ok(value)
    }
}

impl TokenBackend for SecretsBackend {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, TokenStoreError> {
        let value = self.with_records(|r| (r.get(key).cloned(), false))?;
        value
            .map(|v| STANDARD.decode(v).map_err(|_| TokenStoreError::NotFound))
            .transpose()
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), TokenStoreError> {
        let encoded = STANDARD.encode(value);
        self.with_records(|r| {
            let changed = r.get(key) != Some(&encoded);
            r.insert(key.to_string(), encoded);
            ((), changed)
        })
    }

    fn remove(&self, key: &str) -> Result<(), TokenStoreError> {
        self.with_records(|r| ((), r.remove(key).is_some()))
    }
}

/// The token store over `credentials.json` (persistent) and memory (XSTS
/// tokens and other short-lived ones).
pub fn token_manager(path: PathBuf) -> TokenManager {
    TokenManager::new(
        Arc::new(SecretsBackend::new(path)),
        Arc::new(MemoryBackend::default()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_survive_reopening_and_the_file_goes_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        let backend = SecretsBackend::new(path.clone());
        assert_eq!(backend.get("device").unwrap(), None);
        backend.set("device", &[0, 159, 255]).unwrap();
        assert!(path.is_file());
        let reopened = SecretsBackend::new(path.clone());
        assert_eq!(reopened.get("device").unwrap(), Some(vec![0, 159, 255]));
        reopened.remove("device").unwrap();
        assert!(!path.exists());
    }
}
