//! Powering off, restarting, sleeping and logging out, for Big Picture's
//! quick menu: a computer under a TV has no desktop to do it from.
//!
//! Each system's own unprivileged way is used, never `sudo`:
//!
//! - **Linux** (`linux.rs`): logind on the system bus. Its `CanPowerOff`,
//!   `CanReboot` and `CanSuspend` answers decide what is offered; `PowerOff`,
//!   `Reboot` and `Suspend` are called `interactive`, so polkit may ask for a
//!   password where the system wants one ("challenge"). Inside Irori's
//!   Flatpak the system bus is the host's, which the manifest's
//!   `--system-talk-name=org.freedesktop.login1` lets Irori reach. Logging
//!   out asks the desktop's session manager on the session bus (GNOME and the
//!   desktops forked from it, Plasma, Xfce), which lets programs save first;
//!   without one, logind ends the session itself.
//! - **Windows** (`windows.rs`): `ExitWindowsEx`, with the shutdown privilege
//!   every interactive user holds enabled for Irori's process first, and
//!   `SetSuspendState` for sleep, offered only when `IsPwrSuspendAllowed` and
//!   the PC is not a Modern Standby one (which doesn't sleep that way).
//! - **macOS** (`macos.rs`): System Events through `osascript`. macOS asks the
//!   user once whether Irori may control System Events.
//!
//! A running game is stopped first and its session ended in full (recorded,
//! saves uploaded, mods restored) before the system is asked
//! ([`Backend::power`]). [`PowerSystem`] is the seam between that and the
//! system, so tests never power anything off.

use std::time::Duration;

use anyhow::{Result, bail};
use futures::future::BoxFuture;
use tokio::sync::watch;

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

    pub(super) async fn permissions() -> Vec<(PowerAction, Permission)> {
        Vec::new()
    }

    pub(super) async fn perform(_action: PowerAction) -> Result<()> {
        bail!("Irori can't power this system off")
    }
}

/// What can be done to the computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PowerAction {
    PowerOff,
    Restart,
    Sleep,
    LogOut,
}

impl PowerAction {
    /// Every action, in the order they are offered.
    pub const ALL: [PowerAction; 4] = [
        PowerAction::PowerOff,
        PowerAction::Restart,
        PowerAction::Sleep,
        PowerAction::LogOut,
    ];
}

/// Whether the system lets this user take an action now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// Allowed.
    Yes,
    /// Allowed after the system asks for a password (polkit).
    Challenge,
    /// Not allowed for this user.
    No,
    /// The system can't (no sleep support, no session to log out of).
    Unavailable,
}

impl Permission {
    /// logind's answer to `CanPowerOff` and the like: `yes`, `challenge`,
    /// `no` or `na`. Anything else, a newer answer included, is unavailable.
    pub fn from_logind(answer: &str) -> Self {
        match answer.trim() {
            "yes" => Permission::Yes,
            "challenge" => Permission::Challenge,
            "no" => Permission::No,
            _ => Permission::Unavailable,
        }
    }

    /// Worth offering: allowed, perhaps after a password.
    pub fn offered(self) -> bool {
        matches!(self, Permission::Yes | Permission::Challenge)
    }
}

/// The actions to offer, in [`PowerAction::ALL`]'s order: those the system
/// allows, perhaps after asking for a password. An action it didn't answer
/// for isn't offered.
pub fn offered(permissions: &[(PowerAction, Permission)]) -> Vec<PowerAction> {
    PowerAction::ALL
        .into_iter()
        .filter(|action| permissions.iter().any(|(a, p)| a == action && p.offered()))
        .collect()
}

/// The AppleScript that takes `action` on macOS.
// Used on macOS; the table is tested everywhere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn apple_script(action: PowerAction) -> &'static str {
    match action {
        PowerAction::PowerOff => "tell application \"System Events\" to shut down",
        PowerAction::Restart => "tell application \"System Events\" to restart",
        PowerAction::Sleep => "tell application \"System Events\" to sleep",
        PowerAction::LogOut => "tell application \"System Events\" to log out",
    }
}

