//! Save backups with Ludusavi for every game, whichever store it comes from:
//! matching a game to Ludusavi's title, pointing Ludusavi at the game's real
//! Wine prefix, backing up on request and after a session, listing,
//! naming, locking and deleting the backups, restoring one into the game's
//! current prefix ([`restore`]) and undoing a restore ([`undo`]).
//!
//! Store cloud saves keep working beside it. They never run at the same
//! time: after a session the cloud upload finishes before Ludusavi reads the
//! files, both hold the game's busy status, and [`SavesKeeper`] tells the UI
//! which of them handles a game.

pub mod client;
mod mapping;
mod names;
pub mod restore;
mod undo;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::process::CancellationToken;
use irori_core::{
    GameId, GameInfo, HostPlatform, InstallPlatform, Notice, NoticeLevel, Runner, StatusKind,
};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::Value;

pub use client::{Change, INSTALL_URL, Ludusavi, Retention, Source, TitleMatch, TitleQuery};
pub use restore::FileKind;

use crate::Backend;

/// Settings key of the title the user matched a game to.
pub const TITLE_KEY: &str = "iroriLudusaviTitle";

/// Automatic matches found this session, by game key; `None` when Ludusavi
/// doesn't know the game. Matching reads Ludusavi's whole data set, so it is
/// done once per game.
static MATCHES: Lazy<Mutex<HashMap<String, Option<String>>>> = Lazy::new(Default::default);

/// Names the wider search found ([`Backend::ludusavi_fallback`]), kept in
/// `<cache>/ludusavi-names.json` by game key apart from the user's choice.
/// Each remembers the title it was found for, so a renamed game searches
/// again.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct FoundName {
    for_title: String,
    name: String,
}

/// Serialises reads and writes of the found names file.
static FOUND_FILE: Mutex<()> = parking_lot::const_mutex(());

fn found_names_file() -> PathBuf {
    irori_core::Paths::get().cache.join("ludusavi-names.json")
}

/// The name found earlier for `key` while its title was `title`.
fn found_name(key: &str, title: &str) -> Option<String> {
    let _guard = FOUND_FILE.lock();
    let names: HashMap<String, FoundName> =
        irori_core::json_store::read_or_default(&found_names_file());
    names
        .get(key)
        .filter(|f| f.for_title == title)
        .map(|f| f.name.clone())
}

/// Forget this session's matches, as a restart does.
#[cfg(test)]
pub(crate) fn forget_matches_for_test() {
    MATCHES.lock().clear();
}

fn remember_found_name(key: &str, title: &str, name: &str) {
    let _guard = FOUND_FILE.lock();
    let file = found_names_file();
    let mut names: HashMap<String, FoundName> = irori_core::json_store::read_or_default(&file);
    names.insert(
        key.to_string(),
        FoundName {
            for_title: title.to_string(),
            name: name.to_string(),
        },
    );
    if let Err(e) = irori_core::json_store::write(&file, &names) {
        tracing::warn!("keeping Ludusavi's name of {key}: {e:#}");
    }
}

/// The installed Ludusavi, for the settings page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LudusaviInfo {
    /// How it is started (its path, or the Flatpak command).
    pub command: String,
    pub source: Source,
    /// Its version; `None` when it didn't answer.
    pub version: Option<String>,
}

/// Who keeps a game's saves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavesKeeper {
    /// The store whose cloud saves the game uses, and whether they sync
    /// around every session.
    pub cloud: Option<(Runner, bool)>,
    /// Ludusavi backs the saves up after every session.
    pub ludusavi_after_session: bool,
}

/// The backups of one game.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GameBackups {
    /// The game's title in Ludusavi; `None` when it has none.
    pub title: Option<String>,
    /// The user chose the title rather than the automatic match.
    pub chosen: bool,
    /// The prefix Ludusavi looks in, for Windows games run with Wine.
    pub wine_prefix: Option<PathBuf>,
    /// The game's folder of backups, once there is one.
    pub folder: Option<PathBuf>,
    /// Newest first.
    pub backups: Vec<BackupItem>,
    /// The latest restore that can be undone: when (RFC 3339) and which
    /// backup.
    pub undo: Option<(String, String)>,
}

