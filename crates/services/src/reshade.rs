//! ReShade per game ([`irori_reshade`]) behind [`Backend`]: what is
//! installed, installing (refusing games ReShade's list bans), turning it on
//! and off, removing it, and the DLL overrides a Wine launch needs.
//!
//! One operation per game at a time, and none while the game runs: the DLL
//! can't be renamed or replaced under a running game. Removing first vets
//! the install folder with `fs_guard`, and deletes only files Irori wrote
//! that are unchanged.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use irori_core::install::ProgressSink;
use irori_core::process::CancellationToken;
use irori_core::{GameId, GameInfo, InstallPlatform, Runner, fs_guard};
use irori_extras::anticheat::{self, AntiCheatQuery};
use irori_reshade::{InstallRequest, Manifest, RemoveReport, Step, journal};
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

/// Games with a ReShade operation running.
static BUSY: Lazy<Mutex<HashSet<String>>> = Lazy::new(Default::default);

struct Busy(String);

impl Busy {
    fn take(id: &GameId) -> Result<Self> {
        let key = id.key();
        if !BUSY.lock().insert(key.clone()) {
            bail!("ReShade is already being changed for this game");
        }
        Ok(Self(key))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.lock().remove(&self.0);
    }
}

async fn load(id: &GameId) -> Result<Option<Manifest>> {
    let path = irori_reshade::journal_path(id);
    tokio::task::spawn_blocking(move || journal::load(&path)).await?
}

impl Backend {
    fn reshade_game(&self, id: &GameId) -> Result<Arc<GameInfo>> {
        let game = self.installed_game(id)?;
        if !supports_reshade(&game) {
            bail!("ReShade can only be installed into Windows games Irori installed");
        }
        Ok(game)
    }

    fn reshade_ready(&self, id: &GameId) -> Result<Busy> {
        if self.is_running(id) {
            bail!("Close the game first");
        }
        Busy::take(id)
    }

    /// The program the store or the user's settings start.
    fn configured_exe(&self, id: &GameId, game: &GameInfo) -> Option<PathBuf> {
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
        let manifest = load(id).await?;
        let latest = match irori_reshade::download::latest().await {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::info!("ReShade version check: {e:#}");
                None
            }
        };
        let anticheat = if id.runner == Runner::Sideload {
            Vec::new()
        } else {
            let query = AntiCheatQuery {
                title: &game.title,
                epic_namespace: game.namespace.as_deref().filter(|n| !n.is_empty()),
                steam_app_id: None,
            };
            anticheat::lookup(&query)
                .await
                .map(|i| i.anticheats)
                .unwrap_or_default()
        };
        Ok(ReshadeStatus {
            manifest,
            latest,
            anticheat,
        })
    }

    /// Install ReShade, or replace an installed one with the newest
    /// (keeping the files the user changed). `chosen_exe` overrides the
    /// program found in the game folder.
    pub async fn install_reshade(
        &self,
        id: &GameId,
        chosen_exe: Option<PathBuf>,
        step: Arc<dyn Fn(Step) + Send + Sync>,
        progress: ProgressSink,
        cancel: CancellationToken,
    ) -> Result<Manifest> {
        let game = self.reshade_game(id)?;
        let _busy = self.reshade_ready(id)?;
        let root = game.install_path().context("The game has no folder")?;
        let journal = irori_reshade::journal_path(id);
        let previous = load(id).await?;
        // An update or a reinstall keeps the program chosen before.
        let chosen = chosen_exe.or_else(|| previous.as_ref().map(|m| m.exe.clone()));
        if let Some(previous) = previous {
            self.remove_files(&journal, previous).await?;
        }
        let request = InstallRequest {
            game_root: root,
            configured_exe: self.configured_exe(id, &game),
            chosen_exe: chosen,
            wine: irori_core::HostPlatform::current().needs_wine(InstallPlatform::Windows),
            journal,
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

    async fn remove_files(&self, journal: &Path, manifest: Manifest) -> Result<RemoveReport> {
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
        self.reshade_game(id)?;
        let _busy = self.reshade_ready(id)?;
        let mut manifest = load(id).await?.context("ReShade is not installed")?;
        let journal = irori_reshade::journal_path(id);
        tokio::task::spawn_blocking(move || journal::set_enabled(&journal, &mut manifest, enabled))
            .await?
    }

    /// Remove ReShade: the files Irori wrote and nobody changed since.
    pub async fn remove_reshade(&self, id: &GameId) -> Result<RemoveReport> {
        let _busy = self.reshade_ready(id)?;
        let manifest = load(id).await?.context("ReShade is not installed")?;
        let report = self
            .remove_files(&irori_reshade::journal_path(id), manifest)
            .await?;
        tracing::info!(
            "removed ReShade from {id}: {} files, {} kept",
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
        let manifest = match load(id).await {
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
