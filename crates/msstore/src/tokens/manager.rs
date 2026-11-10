use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::models::secrets::{Device, Token, TokenStore, User};
use crate::models::xbox::XstsResponse;
use crate::tokens::backend::MemoryBackend;
use crate::tokens::store::{ExpiringTokenBackend, TokenBackend, TokenStoreError};

#[path = "user_snapshot.rs"]
mod user_snapshot;
pub use user_snapshot::UserTokenSnapshot;
#[path = "accounts.rs"]
mod accounts;
pub use accounts::{AccountIdentitySnapshot, AccountRole, LoginAttempt};

mod keys {
    pub const DEV_LICENSE: &str = "dev_license";
    pub const DEVICE_TOKENS: &str = "device-tokens";
    pub const USER_TOKENS: &str = "user-tokens";
    pub const USER_INFO: &str = "user-DA";
    pub const USER_SESSION: &str = "user-session";
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct UserSession {
    user: User,
    tokens: HashMap<String, Token>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    generation: Option<String>,
}

pub const PASSPORT_STS: &str = "http://Passport.NET/STS";

#[cfg(test)]
mod credential_account_metadata_tests {
    use super::*;
    #[test]
    fn credential_account_metadata_snapshot_is_atomic_and_logout_removes_it() {
        let manager = TokenManager::with_memory();
        assert!(manager.get_user_sts_session().unwrap().is_none());
        manager
            .save_user(&User {
                puid: "legacy".into(),
                username: "legacy".into(),
            })
            .unwrap();
        manager
            .save_user_token(PASSPORT_STS.into(), Token::Compact("legacy-token".into()))
            .unwrap();
        assert!(manager.get_user_sts_session().unwrap().is_none());
        manager
            .save_user_session(
                User {
                    puid: "0123456789abcdef".into(),
                    username: "synthetic".into(),
                },
                HashMap::from([(
                    PASSPORT_STS.into(),
                    Token::Compact("synthetic-session-ticket".into()),
                )]),
            )
            .unwrap();
        let (user, token) = manager.get_user_sts_session().unwrap().unwrap();
        assert_eq!(user.puid, "0123456789abcdef");
        assert!(matches!(token, Token::Compact(value) if value == "synthetic-session-ticket"));
        manager.remove_persistent().unwrap();
        assert!(manager.get_user_sts_session().unwrap().is_none());
    }
    #[test]
    fn credential_account_metadata_never_pairs_different_sessions_under_concurrent_reads() {
        let manager = TokenManager::with_memory();
        let save = |m: &TokenManager, name: &str| {
            m.save_user_session(
                User {
                    puid: name.into(),
                    username: name.into(),
                },
                HashMap::from([(PASSPORT_STS.into(), Token::Compact(name.into()))]),
            )
            .unwrap()
        };
        save(&manager, "a");
        let writer = manager.clone();
        let thread = std::thread::spawn(move || {
            for _ in 0..1000 {
                save(&writer, "b");
                save(&writer, "a");
            }
        });
        for _ in 0..10000 {
            let (user, token) = manager.get_user_sts_session().unwrap().unwrap();
            assert!(matches!(token, Token::Compact(value) if value == user.puid));
        }
        thread.join().unwrap();
    }
}

/// Semantic facade over the two storage tiers: a persistent, keychain-backed tier
/// for STS/device/user credentials, and an ephemeral tier for short-lived
/// per-relying-party XSTS tokens. Centralizes the read-merge-write pattern that was
/// previously duplicated across `xodus-cli` and `xodus-service`.
#[derive(Clone)]
pub struct TokenManager {
    persistent: Arc<dyn TokenBackend>,
    ephemeral: Arc<dyn ExpiringTokenBackend>,
}

impl TokenManager {
    pub fn new(
        persistent: Arc<dyn TokenBackend>,
        ephemeral: Arc<dyn ExpiringTokenBackend>,
    ) -> Self {
        Self {
            persistent,
            ephemeral,
        }
    }

    /// Memory for both tiers, for tests and one-off use.
    pub fn with_memory() -> Self {
        Self::new(
            Arc::new(MemoryBackend::default()),
            Arc::new(MemoryBackend::default()),
        )
    }

    pub fn remove_persistent(&self) -> Result<(), TokenStoreError> {
        self.revoke_active_and_device(false)
    }

    // ---- Device identity / license -----------------------------------------

