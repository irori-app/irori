//! macOS: System Events makes the game's application process frontmost
//! (see the parent module). Of the game's processes only those with a
//! window are application processes to System Events; the others are
//! skipped by the script.

use std::collections::HashSet;

use anyhow::{Result, bail};
use irori_core::process::{self, CommandSpec, RunOptions};

use super::Raised;
use crate::audio::{GameProcess, session_pids};
use crate::follow::Proc;

fn game_pids(game: &GameProcess) -> Vec<u32> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let table: Vec<Proc> = sys
        .processes()
        .values()
        .map(|p| Proc {
            pid: p.pid().as_u32(),
            parent: p.parent().map(|p| p.as_u32()),
            start: p.start_time(),
        })
        .collect();
    let mut pids: HashSet<u32> = session_pids(&table, game.pid, game.since);
    pids.insert(game.pid);
    let mut pids: Vec<u32> = pids.into_iter().collect();
    pids.sort_unstable();
    pids
}

/// The AppleScript that brings the first of `pids` System Events knows as
/// an application process to the front, and says `raised` or `none`.
pub(super) fn script(pids: &[u32]) -> String {
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "tell application \"System Events\"\n\
         repeat with p in {{{list}}}\n\
         try\n\
         set frontmost of (first process whose unix id is (p as integer)) to true\n\
         return \"raised\"\n\
         end try\n\
         end repeat\n\
         end tell\n\
         return \"none\""
    )
}

pub(super) async fn raise(game: GameProcess) -> Result<Raised> {
    let pids = tokio::task::spawn_blocking(move || game_pids(&game))
        .await
        .unwrap_or_default();
    if pids.is_empty() {
        return Ok(Raised::NoWindow);
    }
    let spec = CommandSpec::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script(&pids));
    let out = process::run(&spec, RunOptions::default()).await?;
    if !out.success() {
        bail!("osascript: {}", out.error_summary());
    }
    Ok(if out.stdout.trim() == "raised" {
        Raised::Raised
    } else {
        Raised::NoWindow
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_tries_each_process_in_turn() {
        let script = script(&[12, 345]);
        assert!(script.contains("repeat with p in {12, 345}"));
        assert!(script.contains("unix id is (p as integer)"));
        assert!(script.ends_with("return \"none\""));
    }
}
