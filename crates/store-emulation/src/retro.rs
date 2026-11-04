//! RetroAchievements for the ROM library: the ROM library's account is a
//! RetroAchievements one (a username and Web API key, kept through
//! `irori_secrets`). It is optional and only adds achievements.
//!
//! A game is matched by the hash RetroAchievements gives its file
//! ([`irori_retroachievements_api::hash`]), against the list of the
//! console's games and their hashes. Hashes are cached by file size and
//! modification time, console lists for a week, progress for a quarter of
//! an hour; nothing is fetched until a game's achievements are asked for.
//!
//! [`sign_in_emulators`] signs RetroArch and the standalone emulators in to
//! the same account with the token RetroAchievements gives emulators.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context as _, Result};
use irori_core::achievement_cache::AchievementCache;
use irori_core::{Achievement, UserInfo, json_store, util};
use irori_engines::cheevos;
use irori_retroachievements_api::model::{hash_index, rfc3339};
use irori_retroachievements_api::{
    Client, Credentials, Endpoints, GameProgress, badge_url, console_for, hash, locked_badge_url,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// How long a game's progress is served without asking.
const PROGRESS_MAX_AGE_SECS: u64 = 15 * 60;
/// How long a console's list of games is kept.
const GAMES_MAX_AGE_SECS: u64 = 7 * 24 * 3600;
/// The account's slot in [`irori_core::StoreAccount`].
pub const SLOT: &str = "retroachievements";

/// The saved account. `Debug` leaves the key out.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Account {
    pub username: String,
    pub api_key: String,
}

impl std::fmt::Debug for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Account")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

impl Account {
    fn credentials(&self) -> Credentials {
        Credentials {
            username: self.username.clone(),
            api_key: self.api_key.clone(),
        }
    }
}

/// What [`irori_core::Store::login`] takes for the ROM library: the
/// username and Web API key, as JSON.
pub fn login_input(username: &str, api_key: &str) -> String {
    serde_json::json!({"username": username.trim(), "apiKey": api_key.trim()}).to_string()
}

