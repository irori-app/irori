//! Linux: Hyprland, sway, then X11 (see the parent module).

use std::collections::{HashMap, HashSet};

use anyhow::{Context as _, Result, bail};
use irori_core::process::{self, CommandSpec, RunOptions};

use super::{Raised, first_window_of, hyprland_window, sway_window};
use crate::audio::{GameProcess, belongs_to, parse_stat};

/// The game's processes: those in its process group or descending from
/// the process Irori started.
fn game_pids(root: u32) -> HashSet<u32> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return HashSet::from([root]);
    };
    let table: HashMap<u32, (u32, u32)> = dir
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            Some((pid, parse_stat(&stat)?))
        })
        .collect();
    let mut pids: HashSet<u32> = table
        .keys()
        .copied()
        .filter(|pid| belongs_to(*pid, root, &table))
        .collect();
    pids.insert(root);
    pids
}

/// Run a compositor's tool (on the host inside the Flatpak); its output.
async fn tool(name: &str, args: &[&str]) -> Result<String> {
    let program = irori_core::host::find_program(name)
        .await
        .with_context(|| format!("{name} isn't installed"))?;
    let spec = irori_core::host::command(CommandSpec::new(program).args(args.iter().copied()));
    let out = process::run(&spec, RunOptions::default()).await?;
    if !out.success() {
        bail!("{name}: {}", out.error_summary());
    }
    Ok(out.stdout)
}

/// Ask the X server's window manager to make the game's window active, as
/// a pager does (`_NET_ACTIVE_WINDOW` with source 2).
fn raise_x11(pids: &HashSet<u32>) -> Result<Raised> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask};

    let (conn, screen) = x11rb::connect(None).context("connecting to the X server")?;
    let root = conn.setup().roots[screen].root;
    let atom = |name: &[u8]| -> Result<u32> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
    let client_list = atom(b"_NET_CLIENT_LIST")?;
    let wm_pid = atom(b"_NET_WM_PID")?;
    let active = atom(b"_NET_ACTIVE_WINDOW")?;
    let list = conn
        .get_property(false, root, client_list, AtomEnum::WINDOW, 0, 4096)?
        .reply()?;
    let windows: Vec<u32> = list.value32().map(Iterator::collect).unwrap_or_default();
    let mut owned = Vec::new();
    for window in windows {
        let Ok(reply) = conn
            .get_property(false, window, wm_pid, AtomEnum::CARDINAL, 0, 1)?
            .reply()
        else {
            continue;
        };
        if let Some(pid) = reply.value32().and_then(|mut v| v.next()) {
            owned.push((window, pid));
        }
    }
    let Some(window) = first_window_of(&owned, pids) else {
        return Ok(Raised::NoWindow);
    };
    // Source 2: asked for by the user through a pager, which window managers
    // honour without their focus-stealing prevention; time 0 is "now".
    let event = ClientMessageEvent::new(32, window, active, [2u32, 0, 0, 0, 0]);
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        event,
    )?;
    conn.flush()?;
    Ok(Raised::Raised)
}

pub(super) async fn raise(game: GameProcess) -> Result<Raised> {
    let root = game.pid;
    let pids = tokio::task::spawn_blocking(move || game_pids(root))
        .await
        .unwrap_or_else(|_| HashSet::from([root]));
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        let clients = tool("hyprctl", &["clients", "-j"]).await?;
        let Some(address) = hyprland_window(&clients, &pids) else {
            return Ok(Raised::NoWindow);
        };
        let target = format!("address:{address}");
        tool("hyprctl", &["dispatch", "focuswindow", &target]).await?;
        return Ok(Raised::Raised);
    }
    if std::env::var_os("SWAYSOCK").is_some() {
        let tree = tool("swaymsg", &["-t", "get_tree"]).await?;
        let Some(id) = sway_window(&tree, &pids) else {
            return Ok(Raised::NoWindow);
        };
        tool("swaymsg", &[&format!("[con_id={id}]"), "focus"]).await?;
        return Ok(Raised::Raised);
    }
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    if std::env::var_os("DISPLAY").is_some() {
        // Under Wayland only games run through Xwayland are X11 windows.
        let raised = tokio::task::spawn_blocking(move || raise_x11(&pids))
            .await
            .context("The game's window couldn't be looked for")?;
        return match raised {
            Ok(Raised::NoWindow) if wayland => Ok(Raised::Unsupported),
            other => other,
        };
    }
    Ok(Raised::Unsupported)
}
