//! Where Irori's games and everything that serves them take disk space, and
//! what of it can be freed safely.
//!
//! The report measures installed games, Wine prefixes, the Wine and Proton
//! builds Irori downloaded, emulators and Irori's own caches and logs, groups
//! them by the disk holding them, and lists cleanups: download leftovers,
//! prefixes no installed game uses, old Wine builds nothing uses, the art
//! thumbnail cache and logs.
//!
//! Measuring walks folders, so it runs on a blocking thread, and each folder's
//! size is cached in `<cache>/storage.json` with the folder's modified time. A
//! folder measured within [`SIZE_MAX_AGE`] whose modified time is unchanged is
//! not walked again unless the user asks for a rescan.
//!
//! A cleanup deletes only items the report offers at that moment (the report
//! is rebuilt first, so a game installed since cannot lose its prefix), and
//! every deletion goes through `fs_guard::ensure_deletable`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use irori_core::settings::AppSettings;
use irori_core::{Event, GameId, GameInfo, Paths, Runner, fs_guard};
use irori_runtime::manager::{self, Release};
use serde::{Deserialize, Serialize};

use crate::Backend;

/// The longest a cached folder size is trusted without a rescan, in seconds.
/// A folder's modified time only changes with its direct entries, so an
/// update deep inside a game goes unnoticed until then.
const SIZE_MAX_AGE: u64 = 7 * 24 * 60 * 60;

/// What the depot names a file it is still assembling.
const PART_SUFFIX: &str = ".irori-part";

/// What a measured folder holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Holding {
    Game,
    Prefix,
    WineBuild,
    Emulators,
    /// Irori's caches and logs.
    IroriData,
    /// Installs that never finished.
    Unfinished,
}

impl Holding {
    /// Everything but games, in the order the disk breakdown lists them.
    pub const SUPPORT: [Holding; 5] = [
        Holding::Prefix,
        Holding::WineBuild,
        Holding::Emulators,
        Holding::IroriData,
        Holding::Unfinished,
    ];
}

/// A mounted disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drive {
    /// The volume's name; may be empty or a device path.
    pub name: String,
    pub mount: PathBuf,
    pub total: u64,
    pub free: u64,
}

impl Drive {
    /// Name to show: the volume name when it reads like one, else where it
    /// is mounted.
    pub fn label(&self) -> String {
        let name = self.name.trim();
        if name.is_empty() || name.starts_with('/') || name.starts_with("\\\\") {
            self.mount.display().to_string()
        } else {
            format!("{name} ({})", self.mount.display())
        }
    }

    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }

    /// Whether both are mounts of one volume: btrfs subvolumes (`/` and
    /// `/home` on Fedora) and bind mounts list the same device with the same
    /// size, and counting them apart shows the disk twice. A device path is
    /// unique; other names (`tmpfs`, volume labels) repeat, so those must also
    /// agree on free space.
    fn same_volume(&self, other: &Drive) -> bool {
        let name = self.name.trim();
        self.mount == other.mount
            || (!name.is_empty()
                && self.name == other.name
                && self.total == other.total
                && (name.starts_with('/') || self.free == other.free))
    }
}

/// One installed game's size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameSize {
    pub id: GameId,
    pub title: String,
    pub bytes: u64,
}

/// How one disk is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskUsage {
    pub drive: Drive,
    /// Installed games, largest first.
    pub games: Vec<GameSize>,
    /// Bytes of everything else Irori keeps here, by what it is.
    pub support: BTreeMap<Holding, u64>,
}

impl DiskUsage {
    pub fn games_bytes(&self) -> u64 {
        self.games.iter().map(|g| g.bytes).sum()
    }

    pub fn support_bytes(&self) -> u64 {
        self.support.values().sum()
    }

    /// What the disk holds besides Irori's games and their companions.
    pub fn other_bytes(&self) -> u64 {
        self.drive
            .used()
            .saturating_sub(self.games_bytes() + self.support_bytes())
    }
}

/// A kind of cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CleanupKind {
    /// Partly downloaded files no download will finish, unfinished installs
    /// and launcher installers already used.
    DownloadLeftovers,
    /// Wine prefixes no installed game or launcher uses.
    OrphanPrefixes,
    /// Wine and Proton builds Irori downloaded that nothing uses and a newer
    /// build of the same kind replaces.
    UnusedWineBuilds,
    /// Cover and backdrop thumbnails, made again when shown.
    ArtCache,
    /// Game, install and older Irori logs.
    Logs,
}

impl CleanupKind {
    pub const ALL: [CleanupKind; 5] = [
        CleanupKind::DownloadLeftovers,
        CleanupKind::OrphanPrefixes,
        CleanupKind::UnusedWineBuilds,
        CleanupKind::ArtCache,
        CleanupKind::Logs,
    ];
}

