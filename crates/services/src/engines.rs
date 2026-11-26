//! Emulators for the UI: the engines Irori knows, their installations,
//! downloadable versions, installs, updates and removals, RetroArch cores,
//! each system's default emulator and BIOS status, starting one without a
//! game, and the usual ROM folders found on this computer. Every change
//! emits [`Event::EnginesChanged`].

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use irori_core::{Event, OpContext};
use irori_engines::bios::{self, BiosIndex, SystemBios};
use irori_engines::custom::CustomEngine;
use irori_engines::releases::{self, Release};
use irori_engines::rom_sources::{self, Plan as RomPlan, RomSource};
use irori_engines::state::{CoreInstall, ManagedInstall};
use irori_engines::{EngineManifest, EngineRef, Installation, discovery, install, retroarch};

use crate::Backend;

/// How long an emulator opened on its own has to fail at start for the
/// failure to reach the user (a missing library, a bad program).
const OPEN_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// One engine with what this computer has of it.
#[derive(Debug, Clone)]
pub struct EngineStatus {
    pub manifest: Arc<EngineManifest>,
    /// Best first.
    pub installations: Vec<Installation>,
    /// The installation used unless a game says otherwise.
    pub default: Option<String>,
    /// Installed cores, by id (RetroArch).
    pub cores: Vec<(String, CoreInstall)>,
}

impl EngineStatus {
    pub fn is_installed(&self) -> bool {
        !self.installations.is_empty()
    }

    /// The installation games use by default.
    pub fn default_installation(&self) -> Option<&Installation> {
        self.default
            .as_deref()
            .and_then(|d| self.installations.iter().find(|i| i.id() == d))
            .or_else(|| self.installations.first())
    }

    /// The newest managed version installed.
    pub fn newest_managed(&self) -> Option<&str> {
        self.installations
            .iter()
            .filter_map(|i| i.version.as_deref())
            .max_by(|a, b| releases::compare_versions(a, b))
    }

    pub fn has_core(&self, core: &str) -> bool {
        self.cores.iter().any(|(id, _)| id == core)
    }

    /// Whether `choice` can play right now: the engine is installed and,
    /// for RetroArch, so is the core.
    pub fn plays(&self, choice: &EngineRef) -> bool {
        self.is_installed() && choice.core.as_deref().is_none_or(|c| self.has_core(c))
    }
}

impl Backend {
    /// Every engine with its installations (discovery reads the disk).
    pub async fn engine_statuses(&self) -> Vec<EngineStatus> {
        self.spawn_blocking(|| {
            let state = irori_engines::state::load();
            let cores: Vec<(String, CoreInstall)> = state
                .cores
                .iter()
                .filter(|(_, c)| c.path.exists())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            irori_engines::engines()
                .iter()
                .map(|m| EngineStatus {
                    installations: discovery::find(m, &state),
                    default: state.preferred.get(&m.id).cloned(),
                    cores: if m.kind == irori_engines::EngineKind::Retroarch {
                        cores.clone()
                    } else {
                        Vec::new()
                    },
                    manifest: m.clone(),
                })
                .collect()
        })
        .await
        .unwrap_or_default()
    }

    /// Downloadable versions of `engine`, newest first.
    pub async fn engine_releases(&self, engine: &str, refresh: bool) -> Result<Vec<Release>> {
        let check = if refresh {
            irori_core::github_releases::CheckKind::Manual
        } else {
            irori_core::github_releases::CheckKind::Automatic
        };
        self.engine_releases_with_kind(engine, refresh, check).await
    }

    /// Downloadable versions of `engine`, with an explicit [`CheckKind`] for
    /// GitHub's shared API rate-limit budget.
    pub async fn engine_releases_with_kind(
        &self,
        engine: &str,
        refresh: bool,
        check: irori_core::github_releases::CheckKind,
    ) -> Result<Vec<Release>> {
        let manifest = irori_engines::engine(engine).context("unknown emulator")?;
        releases::list_with_kind(&manifest, refresh, check).await
    }

