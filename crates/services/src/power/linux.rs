//! Linux: logind on the system bus for powering off, restarting and sleep;
//! the desktop's session manager on the session bus for logging out, else
//! logind's own session.

use super::*;
use anyhow::Context as _;
use ashpd::zbus::{self, Connection, Proxy};

const LOGIND: &str = "org.freedesktop.login1";
const LOGIND_PATH: &str = "/org/freedesktop/login1";
const LOGIND_MANAGER: &str = "org.freedesktop.login1.Manager";
/// The session the caller runs in, as logind resolves it.
const OWN_SESSION: &str = "/org/freedesktop/login1/session/auto";

/// A desktop's session manager, which logs out the way its own menu does:
/// programs are asked to save and quit first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SessionManager {
    pub name: &'static str,
    pub path: &'static str,
    pub interface: &'static str,
    pub method: &'static str,
    pub kind: LogoutArgs,
}

/// The arguments a session manager's logout method takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LogoutArgs {
    /// GNOME's `Logout(u mode)`: 1 logs out without its own question
    /// (Irori asked already).
    GnomeMode,
    /// Plasma 6's `logout()`, which asks nothing.
    None,
    /// Plasma 5's `logout(i confirm, i type, i mode)`, all 0: no question.
    KsmServer,
    /// Xfce's `Logout(b show_dialog, b allow_save)`.
    Xfce,
}

/// The session managers asked, in order. MATE's and Cinnamon's own session
/// managers answer as `org.gnome.SessionManager` too.
pub(super) const SESSION_MANAGERS: [SessionManager; 4] = [
    SessionManager {
        name: "org.gnome.SessionManager",
        path: "/org/gnome/SessionManager",
        interface: "org.gnome.SessionManager",
        method: "Logout",
        kind: LogoutArgs::GnomeMode,
    },
    SessionManager {
        name: "org.kde.Shutdown",
        path: "/Shutdown",
        interface: "org.kde.Shutdown",
        method: "logout",
        kind: LogoutArgs::None,
    },
    SessionManager {
        name: "org.kde.ksmserver",
        path: "/KSMServer",
        interface: "org.kde.KSMServerInterface",
        method: "logout",
        kind: LogoutArgs::KsmServer,
    },
    SessionManager {
        name: "org.xfce.SessionManager",
        path: "/org/xfce/SessionManager",
        interface: "org.xfce.Session.Manager",
        method: "Logout",
        kind: LogoutArgs::Xfce,
    },
];

/// How to log out, given which bus names have an owner: the first session
/// manager running, else ending logind's session when there is one.
pub(super) fn logout_way(running: &[&str], logind_session: bool) -> Option<LogoutWay> {
    SESSION_MANAGERS
        .iter()
        .find(|m| running.contains(&m.name))
        .map(|m| LogoutWay::SessionManager(*m))
        .or(logind_session.then_some(LogoutWay::EndSession))
}

/// How Irori logs out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LogoutWay {
    SessionManager(SessionManager),
    /// logind's `Session.Terminate` on the caller's own session, which needs
    /// no permission. Programs get no chance to save, so it comes last.
    EndSession,
}

/// logind's method and question for an action other than logging out.
pub(super) fn logind_methods(action: PowerAction) -> Option<(&'static str, &'static str)> {
    match action {
        PowerAction::PowerOff => Some(("CanPowerOff", "PowerOff")),
        PowerAction::Restart => Some(("CanReboot", "Reboot")),
        PowerAction::Sleep => Some(("CanSuspend", "Suspend")),
        PowerAction::LogOut => None,
    }
}

async fn manager(system: &Connection) -> zbus::Result<Proxy<'static>> {
    Proxy::new(system, LOGIND, LOGIND_PATH, LOGIND_MANAGER).await
}

/// Whether logind knows the session Irori runs in, and it is a desktop one
/// (not a remote shell's).
async fn has_logind_session(system: &Connection) -> bool {
    let Ok(session) = Proxy::new(
        system,
        LOGIND,
        OWN_SESSION,
        "org.freedesktop.login1.Session",
    )
    .await
    else {
        return false;
    };
    match session.get_property::<String>("Type").await {
        Ok(kind) => matches!(kind.as_str(), "x11" | "wayland" | "mir"),
        Err(e) => {
            tracing::debug!("reading logind's session: {e}");
            false
        }
    }
}

/// The session managers running on the session bus.
async fn running_session_managers() -> Vec<&'static str> {
    let Ok(session) = Connection::session().await else {
        return Vec::new();
    };
    let Ok(dbus) = zbus::fdo::DBusProxy::new(&session).await else {
        return Vec::new();
    };
    let mut running = Vec::new();
    for manager in SESSION_MANAGERS {
        let Ok(name) = zbus::names::BusName::try_from(manager.name) else {
            continue;
        };
        if dbus.name_has_owner(name).await.unwrap_or(false) {
            running.push(manager.name);
        }
    }
    running
}

