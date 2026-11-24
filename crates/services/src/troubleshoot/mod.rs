//! Launch troubleshooting: gathers what the diagnosis reads about a game's
//! last run (its log, its program's imports, the winetricks components in
//! its prefix, its anticheat entry, its graphics and sync settings,
//! `vm.max_map_count`, Yama's `ptrace_scope` and the Rockstar Games
//! Launcher's log for Rockstar titles from Epic, the distribution and GPU),
//! remembers the fixes tried
//! this session, and applies the fixes that need the backend: winetricks
//! components, and stopping a wineserver of another Wine version.

pub mod analysis;
pub mod pe;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use irori_core::{GameId, GameInfo, HostPlatform, InstallPlatform, Paths, Runner, WineKind};
use irori_extras::anticheat::{self, AntiCheatQuery};
use parking_lot::Mutex;

pub use analysis::{
    Detail, Diagnosis, Facts, Finding, Fix, FixContext, GpuVendor, LaunchHint,
    MAP_COUNT_RECOMMENDED, MAP_COUNT_WANTED, OfferedFix, diagnose, distro_ids, game_output_start,
    launch_hints, map_count_commands, missing_dlls, ptrace_scope_commands, runtime_verbs,
    vulkan_driver_command,
};
pub use pe::PeInfo;

use crate::Backend;

/// The end of a game log the diagnosis reads.
const LOG_TAIL_BYTES: u64 = 1024 * 1024;

/// What troubleshooting remembers for the session.
#[derive(Default)]
pub(crate) struct TroubleshootState {
    /// [`Fix::key`]s tried per game.
    tried: Mutex<HashMap<GameId, HashSet<String>>>,
    /// Exit code of each game's last session, when it failed.
    last_exit: Mutex<HashMap<GameId, Option<i32>>>,
}

impl TroubleshootState {
    pub(crate) fn record_exit(&self, id: &GameId, exit_code: Option<i32>) {
        self.last_exit.lock().insert(id.clone(), exit_code);
    }
}

impl Backend {
    /// Work out why `id`'s last run failed. `exit_code` is the code it
    /// exited with; `None` uses the code of its last session this run.
    pub async fn troubleshoot_launch(
        &self,
        id: &GameId,
        exit_code: Option<i32>,
    ) -> Result<Diagnosis> {
        let game = self.game(id).context("unknown game")?;
        let facts = self.launch_facts(id, &game, exit_code).await;
        let tried = self
            .troubleshoot
            .tried
            .lock()
            .get(id)
            .cloned()
            .unwrap_or_default();
        Ok(diagnose(&facts, &tried))
    }

    /// Remember that `fix` was tried for `id`, so it isn't offered first
    /// again this session.
    pub fn mark_fix_tried(&self, id: &GameId, fix: &Fix) {
        self.troubleshoot
            .tried
            .lock()
            .entry(id.clone())
            .or_default()
            .insert(fix.key());
    }

    /// Install winetricks `verbs` into the game's prefix: winetricks (or
    /// umu's) for Irori's prefixes, protontricks for Steam's own.
    pub async fn install_prefix_verbs(&self, id: &GameId, verbs: Vec<String>) -> Result<()> {
        anyhow::ensure!(!verbs.is_empty(), "nothing to install");
        if id.runner == Runner::Steam {
            self.protontricks(id, verbs).await?;
        } else {
            self.winetricks(id, verbs).await?;
        }
        Ok(())
    }

    /// Stop the wineserver of another Wine version still running for `id`'s
    /// prefix, which makes every start fail with "version mismatch". On
    /// Linux it is found by its `WINEPREFIX`, since a client of the game's
    /// own Wine can't talk to it; elsewhere the game's Wine asks it to stop.
    pub async fn stop_prefix_wineserver(&self, id: &GameId) -> Result<()> {
        let game = self.game(id).context("unknown game")?;
        let app = self.settings.app();
        let settings = self.settings.game(id);
        let prefix = irori_runtime::prefix_path(&app, &settings, &game);
        if cfg!(target_os = "linux") {
            let wanted = vec![prefix.clone(), prefix.join("pfx")];
            let pids = self
                .spawn_blocking(move || prefix_servers(Path::new("/proc"), &wanted))
                .await
                .unwrap_or_default();
            if !pids.is_empty() {
                let kill = irori_core::process::CommandSpec::new("kill")
                    .arg("-TERM")
                    .args(pids.iter().map(u32::to_string));
                let out = irori_core::process::run(&kill, Default::default())
                    .await
                    .context("could not stop the wineserver")?;
                anyhow::ensure!(out.success(), "{}", out.error_summary());
                return Ok(());
            }
        }
        irori_runtime::kill_game_prefix(&app, &settings, &game)
            .await
            .context("could not stop the wineserver")
    }

