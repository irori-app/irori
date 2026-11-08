//! The optional Xbox (Microsoft) account of the Xbox app's games.
//!
//! Signing in (Microsoft's OAuth with PKCE in the sign-in window, then the
//! Xbox Live and XSTS tokens, `irori_xbox_api`) adds what the Xbox network
//! knows about the account's PC games: minutes played, when each was last
//! played, achievements and store art. Account titles match installed
//! packages by package family name; PC titles the account played that
//! aren't installed are listed too, never installed by Irori (the
//! Microsoft Store page installs them).
//!
//! Only the refresh token, xuid and gamertag are kept, through
//! `irori_secrets`; access and XSTS tokens live in memory. A refused
//! refresh token becomes [`SessionExpired`] and the account stays listed as
//! expired until the user signs in again.
//!
//! The Microsoft app Irori signs in with is [`XBOX_CLIENT_ID`]; packagers
//! may name another with `IRORI_XBOX_CLIENT_ID`, or turn the account off by
//! setting it empty. Without a client id nothing connects and the account
//! is hidden.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use futures::StreamExt as _;
use irori_core::achievement_cache::AchievementCache;
use irori_core::{Achievement, LoginFlow, Runner, SessionExpired, UserInfo, json_store, util};
use irori_xbox_api::{Client, Endpoints, Pkce, Session, Title, XboxError};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

/// Irori's own Microsoft (Azure) app: personal accounts only, a public
/// client without a secret, redirecting to the desktop page. The id is
/// public; it only names the app on Microsoft's consent page.
pub const XBOX_CLIENT_ID: &str = "0c31b03a-aedf-47a6-b525-bca019d91e1e";
/// Names another Microsoft app's client id; empty turns the account off.
pub const CLIENT_ID_VAR: &str = "IRORI_XBOX_CLIENT_ID";
/// The account's slot in [`irori_core::StoreAccount`].
pub const SLOT: &str = "xbox";

/// How long the title history is served without asking.
const TITLES_MAX_AGE_SECS: u64 = 15 * 60;
/// How long a title's achievements are served without asking.
const ACHIEVEMENTS_MAX_AGE_SECS: u64 = 15 * 60;
/// Minutes-played requests at once.
const STATS_AT_ONCE: usize = 4;

/// The client id to sign in with, if any.
pub fn client_id() -> Option<String> {
    pick_client_id(std::env::var(CLIENT_ID_VAR).ok())
}

fn pick_client_id(var: Option<String>) -> Option<String> {
    let id = var.unwrap_or_else(|| XBOX_CLIENT_ID.to_string());
    let id = id.trim();
    (!id.is_empty()).then(|| id.to_string())
}

/// The kept sign-in. `Debug` leaves the token out.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    refresh_token: String,
    xuid: String,
    gamertag: String,
}

impl std::fmt::Debug for Saved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Saved")
            .field("xuid", &self.xuid)
            .field("gamertag", &self.gamertag)
            .finish_non_exhaustive()
    }
}

/// A PC game the account played, as the library uses it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountTitle {
    /// The title record played most recently.
    pub title_id: String,
    /// Every title record of the game, most recently played first: the
    /// title history can list one game (one package family) under several
    /// title ids.
    pub title_ids: Vec<String>,
    /// Package family name: what installed packages match by.
    pub pfn: String,
    pub name: String,
    /// RFC 3339.
    pub last_played: Option<String>,
    pub minutes: Option<u64>,
    pub cover: Option<String>,
    pub square: Option<String>,
    pub background: Option<String>,
    pub logo: Option<String>,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub release_date: Option<String>,
    pub genres: Vec<String>,
    pub product_id: Option<String>,
}

impl AccountTitle {
    /// Irori's app name: the same as the installed package's.
    pub fn app_name(&self) -> String {
        format!("{}:{}", irori_launchers_data::xbox::ID, self.pfn)
    }

    fn from_title(t: &Title) -> Self {
        let detail = t.detail.clone().unwrap_or_default();
        let nonempty = |s: Option<String>| s.filter(|s| !s.trim().is_empty());
        Self {
            title_id: t.title_id.clone(),
            title_ids: vec![t.title_id.clone()],
            pfn: t.pfn.clone().unwrap_or_default(),
            name: t.name.clone(),
            last_played: t.last_played().map(str::to_string),
            minutes: None,
            cover: t.image(&["Poster", "BoxArt"]),
            square: t
                .image(&["BoxArt", "FeaturePromotionalSquareArt"])
                .or_else(|| t.display_image.as_deref().map(irori_xbox_api::model::https)),
            background: t.image(&["SuperHeroArt", "TitledHeroArt", "Screenshot"]),
            logo: t.image(&["Logo"]),
            developer: nonempty(detail.developer_name),
            publisher: nonempty(detail.publisher_name),
            description: nonempty(detail.short_description).or(nonempty(detail.description)),
            release_date: nonempty(detail.release_date),
            genres: detail.genres,
            product_id: nonempty(t.product_id.clone()),
        }
    }

