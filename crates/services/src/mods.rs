//! Per-game mods ([`irori_mods`]) behind [`Backend`]: installing from
//! archives, FOMOD installers, Thunderstore and Nexus Mods, the load order,
//! and applying the enabled mods while the game runs.
//!
//! One lock per game keeps an install, a reorder and a launch from touching
//! the same mods at once. Mods are applied right before the game starts and
//! taken out after it exits (and after its saves sync); a deployment a crash
//! left behind is undone when Irori next starts. Changes emit
//! [`Event::ModsChanged`].

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, Result, bail};
use irori_core::process::CancellationToken;
use irori_core::{Event, GameId, GameInfo, Notice, NoticeLevel, Paths, Runner};
use irori_mods::fomod::{self, Choices, FileState, ModuleConfig};
use irori_mods::nexus::{self, ApiKey, NexusUser, NxmLink};
use irori_mods::thunderstore::{self, Community, Ecosystem, GameKeys, SearchPage};
use irori_mods::{
    Applied, Conflicts, DeployMethod, DownloadProgress, ModEntry, ModLibrary, ModList, ModSource,
    Staging, conflicts, deploy, layout, overlay,
};
use once_cell::sync::Lazy;
use parking_lot::{Mutex, RwLock};

use crate::Backend;

/// What the mods tab shows.
#[derive(Debug, Clone, Default)]
pub struct ModsView {
    pub list: ModList,
    pub conflicts: Conflicts,
    /// Files the game wrote into mod files and folders.
    pub written_files: usize,
    /// Mods are in the game folder right now.
    pub applied: bool,
    /// `fuse-overlayfs` can mount mods here.
    pub overlay_available: bool,
    /// The game's mod folder.
    pub folder: PathBuf,
    /// Icons of mods that came with one, by mod id.
    pub icons: HashMap<String, PathBuf>,
}

/// An archive install that needs the user's choices first.
#[derive(Debug, Clone)]
pub struct FomodPrompt {
    pub token: u64,
    pub mod_name: String,
    pub config: Arc<ModuleConfig>,
    /// The installer's folder; images are relative to it.
    pub root: PathBuf,
    /// The installer's images found on disk, by the path the installer
    /// names them with, so drawing a step reads no files.
    pub images: Arc<HashMap<String, PathBuf>>,
    /// Lower-cased paths of files the installer's conditions ask about
    /// that the game or an enabled mod has.
    pub present: Arc<HashSet<String>>,
}

impl FomodPrompt {
    /// Answers the installer's file conditions.
    pub fn file_state(&self, path: &str) -> FileState {
        file_state(&self.present, path)
    }

    /// The file of an image the installer names, when it has one.
    pub fn image(&self, path: &str) -> Option<&Path> {
        self.images.get(path).map(PathBuf::as_path)
    }
}

fn installer_images(config: &ModuleConfig, root: &Path) -> HashMap<String, PathBuf> {
    let plugins = config
        .steps
        .iter()
        .flat_map(|s| &s.groups)
        .flat_map(|g| &g.plugins)
        .filter_map(|p| p.image.as_deref());
    config
        .image
        .as_deref()
        .into_iter()
        .chain(plugins)
        .filter_map(|rel| Some((rel.to_string(), fomod::image_path(root, rel)?)))
        .collect()
}

fn file_state(present: &HashSet<String>, path: &str) -> FileState {
    if present.contains(&path.replace('\\', "/").to_lowercase()) {
        FileState::Active
    } else {
        FileState::Missing
    }
}

/// How an install from an archive ended.
#[derive(Debug, Clone)]
pub enum InstallOutcome {
    Installed {
        name: String,
    },
    /// A FOMOD installer asks; answer with [`Backend::finish_fomod`].
    Choose(FomodPrompt),
}

/// Live progress of a download, for the UI to poll while it renders.
pub type ModProgress = Arc<DownloadProgress>;

/// An installer waiting for choices.
struct Pending {
    game: GameId,
    game_dir: PathBuf,
    staging: Staging,
    root: PathBuf,
    config: Arc<ModuleConfig>,
    entry: ModEntry,
    present: Arc<HashSet<String>>,
}

static PENDING: Lazy<Mutex<HashMap<u64, Pending>>> = Lazy::new(Default::default);
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
static LOCKS: Lazy<Mutex<HashMap<GameId, Arc<tokio::sync::Mutex<()>>>>> =
    Lazy::new(Default::default);
static ECOSYSTEM: Lazy<RwLock<Option<Arc<Ecosystem>>>> = Lazy::new(Default::default);
/// Pending installers past this many are dropped, oldest first.
const MAX_PENDING: usize = 8;