fn parse_login_input(input: &str) -> Result<Account> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Input {
        username: String,
        api_key: String,
    }
    let i: Input = serde_json::from_str(input.trim())
        .context("Sign in to RetroAchievements with your username and Web API key")?;
    let account = Account {
        username: i.username.trim().to_string(),
        api_key: i.api_key.trim().to_string(),
    };
    if account.username.is_empty() {
        anyhow::bail!("Enter your RetroAchievements username");
    }
    if !irori_retroachievements_api::is_api_key(&account.api_key) {
        anyhow::bail!(
            "That is not a RetroAchievements Web API key: it has 32 letters and digits and is shown in the site's settings"
        );
    }
    Ok(account)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct HashEntry {
    size: u64,
    modified: u64,
    hash: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct GameIndex {
    at: u64,
    /// Hash to game id.
    games: BTreeMap<String, u64>,
}

/// RetroAchievements state of the ROM library store.
pub(crate) struct Retro {
    client: Option<Client>,
    account_file: PathBuf,
    cache_dir: PathBuf,
    account: RwLock<Option<Account>>,
    progress: AchievementCache,
}

impl Retro {
    pub fn new(config_dir: &Path, cache_dir: &Path, endpoints: Endpoints) -> Self {
        Self {
            client: Client::with_endpoints(endpoints)
                .inspect_err(|e| tracing::warn!("RetroAchievements client: {e:#}"))
                .ok(),
            account_file: config_dir.join("retroachievements.json"),
            cache_dir: cache_dir.join("retroachievements"),
            account: RwLock::new(None),
            progress: AchievementCache::new(
                cache_dir.join("retroachievements").join("progress.json"),
                PROGRESS_MAX_AGE_SECS,
            ),
        }
    }

    fn client(&self) -> Result<&Client> {
        self.client
            .as_ref()
            .context("Irori couldn't set up its connection to RetroAchievements")
    }

    /// Read the saved account (disk only).
    pub async fn load(&self) {
        let path = self.account_file.clone();
        let account = tokio::task::spawn_blocking(move || irori_secrets::load::<Account>(&path))
            .await
            .ok()
            .flatten();
        *self.account.write() = account;
    }

    pub fn user(&self) -> Option<UserInfo> {
        self.account.read().as_ref().map(|a| UserInfo {
            account_id: a.username.clone(),
            display_name: a.username.clone(),
        })
    }

    /// Check the username and key with RetroAchievements, then keep them.
    pub async fn login(&self, input: &str) -> Result<UserInfo> {
        let account = parse_login_input(input)?;
        let profile = self.client()?.user_profile(&account.credentials()).await?;
        let account = Account {
            username: profile.user,
            ..account
        };
        let path = self.account_file.clone();
        let saved = account.clone();
        tokio::task::spawn_blocking(move || irori_secrets::write_json(&path, &saved))
            .await
            .context("saving the sign-in stopped")?
            .context("Could not save the RetroAchievements sign-in")?;
        let user = UserInfo {
            account_id: account.username.clone(),
            display_name: account.username.clone(),
        };
        *self.account.write() = Some(account);
        Ok(user)
    }

    pub async fn logout(&self) -> Result<()> {
        *self.account.write() = None;
        let path = self.account_file.clone();
        tokio::task::spawn_blocking(move || match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        })
        .await
        .context("signing out stopped")?
        .context("Could not remove the RetroAchievements sign-in")?;
        let progress = self.progress.path().to_path_buf();
        let _ = tokio::fs::remove_file(progress).await;
        Ok(())
    }

    pub async fn forget(&self, app_name: &str) {
        self.progress.forget(app_name).await
    }

    /// The hash of `file` for `system`, from the cache when the file is
    /// unchanged. Blocking work runs on the blocking pool.
    async fn hash_of(&self, system: &str, file: &Path) -> Result<Option<String>> {
        let Some(console) = console_for(system) else {
            return Ok(None);
        };
        let cache = self.cache_dir.join("hashes.json");
        let file = file.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<Option<String>> {
            let meta = std::fs::metadata(&file)
                .with_context(|| format!("could not read {}", file.display()))?;
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            let key = file.to_string_lossy().to_string();
            let mut all: HashMap<String, HashEntry> = json_store::read_or_default(&cache);
            if let Some(e) = all.get(&key)
                && e.size == meta.len()
                && e.modified == modified
            {
                return Ok(Some(e.hash.clone()));
            }
            let hash = match hash::hash_file(console.method, &file) {
                Ok(h) => h,
                Err(e) => {
                    tracing::debug!("no RetroAchievements hash for {}: {e:#}", file.display());
                    return Ok(None);
                }
            };
            all.insert(
                key,
                HashEntry {
                    size: meta.len(),
                    modified,
                    hash: hash.clone(),
                },
            );
            if let Err(e) = json_store::write(&cache, &all) {
                tracing::warn!("saving RetroAchievements hashes: {e:#}");
            }
            Ok(Some(hash))
        })
        .await
        .context("hashing the game stopped")?
    }

    /// The RetroAchievements game a hash belongs to, from the console's list.
    async fn game_id(&self, account: &Account, console: u32, hash: &str) -> Result<Option<u64>> {
        let path = self.cache_dir.join(format!("games-{console}.json"));
        let read = path.clone();
        let cached: Option<GameIndex> = tokio::task::spawn_blocking(move || {
            json_store::read::<GameIndex>(&read).ok().flatten()
        })
        .await
        .ok()
        .flatten();
        if let Some(index) = cached
            .as_ref()
            .filter(|i| util::unix_now().saturating_sub(i.at) < GAMES_MAX_AGE_SECS)
        {
            return Ok(index.games.get(hash).copied());
        }
        let list = match self
            .client()?
            .game_list(&account.credentials(), console)
            .await
        {
            Ok(list) => list,
            Err(e) => {
                return match cached {
                    Some(index) => Ok(index.games.get(hash).copied()),
                    None => Err(e),
                };
            }
        };
        let index = GameIndex {
            at: util::unix_now(),
            games: hash_index(&list),
        };
        let found = index.games.get(hash).copied();
        let _ = tokio::task::spawn_blocking(move || json_store::write(&path, &index)).await;
        Ok(found)
    }

    /// Achievements of the game in `file`: empty without an account, for a
    /// system RetroAchievements doesn't cover, or a file it doesn't know.
    pub async fn achievements(
        &self,
        app_name: &str,
        system: &str,
        file: &Path,
    ) -> Result<Vec<Achievement>> {
        let Some(account) = self.account.read().clone() else {
            return Ok(Vec::new());
        };
        let Some(console) = console_for(system) else {
            return Ok(Vec::new());
        };
        let cached = self.progress.get(app_name, "").await;
        if let Some(c) = cached.as_ref().filter(|c| c.fresh) {
            return Ok(c.list.clone());
        }
        let fetched = async {
            let Some(hash) = self.hash_of(system, file).await? else {
                return Ok(Vec::new());
            };
            let Some(game) = self.game_id(&account, console.id, &hash).await? else {
                return Ok(Vec::new());
            };
            let progress = self
                .client()?
                .game_progress(&account.credentials(), game)
                .await?;
            anyhow::Ok(map_progress(&progress))
        }
        .await;
        match fetched {
            Ok(list) => {
                self.progress.put(app_name, "", &list).await;
                Ok(list)
            }
            Err(e) => match cached {
                Some(c) => {
                    tracing::debug!("RetroAchievements of {app_name}: {e:#}");
                    Ok(c.list)
                }
                None => Err(e),
            },
        }
    }
}