    /// Every title id of the game (titles cached before they were merged
    /// name only one).
    pub fn ids(&self) -> Vec<String> {
        if self.title_ids.is_empty() {
            vec![self.title_id.clone()]
        } else {
            self.title_ids.clone()
        }
    }

    /// The title's page in the Microsoft Store app.
    pub fn store_link(&self) -> String {
        match &self.product_id {
            Some(id) => format!("ms-windows-store://pdp/?ProductId={id}"),
            None => format!("ms-windows-store://pdp/?PFN={}", self.pfn),
        }
    }

    /// The title's page on the Microsoft Store website.
    pub fn store_web_url(&self) -> Option<String> {
        self.product_id
            .as_ref()
            .map(|id| format!("https://apps.microsoft.com/detail/{id}"))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct TitlesFile {
    at: u64,
    xuid: String,
    titles: Vec<AccountTitle>,
    /// Minutes played per title id, with the last-played time they were
    /// read at.
    stats: HashMap<String, TitleStat>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TitleStat {
    last_played: Option<String>,
    minutes: Option<u64>,
}

/// The account's PC games from the title history's PC records, one per
/// package family. A game listed under several title ids (an old and a
/// new release of one package) is one game: its newest record gives the
/// name, art and details (older ones fill what it lacks), its last played
/// is the latest, and its minutes are the sum of the records' minutes,
/// since each title id counts the sessions played under it and no session
/// counts under two.
fn merge_titles(records: &[&Title], stats: &HashMap<String, TitleStat>) -> Vec<AccountTitle> {
    let played = |t: &Title| t.last_played().and_then(util::parse_rfc3339);
    let mut families: Vec<(String, Vec<&Title>)> = Vec::new();
    for t in records {
        let pfn = t.pfn.clone().unwrap_or_default().to_lowercase();
        match families.iter_mut().find(|(f, _)| *f == pfn) {
            Some((_, list)) => list.push(t),
            None => families.push((pfn, vec![t])),
        }
    }
    families
        .into_iter()
        .map(|(_, mut list)| {
            list.sort_by_key(|t| std::cmp::Reverse(played(t)));
            let mut game = AccountTitle::from_title(list[0]);
            for older in &list[1..] {
                let o = AccountTitle::from_title(older);
                game.cover = game.cover.or(o.cover);
                game.square = game.square.or(o.square);
                game.background = game.background.or(o.background);
                game.logo = game.logo.or(o.logo);
                game.developer = game.developer.or(o.developer);
                game.publisher = game.publisher.or(o.publisher);
                game.description = game.description.or(o.description);
                game.release_date = game.release_date.or(o.release_date);
                game.product_id = game.product_id.or(o.product_id);
                if game.genres.is_empty() {
                    game.genres = o.genres;
                }
            }
            game.title_ids = list.iter().map(|t| t.title_id.clone()).collect();
            game.minutes = game
                .title_ids
                .iter()
                .filter_map(|id| stats.get(id)?.minutes)
                .reduce(|a, b| a + b);
            game
        })
        .collect()
}

/// Achievements of several title records of one game, each once: the
/// same achievement (same id and name) listed for two records counts once,
/// unlocked if either has it.
fn merge_achievements(lists: Vec<irori_xbox_api::Achievement>) -> Vec<irori_xbox_api::Achievement> {
    let mut out: Vec<irori_xbox_api::Achievement> = Vec::new();
    for a in lists {
        match out.iter_mut().find(|b| b.id == a.id && b.name == a.name) {
            Some(b) if !b.is_unlocked() && a.is_unlocked() => *b = a,
            Some(_) => {}
            None => out.push(a),
        }
    }
    out
}

/// What the Xbox card shows about the account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct XboxAccountStatus {
    /// Irori has a Microsoft app to sign in with.
    pub available: bool,
    pub gamertag: Option<String>,
    /// Microsoft refused the saved sign-in, with its reason.
    pub expired: Option<String>,
    /// PC games the account played.
    pub titles: usize,
}

/// `Accept-Language` for an Irori language code (`pt_BR` → `pt-BR`).
fn accept_language(language: &str) -> String {
    let lang = language.trim().replace('_', "-");
    if lang.is_empty() || lang.eq_ignore_ascii_case("en") {
        "en-US".into()
    } else {
        format!("{lang}, en-US;q=0.5")
    }
}

/// The Xbox account of the launchers store.
pub(crate) struct XboxAccount {
    client: Option<Client>,
    account_file: PathBuf,
    titles_file: PathBuf,
    saved: RwLock<Option<Saved>>,
    expired: RwLock<Option<String>>,
    session: tokio::sync::Mutex<Option<Session>>,
    /// The PKCE pair of the sign-in page last shown.
    pending: Mutex<Option<Pkce>>,
    titles: RwLock<TitlesFile>,
    achievements: AchievementCache,
}

impl XboxAccount {
    pub fn new(config_dir: &Path, cache_dir: &Path, endpoints: Endpoints) -> Self {
        let client = client_id().and_then(|id| {
            Client::with_endpoints(&id, endpoints)
                .inspect_err(|e| tracing::warn!("Xbox client: {e:#}"))
                .ok()
        });
        Self::with_client(config_dir, cache_dir, client)
    }

    pub fn with_client(config_dir: &Path, cache_dir: &Path, client: Option<Client>) -> Self {
        let cache = cache_dir.join("xbox");
        Self {
            client,
            account_file: config_dir.join("xbox-account.json"),
            titles_file: cache.join("titles.json"),
            saved: RwLock::new(None),
            expired: RwLock::new(None),
            session: tokio::sync::Mutex::new(None),
            pending: Mutex::new(None),
            titles: RwLock::new(TitlesFile::default()),
            achievements: AchievementCache::new(
                cache.join("achievements.json"),
                ACHIEVEMENTS_MAX_AGE_SECS,
            ),
        }
    }

    fn client(&self) -> Result<&Client> {
        self.client
            .as_ref()
            .context("This build of Irori has no Microsoft app to sign in to Xbox with")
    }

    pub fn available(&self) -> bool {
        self.client.is_some()
    }

    /// Read the kept sign-in and titles (disk only).
    pub async fn load(&self) {
        if !self.available() {
            return;
        }
        let (account, titles) = (self.account_file.clone(), self.titles_file.clone());
        let (saved, file) = tokio::task::spawn_blocking(move || {
            let saved = irori_secrets::load::<Saved>(&account);
            let file: TitlesFile = json_store::read_or_default(&titles);
            (saved, file)
        })
        .await
        .unwrap_or_default();
        let file = match &saved {
            Some(s) if s.xuid == file.xuid => file,
            _ => TitlesFile::default(),
        };
        *self.saved.write() = saved;
        *self.titles.write() = file;
    }

    pub fn user(&self) -> Option<UserInfo> {
        self.saved.read().as_ref().map(|s| UserInfo {
            account_id: s.xuid.clone(),
            display_name: s.gamertag.clone(),
        })
    }

    pub fn status(&self) -> XboxAccountStatus {
        XboxAccountStatus {
            available: self.available(),
            gamertag: self.saved.read().as_ref().map(|s| s.gamertag.clone()),
            expired: self.expired.read().clone(),
            titles: self.titles.read().titles.len(),
        }
    }

    pub fn expired(&self) -> Option<String> {
        self.expired.read().clone()
    }

    /// The account's PC games, as last fetched.
    pub fn titles(&self) -> Vec<AccountTitle> {
        if self.saved.read().is_none() {
            return Vec::new();
        }
        self.titles.read().titles.clone()
    }

    fn title(&self, app_name: &str) -> Option<AccountTitle> {
        self.titles()
            .into_iter()
            .find(|t| t.app_name().eq_ignore_ascii_case(app_name))
    }

    /// The Microsoft sign-in page, with a fresh PKCE pair.
    pub fn login_flow(&self) -> LoginFlow {
        let Ok(client) = self.client() else {
            return LoginFlow::None;
        };
        let pkce = Pkce::new();
        let url = client.authorize_url(&pkce);
        *self.pending.lock() = Some(pkce);
        LoginFlow::Browser {
            url,
            capture: irori_xbox_api::CAPTURE.into(),
            instructions: format!(
                "Sign in with your Microsoft account. You land on a blank page; copy its address (it starts with {}) and paste it here.",
                irori_xbox_api::REDIRECT_URL
            ),
        }
    }

    /// Finish the sign-in with what the sign-in page gave: its address,
    /// the page's text or the bare code.
    pub async fn login(&self, input: &str) -> Result<UserInfo> {
        let code = irori_xbox_api::code_from_input(input)?;
        let pkce = self
            .pending
            .lock()
            .clone()
            .context("Open the Xbox sign-in again: this code belongs to an earlier one")?;
        let client = self.client()?;
        let tokens = client.redeem_code(&code, &pkce).await?;
        let refresh_token = tokens
            .refresh_token
            .clone()
            .context("Microsoft didn't allow Irori to stay signed in to Xbox")?;
        let session = client.session(&tokens.access_token).await?;
        let saved = Saved {
            refresh_token,
            xuid: session.xuid.clone(),
            gamertag: session.gamertag.clone(),
        };
        self.save(&saved).await?;
        *self.pending.lock() = None;
        let user = UserInfo {
            account_id: saved.xuid.clone(),
            display_name: saved.gamertag.clone(),
        };
        let changed =
            self.saved.read().as_ref().map(|s| s.xuid.clone()) != Some(saved.xuid.clone());
        if changed {
            *self.titles.write() = TitlesFile::default();
        }
        *self.saved.write() = Some(saved);
        *self.expired.write() = None;
        *self.session.lock().await = Some(session);
        Ok(user)
    }

    async fn save(&self, saved: &Saved) -> Result<()> {
        let path = self.account_file.clone();
        let saved = saved.clone();
        tokio::task::spawn_blocking(move || irori_secrets::write_json(&path, &saved))
            .await
            .context("saving the sign-in stopped")?
            .context("Could not save the Xbox sign-in")
    }

    /// Forget the sign-in and everything fetched with it.
    pub async fn logout(&self) -> Result<()> {
        *self.saved.write() = None;
        *self.expired.write() = None;
        *self.titles.write() = TitlesFile::default();
        *self.session.lock().await = None;
        let files = [
            self.account_file.clone(),
            self.titles_file.clone(),
            self.achievements.path().to_path_buf(),
        ];
        tokio::task::spawn_blocking(move || {
            for file in files {
                match std::fs::remove_file(&file) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                    _ => {}
                }
            }
            Ok(())
        })
        .await
        .context("signing out stopped")?
        .context("Could not remove the Xbox sign-in")
    }

    /// A valid XSTS session: the one in memory, else a new one from the
    /// kept refresh token. A refused refresh token marks the account
    /// expired and fails with [`SessionExpired`].
    async fn session(&self) -> Result<Session> {
        let mut current = self.session.lock().await;
        if let Some(s) = current
            .as_ref()
            .filter(|s| s.is_valid_at(util::unix_now() as i64))
        {
            return Ok(s.clone());
        }
        let saved = self.saved.read().clone().context("Not signed in to Xbox")?;
        let client = self.client()?;
        let tokens = match client.refresh(&saved.refresh_token).await {
            Ok(tokens) => tokens,
            Err(e) => {
                return Err(match e.downcast_ref::<XboxError>() {
                    Some(XboxError::SignInRefused(why)) => {
                        *self.expired.write() = Some(why.clone());
                        SessionExpired::new(Runner::Launchers, why.clone()).into()
                    }
                    _ => e,
                });
            }
        };
        let session = client.session(&tokens.access_token).await?;
        // Microsoft hands out a new refresh token with each refresh.
        if let Some(rotated) = tokens.refresh_token.filter(|t| *t != saved.refresh_token) {
            let next = Saved {
                refresh_token: rotated,
                gamertag: if session.gamertag.is_empty() {
                    saved.gamertag.clone()
                } else {
                    session.gamertag.clone()
                },
                ..saved
            };
            if let Err(e) = self.save(&next).await {
                tracing::warn!("{e:#}");
            }
            *self.saved.write() = Some(next);
        }
        *current = Some(session.clone());
        Ok(session)
    }

    /// Run `call` with a session, once more with a new one when the
    /// service says the token ran out.
    async fn with_session<T, F, Fut>(&self, call: F) -> Result<T>
    where
        F: Fn(Session) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let session = self.session().await?;
        match call(session).await {
            Err(e) if e.downcast_ref::<XboxError>() == Some(&XboxError::Unauthorized) => {
                *self.session.lock().await = None;
                call(self.session().await?).await
            }
            other => other,
        }
    }