fn lock_for(id: &GameId) -> Arc<tokio::sync::Mutex<()>> {
    LOCKS.lock().entry(id.clone()).or_default().clone()
}

fn nexus_key_file() -> PathBuf {
    Paths::get().config.join("nexus").join("key.json")
}

fn ecosystem_cache() -> PathBuf {
    Paths::get()
        .cache
        .join("thunderstore")
        .join("ecosystem.json")
}

/// Whether mods can be managed for this game: installed into a folder of
/// its own, and not a ROM or a web page. Reads no files, so screens may ask
/// while they draw.
pub fn supports_mods(game: &GameInfo) -> bool {
    game.is_installed
        && game.browser_url.is_none()
        && !matches!(game.runner(), Runner::Emulation)
        && game.install_path().is_some()
}

/// A display name from an archive's file name: Nexus appends ids and a
/// timestamp (`SkyUI_5_2_SE-12604-5-2SE-1572000000.7z`).
pub fn name_from_archive(file_name: &str) -> String {
    // `Path::file_stem` keeps ".zip" whole, as if it were a hidden file.
    let file_name = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    let stem = file_name
        .rsplit_once('.')
        .map_or(file_name, |(stem, _)| stem);
    let mut parts: Vec<&str> = stem.split('-').collect();
    if parts.len() >= 3
        && parts
            .last()
            .is_some_and(|p| p.len() >= 9 && p.bytes().all(|b| b.is_ascii_digit()))
        && parts
            .get(1)
            .is_some_and(|p| p.bytes().all(|b| b.is_ascii_digit()))
    {
        parts.truncate(1);
    }
    let name = parts.join("-").replace('_', " ");
    let name = name.trim();
    if name.is_empty() {
        "Mod".into()
    } else {
        name.to_string()
    }
}

impl Backend {
    fn mod_game(&self, id: &GameId) -> Result<(Arc<GameInfo>, PathBuf)> {
        let game = self
            .game(id)
            .with_context(|| format!("{id} is not in the library"))?;
        let dir = game
            .install_path()
            .filter(|dir| supports_mods(&game) && dir.is_dir())
            .with_context(|| {
                format!("{} is not installed in a folder Irori can mod", game.title)
            })?;
        Ok((game, dir))
    }

    /// [`Backend::mod_game`] for changes: not while the game runs.
    fn mod_game_idle(&self, id: &GameId) -> Result<(Arc<GameInfo>, PathBuf)> {
        if self.is_running(id) {
            bail!("mods can't change while the game runs");
        }
        self.mod_game(id)
    }

    fn mods_changed(&self, id: &GameId) {
        self.bus.emit(Event::ModsChanged(id.clone()));
    }

    /// The game's mods, their conflicts and whether they are applied.
    pub async fn mods(&self, id: &GameId) -> Result<ModsView> {
        let (_, game_dir) = self.mod_game(id)?;
        let lib = ModLibrary::for_game(id);
        let running = self.is_running(id);
        self.spawn_blocking(move || {
            let list = lib.load()?;
            let enabled: Vec<(String, Vec<String>)> = list
                .mods
                .iter()
                .filter(|m| m.enabled)
                .map(|m| Ok((m.id.clone(), lib.index(&m.id)?)))
                .collect::<Result<_>>()?;
            let refs: Vec<(&str, &[String])> = enabled
                .iter()
                .map(|(id, files)| (id.as_str(), files.as_slice()))
                .collect();
            let conflicts = conflicts::find(&refs, |rel| {
                let path = irori_mods::align_case(&game_dir, rel);
                game_dir.join(&path).is_file()
            });
            let icons = list
                .mods
                .iter()
                .filter_map(|m| {
                    let icon = lib.mod_dir(&m.id).ok()?.join("icon.png");
                    icon.is_file().then(|| (m.id.clone(), icon))
                })
                .collect();
            Ok(ModsView {
                conflicts,
                written_files: lib.written_files(),
                applied: running && deploy::is_deployed(&lib.manifest_file()),
                overlay_available: overlay::tool().is_some(),
                folder: lib.root().to_path_buf(),
                icons,
                list,
            })
        })
        .await?
    }

    async fn edit_mods(&self, id: &GameId, f: impl FnOnce(&mut ModList) -> bool) -> Result<()> {
        if self.is_running(id) {
            bail!("mods can't change while the game runs");
        }
        let lock = lock_for(id);
        let _guard = lock.lock().await;
        let lib = ModLibrary::for_game(id);
        let mut list = lib.load()?;
        if f(&mut list) {
            lib.save(&list)?;
            self.mods_changed(id);
        }
        Ok(())
    }

