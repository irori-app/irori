//! Keyboard shortcuts that save a clip and take a screenshot while the game
//! has the keyboard, and the one that opens the quick launcher (the command
//! palette in a window of its own) from anywhere; one session asks for all
//! three. Only the desktop sees keys pressed in another program's
//! window, so Irori tries, in order ([`bindings::sources`]):
//!
//! 1. the desktop portal's GlobalShortcuts interface (KDE Plasma 6, GNOME 48,
//!    Hyprland). The desktop lets the user choose the keys and change them
//!    later in its own shortcut settings; Irori suggests Alt+F10 and Alt+F12;
//! 2. on an X11 session, grabbing Alt+F10 and Alt+F12 from the X server
//!    (see [`super::x11`]);
//! 3. otherwise (Sway and other wlroots compositors, niri, a grab another
//!    program holds), the commands `irori --save-clip` and
//!    `irori --screenshot`, which reach the running Irori through its
//!    single-instance channel: the settings show the exact lines to paste in
//!    the detected compositor's configuration.
//!
//! Keyboards are never read from `/dev/input`. macOS and Windows have none of
//! these; there the in-game overlay's hotkey and the commands work.

use std::future::Future;

use parking_lot::Mutex;

use super::bindings::{self, Binding, Program, Source};
use crate::Backend;

/// Whether the quick launcher's shortcut works, and how.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum QuickLaunchShortcut {
    /// Not asked for yet.
    #[default]
    Off,
    /// The desktop (its portal, or the X server Irori grabbed it from)
    /// holds it: its keys as described, `None` while the desktop has none
    /// for it.
    Keys(Option<String>),
    /// The user binds `irori --quick-launch` in their compositor, with
    /// these lines.
    Commands(Binding),
    /// No global shortcut from here (macOS and Windows register theirs in
    /// the interface).
    Unavailable,
}

/// Whether the clip and screenshot shortcuts work, and how.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ShortcutState {
    /// Not asked for yet (the replay buffer was never on).
    #[default]
    Off,
    /// The desktop portal holds them: each one's keys as the desktop
    /// describes them (empty when it gave no description), `None` while the
    /// desktop has no keys for it.
    Portal {
        clip: Option<String>,
        screenshot: Option<String>,
    },
    /// Irori holds these keys, grabbed from the X server.
    Grabbed { clip: String, screenshot: String },
    /// Nothing listens for keys here: the user binds Irori's commands in
    /// their compositor, with these lines.
    Commands(Binding),
    /// No keyboard shortcuts on this system.
    Unavailable,
}

/// The shortcut's state and the task that listens for it.
#[derive(Default)]
pub(crate) struct Shortcut {
    state: Mutex<ShortcutState>,
    quick: Mutex<QuickLaunchShortcut>,
    /// What a press of the quick launcher's shortcut runs.
    on_quick_launch: Mutex<Option<std::sync::Arc<dyn Fn() + Send + Sync>>>,
    listening: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// Ids of the shortcuts within Irori's portal session.
#[cfg(target_os = "linux")]
const CLIP_ID: &str = "save-clip";
#[cfg(target_os = "linux")]
const SCREENSHOT_ID: &str = "screenshot";
#[cfg(target_os = "linux")]
const QUICK_LAUNCH_ID: &str = "quick-launch";

/// What answered, and what to listen to.
enum Listener {
    #[cfg(target_os = "linux")]
    Portal(PortalSession, ShortcutState, QuickLaunchShortcut),
    #[cfg(target_os = "linux")]
    X11(
        tokio::sync::mpsc::UnboundedReceiver<super::x11::Action>,
        bool,
    ),
    Commands(Binding),
}

/// Try `sources` in order and keep the first that works, so a desktop
/// without the portal falls back to the next way.
async fn first_working<T, F, Fut>(sources: &[Source], mut attempt: F) -> Option<T>
where
    F: FnMut(Source) -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    for &source in sources {
        match attempt(source).await {
            Ok(found) => return Some(found),
            Err(e) => tracing::info!("no {source:?} shortcuts: {e:#}"),
        }
    }
    None
}

impl Backend {
    /// Whether the keyboard shortcuts that save a clip and take a screenshot
    /// work, and how.
    pub fn clip_shortcut(&self) -> ShortcutState {
        self.replay.shortcut.state.lock().clone()
    }

    /// Whether the quick launcher's shortcut works, and how.
    pub fn quick_launch_shortcut(&self) -> QuickLaunchShortcut {
        self.replay.shortcut.quick.lock().clone()
    }

    /// Say how the quick launcher's shortcut works where the interface
    /// registers it itself (macOS, Windows).
    pub fn set_quick_launch_shortcut(&self, state: QuickLaunchShortcut) {
        *self.replay.shortcut.quick.lock() = state;
    }

    /// Run `open` each time the quick launcher's shortcut is pressed.
    pub fn on_quick_launch_shortcut(&self, open: impl Fn() + Send + Sync + 'static) {
        *self.replay.shortcut.on_quick_launch.lock() = Some(std::sync::Arc::new(open));
    }