/// Achievements of a progress answer, with rarity from the share of the
/// game's players who unlocked each.
pub(crate) fn map_progress(p: &GameProgress) -> Vec<Achievement> {
    p.achievements
        .iter()
        .map(|a| Achievement {
            id: a.id.to_string(),
            key: a.id.to_string(),
            visible: true,
            name: a.title.clone(),
            description: a.description.clone(),
            image_unlocked: badge_url(&a.badge_name),
            image_locked: locked_badge_url(&a.badge_name),
            rarity: if p.num_distinct_players > 0 {
                (a.num_awarded as f64 / p.num_distinct_players as f64 * 100.0).min(100.0) as f32
            } else {
                0.0
            },
            unlocked_at: a.date_earned.as_deref().and_then(rfc3339),
            hardcore_unlocked_at: a.date_earned_hardcore.as_deref().and_then(rfc3339),
            points: a.points.min(u32::MAX as u64) as u32,
            ..Default::default()
        })
        .collect()
}

/// What signing one emulator in did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmulatorSignIn {
    pub engine: String,
    /// The emulator's name.
    pub name: String,
    pub result: std::result::Result<cheevos::Outcome, String>,
}

/// Sign every installed emulator that supports RetroAchievements in to
/// `username`'s account: `password` is exchanged once for the emulators'
/// token and not kept. Emulators that aren't installed are left out.
pub async fn sign_in_emulators(
    username: &str,
    password: &str,
    hardcore: bool,
) -> Result<Vec<EmulatorSignIn>> {
    sign_in_emulators_at(Endpoints::default(), username, password, hardcore).await
}