    pub async fn set_mod_enabled(&self, id: &GameId, mod_id: &str, enabled: bool) -> Result<()> {
        self.edit_mods(id, |list| match list.get_mut(mod_id) {
            Some(m) if m.enabled != enabled => {
                m.enabled = enabled;
                true
            }
            _ => false,
        })
        .await
    }

    /// Put the mods in this load order (earliest first).
    pub async fn reorder_mods(&self, id: &GameId, order: Vec<String>) -> Result<()> {
        self.edit_mods(id, |list| {
            let before: Vec<String> = list.mods.iter().map(|m| m.id.clone()).collect();
            list.reorder(&order);
            list.mods.iter().map(|m| &m.id).ne(before.iter())
        })
        .await
    }

    pub async fn set_mod_method(&self, id: &GameId, method: DeployMethod) -> Result<()> {
        self.edit_mods(id, |list| {
            let changed = list.method != method;
            list.method = method;
            changed
        })
        .await
    }

    pub async fn remove_mod(&self, id: &GameId, mod_id: &str) -> Result<()> {
        if self.is_running(id) {
            bail!("mods can't change while the game runs");
        }
        let lock = lock_for(id);
        let _guard = lock.lock().await;
        let lib = ModLibrary::for_game(id);
        let app = self.settings.app();
        let mod_id = mod_id.to_string();
        self.spawn_blocking(move || {
            let mut list = lib.load()?;
            lib.remove(&mut list, &mod_id, &app)
        })
        .await??;
        self.mods_changed(id);
        Ok(())
    }

    /// Forget the files the game wrote into mod files and folders.
    pub async fn clear_written_mod_files(&self, id: &GameId) -> Result<()> {
        if self.is_running(id) {
            bail!("mods can't change while the game runs");
        }
        let lock = lock_for(id);
        let _guard = lock.lock().await;
        let lib = ModLibrary::for_game(id);
        let app = self.settings.app();
        self.spawn_blocking(move || -> Result<()> {
            let dir = lib.written_dir();
            if dir.exists() {
                irori_core::fs_guard::ensure_deletable(&dir, &app)?;
                std::fs::remove_dir_all(&dir)
                    .with_context(|| format!("removing {}", dir.display()))?;
            }
            Ok(())
        })
        .await??;
        self.mods_changed(id);
        Ok(())
    }

    /// Install a mod from an archive the user picked.
    pub async fn install_mod_archive(
        &self,
        id: &GameId,
        archive: PathBuf,
    ) -> Result<InstallOutcome> {
        let file_name = archive
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let entry = new_entry(
            name_from_archive(&file_name),
            None,
            ModSource::Archive { file_name },
        );
        self.install_from_file(id, &archive, entry, None).await
    }

    /// Unpack `archive` and add it as `entry`, asking through a FOMOD
    /// installer when it has one. `staging` holds `archive` when it was
    /// downloaded, so both go away together.
    async fn install_from_file(
        &self,
        id: &GameId,
        archive: &Path,
        mut entry: ModEntry,
        staging: Option<Staging>,
    ) -> Result<InstallOutcome> {
        let (_, game_dir) = self.mod_game_idle(id)?;
        let lib = ModLibrary::for_game(id);
        let staging = match staging {
            Some(s) => s,
            None => lib.staging()?,
        };
        let unpacked = staging.path().join("unpacked");
        irori_mods::archive::extract(archive, &unpacked, &CancellationToken::new()).await?;
        let found = {
            let unpacked = unpacked.clone();
            self.spawn_blocking(move || fomod::find(&unpacked)).await?
        };
        if let Some((root, xml)) = found {
            let bytes = tokio::fs::read(&xml).await?;
            let config = fomod::parse(&bytes)?;
            if !config.name.is_empty() && matches!(entry.source, ModSource::Archive { .. }) {
                entry.name = config.name.clone();
            }
            let (present, images) = {
                let (config, game_dir, lib, root) =
                    (config.clone(), game_dir.clone(), lib.clone(), root.clone());
                self.spawn_blocking(move || {
                    (
                        present_files(&config, &game_dir, &lib),
                        installer_images(&config, &root),
                    )
                })
                .await?
            };
            let config = Arc::new(config);
            if !config.steps.is_empty() {
                let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
                let prompt = FomodPrompt {
                    token,
                    mod_name: entry.name.clone(),
                    config: config.clone(),
                    root: root.clone(),
                    images: Arc::new(images),
                    present: present.clone(),
                };
                let mut pending = PENDING.lock();
                if pending.len() >= MAX_PENDING
                    && let Some(oldest) = pending.keys().min().copied()
                {
                    pending.remove(&oldest);
                }
                pending.insert(
                    token,
                    Pending {
                        game: id.clone(),
                        game_dir,
                        staging,
                        root,
                        config,
                        entry,
                        present,
                    },
                );
                return Ok(InstallOutcome::Choose(prompt));
            }
            let choices = Choices::default();
            let name = entry.name.clone();
            self.add_fomod(
                id, &game_dir, &staging, &root, &config, &choices, &present, entry,
            )
            .await?;
            return Ok(InstallOutcome::Installed { name });
        }
        let name = entry.name.clone();
        let lock = lock_for(id);
        let _guard = lock.lock().await;
        let app = self.settings.app();
        let tree = staging.path().join("tree");
        let lib2 = lib.clone();
        self.spawn_blocking(move || -> Result<()> {
            let root = layout::content_root(&unpacked, &game_dir);
            let prefix = layout::target_prefix(&root, &game_dir);
            layout::import(&root, &tree, &game_dir, |rel| {
                Some(layout::prefixed(&prefix, rel))
            })?;
            let mut list = lib2.load()?;
            let replaces = list.find_source(&entry.source).map(|m| m.id.clone());
            lib2.add(&mut list, &tree, entry, replaces.as_deref(), &app)?;
            Ok(())
        })
        .await??;
        drop(staging);
        self.mods_changed(id);
        Ok(InstallOutcome::Installed { name })
    }

