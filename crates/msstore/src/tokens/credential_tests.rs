//! Synthetic credentials only. Never initializes Secret Service or uses a real account.
use super::{
    backend::MemoryBackend,
    manager::TokenManager,
    store::{TokenBackend, TokenStoreError},
};
use crate::models::secrets::{Device, Token, User};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct FaultyBackend {
    values: Mutex<HashMap<String, Vec<u8>>>,
    fault: Mutex<Option<&'static str>>,
    reads: Mutex<usize>,
    read_error: Mutex<bool>,
}
impl TokenBackend for FaultyBackend {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, TokenStoreError> {
        *self.reads.lock().unwrap() += 1;
        if *self.read_error.lock().unwrap() {
            return Err(std::io::Error::other("synthetic-secret-never-print").into());
        }
        Ok(self.values.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), TokenStoreError> {
        let mut values = self.values.lock().unwrap();
        match *self.fault.lock().unwrap() {
            Some("missing") => {
                values.remove(key);
            }
            Some("empty") => {
                values.insert(key.into(), vec![]);
            }
            Some("different") => {
                values.insert(key.into(), b"synthetic-secret-never-print".to_vec());
            }
            _ => {
                values.insert(key.into(), value.to_vec());
            }
        }
        Ok(())
    }
    fn remove(&self, key: &str) -> Result<(), TokenStoreError> {
        self.values.lock().unwrap().remove(key);
        Ok(())
    }
}
fn setup() -> (Arc<FaultyBackend>, TokenManager) {
    let backend = Arc::new(FaultyBackend::default());
    let manager = TokenManager::new(backend.clone(), Arc::new(MemoryBackend::default()));
    (backend, manager)
}
fn device() -> Device {
    Device {
        puid: "synthetic".into(),
        hwid: "synthetic".into(),
        device_id: "synthetic".into(),
        splicense: "synthetic".into(),
        username: "synthetic".into(),
        password: "synthetic-secret-never-print".into(),
    }
}

fn user(name: &str) -> User {
    User {
        puid: name.into(),
        username: name.into(),
    }
}
fn session_tokens(value: &str) -> HashMap<String, Token> {
    HashMap::from([(super::PASSPORT_STS.into(), Token::Compact(value.into()))])
}
#[test]
fn credential_login_session_is_one_verified_value_and_never_merges_old_accounts_tokens() {
    let (backend, manager) = setup();
    manager.save_user(&user("old")).unwrap();
    manager
        .save_user_token(
            super::PASSPORT_STS.into(),
            Token::Compact("old-token".into()),
        )
        .unwrap();
    let legacy = backend.values.lock().unwrap().clone();
    manager
        .save_user_session(user("new"), session_tokens("new-token"))
        .unwrap();
    assert_eq!(manager.get_user().unwrap().username, "new");
    assert!(
        matches!(manager.get_user_sts_token().unwrap(), Token::Compact(value) if value == "new-token")
    );
    for (key, value) in legacy
        .iter()
        .filter(|(key, _)| key.as_str() != "user-accounts-v2")
    {
        assert_eq!(&backend.values.lock().unwrap()[key], value);
    }
    manager
        .save_user_token("renewed".into(), Token::Compact("renewed-token".into()))
        .unwrap();
    assert_eq!(manager.get_user().unwrap().username, "new");
    assert!(manager.get_user_token_for("renewed").unwrap().is_some());
    assert!(manager.get_user_sts_session().unwrap().is_some());
    manager
        .save_user_session(user("third"), session_tokens("third-token"))
        .unwrap();
    assert!(manager.get_user_token_for("renewed").unwrap().is_none());
    manager.remove_persistent().unwrap();
    assert!(manager.get_user().is_err());
    assert!(manager.get_user_sts_token().is_err());
}
#[test]
fn credential_login_session_readback_failure_never_exposes_mixed_account_and_token() {
    for fault in ["missing", "empty", "different"] {
        let (backend, manager) = setup();
        manager.save_user(&user("old")).unwrap();
        manager
            .save_user_token(
                super::PASSPORT_STS.into(),
                Token::Compact("old-token".into()),
            )
            .unwrap();
        *backend.fault.lock().unwrap() = Some(fault);
        assert!(
            manager
                .save_user_session(user("new"), session_tokens("new-token"))
                .is_err()
        );
        if let Ok(account) = manager.get_user() {
            assert_eq!(account.username, "old");
            assert!(
                matches!(manager.get_user_sts_token().unwrap(), Token::Compact(value) if value == "old-token")
            );
        } else {
            assert!(manager.get_user_sts_token().is_err());
        }
    }
}
#[test]
fn credential_login_session_failed_write_preserves_previous_session_and_empty_set_is_refused() {
    struct RefuseWrite(MemoryBackend, std::sync::atomic::AtomicBool);
    impl TokenBackend for RefuseWrite {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, TokenStoreError> {
            self.0.get(key)
        }
        fn set(&self, key: &str, bytes: &[u8]) -> Result<(), TokenStoreError> {
            if self.1.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(std::io::Error::other("synthetic write failure").into());
            }
            self.0.set(key, bytes)
        }
        fn remove(&self, key: &str) -> Result<(), TokenStoreError> {
            self.0.remove(key)
        }
    }
    let backend = Arc::new(RefuseWrite(MemoryBackend::default(), false.into()));
    let manager = TokenManager::new(backend.clone(), Arc::new(MemoryBackend::default()));
    manager
        .save_user_session(user("old"), session_tokens("old-token"))
        .unwrap();
    backend.1.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        manager
            .save_user_session(user("new"), session_tokens("new-token"))
            .is_err()
    );
    assert!(
        manager
            .save_user_session(user("new"), HashMap::new())
            .is_err()
    );
    assert_eq!(manager.get_user().unwrap().username, "old");
    assert!(
        matches!(manager.get_user_sts_token().unwrap(), Token::Compact(value) if value == "old-token")
    );
}

