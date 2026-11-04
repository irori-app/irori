//! A keyboard shortcut that saves a clip while the game has the keyboard.
//! Only the desktop can see keys pressed in another program's window, so
//! the shortcut is asked of it through the desktop portal's GlobalShortcuts
//! interface (KDE Plasma 6, GNOME 48 and Hyprland have one). The desktop
//! lets the user choose the keys, and change them later in its own shortcut
//! settings; Irori suggests Alt+F10. Where the portal is missing, the state
//! says so and the settings tell the user plainly.

use parking_lot::Mutex;

use crate::Backend;

/// Whether the clip shortcut works.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ShortcutState {
    /// Not asked for yet (the replay buffer was never on).
    #[default]
    Off,
    /// The desktop holds it; the keys as the desktop describes them, empty
    /// when it gave no description.
    Bound(String),
    /// The desktop has the shortcut but no keys are assigned to it.
    Unassigned,
    /// The desktop offers no global shortcuts.
    Unavailable,
}

/// The shortcut's state and the task that listens for it.
#[derive(Default)]
pub(crate) struct Shortcut {
    state: Mutex<ShortcutState>,
    listening: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// Id of the shortcut within Irori's portal session.
#[cfg(target_os = "linux")]
const SHORTCUT_ID: &str = "save-clip";
/// The keys suggested to the desktop, in the XDG shortcuts notation.
#[cfg(target_os = "linux")]
const PREFERRED_TRIGGER: &str = "ALT+F10";

impl Backend {
    /// Whether the keyboard shortcut that saves a clip works.
    pub fn clip_shortcut(&self) -> ShortcutState {
        self.replay.shortcut.state.lock().clone()
    }

    /// Ask the desktop for the clip shortcut, once per run, and save a clip
    /// each time it is pressed. Returns the resulting state; the first time,
    /// the desktop may first ask the user which keys to use.
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

    #[cfg(target_os = "linux")]
    async fn shortcut_session(&self, ready: tokio::sync::oneshot::Sender<ShortcutState>) {
        use ashpd::desktop::global_shortcuts::NewShortcut;
        use futures::StreamExt as _;

        let bound = async {
            let (portal, session) = connect().await?;
            let shortcut = NewShortcut::new(SHORTCUT_ID, "Save a clip of the last moments of play")
                .preferred_trigger(PREFERRED_TRIGGER);
            let response = portal
                .bind_shortcuts(&session, &[shortcut], None, Default::default())
                .await?
                .response()?;
            let state = response
                .shortcuts()
                .iter()
                .find(|s| s.id() == SHORTCUT_ID)
                .map_or(ShortcutState::Unassigned, |s| {
                    ShortcutState::Bound(s.trigger_description().to_owned())
                });
            anyhow::Ok((portal, session, state))
        };
        let (portal, session, state) = match bound.await {
            Ok(bound) => bound,
            Err(e) => {
                tracing::info!("no global shortcut for clips: {e:#}");
                let _ = ready.send(ShortcutState::Unavailable);
                return;
            }
        };
        let (Ok(mut activated), Ok(mut changed)) = (
            portal.receive_activated().await,
            portal.receive_shortcuts_changed().await,
        ) else {
            let _ = ready.send(ShortcutState::Unavailable);
            return;
        };
        let _ = ready.send(state);
        loop {
            tokio::select! {
                Some(event) = activated.next() => {
                    if event.shortcut_id() == SHORTCUT_ID && self.replay().is_some() {
                        self.save_clip();
                    }
                }
                Some(event) = changed.next() => {
                    let state = event
                        .shortcuts()
                        .iter()
                        .find(|s| s.id() == SHORTCUT_ID)
                        .map_or(ShortcutState::Unassigned, |s| {
                            ShortcutState::Bound(s.trigger_description().to_owned())
                        });
                    *self.replay.shortcut.state.lock() = state;
                }
                else => break,
            }
        }
        // Keep the session open for as long as Irori listens.
        drop(session);
    }

    #[cfg(not(target_os = "linux"))]
    async fn shortcut_session(&self, ready: tokio::sync::oneshot::Sender<ShortcutState>) {
        let _ = ready.send(ShortcutState::Unavailable);
    }
}

/// A portal session on a connection of its own, registered as Irori
/// (`app.irori.Irori`) so the desktop names the shortcut after it and
/// remembers its keys. The registration must come first on a connection,
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