    #[allow(clippy::too_many_arguments)] // one install's parts, used once
    async fn add_fomod(
        &self,
        id: &GameId,
        game_dir: &Path,
        staging: &Staging,
        root: &Path,
        config: &ModuleConfig,
        choices: &Choices,
        present: &HashSet<String>,
        entry: ModEntry,
    ) -> Result<()> {
        self.mod_game_idle(id)?;
        let items = config.files(choices, &|p| file_state(present, p));
        let lock = lock_for(id);
        let _guard = lock.lock().await;
        let lib = ModLibrary::for_game(id);
        let app = self.settings.app();
        let (root, game_dir) = (root.to_path_buf(), game_dir.to_path_buf());
        let built = staging.path().join("fomod");
        let tree = staging.path().join("tree");
        self.spawn_blocking(move || -> Result<()> {
            if fomod::install(&root, &items, &built)? == 0 {
                bail!("the choices install no files");
            }
            let prefix = layout::data_dir(&game_dir).unwrap_or_default();
            layout::import(&built, &tree, &game_dir, |rel| {
                Some(layout::prefixed(&prefix, rel))
            })?;
            let mut list = lib.load()?;
            let replaces = list.find_source(&entry.source).map(|m| m.id.clone());
            lib.add(&mut list, &tree, entry, replaces.as_deref(), &app)?;
            Ok(())
        })
        .await??;
        self.mods_changed(id);
        Ok(())
    }

    /// Finish a FOMOD install with the user's choices.
    pub async fn finish_fomod(&self, token: u64, choices: Choices) -> Result<String> {
        let pending = PENDING
            .lock()
            .remove(&token)
            .context("this installer has expired; start the install again")?;
        let name = pending.entry.name.clone();
        let id = pending.game.clone();
        self.add_fomod(
            &id,
            &pending.game_dir,
            &pending.staging,
            &pending.root,
            &pending.config,
            &choices,
            &pending.present,
            pending.entry,
        )
        .await?;
        Ok(name)
    }

    /// Drop a FOMOD install the user cancelled.
    pub fn cancel_fomod(&self, token: u64) {
        PENDING.lock().remove(&token);
    }

    /// Thunderstore's communities (fetched at most daily).
    pub async fn thunderstore_ecosystem(&self) -> Result<Arc<Ecosystem>> {
        if let Some(eco) = ECOSYSTEM.read().clone() {
            return Ok(eco);
        }
        let eco = Arc::new(thunderstore::ecosystem(&ecosystem_cache()).await?);
        *ECOSYSTEM.write() = Some(eco.clone());
        Ok(eco)
    }

    /// The game's Thunderstore community: the user's choice, else the one
    /// recognised from its store ids, program or title.
    pub async fn mod_community(&self, id: &GameId) -> Result<Option<Community>> {
        let (game, game_dir) = self.mod_game(id)?;
        let eco = self.thunderstore_ecosystem().await?;
        let chosen = ModLibrary::for_game(id).load()?.thunderstore_community;
        if let Some(c) = chosen {
            return Ok(eco.community(&c).cloned());
        }
        let mut epic_ids = Vec::new();
        if id.runner == Runner::Legendary {
            epic_ids.push(game.app_name.as_str());
            epic_ids.extend(game.namespace.as_deref());
        }
        let keys = GameKeys {
            steam_id: (id.runner == Runner::Steam).then_some(game.app_name.as_str()),
            epic_ids,
            title: &game.title,
            game_dir: Some(&game_dir),
        };
        Ok(eco.find(&keys).cloned())
    }

