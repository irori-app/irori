//! The Copies card beside the details, for a game owned on several stores
//! (or several editions combined into one entry): each copy with what tells
//! it apart, its own time played, Play or Install, and which one the entry
//! plays by default; the time of every copy added up at the bottom.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Selectable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement, SharedString, Styled,
    div,
};
use irori_core::{GameId, GameInfo};
use irori_ui::copies::{CopyAction, CopyLabel};
use irori_ui::i18n::t;
use irori_ui::models;

use super::GamePage;
use crate::i18n::playtime_label;

/// Each copy's own minutes, in the copies' order, and their sum.
pub(super) fn minutes(copies: &[GameId], minutes_of: impl Fn(&GameId) -> u64) -> (Vec<u64>, u64) {
    let each: Vec<u64> = copies.iter().map(minutes_of).collect();
    let total = each.iter().sum();
    (each, total)
}

impl GamePage {
    /// The card, for an entry of two or more copies.
    pub(super) fn render_copies(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let copies: Vec<(Arc<GameInfo>, CopyLabel)> = irori_ui::copies::labelled(&self.id, cx);
        if copies.len() < 2 {
            return None;
        }
        let ids: Vec<GameId> = copies.iter().map(|(c, _)| c.id()).collect();
        let (each, total) = {
            let library = models(cx).library.read(cx);
            minutes(&ids, |id| library.playtime_minutes(id))
        };
        let statuses = models(cx).statuses.read(cx);
        let busy: Vec<bool> = ids
            .iter()
            .map(|id| {
                statuses
                    .kind(id)
                    .is_some_and(irori_core::StatusKind::is_busy)
            })
            .collect();
        let ink = self.ink;
        let theme = cx.theme();
        let default = ids.first().cloned();
        let rows = copies.into_iter().enumerate().map(|(i, (copy, label))| {
            let id = copy.id();
            let current = id == self.id;
            let is_default = default.as_ref() == Some(&id);
            // The page's own primary action plays or installs this copy.
            let action = (!current)
                .then(|| irori_ui::copies::copy_action(&copy, busy[i]))
                .flatten();
            let text: SharedString = label.text().into();
            let star = {
                let (id, group) = (id.clone(), ids.clone());
                Button::new(("copy-default", i))
                    .ghost()
                    .xsmall()
                    .icon(if is_default {
                        IconName::StarFill
                    } else {
                        IconName::Star
                    })
                    .tooltip(if is_default {
                        t("game.copies.is_default")
                    } else {
                        irori_ui::copies::make_default_label()
                    })
                    .on_click(move |_, _, cx| {
                        if !is_default {
                            irori_ui::copies::make_default(id.clone(), group.clone(), cx)
                        }
                    })
            };
            let name = {
                let id = id.clone();
                Button::new(("copy-open", i))
                    .ghost()
                    .xsmall()
                    .when(current, |b| b.selected(true))
                    .label(text.clone())
                    .tooltip(irori_ui::copies::open_label(&text))
                    .on_click(move |_, _, cx| {
                        if !current {
                            irori_ui::navigate(irori_ui::Route::Game(id.clone()), cx)
                        }
                    })
            };
            let run = action.map(|action| {
                let id = id.clone();
                Button::new(("copy-run", i))
                    .xsmall()
                    .outline()
                    .icon(match action {
                        CopyAction::Play => IconName::Play,
                        CopyAction::Install => IconName::Download,
                    })
                    .tooltip(irori_ui::copies::action_label(action, &label))
                    .on_click(move |_, window, cx| {
                        irori_ui::copies::run_action(action, id.clone(), window, cx)
                    })
            });
            h_flex()
                .w_full()
                .gap_1()
                .child(star)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .items_start()
                        .child(div().max_w_full().min_w_0().child(name))
                        .child(
                            div()
                                .pl_2()
                                .text_xs()
                                .text_color(ink.panel_muted)
                                .truncate()
                                .child(playtime_label(each[i], each[i] > 0)),
                        ),
                )
                .children(run)
        });
        Some(
            v_flex()
                .id("game-copies")
                .w_full()
                .px_4()
                .pt_3()
                .pb_2()
                .gap_1()
                .rounded(theme.radius_lg)
                .bg(ink.panel)
                .border_1()
                .border_color(theme.border)
                .child(div().pb_1().child(super::section_title(
                    irori_ui::copies::title_label(),
                    ink.text,
                )))
                .children(rows)
                .child(
                    h_flex()
                        .w_full()
                        .pt_2()
                        .mt_1()
                        .gap_2()
                        .justify_between()
                        .border_t_1()
                        .border_color(theme.border)
                        .text_sm()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(ink.panel_muted)
                                .child(t("game.copies.total")),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .font_semibold()
                                .child(playtime_label(total, total > 0)),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;

    #[test]
    fn each_copy_keeps_its_time_and_the_total_adds_them() {
        let ids = [
            GameId::new(Runner::Steam, "1"),
            GameId::new(Runner::Gog, "2"),
            GameId::new(Runner::Legendary, "3"),
        ];
        let (each, total) = minutes(&ids, |id| match id.runner {
            Runner::Steam => 600,
            Runner::Gog => 45,
            _ => 0,
        });
        assert_eq!(each, [600, 45, 0]);
        assert_eq!(total, 645);
    }
}
