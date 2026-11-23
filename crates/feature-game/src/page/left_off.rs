//! "Where I left off" on the game page: a short note under the status, with
//! buttons to change or clear it (clearing offers Undo), or a quiet button
//! to write one. The note itself lives in the user data
//! (`UserData::left_off`) and travels with backups and sync.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, IntoElement, ParentElement as _, SharedString, Styled as _, div,
};
use irori_core::GameInfo;
use irori_ui::i18n::t;
use irori_ui::models;

use super::GamePage;

impl GamePage {
    pub(super) fn render_left_off(
        &self,
        game: &Arc<GameInfo>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let note = models(cx)
            .library
            .read(cx)
            .user
            .left_off
            .get(&self.id.key())
            .map(|l| SharedString::from(l.text.clone()));
        let (id, title) = (self.id.clone(), game.title.clone());
        let edit = move |label: Option<SharedString>| {
            let (id, title) = (id.clone(), title.clone());
            let button = Button::new("left-off-edit")
                .ghost()
                .small()
                .icon(IconName::Bookmark)
                .on_click(move |_, window, cx| {
                    irori_ui::journal::open_left_off_editor(id.clone(), &title, window, cx)
                });
            match label {
                Some(label) => button.label(label),
                None => button
                    .tooltip(t("game.left_off.edit"))
                    .accessibility_label(t("game.left_off.edit")),
            }
        };
        let Some(note) = note else {
            return div()
                .child(edit(Some(t("game.left_off.write"))))
                .into_any_element();
        };
        let clear_id = self.id.clone();
        let ink = self.ink;
        let theme = cx.theme();
        v_flex()
            .w_full()
            .gap_1()
            .p_3()
            .rounded(theme.radius_lg)
            .bg(ink.panel)
            .border_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .items_center()
                    .child(
                        Icon::new(IconName::Bookmark)
                            .xsmall()
                            .text_color(ink.panel_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .font_semibold()
                            .text_color(ink.panel_muted)
                            .child(t("game.left_off.title")),
                    )
                    .child(edit(None))
                    .child(
                        Button::new("left-off-clear")
                            .ghost()
                            .small()
                            .icon(IconName::X)
                            .tooltip(t("game.left_off.clear"))
                            .accessibility_label(t("game.left_off.clear"))
                            .on_click(move |_, window, cx| {
                                irori_ui::journal::set_left_off(
                                    clear_id.clone(),
                                    String::new(),
                                    window,
                                    cx,
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .text_sm()
                    .text_color(ink.text)
                    .line_clamp(4)
                    .child(note),
            )
            .into_any_element()
    }
}