    pub async fn set_mod_community(&self, id: &GameId, community: Option<String>) -> Result<()> {
        self.edit_mods(id, |list| {
            let changed = list.thunderstore_community != community;
            list.thunderstore_community = community;
            changed
        })
        .await
    }

    pub async fn search_thunderstore(
        &self,
        community: &str,
        query: &str,
        page: u32,
    ) -> Result<SearchPage> {
        thunderstore::search(thunderstore::BASE, community, query, page).await
    }

    /// Install a Thunderstore package and whatever it needs that isn't
    /// installed yet. Returns the names installed, dependencies first.
    pub async fn install_thunderstore(
        &self,
        id: &GameId,
        community: &Community,
        namespace: &str,
        name: &str,
        progress: ModProgress,
    ) -> Result<Vec<String>> {
        if !community.supported() {
            bail!(
                "Irori can't install mods for {} yet (it uses the {} loader)",
                community.name,
                community.loader
            );
        }
        let (_, game_dir) = self.mod_game_idle(id)?;
        let eco = self.thunderstore_ecosystem().await?;
        let lib = ModLibrary::for_game(id);
        let list = lib.load()?;
        let wanted = format!("{namespace}-{name}").to_lowercase();
        let installed: HashSet<String> = list
            .mods
            .iter()
            .filter_map(|m| match &m.source {
                ModSource::Thunderstore {
                    namespace, name, ..
                } => Some(format!("{namespace}-{name}").to_lowercase()),
                _ => None,
            })
            // The package asked for is installed again (an update).
            .filter(|key| *key != wanted)
            .collect();
        let packages =
            thunderstore::resolve(thunderstore::BASE, namespace, name, &installed).await?;
        let mut done = Vec::new();
        for package in packages {
            let full = package.full_name();
            let staging = lib.staging()?;
            let archive = staging.path().join("package.zip");
            progress.done.store(0, Ordering::Relaxed);
            progress.total.store(0, Ordering::Relaxed);
            irori_mods::download_to(
                &package.download_url,
                &archive,
                irori_mods::MAX_MOD_BYTES,
                Some(progress.clone()),
                &CancellationToken::new(),
            )
            .await
            .with_context(|| format!("downloading {full}"))?;
            let unpacked = staging.path().join("unpacked");
            irori_mods::archive::extract(&archive, &unpacked, &CancellationToken::new()).await?;
            let lock = lock_for(id);
            let _guard = lock.lock().await;
            let (lib2, community, eco, game_dir) = (
                lib.clone(),
                community.clone(),
                eco.clone(),
                game_dir.clone(),
            );
            let app = self.settings.app();
            let tree = staging.path().join("tree");
            let entry = new_entry(
                package.name.replace('_', " "),
                Some(package.version.clone()),
                ModSource::Thunderstore {
                    community: community.id.clone(),
                    namespace: package.namespace.clone(),
                    name: package.name.clone(),
                },
            );
            let entry = ModEntry {
                author: Some(package.namespace.clone()),
                summary: (!package.description.is_empty()).then(|| package.description.clone()),
                dependencies: package
                    .dependencies
                    .iter()
                    .filter_map(|d| thunderstore::split_dependency(d))
                    .map(|(ns, n)| format!("{ns}-{n}"))
                    .collect(),
                ..entry
            };
            self.spawn_blocking(move || -> Result<()> {
                let loader = eco.loader_package(&community, &full);
                let icon = unpacked.join("icon.png");
                layout::import(&unpacked, &tree, &game_dir, |rel| {
                    thunderstore::route(&community, loader, &full, rel)
                })?;
                let mut list = lib2.load()?;
                let replaces = list.find_source(&entry.source).map(|m| m.id.clone());
                let new = lib2.add(&mut list, &tree, entry, replaces.as_deref(), &app)?;
                if icon.is_file() {
                    let _ = std::fs::copy(&icon, lib2.mod_dir(&new)?.join("icon.png"));
                }
                Ok(())
            })
            .await??;
            done.push(package.name.replace('_', " "));
            self.mods_changed(id);
        }
        Ok(done)
    }

    /// The saved Nexus Mods account, when a key is saved.
    pub async fn nexus_user(&self) -> Option<NexusUser> {
        let path = nexus_key_file();
        self.spawn_blocking(move || irori_secrets::load::<SavedNexus>(&path).map(|s| s.user))
            .await
            .ok()
            .flatten()
    }

