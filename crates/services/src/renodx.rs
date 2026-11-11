//! RenoDX's HDR add-ons per game ([`irori_reshade::renodx`]) behind
//! [`Backend`]: the official mods offered and the ones for the game, and
//! installing one (or a file the user chose) beside ReShade's build with
//! add-on support, which is installed or switched to on the way. Turning
//! the add-on on and off and removing it are the shared graphics mod calls
//! with [`GraphicsMod::Renodx`].
//!
//! The same rules as ReShade's hold: games ReShade's compatibility list
//! bans are refused, one graphics mod operation per game at a time and
//! none while the game runs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use irori_core::GameId;
use irori_core::install::ProgressSink;
use irori_core::process::CancellationToken;
use irori_reshade::renodx::{
    self, AddonChoice, AddonRequest, Build, Catalog, Endpoints, RenodxMod,
};
use irori_reshade::{CompatList, GraphicsMod, Manifest, ReshadeError, Step};

use crate::Backend;
use crate::reshade::{anticheat_of, load};

/// What the RenoDX panel shows.
#[derive(Debug, Clone, Default)]
pub struct RenodxStatus {
    /// The add-on installed, when one is.
    pub manifest: Option<Manifest>,
    /// ReShade, when installed: the add-on needs its build with add-on
    /// support.
    pub reshade: Option<Manifest>,
    /// RenoDX's official mods, when its release could be read.
    pub catalog: Option<Arc<Catalog>>,
    /// Why the catalog couldn't be read.
    pub catalog_error: Option<String>,
    /// Ids of the catalog's mods for this game, best first.
    pub suggested: Vec<String>,
    /// Anticheat AreWeAntiCheatYet lists for the game (ask before
    /// installing).
    pub anticheat: Vec<String>,
}

impl RenodxStatus {
    /// The catalog's entry of the installed official mod.
    pub fn installed_mod(&self) -> Option<&RenodxMod> {
        let id = &self.manifest.as_ref()?.source.as_ref()?.id;
        self.catalog.as_ref()?.get(id)
    }

    /// A build of the installed official mod other than the installed
    /// file: RenoDX rebuilt it.
    pub fn update(&self) -> Option<&Build> {
        let manifest = self.manifest.as_ref()?;
        let installed = manifest.switch_record()?;
        let build = self.installed_mod()?.build_for(manifest.machine)?;
        (build.sha256 != installed.sha256 && manifest.complete).then_some(build)
    }
}

/// Which add-on to install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenodxChoice {
    /// A mod of RenoDX's catalog, by id.
    Mod(String),
    /// A `.addon64`/`.addon32` file the user downloaded.
    File(PathBuf),
}

