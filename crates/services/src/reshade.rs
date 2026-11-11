//! Graphics mods per game ([`irori_reshade`]) behind [`Backend`]: what is
//! installed, installing ReShade (refusing games ReShade's list bans),
//! turning a mod on and off, removing it, and the DLL overrides a Wine
//! launch needs. RenoDX's add-ons ([`crate::renodx`]) go beside ReShade's
//! build with add-on support; removing ReShade removes them too.
//!
//! One operation per game at a time, whichever the mod, and none while the
//! game runs: files can't be renamed or replaced under a running game.
//! Removing first vets the install folder with `fs_guard`, and deletes only
//! files Irori wrote that are unchanged.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use irori_core::install::ProgressSink;
use irori_core::process::CancellationToken;
use irori_core::{GameId, GameInfo, InstallPlatform, Runner, fs_guard};
use irori_extras::anticheat::{self, AntiCheatQuery};
use irori_reshade::{GraphicsMod, InstallRequest, Manifest, RemoveReport, Step, journal};
use once_cell::sync::Lazy;
use parking_lot::Mutex;

use crate::Backend;

/// What the ReShade panel shows.
#[derive(Debug, Clone, Default)]
pub struct ReshadeStatus {
    /// What is installed, when anything is.
    pub manifest: Option<Manifest>,
    /// The newest ReShade, when reshade.me could be asked.
    pub latest: Option<String>,
    /// Anticheat AreWeAntiCheatYet lists for the game (ask before
    /// installing).
    pub anticheat: Vec<String>,
    /// The RenoDX mod installed beside ReShade, which goes with it.
    pub renodx: Option<Manifest>,
}

impl ReshadeStatus {
    /// A newer ReShade than the installed one is out.
    pub fn update_available(&self) -> bool {
        match (&self.manifest, &self.latest) {
            (Some(m), Some(latest)) => version_key(latest) > version_key(&m.version),
            _ => false,
        }
    }
}

