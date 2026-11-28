//! Turning the screen off now, for a night's downloads (the Downloads page's
//! button, the palette). The desktop does it through its own display power
//! interface, which turns the screen on again at the next key press or
//! mouse move: KDE's `kscreen-doctor`, GNOME's display configuration on
//! the session bus, X11's DPMS, macOS's `pmset displaysleepnow`, Windows'
//! monitor power message. Nothing needs root, and Irori never turns the
//! screen off where nothing would turn it on again (sway, Hyprland and
//! gamescope keep an output off until told otherwise), so there the button
//! isn't offered.
//!
//! While the screen is off and downloads run, the computer is kept from
//! sleeping (the display may still), so the queue finishes and the "when
//! downloads finish" choice still applies.

use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use irori_core::Event;
use tokio::sync::broadcast::error::RecvError;

use crate::Backend;
use crate::inhibit::Inhibitor;

/// Time for the click (or key press) that asked to settle first: a mouse
/// still moving would wake the screen at once.
const SETTLE: Duration = Duration::from_millis(800);

/// How this computer turns its screen off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenOff {
    /// A program of the desktop's, run as given.
    Program {
        program: &'static str,
        args: &'static [&'static str],
    },
    /// Windows' `SC_MONITORPOWER` broadcast.
    WindowsMessage,
}

/// GNOME's display configuration: power save mode 3 is off; gnome-settings-
/// daemon turns the screen on again at the user's next input.
const GNOME_ARGS: &[&str] = &[
    "call",
    "--session",
    "--dest",
    "org.gnome.Mutter.DisplayConfig",
    "--object-path",
    "/org/gnome/Mutter/DisplayConfig",
    "--method",
    "org.freedesktop.DBus.Properties.Set",
    "org.gnome.Mutter.DisplayConfig",
    "PowerSaveMode",
    "<int32 3>",
];

/// How a Linux session turns its screen off, from `XDG_SESSION_TYPE` and
/// `XDG_CURRENT_DESKTOP`; `None` where nothing would turn it on again.
pub fn linux_method(session_type: &str, desktop: &str) -> Option<ScreenOff> {
    let desktops: Vec<String> = desktop
        .split(':')
        .map(|d| d.trim().to_ascii_lowercase())
        .collect();
    let is = |name: &str| desktops.iter().any(|d| d == name);
    if is("gamescope") {
        return None;
    }
    if is("kde") {
        return Some(ScreenOff::Program {
            program: "kscreen-doctor",
            args: &["--dpms", "off"],
        });
    }
    if is("gnome") || is("unity") {
        return Some(ScreenOff::Program {
            program: "gdbus",
            args: GNOME_ARGS,
        });
    }
    // X11's DPMS wakes at the next input on every window manager.
    (session_type.eq_ignore_ascii_case("x11")).then_some(ScreenOff::Program {
        program: "xset",
        args: &["dpms", "force", "off"],
    })
}

/// How this computer turns its screen off, `None` where it can't safely.
/// Reads only the environment, so the UI may ask it while drawing.
pub fn method() -> Option<ScreenOff> {
    if cfg!(windows) {
        Some(ScreenOff::WindowsMessage)
    } else if cfg!(target_os = "macos") {
        Some(ScreenOff::Program {
            program: "pmset",
            args: &["displaysleepnow"],
        })
    } else {
        let var = |name: &str| std::env::var(name).unwrap_or_default();
        linux_method(&var("XDG_SESSION_TYPE"), &var("XDG_CURRENT_DESKTOP"))
    }
}

#[cfg(windows)]
fn windows_monitor_off() -> Result<()> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, PostMessageW, SC_MONITORPOWER, WM_SYSCOMMAND,
    };
    // SAFETY: a broadcast message with plain integer arguments; posting
    // (unlike sending) never waits on another program's window.
    let ok = unsafe { PostMessageW(HWND_BROADCAST, WM_SYSCOMMAND, SC_MONITORPOWER as usize, 2) };
    anyhow::ensure!(ok != 0, "Windows didn't take the request");
    Ok(())
}

#[cfg(not(windows))]
fn windows_monitor_off() -> Result<()> {
    bail!("only Windows takes this request")
}

impl Backend {
    /// Whether the screen can be turned off here (see the module notes).
    pub fn can_turn_screen_off(&self) -> bool {
        method().is_some()
    }

    /// Turn the screen off now; it comes back at the next input. While
    /// downloads run, the computer stays awake until they are done.
    pub async fn turn_screen_off(&self) -> Result<()> {
        let Some(method) = method() else {
            bail!("This desktop can't turn the screen off and on again by itself");
        };
        tokio::time::sleep(SETTLE).await;
        match method {
            ScreenOff::WindowsMessage => windows_monitor_off()?,
            ScreenOff::Program { program, args } => {
                use irori_core::process::{CommandSpec, RunOptions, run};
                let spec = irori_core::host::command(CommandSpec::new(program).args(args));
                run(&spec, RunOptions::default())
                    .await
                    .with_context(|| format!("{program} couldn't be started"))?
                    .check()
                    .with_context(|| format!("{program} didn't turn the screen off"))?;
            }
        }
        self.stay_awake_for_downloads();
        Ok(())
    }

    /// Keep the computer from sleeping until store, file, and web downloads
    /// (including extractions and setup installers) are done or paused, while
    /// the display may sleep.
    fn stay_awake_for_downloads(&self) {
        let running = |b: &Backend| b.has_active_transfers();
        if !running(self)
            || self
                .screen_off_hold
                .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let b = self.clone();
        self.spawn(async move {
            let mut rx = b.subscribe();
            let awake = Inhibitor::acquire_system("Downloading games").await;
            while running(&b) {
                match rx.recv().await {
                    Ok(Event::DownloadQueueChanged) | Err(RecvError::Lagged(_)) => {}
                    Ok(_) => {}
                    Err(RecvError::Closed) => break,
                }
            }
            if let Some(awake) = awake {
                awake.release().await;
            }
            b.screen_off_hold
                .store(false, std::sync::atomic::Ordering::Release);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktops_that_wake_the_screen_again() {
        let program = |m: Option<ScreenOff>| match m {
            Some(ScreenOff::Program { program, .. }) => Some(program),
            _ => None,
        };
        assert_eq!(
            program(linux_method("wayland", "KDE")),
            Some("kscreen-doctor")
        );
        assert_eq!(program(linux_method("x11", "KDE")), Some("kscreen-doctor"));
        assert_eq!(
            program(linux_method("wayland", "ubuntu:GNOME")),
            Some("gdbus")
        );
        assert_eq!(program(linux_method("x11", "XFCE")), Some("xset"));
        assert_eq!(program(linux_method("x11", "i3")), Some("xset"));
        // These keep an output off until told otherwise.
        assert_eq!(linux_method("wayland", "sway"), None);
        assert_eq!(linux_method("wayland", "Hyprland"), None);
        assert_eq!(linux_method("wayland", "gamescope"), None);
        assert_eq!(linux_method("x11", "gamescope"), None);
        assert_eq!(linux_method("", ""), None);
    }
}