    pub fn get_device_license(&self) -> Result<Device, TokenStoreError> {
        let bytes = self
            .persistent
            .get(keys::DEV_LICENSE)?
            .ok_or(TokenStoreError::NotFound)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn save_device_license(&self, device: &Device) -> Result<(), TokenStoreError> {
        let _update = self.persistent_update()?;
        Self::write_verified(
            &*self.persistent,
            keys::DEV_LICENSE,
            &serde_json::to_vec(device)?,
        )
    }

    pub fn remove_device_license(&self) -> Result<(), TokenStoreError> {
        let _update = self.persistent_update()?;
        self.persistent.remove(keys::DEV_LICENSE)
    }

    // ---- Device STS tokens (keyed by SOAP "applies_to" address) -----------

    pub fn get_device_token_for(&self, address: &str) -> Result<Option<Token>, TokenStoreError> {
        Self::read_token_store(&*self.persistent, keys::DEVICE_TOKENS, address)
    }

    pub fn save_device_token(&self, address: String, token: Token) -> Result<(), TokenStoreError> {
        let _update = self.persistent_update()?;
        Self::write_token_store(&*self.persistent, keys::DEVICE_TOKENS, address, token)
    }

    pub fn get_device_sts_token(&self) -> Result<Token, TokenStoreError> {
        self.get_device_token_for(PASSPORT_STS)?
            .ok_or(TokenStoreError::NotFound)
    }

    // ---- User STS tokens (keyed by SOAP "applies_to" address) --------------

    pub fn get_user_token_for(&self, address: &str) -> Result<Option<Token>, TokenStoreError> {
        self.role_token_for(AccountRole::Xbox, address)
    }

    pub fn save_user_token(&self, address: String, token: Token) -> Result<(), TokenStoreError> {
        self.save_role_token(AccountRole::Xbox, address, token)
    }

    pub fn get_user_sts_token(&self) -> Result<Token, TokenStoreError> {
        self.get_user_token_for(PASSPORT_STS)?
            .ok_or(TokenStoreError::NotFound)
    }

    // ---- User info -----------------------------------------------------------

    /// An atomic authenticated user/token pair from the single session record.
    /// Legacy independent entries are not sufficient for identity projection.
    pub fn get_user_sts_session(&self) -> Result<Option<(User, Token)>, TokenStoreError> {
        self.role_session(AccountRole::Xbox)
    }

    pub fn get_user(&self) -> Result<User, TokenStoreError> {
        self.role_user(AccountRole::Xbox)
    }

    pub fn save_user(&self, user: &User) -> Result<(), TokenStoreError> {
        let _guard = self.persistent_update()?;
        let mut s = self.accounts_unlocked()?;
        let slot = &mut s.store;
        let c = slot
            .credentials
            .get_or_insert_with(|| accounts::Credentials {
                user: None,
                tokens: HashMap::new(),
                authenticated: false,
            });
        if c.authenticated {
            return Err(TokenStoreError::SessionChanged);
        }
        c.user = Some(user.clone());
        slot.epoch = uuid::Uuid::new_v4().to_string();
        self.write_accounts_unlocked(&s)
    }

    fn user_session(&self) -> Result<Option<UserSession>, TokenStoreError> {
        self.persistent
            .get(keys::USER_SESSION)?
            .map(|bytes| serde_json::from_slice(&bytes).map_err(Into::into))
            .transpose()
    }

    /// Publish a newly authenticated account and its complete token set using
    /// one keyring value. Never merge tokens belonging to the previous account.
    /// Legacy entries remain readable until the first successful session save.
    pub fn save_user_session(
        &self,
        user: User,
        tokens: HashMap<String, Token>,
    ) -> Result<(), TokenStoreError> {
        self.save_role_session(AccountRole::License, user, tokens)
    }

    // ---- Ephemeral XSTS-by-relying-party cache --------------------------------

    pub fn get_cached_xsts(&self, relying_party: &str) -> Option<XstsResponse> {
        let bytes = self.ephemeral.get(relying_party).ok()??;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn cache_xsts(&self, relying_party: &str, token: &XstsResponse) {
        self.cache_xsts_response(relying_party, token);
    }

    pub fn remove_cached_xsts(&self, key: &str) {
        let _ = self.ephemeral.remove(key);
    }

    fn cache_xsts_response(&self, key: &str, token: &XstsResponse) {
        let Ok(bytes) = serde_json::to_vec(token) else {
            return;
        };
        let remaining = (token.not_after - chrono::Utc::now())
            .to_std()
            .unwrap_or(std::time::Duration::ZERO);
        let _ = self
            .ephemeral
            .set_with_expiry(key, &bytes, Instant::now() + remaining);
    }

    // ---- shared TokenStore read/modify/write helper ---------------------------

    fn persistent_update(
        &self,
    ) -> Result<Box<dyn crate::tokens::store::TokenMutationGuard + '_>, TokenStoreError> {
        self.persistent.mutation_lock()
    }

    fn write_verified(
        backend: &dyn TokenBackend,
        key: &str,
        bytes: &[u8],
    ) -> Result<(), TokenStoreError> {
        backend.set(key, bytes)?;
        match backend.get(key)? {
            Some(saved) if saved == bytes => Ok(()),
            _ => Err(TokenStoreError::WriteVerification),
        }
    }

    fn read_token_store(
        backend: &dyn TokenBackend,
        key: &str,
        address: &str,
    ) -> Result<Option<Token>, TokenStoreError> {
        let Some(bytes) = backend.get(key)? else {
            return Ok(None);
        };
        let store: TokenStore = serde_json::from_slice(&bytes)?;
        Ok(store.tokens.get(address).cloned())
    }

    fn write_token_store(
        backend: &dyn TokenBackend,
        key: &str,
        address: String,
        token: Token,
    ) -> Result<(), TokenStoreError> {
        let mut tokens: HashMap<String, Token> = match backend.get(key)? {
            Some(bytes) if !bytes.is_empty() => {
                match serde_json::from_slice::<TokenStore>(&bytes) {
                    Ok(store) => store.tokens,
                    // Only a freshly authenticated device token may replace its
                    // damaged cache. Never discard device identity or user tokens.
                    Err(_) if key == keys::DEVICE_TOKENS => HashMap::new(),
                    Err(error) => return Err(error.into()),
                }
            }
            _ => HashMap::new(),
        };
        tokens.insert(address, token);
        Self::write_verified(backend, key, &serde_json::to_vec(&TokenStore { tokens })?)
    }
}
