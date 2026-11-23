//! OptiScaler per game ([`irori_reshade::optiscaler`]) behind [`Backend`]:
//! status checks (using the 15-minute TTL/ETag cache on demand with zero
//! background polling), installing or updating OptiScaler with optional
//! FakeNVAPI and OptiPatcher, toggling and removing through the shared
//! graphics mod journal, and assembling Wine/Proton launch environment
//! variables.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use irori_core::GameId;
use irori_core::install::ProgressSink;
use irori_core::process::CancellationToken;
use irori_reshade::optiscaler::{
    self, GpuDriverMode, OptiscalerChannel, OptiscalerConfig, OptiscalerEndpoints,
    OptiscalerInstallRequest, OptiscalerRelease,
};
use irori_reshade::{Detection, GraphicsMod, Manifest, RemoveReport, ReshadeError, Step, journal};

use crate::Backend;
use crate::reshade::{anticheat_of, load};

/// What the OptiScaler panel shows for one game.
#[derive(Debug, Clone, Default)]
pub struct OptiscalerStatus {
    /// The installed OptiScaler manifest, when installed.
    pub manifest: Option<Manifest>,
    /// The release channel checked for [`Self::latest`].
    pub channel: OptiscalerChannel,
    /// Latest release in [`Self::channel`], when GitHub or cache could be read.
    pub latest: Option<OptiscalerRelease>,
    /// Why the release check failed, if it did.
    pub catalog_error: Option<String>,
    /// Auto-detected 64-bit game executable and target folder, when found.
    pub detected: Option<Detection>,
    /// Anticheat systems AreWeAntiCheatYet lists for the game (ask before
    /// installing).
    pub anticheat: Vec<String>,
}

impl OptiscalerStatus {
    /// Whether a newer release than the installed one is available in
    /// [`Self::channel`], or the selected channel differs from the installed
    /// channel.
    pub fn update_available(&self) -> bool {
        let (Some(manifest), Some(latest)) = (&self.manifest, &self.latest) else {
            return false;
        };
        if !manifest.complete {
            return false;
        }
        if manifest
            .optiscaler
            .as_ref()
            .is_some_and(|cfg| cfg.channel != self.channel)
        {
            return true;
        }
        optiscaler::is_newer_release(self.channel, &manifest.version, &latest.version)
    }

    /// Persisted OptiScaler options of the installed manifest, if present.
    pub fn installed_config(&self) -> Option<&OptiscalerConfig> {
        self.manifest.as_ref()?.optiscaler.as_ref()
    }

    /// Game executable OptiScaler is installed beside, or the auto-detected one.
    pub fn target_exe(&self) -> Option<&Path> {
        self.manifest
            .as_ref()
            .map(|m| m.exe.as_path())
            .or_else(|| self.detected.as_ref().map(|d| d.exe.as_path()))
    }

    /// Game directory OptiScaler is installed in, or the auto-detected one.
    pub fn target_dir(&self) -> Option<&Path> {
        self.manifest
            .as_ref()
            .map(|m| m.dir.as_path())
            .or_else(|| self.detected.as_ref().map(|d| d.dir.as_path()))
    }
}

/// Parameters for installing or updating OptiScaler for one game.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptiscalerInstallParams {
    /// OptiScaler options (channel, GPU driver mode, proxy DLL, OptiPatcher).
    pub config: OptiscalerConfig,
    /// Explicit executable chosen by the user, overriding auto-detection.
    pub chosen_exe: Option<PathBuf>,
}

impl Backend {
    /// Read the OptiScaler install state, detected target executable,
    /// anticheat list, and latest release for `channel`.
    ///
    /// Performs no background polling: release metadata is checked only when
    /// asked, backed by the 15-minute TTL and ETag cache in `irori-reshade`.
    pub async fn optiscaler_status(
        &self,
        id: &GameId,
        channel: OptiscalerChannel,
    ) -> Result<OptiscalerStatus> {
        let endpoints = (!self.isolated).then(OptiscalerEndpoints::official);
        self.optiscaler_status_with(id, channel, endpoints.as_ref())
            .await
    }