/// The file name of a program path.
fn file_name(path: &Path) -> Option<String> {
    path.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Whether ReShade's list bans the program ReShade was installed for or the
/// launcher the game starts.
fn banned(compat: &CompatList, programs: &[Option<String>]) -> Option<String> {
    programs
        .iter()
        .flatten()
        .find(|name| compat.is_banned(name))
        .cloned()
}

impl Backend {
    /// The add-on installed, ReShade's state, RenoDX's mods and the ones
    /// for this game. A catalog that can't be read is reported, not an
    /// error: a file the user downloaded can still be installed.
    pub async fn renodx_status(&self, id: &GameId) -> Result<RenodxStatus> {
        let game = self.reshade_game(id)?;
        let manifest = load(GraphicsMod::Renodx, id).await?;
        let reshade = load(GraphicsMod::Reshade, id).await?;
        let (catalog, catalog_error) = match renodx::catalog(
            &Endpoints::official(),
            &GraphicsMod::Renodx.cache_dir(),
            false,
        )
        .await
        {
            Ok(c) => (Some(Arc::new(c)), None),
            Err(e) => {
                tracing::info!("RenoDX catalog: {e:#}");
                (None, Some(format!("{e:#}")))
            }
        };
        let exe = reshade
            .as_ref()
            .map(|m| m.exe.clone())
            .or_else(|| self.configured_exe(id, &game))
            .and_then(|p| file_name(&p));
        let suggested = catalog
            .as_ref()
            .map(|c| {
                c.suggest(&game.title, exe.as_deref())
                    .into_iter()
                    .map(|m| m.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let anticheat = anticheat_of(&game).await;
        Ok(RenodxStatus {
            manifest,
            reshade,
            catalog,
            catalog_error,
            suggested,
            anticheat,
        })
    }

    /// Install a RenoDX add-on, replacing one installed before (keeping it
    /// when the user changed it, which then refuses the new one). ReShade's
    /// build with add-on support is installed first when the game has no
    /// ReShade or the standard build.
    pub async fn install_renodx(
        &self,
        id: &GameId,
        choice: RenodxChoice,
        step: Arc<dyn Fn(Step) + Send + Sync>,
        progress: ProgressSink,
        cancel: CancellationToken,
    ) -> Result<Manifest> {
        let game = self.reshade_game(id)?;
        let _busy = self.graphics_mod_ready(id, GraphicsMod::Renodx)?;
        step(Step::Checking);
        let compat = irori_reshade::download::compatibility().await?;
        let root = game.install_path().context("The game has no folder")?;
        let configured = self.configured_exe(id, &game);
        let cache = GraphicsMod::Renodx.cache_dir();
        let chosen_mod = match &choice {
            RenodxChoice::Mod(mod_id) => {
                // Asked again now: the build checked against must be the
                // one the release holds today.
                let catalog = renodx::catalog(&Endpoints::official(), &cache, true).await?;
                Some(
                    catalog
                        .get(mod_id)
                        .cloned()
                        .with_context(|| format!("RenoDX no longer offers the mod {mod_id}"))?,
                )
            }
            RenodxChoice::File(_) => None,
        };

        let mut reshade = load(GraphicsMod::Reshade, id).await?;
        let machine = match &reshade {
            Some(m) => {
                let programs = [file_name(&m.exe), configured.as_deref().and_then(file_name)];
                if let Some(exe) = banned(&compat, &programs) {
                    bail!(ReshadeError::Banned { exe });
                }
                m.machine
            }
            None => {
                let (root, configured, compat) = (root.clone(), configured.clone(), compat.clone());
                let (found, _, _) = tokio::task::spawn_blocking(move || {
                    irori_reshade::install::check(&root, configured.as_deref(), None, &compat)
                })
                .await??;
                found.machine
            }
        };
        let build = match &chosen_mod {
            Some(m) => Some(
                m.build_for(machine)
                    .cloned()
                    .ok_or(ReshadeError::NoAddonBuild)?,
            ),
            None => None,
        };

        if !reshade.as_ref().is_some_and(|m| m.addon && m.complete) {
            let installed = self
                .install_reshade_build(
                    id,
                    &game,
                    None,
                    true,
                    step.clone(),
                    progress.clone(),
                    cancel.clone(),
                )
                .await?;
            tracing::info!(
                "installed ReShade {} with add-on support for {id}",
                installed.version
            );
            reshade = Some(installed);
        }
        let reshade = reshade.context("ReShade is not installed")?;
        if reshade.machine != machine {
            bail!(ReshadeError::NoAddonBuild);
        }

        let journal = GraphicsMod::Renodx.journal_path(id);
        if let Some(previous) = load(GraphicsMod::Renodx, id).await? {
            self.remove_files(&journal, previous).await?;
        }
        let choice = match (choice, build, chosen_mod) {
            (RenodxChoice::Mod(_), Some(build), Some(m)) => AddonChoice::Official {
                build,
                source: m.source(),
            },
            (RenodxChoice::File(path), _, _) => AddonChoice::File(path),
            _ => bail!(ReshadeError::NoAddonBuild),
        };
        let request = AddonRequest {
            reshade,
            choice,
            journal,
            cache,
        };
        let manifest = renodx::install(request, step, progress, cancel).await?;
        tracing::info!(
            "installed RenoDX {} for {id} in {}",
            manifest.dll,
            manifest.dir.display()
        );
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use irori_reshade::{Api, FileRecord, Machine, ModSource};

    use super::*;

    fn build(sha: &str) -> String {
        let release = serde_json::json!({"assets": [{
            "name": "renodx-game.addon64",
            "size": 3,
            "digest": format!("sha256:{sha}"),
            "updated_at": "2026-11-10T00:00:00Z",
            "browser_download_url": format!("{}renodx-game.addon64", renodx::DOWNLOADS_URL),
        }]});
        release.to_string()
    }

    fn catalog(sha: &str) -> Arc<Catalog> {
        let index = serde_json::json!({"games": [{"title": "Game", "mods": [{
            "id": "game", "title": "Game",
            "artifacts": [{"name": "renodx-game.addon64", "arch": "x64"}]
        }]}]});
        Arc::new(
            Catalog::parse(
                index.to_string().as_bytes(),
                build(sha).as_bytes(),
                renodx::DOWNLOADS_URL,
            )
            .unwrap(),
        )
    }

    fn installed(sha: &str, source: Option<&str>) -> Manifest {
        Manifest {
            kind: GraphicsMod::Renodx,
            version: "2026-11-01".into(),
            exe: PathBuf::from("/g/Game.exe"),
            machine: Machine::X64,
            api: Api::Dxgi,
            dll: "renodx-game.addon64".into(),
            dir: PathBuf::from("/g"),
            enabled: true,
            complete: true,
            overrides: Vec::new(),
            files: vec![FileRecord {
                path: PathBuf::from("renodx-game.addon64"),
                sha256: sha.into(),
                size: 3,
            }],
            created_dirs: Vec::new(),
            addon: false,
            source: source.map(|id| ModSource {
                id: id.into(),
                title: "Game".into(),
            }),
        }
    }

    #[test]
    fn tells_when_renodx_rebuilt_the_installed_mod() {
        let old = "a".repeat(64);
        let new = "b".repeat(64);
        let mut status = RenodxStatus {
            manifest: Some(installed(&old, Some("game"))),
            catalog: Some(catalog(&new)),
            ..Default::default()
        };
        assert_eq!(status.installed_mod().unwrap().id, "game");
        assert_eq!(status.update().unwrap().sha256, new);

        status.catalog = Some(catalog(&old));
        assert!(status.update().is_none());
        // A file the user chose is never updated.
        status.manifest = Some(installed(&old, None));
        status.catalog = Some(catalog(&new));
        assert!(status.update().is_none());
        status.catalog = None;
        assert!(status.update().is_none());
    }

    #[test]
    fn banned_programs_are_found_by_file_name() {
        let compat = CompatList::parse("[Launcher.exe]\nBanned=1\n[Game.exe]\nRenderApi=D3D11\n");
        assert_eq!(banned(&compat, &[Some("Game.exe".into()), None]), None);
        assert_eq!(
            banned(
                &compat,
                &[Some("Game.exe".into()), Some("launcher.EXE".into())]
            ),
            Some("launcher.EXE".into())
        );
    }

    /// Write a manifest and its files the way an install would, without
    /// downloading anything.
    #[cfg(unix)]
    fn put(journal: &Path, mut base: Manifest, files: &[(&str, &[u8])]) -> Manifest {
        use irori_reshade::journal::{self, Existing, Planned};
        base.files.clear();
        let planned = files
            .iter()
            .map(|(name, bytes)| Planned::new(*name, bytes.to_vec(), Existing::Refuse))
            .collect();
        journal::apply(journal, base, planned, &|| false).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn removing_reshade_removes_the_addon_it_loads() {
        use irori_core::{GameInfo, InstallPlatform, InstalledInfo, Runner};

        crate::tests::isolate_paths();
        let dir = tempfile::tempdir().unwrap();
        let game = Arc::new(
            GameInfo {
                runner: Some(Runner::Sideload),
                app_name: "graphics-mods".into(),
                title: "Graphics Mods".into(),
                is_installed: true,
                install: Some(InstalledInfo {
                    install_path: dir.path().display().to_string(),
                    platform: InstallPlatform::Windows,
                    ..Default::default()
                }),
                ..Default::default()
            }
            .finalize(),
        );
        let id = game.id();
        let store: irori_core::DynStore = Arc::new(crate::tests::PlayStore {
            game,
            command: irori_core::process::CommandSpec::new("true"),
            saves_fail: false,
        });
        let b = Backend::start_headless(move |_, _| vec![store]).unwrap();
        let rt = b.runtime();

        let mut reshade = installed("", None);
        reshade.kind = GraphicsMod::Reshade;
        reshade.dll = "dxgi.dll".into();
        reshade.dir = dir.path().to_path_buf();
        reshade.addon = true;
        put(
            &GraphicsMod::Reshade.journal_path(&id),
            reshade,
            &[("dxgi.dll", b"reshade")],
        );
        let mut addon = installed("", Some("game"));
        addon.dir = dir.path().to_path_buf();
        put(
            &GraphicsMod::Renodx.journal_path(&id),
            addon,
            &[("renodx-game.addon64", b"hdr")],
        );

        rt.block_on(b.set_graphics_mod_enabled(&id, GraphicsMod::Renodx, false))
            .unwrap();
        assert!(
            dir.path()
                .join("renodx-game.addon64.irori-disabled")
                .exists()
        );
        assert!(dir.path().join("dxgi.dll").exists());

        let report = rt.block_on(b.remove_reshade(&id)).unwrap();
        assert_eq!(report.removed, 2);
        assert!(report.kept.is_empty());
        let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
        assert!(left.is_empty(), "{left:?}");
        assert!(!GraphicsMod::Renodx.journal_path(&id).exists());
        assert!(!GraphicsMod::Reshade.journal_path(&id).exists());
    }
}
