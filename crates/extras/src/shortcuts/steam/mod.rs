//! Steam "non-Steam game" shortcuts: edits `shortcuts.vdf` of every Steam
//! user and places library artwork in their `grid` folders.
//!
//! Steam reads `shortcuts.vdf` at startup and rewrites it when its own
//! shortcuts change, so the file is only edited while Steam is not running.
//! When Steam is running the report says so ([`SteamReport::steam_running`])
//! and nothing is written; the caller asks the user to close Steam.
//!
//! Entries start the game with `irori --launch <runner>/<app> --wait`, so
//! Steam follows the game's session, not a launcher that hands over and
//! exits. Only Irori's own entries are changed, and in them only Irori's
//! fields ([`sync`]); every write is backed up first and read back.

pub mod art;
pub mod shortcuts_file;
pub mod sync;
pub mod vdf;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use irori_core::{AppSettings, GameId, GameInfo};

use self::shortcuts_file::{GridIds, NewShortcut, ShortcutEntry, ShortcutsFile};
use self::sync::{Upsert, write_checked};
use super::LauncherCommand;
use super::icon;

/// One Steam account found under `<steam>/userdata`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamUser {
    pub id: String,
    /// `userdata/<id>/config`.
    pub config_dir: PathBuf,
}

impl SteamUser {
    pub fn shortcuts_file(&self) -> PathBuf {
        self.config_dir.join("shortcuts.vdf")
    }

    pub fn grid_dir(&self) -> PathBuf {
        self.config_dir.join("grid")
    }
}

/// Outcome of a Steam shortcut operation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SteamReport {
    /// Steam is running, so `shortcuts.vdf` was not touched. Close Steam and
    /// retry; changes appear after Steam starts again.
    pub steam_running: bool,
    /// Number of Steam users whose `shortcuts.vdf` was rewritten.
    pub users_changed: usize,
    /// The game was already present (add) for at least one user.
    pub already_present: bool,
    /// Shortcuts added for the user who received the most (add).
    pub added: usize,
    /// Per-user problems (corrupt files, write errors). Non-fatal as long as
    /// at least one user succeeded.
    pub errors: Vec<String>,
}

/// Steam installation directory: the configured path when it has Steam
/// users, else on Linux the native, Flatpak or Snap Steam with them, else
/// `~/.steam/steam`.
pub fn steam_path(app: &AppSettings) -> PathBuf {
    let configured = app.default_steam_path.replace('\'', "");
    let configured = configured.trim();
    if !configured.is_empty() {
        let p = irori_core::paths::expand_home(configured);
        if p.join("userdata").is_dir() {
            return p;
        }
    }
    if !cfg!(target_os = "linux") {
        return irori_core::platform::steam_dir();
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    irori_core::platform::find_linux_steam(&home, "userdata")
        .unwrap_or_else(|| home.join(".steam/steam"))
}

/// Steam users under `<steam>/userdata` (directories other than `0` and `ac`).
pub async fn users(steam: &Path) -> Result<Vec<SteamUser>> {
    let userdata = steam.join("userdata");
    let mut rd = match tokio::fs::read_dir(&userdata).await {
        Ok(rd) => rd,
        Err(_) => bail!(
            "{} does not exist. Can't add/remove game to/from Steam!",
            userdata.display()
        ),
    };
    let mut out = Vec::new();
    while let Some(entry) = rd.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "0" || name == "ac" {
            continue;
        }
        if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            out.push(SteamUser {
                config_dir: entry.path().join("config"),
                id: name,
            });
        }
    }
    if out.is_empty() {
        bail!(
            "{} does not contain a valid user directory!",
            userdata.display()
        );
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Whether Steam is running: always true in Steam Deck game mode, otherwise a
/// live PID in `<steam>/../steam.pid` or `<steam>/steam.pid`.
pub async fn is_running(steam: &Path) -> bool {
    if std::env::var("XDG_CURRENT_DESKTOP").as_deref() == Ok("gamescope") {
        return true;
    }
    let system = tokio::task::spawn_blocking(|| {
        sysinfo::System::new_with_specifics(
            sysinfo::RefreshKind::nothing().with_processes(sysinfo::ProcessRefreshKind::nothing()),
        )
    })
    .await
    .unwrap_or_default();
    for pid_file in [steam.join("../steam.pid"), steam.join("steam.pid")] {
        let Ok(text) = tokio::fs::read_to_string(&pid_file).await else {
            continue;
        };
        if let Ok(pid) = text.trim().parse::<u32>()
            && pid > 0
            && system.process(sysinfo::Pid::from_u32(pid)).is_some()
        {
            return true;
        }
    }
    system.processes().values().any(|p| {
        matches!(
            p.name().to_string_lossy().to_ascii_lowercase().as_str(),
            "steam" | "steam.exe" | "steam_osx"
        )
    })
}

/// `Exe`, `StartDir` and `LaunchOptions` of Irori's shortcut for a game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryCommand {
    pub exe: String,
    pub start_dir: String,
    pub launch_options: String,
}

