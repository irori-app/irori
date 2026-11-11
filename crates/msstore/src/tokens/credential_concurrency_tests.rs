//! Synthetic, deliberately interleaved storage access. No keyring or account.
use super::{
    PASSPORT_STS,
    backend::MemoryBackend,
    manager::TokenManager,
    store::{TokenBackend, TokenStoreError},
};
use crate::models::secrets::{Token, User};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

struct Pause {
    key: &'static str,
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}
#[derive(Default)]
struct PausedBackend {
    data: MemoryBackend,
    pause: Mutex<Option<Pause>>,
    fail_write: AtomicBool,
}
impl TokenBackend for PausedBackend {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, TokenStoreError> {
        let value = self.data.get(key)?;
        let pause = {
            let mut pending = self.pause.lock().unwrap();
            if pending.as_ref().is_some_and(|p| p.key == key) {
                pending.take()
            } else {
                None
            }
        };
        if let Some(pause) = pause {
            pause.entered.send(()).unwrap();
            pause.release.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        Ok(value)
    }
    fn set(&self, key: &str, bytes: &[u8]) -> Result<(), TokenStoreError> {
        if self.fail_write.swap(false, Ordering::SeqCst) {
            return Err(std::io::Error::other("synthetic storage failure").into());
        }
        self.data.set(key, bytes)
    }
    fn remove(&self, key: &str) -> Result<(), TokenStoreError> {
        self.data.remove(key)
    }
}
fn token(value: &str) -> Token {
    Token::Compact(value.into())
}
fn session(manager: &TokenManager, name: &str) {
    manager
        .save_user_session(
            User {
                puid: name.into(),
                username: name.into(),
            },
            HashMap::from([(PASSPORT_STS.into(), token(name))]),
        )
        .unwrap();
}

// Hold the first writer after it has copied the old record. The second writer
// must wait for that entire mutation, not just for an individual get/set call.
fn interleave(
    first: impl FnOnce(TokenManager) + Send + 'static,
    second: impl FnOnce(TokenManager) + Send + 'static,
    key: &'static str,
    backend: &PausedBackend,
    manager: &TokenManager,
) {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    *backend.pause.lock().unwrap() = Some(Pause {
        key,
        entered: entered_tx,
        release: release_rx,
    });
    let first_manager = manager.clone();
    let a = std::thread::spawn(move || first(first_manager));
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let second_manager = manager.clone();
    let b = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        second(second_manager);
        done_tx.send(()).unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let _ = done_rx.recv_timeout(Duration::from_millis(200));
    release_tx.send(()).unwrap();
    a.join().unwrap();
    b.join().unwrap();
}
fn setup() -> (Arc<PausedBackend>, TokenManager) {
    let backend = Arc::new(PausedBackend::default());
    let manager = TokenManager::new(backend.clone(), Arc::new(MemoryBackend::default()));
    (backend, manager)
}

#[test]
fn credential_concurrent_token_merges_preserve_both_addresses() {
    for (key, authenticated) in [
        ("user-accounts-v2", true),
        ("user-accounts-v2", false),
        ("device-tokens", false),
    ] {
        let (backend, manager) = setup();
        if authenticated {
            session(&manager, "account-a");
        }
        let write = move |m: TokenManager, address: &str| {
            if key == "device-tokens" {
                m.save_device_token(address.into(), token(address)).unwrap();
            } else {
                m.save_user_token(address.into(), token(address)).unwrap();
            }
        };
        interleave(
            move |m| write(m, "a"),
            move |m| write(m, "b"),
            key,
            &backend,
            &manager,
        );
        for address in ["a", "b"] {
            let stored = if key == "device-tokens" {
                manager.get_device_token_for(address)
            } else {
                manager.get_user_token_for(address)
            }
            .unwrap();
            assert!(matches!(stored, Some(Token::Compact(value)) if value == address));
        }
    }
}

#[test]
fn credential_session_replacement_waits_for_in_progress_mutation() {
    let (backend, manager) = setup();
    session(&manager, "account-a");
    interleave(
        |m| m.save_user_token("refresh-a".into(), token("a")).unwrap(),
        |m| session(&m, "account-b"),
        "user-accounts-v2",
        &backend,
        &manager,
    );
    assert_eq!(manager.get_user().unwrap().puid, "account-b");
    assert!(manager.get_user_token_for("refresh-a").unwrap().is_none());
}

#[test]
fn credential_logout_waits_for_in_progress_mutation() {
    let (backend, manager) = setup();
    session(&manager, "account-a");
    interleave(
        |m| m.save_user_token("refresh-a".into(), token("a")).unwrap(),
        |m| m.remove_persistent().unwrap(),
        "user-accounts-v2",
        &backend,
        &manager,
    );
    assert!(manager.get_user_sts_session().unwrap().is_none());
    assert!(manager.get_user_token_for("refresh-a").unwrap().is_none());
}

#[test]
fn credential_failed_update_releases_transaction_for_retry() {
    let (backend, manager) = setup();
    session(&manager, "account-a");
    backend.fail_write.store(true, Ordering::SeqCst);
    assert!(
        manager
            .save_user_token("refresh".into(), token("new"))
            .is_err()
    );
    assert!(manager.get_user_token_for("refresh").unwrap().is_none());
    manager
        .clone()
        .save_user_token("refresh".into(), token("new"))
        .unwrap();
    assert!(
        matches!(manager.get_user_token_for("refresh").unwrap(), Some(Token::Compact(value)) if value == "new")
    );
    assert!(
        matches!(manager.get_user_sts_token().unwrap(), Token::Compact(value) if value == "account-a")
    );
}

#[test]
fn credential_independent_managers_share_backend_transaction_guard() {
    let (backend, manager) = setup();
    session(&manager, "account-a");
    let independent = TokenManager::new(backend.clone(), Arc::new(MemoryBackend::default()));
    interleave(
        |m| m.save_user_token("first".into(), token("a")).unwrap(),
        move |_| {
            independent
                .save_user_token("second".into(), token("b"))
                .unwrap()
        },
        "user-accounts-v2",
        &backend,
        &manager,
    );
    assert!(manager.get_user_token_for("first").unwrap().is_some());
    assert!(manager.get_user_token_for("second").unwrap().is_some());
}