    /// Install `release` (the newest when `None`). With `make_default`, games
    /// that don't pin a version switch to it (the Update button).
    pub async fn install_engine(
        &self,
        engine: &str,
        release: Option<Release>,
        make_default: bool,
        ctx: &OpContext,
    ) -> Result<Option<ManagedInstall>> {
        let manifest = irori_engines::engine(engine).context("unknown emulator")?;
        let release = match release {
            Some(r) => r,
            None => releases::list(&manifest, false)
                .await?
                .into_iter()
                .next()
                .with_context(|| format!("{} has no download for this computer", manifest.name))?,
        };
        let installed = install::install(&manifest, &release, ctx).await;
        if let Ok(Some(i)) = &installed
            && make_default
        {
            let (id, version) = (engine.to_string(), format!("managed:{}", i.version));
            self.spawn_blocking(move || install::set_preferred(&id, &version))
                .await
                .context("saving the default")??;
        }
        self.bus.emit(Event::EnginesChanged);
        installed
    }

    /// Delete a managed version of `engine`.
    pub async fn remove_engine_version(&self, engine: &str, version: &str) -> Result<()> {
        let result = install::remove(engine, version, &self.settings.app()).await;
        self.bus.emit(Event::EnginesChanged);
        result
    }

    /// Use `installation` (an [`Installation::id`]) for games of `engine`
    /// that don't pin one.
    pub async fn set_engine_default(&self, engine: &str, installation: &str) -> Result<()> {
        let (e, i) = (engine.to_string(), installation.to_string());
        self.spawn_blocking(move || install::set_preferred(&e, &i))
            .await
            .context("saving the default")??;
        self.bus.emit(Event::EnginesChanged);
        Ok(())
    }

    /// Use the program at `path` for `engine`, or stop using one.
    pub async fn set_engine_program(&self, engine: &str, path: Option<PathBuf>) -> Result<()> {
        if let Some(p) = &path {
            anyhow::ensure!(p.exists(), "{} does not exist", p.display());
        }
        let e = engine.to_string();
        self.spawn_blocking(move || install::set_custom(&e, path.as_deref()))
            .await
            .context("saving the program")??;
        self.bus.emit(Event::EnginesChanged);
        Ok(())
    }

    /// Save an emulator of the user's own (a new one when `id` is `None`)
    /// and use its program by default. Returns its id.
    pub async fn save_custom_engine(
        &self,
        id: Option<String>,
        def: CustomEngine,
    ) -> Result<String> {
        let id = self
            .spawn_blocking(move || {
                let id = irori_engines::custom::save(id.as_deref(), &def)?;
                install::set_preferred(&id, "custom")?;
                anyhow::Ok(id)
            })
            .await
            .context("saving the emulator stopped")??;
        self.bus.emit(Event::EnginesChanged);
        Ok(id)
    }

    /// Forget the emulator `id` the user added, and stop using it as a
    /// system's default. Games that chose it fall back to the default.
    pub async fn remove_custom_engine(&self, id: &str) -> Result<()> {
        let engine = id.to_string();
        self.spawn_blocking(move || irori_engines::custom::remove(&engine))
            .await
            .context("removing the emulator stopped")??;
        self.settings.update_app(|app| {
            app.system_engines
                .retain(|_, choice| EngineRef::parse(choice).is_none_or(|c| c.engine != id));
        });
        self.bus.emit(Event::EnginesChanged);
        Ok(())
    }