impl EntryCommand {
    pub fn new(launcher: &LauncherCommand, id: &GameId) -> Self {
        let mut opts: Vec<String> = launcher
            .prefix_args
            .iter()
            .map(|a| quote_if_needed(a))
            .collect();
        opts.push("--launch".into());
        opts.push(quote(&super::launch_target(id)));
        // Steam counts the session as long as this command runs: `--wait`
        // keeps it until the game itself ends.
        opts.push("--wait".into());
        Self {
            exe: quote(&launcher.program),
            start_dir: quote(&launcher.working_dir.to_string_lossy()),
            launch_options: opts.join(" "),
        }
    }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\\\""))
}

fn quote_if_needed(s: &str) -> String {
    if s.chars().any(|c| c.is_whitespace() || c == '"') {
        quote(s)
    } else {
        s.to_string()
    }
}

/// Whether a shortcut entry is Irori's and starts this game ([`sync::entry_game`]).
/// Titles are never compared, so renamed entries still match and other
/// tools' entries never do.
pub fn matches_game(entry: ShortcutEntry<'_>, id: &GameId) -> bool {
    sync::entry_game(entry).as_ref() == Some(id)
}

/// Steam collection every Irori shortcut is filed under, so Irori's games
/// are listed together instead of only under "Non-Steam".
pub const COLLECTION: &str = "Irori";

/// A game to add, with its Steam app ID (from wiki data) when known, which
/// selects official Steam artwork.
#[derive(Debug, Clone, Copy)]
pub struct SteamGame<'a> {
    pub game: &'a GameInfo,
    pub steam_app_id: Option<&'a str>,
}

/// Add the game to every Steam user's shortcuts and set its artwork.
/// `steam_app_id` (from wiki data) selects official Steam artwork when known.
///
/// The game need not be installed: the shortcut starts Irori, which offers
/// to install a game that isn't installed yet.
pub async fn add(
    game: &GameInfo,
    app: &AppSettings,
    steam_app_id: Option<&str>,
) -> Result<SteamReport> {
    let report = add_many(&[SteamGame { game, steam_app_id }], app).await?;
    if report.users_changed == 0 && !report.already_present && !report.steam_running {
        bail!(
            "Adding {} to Steam failed: {}",
            game.title,
            report.errors.join("; ")
        );
    }
    Ok(report)
}

/// Add several games at once, reading and writing each user's
/// `shortcuts.vdf` once. Games a user already has keep their entry, brought
/// up to date ([`sync::upsert`]).
pub async fn add_many(games: &[SteamGame<'_>], app: &AppSettings) -> Result<SteamReport> {
    let synced = sync(games, None, app).await?;
    let mut report = SteamReport {
        steam_running: synced.steam_running,
        users_changed: synced.users_changed,
        already_present: synced.added < games.len(),
        added: synced.added,
        errors: synced.errors,
    };
    if synced.steam_running {
        let steam = steam_path(app);
        let users = users(&steam).await?;
        report.already_present = false;
        for g in games {
            if is_added_for(&users, &g.game.id()).await {
                report.already_present = true;
                break;
            }
        }
    }
    Ok(report)
}

/// What [`sync`] or [`remove_games`] did. Counts are per game: a game added
/// for two Steam users counts once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Steam is running, so nothing was written. Close Steam and retry.
    pub steam_running: bool,
    pub added: usize,
    /// Entries whose command, tag or icon Irori brought up to date.
    pub updated: usize,
    pub removed: usize,
    /// Steam users whose `shortcuts.vdf` was rewritten.
    pub users_changed: usize,
    /// Per-user problems (corrupt files, write errors).
    pub errors: Vec<String>,
}