    async fn launch_facts(&self, id: &GameId, game: &GameInfo, exit_code: Option<i32>) -> Facts {
        let app = self.settings.app();
        let settings = self.settings.game(id);
        let exit_code = exit_code.or_else(|| {
            self.troubleshoot
                .last_exit
                .lock()
                .get(id)
                .copied()
                .flatten()
        });
        let platform = game.installed_platform();
        let host = HostPlatform::current();
        let needs_wine = platform.is_some_and(|p| host.needs_wine(p));

        let target_exe = settings.target_exe.trim().to_string();
        let target = (!target_exe.is_empty()).then(|| irori_core::paths::expand_home(&target_exe));
        let own_exe = game.install.as_ref().and_then(|install| {
            let exe = install.executable.trim();
            (!exe.is_empty() && !install.install_path.is_empty())
                .then(|| Path::new(&install.install_path).join(exe))
        });
        let prefix = needs_wine.then(|| match id.runner {
            Runner::Steam => game
                .install_path()
                .and_then(|p| irori_runtime::protontricks::prefix_of(&p, &id.app_name)),
            _ => Some(irori_runtime::prefix_path(&app, &settings, game)),
        });
        let log = Paths::get().game_log_file(id);
        let vulkan_command = if cfg!(target_os = "linux") {
            self.spawn_blocking(read_vulkan_command)
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        let rockstar_from_epic = needs_wine && is_rockstar_from_epic(id.runner, game);
        let (ptrace_scope, launcher_lines) = if rockstar_from_epic && cfg!(target_os = "linux") {
            let prefix = prefix.clone().flatten();
            self.spawn_blocking(move || {
                (
                    read_ptrace_scope(),
                    prefix
                        .as_deref()
                        .map(rockstar_launcher_log)
                        .unwrap_or_default(),
                )
            })
            .await
            .unwrap_or_default()
        } else {
            (None, Vec::new())
        };
        let (lines, target_missing, exe_missing, program, installed_verbs, map_count) = {
            let (target, own_exe) = (target.clone(), own_exe.clone());
            let prefix = prefix.flatten();
            self.spawn_blocking(move || {
                let exists = |p: &Path| p.try_exists().unwrap_or(true);
                let target_missing = target.as_deref().is_some_and(|t| !exists(t));
                let exe_missing = own_exe.as_deref().is_some_and(|e| !exists(e));
                let started = target.filter(|_| !target_missing).or(own_exe);
                let program = started
                    .filter(|_| needs_wine)
                    .filter(|p| is_windows_program(p))
                    .and_then(|p| pe::read_file(&p));
                let verbs = prefix.as_deref().map(installed_verbs).unwrap_or_default();
                (
                    read_game_output(&log),
                    target_missing,
                    exe_missing,
                    program,
                    verbs,
                    read_map_count(),
                )
            })
            .await
            .unwrap_or_default()
        };
        let anticheat = if needs_wine && id.runner != Runner::Sideload {
            let steam_app_id = (id.runner == Runner::Steam).then(|| id.app_name.to_string());
            anticheat::lookup(&AntiCheatQuery {
                title: &game.title,
                epic_namespace: game.namespace.as_deref().filter(|n| !n.is_empty()),
                steam_app_id: steam_app_id.as_deref(),
            })
            .await
        } else {
            None
        };
        Facts {
            lines,
            exit_code,
            program,
            installed_verbs,
            anticheat,
            map_count,
            ptrace_scope,
            rockstar_from_epic,
            launcher_lines,
            vulkan_command,
            context: FixContext {
                needs_wine,
                verifiable: id.runner.manages_installs()
                    && !matches!(id.runner, Runner::Sideload | Runner::Emulation),
                anticheat_runtimes_available: cfg!(target_os = "linux"),
                anticheat_runtimes_on: settings.eac_runtime && settings.battl_eye_runtime,
                target_exe_missing: target_missing,
                exe_missing,
                native_without_runtime: cfg!(target_os = "linux")
                    && platform == Some(InstallPlatform::Linux)
                    && settings
                        .steam_runtime
                        .as_deref()
                        .is_none_or(|r| r.trim().is_empty()),
                plain_wine: cfg!(target_os = "linux")
                    && needs_wine
                    && id.runner != Runner::Steam
                    && settings.wine_version.kind == WineKind::Wine,
                dxvk_on: settings.auto_install_dxvk,
                vkd3d_on: settings.auto_install_vkd3d,
                sync_switchable: cfg!(target_os = "linux")
                    && needs_wine
                    && id.runner != Runner::Steam,
                esync_on: settings.enable_esync,
                fsync_on: settings.enable_fsync,
            },
        }
    }
}

/// The command installing this system's Vulkan driver: the distribution from
/// os-release (the host's inside a Flatpak), the GPUs from sysfs.
fn read_vulkan_command() -> Option<String> {
    let os_release = ["/run/host/os-release", "/etc/os-release"]
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())?;
    let gpus = irori_runtime::sysinfo::read_gpus(Path::new("/sys/bus/pci/devices"));
    let mut vendors: Vec<GpuVendor> = Vec::new();
    for v in gpus
        .iter()
        .filter_map(|g| GpuVendor::from_pci(&g.vendor_id))
    {
        if !vendors.contains(&v) {
            vendors.push(v);
        }
    }
    vulkan_driver_command(&distro_ids(&os_release), &vendors)
}