/// Why an item is offered, for its label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemNote {
    None,
    /// A partly downloaded file in this installed game's folder.
    PartOf(String),
    /// The prefix of this game, which is not installed.
    PrefixOf(String),
    /// An install of this folder name that never finished.
    Unfinished,
    /// Installers of third-party launchers, kept after their setup.
    LauncherInstallers,
    /// This many log files.
    LogFiles(usize),
}

/// One thing a cleanup deletes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupItem {
    pub path: PathBuf,
    pub bytes: u64,
    pub note: ItemNote,
}

impl CleanupItem {
    /// The folder or file name.
    pub fn name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

/// What one kind of cleanup would delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleanup {
    pub kind: CleanupKind,
    pub items: Vec<CleanupItem>,
}

impl Cleanup {
    pub fn bytes(&self) -> u64 {
        self.items.iter().map(|i| i.bytes).sum()
    }
}

/// Disk use and what can be freed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageReport {
    /// Disks holding anything Irori measured, the most used by games first.
    pub disks: Vec<DiskUsage>,
    /// One entry per [`CleanupKind`], in [`CleanupKind::ALL`] order.
    pub cleanups: Vec<Cleanup>,
}

impl StorageReport {
    pub fn cleanup(&self, kind: CleanupKind) -> Option<&Cleanup> {
        self.cleanups.iter().find(|c| c.kind == kind)
    }
}

/// A measured folder or file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub holding: Holding,
    /// Where it really is, links followed.
    pub path: PathBuf,
    pub bytes: u64,
    pub game: Option<(GameId, String)>,
}

/// Group `located` by the disk holding each (the deepest mount point its path
/// starts with). Something inside another measured folder (a launcher's game
/// in the launcher's prefix) counts for itself and is taken out of the
/// folder around it. Disks with nothing on them are left out.
pub fn aggregate(located: &[Located], drives: &[Drive]) -> Vec<DiskUsage> {
    let mut sizes: Vec<u64> = located.iter().map(|l| l.bytes).collect();
    for (inner, item) in located.iter().enumerate() {
        // A folder reached twice is taken out of the one around it once.
        if located[..inner].iter().any(|l| l.path == item.path) {
            continue;
        }
        let outer = located
            .iter()
            .enumerate()
            .filter(|(ix, other)| {
                *ix != inner && other.path != item.path && item.path.starts_with(&other.path)
            })
            .max_by_key(|(_, other)| other.path.components().count());
        if let Some((ix, _)) = outer {
            sizes[ix] = sizes[ix].saturating_sub(item.bytes);
        }
    }
    let mut seen: HashSet<&Path> = HashSet::new();
    let mut disks: Vec<DiskUsage> = Vec::new();
    for (item, bytes) in located.iter().zip(sizes) {
        // The same folder reached twice (two games in one folder) counts once.
        if !seen.insert(&item.path) {
            continue;
        }
        let Some(drive) = drives
            .iter()
            .filter(|d| item.path.starts_with(&d.mount))
            .max_by_key(|d| d.mount.components().count())
        else {
            continue;
        };
        let disk = match disks.iter().position(|d| d.drive.same_volume(drive)) {
            Some(ix) => {
                // A volume is shown at its topmost mount.
                if drive.mount.components().count() < disks[ix].drive.mount.components().count() {
                    disks[ix].drive = drive.clone();
                }
                &mut disks[ix]
            }
            None => {
                disks.push(DiskUsage {
                    drive: drive.clone(),
                    games: Vec::new(),
                    support: BTreeMap::new(),
                });
                disks.last_mut().expect("just pushed")
            }
        };
        match (&item.holding, &item.game) {
            (Holding::Game, Some((id, title))) => disk.games.push(GameSize {
                id: id.clone(),
                title: title.clone(),
                bytes,
            }),
            (holding, _) => *disk.support.entry(*holding).or_default() += bytes,
        }
    }
    for disk in &mut disks {
        disk.games.sort_by(|a, b| {
            b.bytes
                .cmp(&a.bytes)
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
    }
    disks.sort_by(|a, b| {
        b.games_bytes()
            .cmp(&a.games_bytes())
            .then(b.support_bytes().cmp(&a.support_bytes()))
    });
    disks
}

/// The prefixes among `candidates` nothing uses: not equal to a used path,
/// and not holding one (a launcher's prefix holds its games' folders).
pub fn orphan_prefixes<'a>(candidates: &'a [PathBuf], used: &[PathBuf]) -> Vec<&'a PathBuf> {
    candidates
        .iter()
        .filter(|prefix| !used.iter().any(|u| u.starts_with(prefix)))
        .collect()
}