/// A game's entry, ready to write.
struct Prepared<'a> {
    game: SteamGame<'a>,
    id: GameId,
    entry: vdf::Map,
}

/// The entries for `games`, with their icons from Irori's art store.
async fn prepare<'a>(games: &[SteamGame<'a>], launcher: &LauncherCommand) -> Vec<Prepared<'a>> {
    let mut prepared = Vec::with_capacity(games.len());
    let mut seen = HashSet::new();
    for g in games {
        let game = g.game;
        let id = game.id();
        if !seen.insert(id.clone()) {
            continue;
        }
        let cmd = EntryCommand::new(launcher, &id);
        let icon = match icon::ensure_icon(game).await {
            Ok(p) => p.map(|p| p.to_string_lossy().into_owned()),
            Err(e) => {
                tracing::warn!("no icon for the Steam shortcut of {}: {e:#}", game.title);
                None
            }
        };
        let entry = NewShortcut {
            app_name: game.title.clone(),
            exe: cmd.exe,
            start_dir: cmd.start_dir,
            icon,
            launch_options: cmd.launch_options,
            last_play_time: i32::try_from(irori_core::util::unix_now()).unwrap_or(i32::MAX),
            tags: vec![COLLECTION.to_string()],
            irori_id: Some(super::launch_target(&id)),
        }
        .to_entry();
        prepared.push(Prepared {
            game: *g,
            id,
            entry,
        });
    }
    prepared
}

/// The art IDs Steam uses for an entry: its stored `appid`, else the one
/// computed from its command and name.
fn grid_ids(entry: ShortcutEntry<'_>) -> GridIds {
    match entry.app_id() {
        Some(short) => GridIds::from_short(short),
        None => GridIds::from_exe_name(entry.exe(), entry.app_name()),
    }
}

/// Remove the art of a removed entry (by its stored and its computed IDs).
async fn remove_art(grid: &Path, entry: &vdf::Map) {
    let e = ShortcutEntry(entry);
    art::remove(grid, grid_ids(e)).await;
    art::remove(grid, GridIds::from_exe_name(e.exe(), e.app_name())).await;
}