    async fn optiscaler_status_with(
        &self,
        id: &GameId,
        channel: OptiscalerChannel,
        endpoints: Option<&OptiscalerEndpoints>,
    ) -> Result<OptiscalerStatus> {
        let game = self.graphics_mod_game(id, GraphicsMod::Optiscaler)?;
        let manifest = load(GraphicsMod::Optiscaler, id).await?;
        let cache = GraphicsMod::Optiscaler.cache_dir();

        let release_fut = async {
            let Some(endpoints) = endpoints else {
                return (None, None);
            };
            match optiscaler::fetch_channel_release(channel, endpoints, &cache, false).await {
                Ok(rel) => (Some(rel), None),
                Err(e) => {
                    tracing::info!("OptiScaler {} version check: {e:#}", channel.id());
                    (None, Some(format!("{e:#}")))
                }
            }
        };

        let already_complete = manifest.as_ref().is_some_and(|m| m.complete);
        let detect_fut = async {
            if already_complete {
                return None;
            }
            let root = game.install_path()?;
            let configured = self.configured_exe(id, &game);
            tokio::task::spawn_blocking(move || {
                optiscaler::detect_target(&root, configured.as_deref(), None).ok()
            })
            .await
            .unwrap_or(None)
        };

        let ((latest, catalog_error), detected, anticheat) =
            tokio::join!(release_fut, detect_fut, anticheat_of(&game));

        Ok(OptiscalerStatus {
            manifest,
            channel,
            latest,
            catalog_error,
            detected,
            anticheat,
        })
    }

    /// Read only the local OptiScaler manifest without querying GitHub or
    /// rescanning the game folder, reusing previously loaded release/anticheat
    /// metadata when the channel has not changed.
    pub async fn optiscaler_local_status(
        &self,
        id: &GameId,
        channel: OptiscalerChannel,
        previous: Option<OptiscalerStatus>,
    ) -> Result<OptiscalerStatus> {
        let game = self.graphics_mod_game(id, GraphicsMod::Optiscaler)?;
        let manifest = load(GraphicsMod::Optiscaler, id).await?;
        let already_complete = manifest.as_ref().is_some_and(|m| m.complete);

        let same_channel = previous.as_ref().is_some_and(|p| p.channel == channel);
        let latest = same_channel
            .then(|| previous.as_ref().and_then(|p| p.latest.clone()))
            .flatten();
        let catalog_error = same_channel
            .then(|| previous.as_ref().and_then(|p| p.catalog_error.clone()))
            .flatten();
        let anticheat = match &previous {
            Some(prev) => prev.anticheat.clone(),
            None => anticheat_of(&game).await,
        };
        let detected = if already_complete {
            None
        } else if let Some(prev_det) = previous.and_then(|p| p.detected) {
            Some(prev_det)
        } else if let Some(root) = game.install_path() {
            let configured = self.configured_exe(id, &game);
            tokio::task::spawn_blocking(move || {
                optiscaler::detect_target(&root, configured.as_deref(), None).ok()
            })
            .await
            .unwrap_or(None)
        } else {
            None
        };

        Ok(OptiscalerStatus {
            manifest,
            channel,
            latest,
            catalog_error,
            detected,
            anticheat,
        })
    }

    /// Reconfigure options of an existing installation in-place (`Apply`)
    /// without downloading or updating the OptiScaler binary version.
    /// Falls back to [`Self::install_optiscaler`] when changing channels or
    /// moving to a different executable folder.
    pub async fn configure_optiscaler(
        &self,
        id: &GameId,
        params: OptiscalerInstallParams,
        step: Arc<dyn Fn(Step) + Send + Sync>,
        progress: ProgressSink,
        cancel: CancellationToken,
    ) -> Result<Manifest> {
        self.configure_optiscaler_with(
            id,
            params,
            OptiscalerEndpoints::official(),
            step,
            progress,
            cancel,
        )
        .await
    }