/// One backup, as listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupItem {
    /// Ludusavi's id of the backup.
    pub id: String,
    /// When it was made, RFC 3339 in UTC.
    pub when: String,
    pub bytes: u64,
    pub differential: bool,
    pub locked: bool,
    pub comment: Option<String>,
}

/// What a backup or restore did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavesDone {
    pub title: String,
    pub files: usize,
    pub bytes: u64,
    /// Files and registry keys that couldn't be copied.
    pub failed: usize,
    /// Backing up: whether anything changed since the last backup.
    pub change: Change,
}

/// What a restore would write, asked before it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorePlan {
    pub files: usize,
    pub bytes: u64,
    /// The saves came from another Wine prefix; they go into the game's
    /// current one.
    pub moved_from: Option<PathBuf>,
    /// The game's current Wine prefix, when it has one.
    pub into: Option<PathBuf>,
    /// The backup holds the prefix's registry files.
    pub has_registry: bool,
    /// Files left out: the registry (unless asked for), another prefix's
    /// files, Windows paths with no place here.
    pub left_out_registry: usize,
    pub left_out_other_prefix: usize,
    pub left_out_no_place: usize,
}

/// Where a game's restores keep what they replaced.
fn undo_dir(id: &GameId) -> PathBuf {
    irori_core::Paths::get()
        .data
        .join("save-restore-undo")
        .join(irori_core::paths::sanitize(&id.key()))
}

/// The retention set in the app settings.
fn retention(app: &irori_core::AppSettings) -> Retention {
    Retention {
        full: u8::try_from(app.ludusavi_full_limit)
            .ok()
            .filter(|n| *n > 0),
        differential: app.ludusavi_differential_limit.trim().parse().ok(),
    }
}

/// The folder Ludusavi should search as `prefix`: the one holding `drive_c`.
/// Proton run directly keeps it in `<prefix>/pfx`; Wine, CrossOver and umu
/// keep it in the prefix itself (umu adds a `pfx` link back to it). `None`
/// until the prefix has been created.
pub fn wine_prefix_root(prefix: &Path) -> Option<PathBuf> {
    let has_drive = |dir: &Path| dir.join("drive_c").is_dir();
    if has_drive(prefix) {
        Some(prefix.to_path_buf())
    } else {
        let pfx = prefix.join("pfx");
        has_drive(&pfx).then_some(pfx)
    }
}

/// What to look `game` up by: its Steam or GOG id, then its title.
fn query_for(game: &GameInfo) -> TitleQuery {
    let id = |runner| (game.runner() == runner).then_some(game.app_name.as_str());
    TitleQuery {
        steam_id: id(Runner::Steam).and_then(|a| a.parse().ok()),
        gog_id: id(Runner::Gog).and_then(|a| a.parse().ok()),
        names: vec![game.title.clone()],
        normalized: true,
        ..Default::default()
    }
}

impl Backend {
    /// The Ludusavi to run: the program chosen in settings, else the one
    /// installed. Looks at the disk, or asks the host inside a Flatpak.
    pub async fn ludusavi(&self) -> Option<Ludusavi> {
        let chosen = self.settings.app().ludusavi_path.clone();
        Ludusavi::locate(&chosen).await
    }

    async fn require_ludusavi(&self) -> Result<Ludusavi> {
        self.ludusavi().await.context(
            "Ludusavi isn’t installed. Install it, or choose its program in Settings → Saves.",
        )
    }