    /// Fetch the title history again (at most every quarter of an hour
    /// unless `full`), with minutes played for titles played since the last
    /// fetch. Returns whether the titles changed.
    pub async fn refresh(&self, full: bool, language: &str) -> Result<bool> {
        let Some(saved) = self.saved.read().clone() else {
            return Ok(false);
        };
        if self.expired.read().is_some() {
            return Ok(false);
        }
        let fresh = {
            let t = self.titles.read();
            t.xuid == saved.xuid && util::unix_now().saturating_sub(t.at) < TITLES_MAX_AGE_SECS
        };
        if fresh && !full {
            return Ok(false);
        }
        let client = self.client()?.clone();
        let language = accept_language(language);
        let history = self
            .with_session(|s| {
                let (client, language) = (client.clone(), language.clone());
                async move { client.title_history(&s, &language).await }
            })
            .await?;
        let known = self.titles.read().stats.clone();
        let records: Vec<&Title> = history.iter().filter(|t| t.is_pc_game()).collect();
        // Minutes only change while a title is played, which moves its
        // last-played time; ask only for those (an answer without minutes
        // counts as an answer too, a failed request doesn't).
        let mut stats: HashMap<String, TitleStat> = HashMap::new();
        let mut ask = Vec::new();
        for t in &records {
            let last_played = t.last_played().map(str::to_string);
            match known.get(&t.title_id) {
                Some(k) if k.last_played == last_played => {
                    stats.insert(t.title_id.clone(), k.clone());
                }
                _ => ask.push((t.title_id.clone(), last_played)),
            }
        }
        if !ask.is_empty() {
            let session = self.session().await?;
            let answers: Vec<(String, TitleStat)> = futures::stream::iter(ask)
                .map(|(id, last_played)| {
                    let (client, session) = (client.clone(), session.clone());
                    let known = known.get(&id).cloned().unwrap_or_default();
                    async move {
                        let stat = match client.minutes_played(&session, &id).await {
                            Ok(minutes) => TitleStat {
                                last_played,
                                minutes: minutes.or(known.minutes),
                            },
                            Err(e) => {
                                tracing::debug!("minutes played of {id}: {e:#}");
                                known
                            }
                        };
                        (id, stat)
                    }
                })
                .buffer_unordered(STATS_AT_ONCE)
                .collect()
                .await;
            stats.extend(answers);
        }
        let titles = merge_titles(&records, &stats);
        debug_assert!(
            {
                let mut names: Vec<String> = titles.iter().map(|t| t.app_name()).collect();
                names.sort();
                names.windows(2).all(|w| !w[0].eq_ignore_ascii_case(&w[1]))
            },
            "one account title per package family"
        );
        let file = TitlesFile {
            at: util::unix_now(),
            xuid: saved.xuid.clone(),
            titles,
            stats,
        };
        let changed = {
            let current = self.titles.read();
            current.titles != file.titles || current.xuid != file.xuid
        };
        let path = self.titles_file.clone();
        let written = file.clone();
        if let Ok(Err(e)) =
            tokio::task::spawn_blocking(move || json_store::write(&path, &written)).await
        {
            tracing::warn!("saving the Xbox titles: {e:#}");
        }
        *self.titles.write() = file;
        Ok(changed)
    }

