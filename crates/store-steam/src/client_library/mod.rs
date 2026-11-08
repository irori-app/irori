//! The whole Steam library without a Steam sign-in in Irori, read from the
//! running Steam client. Opt-in and off by default.
//!
//! Steam's Chromium interface offers remote debugging on a port of this
//! computer when `.cef-enable-remote-debugging` exists at its start
//! ([`marker`]). While it does, one read-only expression evaluated in the
//! client's shared page lists the games of its library with their names,
//! ownership and install state ([`devtools`], [`EXPRESSION`]). The games are
//! kept in `client-games.json` in Steam's cache folder and listed like the
//! games a signed-in account adds; a signed-in account wins over them.
//!
//! While the port is open any program on this computer can control the
//! Steam client, so the default ([`ClientLibraryMode::Once`]) reads once and
//! removes the file again (Steam closes the port at its next start);
//! [`ClientLibraryMode::Keep`] leaves it and reads again on library
//! refreshes. What is read never leaves this computer, and only counts are
//! logged.

mod devtools;
mod marker;

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use irori_core::{Event, Runner, json_store};
use serde::{Deserialize, Serialize};

use crate::SteamStore;
use crate::account::{LibraryGame, NOT_GAMES};

/// How long Irori waits for Steam to come up with its library after
/// starting it.
const START_TIMEOUT: Duration = Duration::from_secs(120);

/// Between looks while Steam starts.
const START_POLL: Duration = Duration::from_secs(2);

/// [`ClientLibraryMode::Keep`] reads again on a refresh at most this often.
const READ_MAX_AGE: Duration = Duration::from_secs(15 * 60);

/// SteamID64 of the account with account ID 0.
const STEAMID64_BASE: u64 = 76561197960265728;

/// What Irori does with Steam's debugging switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClientLibraryMode {
    /// Irori doesn't read the client.
    #[default]
    Off,
    /// Read once, then remove the file Irori created.
    Once,
    /// Keep the file and read again on library refreshes.
    Keep,
}

/// Why the client couldn't be read. In the error chain of the reading
/// methods, so screens can explain and offer what helps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientLibraryError {
    /// The Steam client doesn't run.
    NotRunning,
    /// Steam runs without remote debugging: it has to start again with the
    /// file in place. `millennium` when Millennium looks installed, which
    /// manages the port itself.
    DebuggingOff { millennium: bool },
    /// Something other than Steam answers on the port.
    NotSteam { port: u16 },
    /// Steam's interface or library hasn't loaded yet (still starting, or
    /// signed out of Steam).
    Starting,
    /// This Steam client keeps its library somewhere Irori doesn't know.
    Unsupported,
    /// Steam runs the session (Game Mode), so Irori doesn't restart it.
    SteamSession,
}

impl fmt::Display for ClientLibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => f.write_str("Steam isn't running."),
            Self::DebuggingOff { millennium: false } => {
                f.write_str("Steam runs without remote debugging. Restart Steam to turn it on.")
            }
            Self::DebuggingOff { millennium: true } => f.write_str(
                "Steam runs without a debugging port Irori can find. Millennium manages Steam's debugging port; check its settings.",
            ),
            Self::NotSteam { port } => write!(
                f,
                "Another program uses port {port}, the one Steam's debugging needs."
            ),
            Self::Starting => {
                f.write_str("Steam hasn't loaded your library yet. Sign in to Steam and try again.")
            }
            Self::Unsupported => {
                f.write_str("This version of Steam keeps its library where Irori can't read it.")
            }
            Self::SteamSession => f.write_str("Steam runs this session, so Irori can't restart it."),
        }
    }
}

impl std::error::Error for ClientLibraryError {}

/// What the Steam card shows about reading the client.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientLibraryState {
    pub mode: ClientLibraryMode,
    /// The debugging file is in Steam's folder.
    pub marker: bool,
    /// Irori created that file (and may remove it).
    pub own_marker: bool,
    /// Games read from the client and kept.
    pub games: usize,
    /// When they were read (Unix time).
    pub read_at: Option<u64>,
    /// The user put the library's suggestion away.
    pub hint_dismissed: bool,
    /// Irori may restart Steam (not in Game Mode).
    pub can_restart_steam: bool,
    /// A Steam account is signed in to Irori; its library wins.
    pub signed_in: bool,
    /// Steam's folder exists on this computer.
    pub steam_found: bool,
}

