//! The library with no games at all: what Irori found on this computer
//! (Steam, emulators and ROM folders, other launchers, stores to log in
//! to; drawn by the management screens through
//! `irori_ui::hooks::found_sources`), with Refresh, Add game and Add games
//! from a folder below. Without the hook (tests) it says the library is
//! empty and offers the same ways in.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Styled as _,
    Window, div,
};
use irori_ui::components::EmptyState;
use irori_ui::i18n::t;
use irori_ui::{Route, models, navigate};

use super::LibraryView;
use crate::dialogs;

fn add_game() -> Button {
    Button::new("empty-add")
        .outline()
        .icon(IconName::Plus)
        .label(t("library.header.add_game"))
        .on_click(|_, window, cx| dialogs::sideload::open_add_game_dialog(window, cx))
}

fn set_up_emulation() -> Button {
    Button::new("empty-emulation")
        .outline()
        .icon(IconName::Joystick)
        .label(t("library.empty.set_up_emulation"))
        .on_click(|_, window, cx| irori_ui::hooks::open_emulation_setup(window, cx))
}

impl LibraryView {
    /// The body of a library without games.
    pub(super) fn render_empty(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.found_sources.is_none() {
            self.found_sources = irori_ui::hooks::found_sources(window, cx);
        }
        if let Some(found) = self.found_sources.clone() {
            let actions = h_flex()
                .gap_2()
                .flex_wrap()
                .child(
                    Button::new("empty-refresh")
                        .icon(IconName::RefreshCw)
                        .label(t("library.header.refresh"))
                        .on_click(|_, _, cx| irori_ui::models::refresh_library(false, cx)),
                )
                .child(add_game())
                .child(
                    Button::new("empty-folder")
                        .outline()
                        .icon(IconName::FolderPlus)
                        .label(t("library.folder.title"))
                        .on_click(|_, window, cx| {
                            dialogs::folder::open_add_folder_dialog(window, cx)
                        }),
                );
            return div()
                .relative()
                .size_full()
                .child(
                    div()
                        .id("library-empty")
                        .size_full()
                        .overflow_y_scrollbar()
                        .child(
                            v_flex()
                                .w_full()
                                .items_center()
                                .px_6()
                                .py_10()
                                .gap_4()
                                .child(found)
                                .child(actions),
                        ),
                )
                .child(irori_ui::tour::mark(irori_ui::tour::anchors::LIBRARY_GAMES))
                .into_any_element();
        }
        let state = if models(cx).accounts.read(cx).any_logged_in() {
            EmptyState::new(IconName::Gamepad2, t("library.empty.title"))
                .description(t("library.empty.description"))
                .action(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("empty-refresh")
                                .icon(IconName::RefreshCw)
                                .label(t("library.header.refresh"))
                                .on_click(|_, _, cx| irori_ui::models::refresh_library(false, cx)),
                        )
                        .child(add_game())
                        .child(set_up_emulation()),
                )
        } else {
            EmptyState::new(IconName::LogIn, t("library.empty.no_stores_title"))
                .description(t("library.empty.no_stores_description"))
                .action(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("empty-accounts")
                                .primary()
                                .icon(IconName::LogIn)
                                .label(t("library.empty.log_in"))
                                .on_click(|_, _, cx| navigate(Route::Accounts, cx)),
                        )
                        .child(add_game())
                        .child(set_up_emulation()),
                )
        };
        div()
            .relative()
            .size_full()
            .child(state)
            .child(irori_ui::tour::mark(irori_ui::tour::anchors::LIBRARY_GAMES))
            .into_any_element()
    }
}