    /// Minutes played by app name.
    pub fn playtime(&self) -> HashMap<String, u64> {
        self.titles()
            .iter()
            .filter_map(|t| Some((t.app_name(), t.minutes.filter(|m| *m > 0)?)))
            .collect()
    }

    /// Last played by app name.
    pub fn last_played(&self) -> HashMap<String, String> {
        self.titles()
            .iter()
            .filter_map(|t| Some((t.app_name(), t.last_played.clone()?)))
            .collect()
    }

    /// The title's page in the Microsoft Store, for a title of the account.
    pub fn store_link(&self, app_name: &str) -> Option<String> {
        self.title(app_name).map(|t| t.store_link())
    }

    /// A game's achievements with the account's unlocks; empty without an
    /// account or for a game it never played.
    pub async fn achievements(&self, app_name: &str, language: &str) -> Result<Vec<Achievement>> {
        let Some(title) = self.title(app_name) else {
            return Ok(Vec::new());
        };
        let cached = self.achievements.get(app_name, language).await;
        if let Some(c) = cached.as_ref().filter(|c| c.fresh) {
            return Ok(c.list.clone());
        }
        let client = self.client()?.clone();
        let accept = accept_language(language);
        // The service takes several title ids at once, comma-separated.
        let ids = title.ids().join(",");
        let fetched = self
            .with_session(|s| {
                let (client, accept, ids) = (client.clone(), accept.clone(), ids.clone());
                async move { client.achievements(&s, &ids, &accept).await }
            })
            .await;
        match fetched {
            Ok(list) => {
                let list: Vec<Achievement> = merge_achievements(list)
                    .iter()
                    .map(map_achievement)
                    .collect();
                self.achievements.put(app_name, language, &list).await;
                Ok(list)
            }
            Err(e) => match cached {
                Some(c) if SessionExpired::of(&e).is_none() => {
                    tracing::debug!("Xbox achievements of {app_name}: {e:#}");
                    Ok(c.list)
                }
                _ => Err(e),
            },
        }
    }

