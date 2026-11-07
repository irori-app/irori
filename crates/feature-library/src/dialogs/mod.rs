//! Dialogs owned by the library: categories, adding or editing a manually
//! added app or a whole folder of games, editing a game's title, art and
//! game modes,
//! installing with a Lutris script, and saving and managing library views.

pub mod categories;
pub mod edit_game;
pub mod folder;
pub mod lutris;
pub mod sideload;
pub mod steamgrid;
pub mod views;

use std::collections::BTreeSet;
use std::future::Future;
use std::path::PathBuf;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::h_flex;
use gpui_kit::component::input::InputState;
use gpui_kit::{
    App, Context, Entity, IntoElement, ParentElement as _, PathPromptOptions, Pixels, SharedString,
    Styled as _, Window,
};
use irori_core::GameMode;

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

/// A tick box per game mode, showing `ticked` and changing the set
/// `modes` picks out of the dialog.
pub(crate) fn mode_boxes<T: 'static>(
    ticked: &BTreeSet<GameMode>,
    modes: fn(&mut T) -> &mut BTreeSet<GameMode>,
    cx: &mut Context<T>,
) -> impl IntoElement + use<T> {
    h_flex()
        .flex_wrap()
        .gap_x_4()
        .gap_y_2()
        .children(GameMode::ALL.into_iter().map(|mode| {
            Checkbox::new(SharedString::from(format!("game-mode-{}", mode.id())))
                .label(irori_ui::game_mode::label(mode))
                .checked(ticked.contains(&mode))
                .on_change(cx.listener(move |this, on: &bool, _, cx| {
                    let set = modes(this);
                    if *on {
                        set.insert(mode);
                    } else {
                        set.remove(&mode);
                    }
                    cx.notify();
                }))
        }))
}

/// The game modes to keep as the user's own: the ticked ones when they
/// differ from the `known` ones (what stores and PCGamingWiki say), so a
/// store that learns more still counts while the user changed nothing.
pub(crate) fn chosen_modes(
    known: &[GameMode],
    ticked: &BTreeSet<GameMode>,
) -> Option<Vec<GameMode>> {
    (!ticked.iter().eq(known.iter())).then(|| ticked.iter().copied().collect())
}
