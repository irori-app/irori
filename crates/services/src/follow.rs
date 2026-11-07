//! Following a game that another program starts. The launcher's process is
//! not the game: Battle.net keeps running after the game ends, and Steam or
//! some launchers hand over and exit before it starts. While such a game
//! runs, its session lasts while a probe sees it (for third-party launchers,
//! processes running from its install folder), checked every few seconds
//! and only then.
//!
//! Any other game may hand over too: a crack's or a publisher's small
//! launcher starts the real game and exits, and so do many added games.
//! When the program Irori started exits, [`Handoff`] finds what it left
//! running and the session lasts until that is gone as well.

use std::process::ExitStatus;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::process::Child;

/// How often the game's processes are looked for.
const POLL: Duration = Duration::from_secs(3);
/// How long the launcher gets to start the game before the session is the
/// launcher's own.
const START_GRACE: Duration = Duration::from_secs(180);

/// Whether any process runs from `folder`. Blocking.
fn game_running(folder: &str) -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_exe(UpdateKind::Always),
    );
    sys.processes().values().any(|p| {
        let cmd: Vec<String> = p
            .cmd()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        irori_core::process::runs_from(folder, p.exe(), &cmd)
    })
}

/// Whether the followed game runs right now. Blocking.
pub(crate) type Probe = Arc<dyn Fn() -> bool + Send + Sync>;

/// A probe for the game whose processes run from `folder`.
pub(crate) fn folder_probe(folder: String) -> Probe {
    Arc::new(move || game_running(&folder))
}

/// What a launched program left running after it exited.
///
/// On Unix that is its process group: games start as a group of their own
/// (`irori_core::process::spawn`), and children stay in it unless they start
/// a session, as daemons and wineserver do. Windows has no such group, so it
/// is the program's descendants by parent process, started after it.
#[derive(Debug)]
pub(crate) struct Handoff {
    /// The process group's leader; Windows seeds `known` with it instead.
    #[cfg(unix)]
    pid: u32,
    /// Unix time a second before the program started; descendants started
    /// earlier are other programs that reused a pid.
    // Only Windows looks for descendants; Unix has the process group.
    #[cfg_attr(unix, allow(dead_code))]
    since: u64,
    /// Every pid known to belong to the game, alive or not, so the children
    /// of a step that already exited are still found.
    #[cfg(windows)]
    known: parking_lot::Mutex<std::collections::HashMap<u32, Option<u64>>>,
}

impl Handoff {
    /// Follow what the program `pid`, started just now, leaves behind.
    pub(crate) fn new(pid: u32) -> Arc<Self> {
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs().saturating_sub(1));
        Arc::new(Self {
            #[cfg(unix)]
            pid,
            since,
            #[cfg(windows)]
            known: parking_lot::Mutex::new([(pid, None)].into()),
        })
    }

    /// Whether anything the program left behind still runs. Blocking.
    pub(crate) fn running(&self) -> bool {
        #[cfg(unix)]
        {
            irori_core::process::group_alive(self.pid)
        }
        #[cfg(windows)]
        {
            !self.pids().is_empty()
        }
        #[cfg(not(any(unix, windows)))]
        {
            false
        }
    }

    /// The processes the program left running. Blocking.
    #[cfg(windows)]
    pub(crate) fn pids(&self) -> Vec<u32> {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        let table: Vec<Proc> = sys
            .processes()
            .values()
            .map(|p| Proc {
                pid: p.pid().as_u32(),
                parent: p.parent().map(|p| p.as_u32()),
                start: p.start_time(),
            })
            .collect();
        descendants(&table, &mut self.known.lock(), self.since)
    }

    /// A probe for [`follow`] and [`wait_while`].
    pub(crate) fn probe(self: &Arc<Self>) -> Probe {
        let handoff = self.clone();
        Arc::new(move || handoff.running())
    }
}

/// A process as the descendant search sees it.
#[derive(Debug, Clone, Copy)]
// Used on Windows; the selection itself is tested everywhere.
#[cfg_attr(unix, allow(dead_code))]
pub(crate) struct Proc {
    pub pid: u32,
    pub parent: Option<u32>,
    /// Unix time it started.
    pub start: u64,
}