    async fn configure_optiscaler_with(
        &self,
        id: &GameId,
        params: OptiscalerInstallParams,
        endpoints: OptiscalerEndpoints,
        step: Arc<dyn Fn(Step) + Send + Sync>,
        progress: ProgressSink,
        cancel: CancellationToken,
    ) -> Result<Manifest> {
        let _game = self.graphics_mod_game(id, GraphicsMod::Optiscaler)?;
        let busy = self.graphics_mod_ready(id, GraphicsMod::Optiscaler)?;
        let journal = GraphicsMod::Optiscaler.journal_path(id);
        let cache = GraphicsMod::Optiscaler.cache_dir();
        let previous = load(GraphicsMod::Optiscaler, id).await?;

        let same_channel_and_dir = previous.as_ref().is_some_and(|prev| {
            prev.complete
                && prev
                    .optiscaler
                    .as_ref()
                    .is_some_and(|c| c.channel == params.config.channel)
                && params
                    .chosen_exe
                    .as_deref()
                    .and_then(Path::parent)
                    .is_none_or(|dir| dir == prev.dir)
        });

        if same_channel_and_dir {
            let prev = previous.expect("checked is_some_and");
            let prev_overrides = prev.overrides.clone();
            let manifest = optiscaler::reconfigure(
                journal,
                prev,
                params.config,
                cache,
                endpoints,
                step,
                progress,
                cancel,
            )
            .await?;
            if manifest.enabled {
                self.sync_steam_user_reg(id, &manifest.overrides, &prev_overrides)
                    .await;
            }
            return Ok(manifest);
        }

        drop(busy);
        self.install_optiscaler_with(id, params, endpoints, step, progress, cancel)
            .await
    }

    /// Install OptiScaler, or replace/update an existing install (keeping any
    /// files the user modified).
    pub async fn install_optiscaler(
        &self,
        id: &GameId,
        params: OptiscalerInstallParams,
        step: Arc<dyn Fn(Step) + Send + Sync>,
        progress: ProgressSink,
        cancel: CancellationToken,
    ) -> Result<Manifest> {
        self.install_optiscaler_with(
            id,
            params,
            OptiscalerEndpoints::official(),
            step,
            progress,
            cancel,
        )
        .await
    }

    async fn install_optiscaler_with(
        &self,
        id: &GameId,
        params: OptiscalerInstallParams,
        endpoints: OptiscalerEndpoints,
        step: Arc<dyn Fn(Step) + Send + Sync>,
        progress: ProgressSink,
        cancel: CancellationToken,
    ) -> Result<Manifest> {
        let game = self.graphics_mod_game(id, GraphicsMod::Optiscaler)?;
        let _busy = self.graphics_mod_ready(id, GraphicsMod::Optiscaler)?;
        let root = game.install_path().context("The game has no folder")?;
        let configured = self.configured_exe(id, &game);
        let journal = GraphicsMod::Optiscaler.journal_path(id);
        let cache = GraphicsMod::Optiscaler.cache_dir();
        let previous = load(GraphicsMod::Optiscaler, id).await?;

        // An update or reinstall keeps the previously chosen executable unless
        // the caller explicitly picked a new one.
        let chosen = params
            .chosen_exe
            .or_else(|| previous.as_ref().map(|m| m.exe.clone()));

        // Validate target detection and check for proxy collisions against
        // non-owned files before removing a working previous installation.
        step(Step::Checking);
        let found = {
            let root = root.clone();
            let configured = configured.clone();
            let chosen = chosen.clone();
            tokio::task::spawn_blocking(move || {
                optiscaler::detect_target(&root, configured.as_deref(), chosen.as_deref())
            })
            .await??
        };
        let proxy_name = params.config.proxy_dll.file_name();
        let replaces_own_proxy = previous
            .as_ref()
            .is_some_and(|prev| prev.dir == found.dir && prev.dll.eq_ignore_ascii_case(proxy_name));
        if !replaces_own_proxy {
            let proxy_path = found.dir.join(proxy_name);
            let proxy_disabled = found
                .dir
                .join(format!("{proxy_name}{}", journal::DISABLED_SUFFIX));
            if tokio::fs::symlink_metadata(&proxy_path).await.is_ok()
                || tokio::fs::symlink_metadata(&proxy_disabled).await.is_ok()
            {
                bail!(ReshadeError::DllTaken {
                    name: proxy_name.to_string(),
                });
            }
        }

        let prev_overrides = previous
            .as_ref()
            .map(|p| p.overrides.clone())
            .unwrap_or_default();
        let mut keep_previous_for_update = None;
        if let Some(prev) = previous {
            if prev.complete && prev.dir == found.dir {
                keep_previous_for_update = Some(prev);
            } else {
                self.remove_files(&journal, prev).await?;
            }
        }

        let request = OptiscalerInstallRequest {
            game_root: root,
            configured_exe: configured,
            chosen_exe: chosen,
            config: params.config,
            journal,
            cache,
            endpoints,
            previous: keep_previous_for_update,
        };
        let result = optiscaler::install(request, step, progress, cancel).await?;
        let manifest = result.manifest;
        self.sync_steam_user_reg(id, &manifest.overrides, &prev_overrides)
            .await;
        tracing::info!(
            "installed OptiScaler {} for {id} as {} in {}",
            manifest.version,
            manifest.dll,
            manifest.dir.display()
        );
        Ok(manifest)
    }