fn version_key(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Whether Irori can put ReShade into the game: an installed Windows build
/// whose files Irori manages.
pub fn supports_reshade(game: &GameInfo) -> bool {
    game.runner().manages_installs()
        && game.is_installed
        && game.installed_platform() == Some(InstallPlatform::Windows)
        && game.install_path().is_some()
}

/// Games with a graphics mod operation running.
static BUSY: Lazy<Mutex<HashSet<String>>> = Lazy::new(Default::default);

pub(crate) struct Busy(String);

impl Busy {
    fn take(id: &GameId, kind: GraphicsMod) -> Result<Self> {
        let key = id.key();
        if !BUSY.lock().insert(key.clone()) {
            bail!("{} is already being changed for this game", kind.name());
        }
        Ok(Self(key))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.lock().remove(&self.0);
    }
}

/// Anticheat AreWeAntiCheatYet lists for the game; nothing for games the
/// user added, whose titles name no store's game.
pub(crate) async fn anticheat_of(game: &GameInfo) -> Vec<String> {
    if game.runner() == Runner::Sideload {
        return Vec::new();
    }
    let query = AntiCheatQuery {
        title: &game.title,
        epic_namespace: game.namespace.as_deref().filter(|n| !n.is_empty()),
        steam_app_id: None,
    };
    anticheat::lookup(&query)
        .await
        .map(|i| i.anticheats)
        .unwrap_or_default()
}

pub(crate) async fn load(kind: GraphicsMod, id: &GameId) -> Result<Option<Manifest>> {
    let path = kind.journal_path(id);
    tokio::task::spawn_blocking(move || journal::load(&path)).await?
}

impl Backend {
    pub(crate) fn reshade_game(&self, id: &GameId) -> Result<Arc<GameInfo>> {
        let game = self.installed_game(id)?;
        if !supports_reshade(&game) {
            bail!("ReShade can only be installed into Windows games Irori installed");
        }
        Ok(game)
    }

    pub(crate) fn graphics_mod_ready(&self, id: &GameId, kind: GraphicsMod) -> Result<Busy> {
        if self.is_running(id) {
            bail!("Close the game first");
        }
        Busy::take(id, kind)
    }

    /// The program the store or the user's settings start.
    pub(crate) fn configured_exe(&self, id: &GameId, game: &GameInfo) -> Option<PathBuf> {
        let root = game.install_path()?;
        let settings = self.settings.game(id);
        let exe = [settings.target_exe.as_str()]
            .into_iter()
            .chain(game.install.as_ref().map(|i| i.executable.as_str()))
            .map(str::trim)
            .find(|e| !e.is_empty())?;
        let exe = PathBuf::from(exe.replace('\\', "/"));
        Some(if exe.is_absolute() {
            exe
        } else {
            root.join(exe)
        })
    }

    /// What is installed, the newest version, and whether to warn about
    /// anticheat.
    pub async fn reshade_status(&self, id: &GameId) -> Result<ReshadeStatus> {
        let game = self.reshade_game(id)?;
        let manifest = load(GraphicsMod::Reshade, id).await?;
        let latest = match irori_reshade::download::latest().await {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::info!("ReShade version check: {e:#}");
                None
            }
        };
        let anticheat = anticheat_of(&game).await;
        let renodx = load(GraphicsMod::Renodx, id).await?;
        Ok(ReshadeStatus {
            manifest,
            latest,
            anticheat,
            renodx,
        })
    }

    /// Install ReShade, or replace an installed one with the newest
    /// (keeping the files the user changed). `chosen_exe` overrides the
    /// program found in the game folder. A ReShade with add-on support
    /// stays one.
    pub async fn install_reshade(
        &self,
        id: &GameId,
        chosen_exe: Option<PathBuf>,
        step: Arc<dyn Fn(Step) + Send + Sync>,
        progress: ProgressSink,
        cancel: CancellationToken,
    ) -> Result<Manifest> {
        let game = self.reshade_game(id)?;
        let _busy = self.graphics_mod_ready(id, GraphicsMod::Reshade)?;
        self.install_reshade_build(id, &game, chosen_exe, false, step, progress, cancel)
            .await
    }

    /// [`Self::install_reshade`] with the game's operation already taken;
    /// `addon` asks for the build with add-on support.
    #[allow(clippy::too_many_arguments)] // Two callers; a struct would only rename them.
    pub(crate) async fn install_reshade_build(
        &self,
        id: &GameId,
        game: &GameInfo,
        chosen_exe: Option<PathBuf>,
        addon: bool,
        step: Arc<dyn Fn(Step) + Send + Sync>,
        progress: ProgressSink,
        cancel: CancellationToken,
    ) -> Result<Manifest> {
        let root = game.install_path().context("The game has no folder")?;
        let journal = irori_reshade::journal_path(id);
        let previous = load(GraphicsMod::Reshade, id).await?;
        // An update or a reinstall keeps the program chosen before, and the
        // build with add-on support.
        let chosen = chosen_exe.or_else(|| previous.as_ref().map(|m| m.exe.clone()));
        let addon = addon || previous.as_ref().is_some_and(|m| m.addon);
        if let Some(previous) = previous {
            self.remove_files(&journal, previous).await?;
        }
        let request = InstallRequest {
            game_root: root,
            configured_exe: self.configured_exe(id, game),
            chosen_exe: chosen,
            wine: irori_core::HostPlatform::current().needs_wine(InstallPlatform::Windows),
            journal,
            addon,
        };
        let manifest = irori_reshade::install(request, step, progress, cancel).await?;
        tracing::info!(
            "installed ReShade {} for {id} as {} in {}",
            manifest.version,
            manifest.dll,
            manifest.dir.display()
        );
        Ok(manifest)
    }

    pub(crate) async fn remove_files(
        &self,
        journal: &Path,
        manifest: Manifest,
    ) -> Result<RemoveReport> {
        let app = self.settings.app();
        let journal = journal.to_path_buf();
        tokio::task::spawn_blocking(move || {
            journal::remove(&journal, &manifest, &|dir| {
                fs_guard::ensure_deletable(dir, &app)
            })
        })
        .await?
    }

    /// Turn ReShade on or off by renaming its DLL.
    pub async fn set_reshade_enabled(&self, id: &GameId, enabled: bool) -> Result<()> {
        self.set_graphics_mod_enabled(id, GraphicsMod::Reshade, enabled)
            .await
    }

    /// Turn a graphics mod on or off by renaming its switch file.
    pub async fn set_graphics_mod_enabled(
        &self,
        id: &GameId,
        kind: GraphicsMod,
        enabled: bool,
    ) -> Result<()> {
        self.reshade_game(id)?;
        let _busy = self.graphics_mod_ready(id, kind)?;
        let mut manifest = load(kind, id)
            .await?
            .with_context(|| format!("{} is not installed", kind.name()))?;
        let journal = kind.journal_path(id);
        tokio::task::spawn_blocking(move || journal::set_enabled(&journal, &mut manifest, enabled))
            .await?
    }

    /// Remove ReShade: the files Irori wrote and nobody changed since.
    pub async fn remove_reshade(&self, id: &GameId) -> Result<RemoveReport> {
        self.remove_graphics_mod(id, GraphicsMod::Reshade).await
    }

    /// Remove a graphics mod: the files Irori wrote and nobody changed
    /// since. Removing ReShade removes the add-ons it loads first.
    pub async fn remove_graphics_mod(
        &self,
        id: &GameId,
        kind: GraphicsMod,
    ) -> Result<RemoveReport> {
        let _busy = self.graphics_mod_ready(id, kind)?;
        let manifest = load(kind, id)
            .await?
            .with_context(|| format!("{} is not installed", kind.name()))?;
        let mut report = RemoveReport::default();
        // An add-on can't load without ReShade, so it goes first.
        if kind == GraphicsMod::Reshade
            && let Some(addon) = load(GraphicsMod::Renodx, id).await?
        {
            report = self
                .remove_files(&GraphicsMod::Renodx.journal_path(id), addon)
                .await?;
        }
        let own = self.remove_files(&kind.journal_path(id), manifest).await?;
        report.removed += own.removed;
        report.kept.extend(own.kept);
        tracing::info!(
            "removed {} from {id}: {} files, {} kept",
            kind.name(),
            report.removed,
            report.kept.len()
        );
        Ok(report)
    }

    /// Add the overrides an installed, enabled ReShade needs to a Wine
    /// launch's `WINEDLLOVERRIDES`. A missing or broken record adds nothing.
    pub(crate) async fn reshade_launch_env(
        &self,
        id: &GameId,
        env: &mut std::collections::BTreeMap<String, String>,
    ) {
        let manifest = match load(GraphicsMod::Reshade, id).await {
            Ok(Some(m)) => m,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!("ReShade record of {id}: {e:#}");
                return;
            }
        };
        let present = tokio::fs::try_exists(manifest.dll_path())
            .await
            .unwrap_or(false);
        add_overrides(&manifest, present, env);
    }
}