/// The running processes in `table` that descend from a pid in `known`,
/// directly or through each other, started at `since` or later. `known`
/// maps each pid of the game to its start time (`None` for the launcher,
/// which has exited) and keeps what is found, so the children of a step
/// that has since exited are still found, and a pid another program reuses
/// later doesn't count.
// Used on Windows; the selection itself is tested everywhere.
#[cfg_attr(unix, allow(dead_code))]
pub(crate) fn descendants(
    table: &[Proc],
    known: &mut std::collections::HashMap<u32, Option<u64>>,
    since: u64,
) -> Vec<u32> {
    loop {
        let mut grew = false;
        for p in table {
            if p.start >= since
                && !known.contains_key(&p.pid)
                && p.parent.is_some_and(|parent| known.contains_key(&parent))
            {
                known.insert(p.pid, Some(p.start));
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    let mut running: Vec<u32> = table
        .iter()
        .filter(|p| known.get(&p.pid) == Some(&Some(p.start)))
        .map(|p| p.pid)
        .collect();
    running.sort_unstable();
    running
}

/// Wait while `probe` sees the game, checking every few seconds.
pub(crate) async fn wait_while(probe: Probe) {
    loop {
        let p = probe.clone();
        let running = tokio::task::spawn_blocking(move || p())
            .await
            .unwrap_or(false);
        if !running {
            return;
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Wait for the followed game to end: once `probe` saw it and no longer
/// does, or — when it never shows up — once the launcher exits or the grace
/// period passes. Returns the launcher's exit status when it ended.
pub(crate) async fn follow(child: &mut Child, probe: Probe) -> Option<ExitStatus> {
    let started = Instant::now();
    let mut seen = false;
    let mut launcher: Option<Option<ExitStatus>> = None;
    loop {
        tokio::select! {
            status = child.wait(), if launcher.is_none() => launcher = Some(status.ok()),
            _ = tokio::time::sleep(POLL) => {}
        }
        let p = probe.clone();
        let running = tokio::task::spawn_blocking(move || p())
            .await
            .unwrap_or(false);
        if running {
            seen = true;
            continue;
        }
        if seen {
            return launcher.flatten();
        }
        if let Some(status) = launcher
            && started.elapsed() >= POLL * 2
        {
            // The launcher closed without starting the game: let the
            // failed-start checks judge its exit.
            if started.elapsed() >= START_GRACE || status.is_some_and(|s| !s.success()) {
                return status;
            }
        }
        if started.elapsed() >= START_GRACE {
            return launcher.flatten();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{Proc, descendants};

    fn proc(pid: u32, parent: u32, start: u64) -> Proc {
        Proc {
            pid,
            parent: Some(parent),
            start,
        }
    }

    #[test]
    fn descendants_follow_a_chain_through_steps_that_exited() {
        // Launcher 10 started loader 11, which started the game 12; the game
        // later starts a helper 13, after the loader has exited.
        let mut known = HashMap::from([(10, None)]);
        let first = [proc(11, 10, 100), proc(500, 1, 50), proc(12, 11, 101)];
        assert_eq!(descendants(&first, &mut known, 99), [11, 12]);
        let later = [proc(12, 11, 101), proc(13, 12, 102), proc(500, 1, 50)];
        assert_eq!(descendants(&later, &mut known, 99), [12, 13]);
        assert!(descendants(&[proc(500, 1, 50)], &mut known, 99).is_empty());
    }

    #[test]
    fn descendants_leave_out_reused_pids_and_older_processes() {
        let mut known = HashMap::from([(10, None)]);
        // 20 names the launcher as parent but started before it: its real
        // parent's pid was reused by the launcher.
        let table = [proc(20, 10, 90), proc(21, 10, 100)];
        assert_eq!(descendants(&table, &mut known, 99), [21]);
        // 21 exited and another program got its pid.
        assert!(descendants(&[proc(21, 1, 300)], &mut known, 99).is_empty());
        // The launcher's own pid, reused, isn't the game.
        assert!(descendants(&[proc(10, 1, 400)], &mut known, 99).is_empty());
    }
}