    /// The installed Ludusavi and its version; `None` when it isn't found.
    pub async fn ludusavi_info(&self) -> Option<LudusaviInfo> {
        let ludusavi = self.ludusavi().await?;
        let version = match ludusavi.version().await {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("asking Ludusavi its version: {e:#}");
                None
            }
        };
        Some(LudusaviInfo {
            command: ludusavi.display(),
            source: ludusavi.source,
            version,
        })
    }

    /// The backup folder chosen in settings; `None` uses Ludusavi's own.
    fn ludusavi_folder(&self) -> Option<PathBuf> {
        let folder = self.settings.app().ludusavi_backup_path.clone();
        (!folder.trim().is_empty()).then(|| irori_core::paths::expand_home(&folder))
    }

    /// Where Ludusavi keeps the backups: the folder chosen in settings,
    /// else the one in Ludusavi's own configuration. `None` when Ludusavi
    /// isn't installed or doesn't say.
    pub(crate) async fn ludusavi_backup_root(&self) -> Option<PathBuf> {
        if let Some(folder) = self.ludusavi_folder() {
            return Some(folder);
        }
        let config = self.ludusavi().await?.config_path().await.ok()?;
        let text = tokio::fs::read_to_string(&config).await.ok()?;
        restore::backup_path(&text)
    }

    /// The Wine prefix Ludusavi should search for `game`'s saves: its Proton
    /// or Wine prefix as Irori runs it, or Steam's for a Steam game. `None`
    /// for games that run natively, and before the prefix exists.
    pub fn ludusavi_wine_prefix(&self, game: &GameInfo) -> Option<PathBuf> {
        let platform = game
            .installed_platform()
            .unwrap_or(InstallPlatform::Windows);
        if !HostPlatform::current().needs_wine(platform) || game.browser_url.is_some() {
            return None;
        }
        let id = game.id();
        let prefix = if id.runner == Runner::Steam {
            irori_runtime::protontricks::prefix_of(&game.install_path()?, &game.app_name)?
        } else {
            let app = self.settings.app();
            irori_runtime::prefix_path(&app, &self.settings.game(&id), game)
        };
        wine_prefix_root(&prefix)
    }

    /// Who keeps `id`'s saves: its store's cloud saves, Ludusavi after each
    /// session, both or neither.
    pub fn saves_keeper(&self, id: &GameId) -> SavesKeeper {
        let settings = self.settings.game(id);
        let cloud = self.game(id).and_then(|game| {
            let store = self.store(id.runner)?;
            (store.supports_cloud_saves() && game.cloud_save_enabled)
                .then_some((id.runner, settings.auto_sync_saves))
        });
        SavesKeeper {
            cloud,
            ludusavi_after_session: settings.ludusavi_auto_backup,
        }
    }

    /// `id`'s title in Ludusavi: the one the user chose, else the automatic
    /// match (Steam id, GOG id, then title, then [`Self::ludusavi_fallback`]).
    /// `Ok(None)` when Ludusavi doesn't know the game.
    pub async fn ludusavi_title(&self, id: &GameId) -> Result<Option<String>> {
        let chosen = self.settings.game(id).ludusavi_title;
        if !chosen.trim().is_empty() {
            return Ok(Some(chosen));
        }
        if let Some(found) = MATCHES.lock().get(&id.key()) {
            return Ok(found.clone());
        }
        let game = self.game(id).context("The game is not in the library")?;
        let ludusavi = self.require_ludusavi().await?;
        let mut found = ludusavi
            .find(&query_for(&game))
            .await?
            .into_iter()
            .next()
            .map(|m| m.title);
        if found.is_none() {
            found = match found_name(&id.key(), &game.title) {
                Some(name) => Some(name),
                None => {
                    let name = self.ludusavi_fallback(&ludusavi, &game).await?;
                    // Kept apart from the user's choice, so the wider
                    // search runs once.
                    if let Some(name) = &name {
                        remember_found_name(&id.key(), &game.title, name);
                    }
                    name
                }
            };
        }
        MATCHES.lock().insert(id.key(), found.clone());
        Ok(found)
    }

    /// The names tried when `game`'s ids and title find nothing: the title
    /// without its region or revision tags, then, for a game played in an
    /// emulator, the name an emulator manifest gives it (`… (PS2)`,
    /// `… (RetroArch)`), taken from Ludusavi's fuzzy search.
    async fn ludusavi_fallback(
        &self,
        ludusavi: &Ludusavi,
        game: &GameInfo,
    ) -> Result<Option<String>> {
        if let Some(untagged) = names::untagged_title(game) {
            let query = TitleQuery {
                names: vec![untagged],
                normalized: true,
                ..Default::default()
            };
            if let Some(m) = ludusavi.find(&query).await?.into_iter().next() {
                return Ok(Some(m.title));
            }
        }
        let settings = self.settings.game(&game.id());
        let engine = match &game.system {
            _ if !settings.engine.trim().is_empty() => Some(settings.engine.clone()),
            Some(system) => self.settings.app().system_engines.get(system).cloned(),
            None => return Ok(None),
        };
        let retroarch = engine.map(|e| e.starts_with("retroarch"));
        let bases = names::emulated_bases(game);
        for base in &bases {
            let query = TitleQuery {
                names: vec![base.clone()],
                fuzzy: true,
                multiple: true,
                ..Default::default()
            };
            let found = ludusavi.find(&query).await?;
            if let Some(title) = names::pick_emulated(&found, &bases, retroarch) {
                return Ok(Some(title));
            }
        }
        Ok(None)
    }

    /// Titles of Ludusavi's data set matching what the user typed, best first.
    pub async fn ludusavi_search(&self, text: &str) -> Result<Vec<TitleMatch>> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let mut found = self
            .require_ludusavi()
            .await?
            .find(&TitleQuery {
                names: vec![text.to_string()],
                fuzzy: true,
                multiple: true,
                ..Default::default()
            })
            .await?;
        found.truncate(SEARCH_RESULTS);
        Ok(found)
    }

    /// Match `id` to `title` in Ludusavi, or back to the automatic match
    /// with `None`.
    pub fn set_ludusavi_title(&self, id: &GameId, title: Option<&str>) {
        let value = title
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| Value::String(t.to_string()));
        self.settings.set_game_value(id, TITLE_KEY, value);
        MATCHES.lock().remove(&id.key());
    }

    /// The match, prefix and backups of `id`.
    pub async fn ludusavi_backups(&self, id: &GameId) -> Result<GameBackups> {
        let game = self.game(id).context("The game is not in the library")?;
        let ludusavi = self.require_ludusavi().await?;
        let chosen = !self.settings.game(id).ludusavi_title.trim().is_empty();
        let wine_prefix = self.ludusavi_wine_prefix(&game);
        let Some(title) = self.ludusavi_title(id).await? else {
            return Ok(GameBackups {
                wine_prefix,
                ..Default::default()
            });
        };
        let stored = ludusavi
            .backups(&title, self.ludusavi_folder().as_deref())
            .await?;
        let (folder, backups) = match stored {
            Some(stored) => {
                let folder = stored.folder.clone();
                let items = self
                    .spawn_blocking(move || {
                        stored
                            .backups
                            .iter()
                            .rev()
                            .map(|b| BackupItem {
                                id: b.name.clone(),
                                when: b.when.clone(),
                                bytes: b.size_on_disk(&stored.folder),
                                differential: b.is_differential(),
                                locked: b.locked,
                                comment: b.comment.clone(),
                            })
                            .collect()
                    })
                    .await
                    .context("Couldn’t measure the backups")?;
                (Some(folder), items)
            }
            None => (None, Vec::new()),
        };
        let undo_at = undo_dir(id);
        let undo = self
            .spawn_blocking(move || undo::latest(&undo_at))
            .await
            .ok()
            .flatten();
        Ok(GameBackups {
            title: Some(title),
            chosen,
            wine_prefix,
            folder,
            backups,
            undo,
        })
    }

    /// Back up `id`'s saves with Ludusavi now.
    pub async fn back_up_saves(&self, id: &GameId) -> Result<SavesDone> {
        if self.statuses.is_busy(id) && !self.is_running(id) {
            bail!("another operation is in progress");
        }
        let _guard =
            (!self.is_running(id)).then(|| self.statuses.guard(id, StatusKind::SyncingSaves));
        self.back_up_saves_inner(id).await
    }

    async fn back_up_saves_inner(&self, id: &GameId) -> Result<SavesDone> {
        let game = self.game(id).context("The game is not in the library")?;
        let ludusavi = self.require_ludusavi().await?;
        let title = self
            .ludusavi_title(id)
            .await?
            .with_context(|| no_match(&game.title))?;
        let prefix = self.ludusavi_wine_prefix(&game);
        let op = ludusavi
            .backup(
                &title,
                prefix.as_deref(),
                self.ludusavi_folder().as_deref(),
                retention(&self.settings.app()),
                CancellationToken::new(),
            )
            .await?;
        if op.unknown {
            bail!(no_match(&game.title));
        }
        if !op.found_saves() {
            bail!(
                "Ludusavi found no saves of {} ({title}){}",
                game.title,
                prefix
                    .map(|p| format!(", also in the Wine prefix {}", p.display()))
                    .unwrap_or_default()
            );
        }
        let failed = op.failed().count() + op.registry_failed;
        if failed > 0 {
            for f in op.failed() {
                tracing::warn!(
                    "Ludusavi couldn’t back up {}: {}",
                    f.path,
                    f.error.as_deref().unwrap_or("unknown error")
                );
            }
        }
        Ok(SavesDone {
            title,
            files: op.files.iter().filter(|f| !f.ignored).count(),
            bytes: op.bytes(),
            failed,
            change: op.change,
        })
    }

    /// Where restoring `backup` of `id` would write: Ludusavi's preview of
    /// it, retargeted into the game's current Wine prefix. `registry`
    /// includes the prefix's registry files.
    async fn restore_plan(
        &self,
        id: &GameId,
        backup: &str,
        registry: bool,
    ) -> Result<(Ludusavi, String, restore::Plan, Option<PathBuf>)> {
        let game = self.game(id).context("The game is not in the library")?;
        let ludusavi = self.require_ludusavi().await?;
        let title = self
            .ludusavi_title(id)
            .await?
            .with_context(|| no_match(&game.title))?;
        let op = ludusavi
            .restore(
                &title,
                Some(backup),
                self.ludusavi_folder().as_deref(),
                true,
                None,
            )
            .await?;
        if op.unknown || op.title.is_none() {
            bail!("Ludusavi has no backup {backup} of {}", game.title);
        }
        let files: Vec<(String, u64)> = op
            .files
            .iter()
            .filter(|f| !f.ignored)
            .map(|f| (f.path.clone(), f.bytes))
            .collect();
        let current = self.ludusavi_wine_prefix(&game);
        let user = current.as_deref().and_then(restore::wine_user);
        let plan = restore::plan(
            &files,
            current.as_deref(),
            user.as_deref(),
            cfg!(windows),
            registry,
        );
        Ok((ludusavi, title, plan, current))
    }

    /// What restoring `backup` of `id` would write, and where. `registry`
    /// includes the Wine prefix's registry files.
    pub async fn preview_restore(
        &self,
        id: &GameId,
        backup: &str,
        registry: bool,
    ) -> Result<RestorePlan> {
        let (_, _, plan, current) = self.restore_plan(id, backup, registry).await?;
        Ok(RestorePlan {
            files: plan.included().count(),
            bytes: plan.included().map(|f| f.bytes).sum(),
            moved_from: plan.moved_from.as_ref().map(PathBuf::from),
            into: current,
            has_registry: plan.files.iter().any(|f| f.kind == FileKind::Registry),
            left_out_registry: plan.left_out(FileKind::Registry),
            left_out_other_prefix: plan.left_out(FileKind::OtherPrefix),
            left_out_no_place: plan.left_out(FileKind::NoPlace),
        })
    }

    /// Put `backup` of `id`'s saves back into the game's current prefix,
    /// replacing the saves there now; what it replaces is kept for
    /// [`Self::undo_restore`]. `registry` restores the prefix's registry
    /// files too.
    pub async fn restore_saves(
        &self,
        id: &GameId,
        backup: &str,
        registry: bool,
    ) -> Result<SavesDone> {
        let game = self.game(id).context("The game is not in the library")?;
        if self.is_running(id) {
            bail!("Close {} before restoring its saves", game.title);
        }
        if self.statuses.is_busy(id) {
            bail!("another operation is in progress");
        }
        let _guard = self.statuses.guard(id, StatusKind::SyncingSaves);
        let (ludusavi, title, plan, _) = self.restore_plan(id, backup, registry).await?;
        if plan.included().next().is_none() {
            bail!("Nothing of this backup can be restored here");
        }

        // A copy of Ludusavi's settings with the redirects and the files
        // left out, when not everything goes back where it came from.
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ").to_string();
        let needs_config = !plan.redirects.is_empty() || plan.files.iter().any(|f| !f.included);
        let config_dir = if needs_config {
            let own = ludusavi.config_path().await?;
            let dir = irori_core::Paths::get()
                .cache
                .join("ludusavi-restore")
                .join(&stamp);
            let (plan, title, made) = (plan.clone(), title.clone(), dir.clone());
            self.spawn_blocking(move || write_restore_config(&own, &made, &title, &plan))
                .await
                .context("Couldn’t prepare the restore")??;
            Some(dir)
        } else {
            None
        };

        // Keep what the restore replaces.
        let targets: Vec<PathBuf> = plan.included().map(|f| PathBuf::from(&f.target)).collect();
        let (undo_at, when, backup_id, kept_as) = (
            undo_dir(id),
            irori_core::util::now_rfc3339(),
            backup.to_string(),
            stamp.clone(),
        );
        let kept = self
            .spawn_blocking(move || undo::save(&undo_at, &kept_as, &when, &backup_id, &targets))
            .await
            .context("Couldn’t keep a copy of the current saves")
            .and_then(|r| r);
        if let Err(e) = kept {
            remove_quietly(config_dir.as_deref());
            return Err(e);
        }

        let result = ludusavi
            .restore(
                &title,
                Some(backup),
                self.ludusavi_folder().as_deref(),
                false,
                config_dir.as_deref(),
            )
            .await;
        remove_quietly(config_dir.as_deref());
        let op = result?;
        if op.unknown || op.title.is_none() {
            bail!("Ludusavi has no backup {backup} of {}", game.title);
        }
        Ok(SavesDone {
            title,
            files: op.files.iter().filter(|f| !f.ignored).count(),
            bytes: op.bytes(),
            failed: op.failed().count() + op.registry_failed,
            change: op.change,
        })
    }

    /// Undo the latest restore of `id`'s saves: the files it replaced come
    /// back and those it created go. Returns how many files changed.
    pub async fn undo_restore(&self, id: &GameId) -> Result<usize> {
        let game = self.game(id).context("The game is not in the library")?;
        if self.is_running(id) {
            bail!("Close {} before undoing the restore", game.title);
        }
        if self.statuses.is_busy(id) {
            bail!("another operation is in progress");
        }
        let _guard = self.statuses.guard(id, StatusKind::SyncingSaves);
        let dir = undo_dir(id);
        self.spawn_blocking(move || undo::undo(&dir))
            .await
            .context("Couldn’t undo the restore")?
    }

    /// Lock or unlock `backup` of `id`; a locked backup is never removed by
    /// Ludusavi's retention or deleted here.
    pub async fn set_saves_backup_locked(
        &self,
        id: &GameId,
        backup: &str,
        locked: bool,
    ) -> Result<()> {
        let (ludusavi, title) = self.ludusavi_and_title(id).await?;
        ludusavi
            .edit_backup(
                &title,
                backup,
                self.ludusavi_folder().as_deref(),
                Some(locked),
                None,
            )
            .await
    }

    /// Name `backup` of `id` (Ludusavi's comment, which its window shows
    /// too); an empty name clears it.
    pub async fn set_saves_backup_comment(
        &self,
        id: &GameId,
        backup: &str,
        comment: &str,
    ) -> Result<()> {
        let (ludusavi, title) = self.ludusavi_and_title(id).await?;
        ludusavi
            .edit_backup(
                &title,
                backup,
                self.ludusavi_folder().as_deref(),
                None,
                Some(comment.trim()),
            )
            .await
    }

    /// Delete `backup` of `id`, with its differential backups when it is a
    /// full one. Returns how many backups went.
    pub async fn delete_saves_backup(&self, id: &GameId, backup: &str) -> Result<usize> {
        if self.statuses.is_busy(id) && !self.is_running(id) {
            bail!("another operation is in progress");
        }
        let (ludusavi, title) = self.ludusavi_and_title(id).await?;
        let stored = ludusavi
            .backups(&title, self.ludusavi_folder().as_deref())
            .await?
            .context("Ludusavi has no backups of this game")?;
        let folder = stored.folder;
        let backup = backup.to_string();
        let app = self.settings.app();
        self.spawn_blocking(move || -> Result<usize> {
            irori_core::fs_guard::ensure_deletable(&folder, &app)?;
            Ok(mapping::delete(&folder, &backup)?.len())
        })
        .await
        .context("Couldn’t delete the backup")?
    }

    async fn ludusavi_and_title(&self, id: &GameId) -> Result<(Ludusavi, String)> {
        let game = self.game(id).context("The game is not in the library")?;
        let ludusavi = self.require_ludusavi().await?;
        let title = self
            .ludusavi_title(id)
            .await?
            .with_context(|| no_match(&game.title))?;
        Ok((ludusavi, title))
    }

    /// The backup after a session, when the game's settings ask for one.
    /// Runs once the store's cloud upload is done; failures become a
    /// notice, since nobody is waiting on it.
    pub(crate) async fn back_up_after_session(&self, id: &GameId, title: &str) {
        let guard = self.statuses.guard(id, StatusKind::SyncingSaves);
        let result = self.back_up_saves_inner(id).await;
        drop(guard);
        let error = match result {
            Ok(done) => {
                tracing::info!(
                    "Ludusavi backed up {} files ({} bytes) of {id}",
                    done.files,
                    done.bytes
                );
                if done.failed == 0 {
                    return;
                }
                format!(
                    "{} files couldn’t be copied; the app log lists them.",
                    done.failed
                )
            }
            Err(e) => {
                tracing::warn!("Ludusavi backup of {id} after the session: {e:#}");
                format!("{e:#}")
            }
        };
        self.bus.notice(
            Notice::new(
                NoticeLevel::Warning,
                format!("Saves of {title} weren’t fully backed up"),
            )
            .message(error)
            .keys(
                "shell.notice.ludusavi_backup_failed",
                None,
                &[("title", title)],
            )
            .game(id.clone()),
        );
    }
}

