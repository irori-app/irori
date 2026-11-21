//! Following a Steam game after `steam://rungameid` hands it to the client.
//! The opener exits at once, so the game is found by its processes: on Linux
//! Steam starts every game under a "reaper" whose command line names the app
//! (`reaper SteamLaunch AppId=620 -- …`), on every system the game's own
//! processes run from its install folder, and while Steam updates or
//! validates the game before it starts, its manifest says so.
//!
//! Once processes were found, later checks look at those processes only and
//! fall back to a full scan when they are gone, so a long session costs one
//! lookup per check rather than a walk over every process on the computer.
//!
//! Inside a Flatpak Irori sees only its sandbox's processes, so the game's are
//! listed and stopped on the host (`irori_core::host`) instead.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use irori_core::GameWatch;
use irori_core::host::{self, HostProcess};
use parking_lot::Mutex;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System, UpdateKind};

use crate::scan::{self, state};

/// Watch over one Steam game.
pub(crate) struct SteamGameWatch {
    app_id: String,
    install_path: String,
    manifest: PathBuf,
    /// The game's processes found by the last check.
    pids: Mutex<Vec<Pid>>,
    /// Whether any process belonging to the session has been observed yet.
    seen_any: AtomicBool,
    /// Whether an executable inside [`Self::install_path`] itself has started.
    seen_direct: AtomicBool,
    /// Whether the game's processes are the host's, Irori being a Flatpak.
    on_host: bool,
}

impl SteamGameWatch {
    pub(crate) fn new(app_id: String, install_path: String, manifest: PathBuf) -> Self {
        Self {
            app_id,
            install_path,
            manifest,
            pids: Mutex::new(Vec::new()),
            seen_any: AtomicBool::new(false),
            seen_direct: AtomicBool::new(false),
            on_host: cfg!(target_os = "linux") && host::is_flatpak(),
        }
    }

    /// Whether the process with this command line and program belongs to
    /// the game.
    fn is_game_process(&self, exe: Option<&std::path::Path>, cmd: &[String]) -> bool {
        is_reaper_for(&self.app_id, cmd)
            || irori_core::process::runs_from(&self.install_path, exe, cmd)
    }

    /// Whether the host process belongs to the game. `ps` joins the
    /// arguments with spaces, so the reaper is matched on the split line and
    /// the install folder, which may hold spaces, on the whole of it.
    fn is_game_host_process(&self, process: &HostProcess) -> bool {
        is_reaper_for(&self.app_id, &process.args())
            || irori_core::process::runs_from(
                &self.install_path,
                None,
                std::slice::from_ref(&process.command_line),
            )
    }

    fn is_direct_process(&self, exe: Option<&std::path::Path>, cmd: &[String]) -> bool {
        is_direct_install_program(&self.install_path, exe, cmd)
    }

    fn is_direct_host_process(&self, process: &HostProcess) -> bool {
        is_direct_install_program(&self.install_path, None, &process.args())
            || command_line_starts_with_install_dir(&self.install_path, &process.command_line)
    }

    /// Scan processes for the session. When `allow_direct_narrowing` is true
    /// and a process running directly from [`Self::install_path`] has already
    /// appeared, subsequent polls only track direct game executables so
    /// `reaper`/`wineserver` teardown does not delay exit detection.
    fn find(&self, allow_direct_narrowing: bool) -> Vec<Pid> {
        let found = if self.on_host {
            self.find_on_host(allow_direct_narrowing)
        } else {
            self.find_local(allow_direct_narrowing)
        };
        if allow_direct_narrowing && !found.is_empty() {
            self.seen_any.store(true, Ordering::Relaxed);
        }
        found
    }

