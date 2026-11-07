//! Save backups with Ludusavi for every game, whichever store it comes from:
//! matching a game to Ludusavi's title, pointing Ludusavi at the game's real
//! Wine prefix, backing up on request and after a session, listing the
//! backups and restoring one.
//!
//! Store cloud saves keep working beside it. They never run at the same
//! time: after a session the cloud upload finishes before Ludusavi reads the
//! files, both hold the game's busy status, and [`SavesKeeper`] tells the UI
//! which of them handles a game.

pub mod client;

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::process::CancellationToken;
use irori_core::{
    GameId, GameInfo, HostPlatform, InstallPlatform, Notice, NoticeLevel, Runner, StatusKind,
};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::Value;

pub use client::{Change, INSTALL_URL, Ludusavi, Source, TitleMatch, TitleQuery};

use crate::Backend;

/// Settings key of the title the user matched a game to.
pub const TITLE_KEY: &str = "iroriLudusaviTitle";

/// Automatic matches found this session, by game key; `None` when Ludusavi
/// doesn't know the game. Matching reads Ludusavi's whole data set, so it is
/// done once per game.
static MATCHES: Lazy<Mutex<HashMap<String, Option<String>>>> = Lazy::new(Default::default);

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
    /// The backup came from another Wine prefix than the game's current
    /// one; Ludusavi puts the files back there.
    pub elsewhere: Option<PathBuf>,
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

/// The Wine prefix a file of a backup lives in: everything before its
/// `drive_c`. `None` for files outside a prefix.
fn prefix_of_file(path: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    let mut prefix = PathBuf::new();
    for part in path.components() {
        if matches!(part, Component::Normal(n) if n.eq_ignore_ascii_case("drive_c")) {
            return Some(prefix);
        }
        prefix.push(part);
    }
    None
}

/// Where a restore of these files would land, when it isn't `current`.
fn other_prefix<'a>(
    files: impl Iterator<Item = &'a str>,
    current: Option<&Path>,
) -> Option<PathBuf> {
    files
        .filter_map(prefix_of_file)
        .find(|p| current.is_none_or(|c| !same_path(p, c)))
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(a), Ok(b)) if a == b
        )
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
    /// match (Steam id, GOG id, then title). `Ok(None)` when Ludusavi
    /// doesn't know the game.
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
        let found = ludusavi
            .find(&query_for(&game))
            .await?
            .into_iter()
            .next()
            .map(|m| m.title);
        MATCHES.lock().insert(id.key(), found.clone());
        Ok(found)
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
        Ok(GameBackups {
            title: Some(title),
            chosen,
            wine_prefix,
            folder,
            backups,
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

    /// What restoring `backup` of `id` would write, and where.
    pub async fn preview_restore(&self, id: &GameId, backup: &str) -> Result<RestorePlan> {
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
            )
            .await?;
        let current = self.ludusavi_wine_prefix(&game);
        let elsewhere = other_prefix(
            op.files
                .iter()
                .filter(|f| !f.ignored)
                .map(|f| f.path.as_str()),
            current.as_deref(),
        );
        Ok(RestorePlan {
            files: op.files.iter().filter(|f| !f.ignored).count(),
            bytes: op.bytes(),
            elsewhere,
        })
    }

    /// Put `backup` of `id`'s saves back, replacing the saves there now.
    pub async fn restore_saves(&self, id: &GameId, backup: &str) -> Result<SavesDone> {
        let game = self.game(id).context("The game is not in the library")?;
        if self.is_running(id) {
            bail!("Close {} before restoring its saves", game.title);
        }
        if self.statuses.is_busy(id) {
            bail!("another operation is in progress");
        }
        let _guard = self.statuses.guard(id, StatusKind::SyncingSaves);
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
                false,
            )
            .await?;
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
    fn tells_which_prefix_a_backup_came_from() {
        assert_eq!(
            prefix_of_file("/games/Stalker 2/pfx/drive_c/users/steamuser/a.sav"),
            Some(PathBuf::from("/games/Stalker 2/pfx"))
        );
        assert_eq!(
            prefix_of_file("C:/Users/me/AppData/Local/Game/save.dat"),
            None,
            "a Windows path keeps its drive letter, not a drive_c folder"
        );
        assert_eq!(
            prefix_of_file("/home/me/.local/share/Celeste/0.celeste"),
            None
        );

        let files = [
            "/home/me/.config/game/settings.ini",
            "/old/pfx/drive_c/users/steamuser/save.sav",
        ];
        assert_eq!(
            other_prefix(files.into_iter(), Some(Path::new("/new/pfx"))),
            Some(PathBuf::from("/old/pfx"))
        );
        assert_eq!(
            other_prefix(files.into_iter(), Some(Path::new("/old/pfx"))),
            None
        );
        assert_eq!(
            other_prefix(files.into_iter(), None),
            Some(PathBuf::from("/old/pfx"))
        );
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