    async fn nexus_client(&self) -> Result<nexus::Client> {
        let path = nexus_key_file();
        let saved = self
            .spawn_blocking(move || irori_secrets::read_json::<SavedNexus>(&path))
            .await??
            .context("add your Nexus Mods API key first")?;
        Ok(nexus::Client::new(saved.key))
    }

    /// Check `key` with Nexus Mods and save it.
    pub async fn set_nexus_key(&self, key: String) -> Result<NexusUser> {
        let key = ApiKey(key.trim().to_string());
        if key.0.is_empty() {
            bail!("paste the API key from your Nexus Mods account page");
        }
        let user = nexus::Client::new(key.clone()).validate().await?;
        let saved = SavedNexus {
            key,
            user: user.clone(),
        };
        let path = nexus_key_file();
        self.spawn_blocking(move || irori_secrets::write_json(&path, &saved))
            .await??;
        Ok(user)
    }

    pub async fn forget_nexus_key(&self) -> Result<()> {
        let path = nexus_key_file();
        match tokio::fs::remove_file(&path).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    pub async fn nexus_mod(
        &self,
        domain: &str,
        mod_id: u64,
    ) -> Result<(nexus::ModInfo, Vec<nexus::ModFile>)> {
        let client = self.nexus_client().await?;
        let (info, files) = futures::try_join!(
            client.mod_info(domain, mod_id),
            client.files(domain, mod_id)
        )?;
        Ok((info, files))
    }

    /// Download and install a Nexus Mods file (from an `nxm://` link, or
    /// picked from a mod's files by a Premium member).
    pub async fn install_nexus(
        &self,
        id: &GameId,
        link: NxmLink,
        progress: ModProgress,
    ) -> Result<InstallOutcome> {
        self.mod_game_idle(id)?;
        let client = self.nexus_client().await?;
        let (info, files) = futures::try_join!(
            client.mod_info(&link.domain, link.mod_id),
            client.files(&link.domain, link.mod_id)
        )?;
        let file = files.iter().find(|f| f.file_id == link.file_id);
        let url = client.download_url(&link).await?;
        let lib = ModLibrary::for_game(id);
        let staging = lib.staging()?;
        let file_name = file
            .map(|f| irori_core::paths::sanitize(&f.file_name))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "download.zip".into());
        let archive = staging.path().join(&file_name);
        irori_mods::download_to(
            &url,
            &archive,
            irori_mods::MAX_MOD_BYTES,
            Some(progress),
            &CancellationToken::new(),
        )
        .await?;
        let mut entry = new_entry(
            if info.name.is_empty() {
                name_from_archive(&file_name)
            } else {
                info.name.clone()
            },
            file.map(|f| f.version.clone())
                .filter(|v| !v.is_empty())
                .or_else(|| (!info.version.is_empty()).then(|| info.version.clone())),
            ModSource::Nexus {
                domain: link.domain.clone(),
                mod_id: link.mod_id,
                file_id: link.file_id,
            },
        );
        entry.author = (!info.author.is_empty()).then(|| info.author.clone());
        entry.summary = (!info.summary.is_empty()).then(|| info.summary.clone());
        self.install_from_file(id, &archive, entry, Some(staging))
            .await
    }

    /// Make Irori the program that opens `nxm://` links (the site's "Mod
    /// manager download" button), for this user.
    pub async fn handle_nxm_links(&self) -> Result<()> {
        let exe = std::env::current_exe().context("Irori's program was not found")?;
        register_nxm(&exe).await
    }