    /// The quick launcher's shortcut was pressed.
    #[cfg(target_os = "linux")]
    fn quick_launch_pressed(&self) {
        let open = self.replay.shortcut.on_quick_launch.lock().clone();
        if let Some(open) = open {
            open();
        }
    }

    /// Set up the clip and screenshot shortcuts, once per run, and act each
    /// time one is pressed. Returns the resulting state; the first time, the
    /// desktop may first ask the user which keys to use.
    pub async fn listen_for_clip_shortcut(&self) -> ShortcutState {
        let running = self
            .replay
            .shortcut
            .listening
            .lock()
            .as_ref()
            .is_some_and(|t| !t.is_finished());
        if running {
            return self.clip_shortcut();
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let b = self.clone();
        let task = self.spawn(async move { b.shortcut_session(tx).await });
        *self.replay.shortcut.listening.lock() = Some(task);
        let state = rx.await.unwrap_or(ShortcutState::Unavailable);
        *self.replay.shortcut.state.lock() = state.clone();
        state
    }

    /// Save a clip (when the replay buffer records) or take a screenshot of
    /// the game that runs.
    #[cfg(target_os = "linux")]
    fn shortcut_pressed(&self, screenshot: bool) {
        if screenshot {
            self.take_screenshot(self.running_games().into_iter().next());
        } else if self.replay().is_some() {
            self.save_clip();
        }
    }

    async fn shortcut_session(&self, ready: tokio::sync::oneshot::Sender<ShortcutState>) {
        let session = bindings::Session::detect(|name| std::env::var(name).ok());
        let sources = bindings::sources(cfg!(target_os = "linux"), &session);
        let found = first_working(&sources, |source| self.open_source(source, &session)).await;
        let commands = || {
            QuickLaunchShortcut::Commands(Binding::quick_launch(
                session.compositor,
                &Program::current(),
            ))
        };
        let quick = |state: QuickLaunchShortcut| {
            *self.replay.shortcut.quick.lock() = state;
        };
        match found {
            None => {
                quick(QuickLaunchShortcut::Unavailable);
                let _ = ready.send(ShortcutState::Unavailable);
            }
            Some(Listener::Commands(binding)) => {
                quick(commands());
                let _ = ready.send(ShortcutState::Commands(binding));
            }
            #[cfg(target_os = "linux")]
            Some(Listener::X11(mut keys, quick_grabbed)) => {
                quick(if quick_grabbed {
                    QuickLaunchShortcut::Keys(Some(bindings::QUICK_LAUNCH_KEYS.into()))
                } else {
                    commands()
                });
                let _ = ready.send(ShortcutState::Grabbed {
                    clip: bindings::CLIP_KEYS.into(),
                    screenshot: bindings::SCREENSHOT_KEYS.into(),
                });
                while let Some(action) = keys.recv().await {
                    match action {
                        super::x11::Action::Clip => self.shortcut_pressed(false),
                        super::x11::Action::Screenshot => self.shortcut_pressed(true),
                        super::x11::Action::QuickLaunch => self.quick_launch_pressed(),
                    }
                }
            }
            #[cfg(target_os = "linux")]
            Some(Listener::Portal(portal, state, launcher)) => {
                quick(launcher);
                let _ = ready.send(state);
                self.portal_loop(portal).await;
            }
        }
    }

    async fn open_source(
        &self,
        source: Source,
        session: &bindings::Session,
    ) -> anyhow::Result<Listener> {
        match source {
            Source::Commands => Ok(Listener::Commands(Binding::new(
                session.compositor,
                &Program::current(),
            ))),
            #[cfg(target_os = "linux")]
            Source::Portal => {
                let (portal, state, quick) = bind_portal().await?;
                Ok(Listener::Portal(portal, state, quick))
            }
            #[cfg(target_os = "linux")]
            Source::X11Grab => {
                let (keys, quick) = self.spawn_blocking(super::x11::grab).await??;
                Ok(Listener::X11(keys, quick))
            }
            #[cfg(not(target_os = "linux"))]
            Source::Portal | Source::X11Grab => anyhow::bail!("not on this system"),
        }
    }

    #[cfg(target_os = "linux")]
    async fn portal_loop(&self, portal: PortalSession) {
        use futures::StreamExt as _;

        let PortalSession { portal, session } = portal;
        let (Ok(mut activated), Ok(mut changed)) = (
            portal.receive_activated().await,
            portal.receive_shortcuts_changed().await,
        ) else {
            return;
        };
        loop {
            tokio::select! {
                Some(event) = activated.next() => match event.shortcut_id() {
                    CLIP_ID => self.shortcut_pressed(false),
                    SCREENSHOT_ID => self.shortcut_pressed(true),
                    QUICK_LAUNCH_ID => self.quick_launch_pressed(),
                    _ => {}
                },
                Some(event) = changed.next() => {
                    *self.replay.shortcut.state.lock() = portal_state(event.shortcuts());
                    *self.replay.shortcut.quick.lock() = portal_quick(event.shortcuts());
                }
                else => break,
            }
        }
        // Keep the session open for as long as Irori listens.
        drop(session);
    }
}

/// A portal session on a connection of its own.
#[cfg(target_os = "linux")]
struct PortalSession {
    portal: ashpd::desktop::global_shortcuts::GlobalShortcuts,
    session: ashpd::desktop::Session<ashpd::desktop::global_shortcuts::GlobalShortcuts>,
}

/// The state the portal's list of shortcuts describes.
#[cfg(target_os = "linux")]
fn portal_state(shortcuts: &[ashpd::desktop::global_shortcuts::Shortcut]) -> ShortcutState {
    let keys = |id: &str| {
        shortcuts
            .iter()
            .find(|s| s.id() == id)
            .map(|s| s.trigger_description().to_owned())
    };
    ShortcutState::Portal {
        clip: keys(CLIP_ID),
        screenshot: keys(SCREENSHOT_ID),
    }
}

/// The quick launcher's keys in the portal's list of shortcuts.
#[cfg(target_os = "linux")]
fn portal_quick(shortcuts: &[ashpd::desktop::global_shortcuts::Shortcut]) -> QuickLaunchShortcut {
    QuickLaunchShortcut::Keys(
        shortcuts
            .iter()
            .find(|s| s.id() == QUICK_LAUNCH_ID)
            .map(|s| s.trigger_description().to_owned()),
    )
}

/// Ask the portal for every shortcut.
#[cfg(target_os = "linux")]
async fn bind_portal() -> anyhow::Result<(PortalSession, ShortcutState, QuickLaunchShortcut)> {
    use ashpd::desktop::global_shortcuts::NewShortcut;

    let (portal, session) = connect().await?;
    let shortcuts = [
        NewShortcut::new(CLIP_ID, "Save a clip of the last moments of play")
            .preferred_trigger("ALT+F10"),
        NewShortcut::new(SCREENSHOT_ID, "Take a screenshot of the game")
            .preferred_trigger("ALT+F12"),
        NewShortcut::new(QUICK_LAUNCH_ID, "Open Irori's quick launcher")
            .preferred_trigger("CTRL+ALT+K"),
    ];
    let response = portal
        .bind_shortcuts(&session, &shortcuts, None, Default::default())
        .await?
        .response()?;
    let state = portal_state(response.shortcuts());
    let quick = portal_quick(response.shortcuts());
    Ok((PortalSession { portal, session }, state, quick))
}

/// A portal session on a connection of its own, registered as Irori
/// (`app.irori.Irori`) so the desktop names the shortcuts after it and
/// remembers their keys. The registration must come first on a connection,
/// and the shared one already served other portals.
#[cfg(target_os = "linux")]
async fn connect() -> anyhow::Result<(
    ashpd::desktop::global_shortcuts::GlobalShortcuts,
    ashpd::desktop::Session<ashpd::desktop::global_shortcuts::GlobalShortcuts>,
)> {
    use ashpd::desktop::global_shortcuts::GlobalShortcuts;
    let connection = ashpd::zbus::Connection::session().await?;
    if let Ok(id) = ashpd::AppID::try_from("app.irori.Irori")
        && let Err(e) = ashpd::register_host_app_with_connection(connection.clone(), id).await
    {
        // Older portals have no registry; the shortcut still works.
        tracing::debug!("registering with the desktop portal: {e}");
    }
    let portal = GlobalShortcuts::with_connection(connection).await?;
    let session = portal.create_session(Default::default()).await?;
    Ok((portal, session))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tries `sources`, where the ones in `working` succeed; returns what
    /// answered and every source tried.
    async fn run(sources: &[Source], working: &[Source]) -> (Option<Source>, Vec<Source>) {
        let mut tried = Vec::new();
        let found = first_working(sources, |source| {
            tried.push(source);
            let works = working.contains(&source);
            async move {
                if works {
                    Ok(source)
                } else {
                    anyhow::bail!("{source:?} isn't here")
                }
            }
        })
        .await;
        (found, tried)
    }

    #[tokio::test]
    async fn each_desktop_falls_back_to_the_next_way() {
        use Source::*;
        let x11 = [Portal, X11Grab, Commands];
        // KDE or GNOME: the portal, and nothing else is tried.
        assert_eq!(
            run(&x11, &[Portal, X11Grab, Commands]).await,
            (Some(Portal), vec![Portal])
        );
        // i3 or Xfce: no portal, Irori grabs the keys.
        assert_eq!(
            run(&x11, &[X11Grab, Commands]).await,
            (Some(X11Grab), vec![Portal, X11Grab])
        );
        // Another program holds the keys: the commands.
        assert_eq!(
            run(&x11, &[Commands]).await,
            (Some(Commands), vec![Portal, X11Grab, Commands])
        );
        // Sway: no portal and no grab on Wayland.
        assert_eq!(
            run(&[Portal, Commands], &[Commands]).await,
            (Some(Commands), vec![Portal, Commands])
        );
        // macOS and Windows: nothing.
        assert_eq!(run(&[], &[Commands]).await, (None, vec![]));
    }
}
