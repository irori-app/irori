//! Dialogs owned by the library: categories, adding or editing a manually
//! added app or a whole folder of games, editing a game's title and art,
//! installing with a Lutris script, and saving and managing library views.

pub mod categories;
pub mod edit_game;
pub mod folder;
pub mod lutris;
pub mod sideload;
pub mod steamgrid;
pub mod views;

use std::future::Future;
use std::path::PathBuf;

use gpui_kit::component::input::InputState;
use gpui_kit::{App, Entity, PathPromptOptions, Pixels, SharedString, Window};

/// Ask the user for one file (or folder). Resolves to `None` when the picker
/// is dismissed or unavailable.
pub(crate) fn pick_path(
    directories: bool,
    prompt: SharedString,
    cx: &mut App,
) -> impl Future<Output = Option<PathBuf>> + use<> {
    let rx = cx.prompt_for_paths(PathPromptOptions {
        files: !directories,
        directories,
        multiple: false,
        prompt: Some(prompt),
    });
    async move { rx.await.ok()?.ok()??.into_iter().next() }
}

/// Trimmed text of an input.
pub(crate) fn text(input: &Entity<InputState>, cx: &App) -> String {
    input.read(cx).value().trim().to_string()
}

/// `Some(text)` for a non-empty input.
pub(crate) fn optional_text(input: &Entity<InputState>, cx: &App) -> Option<String> {
    Some(text(input, cx)).filter(|s| !s.is_empty())
}

/// Dialog width derived from the window's rem size, so dialogs zoom with the
/// interface.
pub(crate) fn dialog_width(window: &Window, rems: f32) -> Pixels {
    window.rem_size() * rems
}