impl Backend {
    /// Back up `id`'s saves every `every` while its session lasts
    /// (`iroriLudusaviInterval`), one backup at a time, until `stop`. Only a
    /// sleeping timer between backups, and nothing at all outside a session.
    /// A backup under way when the session ends finishes first, so it never
    /// overlaps the cloud upload and the backup after the session. The game
    /// keeps its playing status; a failure is told once per session.
    pub(crate) async fn back_up_while_playing(
        &self,
        id: GameId,
        title: String,
        every: std::time::Duration,
        stop: CancellationToken,
    ) {
        let mut told = false;
        loop {
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tokio::time::sleep(every) => {}
            }
            match self.back_up_saves_inner(&id).await {
                Ok(done) if done.failed == 0 => tracing::info!(
                    "Ludusavi backed up {} files ({} bytes) of {id} while playing",
                    done.files,
                    done.bytes
                ),
                result => {
                    let error = match result {
                        Ok(done) => format!(
                            "{} files couldn’t be copied; the app log lists them.",
                            done.failed
                        ),
                        Err(e) => format!("{e:#}"),
                    };
                    tracing::warn!("Ludusavi backup of {id} while playing: {error}");
                    if !told {
                        told = true;
                        self.bus.notice(
                            Notice::new(
                                NoticeLevel::Warning,
                                format!("Saves of {title} weren’t fully backed up"),
                            )
                            .message(error)
                            .keys(
                                "shell.notice.ludusavi_backup_failed",
                                None,
                                &[("title", &title)],
                            )
                            .game(id.clone()),
                        );
                    }
                }
            }
        }
    }
}