    /// Before the game starts: apply its enabled mods. Returns whether
    /// anything was applied (then [`Backend::restore_mods`] must follow).
    pub(crate) async fn apply_mods(
        &self,
        id: &GameId,
        game: &GameInfo,
        log: &Path,
    ) -> Result<bool> {
        if !supports_mods(game) {
            return Ok(false);
        }
        let lib = ModLibrary::for_game(id);
        if !lib.root().exists() {
            return Ok(false);
        }
        let lock = lock_for(id);
        let _guard = lock.lock().await;
        // A deployment a crash left behind comes out first.
        self.take_out(&lib).await?;
        let list = lib.load()?;
        if !list.any_enabled() {
            return Ok(false);
        }
        let Some(game_dir) = game.install_path().filter(|d| d.is_dir()) else {
            return Ok(false);
        };
        let layers = lib.layers(&list)?;
        let count = list.mods.iter().filter(|m| m.enabled).count();
        let tool = overlay::tool().filter(|_| list.method == DeployMethod::Auto);
        if let Some(tool) = tool {
            let lower = &layers[..layers.len() - 1];
            let manifest = irori_mods::Deployment {
                game_dir: game_dir.clone(),
                applied: Applied::Overlay {
                    upper: lib.written_dir(),
                },
                files: Vec::new(),
                dirs: Vec::new(),
                created_at: irori_core::util::now_rfc3339(),
            };
            irori_core::json_store::write(&lib.manifest_file(), &manifest)?;
            match overlay::mount(&tool, &game_dir, lower, &lib.written_dir(), &lib.work_dir()).await
            {
                Ok(()) => {
                    crate::games::append_log(
                        log,
                        &format!("Mods: {count} applied with fuse-overlayfs\n"),
                    );
                    self.mods_changed(id);
                    return Ok(true);
                }
                Err(e) => {
                    tracing::warn!(
                        "mounting mods over {}: {e:#}; linking instead",
                        game_dir.display()
                    );
                    let _ = std::fs::remove_file(lib.manifest_file());
                }
            }
        }
        let manifest = lib.manifest_file();
        let deployed = {
            let (game_dir, manifest) = (game_dir.clone(), manifest.clone());
            self.spawn_blocking(move || deploy::deploy_links(&game_dir, &layers, &manifest))
                .await?
        };
        match deployed {
            Ok(d) => {
                crate::games::append_log(
                    log,
                    &format!("Mods: {count} applied, {} files linked\n", d.files.len()),
                );
                self.mods_changed(id);
                Ok(true)
            }
            Err(e) => {
                // Take back whatever got in before the failure.
                if let Err(undo) = self.take_out(&lib).await {
                    tracing::error!("undoing a failed mod deployment: {undo:#}");
                }
                Err(e.context("the mods could not be applied"))
            }
        }
    }

    /// After the game exits: take its mods back out of the game folder.
    pub(crate) async fn restore_mods(&self, id: &GameId, title: &str) {
        let lib = ModLibrary::for_game(id);
        let lock = lock_for(id);
        let _guard = lock.lock().await;
        if let Err(e) = self.take_out(&lib).await {
            tracing::error!("taking the mods of {id} out: {e:#}");
            self.bus.notice(
                Notice::new(
                    NoticeLevel::Warning,
                    format!("The mods of {title} are still in its folder"),
                )
                .message(format!("{e:#}"))
                .keys("shell.notice.mods_not_restored", None, &[("title", title)])
                .game(id.clone()),
            );
        }
        self.mods_changed(id);
    }

    /// Undo whatever deployment `lib` records.
    async fn take_out(&self, lib: &ModLibrary) -> Result<()> {
        let manifest = lib.manifest_file();
        let Some(deployment) = deploy::read(&manifest)? else {
            return Ok(());
        };
        match &deployment.applied {
            Applied::Overlay { upper } => {
                overlay::unmount(&deployment.game_dir).await?;
                let mod_files = enabled_files(lib)?;
                let (game_dir, upper) = (deployment.game_dir.clone(), upper.clone());
                let moved = self
                    .spawn_blocking(move || overlay::settle(&game_dir, &upper, &mod_files))
                    .await??;
                if moved > 0 {
                    tracing::info!("{moved} files the game wrote went to its folder");
                }
                std::fs::remove_file(&manifest)?;
            }
            Applied::Links => {
                let app = self.settings.app();
                let written = lib.written_dir();
                self.spawn_blocking(move || deploy::undeploy(&manifest, &written, &app))
                    .await??;
            }
        }
        Ok(())
    }

    /// At start: take out deployments a crash left in game folders.
    pub(crate) async fn recover_mods(&self) {
        let base = ModLibrary::base_dir();
        let manifests: Vec<PathBuf> = self
            .spawn_blocking(move || {
                let mut out = Vec::new();
                for runner in std::fs::read_dir(&base).into_iter().flatten().flatten() {
                    for game in std::fs::read_dir(runner.path())
                        .into_iter()
                        .flatten()
                        .flatten()
                    {
                        let manifest = game.path().join("deployment.json");
                        if manifest.is_file() {
                            out.push(game.path());
                        }
                    }
                }
                out
            })
            .await
            .unwrap_or_default();
        for root in manifests {
            let lib = ModLibrary::new(root);
            if let Err(e) = self.take_out(&lib).await {
                tracing::error!("taking leftover mods out ({}): {e:#}", lib.root().display());
            }
        }
    }
}

/// Whether Irori can make itself the `nxm://` handler here. macOS reads
/// link handlers from the app bundle only.
pub fn can_handle_nxm_links() -> bool {
    cfg!(any(target_os = "linux", windows))
}

