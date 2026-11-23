//! The overlay's keyboard shortcut, Ctrl+Shift+O (⌃⇧O on a Mac), on the
//! systems that let a program take one for the whole desktop without extra
//! permissions ([`super::support`]): `RegisterHotKey` on Windows,
//! `RegisterEventHotKey` on macOS (which, unlike watching every key press,
//! needs no Accessibility permission). It is the way in where the Guide
//! button doesn't reach Irori (Xbox controllers on Windows, whose Guide
//! button Windows keeps for the Xbox Game Bar).
//!
//! The shortcut is registered only while the overlay is on and a game Irori
//! started runs, so other programs keep it the rest of the time. A press
//! opens the overlay, or closes it when it is open.
//!
//! The same registration gives the shell the quick launcher's Ctrl+Alt+K
//! (⌃⌥K) on these systems ([`register_quick_launch`]); each key has an id of
//! its own, which a press is told apart by.

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod platform;
#[cfg(windows)]
#[path = "windows.rs"]
mod platform;

use futures::StreamExt as _;
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::{App, Global};

use super::support::Support;

/// One global shortcut: its id among Irori's and its keys on each system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Key {
    pub(crate) id: u32,
    /// The key's virtual key code on Windows (`VK_*`, letters are ASCII).
    #[cfg_attr(not(windows), allow(dead_code))] // Read on Windows only.
    pub(crate) windows_key: u32,
    /// The key's position on a Mac (`kVK_ANSI_*`): other layouts press the
    /// key where it is on a US keyboard.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))] // Read on macOS only.
    pub(crate) mac_key: u32,
    /// Control and Shift (the overlay) or Control and Alt (the launcher).
    pub(crate) alt: bool,
}

/// The overlay's Ctrl+Shift+O.
const OVERLAY: Key = Key {
    id: 1,
    windows_key: super::support::SHORTCUT_KEY as u32,
    mac_key: 0x1F,
    alt: false,
};

/// The quick launcher's Ctrl+Alt+K: K as in the palette's Ctrl+K.
const QUICK_LAUNCH: Key = Key {
    id: 2,
    windows_key: 'K' as u32,
    mac_key: 0x28,
    alt: true,
};

/// The quick launcher's keys as the settings name them.
pub const QUICK_LAUNCH_LABEL: &str = if cfg!(target_os = "macos") {
    "⌃⌥K"
} else {
    "Ctrl+Alt+K"
};

/// A global shortcut the shell holds; dropping it gives the keys back.
pub struct GlobalShortcut(#[allow(dead_code)] platform::Registration); // Held for its drop.

/// Take Ctrl+Alt+K (⌃⌥K) for the whole desktop: each press sends to
/// `pressed`. Fails when another program has it. Call on the main thread.
pub fn register_quick_launch(pressed: UnboundedSender<()>) -> anyhow::Result<GlobalShortcut> {
    platform::register(QUICK_LAUNCH, pressed).map(GlobalShortcut)
}

#[derive(Default)]
struct HotkeyState {
    /// Dropping it gives the shortcut back.
    registration: Option<platform::Registration>,
    /// The last registration failed (another program has the shortcut);
    /// said once in the log, not on every status change.
    failed: bool,
}

impl Global for HotkeyState {}

/// Follow the overlay setting and the running games, registering the
/// shortcut while both call for it. Call once at startup.
pub(crate) fn install(cx: &mut App) {
    if !Support::current().shortcut || cx.has_global::<HotkeyState>() {
        return;
    }
    let Some(models) = cx.try_global::<irori_ui::Models>() else {
        return;
    };
    let (settings, statuses) = (models.settings.clone(), models.statuses.clone());
    cx.set_global(HotkeyState::default());
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
    cx.spawn(async move |cx| {
        while rx.next().await.is_some() {
            cx.update(toggle);
        }
    })
    .detach();
    {
        let tx = tx.clone();
        cx.observe(&settings, move |_, cx| sync(&tx, cx)).detach();
    }
    {
        let tx = tx.clone();
        cx.observe(&statuses, move |_, cx| sync(&tx, cx)).detach();
    }
    sync(&tx, cx);
}

/// Register or give back the shortcut as the overlay is wanted or not.
fn sync(tx: &UnboundedSender<()>, cx: &mut App) {
    let wanted = super::wanted(cx) || super::is_open(cx);
    let state = cx.default_global::<HotkeyState>();
    match (wanted, state.registration.is_some()) {
        (true, false) => match platform::register(OVERLAY, tx.clone()) {
            Ok(registration) => {
                state.registration = Some(registration);
                state.failed = false;
            }
            Err(e) => {
                if !state.failed {
                    tracing::warn!("the in-game overlay's shortcut isn't available: {e:#}");
                }
                state.failed = true;
            }
        },
        (false, true) => state.registration = None,
        _ => {}
    }
}

/// The shortcut was pressed.
fn toggle(cx: &mut App) {
    if super::is_open(cx) {
        super::close(cx);
    } else if super::wanted(cx) {
        super::open(cx);
    }
}
