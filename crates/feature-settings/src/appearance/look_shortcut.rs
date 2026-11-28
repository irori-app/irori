//! One-click shortcut panel inside Settings → Appearance jumping straight to
//! Library → Look where layout, cover size, shape, corners, captions, badges,
//! greeting, and list columns are edited live on the library itself.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _, Window,
    div,
};
use irori_ui::i18n::t;

/// Shortcut card pointing users to the live Library → Look popover.
pub(crate) struct LibraryLookShortcut;

impl LibraryLookShortcut {
    pub(crate) fn new(_cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl Render for LibraryLookShortcut {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        h_flex()
            .w_full()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_2p5()
                    .child(Icon::new(IconName::Palette).text_color(muted))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .child(t("settings.appearance.look_shortcut_title")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(t("settings.appearance.look_shortcut_help")),
                            ),
                    ),
            )
            .child(
                Button::new("open-library-look")
                    .debug_selector(|| "open-library-look".into())
                    .primary()
                    .small()
                    .icon(IconName::Palette)
                    .label(t("settings.appearance.open_look"))
                    .on_click(|_, window, cx| irori_ui::hooks::open_library_look(window, cx)),
            )
    }
}