    /// Start `engine` without a game (its default installation, with the
    /// settings the global defaults give games). Its output goes to
    /// `engine-<id>.log`; one that ends with an error within a few seconds
    /// fails here, so the user hears why.
    pub async fn open_engine(&self, engine: &str) -> Result<()> {
        let manifest = irori_engines::engine(engine).context("unknown emulator")?;
        let app = self.settings.app();
        let defaults = &app.game;
        let values_set: serde_json::Map<String, serde_json::Value> = defaults
            .engine_options
            .get(&manifest.id)
            .cloned()
            .unwrap_or_default();
        let own_settings = defaults.engine_own_settings;
        let m = manifest.clone();
        let spec = self
            .spawn_blocking(move || -> Result<_> {
                let state = irori_engines::state::load();
                let installs = discovery::find(&m, &state);
                let installation = irori_engines::launch::resolve(
                    &m,
                    &installs,
                    None,
                    state.preferred.get(&m.id).map(String::as_str),
                )?;
                let values = m.option_values(&values_set.into_iter().collect());
                irori_engines::open::command(&m, installation, &values, own_settings)
            })
            .await
            .context("starting the emulator stopped")??;
        let log = irori_core::Paths::get()
            .logs_dir()
            .join(format!("engine-{}.log", manifest.id));
        let mut child = irori_core::process::spawn(&spec, Some(&log))
            .with_context(|| format!("starting {}", manifest.name))?;
        match tokio::time::timeout(OPEN_GRACE, child.wait()).await {
            Ok(Ok(status)) if !status.success() => anyhow::bail!(
                "{} stopped at once ({status}); its log is {}",
                manifest.name,
                log.display()
            ),
            Ok(_) => {}
            // Still running, as it should: reap it whenever it ends.
            Err(_) => {
                self.spawn(async move {
                    let _ = child.wait().await;
                });
            }
        }
        Ok(())
    }

    /// Download a RetroArch core.
    pub async fn install_core(&self, core: &str, ctx: &OpContext) -> Result<Option<CoreInstall>> {
        let result = retroarch::install_core(core, ctx).await;
        self.bus.emit(Event::EnginesChanged);
        result
    }

    pub async fn remove_core(&self, core: &str) -> Result<()> {
        let result = retroarch::remove_core(core).await;
        self.bus.emit(Event::EnginesChanged);
        result
    }

    /// The emulator a game of `system` plays with when it names none: the
    /// user's choice for the system, else the first recommended emulator
    /// that is installed, else the first recommended one.
    pub fn system_engine(&self, system: &str, statuses: &[EngineStatus]) -> Option<EngineRef> {
        let app = self.settings.app();
        irori_engines::default_choice(
            system,
            app.system_engines.get(system).map(String::as_str),
            |c| {
                statuses
                    .iter()
                    .any(|s| s.manifest.id == c.engine && s.plays(c))
            },
        )
    }

    /// Make `choice` the default emulator of `system` (`None`: Irori's
    /// recommendation).
    pub fn set_system_engine(&self, system: &str, choice: Option<&EngineRef>) {
        self.settings.update_app(|app| match choice {
            Some(c) => {
                app.system_engines.insert(system.to_string(), c.to_string());
            }
            None => {
                app.system_engines.remove(system);
            }
        });
        self.bus.emit(Event::EnginesChanged);
    }

    /// BIOS status of every system from the saved index.
    pub async fn bios_statuses(&self) -> Vec<SystemBios> {
        self.spawn_blocking(|| {
            let index = bios::cached_index();
            irori_engines::systems::all()
                .iter()
                .map(|s| bios::status(s, &index))
                .collect()
        })
        .await
        .unwrap_or_default()
    }

    /// Index the BIOS folder `root` again (only new or changed files are
    /// hashed).
    pub async fn rescan_bios(&self, root: PathBuf) -> Result<BiosIndex> {
        let index = self
            .spawn_blocking(move || {
                let previous = bios::cached_index();
                bios::scan(&root, &previous)
            })
            .await
            .context("scanning stopped")??;
        self.bus.emit(Event::EnginesChanged);
        Ok(index)
    }
}