    fn select_pids<T>(
        &self,
        allow_direct_narrowing: bool,
        known: &[u32],
        scan: impl Fn(Option<&[u32]>) -> T,
        pick: impl Fn(&T, bool) -> Vec<Pid>,
    ) -> Vec<Pid> {
        let direct_only = allow_direct_narrowing && self.seen_direct.load(Ordering::Relaxed);
        if allow_direct_narrowing && direct_only && !known.is_empty() {
            let alive = pick(&scan(Some(known)), true);
            if !alive.is_empty() {
                *self.pids.lock() = alive.clone();
                return alive;
            }
        }
        let all = scan(None);
        if !allow_direct_narrowing {
            return pick(&all, false);
        }
        if direct_only {
            let found = pick(&all, true);
            *self.pids.lock() = found.clone();
            return found;
        }
        let direct = pick(&all, true);
        if !direct.is_empty() {
            self.seen_direct.store(true, Ordering::Relaxed);
            *self.pids.lock() = direct.clone();
            return direct;
        }
        let found = pick(&all, false);
        *self.pids.lock() = found.clone();
        found
    }

    fn find_local(&self, allow_direct_narrowing: bool) -> Vec<Pid> {
        let known: Vec<u32> = self.pids.lock().iter().map(|p| p.as_u32()).collect();
        let refresh = ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_exe(UpdateKind::Always);
        self.select_pids(
            allow_direct_narrowing,
            &known,
            |subset| {
                let mut sys = System::new();
                match subset {
                    Some(ids) => {
                        let pids: Vec<Pid> = ids.iter().map(|&id| Pid::from_u32(id)).collect();
                        sys.refresh_processes_specifics(
                            ProcessesToUpdate::Some(&pids),
                            true,
                            refresh,
                        );
                    }
                    None => {
                        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);
                    }
                }
                sys
            },
            |sys, direct| self.matching(sys, direct),
        )
    }

    /// [`Self::find_local`] among the host's processes. Runs on a blocking
    /// thread of the backend's runtime, as every check of a watch does.
    fn find_on_host(&self, allow_direct_narrowing: bool) -> Vec<Pid> {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return Vec::new();
        };
        let known: Vec<u32> = self.pids.lock().iter().map(|p| p.as_u32()).collect();
        self.select_pids(
            allow_direct_narrowing,
            &known,
            |subset| match runtime.block_on(host::processes(subset)) {
                Ok(processes) => processes,
                Err(e) => {
                    tracing::warn!("listing the host's processes: {e:#}");
                    Vec::new()
                }
            },
            |processes, direct| {
                processes
                    .iter()
                    .filter(|p| {
                        if direct {
                            self.is_direct_host_process(p)
                        } else {
                            self.is_game_host_process(p)
                        }
                    })
                    .map(|p| Pid::from_u32(p.pid))
                    .collect()
            },
        )
    }

    fn matching(&self, sys: &System, direct_only: bool) -> Vec<Pid> {
        sys.processes()
            .iter()
            .filter(|(_, p)| {
                let cmd: Vec<String> = p
                    .cmd()
                    .iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect();
                if direct_only {
                    self.is_direct_process(p.exe(), &cmd)
                } else {
                    self.is_game_process(p.exe(), &cmd)
                }
            })
            .map(|(pid, _)| *pid)
            .collect()
    }
}

impl GameWatch for SteamGameWatch {
    fn running(&self) -> bool {
        if !self.find(true).is_empty() {
            return true;
        }
        if self.seen_any.load(Ordering::Relaxed) {
            return false;
        }
        // Before any game process has appeared, a launch-time update or
        // validation shows in the manifest.
        scan::manifest_flags(&self.manifest)
            .is_some_and(|flags| flags & state::LAUNCHING_OR_RUNNING != 0)
    }

    /// Ends the reaper and the game's processes. The Steam client itself
    /// never matches, so it keeps running.
    fn stop(&self, force: bool) {
        let pids = self.find(false);
        if self.on_host {
            let pids: Vec<u32> = pids.iter().map(|p| p.as_u32()).collect();
            if let Ok(runtime) = tokio::runtime::Handle::try_current()
                && let Err(e) = runtime.block_on(host::signal(&pids, force))
            {
                tracing::warn!("stopping the game's host processes: {e:#}");
            }
            return;
        }
        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&pids),
            true,
            ProcessRefreshKind::nothing(),
        );
        for pid in pids {
            if let Some(process) = sys.process(pid) {
                // Windows has no polite signal; terminating is the only stop.
                let sent = (!force)
                    .then(|| process.kill_with(Signal::Term))
                    .flatten()
                    .unwrap_or(false);
                if !sent {
                    process.kill();
                }
            }
        }
    }

    fn wait_for_exit(&self, timeout: std::time::Duration) -> bool {
        if self.on_host || !self.seen_direct.load(Ordering::Relaxed) {
            return false;
        }
        let pids: Vec<u32> = self.pids.lock().iter().map(|p| p.as_u32()).collect();
        irori_core::process::wait_pids_exit(&pids, timeout)
    }
}