    /// Turn OptiScaler on or off by renaming its switch file.
    pub async fn set_optiscaler_enabled(&self, id: &GameId, enabled: bool) -> Result<()> {
        self.set_graphics_mod_enabled(id, GraphicsMod::Optiscaler, enabled)
            .await
    }

    /// Remove OptiScaler: the files Irori wrote and nobody changed since.
    pub async fn remove_optiscaler(&self, id: &GameId) -> Result<RemoveReport> {
        self.remove_graphics_mod(id, GraphicsMod::Optiscaler).await
    }

    /// Add the overrides and NVAPI environment variables an installed, enabled
    /// OptiScaler needs to a Wine/Proton launch. A missing or broken record
    /// adds nothing.
    pub(crate) async fn optiscaler_launch_env(
        &self,
        id: &GameId,
        env: &mut BTreeMap<String, String>,
    ) {
        let manifest = match load(GraphicsMod::Optiscaler, id).await {
            Ok(Some(m)) => m,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!("OptiScaler record of {id}: {e:#}");
                return;
            }
        };
        let present = tokio::fs::try_exists(manifest.dll_path())
            .await
            .unwrap_or(false);
        apply_launch_env(&manifest, present, cfg!(target_os = "linux"), env);
    }
}

/// Returns true for `.asi` plugins (such as `OptiScaler.asi`) which are loaded
/// by an ASI loader rather than Wine's DLL override loader.
fn is_asi_override(name: &str) -> bool {
    name.eq_ignore_ascii_case("OptiScaler.asi")
        || name
            .rsplit_once('.')
            .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("asi"))
}

fn is_nvidia_mode(manifest: &Manifest) -> bool {
    manifest
        .optiscaler
        .as_ref()
        .is_some_and(|cfg| cfg.gpu_driver == GpuDriverMode::Nvidia)
}