impl ClientLibraryState {
    /// Whether the library should suggest reading the client: Steam is
    /// here, no account is signed in, nothing was read this way, the
    /// feature is off and the user didn't put the suggestion away.
    pub fn suggest(&self) -> bool {
        self.steam_found
            && !self.signed_in
            && self.read_at.is_none()
            && self.mode == ClientLibraryMode::Off
            && !self.hint_dismissed
            && self.can_restart_steam
    }
}

/// `client-library.json`: the user's choice.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ClientSettings {
    pub mode: ClientLibraryMode,
    pub own_marker: bool,
    pub hint_dismissed: bool,
}

/// `client-games.json`: what was read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ClientCache {
    /// Unix time; 0 when never read.
    pub read_at: u64,
    pub games: Vec<LibraryGame>,
}

/// Where the feature keeps its files.
#[derive(Debug, Clone)]
pub(crate) struct ClientFiles {
    settings: PathBuf,
    cache: PathBuf,
}

impl ClientFiles {
    pub fn new(config: &Path, cache: &Path) -> Self {
        Self {
            settings: config.join("client-library.json"),
            cache: cache.join("client-games.json"),
        }
    }

    pub fn read_settings(&self) -> ClientSettings {
        json_store::read_or_default(&self.settings)
    }

    pub fn write_settings(&self, settings: &ClientSettings) -> Result<()> {
        json_store::write(&self.settings, settings)
    }

    pub fn read_cache(&self) -> ClientCache {
        json_store::read_or_default(&self.cache)
    }

    pub fn write_cache(&self, cache: &ClientCache) -> Result<()> {
        json_store::write(&self.cache, cache)
    }
}

/// Evaluated in Steam's shared page. It only reads the client's library
/// collection (`collectionStore.allGamesCollection.allApps`, app overviews)
/// and calls nothing but its ownership getters. Every step checks what it
/// finds, so a client that moved things answers `missing` instead of
/// throwing.
pub(crate) const EXPRESSION: &str = r#"(() => {
  const cs = globalThis.collectionStore;
  const all = cs && (cs.allGamesCollection || cs.allAppsCollection);
  const apps = all && all.allApps;
  if (!Array.isArray(apps)) {
    const started = document.readyState === "complete" && !!globalThis.appStore;
    return { v: 1, state: cs || started ? "missing" : "loading" };
  }
  const ask = (a, f) => (typeof a[f] === "function" ? !!a[f]() : null);
  const out = [];
  for (const a of apps) {
    if (!a || typeof a.appid !== "number" || a.app_type !== 1) continue;
    if (ask(a, "BIsShortcut")) continue;
    const local = a.local_per_client_data || {};
    out.push({
      id: a.appid,
      name: String(a.display_name || ""),
      owned: ask(a, "BIsOwned"),
      borrowed: ask(a, "BIsBorrowed"),
      owner: typeof a.owner_account_id === "number" ? a.owner_account_id : null,
      installed: local.installed === true,
      minutes: Number(a.minutes_playtime_forever) || 0,
      lastPlayed: Number(a.rt_last_time_played) || 0,
    });
  }
  return { v: 1, state: apps.length ? "ready" : "loading", apps: out };
})()"#;

/// One app as [`EXPRESSION`] lists it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientApp {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    owned: Option<bool>,
    #[serde(default)]
    borrowed: Option<bool>,
    #[serde(default)]
    owner: Option<u64>,
    #[serde(default)]
    minutes: f64,
    #[serde(default)]
    last_played: f64,
}

#[derive(Debug, Deserialize)]
struct Answer {
    v: u32,
    state: String,
    #[serde(default)]
    apps: Vec<ClientApp>,
}