/// Whether `cmd` is the command line of the reaper Steam runs a game under
/// (Linux): `reaper SteamLaunch AppId=<app_id> -- …`, matched on the whole
/// ID so app 440 doesn't match 4400.
fn is_reaper_for(app_id: &str, cmd: &[String]) -> bool {
    let is_reaper = cmd.first().is_some_and(|program| {
        std::path::Path::new(program)
            .file_name()
            .is_some_and(|name| name == "reaper")
    });
    if !is_reaper || !cmd.iter().any(|a| a == "SteamLaunch") {
        return false;
    }
    let wanted = format!("AppId={app_id}");
    cmd.iter().take_while(|a| *a != "--").any(|a| *a == wanted)
}

fn path_matches_install_prefix(
    candidate: &str,
    install_path: &str,
    allow_space_boundary: bool,
) -> bool {
    let normalize = |s: &str| s.replace('\\', "/").to_lowercase();
    let folder = normalize(install_path.trim_end_matches(['\\', '/']));
    if folder.len() < 6 {
        return false;
    }
    let n = normalize(candidate);
    let matches_tail = |rest: &str| {
        rest.is_empty() || rest.starts_with('/') || (allow_space_boundary && rest.starts_with(' '))
    };
    if let Some(rest) = n.strip_prefix(&folder)
        && matches_tail(rest)
    {
        return true;
    }
    let tail_no_drive = folder.split_once(':').map_or(folder.as_str(), |(_, r)| r);
    if !tail_no_drive.is_empty()
        && n.len() >= 2
        && n.as_bytes()[0].is_ascii_alphabetic()
        && n.as_bytes()[1] == b':'
        && let Some(rest) = n[2..].strip_prefix(tail_no_drive)
        && matches_tail(rest)
    {
        return true;
    }
    let Some((_, common_tail)) = folder.split_once("/steamapps/common/") else {
        return false;
    };
    if common_tail.is_empty() {
        return false;
    }
    let marker = format!("/steamapps/common/{common_tail}");
    if let Some((_, rest)) = n.split_once(&marker)
        && !n.starts_with("c:/windows/")
        && !n.contains("/ubuntu12_32/")
        && !n.contains("/steamrt")
        && !n.contains("/pressure-vessel/")
        && !n.contains(" -- ")
        && matches_tail(rest)
    {
        return true;
    }
    false
}

/// Whether the process's own executable (`exe` or `cmd[0]`, not a trailing
/// argument passed to `reaper`, `pv-adverb`, `proton` or Wine's `steam.exe`)
/// lives inside `install_path` (including Proton's `S:\steamapps\common\...`
/// mount of the Steam library folder).
fn is_direct_install_program(
    install_path: &str,
    exe: Option<&std::path::Path>,
    cmd: &[String],
) -> bool {
    exe.is_some_and(|e| path_matches_install_prefix(&e.to_string_lossy(), install_path, false))
        || cmd
            .first()
            .is_some_and(|a| path_matches_install_prefix(a, install_path, true))
}