/// Apply OptiScaler launch environment variables once the manifest and switch
/// file presence are known:
/// - Merges `<dll>=n,b` overrides via [`irori_runtime::env::add_dll_overrides`],
///   skipping any `.asi` entry (such as `OptiScaler.asi`).
/// - On Linux/Proton in [`GpuDriverMode::Nvidia`], sets `PROTON_ENABLE_NVAPI=1`
///   unless `PROTON_ENABLE_NVAPI` is already set in `env`.
fn apply_launch_env(
    manifest: &Manifest,
    present: bool,
    is_linux: bool,
    env: &mut BTreeMap<String, String>,
) {
    if !manifest.enabled || !manifest.complete || !present {
        return;
    }

    let add: Vec<(&str, &str)> = manifest
        .overrides
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty() && !is_asi_override(s))
        .map(|dll| (dll, "n,b"))
        .collect();

    if !add.is_empty() {
        let key = env
            .keys()
            .find(|k| k.eq_ignore_ascii_case("WINEDLLOVERRIDES"))
            .cloned()
            .unwrap_or_else(|| "WINEDLLOVERRIDES".to_string());
        let value = irori_runtime::env::add_dll_overrides(env.get(&key).map(String::as_str), &add);
        env.insert(key, value);
    }

    if is_linux && is_nvidia_mode(manifest) && !env.contains_key("PROTON_ENABLE_NVAPI") {
        env.insert("PROTON_ENABLE_NVAPI".into(), "1".into());
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use irori_reshade::{Api, Machine, ProxyDll};

    use super::*;

    fn optiscaler_manifest(config: OptiscalerConfig, overrides: &[&str]) -> Manifest {
        Manifest {
            kind: GraphicsMod::Optiscaler,
            version: "v0.7.7".into(),
            exe: PathBuf::from("/g/Game.exe"),
            machine: Machine::X64,
            api: Api::Dxgi,
            dll: config.proxy_dll.file_name().into(),
            dir: PathBuf::from("/g"),
            enabled: true,
            complete: true,
            overrides: overrides.iter().map(|s| (*s).to_string()).collect(),
            files: Vec::new(),
            created_dirs: Vec::new(),
            addon: false,
            source: None,
            optiscaler: Some(config),
        }
    }

    fn release(version: &str) -> OptiscalerRelease {
        OptiscalerRelease {
            version: version.into(),
            asset_name: format!("OptiScaler_{version}.7z"),
            download_url: format!("https://github.com/optiscaler/OptiScaler/releases/{version}.7z"),
            sha256: "a".repeat(64),
            size: 1024,
            published_at: "2026-11-20".into(),
        }
    }

    #[test]
    fn winedlloverrides_merges_proxy_and_fakenvapi_and_skips_asi() {
        let mesa_dxgi = optiscaler_manifest(
            OptiscalerConfig {
                channel: OptiscalerChannel::Stable,
                gpu_driver: GpuDriverMode::MesaAuto,
                proxy_dll: ProxyDll::Dxgi,
                enable_optipatcher: false,
                shortcut_key: None,
            },
            &["dxgi", "nvapi64"],
        );

        let mut env = BTreeMap::from([(
            "WINEDLLOVERRIDES".to_string(),
            "winemenubuilder.exe=d;dxgi=b".to_string(),
        )]);

        // Disabled, incomplete, or missing switch file must not touch env.
        let mut disabled = mesa_dxgi.clone();
        disabled.enabled = false;
        apply_launch_env(&disabled, true, true, &mut env);

        let mut incomplete = mesa_dxgi.clone();
        incomplete.complete = false;
        apply_launch_env(&incomplete, true, true, &mut env);

        apply_launch_env(&mesa_dxgi, false, true, &mut env);
        assert_eq!(env["WINEDLLOVERRIDES"], "winemenubuilder.exe=d;dxgi=b");
        assert!(!env.contains_key("PROTON_ENABLE_NVAPI"));

        // Enabled + present merges proxy DLL and nvapi64, replacing prior dxgi=b.
        apply_launch_env(&mesa_dxgi, true, true, &mut env);
        assert_eq!(
            env["WINEDLLOVERRIDES"],
            "winemenubuilder.exe=d;dxgi=n,b;nvapi64=n,b"
        );
        assert!(!env.contains_key("PROTON_ENABLE_NVAPI"));

        // Fresh env gets both overrides cleanly.
        let mut empty = BTreeMap::new();
        apply_launch_env(&mesa_dxgi, true, true, &mut empty);
        assert_eq!(empty["WINEDLLOVERRIDES"], "dxgi=n,b;nvapi64=n,b");

        // OptiScaler.asi is never emitted into WINEDLLOVERRIDES even if present in overrides.
        let asi_mesa = optiscaler_manifest(
            OptiscalerConfig {
                channel: OptiscalerChannel::Stable,
                gpu_driver: GpuDriverMode::MesaAuto,
                proxy_dll: ProxyDll::OptiScalerAsi,
                enable_optipatcher: false,
                shortcut_key: None,
            },
            &["OptiScaler.asi", "nvapi64"],
        );
        let mut asi_env = BTreeMap::new();
        apply_launch_env(&asi_mesa, true, true, &mut asi_env);
        assert_eq!(asi_env["WINEDLLOVERRIDES"], "nvapi64=n,b");

        // OptiScaler.asi without FakeNVAPI emits no WINEDLLOVERRIDES key at all.
        let asi_nvidia = optiscaler_manifest(
            OptiscalerConfig {
                channel: OptiscalerChannel::Stable,
                gpu_driver: GpuDriverMode::Nvidia,
                proxy_dll: ProxyDll::OptiScalerAsi,
                enable_optipatcher: true,
                shortcut_key: None,
            },
            &["OptiScaler.asi"],
        );
        let mut asi_nv_env = BTreeMap::new();
        apply_launch_env(&asi_nvidia, true, true, &mut asi_nv_env);
        assert!(!asi_nv_env.contains_key("WINEDLLOVERRIDES"));
        assert_eq!(asi_nv_env["PROTON_ENABLE_NVAPI"], "1");
    }

    #[test]
    fn nvidia_mode_sets_proton_enable_nvapi_without_clobbering_user_override() {
        let nvidia_dxgi = optiscaler_manifest(
            OptiscalerConfig {
                channel: OptiscalerChannel::Stable,
                gpu_driver: GpuDriverMode::Nvidia,
                proxy_dll: ProxyDll::Version,
                enable_optipatcher: false,
                shortcut_key: None,
            },
            &["version"],
        );

        // Linux: sets PROTON_ENABLE_NVAPI=1 when absent.
        let mut linux_env = BTreeMap::new();
        apply_launch_env(&nvidia_dxgi, true, true, &mut linux_env);
        assert_eq!(linux_env["WINEDLLOVERRIDES"], "version=n,b");
        assert_eq!(linux_env["PROTON_ENABLE_NVAPI"], "1");

        // Linux: respects explicit user override PROTON_ENABLE_NVAPI=0.
        let mut user_off = BTreeMap::from([("PROTON_ENABLE_NVAPI".to_string(), "0".to_string())]);
        apply_launch_env(&nvidia_dxgi, true, true, &mut user_off);
        assert_eq!(user_off["PROTON_ENABLE_NVAPI"], "0");
        assert_eq!(user_off["WINEDLLOVERRIDES"], "version=n,b");

        // Non-Linux (macOS/Windows): never sets PROTON_ENABLE_NVAPI.
        let mut mac_env = BTreeMap::new();
        apply_launch_env(&nvidia_dxgi, true, false, &mut mac_env);
        assert_eq!(mac_env["WINEDLLOVERRIDES"], "version=n,b");
        assert!(!mac_env.contains_key("PROTON_ENABLE_NVAPI"));

        // Disabled Nvidia manifest does not set PROTON_ENABLE_NVAPI.
        let mut off = nvidia_dxgi.clone();
        off.enabled = false;
        let mut off_env = BTreeMap::new();
        apply_launch_env(&off, true, true, &mut off_env);
        assert!(!off_env.contains_key("PROTON_ENABLE_NVAPI"));
    }

    #[test]
    fn status_detects_available_updates_and_channel_switches() {
        let stable_cfg = OptiscalerConfig {
            channel: OptiscalerChannel::Stable,
            gpu_driver: GpuDriverMode::MesaAuto,
            proxy_dll: ProxyDll::Dxgi,
            enable_optipatcher: false,
            shortcut_key: None,
        };
        let manifest = optiscaler_manifest(stable_cfg, &["dxgi", "nvapi64"]);

        let mut status = OptiscalerStatus {
            manifest: Some(manifest.clone()),
            channel: OptiscalerChannel::Stable,
            latest: Some(release("v0.7.9")),
            ..Default::default()
        };
        assert!(status.update_available());
        assert_eq!(status.target_exe(), Some(Path::new("/g/Game.exe")));
        assert_eq!(status.target_dir(), Some(Path::new("/g")));

        status.latest = Some(release("v0.7.7"));
        assert!(!status.update_available());

        // Switching selected channel from Stable to Nightly offers update/switch.
        status.channel = OptiscalerChannel::Nightly;
        status.latest = Some(release("nightly-2026-11-20"));
        assert!(status.update_available());

        // Incomplete install never advertises an update.
        let mut incomplete = manifest;
        incomplete.complete = false;
        status.manifest = Some(incomplete);
        assert!(!status.update_available());

        status.manifest = None;
        assert!(!status.update_available());
    }

    #[cfg(unix)]
    #[test]
    fn enable_disable_and_remove_lifecycle_for_optiscaler() {
        use irori_core::{GameInfo, InstallPlatform, InstalledInfo, Runner};
        use irori_reshade::journal::{Existing, Planned};

        crate::tests::isolate_paths();
        let dir = tempfile::tempdir().unwrap();
        let game = Arc::new(
            GameInfo {
                runner: Some(Runner::Sideload),
                app_name: "optiscaler-lifecycle".into(),
                title: "OptiScaler Lifecycle".into(),
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

        let cfg = OptiscalerConfig {
            channel: OptiscalerChannel::Stable,
            gpu_driver: GpuDriverMode::MesaAuto,
            proxy_dll: ProxyDll::Version,
            enable_optipatcher: true,
            shortcut_key: None,
        };
        let mut base = optiscaler_manifest(cfg, &["version", "nvapi64"]);
        base.dir = dir.path().to_path_buf();
        base.exe = dir.path().join("Game.exe");
        let journal_path = GraphicsMod::Optiscaler.journal_path(&id);
        let planned = vec![
            Planned::new("version.dll", b"optiscaler-bin".to_vec(), Existing::Refuse),
            Planned::new(
                "OptiScaler.ini",
                b"[Plugins]\nLoadAsiPlugins=true\n".to_vec(),
                Existing::Keep,
            ),
            Planned::new("nvapi64.dll", b"fakenvapi-bin".to_vec(), Existing::Keep),
            Planned::new("fakenvapi.ini", b"[fakenvapi]\n".to_vec(), Existing::Keep),
            Planned::new(
                optiscaler::OPTIPATCHER_REL_PATH,
                b"optipatcher-bin".to_vec(),
                Existing::Keep,
            ),
        ];
        journal::apply(&journal_path, base, planned, &|| false).unwrap();

        // While enabled, optiscaler_launch_env merges DLL overrides.
        let mut env = BTreeMap::new();
        rt.block_on(b.optiscaler_launch_env(&id, &mut env));
        assert_eq!(env["WINEDLLOVERRIDES"], "version=n,b;nvapi64=n,b");

        // Status reads the installed manifest without touching the network or rescanning the folder.
        let status = rt
            .block_on(b.optiscaler_status(&id, OptiscalerChannel::Stable))
            .unwrap();
        assert_eq!(
            status.manifest.as_ref().map(|m| m.dll.as_str()),
            Some("version.dll")
        );
        assert!(status.detected.is_none());
        assert!(status.manifest.as_ref().unwrap().enabled);

        // configure_optiscaler renames the proxy in-place with zero network calls and preserves version.
        let (step, _rx): (Arc<dyn Fn(Step) + Send + Sync>, ProgressSink) =
            (Arc::new(|_| {}), Arc::new(|_| {}));
        let reconf = rt
            .block_on(b.configure_optiscaler(
                &id,
                OptiscalerInstallParams {
                    config: OptiscalerConfig {
                        channel: OptiscalerChannel::Stable,
                        gpu_driver: GpuDriverMode::MesaAuto,
                        proxy_dll: ProxyDll::Winmm,
                        enable_optipatcher: true,
                        shortcut_key: None,
                    },
                    chosen_exe: None,
                },
                step,
                _rx,
                CancellationToken::new(),
            ))
            .unwrap();
        assert_eq!(reconf.version, "v0.7.7");
        assert_eq!(reconf.dll, "winmm.dll");
        assert!(!dir.path().join("version.dll").exists());
        assert!(dir.path().join("winmm.dll").exists());

        // Restore version.dll so the rest of the lifecycle test checks the original assertions.
        rt.block_on(b.configure_optiscaler(
            &id,
            OptiscalerInstallParams {
                config: OptiscalerConfig {
                    channel: OptiscalerChannel::Stable,
                    gpu_driver: GpuDriverMode::MesaAuto,
                    proxy_dll: ProxyDll::Version,
                    enable_optipatcher: true,
                    shortcut_key: None,
                },
                chosen_exe: None,
            },
            Arc::new(|_| {}),
            Arc::new(|_| {}),
            CancellationToken::new(),
        ))
        .unwrap();

        // Disabling renames the switch file and suppresses launch env overrides.
        rt.block_on(b.set_graphics_mod_enabled(&id, GraphicsMod::Optiscaler, false))
            .unwrap();
        assert!(!dir.path().join("version.dll").exists());
        assert!(dir.path().join("version.dll.irori-disabled").exists());
        let mut disabled_env = BTreeMap::new();
        rt.block_on(b.optiscaler_launch_env(&id, &mut disabled_env));
        assert!(disabled_env.is_empty());

        // Re-enabling restores the switch file.
        rt.block_on(b.set_optiscaler_enabled(&id, true)).unwrap();
        assert!(dir.path().join("version.dll").exists());
        assert!(!dir.path().join("version.dll.irori-disabled").exists());

        // User modifies OptiScaler.ini; removal must keep it and delete the rest.
        std::fs::write(
            dir.path().join("OptiScaler.ini"),
            b"[Plugins]\nLoadAsiPlugins=true\n; user tweak\n",
        )
        .unwrap();

        let report = rt
            .block_on(b.remove_graphics_mod(&id, GraphicsMod::Optiscaler))
            .unwrap();
        assert_eq!(report.removed, 4);
        assert_eq!(report.kept, vec![PathBuf::from("OptiScaler.ini")]);
        assert!(!dir.path().join("version.dll").exists());
        assert!(!dir.path().join("nvapi64.dll").exists());
        assert!(!dir.path().join("fakenvapi.ini").exists());
        assert!(!dir.path().join(optiscaler::OPTIPATCHER_REL_PATH).exists());
        assert!(!dir.path().join("OptiScaler").exists());
        assert!(dir.path().join("OptiScaler.ini").exists());
        assert!(!journal_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn reshade_and_optiscaler_coexist_in_launch_env_and_removal() {
        use irori_core::{GameInfo, InstallPlatform, InstalledInfo, Runner};
        use irori_reshade::journal::{Existing, Planned};

        crate::tests::isolate_paths();
        let dir = tempfile::tempdir().unwrap();
        let game = Arc::new(
            GameInfo {
                runner: Some(Runner::Sideload),
                app_name: "optiscaler-coexist".into(),
                title: "OptiScaler Coexist".into(),
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

        // Install ReShade on dxgi.dll and OptiScaler on version.dll + Nvidia mode.
        let reshade_base = Manifest {
            kind: GraphicsMod::Reshade,
            version: "6.7.3".into(),
            exe: dir.path().join("Game.exe"),
            machine: Machine::X64,
            api: Api::Dxgi,
            dll: "dxgi.dll".into(),
            dir: dir.path().to_path_buf(),
            enabled: true,
            complete: true,
            overrides: vec!["dxgi".into(), "d3dcompiler_47".into()],
            files: Vec::new(),
            created_dirs: Vec::new(),
            addon: false,
            source: None,
            optiscaler: None,
        };
        journal::apply(
            &GraphicsMod::Reshade.journal_path(&id),
            reshade_base,
            vec![Planned::new(
                "dxgi.dll",
                b"reshade".to_vec(),
                Existing::Refuse,
            )],
            &|| false,
        )
        .unwrap();

        let cfg = OptiscalerConfig {
            channel: OptiscalerChannel::Stable,
            gpu_driver: GpuDriverMode::Nvidia,
            proxy_dll: ProxyDll::Version,
            enable_optipatcher: false,
            shortcut_key: None,
        };
        let mut opti_base = optiscaler_manifest(cfg, &["version"]);
        opti_base.dir = dir.path().to_path_buf();
        opti_base.exe = dir.path().join("Game.exe");
        journal::apply(
            &GraphicsMod::Optiscaler.journal_path(&id),
            opti_base,
            vec![Planned::new(
                "version.dll",
                b"optiscaler".to_vec(),
                Existing::Refuse,
            )],
            &|| false,
        )
        .unwrap();

        let mut env = BTreeMap::new();
        rt.block_on(b.reshade_launch_env(&id, &mut env));
        rt.block_on(b.optiscaler_launch_env(&id, &mut env));
        assert_eq!(
            env["WINEDLLOVERRIDES"],
            "dxgi=n,b;d3dcompiler_47=n,b;version=n,b"
        );
        if cfg!(target_os = "linux") {
            assert_eq!(env["PROTON_ENABLE_NVAPI"], "1");
        }

        // Removing ReShade must leave OptiScaler intact.
        rt.block_on(b.remove_reshade(&id)).unwrap();
        assert!(!dir.path().join("dxgi.dll").exists());
        assert!(dir.path().join("version.dll").exists());
        assert!(GraphicsMod::Optiscaler.journal_path(&id).exists());

        // Removing OptiScaler cleans up cleanly.
        let report = rt.block_on(b.remove_optiscaler(&id)).unwrap();
        assert_eq!(report.removed, 1);
        assert!(!dir.path().join("version.dll").exists());
        assert!(!GraphicsMod::Optiscaler.journal_path(&id).exists());
    }

    #[test]
    fn steam_windows_games_support_graphics_mods() {
        use irori_core::{GameInfo, InstallPlatform, InstalledInfo, Runner};
        let win = GameInfo {
            runner: Some(Runner::Steam),
            app_name: "400".into(),
            title: "Portal".into(),
            is_installed: true,
            install: Some(InstalledInfo {
                install_path: "/games/Portal".into(),
                platform: InstallPlatform::Windows,
                ..Default::default()
            }),
            ..Default::default()
        }
        .finalize();
        // Steam on macOS runs only Mac builds.
        let mac = irori_core::HostPlatform::current() == irori_core::HostPlatform::Mac;
        assert_eq!(crate::supports_reshade(&win), !mac);

        let native = GameInfo {
            runner: Some(Runner::Steam),
            app_name: "400".into(),
            title: "Portal".into(),
            is_installed: true,
            install: Some(InstalledInfo {
                install_path: "/games/Portal".into(),
                platform: InstallPlatform::Linux,
                ..Default::default()
            }),
            ..Default::default()
        }
        .finalize();
        assert!(!crate::supports_reshade(&native));
    }
}
