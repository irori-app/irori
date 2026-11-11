use super::*;
use std::collections::BTreeMap;

pub(super) const ACCOUNT_STATE: &str = "user-accounts-v2";
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_SAVED: usize = 64;

/// The license identity never falls back to the player. Xbox falls back only
/// while the player slot is empty, and captures that empty slot's epoch too.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AccountRole {
    License,
    Xbox,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Credentials {
    pub user: Option<User>,
    pub tokens: HashMap<String, Token>,
    pub authenticated: bool,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Slot {
    pub epoch: String,
    pub credentials: Option<Credentials>,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Accounts {
    schema: u32,
    pub remember_accounts: bool,
    pub store: Slot,
    pub player: Slot,
    pub saved: BTreeMap<String, Credentials>,
}
fn epoch() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn invalid() -> TokenStoreError {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "Stored account state is invalid",
    )
    .into()
}
fn valid_user(user: &User) -> bool {
    !user.username.trim().is_empty()
        && user.username.len() <= 512
        && !user.username.chars().any(char::is_control)
        && !user.puid.is_empty()
        && user.puid.len() <= 128
        && !user.puid.chars().any(char::is_control)
}
impl Credentials {
    fn validate(&self) -> Result<(), TokenStoreError> {
        if self.user.as_ref().is_some_and(|u| !valid_user(u))
            || self.tokens.len() > 256
            || self
                .tokens
                .keys()
                .any(|k| k.is_empty() || k.len() > 2048 || k.chars().any(char::is_control))
            || self.authenticated && (self.user.is_none() || self.tokens.is_empty())
        {
            return Err(invalid());
        }
        Ok(())
    }
}
impl Slot {
    fn empty() -> Self {
        Self {
            epoch: epoch(),
            credentials: None,
        }
    }
    fn replace(&mut self, credentials: Option<Credentials>) {
        self.epoch = epoch();
        self.credentials = credentials;
    }
}
impl Accounts {
    fn empty() -> Self {
        Self {
            schema: 2,
            remember_accounts: false,
            store: Slot::empty(),
            player: Slot::empty(),
            saved: BTreeMap::new(),
        }
    }
    fn validate(&self) -> Result<(), TokenStoreError> {
        if self.schema != 2
            || self.saved.len() > MAX_SAVED
            || (!self.remember_accounts && !self.saved.is_empty())
        {
            return Err(invalid());
        }
        for slot in [&self.store, &self.player] {
            if uuid::Uuid::parse_str(&slot.epoch).is_err() {
                return Err(invalid());
            }
            if let Some(c) = &slot.credentials {
                c.validate()?;
            }
        }
        if self
            .player
            .credentials
            .as_ref()
            .is_some_and(|c| !c.authenticated)
        {
            return Err(invalid());
        }
        for (name, c) in &self.saved {
            c.validate()?;
            if !c.authenticated
                || c.user.as_ref().map(|u| u.username.as_str()) != Some(name.as_str())
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
    pub fn selected(&self, role: AccountRole) -> (AccountRole, &Slot) {
        if role == AccountRole::Xbox && self.player.credentials.is_some() {
            (AccountRole::Xbox, &self.player)
        } else {
            (AccountRole::License, &self.store)
        }
    }
    pub fn slot_mut(&mut self, role: AccountRole) -> &mut Slot {
        if role == AccountRole::Xbox {
            &mut self.player
        } else {
            &mut self.store
        }
    }
    pub fn remember(&mut self, c: Credentials) -> Result<(), TokenStoreError> {
        c.validate()?;
        if !self.remember_accounts {
            return Ok(());
        }
        let user = c.user.as_ref().ok_or(TokenStoreError::NotFound)?;
        if self
            .saved
            .get(&user.username)
            .and_then(|c| c.user.as_ref())
            .is_some_and(|u| u.puid != user.puid)
        {
            return Err(TokenStoreError::AccountConflict);
        }
        if !self.saved.contains_key(&user.username) && self.saved.len() >= MAX_SAVED {
            return Err(TokenStoreError::AccountCapacity);
        }
        self.saved.insert(user.username.clone(), c);
        Ok(())
    }
}
// No Debug: the device binding contains serialized credentials.
pub struct LoginAttempt {
    role: AccountRole,
    epoch: String,
    device: Vec<u8>,
}
/// Identity guard for offline operations that require no current network ticket.
pub struct AccountIdentitySnapshot {
    role: AccountRole,
    selected: AccountRole,
    epoch: String,
    player_epoch: Option<String>,
    user: User,
}
impl AccountIdentitySnapshot {
    pub fn user(&self) -> &User {
        &self.user
    }
}

impl TokenManager {
    /// Presence is authoritative, including signed-out state. Corruption never
    /// falls back to retained v1 keys. Migration does not destructively clean v1.
    pub(super) fn accounts_unlocked(&self) -> Result<Accounts, TokenStoreError> {
        if let Some(bytes) = self.persistent.get(ACCOUNT_STATE)? {
            if bytes.len() > MAX_BYTES {
                return Err(invalid());
            }
            let state: Accounts = serde_json::from_slice(&bytes)?;
            state.validate()?;
            return Ok(state);
        }
        let mut state = Accounts::empty();
        if let Some(session) = self.user_session()? {
            let c = Credentials {
                user: Some(session.user),
                tokens: session.tokens,
                authenticated: true,
            };
            c.validate()?;
            state.store.credentials = Some(c.clone());
            state.remember(c)?;
        } else {
            let tokens = self
                .persistent
                .get(keys::USER_TOKENS)?
                .map(|b| serde_json::from_slice::<TokenStore>(&b))
                .transpose()?
                .map(|s| s.tokens)
                .unwrap_or_default();
            let user = self
                .persistent
                .get(keys::USER_INFO)?
                .map(|b| serde_json::from_slice::<User>(&b))
                .transpose()?;
            if user.is_some() || !tokens.is_empty() {
                state.store.credentials = Some(Credentials {
                    user,
                    tokens,
                    authenticated: false,
                });
            }
        }
        // Pure reads of a genuinely empty keyring do not provision a record.
        if state.store.credentials.is_some() {
            self.write_accounts_unlocked(&state)?;
        }
        Ok(state)
    }
    pub(super) fn write_accounts_unlocked(&self, state: &Accounts) -> Result<(), TokenStoreError> {
        state.validate()?;
        let bytes = serde_json::to_vec(state)?;
        if bytes.len() > MAX_BYTES {
            return Err(invalid());
        }
        Self::write_verified(&*self.persistent, ACCOUNT_STATE, &bytes)
    }
    pub fn account_identity_snapshot(
        &self,
        role: AccountRole,
    ) -> Result<AccountIdentitySnapshot, TokenStoreError> {
        let _guard = self.persistent_update()?;
        let state = self.accounts_unlocked()?;
        let (selected, slot) = state.selected(role);
        let user = slot
            .credentials
            .as_ref()
            .and_then(|c| c.user.clone())
            .ok_or(TokenStoreError::NotFound)?;
        Ok(AccountIdentitySnapshot {
            role,
            selected,
            epoch: slot.epoch.clone(),
            player_epoch: if role == AccountRole::Xbox && selected == AccountRole::License {
                Some(state.player.epoch.clone())
            } else {
                None
            },
            user,
        })
    }
    pub fn validate_account_identity(
        &self,
        snapshot: &AccountIdentitySnapshot,
    ) -> Result<(), TokenStoreError> {
        let _guard = self.persistent_update()?;
        let state = self.accounts_unlocked()?;
        let (selected, slot) = state.selected(snapshot.role);
        let user = slot
            .credentials
            .as_ref()
            .and_then(|c| c.user.as_ref())
            .ok_or(TokenStoreError::SessionChanged)?;
        if selected != snapshot.selected
            || slot.epoch != snapshot.epoch
            || snapshot
                .player_epoch
                .as_ref()
                .is_some_and(|e| e != &state.player.epoch)
            || user.puid != snapshot.user.puid
            || user.username != snapshot.user.username
        {
            return Err(TokenStoreError::SessionChanged);
        }
        Ok(())
    }
    pub fn begin_role_login(
        &self,
        role: AccountRole,
    ) -> Result<(LoginAttempt, Token), TokenStoreError> {
        let _guard = self.persistent_update()?;
        let mut s = self.accounts_unlocked()?;
        let device = Self::read_token_store(&*self.persistent, keys::DEVICE_TOKENS, PASSPORT_STS)?
            .ok_or(TokenStoreError::NotFound)?;
        // An explicit login attempt, unlike an empty status read, needs stable
        // persisted absence epochs across the browser/network interval.
        if self.persistent.get(ACCOUNT_STATE)?.is_none() {
            self.write_accounts_unlocked(&s)?;
        }
        let epoch = s.slot_mut(role).epoch.clone();
        Ok((
            LoginAttempt {
                role,
                epoch,
                device: serde_json::to_vec(&device)?,
            },
            device,
        ))
    }
    pub fn complete_role_login(
        &self,
        attempt: &LoginAttempt,
        user: User,
        tokens: HashMap<String, Token>,
    ) -> Result<(), TokenStoreError> {
        let c = Credentials {
            user: Some(user),
            tokens,
            authenticated: true,
        };
        c.validate()?;
        let _guard = self.persistent_update()?;
        let mut s = self.accounts_unlocked()?;
        let device = Self::read_token_store(&*self.persistent, keys::DEVICE_TOKENS, PASSPORT_STS)?
            .ok_or(TokenStoreError::SessionChanged)?;
        if s.slot_mut(attempt.role).epoch != attempt.epoch
            || serde_json::to_vec(&device)? != attempt.device
        {
            return Err(TokenStoreError::SessionChanged);
        }
        if attempt.role == AccountRole::License {
            s.remember(c.clone())?;
        }
        s.slot_mut(attempt.role).replace(Some(c));
        self.write_accounts_unlocked(&s)
    }
    pub fn save_player_session(
        &self,
        user: User,
        tokens: HashMap<String, Token>,
    ) -> Result<(), TokenStoreError> {
        self.save_role_session(AccountRole::Xbox, user, tokens)
    }
    pub(super) fn save_role_session(
        &self,
        role: AccountRole,
        user: User,
        tokens: HashMap<String, Token>,
    ) -> Result<(), TokenStoreError> {
        let c = Credentials {
            user: Some(user),
            tokens,
            authenticated: true,
        };
        c.validate()?;
        let _guard = self.persistent_update()?;
        let mut s = self.accounts_unlocked()?;
        if role == AccountRole::License {
            s.remember(c.clone())?;
        }
        s.slot_mut(role).replace(Some(c));
        self.write_accounts_unlocked(&s)
    }
    // v2 must be durably verified before deleting obsolete credentials. A
    // cleanup error leaves the signed-out tombstone authoritative and reported.
    fn remove_legacy_accounts_unlocked(&self) -> Result<(), TokenStoreError> {
        let mut error = None;
        for key in [keys::USER_SESSION, keys::USER_INFO, keys::USER_TOKENS] {
            let result = self.persistent.remove(key).and_then(|()| {
                if self.persistent.get(key)?.is_some() {
                    Err(TokenStoreError::WriteVerification)
                } else {
                    Ok(())
                }
            });
            if let Err(failure) = result
                && error.is_none()
            {
                error = Some(failure);
            }
        }
        error.map_or(Ok(()), Err)
    }
    pub fn remove_player(&self) -> Result<(), TokenStoreError> {
        self.clear_roles(false, true)
    }
    pub fn remove_license(&self) -> Result<(), TokenStoreError> {
        self.clear_roles(true, false)
    }
    pub(super) fn clear_roles(&self, store: bool, player: bool) -> Result<(), TokenStoreError> {
        let _guard = self.persistent_update()?;
        let mut s = self.accounts_unlocked()?;
        if store {
            s.store.replace(None);
        }
        if player {
            s.player.replace(None);
        }
        self.write_accounts_unlocked(&s)?;
        if store {
            self.remove_legacy_accounts_unlocked()?;
        }
        Ok(())
    }
    /// One coherent projection, including an explicit player without Store sign-in.
    pub fn account_users(&self) -> Result<(Option<User>, Option<User>), TokenStoreError> {
        let _guard = self.persistent_update()?;
        let s = self.accounts_unlocked()?;
        Ok((
            s.store.credentials.and_then(|c| c.user),
            s.player.credentials.and_then(|c| c.user),
        ))
    }
    pub fn saved_account_status(
        &self,
    ) -> Result<(bool, Vec<String>, Option<String>), TokenStoreError> {
        let _guard = self.persistent_update()?;
        let s = self.accounts_unlocked()?;
        Ok((
            s.remember_accounts,
            s.saved.into_keys().collect(),
            s.store.credentials.and_then(|c| c.user).map(|u| u.username),
        ))
    }
    pub(super) fn revoke_active_and_device(
        &self,
        registration: bool,
    ) -> Result<(), TokenStoreError> {
        let _guard = self.persistent_update()?;
        let mut s = self.accounts_unlocked()?;
        s.store.replace(None);
        s.player.replace(None);
        self.write_accounts_unlocked(&s)?;
        let cleanup = self.remove_legacy_accounts_unlocked();
        self.persistent.remove(keys::DEVICE_TOKENS)?;
        if registration {
            self.persistent.remove(keys::DEV_LICENSE)?;
        }
        cleanup
    }
    pub fn remove_all_accounts_and_device(&self) -> Result<(), TokenStoreError> {
        self.revoke_active_and_device(true)
    }
    pub fn enable_saved_accounts(&self) -> Result<(), TokenStoreError> {
        let _guard = self.persistent_update()?;
        let mut s = self.accounts_unlocked()?;
        s.remember_accounts = true;
        if let Some(c) = s.store.credentials.clone().filter(|c| c.authenticated) {
            s.remember(c)?;
        }
        self.write_accounts_unlocked(&s)
    }
    pub fn saved_accounts(&self) -> Result<Vec<String>, TokenStoreError> {
        let _guard = self.persistent_update()?;
        Ok(self.accounts_unlocked()?.saved.into_keys().collect())
    }
    pub fn activate_saved_account(&self, name: &str) -> Result<(), TokenStoreError> {
        let _guard = self.persistent_update()?;
        let mut s = self.accounts_unlocked()?;
        let c = s
            .saved
            .get(name)
            .cloned()
            .ok_or(TokenStoreError::NotFound)?;
        s.store.replace(Some(c));
        self.write_accounts_unlocked(&s)
    }
    pub fn forget_saved_account(&self, name: &str) -> Result<(), TokenStoreError> {
        let _guard = self.persistent_update()?;
        let mut s = self.accounts_unlocked()?;
        if s.saved.remove(name).is_none() {
            return Err(TokenStoreError::NotFound);
        }
        if s.store
            .credentials
            .as_ref()
            .and_then(|c| c.user.as_ref())
            .is_some_and(|u| u.username == name)
        {
            s.store.replace(None);
        }
        self.write_accounts_unlocked(&s)?;
        self.remove_legacy_accounts_unlocked()
    }
    pub fn get_license_user(&self) -> Result<User, TokenStoreError> {
        self.role_user(AccountRole::License)
    }
    pub fn get_player_user(&self) -> Result<Option<User>, TokenStoreError> {
        let _g = self.persistent_update()?;
        Ok(self
            .accounts_unlocked()?
            .player
            .credentials
            .and_then(|c| c.user))
    }
    pub(super) fn role_user(&self, role: AccountRole) -> Result<User, TokenStoreError> {
        let _g = self.persistent_update()?;
        self.accounts_unlocked()?
            .selected(role)
            .1
            .credentials
            .as_ref()
            .and_then(|c| c.user.clone())
            .ok_or(TokenStoreError::NotFound)
    }
    pub fn get_license_token_for(&self, address: &str) -> Result<Option<Token>, TokenStoreError> {
        self.role_token_for(AccountRole::License, address)
    }
    pub(super) fn role_token_for(
        &self,
        role: AccountRole,
        address: &str,
    ) -> Result<Option<Token>, TokenStoreError> {
        let _g = self.persistent_update()?;
        Ok(self
            .accounts_unlocked()?
            .selected(role)
            .1
            .credentials
            .as_ref()
            .and_then(|c| c.tokens.get(address).cloned()))
    }
    pub fn get_license_sts_token(&self) -> Result<Token, TokenStoreError> {
        self.get_license_token_for(PASSPORT_STS)?
            .ok_or(TokenStoreError::NotFound)
    }
    pub fn get_license_sts_session(&self) -> Result<Option<(User, Token)>, TokenStoreError> {
        self.role_session(AccountRole::License)
    }
    pub(super) fn role_session(
        &self,
        role: AccountRole,
    ) -> Result<Option<(User, Token)>, TokenStoreError> {
        let _g = self.persistent_update()?;
        let s = self.accounts_unlocked()?;
        let Some(c) = &s.selected(role).1.credentials else {
            return Ok(None);
        };
        if !c.authenticated {
            return Ok(None);
        };
        Ok(Some((
            c.user.clone().ok_or(TokenStoreError::NotFound)?,
            c.tokens
                .get(PASSPORT_STS)
                .cloned()
                .ok_or(TokenStoreError::NotFound)?,
        )))
    }
    pub fn save_license_token(&self, address: String, token: Token) -> Result<(), TokenStoreError> {
        self.save_role_token(AccountRole::License, address, token)
    }
    pub(super) fn save_role_token(
        &self,
        role: AccountRole,
        address: String,
        token: Token,
    ) -> Result<(), TokenStoreError> {
        let _g = self.persistent_update()?;
        let mut s = self.accounts_unlocked()?;
        let selected = s.selected(role).0;
        let slot = s.slot_mut(selected);
        let c = slot.credentials.get_or_insert_with(|| Credentials {
            user: None,
            tokens: HashMap::new(),
            authenticated: false,
        });
        c.tokens.insert(address, token);
        if !c.authenticated {
            slot.epoch = epoch();
        }
        if selected == AccountRole::License
            && s.store
                .credentials
                .as_ref()
                .is_some_and(|c| c.authenticated)
        {
            s.remember(s.store.credentials.clone().unwrap())?;
        }
        self.write_accounts_unlocked(&s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn user(name: &str) -> User {
        User {
            puid: name.into(),
            username: name.into(),
        }
    }
    fn tickets(name: &str) -> HashMap<String, Token> {
        HashMap::from([(PASSPORT_STS.into(), Token::Compact(name.into()))])
    }
    fn save(m: &TokenManager, name: &str) {
        m.enable_saved_accounts().unwrap();
        m.save_user_session(user(name), tickets(name)).unwrap();
    }
    #[test]
    fn account_state_legacy_cleanup_follows_tombstone_and_failure_never_restores_session() {
        struct Fault {
            data: MemoryBackend,
            fail: std::sync::atomic::AtomicBool,
        }
        impl TokenBackend for Fault {
            fn get(&self, k: &str) -> Result<Option<Vec<u8>>, TokenStoreError> {
                self.data.get(k)
            }
            fn set(&self, k: &str, v: &[u8]) -> Result<(), TokenStoreError> {
                self.data.set(k, v)
            }
            fn remove(&self, k: &str) -> Result<(), TokenStoreError> {
                if k == keys::USER_SESSION && self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(std::io::Error::other("synthetic cleanup failure").into());
                }
                self.data.remove(k)
            }
        }
        let b = Arc::new(Fault {
            data: MemoryBackend::default(),
            fail: false.into(),
        });
        b.set(keys::USER_SESSION,br#"{"user":{"puid":"a","username":"a"},"tokens":{"http://Passport.NET/STS":"legacy"}}"#).unwrap();
        b.set(keys::USER_INFO, br#"{"puid":"a","username":"a"}"#)
            .unwrap();
        b.set(
            keys::USER_TOKENS,
            br#"{"tokens":{"http://Passport.NET/STS":"legacy"}}"#,
        )
        .unwrap();
        let m = TokenManager::new(b.clone(), Arc::new(MemoryBackend::default()));
        assert_eq!(m.get_user().unwrap().username, "a");
        assert!(b.get(keys::USER_SESSION).unwrap().is_some());
        m.save_player_session(user("p"), tickets("p")).unwrap();
        m.remove_player().unwrap();
        assert!(b.get(keys::USER_SESSION).unwrap().is_some());
        b.fail.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(m.remove_license().is_err());
        assert!(m.get_user().is_err());
        assert!(b.get(keys::USER_SESSION).unwrap().is_some());
        assert!(b.get(keys::USER_INFO).unwrap().is_none());
        assert!(b.get(keys::USER_TOKENS).unwrap().is_none());
        b.fail.store(false, std::sync::atomic::Ordering::SeqCst);
        m.remove_license().unwrap();
        assert!(b.get(keys::USER_SESSION).unwrap().is_none());
        assert!(m.saved_accounts().unwrap().is_empty());
    }
    #[test]
    fn account_state_offline_identity_guard_needs_no_ticket_and_rejects_same_name_aba() {
        let m = TokenManager::with_memory();
        m.save_user(&user("a")).unwrap();
        let a = m.account_identity_snapshot(AccountRole::License).unwrap();
        assert!(m.user_token_snapshot_role(AccountRole::License).is_err());
        m.validate_account_identity(&a).unwrap();
        m.remove_license().unwrap();
        m.save_user(&user("a")).unwrap();
        assert!(m.validate_account_identity(&a).is_err());
    }
    #[test]
    fn account_state_uncertain_enable_and_partial_device_failure_are_reconcilable() {
        struct Fault {
            data: MemoryBackend,
            read_fail: std::sync::atomic::AtomicBool,
            arm: std::sync::atomic::AtomicBool,
            remove_fail: std::sync::atomic::AtomicBool,
        }
        impl TokenBackend for Fault {
            fn get(&self, k: &str) -> Result<Option<Vec<u8>>, TokenStoreError> {
                if k == ACCOUNT_STATE
                    && self
                        .read_fail
                        .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    return Err(std::io::Error::other("synthetic verification failure").into());
                }
                self.data.get(k)
            }
            fn set(&self, k: &str, v: &[u8]) -> Result<(), TokenStoreError> {
                self.data.set(k, v)?;
                if k == ACCOUNT_STATE && self.arm.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    self.read_fail
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
                Ok(())
            }
            fn remove(&self, k: &str) -> Result<(), TokenStoreError> {
                if self.remove_fail.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(std::io::Error::other("synthetic device removal failure").into());
                }
                self.data.remove(k)
            }
        }
        let b = Arc::new(Fault {
            data: MemoryBackend::default(),
            read_fail: false.into(),
            arm: false.into(),
            remove_fail: false.into(),
        });
        let m = TokenManager::new(b.clone(), Arc::new(MemoryBackend::default()));
        m.save_user_session(user("a"), tickets("a")).unwrap();
        b.arm.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(m.enable_saved_accounts().is_err());
        let (enabled, names, active) = m.saved_account_status().unwrap();
        assert!(enabled);
        assert_eq!(names, vec!["a"]);
        assert_eq!(active.as_deref(), Some("a"));
        m.save_player_session(user("p"), tickets("p")).unwrap();
        let captured = m.user_token_snapshot().unwrap();
        b.remove_fail
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(m.remove_all_accounts_and_device().is_err());
        assert!(m.get_user().is_err());
        assert!(m.get_license_user().is_err());
        assert!(m.validate_user_token_snapshot(&captured).is_err());
        assert_eq!(m.saved_accounts().unwrap(), vec!["a"]);
    }
    #[test]
    fn account_state_partial_user_write_cannot_relabel_authenticated_ticket() {
        let b = Arc::new(MemoryBackend::default());
        let m = TokenManager::new(b.clone(), Arc::new(MemoryBackend::default()));
        save(&m, "a");
        let before = b.get(ACCOUNT_STATE).unwrap();
        assert!(m.save_user(&user("b")).is_err());
        assert_eq!(b.get(ACCOUNT_STATE).unwrap(), before);
        let (u, t) = m.get_user_sts_session().unwrap().unwrap();
        assert_eq!(u.username, "a");
        assert!(matches!(t,Token::Compact(s) if s=="a"));
    }
    #[test]
    fn account_state_retention_requires_explicit_enable_and_collapse_does_not_clear() {
        let m = TokenManager::with_memory();
        m.save_user_session(user("ordinary"), tickets("ordinary"))
            .unwrap();
        assert!(!m.saved_account_status().unwrap().0);
        assert!(m.saved_accounts().unwrap().is_empty());
        let mut captured = m.user_token_snapshot().unwrap();
        m.accept_user_token_refresh(
            &mut captured,
            Some((PASSPORT_STS.into(), Token::Compact("new".into()))),
        )
        .unwrap();
        assert!(m.saved_accounts().unwrap().is_empty());
        m.remove_all_accounts_and_device().unwrap();
        assert!(m.saved_accounts().unwrap().is_empty());
        m.save_user_session(user("retained"), tickets("retained"))
            .unwrap();
        m.enable_saved_accounts().unwrap();
        m.remove_all_accounts_and_device().unwrap();
        assert_eq!(m.saved_accounts().unwrap(), vec!["retained"]);
        assert!(m.saved_account_status().unwrap().0);
        m.activate_saved_account("retained").unwrap();
        assert_eq!(m.get_user().unwrap().username, "retained");
    }
    #[test]
    fn account_state_role_isolation_and_fallback_aba() {
        let m = TokenManager::with_memory();
        save(&m, "store-a");
        let license = m.user_token_snapshot_role(AccountRole::License).unwrap();
        let fallback = m.user_token_snapshot().unwrap();
        m.save_player_session(user("player"), tickets("player"))
            .unwrap();
        m.remove_player().unwrap();
        assert!(m.validate_user_token_snapshot(&fallback).is_err());
        m.validate_user_token_snapshot(&license).unwrap();
        m.save_player_session(user("player"), tickets("player"))
            .unwrap();
        let player = m.user_token_snapshot().unwrap();
        save(&m, "store-b");
        m.validate_user_token_snapshot(&player).unwrap();
        assert!(m.validate_user_token_snapshot(&license).is_err());
        m.activate_saved_account("store-a").unwrap();
        assert!(m.validate_user_token_snapshot(&license).is_err());
        assert_eq!(m.get_user().unwrap().username, "player");
        assert_eq!(m.get_license_user().unwrap().username, "store-a");
    }
    #[test]
    fn account_state_refresh_updates_only_same_role_active_session_and_vault() {
        let m = TokenManager::with_memory();
        save(&m, "a");
        m.save_player_session(user("p"), tickets("p")).unwrap();
        let mut license = m.user_token_snapshot_role(AccountRole::License).unwrap();
        let mut player = m.user_token_snapshot().unwrap();
        m.accept_user_token_refresh(
            &mut license,
            Some((PASSPORT_STS.into(), Token::Compact("new-a".into()))),
        )
        .unwrap();
        m.accept_user_token_refresh(
            &mut player,
            Some((PASSPORT_STS.into(), Token::Compact("new-p".into()))),
        )
        .unwrap();
        save(&m, "b");
        assert!(
            m.accept_user_token_refresh(
                &mut license,
                Some((PASSPORT_STS.into(), Token::Compact("stale-a".into())))
            )
            .is_err()
        );
        m.activate_saved_account("a").unwrap();
        assert!(matches!(m.get_license_sts_token().unwrap(),Token::Compact(s) if s=="new-a"));
        assert!(matches!(m.get_user_sts_token().unwrap(),Token::Compact(s) if s=="new-p"));
    }
    #[test]
    fn account_state_signed_out_tombstone_never_revives_retained_v1() {
        let b = Arc::new(MemoryBackend::default());
        b.set(keys::USER_SESSION,br#"{"user":{"puid":"a","username":"a"},"tokens":{"http://Passport.NET/STS":"legacy"}}"#).unwrap();
        let m = TokenManager::new(b.clone(), Arc::new(MemoryBackend::default()));
        assert_eq!(m.get_license_user().unwrap().username, "a");
        let stable = b.get(ACCOUNT_STATE).unwrap().unwrap();
        let first = m.user_token_snapshot_role(AccountRole::License).unwrap();
        let _ = m.user_token_snapshot_role(AccountRole::License).unwrap();
        assert_eq!(b.get(ACCOUNT_STATE).unwrap().unwrap(), stable);
        m.validate_user_token_snapshot(&first).unwrap();
        m.enable_saved_accounts().unwrap();
        m.save_player_session(user("p"), tickets("p")).unwrap();
        m.remove_license().unwrap();
        assert!(m.get_license_user().is_err());
        assert_eq!(m.get_user().unwrap().username, "p");
        assert!(b.get(keys::USER_SESSION).unwrap().is_none());
        m.remove_player().unwrap();
        assert!(m.get_user().is_err());
        assert_eq!(m.saved_accounts().unwrap(), vec!["a"]);
        m.activate_saved_account("a").unwrap();
        m.forget_saved_account("a").unwrap();
        assert!(m.get_license_user().is_err());
        assert!(m.saved_accounts().unwrap().is_empty());
    }
    #[test]
    fn account_state_empty_reads_do_not_provision_and_corrupt_v2_never_falls_back() {
        let b = Arc::new(MemoryBackend::default());
        let m = TokenManager::new(b.clone(), Arc::new(MemoryBackend::default()));
        assert!(m.get_user().is_err());
        assert!(m.saved_accounts().unwrap().is_empty());
        assert!(b.get(ACCOUNT_STATE).unwrap().is_none());
        b.set(keys::USER_SESSION,br#"{"user":{"puid":"a","username":"a"},"tokens":{"http://Passport.NET/STS":"legacy"}}"#).unwrap();
        for bad in [b"".as_slice(), b"null", b"{}", b"not json"] {
            b.set(ACCOUNT_STATE, bad).unwrap();
            assert!(m.get_license_user().is_err());
            assert!(m.saved_accounts().is_err());
        }
    }
    #[test]
    fn account_state_failed_atomic_write_preserves_both_roles_and_vault() {
        struct Fault {
            data: MemoryBackend,
            fail: std::sync::atomic::AtomicBool,
        }
        impl TokenBackend for Fault {
            fn get(&self, k: &str) -> Result<Option<Vec<u8>>, TokenStoreError> {
                self.data.get(k)
            }
            fn set(&self, k: &str, v: &[u8]) -> Result<(), TokenStoreError> {
                if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(std::io::Error::other("synthetic write failure").into());
                }
                self.data.set(k, v)
            }
            fn remove(&self, k: &str) -> Result<(), TokenStoreError> {
                self.data.remove(k)
            }
        }
        let b = Arc::new(Fault {
            data: MemoryBackend::default(),
            fail: false.into(),
        });
        let m = TokenManager::new(b.clone(), Arc::new(MemoryBackend::default()));
        save(&m, "a");
        m.save_player_session(user("p"), tickets("p")).unwrap();
        let before = b.get(ACCOUNT_STATE).unwrap();
        b.fail.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(m.save_user_session(user("b"), tickets("b")).is_err());
        assert!(m.activate_saved_account("a").is_err());
        assert!(m.forget_saved_account("a").is_err());
        assert_eq!(b.get(ACCOUNT_STATE).unwrap(), before);
    }
    #[test]
    fn account_state_username_collision_rejects_without_overwriting_saved_identity() {
        let m = TokenManager::with_memory();
        save(&m, "same");
        let s = m.user_token_snapshot_role(AccountRole::License).unwrap();
        assert!(
            m.save_user_session(
                User {
                    puid: "different".into(),
                    username: "same".into()
                },
                tickets("other")
            )
            .is_err()
        );
        m.validate_user_token_snapshot(&s).unwrap();
    }
}

#[cfg(test)]
mod login_tests {
    use super::*;
    fn creds(n: &str) -> (User, HashMap<String, Token>) {
        (
            User {
                username: n.into(),
                puid: n.into(),
            },
            HashMap::from([(PASSPORT_STS.into(), Token::Compact(n.into()))]),
        )
    }
    #[test]
    fn account_state_pending_login_is_role_bound_and_device_revocation_wins() {
        let m = TokenManager::with_memory();
        m.save_device_token(PASSPORT_STS.into(), Token::Compact("device".into()))
            .unwrap();
        let (a, _) = m.begin_role_login(AccountRole::Xbox).unwrap();
        let (u, t) = creds("store");
        m.save_user_session(u, t).unwrap();
        let (u, t) = creds("player");
        m.complete_role_login(&a, u, t).unwrap();
        let (a, _) = m.begin_role_login(AccountRole::License).unwrap();
        m.remove_license().unwrap();
        let (u, t) = creds("stale");
        assert!(m.complete_role_login(&a, u, t).is_err());
        let (a, _) = m.begin_role_login(AccountRole::Xbox).unwrap();
        m.remove_all_accounts_and_device().unwrap();
        let (u, t) = creds("stale");
        assert!(m.complete_role_login(&a, u, t).is_err());
        assert!(m.get_user().is_err());
    }
    #[test]
    fn account_state_malformed_player_cannot_become_effective_identity() {
        let b = Arc::new(MemoryBackend::default());
        let m = TokenManager::new(b.clone(), Arc::new(MemoryBackend::default()));
        let (u, t) = creds("a");
        m.save_user_session(u, t).unwrap();
        let mut s = m.accounts_unlocked().unwrap();
        s.player.credentials = Some(Credentials {
            user: None,
            tokens: HashMap::new(),
            authenticated: false,
        });
        b.set(ACCOUNT_STATE, &serde_json::to_vec(&s).unwrap())
            .unwrap();
        assert!(m.get_user().is_err());
        assert!(m.get_license_user().is_err());
    }
}