fn command_line_starts_with_install_dir(install_path: &str, command_line: &str) -> bool {
    path_matches_install_prefix(command_line, install_path, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split(' ').map(String::from).collect()
    }

    #[test]
    fn recognises_the_reaper_of_one_app() {
        let reaper = args(
            "/home/u/.local/share/Steam/ubuntu12_32/reaper SteamLaunch AppId=440 -- /home/u/.local/share/Steam/ubuntu12_32/steam-launch-wrapper -- hl2.sh",
        );
        assert!(is_reaper_for("440", &reaper));
        assert!(!is_reaper_for("4400", &reaper));
        assert!(!is_reaper_for("44", &reaper));
        // The game's own arguments after `--` don't count.
        assert!(!is_reaper_for(
            "620",
            &args("reaper SteamLaunch AppId=440 -- game AppId=620")
        ));
        assert!(!is_reaper_for("440", &args("steam SteamLaunch AppId=440")));
    }

    #[test]
    fn recognises_the_game_among_host_processes() {
        let watch = SteamGameWatch::new(
            "440".into(),
            "/home/u/.local/share/Steam/steamapps/common/Team Fortress 2".into(),
            PathBuf::from("/nonexistent/appmanifest_440.acf"),
        );
        let process = |command_line: &str| HostProcess {
            pid: 7,
            command_line: command_line.into(),
        };
        assert!(watch.is_game_host_process(&process(
            "/home/u/.local/share/Steam/ubuntu12_32/reaper SteamLaunch AppId=440 -- hl2.sh"
        )));
        // A folder holding spaces matches on the whole command line.
        assert!(watch.is_game_host_process(&process(
            "/home/u/.local/share/Steam/steamapps/common/Team Fortress 2/tf_linux64 -game tf"
        )));
        assert!(!watch.is_game_host_process(&process(
            "/home/u/.local/share/Steam/ubuntu12_32/steam -silent"
        )));
    }

    #[test]
    fn follows_the_manifest_while_steam_prepares_the_launch() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("appmanifest_620.acf");
        let watch = SteamGameWatch::new(
            "620".into(),
            dir.path()
                .join("no such game folder")
                .to_string_lossy()
                .into_owned(),
            manifest.clone(),
        );
        let write = |flags: u64| {
            std::fs::write(
                &manifest,
                format!("\"AppState\" {{ \"appid\" \"620\" \"StateFlags\" \"{flags}\" }}"),
            )
            .unwrap()
        };
        write(state::FULLY_INSTALLED | state::UPDATE_RUNNING);
        assert!(watch.running());
        write(state::FULLY_INSTALLED | state::APP_RUNNING);
        assert!(watch.running());
        // A pending update alone is not a launch.
        write(state::FULLY_INSTALLED | 2);
        assert!(!watch.running());
    }

    #[test]
    fn distinguishes_direct_game_executables_from_steam_and_proton_wrappers() {
        let install = "/mnt/Asgard/SteamLibrary/steamapps/common/BangOnBalls";

        assert!(is_direct_install_program(
            install,
            None,
            &[r"S:\steamapps\common\BangOnBalls\BoB.exe".into()],
        ));
        assert!(is_direct_install_program(
            install,
            None,
            &[
                r"S:\steamapps\common\BangOnBalls\BoB\Binaries\Win64\BoB-Win64-Shipping.exe".into(),
                "BoB".into(),
            ],
        ));
        assert!(is_direct_install_program(
            install,
            Some(std::path::Path::new(
                "/mnt/Asgard/SteamLibrary/steamapps/common/BangOnBalls/BoB.x86_64"
            )),
            &["BoB.x86_64".into()],
        ));

        assert!(!is_direct_install_program(
            install,
            Some(std::path::Path::new(
                "/home/u/.local/share/Steam/ubuntu12_32/reaper"
            )),
            &args(
                "/home/u/.local/share/Steam/ubuntu12_32/reaper SteamLaunch AppId=1227650 -- /mnt/Asgard/SteamLibrary/steamapps/common/BangOnBalls/BoB.exe",
            ),
        ));
        assert!(!is_direct_install_program(
            install,
            Some(std::path::Path::new("/usr/bin/python3")),
            &args(
                "python3 /home/u/.local/share/Steam/compatibilitytools.d/Proton/proton waitforexitandrun /mnt/Asgard/SteamLibrary/steamapps/common/BangOnBalls/BoB.exe",
            ),
        ));
        assert!(!is_direct_install_program(
            install,
            None,
            &args(
                r"c:\windows\system32\steam.exe /mnt/Asgard/SteamLibrary/steamapps/common/BangOnBalls/BoB.exe",
            ),
        ));
        assert!(!is_direct_install_program(
            install,
            None,
            &[r"S:\steamapps\common\BangOnBalls2\Game.exe".into()],
        ));
    }
}