/// The games in [`EXPRESSION`]'s value: the account's own and those its
/// family lends, without Steam's own apps and non-Steam shortcuts, by app
/// ID.
pub(crate) fn parse_answer(
    value: serde_json::Value,
) -> Result<Vec<LibraryGame>, ClientLibraryError> {
    let answer: Answer =
        serde_json::from_value(value).map_err(|_| ClientLibraryError::Unsupported)?;
    if answer.v != 1 {
        return Err(ClientLibraryError::Unsupported);
    }
    match answer.state.as_str() {
        "ready" => {}
        "loading" => return Err(ClientLibraryError::Starting),
        _ => return Err(ClientLibraryError::Unsupported),
    }
    let mut seen = HashSet::new();
    let mut games: Vec<LibraryGame> = answer
        .apps
        .into_iter()
        .filter(|a| a.id > 0 && a.id < 1 << 31)
        .filter(|a| !NOT_GAMES.contains(&(a.id as u32)))
        .filter(|a| !a.name.trim().is_empty())
        // Apps in the library the account doesn't own nor borrow (free
        // weekends, removed licences) can't be played.
        .filter(|a| a.owned != Some(false) || a.borrowed == Some(true))
        .filter(|a| seen.insert(a.id))
        .map(|a| {
            let borrowed = a.borrowed == Some(true);
            LibraryGame {
                app_id: a.id.to_string(),
                name: a.name.trim().to_string(),
                playtime_minutes: if borrowed {
                    0
                } else {
                    a.minutes.max(0.) as u64
                },
                last_played: a.last_played.max(0.) as u64,
                shared_by: if borrowed {
                    a.owner
                        .filter(|o| *o > 0 && *o < 1 << 32)
                        .map(|o| (STEAMID64_BASE + o).to_string())
                } else {
                    None
                },
            }
        })
        .collect();
    games.sort_by(|a, b| a.app_id.cmp(&b.app_id));
    Ok(games)
}

/// Read the client's library once: find its debugging port, the shared
/// page, and evaluate [`EXPRESSION`] there.
async fn read_once(steam_dir: &Path) -> Result<Vec<LibraryGame>> {
    let probe = devtools::probe().await?;
    if !probe.running {
        return Err(ClientLibraryError::NotRunning.into());
    }
    let port = probe.port.unwrap_or(devtools::DEFAULT_PORT);
    let Some(list) = devtools::list(port).await? else {
        let dir = steam_dir.to_path_buf();
        let millennium = tokio::task::spawn_blocking(move || marker::millennium_installed(&dir))
            .await
            .unwrap_or(false);
        return Err(ClientLibraryError::DebuggingOff { millennium }.into());
    };
    read_page_list(&list, port).await
}

/// Evaluate [`EXPRESSION`] in the shared page that `list`, the page list of
/// the endpoint on `port`, names.
async fn read_page_list(list: &str, port: u16) -> Result<Vec<LibraryGame>> {
    let websocket = devtools::shared_context(list, port)?;
    let value = devtools::evaluate(&websocket, EXPRESSION).await?;
    Ok(parse_answer(value)?)
}

/// Read the client on `port`, which must answer (tests).
#[cfg(test)]
async fn read_port(port: u16) -> Result<Vec<LibraryGame>> {
    let list = devtools::list(port).await?.context("nothing listens")?;
    read_page_list(&list, port).await
}

/// Whether trying again after a moment may succeed while Steam starts.
fn worth_waiting(e: &anyhow::Error) -> bool {
    match e.downcast_ref::<ClientLibraryError>() {
        Some(ClientLibraryError::NotRunning)
        | Some(ClientLibraryError::DebuggingOff { .. })
        | Some(ClientLibraryError::Starting) => true,
        Some(_) => false,
        // A port that closes or resets while Steam's interface comes up.
        None => e
            .chain()
            .any(|c| c.downcast_ref::<std::io::Error>().is_some()),
    }
}

impl SteamStore {
    /// What reading the Steam client stands at. Reads memory only.
    pub fn client_library_state(&self) -> ClientLibraryState {
        let settings = self.client_settings.read().clone();
        let cache = self.client_cache.read();
        ClientLibraryState {
            mode: settings.mode,
            marker: *self.client_marker.read(),
            own_marker: settings.own_marker,
            games: cache.games.len(),
            read_at: (cache.read_at > 0).then_some(cache.read_at),
            hint_dismissed: settings.hint_dismissed,
            can_restart_steam: !irori_core::platform::in_steam_gamepad_ui(),
            signed_in: self.session.read().is_some(),
            steam_found: self.library.read().found,
        }
    }

