//! Windows: the game's visible top-level windows, raised with
//! `SetForegroundWindow` (see the parent module).

use std::collections::HashSet;

use anyhow::{Context as _, Result};
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GetForegroundWindow, GetWindow, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindow,
};

use super::{Raised, first_window_of};
use crate::audio::{GameProcess, session_pids};
use crate::follow::Proc;

/// The game's processes (see `audio::session_pids`), and what a launcher
/// that handed over left running.
fn game_pids(game: &GameProcess) -> HashSet<u32> {
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
    let mut pids = session_pids(&table, game.pid, game.since);
    pids.insert(game.pid);
    if let Some(handoff) = &game.handoff {
        pids.extend(handoff.pids());
    }
    pids
}

unsafe extern "system" fn collect(window: HWND, list: LPARAM) -> i32 {
    // SAFETY: `list` is the `Vec` `top_windows` passed, alive for the
    // whole `EnumWindows` call; the window calls only read.
    unsafe {
        let list = &mut *(list as *mut Vec<(usize, u32)>);
        if IsWindowVisible(window) != 0 && GetWindow(window, GW_OWNER).is_null() {
            let mut pid = 0u32;
            GetWindowThreadProcessId(window, &mut pid);
            list.push((window as usize, pid));
        }
    }
    1
}

/// Every visible top-level window without an owner, with its process.
fn top_windows() -> Vec<(usize, u32)> {
    let mut list: Vec<(usize, u32)> = Vec::new();
    // SAFETY: `collect` only pushes into `list`, which outlives the call.
    unsafe { EnumWindows(Some(collect), &mut list as *mut _ as LPARAM) };
    list
}

/// Bring `window` to the front. Windows lets only the program in front
/// do that, so the call is made once attached to its input thread (and
/// detached right after) when the plain call is refused.
fn bring_to_front(window: HWND) -> bool {
    // SAFETY: window calls on a handle `EnumWindows` just listed; a window
    // that closed meanwhile makes them fail, nothing else. The threads are
    // attached for this call only.
    unsafe {
        if IsIconic(window) != 0 {
            ShowWindow(window, SW_RESTORE);
        }
        if SetForegroundWindow(window) != 0 {
            return true;
        }
        let front = GetForegroundWindow();
        let theirs = GetWindowThreadProcessId(front, std::ptr::null_mut());
        let ours = GetCurrentThreadId();
        if theirs == 0 || theirs == ours {
            return false;
        }
        AttachThreadInput(ours, theirs, 1);
        let raised = SetForegroundWindow(window) != 0;
        AttachThreadInput(ours, theirs, 0);
        raised
    }
}

fn raise_now(game: &GameProcess) -> Raised {
    let pids = game_pids(game);
    let Some(window) = first_window_of(&top_windows(), &pids) else {
        return Raised::NoWindow;
    };
    if bring_to_front(window as HWND) {
        Raised::Raised
    } else {
        tracing::info!("Windows kept another window in front of the game's");
        Raised::NoWindow
    }
}

pub(super) async fn raise(game: GameProcess) -> Result<Raised> {
    tokio::task::spawn_blocking(move || raise_now(&game))
        .await
        .context("The game's window couldn't be looked for")
}