/// Installed `releases` nothing uses (no `used` program is inside them) that a
/// newer installed release of the same kind replaces. The newest of each kind
/// stays, so a cleanup never leaves a kind without a build.
pub fn unused_builds<'a>(releases: &'a [Release], used: &[PathBuf]) -> Vec<&'a Release> {
    let dir = |r: &Release| r.install_dir.as_deref().map(PathBuf::from);
    let mut newest: HashMap<&str, &Release> = HashMap::new();
    for release in releases {
        let entry = newest.entry(release.kind.as_str()).or_insert(release);
        if (release.date.as_str(), release.version.as_str())
            > (entry.date.as_str(), entry.version.as_str())
        {
            *entry = release;
        }
    }
    releases
        .iter()
        .filter(|r| !std::ptr::eq(*r, newest[r.kind.as_str()]))
        .filter(|r| dir(r).is_some_and(|d| !used.iter().any(|u| u.starts_with(&d))))
        .collect()
}

/// The items of `kind` the report offers among `chosen`. Anything not offered
/// right now (a path from an older report, a game installed since) is left out.
pub fn select(report: &StorageReport, kind: CleanupKind, chosen: &[PathBuf]) -> Vec<CleanupItem> {
    report
        .cleanup(kind)
        .map(|c| {
            c.items
                .iter()
                .filter(|i| chosen.contains(&i.path))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// A folder's size as last measured.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Cached {
    bytes: u64,
    /// The folder's modified time, in Unix seconds.
    modified: u64,
    /// When it was measured, in Unix seconds.
    measured: u64,
    /// Partly downloaded files inside, with their sizes.
    #[serde(default)]
    parts: Vec<(PathBuf, u64)>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SizeCache {
    folders: HashMap<PathBuf, Cached>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn modified(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs())
}

/// Measures folders, reusing what the cache knows.
struct Measurer {
    cache: SizeCache,
    rescan: bool,
    now: u64,
    file: PathBuf,
}

impl Measurer {
    fn open(file: PathBuf, rescan: bool) -> Self {
        Self {
            cache: irori_core::json_store::read_or_default(&file),
            rescan,
            now: now(),
            file,
        }
    }

    /// The size of `path` (a folder or a file) and the partly downloaded files
    /// in it. Links are not followed: what they point at is counted where it is.
    fn measure(&mut self, path: &Path) -> Option<Cached> {
        let meta = std::fs::symlink_metadata(path).ok()?;
        if !meta.is_dir() {
            return Some(Cached {
                bytes: meta.len(),
                ..Cached::default()
            });
        }
        let stamp = modified(&meta);
        if !self.rescan
            && let Some(hit) = self.cache.folders.get(path)
            && hit.modified == stamp
            && self.now.saturating_sub(hit.measured) < SIZE_MAX_AGE
        {
            return Some(hit.clone());
        }
        let mut found = Cached {
            modified: stamp,
            measured: self.now,
            ..Cached::default()
        };
        let mut stack = vec![path.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                if meta.is_dir() {
                    stack.push(entry.path());
                } else if meta.is_file() {
                    found.bytes += meta.len();
                    if entry.file_name().to_string_lossy().ends_with(PART_SUFFIX) {
                        found.parts.push((entry.path(), meta.len()));
                    }
                }
            }
        }
        self.cache.folders.insert(path.to_path_buf(), found.clone());
        Some(found)
    }

    fn forget(&mut self, path: &Path) {
        self.cache.folders.retain(|p, _| !p.starts_with(path));
    }

    fn save(&self) {
        if let Err(e) = irori_core::json_store::write(&self.file, &self.cache) {
            tracing::debug!("saving folder sizes: {e:#}");
        }
    }
}

/// Where `path` really is, following links (a games folder linked to another
/// disk counts for that disk).
fn resolve(path: &Path) -> PathBuf {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    // Windows canonical paths are verbatim (`\\?\C:\…`); drives mount at `C:\`.
    if cfg!(windows)
        && let Some(plain) = resolved.to_str().and_then(|s| s.strip_prefix(r"\\?\"))
    {
        return PathBuf::from(plain);
    }
    resolved
}

fn read_drives() -> Vec<Drive> {
    sysinfo::Disks::new_with_refreshed_list()
        .list()
        .iter()
        .map(|d| Drive {
            name: d.name().to_string_lossy().into_owned(),
            mount: d.mount_point().to_path_buf(),
            total: d.total_space(),
            free: d.available_space(),
        })
        .collect()
}

/// Every absolute path a JSON document names, for state files whose layout
/// this module doesn't know (third-party launchers' setups).
fn paths_in_json(value: &serde_json::Value, out: &mut Vec<PathBuf>) {
    match value {
        serde_json::Value::String(s) if Path::new(s).is_absolute() => out.push(PathBuf::from(s)),
        serde_json::Value::Array(items) => items.iter().for_each(|v| paths_in_json(v, out)),
        serde_json::Value::Object(map) => map.values().for_each(|v| paths_in_json(v, out)),
        _ => {}
    }
}

/// What the report is built from, gathered on the async side.
struct Plan {
    app: Arc<AppSettings>,
    /// Installed games with their prefix and Wine program.
    installed: Vec<(Arc<GameInfo>, PathBuf, Option<PathBuf>)>,
    /// Games not installed, by their default prefix.
    uninstalled_prefixes: HashMap<PathBuf, String>,
    /// Games with a download queued, running or paused.
    downloading: HashSet<GameId>,
}

impl Backend {
    fn storage_plan(&self) -> Plan {
        let app = self.settings.app();
        let library = self.library();
        let mut installed = Vec::new();
        let mut uninstalled_prefixes = HashMap::new();
        for game in library.all_games().filter(|g| !g.is_dlc) {
            let id = game.id();
            let settings = self.settings.game(&id);
            if game.is_installed {
                let prefix = irori_runtime::prefix_path(&app, &settings, game);
                let wine = [&settings.wine_version, &app.game.wine_version]
                    .into_iter()
                    .find(|w| w.is_set())
                    .map(|w| PathBuf::from(&w.bin));
                installed.push((game.clone(), prefix, wine));
            } else {
                uninstalled_prefixes.insert(
                    irori_runtime::default_prefix(&app, game),
                    game.title.clone(),
                );
            }
        }
        let downloading = self
            .downloads()
            .active
            .into_iter()
            .map(|item| item.id)
            .collect();
        Plan {
            app,
            installed,
            uninstalled_prefixes,
            downloading,
        }
    }

    /// Measure disk use and list what can be freed. Folder sizes come from
    /// the cache where it is fresh; `rescan` measures everything again.
    pub async fn storage_report(&self, rescan: bool) -> Result<Arc<StorageReport>> {
        let plan = self.storage_plan();
        let file = Paths::get().cache.join("storage.json");
        tokio::task::spawn_blocking(move || {
            let mut measurer = Measurer::open(file, rescan);
            let report = build(&plan, &mut measurer, read_drives());
            measurer.save();
            Arc::new(report)
        })
        .await
        .context("measuring disk use")
    }

    /// Delete what `kind` offers among `chosen`, returning the bytes freed.
    /// The report is rebuilt first, so only what is still safe to delete goes.
    pub async fn clean_up(&self, kind: CleanupKind, chosen: Vec<PathBuf>) -> Result<u64> {
        let plan = self.storage_plan();
        let file = Paths::get().cache.join("storage.json");
        let freed = tokio::task::spawn_blocking(move || -> Result<u64> {
            let mut measurer = Measurer::open(file, false);
            let report = build(&plan, &mut measurer, read_drives());
            let items = select(&report, kind, &chosen);
            let mut freed = 0;
            let mut failed = Vec::new();
            for item in &items {
                match delete(kind, item, &plan.app) {
                    Ok(()) => freed += item.bytes,
                    Err(e) => failed.push(format!("{e:#}")),
                }
                measurer.forget(&item.path);
                if let Some(parent) = item.path.parent() {
                    measurer.forget(parent);
                }
            }
            measurer.save();
            match failed.first() {
                Some(first) if freed == 0 => anyhow::bail!("{first}"),
                Some(first) => {
                    tracing::warn!("{} items could not be deleted: {first}", failed.len());
                    Ok(freed)
                }
                None => Ok(freed),
            }
        })
        .await
        .context("cleaning up")??;
        if kind == CleanupKind::UnusedWineBuilds {
            self.bus.emit(Event::WineVersionsChanged);
        }
        Ok(freed)
    }
}

/// Delete one cleanup item.
fn delete(kind: CleanupKind, item: &CleanupItem, app: &AppSettings) -> Result<()> {
    let path = &item.path;
    fs_guard::ensure_deletable(path, app)?;
    let removed = match kind {
        CleanupKind::UnusedWineBuilds => {
            let release = manager::cached_releases()
                .into_iter()
                .find(|r| r.install_dir.as_deref().map(Path::new) == Some(path.as_path()))
                .with_context(|| format!("{} is no longer a known build", path.display()))?;
            // Forgetting it goes through the manager, so its list stays true.
            // This runs on a blocking thread of the backend's runtime.
            return tokio::runtime::Handle::current().block_on(manager::remove(&release));
        }
        CleanupKind::Logs => {
            let keep = Paths::get().launcher_log_file();
            let mut stack = vec![path.clone()];
            while let Some(dir) = stack.pop() {
                for entry in std::fs::read_dir(&dir)?.flatten() {
                    let p = entry.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if p != keep {
                        // A log a running game still writes may be locked
                        // (Windows); it goes next time.
                        let _ = std::fs::remove_file(&p);
                    }
                }
            }
            Ok(())
        }
        _ if path.is_dir() => std::fs::remove_dir_all(path),
        _ => std::fs::remove_file(path),
    };
    removed.with_context(|| format!("deleting {}", path.display()))
}

fn build(plan: &Plan, measurer: &mut Measurer, drives: Vec<Drive>) -> StorageReport {
    let paths = Paths::get();
    let app = &plan.app;
    let mut located: Vec<Located> = Vec::new();
    let mut used_prefixes: Vec<PathBuf> = Vec::new();
    let mut used_programs: Vec<PathBuf> = Vec::new();
    let mut leftovers: Vec<CleanupItem> = Vec::new();
    let mut game_dirs: HashSet<PathBuf> = HashSet::new();

    for (game, prefix, wine) in &plan.installed {
        used_prefixes.push(resolve(prefix));
        used_programs.extend(wine.iter().map(|w| resolve(w)));
        let Some(install) = game.install.as_ref().filter(|i| !i.install_path.is_empty()) else {
            continue;
        };
        let dir = resolve(Path::new(&install.install_path));
        // A launcher's game lives in its prefix: that prefix is in use.
        used_prefixes.push(dir.clone());
        game_dirs.insert(dir.clone());
        let Some(size) = measurer.measure(&dir) else {
            continue;
        };
        let id = game.id();
        if !plan.downloading.contains(&id) && game.runner().manages_installs() {
            leftovers.extend(size.parts.iter().map(|(path, bytes)| CleanupItem {
                path: path.clone(),
                bytes: *bytes,
                note: ItemNote::PartOf(game.title.clone()),
            }));
        }
        located.push(Located {
            holding: Holding::Game,
            path: dir,
            bytes: size.bytes,
            game: Some((id, game.title.clone())),
        });
    }

    // Prefixes, beside each other in the prefix folder.
    let prefix_root = if app.default_wine_prefix_dir.is_empty() {
        irori_core::paths::expand_home(&app.default_install_path).join("Prefixes")
    } else {
        irori_core::paths::expand_home(&app.default_wine_prefix_dir)
    };
    let launchers_state = paths
        .store_config_dir(Runner::Launchers)
        .join("launchers.json");
    if let Ok(Some(state)) = irori_core::json_store::read::<serde_json::Value>(&launchers_state) {
        let mut named = Vec::new();
        paths_in_json(&state, &mut named);
        // A launcher's prefix, and the Wine build it was set up with.
        used_prefixes.extend(named.iter().map(|p| resolve(p)));
        used_programs.extend(named.into_iter().map(|p| resolve(&p)));
    }
    if !app.game.wine_prefix.trim().is_empty() {
        used_prefixes.push(resolve(&irori_core::paths::expand_home(
            &app.game.wine_prefix,
        )));
    }
    let mut prefixes: Vec<PathBuf> = std::fs::read_dir(&prefix_root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && irori_runtime::is_prefix(p))
        .map(|p| resolve(&p))
        .collect();
    prefixes.sort();
    let mut prefix_sizes: HashMap<PathBuf, u64> = HashMap::new();
    for prefix in &prefixes {
        if let Some(size) = measurer.measure(prefix) {
            prefix_sizes.insert(prefix.clone(), size.bytes);
            located.push(Located {
                holding: Holding::Prefix,
                path: prefix.clone(),
                bytes: size.bytes,
                game: None,
            });
        }
    }
    // Prefixes kept elsewhere by a game's own setting count too.
    for prefix in used_prefixes.iter().filter(|p| irori_runtime::is_prefix(p)) {
        if !prefix_sizes.contains_key(prefix)
            && !game_dirs.contains(prefix)
            && let Some(size) = measurer.measure(prefix)
        {
            prefix_sizes.insert(prefix.clone(), size.bytes);
            located.push(Located {
                holding: Holding::Prefix,
                path: prefix.clone(),
                bytes: size.bytes,
                game: None,
            });
        }
    }
    let uninstalled: HashMap<PathBuf, &String> = plan
        .uninstalled_prefixes
        .iter()
        .map(|(p, title)| (resolve(p), title))
        .collect();
    let orphans: Vec<CleanupItem> = orphan_prefixes(&prefixes, &used_prefixes)
        .into_iter()
        .filter(|p| !fs_guard::protected_dirs(app).contains(p))
        .map(|p| CleanupItem {
            path: p.clone(),
            bytes: prefix_sizes.get(p).copied().unwrap_or(0),
            note: uninstalled
                .get(p)
                .map_or(ItemNote::None, |t| ItemNote::PrefixOf((*t).clone())),
        })
        .collect();

    // Wine and Proton builds Irori downloaded.
    let releases: Vec<Release> = manager::cached_releases()
        .into_iter()
        .filter(Release::installed)
        .collect();
    let mut build_sizes: HashMap<PathBuf, u64> = HashMap::new();
    for release in &releases {
        let Some(dir) = release.install_dir.as_deref().map(PathBuf::from) else {
            continue;
        };
        if let Some(size) = measurer.measure(&dir) {
            build_sizes.insert(dir.clone(), size.bytes);
            located.push(Located {
                holding: Holding::WineBuild,
                path: resolve(&dir),
                bytes: size.bytes,
                game: None,
            });
        }
    }
    let unused: Vec<CleanupItem> = unused_builds(&releases, &used_programs)
        .into_iter()
        .filter_map(|r| {
            let dir = PathBuf::from(r.install_dir.as_deref()?);
            Some(CleanupItem {
                bytes: build_sizes.get(&dir).copied().unwrap_or(0),
                path: dir,
                note: ItemNote::None,
            })
        })
        .collect();

    // Installs that never finished: folders in the games folder holding
    // partly downloaded files and no installed game. While anything
    // downloads they are left alone, since one of them may be its target.
    let install_root = irori_core::paths::expand_home(&app.default_install_path);
    if let Ok(entries) = std::fs::read_dir(&install_root) {
        for dir in entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
            let dir = resolve(&dir);
            if game_dirs.contains(&dir)
                || prefix_sizes.contains_key(&dir)
                || dir == resolve(&prefix_root)
            {
                continue;
            }
            let Some(size) = measurer.measure(&dir) else {
                continue;
            };
            if size.parts.is_empty() {
                continue;
            }
            located.push(Located {
                holding: Holding::Unfinished,
                path: dir.clone(),
                bytes: size.bytes,
                game: None,
            });
            if plan.downloading.is_empty() {
                leftovers.push(CleanupItem {
                    path: dir,
                    bytes: size.bytes,
                    note: ItemNote::Unfinished,
                });
            }
        }
    }
    let installers = paths.store_cache_dir(Runner::Launchers).join("installers");
    if let Some(size) = measurer.measure(&installers).filter(|s| s.bytes > 0) {
        leftovers.push(CleanupItem {
            path: installers,
            bytes: size.bytes,
            note: ItemNote::LauncherInstallers,
        });
    }

    // Emulators, then Irori's own caches and logs.
    let engines = paths.data.join("engines");
    if let Some(size) = measurer.measure(&engines) {
        located.push(Located {
            holding: Holding::Emulators,
            path: resolve(&engines),
            bytes: size.bytes,
            game: None,
        });
    }
    for dir in [paths.cache.clone(), paths.logs_dir()] {
        if let Some(size) = measurer.measure(&dir) {
            located.push(Located {
                holding: Holding::IroriData,
                path: resolve(&dir),
                bytes: size.bytes,
                game: None,
            });
        }
    }
    let thumbs = paths.images_dir().join("thumbs");
    let art: Vec<CleanupItem> = measurer
        .measure(&thumbs)
        .filter(|s| s.bytes > 0)
        .map(|s| CleanupItem {
            path: thumbs,
            bytes: s.bytes,
            note: ItemNote::None,
        })
        .into_iter()
        .collect();
    let logs: Vec<CleanupItem> = log_files(&paths.logs_dir(), &paths.launcher_log_file())
        .filter(|(count, _)| *count > 0)
        .map(|(count, bytes)| CleanupItem {
            path: paths.logs_dir(),
            bytes,
            note: ItemNote::LogFiles(count),
        })
        .into_iter()
        .collect();

    leftovers.sort_by_key(|item| std::cmp::Reverse(item.bytes));
    let cleanups = vec![
        Cleanup {
            kind: CleanupKind::DownloadLeftovers,
            items: leftovers,
        },
        Cleanup {
            kind: CleanupKind::OrphanPrefixes,
            items: orphans,
        },
        Cleanup {
            kind: CleanupKind::UnusedWineBuilds,
            items: unused,
        },
        Cleanup {
            kind: CleanupKind::ArtCache,
            items: art,
        },
        Cleanup {
            kind: CleanupKind::Logs,
            items: logs,
        },
    ];
    StorageReport {
        disks: aggregate(&located, &drives),
        cleanups,
    }
}

/// How many log files the logs folder holds besides `keep`, and their size.
/// Logs change with every launch, so they are counted, never cached.
fn log_files(dir: &Path, keep: &Path) -> Option<(usize, u64)> {
    let mut found = (0, 0);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() && entry.path() != keep {
                found.0 += 1;
                found.1 += meta.len();
            }
        }
    }
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(mount: &str, total: u64, free: u64) -> Drive {
        Drive {
            name: String::new(),
            mount: mount.into(),
            total,
            free,
        }
    }

    fn game(app: &str, path: &str, bytes: u64) -> Located {
        Located {
            holding: Holding::Game,
            path: path.into(),
            bytes,
            game: Some((GameId::new(Runner::Gog, app), app.to_uppercase())),
        }
    }

    fn other(holding: Holding, path: &str, bytes: u64) -> Located {
        Located {
            holding,
            path: path.into(),
            bytes,
            game: None,
        }
    }

    #[test]
    fn sizes_add_up_per_disk_on_the_deepest_mount() {
        let drives = [drive("/", 1000, 300), drive("/mnt/games", 5000, 1000)];
        let located = [
            game("a", "/home/u/Games/A", 100),
            game("b", "/mnt/games/B", 900),
            game("c", "/mnt/games/C", 2000),
            other(Holding::Prefix, "/mnt/games/Prefixes/C", 50),
            other(
                Holding::WineBuild,
                "/home/u/.local/share/irori/tools/wine/x",
                30,
            ),
            other(Holding::IroriData, "/home/u/.cache/irori", 20),
            // On no known disk: left out.
            other(Holding::Emulators, "relative/engines", 7),
        ];
        let disks = aggregate(&located, &drives);
        assert_eq!(disks.len(), 2);
        let games = &disks[0];
        assert_eq!(games.drive.mount, Path::new("/mnt/games"));
        assert_eq!(games.games_bytes(), 2900);
        assert_eq!(games.games[0].title, "C");
        assert_eq!(games.support[&Holding::Prefix], 50);
        assert_eq!(games.other_bytes(), 4000 - 2950);
        let root = &disks[1];
        assert_eq!(root.games_bytes(), 100);
        assert_eq!(root.support_bytes(), 50);
        assert_eq!(root.other_bytes(), 700 - 150);
    }

    #[test]
    fn mounts_of_one_volume_are_one_disk() {
        let subvolume = |mount: &str| Drive {
            name: "/dev/nvme0n1p3".into(),
            ..drive(mount, 1000, 100)
        };
        let drives = [
            subvolume("/"),
            subvolume("/home"),
            drive("/mnt/usb", 1000, 100),
        ];
        let located = [
            game("a", "/home/u/Games/A", 300),
            other(Holding::IroriData, "/var/cache/irori", 20),
            game("b", "/mnt/usb/B", 50),
        ];
        let disks = aggregate(&located, &drives);
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].drive.mount, Path::new("/"));
        assert_eq!(disks[0].games_bytes(), 300);
        assert_eq!(disks[0].support_bytes(), 20);
        assert_eq!(disks[0].other_bytes(), 900 - 320);
        // Same size and free space but no device name to match: kept apart.
        assert_eq!(disks[1].drive.mount, Path::new("/mnt/usb"));

        // Free space read a moment apart still matches by device.
        let busy = Drive {
            free: 90,
            ..subvolume("/home")
        };
        assert!(subvolume("/").same_volume(&busy));
        let tmpfs = |mount: &str, free| Drive {
            name: "tmpfs".into(),
            ..drive(mount, 1000, free)
        };
        assert!(!tmpfs("/tmp", 900).same_volume(&tmpfs("/dev/shm", 1000)));
    }

    #[test]
    fn a_game_inside_a_prefix_counts_once() {
        let drives = [drive("/", 10_000, 0)];
        let located = [
            other(Holding::Prefix, "/p/Battle.net", 5000),
            game("d", "/p/Battle.net/drive_c/Games/Diablo", 4000),
            game("d2", "/p/Battle.net/drive_c/Games/Diablo", 4000),
        ];
        let disks = aggregate(&located, &drives);
        assert_eq!(disks[0].games_bytes(), 4000);
        assert_eq!(disks[0].support[&Holding::Prefix], 1000);
        // More used than measured never goes negative.
        assert_eq!(disks[0].other_bytes(), 10_000 - 5000);
    }

    #[test]
    fn prefixes_nothing_uses_are_orphans() {
        let candidates: Vec<PathBuf> = ["/p/Hades", "/p/Battle.net", "/p/Old", "/p/Shared"]
            .map(PathBuf::from)
            .to_vec();
        let used = [
            PathBuf::from("/p/Hades"),
            // A launcher's game installed inside its prefix.
            PathBuf::from("/p/Battle.net/drive_c/Program Files/Diablo"),
            // A game's custom prefix outside the folder doesn't save anything here.
            PathBuf::from("/elsewhere/Shared"),
        ];
        let orphans = orphan_prefixes(&candidates, &used);
        assert_eq!(orphans, [&candidates[2], &candidates[3]]);
    }

    fn release(kind: &str, version: &str, date: &str, dir: &str) -> Release {
        Release {
            version: version.into(),
            kind: kind.into(),
            date: date.into(),
            install_dir: Some(dir.into()),
            is_installed: Some(true),
            ..Release::default()
        }
    }

    #[test]
    fn only_old_builds_nothing_uses_go() {
        let releases = [
            release(
                "GE-Proton",
                "GE-Proton9-1",
                "2024-01-01",
                "/t/proton/GE-Proton9-1",
            ),
            release(
                "GE-Proton",
                "GE-Proton9-5",
                "2024-06-01",
                "/t/proton/GE-Proton9-5",
            ),
            release(
                "GE-Proton",
                "GE-Proton10-1",
                "2025-02-01",
                "/t/proton/GE-Proton10-1",
            ),
            release("Wine-GE", "wine-ge-8", "2023-01-01", "/t/wine/wine-ge-8"),
        ];
        let used = [PathBuf::from("/t/proton/GE-Proton9-5/proton")];
        let unused: Vec<&str> = unused_builds(&releases, &used)
            .into_iter()
            .map(|r| r.version.as_str())
            .collect();
        // 9-5 is used, 10-1 is the newest Proton, wine-ge-8 the only Wine.
        assert_eq!(unused, ["GE-Proton9-1"]);
    }

    #[test]
    fn a_cleanup_deletes_only_what_the_report_offers() {
        let item = |path: &str| CleanupItem {
            path: path.into(),
            bytes: 10,
            note: ItemNote::None,
        };
        let report = StorageReport {
            disks: Vec::new(),
            cleanups: vec![
                Cleanup {
                    kind: CleanupKind::OrphanPrefixes,
                    items: vec![item("/p/Old"), item("/p/Older")],
                },
                Cleanup {
                    kind: CleanupKind::Logs,
                    items: vec![item("/logs")],
                },
            ],
        };
        let chosen = [
            PathBuf::from("/p/Older"),
            PathBuf::from("/home/u"),
            PathBuf::from("/logs"),
        ];
        let picked = select(&report, CleanupKind::OrphanPrefixes, &chosen);
        assert_eq!(picked, [item("/p/Older")]);
        assert!(select(&report, CleanupKind::ArtCache, &chosen).is_empty());
    }

    #[test]
    fn measuring_counts_files_finds_parts_and_reuses_the_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("Game");
        std::fs::create_dir_all(game.join("data")).unwrap();
        std::fs::write(game.join("game.exe"), [0u8; 100]).unwrap();
        std::fs::write(game.join("data/pak.bin.irori-part"), [0u8; 40]).unwrap();
        let file = tmp.path().join("storage.json");
        let mut measurer = Measurer::open(file.clone(), false);
        let size = measurer.measure(&game).unwrap();
        assert_eq!(size.bytes, 140);
        assert_eq!(
            size.parts,
            [(game.join("data/pak.bin.irori-part"), 40)].to_vec()
        );
        measurer.save();

        // A change deep inside goes unseen until a rescan...
        std::fs::write(game.join("data/more.bin"), [0u8; 60]).unwrap();
        let mut cached = Measurer::open(file.clone(), false);
        assert_eq!(cached.measure(&game).unwrap().bytes, 140);
        let mut rescan = Measurer::open(file, true);
        assert_eq!(rescan.measure(&game).unwrap().bytes, 200);
        // ...while a file is always measured as it is.
        assert_eq!(rescan.measure(&game.join("game.exe")).unwrap().bytes, 100);
        assert!(rescan.measure(&tmp.path().join("missing")).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn launcher_state_names_its_prefixes() {
        let state = serde_json::json!({
            "setups": {"battlenet": {"prefix": "/p/Battle.net", "wine": {"bin": "/t/wine/x/bin/wine"}}},
            "note": "not a path",
        });
        let mut out = Vec::new();
        paths_in_json(&state, &mut out);
        out.sort();
        assert_eq!(
            out,
            [PathBuf::from("/p/Battle.net"), "/t/wine/x/bin/wine".into()]
        );
    }
}