    /// Load the feature's files and look for the debugging file.
    pub(crate) async fn load_client_library(&self) -> Result<()> {
        let (files, dir) = (self.client_files.clone(), self.steam_dir());
        let (settings, cache, present) = tokio::task::spawn_blocking(move || {
            (
                files.read_settings(),
                files.read_cache(),
                marker::exists(&dir),
            )
        })
        .await?;
        *self.client_marker.write() = present;
        *self.client_settings.write() = settings;
        *self.client_cache.write() = Arc::new(cache);
        Ok(())
    }

    /// The games read from the client, unless a signed-in account lists
    /// the library itself.
    pub(crate) fn client_games(&self) -> Arc<ClientCache> {
        if self.session.read().is_some() {
            return Arc::default();
        }
        self.client_cache.read().clone()
    }

    async fn save_client_settings(&self, settings: ClientSettings) -> Result<()> {
        let files = self.client_files.clone();
        let to_write = settings.clone();
        tokio::task::spawn_blocking(move || files.write_settings(&to_write)).await??;
        *self.client_settings.write() = settings;
        Ok(())
    }

    /// Turn reading on in `mode`: put the debugging file in Steam's folder
    /// unless it is there. Steam reads it at its next start.
    pub async fn enable_client_library(&self, mode: ClientLibraryMode) -> Result<()> {
        let dir = self.steam_dir();
        let created = tokio::task::spawn_blocking(move || marker::create(&dir)).await??;
        let mut settings = self.client_settings.read().clone();
        settings.mode = mode;
        settings.own_marker |= created;
        // The user found the feature; the library needn't suggest it again.
        settings.hint_dismissed = true;
        self.save_client_settings(settings).await?;
        *self.client_marker.write() = true;
        if created {
            tracing::info!("turned on the Steam client's remote debugging for its next start");
        }
        self.bus.emit(Event::AccountChanged(Runner::Steam));
        Ok(())
    }

    /// Turn reading off and remove the debugging file Irori created (Steam
    /// closes the port at its next start). A file another program put
    /// there stays. The games already read stay listed.
    pub async fn disable_client_library(&self) -> Result<()> {
        let settings = self.client_settings.read().clone();
        if settings.own_marker {
            let dir = self.steam_dir();
            tokio::task::spawn_blocking(move || marker::remove(&dir)).await??;
            tracing::info!("removed the Steam client's remote debugging file");
        }
        let dir = self.steam_dir();
        let present = tokio::task::spawn_blocking(move || marker::exists(&dir)).await?;
        self.save_client_settings(ClientSettings {
            mode: ClientLibraryMode::Off,
            own_marker: false,
            ..settings
        })
        .await?;
        *self.client_marker.write() = present;
        self.bus.emit(Event::AccountChanged(Runner::Steam));
        Ok(())
    }

    /// Forget the games read from the client; installed games stay.
    pub async fn forget_client_library(&self) -> Result<()> {
        let files = self.client_files.clone();
        tokio::task::spawn_blocking(move || files.write_cache(&ClientCache::default())).await??;
        *self.client_cache.write() = Arc::default();
        if self.scan().await? {
            self.bus.emit(Event::LibraryChanged(Runner::Steam));
        }
        self.bus.emit(Event::AccountChanged(Runner::Steam));
        Ok(())
    }

    /// Stop suggesting the feature in the library.
    pub async fn dismiss_client_library_hint(&self) -> Result<()> {
        let settings = ClientSettings {
            hint_dismissed: true,
            ..self.client_settings.read().clone()
        };
        self.save_client_settings(settings).await?;
        self.bus.emit(Event::AccountChanged(Runner::Steam));
        Ok(())
    }

