use super::accounts::Credentials;
use super::*;

// Intentionally no Debug implementation: snapshots contain credentials.
pub struct UserTokenSnapshot {
    requested: AccountRole,
    selected: AccountRole,
    epoch: String,
    fallback_epoch: Option<String>,
    credentials: Credentials,
    token: Token,
}
impl UserTokenSnapshot {
    pub fn token(&self) -> Token {
        self.token.clone()
    }
    /// Only an authenticated atomic session can project account metadata.
    pub fn user(&self) -> Option<&User> {
        if self.credentials.authenticated {
            self.credentials.user.as_ref()
        } else {
            None
        }
    }
    /// A legacy display name is not authenticated account metadata.
    pub fn stored_user(&self) -> Option<&User> {
        self.credentials.user.as_ref()
    }
    pub fn role(&self) -> AccountRole {
        self.requested
    }
    pub fn cache_binding(&self) -> String {
        format!(
            "{}:{}:{}",
            if self.requested == AccountRole::License {
                "license"
            } else {
                "xbox"
            },
            self.epoch,
            self.fallback_epoch.as_deref().unwrap_or("")
        )
    }
}
impl TokenManager {
    pub fn user_token_snapshot(&self) -> Result<UserTokenSnapshot, TokenStoreError> {
        self.user_token_snapshot_role(AccountRole::Xbox)
    }
    pub fn user_token_snapshot_role(
        &self,
        role: AccountRole,
    ) -> Result<UserTokenSnapshot, TokenStoreError> {
        let _guard = self.persistent_update()?;
        let state = self.accounts_unlocked()?;
        Self::snapshot_unlocked(&state, role)
    }
    pub fn runtime_identity_snapshots(
        &self,
    ) -> Result<(UserTokenSnapshot, Option<UserTokenSnapshot>), TokenStoreError> {
        let _guard = self.persistent_update()?;
        let state = self.accounts_unlocked()?;
        let xbox = Self::snapshot_unlocked(&state, AccountRole::Xbox)?;
        let store = if state
            .store
            .credentials
            .as_ref()
            .is_some_and(|c| c.authenticated && c.tokens.contains_key(PASSPORT_STS))
        {
            Some(Self::snapshot_unlocked(&state, AccountRole::License)?)
        } else {
            None
        };
        Ok((xbox, store))
    }
    fn snapshot_unlocked(
        state: &accounts::Accounts,
        role: AccountRole,
    ) -> Result<UserTokenSnapshot, TokenStoreError> {
        let (selected, slot) = state.selected(role);
        let c = slot.credentials.as_ref().ok_or(TokenStoreError::NotFound)?;
        let token = c
            .tokens
            .get(PASSPORT_STS)
            .cloned()
            .ok_or(TokenStoreError::NotFound)?;
        Ok(UserTokenSnapshot {
            requested: role,
            selected,
            epoch: slot.epoch.clone(),
            fallback_epoch: if role == AccountRole::Xbox && selected == AccountRole::License {
                Some(state.player.epoch.clone())
            } else {
                None
            },
            credentials: c.clone(),
            token,
        })
    }
    fn current_snapshot<'a>(
        state: &'a accounts::Accounts,
        snapshot: &UserTokenSnapshot,
    ) -> Result<&'a accounts::Credentials, TokenStoreError> {
        let (selected, slot) = state.selected(snapshot.requested);
        if selected != snapshot.selected
            || slot.epoch != snapshot.epoch
            || snapshot
                .fallback_epoch
                .as_ref()
                .is_some_and(|g| g != &state.player.epoch)
        {
            return Err(TokenStoreError::SessionChanged);
        }
        let c = slot
            .credentials
            .as_ref()
            .ok_or(TokenStoreError::SessionChanged)?;
        if c.user.as_ref().map(|u| (&u.puid, &u.username))
            != snapshot
                .credentials
                .user
                .as_ref()
                .map(|u| (&u.puid, &u.username))
        {
            return Err(TokenStoreError::SessionChanged);
        }
        Ok(c)
    }
    pub fn validate_user_token_snapshot(
        &self,
        snapshot: &UserTokenSnapshot,
    ) -> Result<(), TokenStoreError> {
        let _guard = self.persistent_update()?;
        let state = self.accounts_unlocked()?;
        Self::current_snapshot(&state, snapshot)?;
        Ok(())
    }
    /// Refresh only the captured role/session. A newer same-session refresh is
    /// retained; active Store and its saved copy change in one verified value.
    pub fn accept_user_token_refresh(
        &self,
        snapshot: &mut UserTokenSnapshot,
        refreshed: Option<(String, Token)>,
    ) -> Result<(), TokenStoreError> {
        let _guard = self.persistent_update()?;
        let mut state = self.accounts_unlocked()?;
        let current = Self::current_snapshot(&state, snapshot)?;
        if let Some((address, token)) = refreshed
            && serde_json::to_vec(&current.tokens.get(PASSPORT_STS))?
                == serde_json::to_vec(&snapshot.credentials.tokens.get(PASSPORT_STS))?
            && serde_json::to_vec(&current.tokens.get(&address))?
                == serde_json::to_vec(&snapshot.credentials.tokens.get(&address))?
        {
            let slot = state.slot_mut(snapshot.selected);
            let c = slot
                .credentials
                .as_mut()
                .ok_or(TokenStoreError::SessionChanged)?;
            c.tokens.insert(address, token);
            if !c.authenticated {
                slot.epoch = uuid::Uuid::new_v4().to_string();
            }
            if snapshot.selected == AccountRole::License
                && state
                    .store
                    .credentials
                    .as_ref()
                    .is_some_and(|c| c.authenticated)
            {
                state.remember(state.store.credentials.clone().unwrap())?;
            }
            self.write_accounts_unlocked(&state)?;
        }
        *snapshot = Self::snapshot_unlocked(&state, snapshot.requested)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session(manager: &TokenManager, name: &str) {
        manager
            .save_user_session(
                User {
                    puid: name.into(),
                    username: name.into(),
                },
                HashMap::from([(PASSPORT_STS.into(), Token::Compact(name.into()))]),
            )
            .unwrap();
    }
    #[test]
    fn credential_snapshot_rejects_logout_account_switch_and_identical_relogin() {
        for mode in [0, 1, 2] {
            let manager = TokenManager::with_memory();
            session(&manager, "a");
            let mut snapshot = manager.user_token_snapshot().unwrap();
            match mode {
                0 => manager.remove_persistent().unwrap(),
                1 => session(&manager, "b"),
                _ => {
                    manager.remove_persistent().unwrap();
                    session(&manager, "a");
                }
            }
            assert!(matches!(
                manager.accept_user_token_refresh(
                    &mut snapshot,
                    Some((PASSPORT_STS.into(), Token::Compact("stale".into())))
                ),
                Err(TokenStoreError::SessionChanged)
            ));
            assert!(matches!(
                manager.validate_user_token_snapshot(&snapshot),
                Err(TokenStoreError::SessionChanged)
            ));
            assert!(
                !matches!(manager.get_user_token_for(PASSPORT_STS).unwrap(), Some(Token::Compact(value)) if value == "stale")
            );
        }
    }
    #[test]
    fn credential_snapshot_does_not_overwrite_newer_refresh_in_same_session() {
        let manager = TokenManager::with_memory();
        session(&manager, "a");
        let mut early = manager.user_token_snapshot().unwrap();
        let mut late = manager.user_token_snapshot().unwrap();
        manager
            .accept_user_token_refresh(
                &mut early,
                Some((PASSPORT_STS.into(), Token::Compact("newer".into()))),
            )
            .unwrap();
        manager
            .accept_user_token_refresh(
                &mut late,
                Some((PASSPORT_STS.into(), Token::Compact("older".into()))),
            )
            .unwrap();
        assert!(
            matches!(manager.get_user_sts_token().unwrap(), Token::Compact(value) if value == "newer")
        );
        manager.validate_user_token_snapshot(&late).unwrap();
    }
    #[test]
    fn credential_snapshot_preserves_parallel_distinct_addresses_and_checks_ticket_only_replies() {
        let manager = TokenManager::with_memory();
        session(&manager, "a");
        let mut a = manager.user_token_snapshot().unwrap();
        let mut b = manager.user_token_snapshot().unwrap();
        manager
            .accept_user_token_refresh(&mut a, Some(("first".into(), Token::Compact("a".into()))))
            .unwrap();
        manager
            .accept_user_token_refresh(&mut b, Some(("second".into(), Token::Compact("b".into()))))
            .unwrap();
        assert!(manager.get_user_token_for("first").unwrap().is_some());
        assert!(manager.get_user_token_for("second").unwrap().is_some());
        session(&manager, "b");
        assert!(matches!(
            manager.accept_user_token_refresh(&mut a, None),
            Err(TokenStoreError::SessionChanged)
        ));
    }
    #[test]
    fn credential_snapshot_legacy_tombstone_prevents_identical_relogin_aba() {
        let manager = TokenManager::with_memory();
        manager
            .save_user_token(PASSPORT_STS.into(), Token::Compact("same".into()))
            .unwrap();
        let mut snapshot = manager.user_token_snapshot().unwrap();
        manager.remove_persistent().unwrap();
        manager
            .save_user_token(PASSPORT_STS.into(), Token::Compact("same".into()))
            .unwrap();
        assert!(matches!(
            manager.accept_user_token_refresh(&mut snapshot, None),
            Err(TokenStoreError::SessionChanged)
        ));
    }
    #[test]
    fn credential_snapshot_migrates_old_session_once_without_changing_tokens() {
        let backend = Arc::new(MemoryBackend::default());
        backend.set(keys::USER_SESSION, br#"{"user":{"puid":"a","username":"a"},"tokens":{"http://Passport.NET/STS":"original"}}"#).unwrap();
        let manager = TokenManager::new(backend.clone(), Arc::new(MemoryBackend::default()));
        let first = manager.user_token_snapshot().unwrap();
        let bytes = backend.get(keys::USER_SESSION).unwrap();
        let second = manager.user_token_snapshot().unwrap();
        assert_eq!(bytes, backend.get(keys::USER_SESSION).unwrap());
        assert!(matches!(second.token(), Token::Compact(value) if value == "original"));
        manager.validate_user_token_snapshot(&first).unwrap();
    }
    #[test]
    fn credential_snapshot_storage_failures_do_not_publish_or_advance_credentials() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Backend {
            data: MemoryBackend,
            fail: AtomicBool,
        }
        impl TokenBackend for Backend {
            fn get(&self, key: &str) -> Result<Option<Vec<u8>>, TokenStoreError> {
                self.data.get(key)
            }
            fn remove(&self, key: &str) -> Result<(), TokenStoreError> {
                self.data.remove(key)
            }
            fn set(&self, key: &str, value: &[u8]) -> Result<(), TokenStoreError> {
                if self.fail.load(Ordering::SeqCst) {
                    return Err(std::io::Error::other("fixture storage unavailable").into());
                }
                self.data.set(key, value)
            }
        }
        let backend = Arc::new(Backend {
            data: MemoryBackend::default(),
            fail: AtomicBool::new(false),
        });
        let original = br#"{"user":{"puid":"a","username":"a"},"tokens":{"http://Passport.NET/STS":"original"}}"#;
        backend.set(keys::USER_SESSION, original).unwrap();
        let manager = TokenManager::new(backend.clone(), Arc::new(MemoryBackend::default()));
        backend.fail.store(true, Ordering::SeqCst);
        assert!(manager.user_token_snapshot().is_err());
        assert_eq!(backend.get(keys::USER_SESSION).unwrap().unwrap(), original);
        backend.fail.store(false, Ordering::SeqCst);
        let mut snapshot = manager.user_token_snapshot().unwrap();
        backend.fail.store(true, Ordering::SeqCst);
        assert!(
            manager
                .accept_user_token_refresh(
                    &mut snapshot,
                    Some((PASSPORT_STS.into(), Token::Compact("new".into())))
                )
                .is_err()
        );
        assert!(
            matches!(manager.get_user_sts_token().unwrap(), Token::Compact(value) if value == "original")
        );
        backend.fail.store(false, Ordering::SeqCst);
        manager
            .accept_user_token_refresh(
                &mut snapshot,
                Some((PASSPORT_STS.into(), Token::Compact("new".into()))),
            )
            .unwrap();
        assert!(
            matches!(manager.get_user_sts_token().unwrap(), Token::Compact(value) if value == "new")
        );
    }
}
