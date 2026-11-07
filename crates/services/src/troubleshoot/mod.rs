//! Launch troubleshooting: gathers what the diagnosis reads about a game's
//! last run (its log, its program's imports, the winetricks components in
//! its prefix, its anticheat entry, `vm.max_map_count`), remembers the fixes
//! tried this session, and installs winetricks components as a fix.

pub mod analysis;
pub mod pe;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use irori_core::{GameId, GameInfo, HostPlatform, InstallPlatform, Paths, Runner};
use irori_extras::anticheat::{self, AntiCheatQuery};
use parking_lot::Mutex;

pub use analysis::{
    Detail, Diagnosis, Facts, Finding, Fix, FixContext, LaunchHint, MAP_COUNT_RECOMMENDED,
    MAP_COUNT_WANTED, OfferedFix, diagnose, game_output_start, launch_hints, map_count_commands,
    missing_dlls, runtime_verbs,
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
            },
        }
    }
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

/// `vm.max_map_count`, on Linux.
fn read_map_count() -> Option<u64> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    std::fs::read_to_string("/proc/sys/vm/max_map_count")
        .ok()
        .and_then(|t| analysis::parse_map_count(&t))
}