async fn current_logout_way(system: Option<&Connection>) -> Option<LogoutWay> {
    let running = running_session_managers().await;
    let logind = match system {
        Some(system) if running.is_empty() => has_logind_session(system).await,
        _ => false,
    };
    logout_way(&running, logind)
}

pub(super) async fn permissions() -> Vec<(PowerAction, Permission)> {
    let system = match Connection::system().await {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::debug!("connecting to the system bus: {e}");
            None
        }
    };
    let mut out = Vec::new();
    if let Some(system) = &system
        && let Ok(manager) = manager(system).await
    {
        for action in PowerAction::ALL {
            let Some((question, _)) = logind_methods(action) else {
                continue;
            };
            let permission = match manager.call::<_, _, String>(question, &()).await {
                Ok(answer) => Permission::from_logind(&answer),
                Err(e) => {
                    tracing::debug!("asking logind {question}: {e}");
                    Permission::Unavailable
                }
            };
            out.push((action, permission));
        }
    }
    let logout = if current_logout_way(system.as_ref()).await.is_some() {
        Permission::Yes
    } else {
        Permission::Unavailable
    };
    out.push((PowerAction::LogOut, logout));
    out
}

pub(super) async fn perform(action: PowerAction) -> Result<()> {
    if let Some((_, method)) = logind_methods(action) {
        let system = Connection::system()
            .await
            .context("Couldn't reach the system's login manager (logind)")?;
        let manager = manager(&system)
            .await
            .context("Couldn't reach the system's login manager (logind)")?;
        // Interactive, so polkit may ask for a password where it wants one.
        manager
            .call::<_, _, ()>(method, &(true,))
            .await
            .with_context(|| format!("The system refused to {}", verb(action)))?;
        return Ok(());
    }
    let system = Connection::system().await.ok();
    match current_logout_way(system.as_ref()).await {
        Some(LogoutWay::SessionManager(m)) => {
            let session = Connection::session()
                .await
                .context("Couldn't reach the desktop session")?;
            let proxy = Proxy::new(&session, m.name, m.path, m.interface)
                .await
                .context("Couldn't reach the desktop's session manager")?;
            let called = match m.kind {
                LogoutArgs::GnomeMode => proxy.call::<_, _, ()>(m.method, &(1u32,)).await,
                LogoutArgs::None => proxy.call::<_, _, ()>(m.method, &()).await,
                LogoutArgs::KsmServer => {
                    proxy.call::<_, _, ()>(m.method, &(0i32, 0i32, 0i32)).await
                }
                LogoutArgs::Xfce => proxy.call::<_, _, ()>(m.method, &(false, true)).await,
            };
            called.context("The desktop refused to log out")
        }
        Some(LogoutWay::EndSession) => {
            let system = system.context("Couldn't reach the system's login manager (logind)")?;
            let session = Proxy::new(
                &system,
                LOGIND,
                OWN_SESSION,
                "org.freedesktop.login1.Session",
            )
            .await
            .context("Couldn't reach the system's login manager (logind)")?;
            session
                .call::<_, _, ()>("Terminate", &())
                .await
                .context("The system refused to end the session")
        }
        None => bail!("No desktop session to log out of was found"),
    }
}

