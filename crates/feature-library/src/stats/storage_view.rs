//! The storage section under both tabs: per drive, how much the installed
//! games take next to everything else, and the largest of them.

use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, px, relative,
};
use irori_ui::components::SectionTitle;
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{KeyboardFocusable as _, panel};
use irori_ui::{Route, models, navigate};

use super::StatsView;
use super::storage::DriveUsage;

/// Games listed under each drive, largest first.
const DRIVE_GAMES: usize = 5;

impl StatsView {
    pub(super) fn render_storage(
        &self,
        drives: &[DriveUsage],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows: Vec<AnyElement> = drives
            .iter()
            .enumerate()
            .map(|(ix, usage)| self.render_drive(ix, usage, cx))
            .collect();
        let theme = cx.theme();
        let legend = |color, label: SharedString| {
            h_flex()
                .gap_1p5()
                .child(div().size_2p5().rounded_full().bg(color))
                .child(label)
        };
        panel(cx)
            .gap_4()
            .child(
                h_flex()
                    .flex_wrap()
                    .justify_between()
                    .gap_2()
                    .child(SectionTitle::new(t("library.stats.storage")))
                    .child(
                        h_flex()
                            .gap_4()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(legend(
                                theme.chart_1,
                                t("library.stats.storage_legend_games"),
                            ))
                            .child(legend(
                                theme.chart_3,
                                t("library.stats.storage_legend_other"),
                            )),
                    ),
            )
            .children(rows)
            .into_any_element()
    }

    fn render_drive(&self, ix: usize, usage: &DriveUsage, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted_fg, track, games_color, other_color) = (
            theme.muted_foreground,
            theme.muted,
            theme.chart_1,
            theme.chart_3,
        );
        let (radius, hover_bg) = (theme.radius, theme.list_hover);
        let size = |bytes: u64| irori_core::util::format_bytes(bytes);
        let (label, summary) = match &usage.drive {
            Some(d) => (
                SharedString::from(d.label()),
                t_args(
                    "library.stats.storage_summary",
                    &[
                        ("games", &size(usage.used)),
                        ("free", &size(d.free)),
                        ("total", &size(d.total)),
                    ],
                ),
            ),
            None => (
                t("library.stats.storage_unknown"),
                t_args(
                    "library.stats.storage_games",
                    &[("games", &size(usage.used))],
                ),
            ),
        };
        let bar = usage.drive.as_ref().filter(|d| d.total > 0).map(|d| {
            let total = d.total as f32;
            let games = (usage.used as f32 / total).clamp(0.0, 1.0);
            let taken = d.total.saturating_sub(d.free) as f32 / total;
            let other = (taken - games).clamp(0.0, 1.0 - games);
            h_flex()
                .w_full()
                .h(px(10.))
                .rounded(px(5.))
                .overflow_hidden()
                .bg(track)
                .child(div().h_full().w(relative(games)).bg(games_color))
                .child(div().h_full().w(relative(other)).bg(other_color))
        });
        let library = models(cx).library.read(cx).snapshot.clone();
        let games: Vec<AnyElement> = usage
            .games
            .iter()
            .take(DRIVE_GAMES)
            .map(|(id, bytes)| {
                let title = library
                    .get(id)
                    .map(|g| g.title.clone())
                    .unwrap_or_else(|| id.app_name.to_string());
                let id = id.clone();
                h_flex()
                    .id(ElementId::Name(format!("storage-game:{}", id.key()).into()))
                    .keyboard_focusable(cx)
                    .w_full()
                    .gap_3()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .rounded(radius)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover_bg))
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(muted_fg)
                            .child(if *bytes > 0 {
                                SharedString::from(size(*bytes))
                            } else {
                                t("library.stats.storage_size_unknown")
                            }),
                    )
                    .on_click(move |_, _, cx| navigate(Route::Game(id.clone()), cx))
                    .into_any_element()
            })
            .collect();
        let more = usage.games.len().saturating_sub(DRIVE_GAMES);
        v_flex()
            .id(ElementId::Name(format!("storage-drive:{ix}").into()))
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .justify_between()
                    .gap_x_4()
                    .child(div().min_w_0().font_semibold().truncate().child(label))
                    .child(div().text_sm().text_color(muted_fg).child(summary)),
            )
            .children(bar)
            .child(v_flex().children(games))
            .when(more > 0, |d| {
                d.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(muted_fg)
                        .child(if more == 1 {
                            t("library.stats.storage_more_one")
                        } else {
                            t_args(
                                "library.stats.storage_more_other",
                                &[("count", &more.to_string())],
                            )
                        }),
                )
            })
            .into_any_element()
    }
}