/// The system's side: what it allows, and doing it.
pub trait PowerSystem: Send + Sync {
    /// What the system allows now.
    fn permissions(&self) -> BoxFuture<'_, Vec<(PowerAction, Permission)>>;
    /// Take `action`. Powering off may return before the system goes down.
    fn perform(&self, action: PowerAction) -> BoxFuture<'_, Result<()>>;
}

/// This computer.
pub(crate) struct ThisSystem;

impl PowerSystem for ThisSystem {
    fn permissions(&self) -> BoxFuture<'_, Vec<(PowerAction, Permission)>> {
        Box::pin(platform::permissions())
    }

    fn perform(&self, action: PowerAction) -> BoxFuture<'_, Result<()>> {
        Box::pin(platform::perform(action))
    }
}

/// How long running games get to end their sessions (stop, record, upload
/// saves, which alone may take [`crate::SAVE_SYNC_TIMEOUT`]) before the
/// action is given up.
pub const SESSIONS_END_TIMEOUT: Duration = Duration::from_secs(300);

/// A game session from its start until everything after it (saves, mods,
/// [`irori_core::Event::GameExited`]) is done. Powering off waits until none
/// is open.
pub(crate) struct OpenSession(Backend);

impl OpenSession {
    pub(crate) fn new(backend: &Backend) -> Self {
        backend.open_sessions.send_modify(|n| *n += 1);
        Self(backend.clone())
    }
}

impl Drop for OpenSession {
    fn drop(&mut self) {
        self.0
            .open_sessions
            .send_modify(|n| *n = n.saturating_sub(1));
    }
}

/// Wait until no session is open, then take `action`; give up after
/// `timeout` without taking it.
async fn after_sessions(
    mut sessions: watch::Receiver<usize>,
    timeout: Duration,
    system: &dyn PowerSystem,
    action: PowerAction,
) -> Result<()> {
    let ended = async move {
        // A closed channel means the backend is going away with its sessions.
        let _ = sessions.wait_for(|open| *open == 0).await;
    };
    match tokio::time::timeout(timeout, ended).await {
        Ok(()) => system.perform(action).await,
        Err(_) => bail!(
            "A game is still closing, so the computer was left on. Try again once it has closed."
        ),
    }
}

impl Backend {
    /// Act on another system than this computer: a fake one in tests, so
    /// nothing powers off.
    pub fn set_power_system(&self, system: std::sync::Arc<dyn PowerSystem>) {
        *self.power_system.write() = system;
    }

    fn power_system(&self) -> std::sync::Arc<dyn PowerSystem> {
        self.power_system.read().clone()
    }

    /// The actions this computer allows now, in the order to offer them.
    pub async fn power_actions(&self) -> Vec<PowerAction> {
        offered(&self.power_system().permissions().await)
    }

    /// Stop every running game, wait until their sessions have ended in full
    /// (recorded, saves uploaded), then take `action`.
    pub async fn power(&self, action: PowerAction) -> Result<()> {
        self.power_with(&*self.power_system(), action, SESSIONS_END_TIMEOUT)
            .await
    }

    pub(crate) async fn power_with(
        &self,
        system: &dyn PowerSystem,
        action: PowerAction,
        timeout: Duration,
    ) -> Result<()> {
        let sessions = self.open_sessions.subscribe();
        let games = self.running_games();
        for result in futures::future::join_all(games.iter().map(|id| self.stop(id))).await {
            if let Err(e) = result {
                tracing::warn!("stopping a game before {action:?}: {e:#}");
            }
        }
        after_sessions(sessions, timeout, system, action).await
    }

    /// How many game sessions are open (running, or still ending).
    #[cfg(test)]
    pub(crate) fn open_session_count(&self) -> usize {
        *self.open_sessions.borrow()
    }
}

#[cfg(test)]
mod tests;