    pub async fn forget_achievements(&self, app_name: &str) {
        self.achievements.forget(app_name).await
    }
}

/// An Xbox achievement as Irori shows it.
fn map_achievement(a: &irori_xbox_api::Achievement) -> Achievement {
    let icon = a.icon().unwrap_or_default();
    let rarity = a.rarity.as_ref();
    Achievement {
        id: a.id.clone(),
        key: a.id.clone(),
        visible: !a.is_secret || a.is_unlocked(),
        name: a.name.clone(),
        description: if a.description.is_empty() {
            a.locked_description.clone()
        } else {
            a.description.clone()
        },
        image_unlocked: icon.clone(),
        image_locked: icon,
        rarity: rarity
            .and_then(|r| r.current_percentage)
            .unwrap_or_default(),
        rarity_label: rarity
            .and_then(|r| r.current_category.clone())
            .unwrap_or_default(),
        // Some old unlocks carry no time; they still count as unlocked.
        unlocked_at: a.is_unlocked().then(|| {
            a.unlock_time()
                .and_then(util::parse_rfc3339)
                .and_then(util::rfc3339_from_unix)
                .unwrap_or_else(|| "1970-01-01T00:00:00Z".into())
        }),
        points: a.gamerscore(),
        progress: a.progress(),
        ..Default::default()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use irori_depot::test_server::{Resp, TestServer};

    #[test]
    fn the_client_id_can_be_replaced_or_turned_off() {
        assert_eq!(pick_client_id(None).as_deref(), Some(XBOX_CLIENT_ID));
        assert_eq!(
            pick_client_id(Some(" other ".into())).as_deref(),
            Some("other")
        );
        assert_eq!(pick_client_id(Some(String::new())), None);
    }

    #[test]
    fn asks_in_the_users_language() {
        assert_eq!(accept_language("en"), "en-US");
        assert_eq!(accept_language("pt_BR"), "pt-BR, en-US;q=0.5");
        assert_eq!(accept_language("zh_Hans"), "zh-Hans, en-US;q=0.5");
    }

    #[test]
    fn maps_achievements() {
        let a: irori_xbox_api::Achievement = serde_json::from_value(serde_json::json!({
            "id": "7", "name": "Ace", "progressState": "Achieved",
            "progression": {"timeUnlocked": "2026-01-17T03:19:00.3087016Z"},
            "mediaAssets": [{"type": "Icon", "url": "https://images-eds.xboxlive.com/7"}],
            "isSecret": true, "description": "Win.", "lockedDescription": "?",
            "rewards": [{"type": "Gamerscore", "value": "20"}],
            "rarity": {"currentCategory": "Rare", "currentPercentage": 4.5}
        }))
        .unwrap();
        let m = map_achievement(&a);
        assert!(m.visible && m.is_unlocked());
        assert_eq!(m.unlocked_at.as_deref(), Some("2026-01-17T03:19:00Z"));
        assert_eq!(
            (m.points, m.rarity, m.rarity_label.as_str()),
            (20, 4.5, "Rare")
        );
        let locked: irori_xbox_api::Achievement = serde_json::from_value(serde_json::json!({
            "id": "8", "name": "Hidden", "progressState": "NotStarted", "isSecret": true,
            "description": "", "lockedDescription": "Keep playing."
        }))
        .unwrap();
        let m = map_achievement(&locked);
        assert!(!m.visible && !m.is_unlocked());
        assert_eq!(m.description, "Keep playing.");
    }

    pub(crate) const PFN: &str = "Studio.SkyRacer_abcdefghjkmnp";

    /// Microsoft and the Xbox network, as far as the account uses them.
    pub(crate) fn server(refresh_ok: bool) -> TestServer {
        TestServer::start(move |req| {
            let path = req.path.split('?').next().unwrap_or_default().to_string();
            let body = String::from_utf8_lossy(&req.body).to_string();
            match path.as_str() {
                "/oauth20_token.srf" => {
                    if body.contains("grant_type=refresh_token") && !refresh_ok {
                        return Resp {
                            status: 400,
                            body: br#"{"error":"invalid_grant","error_description":"The user could not be signed in."}"#.to_vec(),
                            ..Default::default()
                        };
                    }
                    Resp::json(serde_json::json!({
                        "access_token": "access", "refresh_token": "refresh-next", "expires_in": 3600
                    }))
                }
                "/user/authenticate" => Resp::json(serde_json::json!({
                    "Token": "user", "DisplayClaims": {"xui": [{"uhs": "h"}]}
                })),
                "/xsts/authorize" => Resp::json(serde_json::json!({
                    "Token": "xsts", "NotAfter": "2099-01-01T00:00:00Z",
                    "DisplayClaims": {"xui": [{"uhs": "h", "xid": "25", "gtg": "Pilot"}]}
                })),
                p if p.contains("/titlehistory/") => Resp::json(serde_json::json!({"titles": [
                    {"titleId": "17", "pfn": PFN, "name": "Sky Racer", "type": "Game",
                     "devices": ["PC"], "productId": "9NSKYRACER00",
                     "images": [{"type": "Poster", "url": "https://img/poster"}],
                     "titleHistory": {"lastTimePlayed": "2026-10-08T01:55:12.4411936Z"},
                     "detail": {"publisherName": "Studio"}},
                    {"titleId": "18", "pfn": "Studio.Console_abcdefghjkmnp", "name": "Console",
                     "type": "Game", "devices": ["XboxOne"]}
                ]})),
                "/batch" => Resp::json(serde_json::json!({"statlistscollection": [
                    {"stats": [{"name": "MinutesPlayed", "value": "754"}]}]})),
                p if p.ends_with("/achievements") => Resp::json(serde_json::json!({
                    "achievements": [{"id": "1", "name": "First", "progressState": "Achieved",
                        "progression": {"timeUnlocked": "2026-10-01T00:00:00Z"},
                        "rewards": [{"type": "Gamerscore", "value": "10"}]}],
                    "pagingInfo": {"continuationToken": null}
                })),
                _ => Resp::status(404),
            }
        })
    }

    fn account(dir: &Path, server: &TestServer) -> XboxAccount {
        let client = Client::with_endpoints("client", Endpoints::all_at(&server.base())).unwrap();
        XboxAccount::with_client(&dir.join("config"), &dir.join("cache"), Some(client))
    }

    #[tokio::test]
    async fn signs_in_and_reads_playtime_and_achievements() {
        let dir = tempfile::tempdir().unwrap();
        let s = server(true);
        let xbox = account(dir.path(), &s);
        let LoginFlow::Browser { url, capture, .. } = xbox.login_flow() else {
            panic!("a browser sign-in");
        };
        assert!(url.contains("code_challenge="));
        assert!(capture.contains("oauth20_desktop"));
        let user = xbox
            .login("https://login.live.com/oauth20_desktop.srf?code=M.C1.abc&lc=1033")
            .await
            .unwrap();
        assert_eq!(user.display_name, "Pilot");
        // The kept file holds the refresh token but no access or XSTS token.
        let kept = std::fs::read_to_string(dir.path().join("config/xbox-account.json")).unwrap();
        assert!(kept.contains("refresh-next") && !kept.contains("xsts"));

        assert!(xbox.refresh(false, "en").await.unwrap());
        let titles = xbox.titles();
        assert_eq!(titles.len(), 1, "only PC games");
        let app = format!("xbox:{PFN}");
        assert_eq!(titles[0].app_name(), app);
        assert_eq!(titles[0].cover.as_deref(), Some("https://img/poster"));
        assert_eq!(
            titles[0].store_web_url().as_deref(),
            Some("https://apps.microsoft.com/detail/9NSKYRACER00")
        );
        assert_eq!(xbox.playtime(), HashMap::from([(app.clone(), 754)]));
        assert_eq!(
            xbox.last_played()[&app],
            "2026-10-08T01:55:12.4411936Z".to_string()
        );
        // Fresh: not asked again; full: asked, but minutes only for titles
        // played since.
        assert!(!xbox.refresh(false, "en").await.unwrap());
        let batches = |s: &TestServer| s.requests().iter().filter(|r| r.path == "/batch").count();
        assert_eq!(batches(&s), 1);
        assert!(!xbox.refresh(true, "en").await.unwrap());
        assert_eq!(batches(&s), 1);

        let list = xbox.achievements(&app, "en").await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].points, 10);
        assert!(
            xbox.achievements("xbox:Other_x", "en")
                .await
                .unwrap()
                .is_empty()
        );

        // Read back from disk by a new instance.
        let again = account(dir.path(), &s);
        again.load().await;
        assert_eq!(again.user().unwrap().account_id, "25");
        assert_eq!(again.titles().len(), 1);

        xbox.logout().await.unwrap();
        assert!(xbox.user().is_none() && xbox.titles().is_empty());
        assert!(!dir.path().join("config/xbox-account.json").exists());
    }

    #[tokio::test]
    async fn a_refused_refresh_token_expires_the_account() {
        let dir = tempfile::tempdir().unwrap();
        let s = server(false);
        let xbox = account(dir.path(), &s);
        let _ = xbox.login_flow();
        xbox.login("M.C1.abcdefgh").await.unwrap();
        // The XSTS session in memory runs out.
        *xbox.session.lock().await = None;
        let e = xbox.refresh(true, "en").await.unwrap_err();
        let expired = SessionExpired::of(&e).expect("an expired sign-in");
        assert_eq!(expired.runner, Runner::Launchers);
        assert_eq!(
            xbox.status().expired.as_deref(),
            Some("The user could not be signed in.")
        );
        // Nothing more is asked until the user signs in again.
        assert!(!xbox.refresh(true, "en").await.unwrap());
    }

    #[tokio::test]
    async fn a_code_needs_the_sign_in_it_came_from() {
        let dir = tempfile::tempdir().unwrap();
        let s = server(true);
        let xbox = account(dir.path(), &s);
        assert!(xbox.login("M.C1.abcdefgh").await.is_err());
        let off = XboxAccount::with_client(dir.path(), dir.path(), None);
        assert_eq!(off.login_flow(), LoginFlow::None);
        assert!(!off.status().available);
    }

    const MINESWEEPER: &str = "Microsoft.MicrosoftMinesweeper_8wekyb3d8bbwe";

    /// The title history as a real account sent it (values redacted): one
    /// package family under two title ids, with the same product id.
    fn two_records() -> serde_json::Value {
        serde_json::json!({"titles": [
            {"titleId": "1889321756", "pfn": MINESWEEPER, "name": "Microsoft Minesweeper",
             "type": "Game", "devices": ["PC"], "productId": "9WZDNCRFHWCN",
             "images": [{"type": "Poster", "url": "https://img/new-poster"}],
             "titleHistory": {"lastTimePlayed": "2026-10-20T18:00:00.1234567Z"}},
            {"titleId": "1297290226", "pfn": MINESWEEPER, "name": "Microsoft Minesweeper",
             "type": "Game", "devices": ["PC"], "productId": "9WZDNCRFHWCN",
             "images": [{"type": "Poster", "url": "https://img/old-poster"},
                        {"type": "SuperHeroArt", "url": "https://img/old-hero"}],
             "titleHistory": {"lastTimePlayed": "2024-02-01T10:00:00Z"}}
        ]})
    }

    #[test]
    fn one_game_listed_under_two_title_ids_is_one_game() {
        let hub: Vec<Title> = serde_json::from_value(two_records()["titles"].clone()).unwrap();
        let records: Vec<&Title> = hub.iter().collect();
        // The older record keeps no minutes, the newer one 2.
        let stats = HashMap::from([
            (
                "1297290226".to_string(),
                TitleStat {
                    minutes: None,
                    ..Default::default()
                },
            ),
            (
                "1889321756".to_string(),
                TitleStat {
                    minutes: Some(2),
                    ..Default::default()
                },
            ),
        ]);
        let titles = merge_titles(&records, &stats);
        assert_eq!(titles.len(), 1);
        let game = &titles[0];
        assert_eq!(game.app_name(), format!("xbox:{MINESWEEPER}"));
        assert_eq!(game.title_ids, ["1889321756", "1297290226"]);
        assert_eq!(game.title_id, "1889321756");
        assert_eq!(game.minutes, Some(2));
        assert_eq!(
            game.last_played.as_deref(),
            Some("2026-10-20T18:00:00.1234567Z")
        );
        // Art from the newest record; the older one fills what it lacks.
        assert_eq!(game.cover.as_deref(), Some("https://img/new-poster"));
        assert_eq!(game.background.as_deref(), Some("https://img/old-hero"));
        // Both records counting time add up.
        let both = HashMap::from([
            (
                "1297290226".to_string(),
                TitleStat {
                    minutes: Some(30),
                    ..Default::default()
                },
            ),
            (
                "1889321756".to_string(),
                TitleStat {
                    minutes: Some(2),
                    ..Default::default()
                },
            ),
        ]);
        assert_eq!(merge_titles(&records, &both)[0].minutes, Some(32));
    }

    #[tokio::test]
    async fn the_account_lists_a_game_with_two_title_ids_once() {
        let dir = tempfile::tempdir().unwrap();
        let s = TestServer::start(|req| {
            let path = req.path.split('?').next().unwrap_or_default().to_string();
            let body = String::from_utf8_lossy(&req.body).to_string();
            match path.as_str() {
                "/oauth20_token.srf" => Resp::json(serde_json::json!({
                    "access_token": "access", "refresh_token": "refresh", "expires_in": 3600
                })),
                "/user/authenticate" => Resp::json(serde_json::json!({
                    "Token": "user", "DisplayClaims": {"xui": [{"uhs": "h"}]}
                })),
                "/xsts/authorize" => Resp::json(serde_json::json!({
                    "Token": "xsts", "NotAfter": "2099-01-01T00:00:00Z",
                    "DisplayClaims": {"xui": [{"uhs": "h", "xid": "25", "gtg": "Pilot"}]}
                })),
                p if p.contains("/titlehistory/") => Resp::json(two_records()),
                "/batch" if body.contains("1889321756") => {
                    Resp::json(serde_json::json!({"statlistscollection": [
                        {"stats": [{"name": "MinutesPlayed", "value": "2"}]}]}))
                }
                "/batch" => Resp::json(serde_json::json!({"statlistscollection": [{"stats": []}]})),
                p if p.ends_with("/achievements") => {
                    assert!(
                        req.path.contains("titleId=1889321756%2C1297290226"),
                        "{}",
                        req.path
                    );
                    Resp::json(serde_json::json!({"achievements": [
                        {"id": "1", "name": "Sweeper", "progressState": "NotStarted"},
                        {"id": "1", "name": "Sweeper", "progressState": "Achieved",
                         "progression": {"timeUnlocked": "2024-02-01T10:00:00Z"}},
                        {"id": "2", "name": "Expert", "progressState": "NotStarted"}
                    ]}))
                }
                _ => Resp::status(404),
            }
        });
        let xbox = account(dir.path(), &s);
        let _ = xbox.login_flow();
        xbox.login("M.C1.abcdefgh").await.unwrap();
        xbox.refresh(true, "en").await.unwrap();
        let app = format!("xbox:{MINESWEEPER}");
        assert_eq!(xbox.titles().len(), 1);
        assert_eq!(xbox.playtime(), HashMap::from([(app.clone(), 2)]));
        let list = xbox.achievements(&app, "en").await.unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().find(|a| a.id == "1").unwrap().is_unlocked());
        // Asked again with nothing played since: no minutes asked, not
        // even for the record that keeps none.
        let batches = |s: &TestServer| s.requests().iter().filter(|r| r.path == "/batch").count();
        assert_eq!(batches(&s), 2);
        xbox.refresh(true, "en").await.unwrap();
        assert_eq!(batches(&s), 2);
    }
}
