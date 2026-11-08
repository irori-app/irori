//! A PIN in front of hidden games.
//!
//! Hiding a game only takes it out of the library's usual view; anyone at
//! the computer can show hidden games again. With a PIN set, hidden games
//! stay out of every place that lists games until the PIN is entered: the
//! library (its Filters, search and suggestions), Big Picture, the command
//! palette, the stats page, the command line (`irori list`), Sunshine's
//! applications and Discord's "Playing". Entering it unlocks them until
//! Irori quits or the user locks them again.
//!
//! This is a privacy screen, not encryption: the games' files and Irori's
//! own data are untouched. The PIN is kept as a salted, stretched SHA-256
//! hash in `<config>/hidden-lock.json`, written through `irori_secrets`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Result, bail};
use irori_core::{Event, GameId, Paths};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::Backend;

/// Hash rounds: slow enough that guessing a short PIN from a copied file
/// takes a while, quick enough to check at once.
const ROUNDS: u32 = 100_000;
/// The shortest PIN accepted.
pub const MIN_PIN: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Stored {
    salt: String,
    hash: String,
}

fn file() -> PathBuf {
    Paths::get().config.join("hidden-lock.json")
}

fn hash(pin: &str, salt: &str) -> String {
    let mut digest = Sha256::digest(format!("{salt}:{pin}").as_bytes());
    for _ in 1..ROUNDS {
        digest = Sha256::digest(digest);
    }
    hex::encode(digest)
}

/// Whether `pin` is acceptable: digits only, at least [`MIN_PIN`] of them.
pub fn valid_pin(pin: &str) -> bool {
    pin.len() >= MIN_PIN && pin.len() <= 32 && pin.chars().all(|c| c.is_ascii_digit())
}

pub struct HiddenLock {
    stored: RwLock<Option<Stored>>,
    unlocked: AtomicBool,
}

impl HiddenLock {
    /// Read the PIN file. Hidden games start locked when it holds a PIN.
    pub fn load() -> Self {
        let stored = irori_secrets::load::<Stored>(&file());
        Self {
            stored: RwLock::new(stored),
            unlocked: AtomicBool::new(false),
        }
    }

    pub fn has_pin(&self) -> bool {
        self.stored.read().is_some()
    }

    /// Whether hidden games must stay out of sight now.
    pub fn locked(&self) -> bool {
        self.has_pin() && !self.unlocked.load(Ordering::Relaxed)
    }

    fn matches(&self, pin: &str) -> bool {
        self.stored
            .read()
            .as_ref()
            .is_some_and(|s| hash(pin, &s.salt) == s.hash)
    }

    #[cfg(test)]
    fn in_memory() -> Self {
        Self {
            stored: RwLock::new(None),
            unlocked: AtomicBool::new(false),
        }
    }
}

impl Backend {
    /// Whether hidden games are behind a PIN that hasn't been entered.
    pub fn hidden_locked(&self) -> bool {
        self.hidden_lock.locked()
    }

    /// Whether a PIN guards hidden games at all.
    pub fn hidden_pin_set(&self) -> bool {
        self.hidden_lock.has_pin()
    }

    /// Whether `id` is hidden and must not be shown now.
    pub fn concealed(&self, id: &GameId) -> bool {
        self.hidden_locked() && self.userdata.snapshot().hidden.contains(&id.key())
    }

    /// Recorded sessions, leaving out hidden games' while they are locked:
    /// what the stats page and the Year in review count.
    pub fn visible_sessions(&self) -> Vec<crate::sessions::Session> {
        let sessions = self.sessions();
        if !self.hidden_locked() {
            return sessions;
        }
        let user = self.userdata.snapshot();
        sessions
            .into_iter()
            .filter(|s| !user.hidden.contains(&s.game))
            .collect()
    }

    /// Set a new PIN (`Some`) or remove it (`None`). Changing or removing one
    /// needs the current PIN. Blocks on the hash and the file: call it off
    /// the UI thread.
    pub fn set_hidden_pin(&self, current: Option<&str>, new: Option<&str>) -> Result<()> {
        let lock = &self.hidden_lock;
        if lock.has_pin() && !current.is_some_and(|c| lock.matches(c)) {
            bail!("That PIN isn't right.");
        }
        match new {
            Some(pin) => {
                if !valid_pin(pin) {
                    bail!("A PIN is {MIN_PIN} to 32 digits.");
                }
                let salt = uuid::Uuid::new_v4().simple().to_string();
                let stored = Stored {
                    hash: hash(pin, &salt),
                    salt,
                };
                irori_secrets::write_json(&file(), &stored)?;
                *lock.stored.write() = Some(stored);
                // Whoever just set it knows it.
                lock.unlocked.store(true, Ordering::Relaxed);
            }
            None => {
                match std::fs::remove_file(file()) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(anyhow::Error::from(e).context("Couldn't remove the PIN")),
                }
                *lock.stored.write() = None;
                lock.unlocked.store(false, Ordering::Relaxed);
            }
        }
        self.after_lock_change();
        Ok(())
    }

    /// Show hidden games until Irori quits or they are locked again. False
    /// for a wrong PIN. Blocks on the hash: call it off the UI thread.
    pub fn unlock_hidden(&self, pin: &str) -> bool {
        if !self.hidden_lock.matches(pin) {
            return false;
        }
        self.hidden_lock.unlocked.store(true, Ordering::Relaxed);
        self.after_lock_change();
        true
    }

    /// Put hidden games out of sight again.
    pub fn lock_hidden(&self) {
        if self.hidden_lock.has_pin() {
            self.hidden_lock.unlocked.store(false, Ordering::Relaxed);
            self.after_lock_change();
        }
    }

    fn after_lock_change(&self) {
        // Screens re-read what they show; Sunshine and Discord follow.
        self.bus.emit(Event::SettingsChanged);
        self.rebuild_library();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_are_digits_and_hash_with_their_salt() {
        assert!(valid_pin("1234"));
        assert!(!valid_pin("123"));
        assert!(!valid_pin("12a4"));
        assert_ne!(hash("1234", "a"), hash("1234", "b"));
        assert_eq!(hash("1234", "a"), hash("1234", "a"));
    }

    #[test]
    fn locked_only_with_a_pin_until_unlocked() {
        let lock = HiddenLock::in_memory();
        assert!(!lock.locked(), "no PIN, nothing locked");
        *lock.stored.write() = Some(Stored {
            salt: "s".into(),
            hash: hash("2468", "s"),
        });
        assert!(lock.locked());
        assert!(lock.matches("2468"));
        assert!(!lock.matches("1357"));
        lock.unlocked.store(true, Ordering::Relaxed);
        assert!(!lock.locked());
    }
}
