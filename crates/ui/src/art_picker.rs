//! Opening the library's SteamGridDB art picker from screens that don't
//! depend on the library crate (a game's settings). The shell installs the
//! opener at startup.

use std::rc::Rc;

use gpui_kit::{App, Global, Window};

/// Which artwork to pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtSlot {
    /// Vertical 2:3 library cover.
    Cover,
    /// Wide background behind the game page.
    Background,
    /// The game's name as artwork, usually transparent.
    Logo,
}

/// Called with the full-size image URL the user picked.
pub type OnPicked = Rc<dyn Fn(String, &mut Window, &mut App)>;

type Opener = Rc<dyn Fn(ArtSlot, &str, OnPicked, &mut Window, &mut App)>;

struct ArtPickerHook(Opener);

impl Global for ArtPickerHook {}

/// Install the opener (the shell, with `irori_library`'s picker).
pub fn install(
    open: impl Fn(ArtSlot, &str, OnPicked, &mut Window, &mut App) + 'static,
    cx: &mut App,
) {
    cx.set_global(ArtPickerHook(Rc::new(open)));
}

/// Whether a picker is installed.
pub fn available(cx: &App) -> bool {
    cx.has_global::<ArtPickerHook>()
}

/// Search SteamGridDB for `title` and call `on_picked` with the chosen
/// image. Does nothing without an installed picker.
pub fn open(slot: ArtSlot, title: &str, on_picked: OnPicked, window: &mut Window, cx: &mut App) {
    let Some(open) = cx.try_global::<ArtPickerHook>().map(|h| h.0.clone()) else {
        return;
    };
    open(slot, title, on_picked, window, cx);
}