/// Process ids under `proc` of wineservers whose `WINEPREFIX` is one of
/// `prefixes`. wineserver inherits the environment of the Wine client that
/// started it, so its prefix is known even when its version isn't ours.
fn prefix_servers(proc: &Path, prefixes: &[PathBuf]) -> Vec<u32> {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let wanted: Vec<PathBuf> = prefixes.iter().map(|p| canonical(p)).collect();
    let Ok(entries) = std::fs::read_dir(proc) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            let comm = std::fs::read_to_string(e.path().join("comm")).ok()?;
            if !comm.trim().starts_with("wineserver") {
                return None;
            }
            let environ = std::fs::read(e.path().join("environ")).ok()?;
            let prefix = environ_prefix(&environ)?;
            wanted.contains(&canonical(&prefix)).then_some(pid)
        })
        .collect()
}

/// `WINEPREFIX` in a `/proc/<pid>/environ` (NUL-separated `NAME=value`).
fn environ_prefix(environ: &[u8]) -> Option<PathBuf> {
    environ.split(|b| *b == 0).find_map(|var| {
        let value = var.strip_prefix(b"WINEPREFIX=")?;
        let text = String::from_utf8_lossy(value);
        let trimmed = text.trim_end_matches('/');
        (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
    })
}

/// A file Wine starts as a Windows program.
fn is_windows_program(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
}

/// What the game wrote in its last session: the end of its log, without
/// Irori's header.
fn read_game_output(path: &Path) -> Vec<String> {
    let Some(text) = read_tail(path, LOG_TAIL_BYTES) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    let start = game_output_start(lines.iter().copied());
    lines[start..].iter().map(|l| l.to_string()).collect()
}

/// The last `max` bytes of `path` as text, from the first whole line.
fn read_tail(path: &Path, max: u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(max);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    Some(if start > 0 {
        text.split_once('\n')
            .map_or_else(String::new, |(_, rest)| rest.to_string())
    } else {
        text.into_owned()
    })
}

/// Verbs recorded in a prefix's `winetricks.log`. Proton keeps the Wine
/// prefix in `pfx`, so both places are read.
fn installed_verbs(prefix: &Path) -> Vec<String> {
    let mut verbs: Vec<String> = Vec::new();
    let logs: [PathBuf; 2] = [
        prefix.join("pfx").join("winetricks.log"),
        prefix.join("winetricks.log"),
    ];
    for log in logs {
        let Ok(text) = std::fs::read_to_string(&log) else {
            continue;
        };
        for verb in text.lines().map(str::trim) {
            if !verb.is_empty() && !verb.starts_with('-') && !verbs.iter().any(|v| v == verb) {
                verbs.push(verb.to_string());
            }
        }
    }
    verbs
}

/// Whether `game` is a Rockstar title from Epic: those start through the
/// Rockstar Games Launcher in the game's prefix, which checks how it was
/// started.
fn is_rockstar_from_epic(runner: Runner, game: &GameInfo) -> bool {
    runner == Runner::Legendary
        && [&game.developer, &game.publisher]
            .into_iter()
            .flatten()
            .any(|maker| maker.to_ascii_lowercase().contains("rockstar"))
}

/// Yama's `ptrace_scope`, on Linux kernels that have Yama.
fn read_ptrace_scope() -> Option<u8> {
    std::fs::read_to_string("/proc/sys/kernel/yama/ptrace_scope")
        .ok()
        .and_then(|t| analysis::parse_ptrace_scope(&t))
}

/// The end of the Rockstar Games Launcher's `launcher.log` in `prefix`
/// (any Wine user; Proton keeps `drive_c` in `pfx`).
fn rockstar_launcher_log(prefix: &Path) -> Vec<String> {
    const TAIL: u64 = 64 * 1024;
    let mut lines = Vec::new();
    for root in [prefix.join("pfx"), prefix.to_path_buf()] {
        let Ok(users) = std::fs::read_dir(root.join("drive_c/users")) else {
            continue;
        };
        for user in users.flatten() {
            let log = user
                .path()
                .join("Documents/Rockstar Games/Launcher/launcher.log");
            if let Some(text) = read_tail(&log, TAIL) {
                lines.extend(text.lines().map(str::to_string));
            }
        }
    }
    lines
}

/// `vm.max_map_count`, on Linux.
fn read_map_count() -> Option<u64> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    std::fs::read_to_string("/proc/sys/vm/max_map_count")
        .ok()
        .and_then(|t| analysis::parse_map_count(&t))
}