#[cfg(target_os = "linux")]
async fn register_nxm(exe: &Path) -> Result<()> {
    use irori_core::process::{CommandSpec, RunOptions, run};
    let apps = dirs::data_dir()
        .context("the applications folder was not found")?
        .join("applications");
    tokio::fs::create_dir_all(&apps).await?;
    let quoted = irori_core::process::shell_quote(&exe.to_string_lossy());
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Irori (Nexus Mods links)\nExec={quoted} %u\n\
         NoDisplay=true\nTerminal=false\nMimeType=x-scheme-handler/nxm;\n"
    );
    irori_core::json_store::write_bytes(&apps.join("irori-nxm.desktop"), entry.as_bytes())?;
    let spec =
        CommandSpec::new("xdg-mime").args(["default", "irori-nxm.desktop", "x-scheme-handler/nxm"]);
    run(&spec, RunOptions::default())
        .await?
        .check()
        .context("the desktop did not accept Irori as the Nexus Mods link handler")?;
    Ok(())
}

#[cfg(windows)]
async fn register_nxm(exe: &Path) -> Result<()> {
    use irori_core::process::{CommandSpec, RunOptions, run};
    let key = r"HKCU\Software\Classes\nxm";
    let command = format!("\"{}\" \"%1\"", exe.display());
    let steps: [Vec<String>; 3] = [
        vec![
            "add".into(),
            key.into(),
            "/ve".into(),
            "/d".into(),
            "URL:Nexus Mods".into(),
            "/f".into(),
        ],
        vec![
            "add".into(),
            key.into(),
            "/v".into(),
            "URL Protocol".into(),
            "/d".into(),
            String::new(),
            "/f".into(),
        ],
        vec![
            "add".into(),
            format!(r"{key}\shell\open\command"),
            "/ve".into(),
            "/d".into(),
            command,
            "/f".into(),
        ],
    ];
    for args in steps {
        run(&CommandSpec::new("reg").args(args), RunOptions::default())
            .await?
            .check()
            .context("Windows did not accept Irori as the Nexus Mods link handler")?;
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", windows)))]
async fn register_nxm(_exe: &Path) -> Result<()> {
    bail!("on macOS, Nexus Mods links open only in apps that declare them")
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedNexus {
    key: ApiKey,
    user: NexusUser,
}

fn new_entry(name: String, version: Option<String>, source: ModSource) -> ModEntry {
    ModEntry {
        id: String::new(),
        name,
        version,
        author: None,
        summary: None,
        source,
        enabled: true,
        installed_at: String::new(),
        size: 0,
        files: 0,
        dependencies: Vec::new(),
    }
}

/// Every enabled mod's files, lower-cased.
fn enabled_files(lib: &ModLibrary) -> Result<HashSet<String>> {
    let list = lib.load()?;
    let mut out = HashSet::new();
    for m in list.mods.iter().filter(|m| m.enabled) {
        out.extend(lib.index(&m.id)?.into_iter().map(|f| f.to_lowercase()));
    }
    Ok(out)
}

/// Which files an installer's conditions name are present: in the game's
/// data folder (or root) or an enabled mod.
fn present_files(config: &ModuleConfig, game_dir: &Path, lib: &ModLibrary) -> Arc<HashSet<String>> {
    fn collect(c: &fomod::Condition, out: &mut Vec<String>) {
        match c {
            fomod::Condition::All(v) | fomod::Condition::Any(v) => {
                v.iter().for_each(|c| collect(c, out))
            }
            fomod::Condition::File { path, .. } => out.push(path.clone()),
            _ => {}
        }
    }
    let mut asked = Vec::new();
    for step in &config.steps {
        collect(&step.visible, &mut asked);
        for g in &step.groups {
            for p in &g.plugins {
                if let fomod::PluginKind::Depends { patterns, .. } = &p.kind {
                    patterns.iter().for_each(|(c, _)| collect(c, &mut asked));
                }
            }
        }
    }
    config
        .conditional
        .iter()
        .for_each(|(c, _)| collect(c, &mut asked));
    let prefix = layout::data_dir(game_dir).unwrap_or_default();
    let mod_files = enabled_files(lib).unwrap_or_default();
    let present = asked
        .into_iter()
        .filter(|path| {
            let rel = layout::prefixed(&prefix, path);
            game_dir
                .join(irori_mods::align_case(game_dir, &rel))
                .is_file()
                || mod_files.contains(&rel.to_lowercase())
        })
        .map(|p| p.to_lowercase())
        .collect();
    Arc::new(present)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_names_drop_nexus_suffixes() {
        assert_eq!(
            name_from_archive("SkyUI_5_2_SE-12604-5-2SE-1572000000.7z"),
            "SkyUI 5 2 SE"
        );
        assert_eq!(name_from_archive("Better-Bags-1.2.zip"), "Better-Bags-1.2");
        assert_eq!(name_from_archive(".zip"), "Mod");
    }
}