#[tokio::test]
async fn credential_invalid_device_proof_returns_an_error_before_any_request() {
    use crate::models::{secrets::LegacyToken, soap::Timestamp};
    for secret in [None, Some("AQID".to_owned())] {
        let token = LegacyToken {
            key_name: None,
            token: "synthetic".into(),
            binary_secret: secret,
            tpm_key: None,
            lifetime: Timestamp {
                id: None,
                created: "2000-01-01T00:00:00Z".into(),
                expires: "2099-01-01T00:00:00Z".into(),
            },
        };
        let client = reqwest::Client::new();
        assert!(
            crate::api::live::exchange_device_token(
                &client,
                token.clone(),
                "synthetic".into(),
                "synthetic".into(),
                None
            )
            .await
            .is_err()
        );
        assert!(
            crate::api::live::exchange_user_token(
                &client,
                token.clone(),
                "synthetic".into(),
                token,
                None,
                None,
                "synthetic".into(),
                &[]
            )
            .await
            .is_err()
        );
    }
}

#[test]
fn credential_write_must_verify_device_readback() {
    for fault in ["missing", "empty", "different"] {
        let (backend, manager) = setup();
        *backend.fault.lock().unwrap() = Some(fault);
        assert!(
            manager.save_device_license(&device()).is_err(),
            "silently accepted {fault} keyring readback"
        );
    }
}

#[test]
fn credential_write_must_verify_all_persistent_slots() {
    let (backend, manager) = setup();
    *backend.fault.lock().unwrap() = Some("empty");
    assert!(
        manager
            .save_user(&User {
                puid: "synthetic".into(),
                username: "synthetic".into()
            })
            .is_err()
    );
    assert!(
        manager
            .save_user_token("address".into(), Token::Compact("synthetic".into()))
            .is_err()
    );
    assert!(
        manager
            .save_device_token("address".into(), Token::Compact("synthetic".into()))
            .is_err()
    );
}

#[test]
fn credential_valid_roundtrip_and_merge_preserve_existing_data() {
    let (_, manager) = setup();
    manager.save_device_license(&device()).unwrap();
    assert_eq!(manager.get_device_license().unwrap().device_id, "synthetic");
    manager
        .save_user(&User {
            puid: "synthetic".into(),
            username: "synthetic".into(),
        })
        .unwrap();
    assert_eq!(manager.get_user().unwrap().username, "synthetic");
    for key in ["first", "second"] {
        manager
            .save_device_token(key.into(), Token::Compact(key.into()))
            .unwrap();
    }
    assert!(
        matches!(manager.get_device_token_for("first").unwrap(), Some(Token::Compact(x)) if x == "first")
    );
    assert!(
        matches!(manager.get_device_token_for("second").unwrap(), Some(Token::Compact(x)) if x == "second")
    );
}