impl Backend {
    /// Add a ROM folder and scan it.
    pub async fn add_rom_folder(&self, folder: PathBuf) -> Result<()> {
        anyhow::ensure!(folder.is_dir(), "{} is not a folder", folder.display());
        let text = folder.display().to_string();
        self.settings.update_app(|s| {
            if !s.rom_folders.contains(&text) {
                s.rom_folders.push(text);
            }
        });
        self.refresh_store(irori_core::Runner::Emulation, false)
            .await
    }

    /// Add several ROM folders and scan once. Folders that are gone are
    /// left out.
    pub async fn add_rom_folders(&self, folders: Vec<PathBuf>) -> Result<()> {
        let folders = self
            .spawn_blocking(move || {
                folders
                    .into_iter()
                    .filter(|f| f.is_dir())
                    .collect::<Vec<_>>()
            })
            .await
            .context("checking the folders stopped")?;
        anyhow::ensure!(
            !folders.is_empty(),
            "none of these folders is there anymore"
        );
        self.settings.update_app(|s| {
            for folder in &folders {
                let text = folder.display().to_string();
                if !s.rom_folders.contains(&text) {
                    s.rom_folders.push(text);
                }
            }
        });
        self.refresh_store(irori_core::Runner::Emulation, false)
            .await
    }

    /// The usual ROM folders on this computer (EmuDeck's, RetroDECK's, a
    /// `ROMs` folder, at home and at the top of other disks) and what
    /// adding them would add: only systems an emulator here plays or Irori
    /// can download, and nothing the library already scans. Only reads.
    pub async fn rom_sources(&self) -> (Vec<RomSource>, RomPlan) {
        let existing: Vec<PathBuf> = self
            .settings
            .app()
            .rom_folders
            .iter()
            .map(PathBuf::from)
            .collect();
        self.spawn_blocking(move || {
            let home = dirs::home_dir().unwrap_or_default();
            let sources = rom_sources::find(&home, &rom_sources::mount_roots());
            if sources.is_empty() {
                return (sources, RomPlan::default());
            }
            let state = irori_engines::state::load();
            let host = irori_engines::Host::current();
            let usable: Vec<Arc<EngineManifest>> = irori_engines::engines()
                .iter()
                .filter(|m| m.downloadable_on(host) || !discovery::find(m, &state).is_empty())
                .cloned()
                .collect();
            let playable = |system: &str| {
                let listed = irori_engines::systems::get(system)
                    .map(|s| s.engines.clone())
                    .unwrap_or_default();
                usable.iter().any(|m| {
                    m.systems.iter().any(|s| s == system)
                        || listed
                            .iter()
                            .filter_map(|e| EngineRef::parse(e))
                            .any(|e| e.engine == m.id)
                })
            };
            let plan = rom_sources::plan(&sources, &existing, playable);
            (sources, plan)
        })
        .await
        .unwrap_or_default()
    }

    /// Stop scanning a ROM folder; its games leave the library. Files stay.
    pub async fn remove_rom_folder(&self, folder: &str) -> Result<()> {
        self.settings
            .update_app(|s| s.rom_folders.retain(|f| f != folder));
        self.refresh_store(irori_core::Runner::Emulation, false)
            .await
    }

    /// Use `folder` for BIOS files and index it.
    pub async fn set_bios_folder(&self, folder: PathBuf) -> Result<BiosIndex> {
        anyhow::ensure!(folder.is_dir(), "{} is not a folder", folder.display());
        let text = folder.display().to_string();
        self.settings.update_app(|s| s.bios_folder = text);
        self.rescan_bios(folder).await
    }

    /// Emulated games in the library per system id.
    pub fn emulated_counts(&self) -> std::collections::BTreeMap<String, usize> {
        let mut out = std::collections::BTreeMap::new();
        for g in self.library().all_games() {
            if let Some(system) = &g.system {
                *out.entry(system.clone()).or_insert(0) += 1;
            }
        }
        out
    }
}