/// The time between backups while playing, never under a minute.
pub(crate) fn backup_interval(minutes: u32) -> std::time::Duration {
    std::time::Duration::from_secs(u64::from(minutes.max(1)) * 60)
}

/// Write the copy of Ludusavi's settings a restore runs with into `dir`:
/// its own `config.yaml` (at `own`) with the plan's redirects and left-out
/// files, and its game data beside it.
fn write_restore_config(own: &Path, dir: &Path, title: &str, plan: &restore::Plan) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("Cannot create {}", dir.display()))?;
    let text = match std::fs::read_to_string(own) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("Cannot read Ludusavi’s settings ({})", own.display()));
        }
    };
    std::fs::write(
        dir.join("config.yaml"),
        restore::configure(&text, title, plan)?,
    )?;
    // Its game data, so the copy needs no download.
    if let Some(manifest) = own.parent().map(|d| d.join("manifest.yaml"))
        && manifest.is_file()
    {
        std::fs::copy(&manifest, dir.join("manifest.yaml"))?;
    }
    Ok(())
}

/// Remove a restore's copy of Ludusavi's settings.
fn remove_quietly(dir: Option<&Path>) {
    if let Some(dir) = dir
        && let Err(e) = std::fs::remove_dir_all(dir)
    {
        tracing::debug!("removing {}: {e}", dir.display());
    }
}