fn verb(action: PowerAction) -> &'static str {
    match action {
        PowerAction::PowerOff => "power off",
        PowerAction::Restart => "restart",
        PowerAction::Sleep => "sleep",
        PowerAction::LogOut => "log out",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct KWinWindowCandidate {
    pub match_id: String,
    pub pid: Option<u32>,
    pub resource_class: String,
    pub resource_name: String,
    pub desktop_file: String,
}

pub(super) fn is_irori_window_id(value: &str) -> bool {
    let v = value.trim();
    v.eq_ignore_ascii_case("irori") || v.eq_ignore_ascii_case("app.irori.irori")
}

pub(super) fn pick_kwin_window_match(
    candidates: &[KWinWindowCandidate],
    self_pid: u32,
) -> Option<&str> {
    let activate_candidates = || candidates.iter().filter(|c| c.match_id.starts_with("0_"));
    if let Some(exact) = activate_candidates().find(|c| c.pid == Some(self_pid)) {
        return Some(exact.match_id.as_str());
    }
    activate_candidates()
        .find(|c| {
            is_irori_window_id(&c.resource_class)
                || is_irori_window_id(&c.resource_name)
                || is_irori_window_id(&c.desktop_file)
        })
        .map(|c| c.match_id.as_str())
}

static KWIN_MATCH_ID: parking_lot::Mutex<Option<String>> = parking_lot::Mutex::new(None);

pub(super) async fn activate_window() {
    if let Err(e) = activate_kwin_window(std::process::id()).await {
        tracing::debug!("activating window through KWin: {e}");
    }
}

async fn activate_kwin_window(self_pid: u32) -> Result<()> {
    use std::collections::HashMap;
    use zbus::zvariant::OwnedValue;

    type RunnerMatch = (
        String,
        String,
        String,
        i32,
        f64,
        HashMap<String, OwnedValue>,
    );

    let session = Connection::session().await?;
    let runner = Proxy::new(
        &session,
        "org.kde.KWin",
        "/WindowsRunner",
        "org.kde.krunner1",
    )
    .await?;
    let kwin = Proxy::new(&session, "org.kde.KWin", "/KWin", "org.kde.KWin").await?;
    let cached = KWIN_MATCH_ID.lock().clone();
    if let Some(cached) = cached {
        if let Some(uuid) = cached.strip_prefix("0_")
            && kwin
                .call::<_, _, HashMap<String, OwnedValue>>("getWindowInfo", &(uuid,))
                .await
                .is_ok_and(|info| !info.is_empty())
        {
            runner
                .call::<_, _, ()>("Run", &(cached.as_str(), ""))
                .await?;
            return Ok(());
        }
        *KWIN_MATCH_ID.lock() = None;
    }
    let matches: Vec<RunnerMatch> = runner.call("Match", &("irori",)).await?;
    if matches.is_empty() {
        return Ok(());
    }
    let mut candidates = Vec::new();
    for (match_id, ..) in matches {
        let Some(uuid) = match_id.strip_prefix("0_") else {
            continue;
        };
        let Ok(info) = kwin
            .call::<_, _, HashMap<String, OwnedValue>>("getWindowInfo", &(uuid,))
            .await
        else {
            continue;
        };
        let pid = info.get("pid").and_then(|v| {
            u32::try_from(v)
                .ok()
                .or_else(|| i32::try_from(v).ok().and_then(|p| u32::try_from(p).ok()))
        });
        let str_field = |key: &str| {
            info.get(key)
                .and_then(|v| <&str>::try_from(v).ok())
                .unwrap_or_default()
                .to_string()
        };
        candidates.push(KWinWindowCandidate {
            match_id,
            pid,
            resource_class: str_field("resourceClass"),
            resource_name: str_field("resourceName"),
            desktop_file: str_field("desktopFile"),
        });
    }
    if let Some(match_id) = pick_kwin_window_match(&candidates, self_pid) {
        *KWIN_MATCH_ID.lock() = Some(match_id.to_string());
        runner.call::<_, _, ()>("Run", &(match_id, "")).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_out_through_the_desktop_first() {
        let way = logout_way(
            &["org.xfce.SessionManager", "org.gnome.SessionManager"],
            true,
        );
        assert_eq!(
            way.map(|w| match w {
                LogoutWay::SessionManager(m) => m.name,
                LogoutWay::EndSession => "logind",
            }),
            Some("org.gnome.SessionManager"),
            "the first in order, whichever answered first"
        );
        assert!(matches!(
            logout_way(&["org.kde.Shutdown"], false),
            Some(LogoutWay::SessionManager(m)) if m.kind == LogoutArgs::None
        ));
        assert_eq!(logout_way(&[], true), Some(LogoutWay::EndSession));
        assert_eq!(logout_way(&[], false), None, "nothing to log out of");
        assert_eq!(logout_way(&["org.example.Other"], false), None);
    }

    #[test]
    fn asks_logind_the_matching_question() {
        assert_eq!(
            logind_methods(PowerAction::PowerOff),
            Some(("CanPowerOff", "PowerOff"))
        );
        assert_eq!(
            logind_methods(PowerAction::Restart),
            Some(("CanReboot", "Reboot"))
        );
        assert_eq!(
            logind_methods(PowerAction::Sleep),
            Some(("CanSuspend", "Suspend"))
        );
        assert_eq!(logind_methods(PowerAction::LogOut), None);
    }

    #[test]
    fn picks_own_kwin_window_and_ignores_unrelated_title_matches() {
        let candidate =
            |match_id: &str, pid: Option<u32>, class: &str, desktop: &str| KWinWindowCandidate {
                match_id: match_id.into(),
                pid,
                resource_class: class.into(),
                resource_name: class.into(),
                desktop_file: desktop.into(),
            };

        let mixed = vec![
            candidate("1_non_activate", Some(100), "irori", "irori"),
            candidate("0_editor", Some(200), "kwrite", "org.kde.kwrite"),
            candidate("0_other_irori", Some(300), "irori", "irori"),
            candidate("0_own_irori", Some(400), "irori", "irori"),
        ];
        assert_eq!(
            pick_kwin_window_match(&mixed, 400),
            Some("0_own_irori"),
            "prefers the exact PID over another instance and unrelated title matches"
        );

        let flatpak_or_pid_ns = vec![
            candidate("0_editor", Some(200), "kwrite", "org.kde.kwrite"),
            candidate(
                "0_flatpak",
                Some(9999),
                "app.irori.Irori",
                "app.irori.Irori",
            ),
        ];
        assert_eq!(
            pick_kwin_window_match(&flatpak_or_pid_ns, 400),
            Some("0_flatpak"),
            "falls back to Irori resourceClass/desktopFile when PIDs differ (such as Flatpak)"
        );

        let only_unrelated = vec![candidate("0_editor", Some(200), "kwrite", "org.kde.kwrite")];
        assert_eq!(pick_kwin_window_match(&only_unrelated, 400), None);
    }
}