pub(crate) async fn sign_in_emulators_at(
    endpoints: Endpoints,
    username: &str,
    password: &str,
    hardcore: bool,
) -> Result<Vec<EmulatorSignIn>> {
    let login = Client::with_endpoints(endpoints)?
        .connect_login(username, password)
        .await?;
    let login = cheevos::Login {
        username: login.user,
        token: login.token,
        hardcore,
    };
    tokio::task::spawn_blocking(move || {
        let state = irori_engines::state::load();
        let roots = cheevos::Roots::current();
        cheevos::SUPPORTED
            .iter()
            .filter_map(|id| {
                let manifest = irori_engines::engine(id)?;
                let installs = irori_engines::discovery::find(&manifest, &state);
                let install = installs.first()?;
                Some(EmulatorSignIn {
                    engine: id.to_string(),
                    name: manifest.name.clone(),
                    result: cheevos::sign_in(id, install, &login, &roots)
                        .map_err(|e| format!("{e:#}")),
                })
            })
            .collect()
    })
    .await
    .context("signing emulators in stopped")
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_depot::test_server::{Resp, TestServer};
    use serde_json::json;

    const KEY: &str = "AbCdEf0123456789AbCdEf0123456789";

    #[test]
    fn login_input_round_trips_and_checks_the_key() {
        let a = parse_login_input(&login_input(" player ", KEY)).unwrap();
        assert_eq!(a.username, "player");
        assert!(!format!("{a:?}").contains(KEY));
        assert!(parse_login_input(&login_input("player", "short")).is_err());
        assert!(parse_login_input(&login_input("", KEY)).is_err());
    }

    #[tokio::test]
    async fn matches_a_rom_by_hash_and_maps_progress() {
        let dir = tempfile::tempdir().unwrap();
        let rom = dir.path().join("game.gb");
        std::fs::write(&rom, b"a game boy game").unwrap();
        let rom_hash = hash::hash_file(hash::Method::Whole, &rom).unwrap();
        let games = rom_hash.clone();
        let server = TestServer::start(move |req| {
            let p = req.path.as_str();
            if !p.contains(KEY) {
                return Resp::status(401);
            }
            if p.starts_with("/API/API_GetUserProfile.php") {
                Resp::json(json!({"User": "Player"}))
            } else if p.starts_with("/API/API_GetGameList.php") && p.contains("i=4") {
                Resp::json(json!([{"ID": 77, "Title": "Game", "Hashes": [games.to_uppercase()]}]))
            } else if p.starts_with("/API/API_GetGameInfoAndUserProgress.php") && p.contains("g=77")
            {
                Resp::json(
                    json!({"ID": 77, "NumDistinctPlayers": 200, "Achievements": {
                        "5": {"ID": 5, "Title": "First", "Description": "d", "Points": 10,
                              "BadgeName": "123", "DisplayOrder": 1, "NumAwarded": 150,
                              "DateEarned": "2024-02-03 04:05:06",
                              "DateEarnedHardcore": "2024-02-03 04:05:06"},
                        "6": {"ID": 6, "Title": "Second", "Points": 25, "BadgeName": "124",
                              "DisplayOrder": 2, "NumAwarded": 10}
                    }}),
                )
            } else {
                Resp::status(404)
            }
        });
        let retro = Retro::new(
            &dir.path().join("config"),
            &dir.path().join("cache"),
            Endpoints {
                site: server.base(),
            },
        );
        assert!(
            retro
                .achievements("g", "gb", &rom)
                .await
                .unwrap()
                .is_empty()
        );
        let user = retro.login(&login_input("player", KEY)).await.unwrap();
        assert_eq!(user.display_name, "Player");
        let list = retro.achievements("g", "gb", &rom).await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].rarity, 75.0);
        assert_eq!(list[0].points, 10);
        assert_eq!(list[0].unlocked_at.as_deref(), Some("2024-02-03T04:05:06Z"));
        assert!(list[0].hardcore_unlocked_at.is_some());
        assert!(!list[1].is_unlocked());
        assert!(list[1].image_locked.ends_with("/Badge/124_lock.png"));
        // A system RetroAchievements doesn't cover has none.
        assert!(
            retro
                .achievements("d", "dos", &rom)
                .await
                .unwrap()
                .is_empty()
        );

        // The sign-in survives a restart and goes on sign-out.
        let again = Retro::new(
            &dir.path().join("config"),
            &dir.path().join("cache"),
            Endpoints::default(),
        );
        again.load().await;
        assert_eq!(again.user().unwrap().display_name, "Player");
        again.logout().await.unwrap();
        assert!(again.user().is_none());
        again.load().await;
        assert!(again.user().is_none());
    }

    #[tokio::test]
    async fn emulator_sign_in_needs_the_right_password() {
        let server = TestServer::start(|_| {
            Resp::json(json!({"Success": false, "Error": "Invalid username/password."}))
        });
        let e = sign_in_emulators_at(
            Endpoints {
                site: server.base(),
            },
            "player",
            "wrong",
            false,
        )
        .await
        .unwrap_err();
        assert!(format!("{e:#}").contains("Invalid username/password"));
    }
}