/// Most titles a search lists.
const SEARCH_RESULTS: usize = 20;

fn no_match(title: &str) -> String {
    format!(
        "Ludusavi doesn’t know {title}. Choose its title in Ludusavi on the game’s Save backups."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_drive_c_in_wine_proton_and_umu_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Wine and CrossOver: drive_c in the prefix itself.
        let wine = root.join("Wine");
        std::fs::create_dir_all(wine.join("drive_c")).unwrap();
        assert_eq!(wine_prefix_root(&wine), Some(wine.clone()));

        // Proton run directly: Irori's prefix is Proton's compat data
        // folder, and the Wine prefix with the saves is `pfx` inside it. A
        // backup that is given the outer folder finds nothing, as happened
        // with S.T.A.L.K.E.R. 2's saves.
        let proton = root.join("Stalker 2");
        let saves =
            proton.join("pfx/drive_c/users/steamuser/AppData/Local/Stalker2/Saved/SaveGames");
        std::fs::create_dir_all(&saves).unwrap();
        std::fs::write(saves.join("SaveSlot_0.sav"), b"save").unwrap();
        assert_eq!(wine_prefix_root(&proton), Some(proton.join("pfx")));
        assert!(
            wine_prefix_root(&proton)
                .unwrap()
                .join(
                    "drive_c/users/steamuser/AppData/Local/Stalker2/Saved/SaveGames/SaveSlot_0.sav"
                )
                .is_file()
        );

        // umu: flat prefix, with a `pfx` link back to itself.
        let umu = root.join("umu");
        std::fs::create_dir_all(umu.join("drive_c")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(".", umu.join("pfx")).unwrap();
        assert_eq!(wine_prefix_root(&umu), Some(umu.clone()));

        // Not created yet.
        assert_eq!(wine_prefix_root(&root.join("New game")), None);
    }

    #[test]
    fn looks_games_up_by_store_id_then_title() {
        let game = |runner, app: &str| GameInfo {
            runner: Some(runner),
            app_name: app.into(),
            title: "Hades".into(),
            ..Default::default()
        };
        let steam = query_for(&game(Runner::Steam, "1145360"));
        assert_eq!((steam.steam_id, steam.gog_id), (Some(1145360), None));
        let gog = query_for(&game(Runner::Gog, "1207658691"));
        assert_eq!((gog.steam_id, gog.gog_id), (None, Some(1207658691)));
        let epic = query_for(&game(Runner::Legendary, "Min"));
        assert_eq!((epic.steam_id, epic.gog_id), (None, None));
        assert_eq!(epic.names, ["Hades"]);
        assert!(epic.normalized && !epic.fuzzy && !epic.multiple);
    }
}