#[test]
fn credential_corrupt_device_cache_can_be_replaced_but_identity_and_account_are_preserved() {
    for bytes in [
        b"".as_slice(),
        b"  ",
        b"{",
        b"[]",
        b"null",
        b"synthetic-secret-never-print",
    ] {
        let (backend, manager) = setup();
        manager.save_device_license(&device()).unwrap();
        manager
            .save_user(&User {
                puid: "synthetic".into(),
                username: "synthetic".into(),
            })
            .unwrap();
        backend
            .values
            .lock()
            .unwrap()
            .insert("device-tokens".into(), bytes.to_vec());
        manager
            .save_device_token(
                super::PASSPORT_STS.into(),
                Token::Compact("genuine-response-fixture".into()),
            )
            .unwrap();
        assert!(manager.get_device_sts_token().is_ok());
        assert_eq!(
            manager.get_device_license().unwrap().password,
            "synthetic-secret-never-print"
        );
        assert_eq!(manager.get_user().unwrap().username, "synthetic");
        backend
            .values
            .lock()
            .unwrap()
            .insert("user-accounts-v2".into(), b"{".to_vec());
        assert!(
            manager
                .save_user_token("other".into(), Token::Compact("synthetic".into()))
                .is_err()
        );
        assert_eq!(backend.values.lock().unwrap()["user-accounts-v2"], b"{");
    }
}

#[test]
fn credential_error_messages_never_include_provider_or_json_values() {
    let bad_json = serde_json::from_slice::<crate::models::secrets::TokenStore>(
        b"{\"synthetic-secret-never-print\":null}",
    )
    .unwrap_err();
    for error in [
        TokenStoreError::Serde(bad_json),
        TokenStoreError::Io(std::io::Error::other("synthetic-secret-never-print")),
        TokenStoreError::Storage("synthetic-secret-never-print".into()),
        TokenStoreError::WriteVerification,
        TokenStoreError::NotFound,
        TokenStoreError::DeviceAuthentication,
    ] {
        assert!(!error.to_string().contains("synthetic-secret-never-print"));
        assert!(!error.diagnostic_code().contains("synthetic-secret"));
        assert!(!error.to_string().contains("panicked"));
    }
}

#[tokio::test]
async fn credential_invalid_identity_is_not_overwritten_or_reprovisioned() {
    for bytes in [b"".as_slice(), b" ", b"{", b"[]", b"null"] {
        let (backend, manager) = setup();
        backend
            .values
            .lock()
            .unwrap()
            .insert("dev_license".into(), bytes.to_vec());
        let result =
            super::device::ensure_device_credentials(&reqwest::Client::new(), &manager).await;
        assert!(matches!(result, Err(TokenStoreError::Serde(_))));
        assert_eq!(backend.values.lock().unwrap()["dev_license"], bytes);
        assert_eq!(backend.values.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn credential_unavailable_store_stops_without_writes_or_network() {
    let (backend, manager) = setup();
    *backend.read_error.lock().unwrap() = true;
    assert!(matches!(
        super::device::ensure_device_credentials(&reqwest::Client::new(), &manager).await,
        Err(TokenStoreError::Io(_))
    ));
    assert!(backend.values.lock().unwrap().is_empty());
}

#[tokio::test]
async fn credential_valid_initialization_preserves_identity_and_tokens_without_network() {
    let (backend, manager) = setup();
    manager.save_device_license(&device()).unwrap();
    let token = Token::Legacy(crate::models::secrets::LegacyToken {
        key_name: Some(super::PASSPORT_STS.into()),
        token: "synthetic".into(),
        binary_secret: None,
        tpm_key: None,
        lifetime: crate::models::soap::Timestamp {
            id: None,
            created: "2026-10-18T00:00:00Z".into(),
            expires: "2099-01-01T00:00:00Z".into(),
        },
    });
    manager
        .save_device_token(super::PASSPORT_STS.into(), token)
        .unwrap();
    let before = backend.values.lock().unwrap().clone();
    super::device::ensure_device_credentials(&reqwest::Client::new(), &manager)
        .await
        .unwrap();
    assert_eq!(*backend.values.lock().unwrap(), before);
}
