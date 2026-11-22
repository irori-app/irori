//! Bringing a running game's window to the front ("Back to the game").
//!
//! The game's processes are the session's: the process Irori started and
//! everything descending from it (on Linux also its process group), which
//! is how Wine's and Proton's processes are found. A window belongs to the
//! game when one of those processes owns it, and the desktop is asked to
//! raise it in its own way:
//!
//! - **Linux** (`linux.rs`): Hyprland's `hyprctl` and sway's `swaymsg`
//!   (their window lists name each window's process), else an X11 server
//!   (the session's, or Xwayland's for a game under X11 on Wayland):
//!   `_NET_CLIENT_LIST` and `_NET_WM_PID` find the window and
//!   `_NET_ACTIVE_WINDOW` raises it, as a pager would ask. GNOME and KDE
//!   Plasma on Wayland let no program raise another's window, so there
//!   nothing is done and the answer is [`Raised::Unsupported`].
//! - **Windows** (`windows.rs`): the game's visible top-level windows
//!   (`EnumWindows`), raised with `SetForegroundWindow`; Windows refuses that
//!   to a program in the background unless it attaches to the thread in
//!   front for the call (`AttachThreadInput`), which it does for that call
//!   only.
//! - **macOS** (`macos.rs`): System Events makes the game's application
//!   process frontmost (`osascript`; macOS asks once whether Irori may
//!   control System Events).
//!
//! Parsers of the desktops' answers are pure and tested here.

use std::collections::HashSet;

use anyhow::Result;
use irori_core::GameId;
use serde_json::Value;

use crate::Backend;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use super::*;

    pub(super) async fn raise(_game: crate::audio::GameProcess) -> Result<Raised> {
        Ok(Raised::Unsupported)
    }
}

/// What asking for the game's window did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Raised {
    /// The desktop was asked to bring it to the front.
    Raised,
    /// None of the game's processes has a window (yet).
    NoWindow,
    /// This desktop lets no program raise another's window.
    Unsupported,
}

/// The window Hyprland's `hyprctl clients -j` lists for one of `pids`: its
/// address, for `focuswindow address:…`. A mapped window first.
pub fn hyprland_window(json: &str, pids: &HashSet<u32>) -> Option<String> {
    let clients: Vec<Value> = serde_json::from_str(json).ok()?;
    let ours = |c: &&Value| {
        c.get("pid")
            .and_then(Value::as_u64)
            .and_then(|p| u32::try_from(p).ok())
            .is_some_and(|p| pids.contains(&p))
    };
    let mapped = |c: &&Value| c.get("mapped").and_then(Value::as_bool).unwrap_or(true);
    let pick = clients
        .iter()
        .filter(ours)
        .find(mapped)
        .or_else(|| clients.iter().find(ours))?;
    pick.get("address")?.as_str().map(str::to_owned)
}

/// The window sway's `swaymsg -t get_tree` holds for one of `pids`: its
/// container id, for `[con_id=…] focus`. Every node of the tree is looked
/// at, floating ones too.
pub fn sway_window(json: &str, pids: &HashSet<u32>) -> Option<u64> {
    let tree: Value = serde_json::from_str(json).ok()?;
    let mut stack = vec![&tree];
    // A tree this deep isn't sway's; the bound keeps a bad answer cheap.
    let mut budget = 100_000;
    while let Some(node) = stack.pop() {
        budget -= 1;
        if budget == 0 {
            return None;
        }
        let pid = node
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|p| u32::try_from(p).ok());
        if pid.is_some_and(|p| pids.contains(&p)) {
            return node.get("id").and_then(Value::as_u64);
        }
        for key in ["nodes", "floating_nodes"] {
            if let Some(children) = node.get(key).and_then(Value::as_array) {
                stack.extend(children.iter());
            }
        }
    }
    None
}

/// Of the windows `(window, pid)` listed, the first one of `pids`.
pub fn first_window_of<W: Copy>(windows: &[(W, u32)], pids: &HashSet<u32>) -> Option<W> {
    windows
        .iter()
        .find(|(_, pid)| pids.contains(pid))
        .map(|(w, _)| *w)
}

impl Backend {
    /// Bring the running game `id`'s window to the front. Fails when the
    /// game isn't running or the desktop refused.
    pub async fn focus_game_window(&self, id: &GameId) -> Result<Raised> {
        let game = self
            .game_process(id)
            .ok_or_else(|| anyhow::anyhow!("The game isn't running"))?;
        platform::raise(game).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pids(list: &[u32]) -> HashSet<u32> {
        list.iter().copied().collect()
    }

    #[test]
    fn finds_the_games_window_in_hyprland() {
        let json = r#"[
            {"address": "0x1", "pid": 10, "mapped": true, "class": "firefox"},
            {"address": "0x2", "pid": 42, "mapped": false, "class": "steam_app_0"},
            {"address": "0x3", "pid": 43, "mapped": true, "class": "game.exe"}
        ]"#;
        assert_eq!(
            hyprland_window(json, &pids(&[42, 43])).as_deref(),
            Some("0x3")
        );
        assert_eq!(hyprland_window(json, &pids(&[42])).as_deref(), Some("0x2"));
        assert_eq!(hyprland_window(json, &pids(&[7])), None);
        assert_eq!(hyprland_window("not json", &pids(&[42])), None);
    }

    #[test]
    fn finds_the_games_window_in_sways_tree() {
        let json = r#"{"id": 1, "type": "root", "nodes": [
            {"id": 2, "type": "output", "nodes": [
                {"id": 3, "type": "workspace", "nodes": [
                    {"id": 4, "pid": 10, "app_id": "foot"}
                ], "floating_nodes": [
                    {"id": 9, "pid": 77, "app_id": null, "window_properties": {"class": "game.exe"}}
                ]}
            ]}
        ]}"#;
        assert_eq!(sway_window(json, &pids(&[77])), Some(9));
        assert_eq!(sway_window(json, &pids(&[10, 11])), Some(4));
        assert_eq!(sway_window(json, &pids(&[5])), None);
        assert_eq!(sway_window("[]", &pids(&[5])), None);
    }

    #[test]
    fn picks_the_first_window_of_the_game() {
        let windows = [(100u32, 1), (200, 42), (300, 43)];
        assert_eq!(first_window_of(&windows, &pids(&[43, 42])), Some(200));
        assert_eq!(first_window_of(&windows, &pids(&[9])), None);
    }
}
