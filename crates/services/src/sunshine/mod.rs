//! Irori's games in Sunshine, the game-streaming host Moonlight connects to.
//!
//! With `iroriSunshine.enabled`, the installed games the library shows (or
//! its favourites) are kept among Sunshine's applications through Sunshine's
//! web API on this computer, each with its cover and the command
//! `irori --launch <runner>/<app> --wait`, plus one that opens Big Picture.
//! The waiting command lasts as long as the game's session, and ending it
//! (Moonlight's Quit) stops the game. Irori marks its applications with
//! [`plan::MARK`] and changes no others; whatever the user sets on them in
//! Sunshine (preparation commands, output, timeout) stays. The login to
//! Sunshine's web UI is kept with `irori_secrets`.
//!
//! A sync follows library and settings changes after a quiet moment, and
//! only talks to Sunshine when what Irori wants there differs from what it
//! last put there, or the last attempt failed.

mod client;
mod cover;
mod detect;
pub(crate) mod plan;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result};
use irori_core::{Event, GameId, Paths, json_store};
use serde::{Deserialize, Serialize};

pub use client::SunshineLogin;
pub use detect::SunshineInstall;

use crate::Backend;
use client::Client;
use plan::{Step, Wanted};

/// Changes are gathered this long before a sync.
const QUIET: Duration = Duration::from_secs(3);
/// A failed automatic sync is tried again after this long.
const RETRY: Duration = Duration::from_secs(10 * 60);

/// How the last sync went.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SunshineReport {
    /// Unix seconds.
    pub at: u64,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    /// Applications that couldn't be changed, with why.
    pub failed: Vec<String>,
    /// Why the sync couldn't run at all.
    pub error: Option<String>,
}

impl SunshineReport {
    /// When the sync ran, in local time.
    pub fn local_time(&self) -> String {
        chrono::DateTime::from_timestamp(self.at as i64, 0)
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_default()
    }
}

/// What the Sunshine panel shows.
#[derive(Debug, Clone, Default)]
pub struct SunshineStatus {
    /// Sunshine's configuration, once Sunshine ran here.
    pub install: Option<SunshineInstall>,
    /// Sunshine's program is installed, even if it never ran.
    pub program_installed: bool,
    /// The saved user name of Sunshine's web UI.
    pub username: Option<String>,
    /// The port Irori talks to.
    pub port: u16,
    /// Applications Irori keeps in Sunshine.
    pub applications: usize,
    pub last: Option<SunshineReport>,
    pub syncing: bool,
}

/// A cover Irori gave Sunshine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct CoverRecord {
    /// The art it was made from.
    url: String,
    /// Where Sunshine keeps it.
    path: String,
}

/// `state/sunshine.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct SunshineState {
    /// What the last successful sync left in Sunshine.
    synced: Vec<Wanted>,
    covers: BTreeMap<String, CoverRecord>,
    last: Option<SunshineReport>,
}

fn state_file() -> PathBuf {
    Paths::get().state_dir().join("sunshine.json")
}

fn login_file() -> PathBuf {
    Paths::get().config.join("sunshine.json")
}

/// The program Sunshine should start: the AppImage itself rather than its
/// mount, which is gone after Irori exits.
fn program() -> Option<String> {
    if let Some(appimage) = std::env::var_os("APPIMAGE").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(appimage).to_string_lossy().into_owned());
    }
    std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// The Sunshine side of the backend.
#[derive(Default)]
pub(crate) struct SunshineService {
    /// One sync at a time.
    lock: tokio::sync::Mutex<()>,
    syncing: std::sync::atomic::AtomicBool,
    /// The last automatic sync that failed, and when.
    failed: parking_lot::Mutex<Option<(Vec<Wanted>, std::time::Instant)>>,
}