/// Bring Irori's entries in every Steam user's `shortcuts.vdf` in step with
/// `games`: add the missing ones, update Irori's fields of the others, and
/// add the art Steam's `grid` folder lacks (the user's own pictures stay).
/// Irori's entries of games `gone` picks are removed with their art. Other
/// programs' entries are never touched. Nothing is written while Steam runs
/// ([`SyncReport::steam_running`]).
pub async fn sync(
    games: &[SteamGame<'_>],
    gone: Option<&Gone<'_>>,
    app: &AppSettings,
) -> Result<SyncReport> {
    let steam = steam_path(app);
    let users = users(&steam).await?;
    if is_running(&steam).await {
        return Ok(SyncReport {
            steam_running: true,
            ..Default::default()
        });
    }
    let prepared = prepare(games, &LauncherCommand::detect()).await;
    sync_users(&users, &prepared, gone).await
}

/// Picks the games whose entries a sync removes.
pub type Gone<'a> = dyn Fn(&GameId) -> bool + Send + Sync + 'a;

/// [`sync`] for the given users, with Steam known to be closed.
async fn sync_users(
    users: &[SteamUser],
    prepared: &[Prepared<'_>],
    gone: Option<&Gone<'_>>,
) -> Result<SyncReport> {
    let icons = icon::icons_dir();
    let mut report = SyncReport::default();
    for user in users {
        let path = user.shortcuts_file();
        let mut file = match read_file(&path).await {
            Ok(Some(f)) => f,
            Ok(None) => ShortcutsFile::empty(),
            Err(e) => {
                report.errors.push(format!(
                    "Can't change the shortcuts of Steam user \"{}\". \"{}\" is corrupted! {e:#}",
                    user.id,
                    path.display()
                ));
                continue;
            }
        };
        let (mut added, mut updated) = (0, 0);
        for p in prepared {
            match sync::upsert(&mut file, &p.id, &p.entry, &icons) {
                Upsert::Added => added += 1,
                Upsert::Updated => updated += 1,
                Upsert::Unchanged => {}
            }
        }
        let removed = match gone {
            Some(gone) => sync::remove_own(&mut file, gone),
            None => Vec::new(),
        };
        if added + updated + removed.len() > 0 {
            if let Err(e) = write_checked(&path, &file).await {
                report.errors.push(format!("{e:#}"));
                continue;
            }
            report.users_changed += 1;
        }
        report.added = report.added.max(added);
        report.updated = report.updated.max(updated);
        report.removed = report.removed.max(removed.len());
        let grid = user.grid_dir();
        for entry in &removed {
            remove_art(&grid, entry).await;
        }
        for p in prepared {
            let Some(entry) = file.entries().find(|e| matches_game(*e, &p.id)) else {
                continue;
            };
            let ids = grid_ids(entry);
            for err in art::prepare(&grid, ids, p.game.game, p.game.steam_app_id).await {
                tracing::warn!("Steam artwork for {}: {err}", p.game.game.title);
            }
        }
    }
    if report.users_changed == 0 && !report.errors.is_empty() {
        bail!(
            "Changing Steam's shortcuts failed: {}",
            report.errors.join("; ")
        );
    }
    Ok(report)
}

/// Remove Irori's entries for `ids` (every one of Irori's with `None`) from
/// every Steam user, with their art. Other programs' entries are never
/// touched. Nothing is written while Steam runs: the report then says so
/// when there was something to remove.
pub async fn remove_games(ids: Option<&HashSet<GameId>>, app: &AppSettings) -> Result<SyncReport> {
    let steam = steam_path(app);
    let users = match users(&steam).await {
        Ok(u) => u,
        Err(e) => {
            tracing::debug!("Steam shortcuts not removed: {e:#}");
            return Ok(SyncReport::default());
        }
    };
    let wanted = |id: &GameId| ids.is_none_or(|ids| ids.contains(id));
    if is_running(&steam).await {
        let mut present = false;
        for user in &users {
            if let Ok(Some(file)) = read_file(&user.shortcuts_file()).await
                && sync::own_games(&file).iter().any(wanted)
            {
                present = true;
                break;
            }
        }
        return Ok(SyncReport {
            steam_running: present,
            ..Default::default()
        });
    }
    remove_for_users(&users, ids).await
}

/// [`remove_games`] for the given users, with Steam known to be closed.
async fn remove_for_users(
    users: &[SteamUser],
    ids: Option<&HashSet<GameId>>,
) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    for user in users {
        let path = user.shortcuts_file();
        let mut file = match read_file(&path).await {
            Ok(Some(f)) => f,
            Ok(None) => continue,
            Err(e) => {
                report.errors.push(format!(
                    "Can't remove games from Steam user \"{}\". \"{}\" is corrupted! {e:#}",
                    user.id,
                    path.display()
                ));
                continue;
            }
        };
        let removed = sync::remove_own(&mut file, |id| ids.is_none_or(|ids| ids.contains(id)));
        if removed.is_empty() {
            continue;
        }
        if let Err(e) = write_checked(&path, &file).await {
            report.errors.push(format!("{e:#}"));
            continue;
        }
        report.users_changed += 1;
        report.removed = report.removed.max(removed.len());
        let grid = user.grid_dir();
        for entry in &removed {
            remove_art(&grid, entry).await;
        }
    }
    if report.users_changed == 0 && !report.errors.is_empty() {
        bail!(
            "Removing games from Steam failed: {}",
            report.errors.join("; ")
        );
    }
    Ok(report)
}

/// Remove the game's shortcut from every Steam user, with its artwork.
pub async fn remove(game: &GameInfo, app: &AppSettings) -> Result<SteamReport> {
    let ids = HashSet::from([game.id()]);
    let r = remove_games(Some(&ids), app)
        .await
        .map_err(|e| e.context(format!("Removing {} from Steam failed", game.title)))?;
    Ok(SteamReport {
        steam_running: r.steam_running,
        users_changed: r.users_changed,
        errors: r.errors,
        ..Default::default()
    })
}

/// Whether any Steam user has an Irori shortcut for the game.
pub async fn is_added(game: &GameInfo, app: &AppSettings) -> bool {
    let steam = steam_path(app);
    match users(&steam).await {
        Ok(users) => is_added_for(&users, &game.id()).await,
        Err(_) => false,
    }
}

async fn is_added_for(users: &[SteamUser], id: &GameId) -> bool {
    for user in users {
        if let Ok(Some(file)) = read_file(&user.shortcuts_file()).await
            && file.entries().any(|e| matches_game(e, id))
        {
            return true;
        }
    }
    false
}

/// Read and validate a `shortcuts.vdf`; `Ok(None)` when it does not exist.
async fn read_file(path: &Path) -> Result<Option<ShortcutsFile>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(Some(ShortcutsFile::parse(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;

    fn launcher() -> LauncherCommand {
        LauncherCommand {
            program: "/usr/bin/irori".into(),
            prefix_args: vec![],
            working_dir: "/usr/bin".into(),
        }
    }

    #[test]
    fn entry_command_and_matching() {
        let id = GameId::new(Runner::Gog, "1423049311");
        let cmd = EntryCommand::new(&launcher(), &id);
        assert_eq!(cmd.exe, "\"/usr/bin/irori\"");
        assert_eq!(cmd.start_dir, "\"/usr/bin\"");
        assert_eq!(cmd.launch_options, "--launch \"gog/1423049311\" --wait");

        let entry = NewShortcut {
            app_name: "Cyberpunk".into(),
            exe: cmd.exe.clone(),
            start_dir: cmd.start_dir.clone(),
            icon: None,
            launch_options: cmd.launch_options.clone(),
            last_play_time: 0,
            tags: Vec::new(),
            irori_id: None,
        }
        .to_entry();
        assert!(matches_game(ShortcutEntry(&entry), &id));
        assert!(!matches_game(
            ShortcutEntry(&entry),
            &GameId::new(Runner::Gog, "142304931")
        ));
        assert!(!matches_game(
            ShortcutEntry(&entry),
            &GameId::new(Runner::Legendary, "1423049311")
        ));

        let url_entry = vdf::Map::new().with(
            "LaunchOptions",
            vdf::Value::String("\"irori://launch/sideload/my%20app\"".into()),
        );
        assert!(matches_game(
            ShortcutEntry(&url_entry),
            &GameId::new(Runner::Sideload, "my app")
        ));
    }

    #[test]
    fn flatpak_launch_options() {
        let l = LauncherCommand {
            program: "flatpak".into(),
            prefix_args: vec!["run".into(), "app.irori.Irori".into()],
            working_dir: "/home/u".into(),
        };
        let cmd = EntryCommand::new(&l, &GameId::new(Runner::Nile, "amzn1.x"));
        assert_eq!(cmd.exe, "\"flatpak\"");
        assert_eq!(
            cmd.launch_options,
            "run app.irori.Irori --launch \"nile/amzn1.x\" --wait"
        );
    }

    fn game(runner: Runner, app: &str, title: &str) -> GameInfo {
        GameInfo {
            runner: Some(runner),
            app_name: app.into(),
            title: title.into(),
            ..Default::default()
        }
    }

    fn prepared<'a>(games: &'a [GameInfo]) -> Vec<Prepared<'a>> {
        games
            .iter()
            .map(|g| {
                let id = g.id();
                let cmd = EntryCommand::new(&launcher(), &id);
                Prepared {
                    game: SteamGame {
                        game: g,
                        steam_app_id: None,
                    },
                    entry: NewShortcut {
                        app_name: g.title.clone(),
                        exe: cmd.exe,
                        start_dir: cmd.start_dir,
                        icon: None,
                        launch_options: cmd.launch_options,
                        last_play_time: 0,
                        tags: vec![COLLECTION.into()],
                        irori_id: Some(super::super::launch_target(&id)),
                    }
                    .to_entry(),
                    id,
                }
            })
            .collect()
    }

    #[tokio::test]
    async fn syncs_and_removes_only_irori_entries() {
        crate::util::test_paths();
        let dir = tempfile::tempdir().unwrap();
        let users: Vec<SteamUser> = ["1", "2"]
            .iter()
            .map(|id| SteamUser {
                id: id.to_string(),
                config_dir: dir.path().join(id).join("config"),
            })
            .collect();
        // Another program's entry, and an older Irori's entry for a game
        // that left the library.
        let mut existing = ShortcutsFile::empty();
        existing.push(
            NewShortcut {
                app_name: "Other".into(),
                exe: "\"/bin/x\"".into(),
                start_dir: "\"/bin\"".into(),
                icon: None,
                launch_options: "--launch \"gog/9\"".into(),
                last_play_time: 5,
                tags: Vec::new(),
                irori_id: None,
            }
            .to_entry(),
        );
        let foreign = existing.to_bytes();
        existing.push(
            NewShortcut {
                app_name: "Gone".into(),
                exe: "\"/usr/bin/irori\"".into(),
                start_dir: "\"/usr/bin\"".into(),
                icon: None,
                launch_options: "--launch \"gog/9\"".into(),
                last_play_time: 5,
                tags: Vec::new(),
                irori_id: None,
            }
            .to_entry(),
        );
        std::fs::create_dir_all(&users[0].config_dir).unwrap();
        std::fs::write(users[0].shortcuts_file(), existing.to_bytes()).unwrap();

        let games = [
            game(Runner::Gog, "1", "Hades"),
            game(Runner::Legendary, "Fortnite", "Fortnite"),
        ];
        let list = prepared(&games);
        let keep: HashSet<GameId> = games.iter().map(GameInfo::id).collect();
        let gone = |id: &GameId| !keep.contains(id);
        let report = sync_users(&users, &list, Some(&gone)).await.unwrap();
        assert_eq!(
            (report.added, report.updated, report.removed),
            (2, 0, 1),
            "{report:?}"
        );
        assert_eq!(report.users_changed, 2);
        let read = |u: &SteamUser| {
            ShortcutsFile::parse(&std::fs::read(u.shortcuts_file()).unwrap()).unwrap()
        };
        for user in &users {
            assert_eq!(sync::own_games(&read(user)), keep);
        }
        assert_eq!(
            read(&users[0]).entries().next().unwrap().app_name(),
            "Other"
        );
        // The first user's file was backed up before the change.
        assert_eq!(
            std::fs::read(sync::backup_path(&users[0].shortcuts_file())).unwrap(),
            existing.to_bytes()
        );

        // Nothing new: nothing written.
        let again = sync_users(&users, &list, Some(&gone)).await.unwrap();
        assert_eq!(again.users_changed, 0);

        // Removing one game, then all of Irori's, leaves the other entry.
        let hades = HashSet::from([games[0].id()]);
        let removed = remove_for_users(&users, Some(&hades)).await.unwrap();
        assert_eq!((removed.removed, removed.users_changed), (1, 2));
        assert!(!sync::own_games(&read(&users[1])).contains(&games[0].id()));
        let all = remove_for_users(&users, None).await.unwrap();
        assert_eq!(all.removed, 1);
        assert_eq!(std::fs::read(users[0].shortcuts_file()).unwrap(), foreign);
        assert_eq!(read(&users[1]).entries().count(), 0);
    }

    #[tokio::test]
    async fn users_and_running_detection() {
        let dir = tempfile::tempdir().unwrap();
        let steam = dir.path().join("steam");
        assert!(users(&steam).await.is_err());
        for d in ["userdata/0", "userdata/ac", "userdata/1234"] {
            std::fs::create_dir_all(steam.join(d)).unwrap();
        }
        std::fs::write(steam.join("userdata/file"), b"").unwrap();
        let u = users(&steam).await.unwrap();
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].id, "1234");
        assert!(
            u[0].shortcuts_file()
                .ends_with("userdata/1234/config/shortcuts.vdf")
        );

        if std::env::var("XDG_CURRENT_DESKTOP").as_deref() != Ok("gamescope") {
            assert!(!is_running(&steam).await);
            std::fs::write(steam.join("steam.pid"), std::process::id().to_string()).unwrap();
            assert!(is_running(&steam).await);
        }
    }
}