    /// Read the library from the Steam client now and list it. When Steam
    /// isn't running it is started; when it runs without remote debugging
    /// it is restarted if `restart` (games Steam runs close with it), else
    /// [`ClientLibraryError::DebuggingOff`] comes back so the user can be
    /// asked. Waits for Steam to come up. Returns how many games were read.
    pub async fn read_client_library(&self, restart: bool) -> Result<usize> {
        let _one = self.client_reading.lock().await;
        let dir = self.steam_dir();
        let probe = devtools::probe().await?;
        let games = if !probe.running {
            self.start_steam().await?;
            self.wait_for_client(&dir).await?
        } else {
            match read_once(&dir).await {
                Ok(games) => games,
                Err(e) => match e.downcast_ref::<ClientLibraryError>() {
                    Some(ClientLibraryError::DebuggingOff { .. }) if restart => {
                        if irori_core::platform::in_steam_gamepad_ui() {
                            return Err(ClientLibraryError::SteamSession.into());
                        }
                        crate::compat::close_steam(&dir).await?;
                        self.start_steam().await?;
                        self.wait_for_client(&dir).await?
                    }
                    Some(ClientLibraryError::Starting) => self.wait_for_client(&dir).await?,
                    _ => return Err(e),
                },
            }
        };
        self.keep_client_games(games).await
    }

    /// Start the Steam client (its window opens on the library).
    async fn start_steam(&self) -> Result<()> {
        let command = crate::client::open_url("steam://open/games");
        irori_core::process::run(&command, Default::default())
            .await
            .context("Steam didn't start")?
            .check()
            .context("Steam didn't start")?;
        Ok(())
    }

    /// Read the client once it has come up, trying every [`START_POLL`]
    /// for [`START_TIMEOUT`].
    async fn wait_for_client(&self, dir: &Path) -> Result<Vec<LibraryGame>> {
        let deadline = tokio::time::Instant::now() + START_TIMEOUT;
        loop {
            tokio::time::sleep(START_POLL).await;
            match read_once(dir).await {
                Ok(games) => return Ok(games),
                Err(e) if worth_waiting(&e) && tokio::time::Instant::now() < deadline => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// Keep what was read, list it, and in [`ClientLibraryMode::Once`] remove
    /// the debugging file Irori created.
    async fn keep_client_games(&self, games: Vec<LibraryGame>) -> Result<usize> {
        let count = games.len();
        let cache = ClientCache {
            read_at: irori_core::util::unix_now(),
            games,
        };
        let files = self.client_files.clone();
        let to_write = cache.clone();
        tokio::task::spawn_blocking(move || files.write_cache(&to_write)).await??;
        *self.client_cache.write() = Arc::new(cache);
        tracing::info!("read {count} games from the Steam client");
        if self.client_settings.read().mode != ClientLibraryMode::Keep {
            self.disable_client_library().await?;
        }
        if self.scan().await? {
            self.bus.emit(Event::LibraryChanged(Runner::Steam));
        }
        self.bus.emit(Event::AccountChanged(Runner::Steam));
        Ok(count)
    }

    /// On a library refresh: read the client again when the user turned
    /// reading on, no account is signed in and Steam already offers its
    /// port. Never starts or restarts Steam. Reports whether the games
    /// changed.
    pub(crate) async fn refresh_client_library(&self, full: bool) -> bool {
        let mode = self.client_settings.read().mode;
        if mode == ClientLibraryMode::Off || self.session.read().is_some() {
            return false;
        }
        let read_at = self.client_cache.read().read_at;
        let age = irori_core::util::unix_now().saturating_sub(read_at);
        if mode == ClientLibraryMode::Keep && !full && read_at > 0 && age < READ_MAX_AGE.as_secs() {
            return false;
        }
        let Ok(_one) = self.client_reading.try_lock() else {
            return false;
        };
        let before = self.client_cache.read().games.clone();
        match read_once(&self.steam_dir()).await {
            Ok(games) => {
                let changed = games != before;
                if let Err(e) = self.keep_client_games(games).await {
                    tracing::warn!("keeping the Steam client's library: {e:#}");
                }
                changed
            }
            Err(e) => {
                tracing::debug!("reading the Steam client's library: {e:#}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests;