/// [`Backend::reshade_launch_env`] once the record is read: an enabled,
/// complete install whose DLL is `present` gets `<dll>=n,b` for each DLL it
/// records, merged into the user's own overrides.
fn add_overrides(
    manifest: &Manifest,
    present: bool,
    env: &mut std::collections::BTreeMap<String, String>,
) {
    if !manifest.enabled || !manifest.complete || manifest.overrides.is_empty() || !present {
        return;
    }
    let add: Vec<(&str, &str)> = manifest
        .overrides
        .iter()
        .map(|dll| (dll.as_str(), "n,b"))
        .collect();
    let value = irori_runtime::env::add_dll_overrides(
        env.get("WINEDLLOVERRIDES").map(String::as_str),
        &add,
    );
    env.insert("WINEDLLOVERRIDES".into(), value);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            kind: GraphicsMod::Reshade,
            version: "6.7.3".into(),
            exe: PathBuf::from("/g/Game.exe"),
            machine: irori_reshade::Machine::X64,
            api: irori_reshade::Api::Dxgi,
            dll: "dxgi.dll".into(),
            dir: PathBuf::from("/g"),
            enabled: true,
            complete: true,
            overrides: vec!["dxgi".into(), "d3dcompiler_47".into()],
            files: Vec::new(),
            created_dirs: Vec::new(),
            addon: false,
            source: None,
        }
    }

    #[test]
    fn launches_get_the_overrides_of_an_enabled_install() {
        let mut env = BTreeMap::from([(
            "WINEDLLOVERRIDES".to_string(),
            "winemenubuilder.exe=d;dxgi=b".to_string(),
        )]);
        let mut off = manifest();
        off.enabled = false;
        add_overrides(&off, true, &mut env);
        add_overrides(&manifest(), false, &mut env);
        assert_eq!(env["WINEDLLOVERRIDES"], "winemenubuilder.exe=d;dxgi=b");
        add_overrides(&manifest(), true, &mut env);
        assert_eq!(
            env["WINEDLLOVERRIDES"],
            "winemenubuilder.exe=d;dxgi=n,b;d3dcompiler_47=n,b"
        );
        let mut empty = BTreeMap::new();
        add_overrides(&manifest(), true, &mut empty);
        assert_eq!(empty["WINEDLLOVERRIDES"], "dxgi=n,b;d3dcompiler_47=n,b");
    }

    #[test]
    fn tells_when_an_update_is_out() {
        let mut status = ReshadeStatus {
            manifest: Some(manifest()),
            latest: Some("6.10.0".into()),
            ..Default::default()
        };
        assert!(status.update_available());
        status.latest = Some("6.7.3".into());
        assert!(!status.update_available());
        status.latest = None;
        assert!(!status.update_available());
    }
}
