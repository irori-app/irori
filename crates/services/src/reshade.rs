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

/// Whether Irori can put ReShade, RenoDX or OptiScaler into the game: an
/// installed Windows build whose files Irori manages, or a Steam game's
/// Windows build, natively on Windows and through Proton on Linux (Steam's
/// entry says so by its platform, or by not being Linux-native). Steam on
/// macOS runs only Mac builds. Steam starts its games itself, so under
/// Proton the DLL overrides go into Steam's launch options rather than
/// Irori's launch.
pub fn supports_reshade(game: &GameInfo) -> bool {
    let windows_build = match game.runner() {
        Runner::Steam => match irori_core::HostPlatform::current() {
            irori_core::HostPlatform::Windows => {
                game.installed_platform() != Some(InstallPlatform::Linux)
            }
            irori_core::HostPlatform::Linux => {
                game.installed_platform() == Some(InstallPlatform::Windows) || !game.is_linux_native
            }
            irori_core::HostPlatform::Mac => false,
        },
        runner => {
            runner.manages_installs() && game.installed_platform() == Some(InstallPlatform::Windows)
        }
    };
    windows_build && game.is_installed && game.install_path().is_some()
}

/// Whether Steam starts the game through Proton, so ReShade's DLL overrides
/// belong in Steam's launch options.
pub fn reshade_needs_steam_overrides(game: &GameInfo) -> bool {
    game.runner() == Runner::Steam
        && irori_core::HostPlatform::current() == irori_core::HostPlatform::Linux
        && supports_reshade(game)
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
        steam_app_id: (game.runner() == Runner::Steam).then_some(game.app_name.as_str()),
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
    pub(crate) fn graphics_mod_game(
        &self,
        id: &GameId,
        kind: GraphicsMod,
    ) -> Result<Arc<GameInfo>> {
        let game = self.installed_game(id)?;
        if !supports_reshade(&game) {
            bail!(
                "{} can only be installed into the Windows builds of games Irori installs or Steam runs",
                kind.name()
            );
        }
        Ok(game)
    }

    /// For Steam Windows games on Linux, surgically synchronize DLL overrides in
    /// the game's Proton prefix (`compatdata/<appid>/pfx/user.reg`) so alternative
    /// proxy DLLs (such as `version.dll` or `winmm.dll`) load automatically
    /// without editing Steam Launch Options.
    pub(crate) async fn sync_steam_user_reg(&self, id: &GameId, add: &[String], remove: &[String]) {
        if !cfg!(target_os = "linux") || id.runner != Runner::Steam {
            return;
        }
        let Some(install_path) = self.game(id).and_then(|g| g.install_path()) else {
            return;
        };
        let Some(pfx) = irori_runtime::protontricks::prefix_of(&install_path, &id.app_name) else {
            return;
        };
        let user_reg = pfx.join("user.reg");
        let add_owned: Vec<String> = add.to_vec();
        let remove_owned: Vec<String> = remove.to_vec();
        let res = tokio::task::spawn_blocking(move || {
            let add_refs: Vec<&str> = add_owned.iter().map(String::as_str).collect();
            let rem_refs: Vec<&str> = remove_owned.iter().map(String::as_str).collect();
            irori_reshade::optiscaler::sync_wine_user_reg_overrides(&user_reg, &add_refs, &rem_refs)
        })
        .await;
        if let Ok(Err(e)) = res {
            tracing::warn!("syncing Proton user.reg overrides for {id}: {e:#}");
        }
    }

    pub(crate) fn reshade_game(&self, id: &GameId) -> Result<Arc<GameInfo>> {
        self.graphics_mod_game(id, GraphicsMod::Reshade)
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
    /// With `adopt`, a ReShade installed by hand
    /// (`ReshadeError::ManualInstall`) is taken over.
    pub async fn install_reshade(
        &self,
        id: &GameId,
        chosen_exe: Option<PathBuf>,
        adopt: bool,
        step: Arc<dyn Fn(Step) + Send + Sync>,
        progress: ProgressSink,
        cancel: CancellationToken,
    ) -> Result<Manifest> {
        let game = self.reshade_game(id)?;
        let _busy = self.graphics_mod_ready(id, GraphicsMod::Reshade)?;
        self.install_reshade_build(id, &game, chosen_exe, false, adopt, step, progress, cancel)
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
        adopt: bool,
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
            // Steam's Windows builds run through Proton everywhere but Windows.
            wine: irori_core::HostPlatform::current().needs_wine(InstallPlatform::Windows),
            journal,
            addon,
            adopt,
        };
        let manifest = irori_reshade::install(request, step, progress, cancel).await?;
        self.sync_steam_user_reg(id, &manifest.overrides, &[]).await;
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
        self.graphics_mod_game(id, kind)?;
        let _busy = self.graphics_mod_ready(id, kind)?;
        let mut manifest = load(kind, id)
            .await?
            .with_context(|| format!("{} is not installed", kind.name()))?;
        let overrides = manifest.overrides.clone();
        let journal = kind.journal_path(id);
        tokio::task::spawn_blocking(move || journal::set_enabled(&journal, &mut manifest, enabled))
            .await??;
        if enabled {
            self.sync_steam_user_reg(id, &overrides, &[]).await;
        } else {
            self.sync_steam_user_reg(id, &[], &overrides).await;
        }
        Ok(())
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
        let removed_overrides = manifest.overrides.clone();
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
        self.sync_steam_user_reg(id, &[], &removed_overrides).await;
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

    /// Put back the files of the game's graphics mods that its folder lost
    /// since they were installed: a store's update or file check deletes
    /// what it doesn't know, which would quietly leave the game without
    /// ReShade. Only ReShade's own downloads (its DLL, the compiler, the
    /// shaders) come back, from the cache or their official source, and
    /// only when they match what was installed. Never fails the launch.
    pub(crate) async fn restore_graphics_mods(&self, id: &GameId) {
        let manifest = match load(GraphicsMod::Reshade, id).await {
            Ok(Some(m)) => m,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!("ReShade record of {id}: {e:#}");
                return;
            }
        };
        let missing = {
            let m = manifest.clone();
            tokio::task::spawn_blocking(move || journal::missing(&m))
                .await
                .unwrap_or_default()
        };
        if missing.is_empty() {
            return;
        }
        let mut bytes: Vec<(PathBuf, Vec<u8>)> = Vec::new();
        let progress = irori_core::install::ignore_progress();
        let cancel = CancellationToken::new();
        for file in &missing {
            let name = file.path.to_string_lossy();
            let got = if file.path == Path::new(&manifest.dll) {
                irori_reshade::download::reshade_dll(
                    &manifest.version,
                    manifest.machine,
                    manifest.addon,
                    &progress,
                    &cancel,
                )
                .await
                .map(Some)
            } else if name.eq_ignore_ascii_case("d3dcompiler_47.dll") {
                irori_reshade::download::d3dcompiler(manifest.machine, &progress, &cancel)
                    .await
                    .map(Some)
            } else {
                Ok(None)
            };
            match got {
                Ok(Some(b)) => bytes.push((file.path.clone(), b)),
                Ok(None) => {}
                Err(e) => tracing::info!("putting ReShade back into {id}: {e:#}"),
            }
        }
        if missing
            .iter()
            .any(|f| f.path.starts_with("reshade-shaders"))
        {
            match irori_reshade::download::shaders(&progress, &cancel).await {
                Ok(files) => bytes.extend(files),
                Err(e) => tracing::info!("putting ReShade's shaders back into {id}: {e:#}"),
            }
        }
        let restored = tokio::task::spawn_blocking(move || journal::restore(&manifest, &bytes))
            .await
            .map_err(anyhow::Error::from)
            .and_then(|r| r);
        match restored {
            Ok(0) => {}
            Ok(n) => tracing::info!("put {n} of ReShade's files back into {id}"),
            Err(e) => tracing::warn!("putting ReShade back into {id}: {e:#}"),
        }
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
            optiscaler: None,
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
    fn steam_games_take_reshade_when_they_are_windows_builds() {
        let game = |linux_native: bool| GameInfo {
            runner: Some(Runner::Steam),
            app_name: "620".into(),
            is_installed: true,
            is_linux_native: linux_native,
            install: Some(irori_core::InstalledInfo {
                install_path: "/games/Portal 2".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let expected = |linux_native: bool| match irori_core::HostPlatform::current() {
            irori_core::HostPlatform::Windows => true,
            irori_core::HostPlatform::Linux => !linux_native,
            irori_core::HostPlatform::Mac => false,
        };
        for native in [true, false] {
            assert_eq!(supports_reshade(&game(native)), expected(native));
        }
        let mut gone = game(false);
        gone.is_installed = false;
        assert!(!supports_reshade(&gone));
        assert_eq!(
            reshade_needs_steam_overrides(&game(false)),
            cfg!(target_os = "linux")
        );
        assert!(!reshade_needs_steam_overrides(&game(true)));
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