impl Backend {
    /// Sunshine on this computer and how Irori's games there are doing.
    pub async fn sunshine_status(&self) -> SunshineStatus {
        let prefs = self.settings.app().sunshine.clone();
        let (install, program_installed) =
            tokio::task::spawn_blocking(|| (detect::detect(), detect::program_installed()))
                .await
                .unwrap_or_default();
        let login = self.sunshine_login().await.ok().flatten();
        let state = read_state().await;
        SunshineStatus {
            port: port(prefs.port, install.as_ref()),
            program_installed: program_installed || install.is_some(),
            install,
            username: login.map(|l| l.username),
            applications: state.synced.len(),
            last: state.last,
            syncing: self
                .sunshine
                .syncing
                .load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// The saved login of Sunshine's web UI.
    pub async fn sunshine_login(&self) -> Result<Option<SunshineLogin>> {
        tokio::task::spawn_blocking(|| irori_secrets::read_json::<SunshineLogin>(&login_file()))
            .await
            .context("Could not read Sunshine's login")?
    }

    /// Save (or with `None` forget) the login of Sunshine's web UI, then
    /// sync with it.
    pub async fn set_sunshine_login(&self, login: Option<SunshineLogin>) -> Result<()> {
        tokio::task::spawn_blocking(move || match login {
            Some(login) => irori_secrets::write_json(&login_file(), &login),
            None => match std::fs::remove_file(login_file()) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            },
        })
        .await
        .context("Could not save Sunshine's login")??;
        self.bus.emit(Event::SunshineChanged);
        if self.settings.app().sunshine.enabled {
            let _ = self.sync_sunshine().await;
        }
        Ok(())
    }

    /// Bring Sunshine's applications in line with the library now
    /// ("Sync now"), or take Irori's out of it while the setting is off.
    pub async fn sync_sunshine(&self) -> Result<SunshineReport> {
        let wanted = if self.settings.app().sunshine.enabled {
            self.sunshine_wanted()
        } else {
            Vec::new()
        };
        self.apply_to_sunshine(wanted).await
    }

    /// Take every application Irori added out of Sunshine.
    pub async fn remove_from_sunshine(&self) -> Result<SunshineReport> {
        self.apply_to_sunshine(Vec::new()).await
    }

    /// What Irori wants in Sunshine now.
    fn sunshine_wanted(&self) -> Vec<Wanted> {
        let prefs = self.settings.app().sunshine.clone();
        let Some(program) = program() else {
            return Vec::new();
        };
        let library = self.library();
        let user = self.userdata.snapshot();
        let big_picture = prefs
            .big_picture
            .then(|| plan::command_line(&program, &["--big-picture", "--wait"]))
            .flatten();
        plan::wanted(
            &library.games,
            &user.hidden,
            &user.favourites,
            prefs.games,
            big_picture,
            |id: &GameId| {
                let target = format!("{}/{}", id.runner.id(), id.app_name);
                plan::command_line(&program, &["--launch", &target, "--wait"])
            },
        )
    }

    /// Sync when what Irori wants in Sunshine differs from what it last put
    /// there (turning the setting off counts: Irori's applications leave).
    /// A sync that failed waits [`RETRY`] before the same one is tried again.
    pub(crate) async fn sync_sunshine_if_changed(&self) {
        let enabled = self.settings.app().sunshine.enabled;
        let wanted = if enabled {
            self.sunshine_wanted()
        } else {
            Vec::new()
        };
        let state = read_state().await;
        let clean = state.last.as_ref().is_none_or(|l| l.error.is_none());
        if wanted == state.synced && clean {
            return;
        }
        if !enabled && state.synced.is_empty() {
            return;
        }
        if let Some((tried, at)) = &*self.sunshine.failed.lock()
            && *tried == wanted
            && at.elapsed() < RETRY
        {
            return;
        }
        if self.sunshine_login().await.ok().flatten().is_none() {
            return;
        }
        match self.apply_to_sunshine(wanted.clone()).await {
            Ok(_) => *self.sunshine.failed.lock() = None,
            Err(e) => {
                *self.sunshine.failed.lock() = Some((wanted, std::time::Instant::now()));
                tracing::info!("syncing with Sunshine: {e:#}");
            }
        }
    }

    /// Follow the library and settings, syncing after each quiet moment.
    pub(crate) fn watch_for_sunshine(&self) {
        let b = self.clone();
        let mut rx = self.bus.subscribe();
        self.spawn(async move {
            // The first library build counts as a change.
            let mut pending = true;
            loop {
                let event = if pending {
                    match tokio::time::timeout(QUIET, rx.recv()).await {
                        Ok(event) => Some(event),
                        Err(_) => {
                            pending = false;
                            b.sync_sunshine_if_changed().await;
                            continue;
                        }
                    }
                } else {
                    Some(rx.recv().await)
                };
                match event {
                    Some(Ok(Event::LibraryUpdated(_) | Event::SettingsChanged)) => pending = true,
                    Some(Err(tokio::sync::broadcast::error::RecvError::Closed)) => break,
                    Some(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {
                        pending = true
                    }
                    _ => {}
                }
            }
        });
    }

    /// Retry a failed sync from the periodic tick; Sunshine may run now.
    pub(crate) async fn sync_sunshine_when_due(&self) {
        let due = self
            .sunshine
            .failed
            .lock()
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() >= RETRY);
        if due {
            self.sync_sunshine_if_changed().await;
        }
    }

    async fn apply_to_sunshine(&self, wanted: Vec<Wanted>) -> Result<SunshineReport> {
        let _guard = self.sunshine.lock.lock().await;
        let set = |on: bool| {
            self.sunshine
                .syncing
                .store(on, std::sync::atomic::Ordering::Relaxed);
            self.bus.emit(Event::SunshineChanged);
        };
        set(true);
        let mut state = read_state().await;
        let result = async {
            let login = self
                .sunshine_login()
                .await?
                .context("Enter the user name and password of Sunshine's web UI first")?;
            let prefs = self.settings.app().sunshine.clone();
            let install = tokio::task::spawn_blocking(detect::detect)
                .await
                .unwrap_or_default();
            let client = Client::new(port(prefs.port, install.as_ref()), login)?;
            let program = program().context("Irori couldn't find its own program")?;
            sync(&client, &wanted, &program, &mut state).await
        }
        .await;
        let report = match result {
            Ok(report) => {
                state.synced = wanted;
                report
            }
            Err(e) => SunshineReport {
                error: Some(format!("{e:#}")),
                ..Default::default()
            },
        };
        let report = SunshineReport {
            at: irori_core::util::unix_now(),
            ..report
        };
        state.last = Some(report.clone());
        let saved = state.clone();
        let _ = tokio::task::spawn_blocking(move || json_store::write(&state_file(), &saved)).await;
        set(false);
        match &report.error {
            Some(e) => Err(anyhow::anyhow!("{e}")),
            None => Ok(report),
        }
    }
}

/// The port of Sunshine's web UI: the setting, else Sunshine's own.
fn port(setting: u16, install: Option<&SunshineInstall>) -> u16 {
    if setting != 0 {
        return setting;
    }
    install.map_or(detect::web_port(""), |i| i.web_port)
}

async fn read_state() -> SunshineState {
    tokio::task::spawn_blocking(|| json_store::read_or_default::<SunshineState>(&state_file()))
        .await
        .unwrap_or_default()
}

/// Apply what `wanted` asks of Sunshine through `client`, one step at a
/// time against a fresh list. A step that fails is reported and the rest go
/// on; only reaching Sunshine at all fails the sync.
async fn sync(
    client: &Client,
    wanted: &[Wanted],
    program: &str,
    state: &mut SunshineState,
) -> Result<SunshineReport> {
    let mut report = SunshineReport::default();
    let current = client.apps().await?;
    let mut steps = plan::steps(&current, wanted, program);
    // A new cover is an update too.
    for w in wanted {
        let listed = steps.iter().any(|s| match s {
            Step::Add(x) | Step::Update(x) => x.id == w.id,
            Step::Remove(_) => false,
        });
        if !listed
            && let Some(app) = plan::find(&current, &w.id, program)
            && cover_wanted(app, w, state.covers.get(&w.id))
        {
            steps.push(Step::Update(w.clone()));
        }
    }
    for step in steps {
        let current = client.apps().await?;
        let outcome = match &step {
            Step::Add(w) => {
                let image = give_cover(client, None, w, state).await;
                client
                    .save(plan::merged(None, w, image.as_deref()), None)
                    .await
            }
            Step::Update(w) => match plan::index_of(&current, &w.id, program) {
                Some(ix) => {
                    let image = give_cover(client, Some(&current[ix]), w, state).await;
                    client
                        .save(
                            plan::merged(Some(&current[ix]), w, image.as_deref()),
                            Some(ix),
                        )
                        .await
                }
                None => Ok(()),
            },
            Step::Remove(mark) => match plan::index_of(&current, mark, program) {
                Some(ix) => client.delete(ix).await.map(|()| {
                    state.covers.remove(mark);
                }),
                None => Ok(()),
            },
        };
        match (outcome, &step) {
            (Ok(()), Step::Add(_)) => report.added += 1,
            (Ok(()), Step::Update(_)) => report.updated += 1,
            (Ok(()), Step::Remove(_)) => report.removed += 1,
            (Err(e), Step::Add(w) | Step::Update(w)) => {
                report.failed.push(format!("{}: {e:#}", w.name));
            }
            (Err(e), Step::Remove(mark)) => report.failed.push(format!("{mark}: {e:#}")),
        }
    }
    Ok(report)
}

/// Whether `app` should get Irori's cover for `w`: it has none, or the one
/// Irori gave it is out of date or gone. A picture the user chose stays.
fn cover_wanted(app: &serde_json::Value, w: &Wanted, given: Option<&CoverRecord>) -> bool {
    let Some(url) = &w.cover else { return false };
    let image = app
        .get("image-path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    match given {
        _ if image.is_empty() => true,
        Some(record) if record.path == image => &record.url != url || !Path::new(image).exists(),
        // The user's own picture.
        _ => false,
    }
}

/// Give Sunshine the cover of `w` when it wants one, and say its path. A
/// cover that can't be made leaves the application without one.
async fn give_cover(
    client: &Client,
    app: Option<&serde_json::Value>,
    w: &Wanted,
    state: &mut SunshineState,
) -> Option<String> {
    let url = w.cover.clone()?;
    let wanted = match app {
        Some(app) => cover_wanted(app, w, state.covers.get(&w.id)),
        None => true,
    };
    if !wanted {
        return None;
    }
    let made = async {
        let bytes = cover::fetch(&url).await?;
        let png = tokio::task::spawn_blocking(move || cover::to_cover(&bytes))
            .await
            .context("Could not make the cover")??;
        client.upload_cover(&plan::cover_key(&w.id), &png).await
    }
    .await;
    match made {
        Ok(path) => {
            state.covers.insert(
                w.id.clone(),
                CoverRecord {
                    url,
                    path: path.clone(),
                },
            );
            Some(path)
        }
        Err(e) => {
            tracing::info!("Sunshine cover of {}: {e:#}", w.name);
            None
        }
    }
}

#[cfg(test)]
mod tests;
