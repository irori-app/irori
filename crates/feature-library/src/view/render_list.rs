//! Virtual-list item dispatch, group headings, list view rows, and selection
//! pick marks.

use std::ops::Range;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ClickEvent, Context, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    px, rems,
};
use irori_core::StatusKind;
use irori_core::settings::ListRowSize;
use irori_ui::components::{GameCover, SectionTitle};
use irori_ui::i18n::t;
use irori_ui::motion::gliding_progress;
use irori_ui::{models, play_status};

use super::menus::game_menu;
use super::{Item, LibraryView, status_text};
use crate::commands::{self, GameCommand};
use crate::filter::{self, GroupKind};
use crate::grid;

impl LibraryView {
    pub(super) fn render_items(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let items = self.frame.items.clone();
        range
            .filter_map(|ix| items.get(ix).copied())
            .map(|item| match item {
                Item::Top => self.render_top(window, cx),
                Item::Heading => div()
                    .h(rems(grid::HEADING_REM))
                    .px(rems(grid::EDGE_REM))
                    .flex()
                    .items_end()
                    .pb_2()
                    .child(SectionTitle::new(self.frame.heading.clone()))
                    .into_any_element(),
                Item::GroupHeading(ix) => self.render_group_heading(ix, cx),
                // The toolbar is drawn over this room (`render_list_toolbar`).
                Item::Toolbar => div().into_any_element(),
                Item::ListHeader => self.render_list_header(cx),
                Item::GridRow { start, end } => self.render_grid_row(start..end, window, cx),
                Item::ListRow(ix) => self.render_list_row(ix, window, cx),
            })
            .collect()
    }

    /// A section's heading: its play status's colour or its game mode's
    /// icon, its name and its game count.
    fn render_group_heading(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(group) = self.frame.groups.get(ix) else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let (mark, name): (AnyElement, SharedString) = match group.kind.clone() {
            GroupKind::Status(status) => (
                div()
                    .flex_none()
                    .size_2()
                    .rounded_full()
                    .bg(play_status::color(status, cx))
                    .into_any_element(),
                play_status::label(status),
            ),
            GroupKind::Mode(mode) => (
                Icon::new(
                    mode.map_or_else(irori_ui::game_mode::unknown_icon, irori_ui::game_mode::icon),
                )
                .small()
                .text_color(theme.muted_foreground)
                .into_any_element(),
                mode.map_or_else(
                    irori_ui::game_mode::unknown_label,
                    irori_ui::game_mode::label,
                ),
            ),
            GroupKind::Genre(genre) => (
                Icon::new(IconName::Tag)
                    .small()
                    .text_color(theme.muted_foreground)
                    .into_any_element(),
                genre.map_or_else(
                    || t("library.group.no_genre"),
                    |g| SharedString::from(g.to_string()),
                ),
            ),
            GroupKind::Store(runner) => (
                Icon::new(IconName::Store)
                    .small()
                    .text_color(theme.muted_foreground)
                    .into_any_element(),
                SharedString::from(runner.store_name()),
            ),
            GroupKind::Platform(platform) => (
                Icon::new(IconName::Monitor)
                    .small()
                    .text_color(theme.muted_foreground)
                    .into_any_element(),
                filter::platform_label(platform).into(),
            ),
            GroupKind::Company(company, name) => {
                let (icon, unknown) = match company {
                    filter::Company::Developer => {
                        (IconName::Building2, "library.group.no_developer")
                    }
                    filter::Company::Publisher => {
                        (IconName::Megaphone, "library.group.no_publisher")
                    }
                };
                (
                    Icon::new(icon)
                        .small()
                        .text_color(theme.muted_foreground)
                        .into_any_element(),
                    name.map_or_else(|| t(unknown), |n| SharedString::from(n.to_string())),
                )
            }
            GroupKind::Year(year) => (
                Icon::new(IconName::CalendarDays)
                    .small()
                    .text_color(theme.muted_foreground)
                    .into_any_element(),
                year.map_or_else(
                    || t("library.group.no_year"),
                    |y| SharedString::from(y.to_string()),
                ),
            ),
            GroupKind::Playtime(band) => (
                Icon::new(IconName::Clock)
                    .small()
                    .text_color(theme.muted_foreground)
                    .into_any_element(),
                band.label().into(),
            ),
            GroupKind::Size(band) => (
                Icon::new(IconName::HardDrive)
                    .small()
                    .text_color(theme.muted_foreground)
                    .into_any_element(),
                band.label().into(),
            ),
            GroupKind::Category(None) => (
                Icon::new(IconName::Tag)
                    .small()
                    .text_color(theme.muted_foreground)
                    .into_any_element(),
                t("library.filter.uncategorized"),
            ),
            GroupKind::Category(Some((category, name))) => {
                let user = &models(cx).library.read(cx).user;
                let look = user.category_look(&category);
                let colour = irori_ui::categories::colour(look.colour.as_deref(), cx)
                    .unwrap_or(theme.muted_foreground);
                (
                    Icon::new(irori_ui::categories::icon_for(
                        &category,
                        look.icon.as_deref(),
                    ))
                    .small()
                    .text_color(colour)
                    .into_any_element(),
                    SharedString::from(name.to_string()),
                )
            }
        };
        h_flex()
            .h(rems(grid::HEADING_REM))
            .px(rems(grid::EDGE_REM))
            .pb_2()
            .items_end()
            .child(
                h_flex()
                    .min_w_0()
                    .gap_2()
                    .child(mark)
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_semibold()
                            .child(name),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px_1p5()
                            .rounded_md()
                            .bg(theme.secondary)
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(group.range.len().to_string()),
                    ),
            )
            .into_any_element()
    }

    fn render_list_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let shown = self.frame.list_columns;
        let thumb_rem = grid::list_thumb_rem_at(self.list_row_size, self.density);
        let column = |w: f32, key: &str, right: bool| {
            div()
                .w(rems(w))
                .flex_shrink_0()
                .truncate()
                .when(right, |el| el.text_right())
                .child(t(key))
        };
        h_flex()
            .w_full()
            .h(rems(2.25))
            .px(rems(grid::EDGE_REM))
            .gap_4()
            .text_xs()
            .font_bold()
            .text_color(muted)
            .border_b_1()
            .border_color(border)
            .child(div().w(rems(thumb_rem)).flex_shrink_0())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(t("library.list.title")),
            )
            .when(shown[0], |el| {
                el.child(column(grid::LIST_COLUMNS[0], "library.list.store", false))
            })
            .when(shown[1], |el| {
                el.child(column(grid::LIST_COLUMNS[1], "library.list.size", true))
            })
            .when(shown[2], |el| {
                el.child(column(grid::LIST_COLUMNS[2], "library.list.playtime", true))
            })
            .when(shown[3], |el| {
                el.child(column(
                    grid::LIST_COLUMNS[3],
                    "library.list.last_played",
                    false,
                ))
            })
            .when(shown[4], |el| {
                el.child(column(grid::LIST_COLUMNS[4], "library.list.status", false))
            })
            .when(shown[5], |el| {
                el.child(column(grid::LIST_COLUMNS[5], "library.list.compat", false))
            })
            .child(div().w(rems(grid::LIST_COLUMNS[6])).flex_shrink_0())
            .into_any_element()
    }

    fn render_list_row(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(game) = self.frame.games.get(ix).cloned() else {
            return div().into_any_element();
        };
        let id = game.id();
        let key = id.key();
        let state = self.state_for(&game, false, cx);
        let status = models(cx).statuses.read(cx).get(&id).cloned();
        let (minutes, last) = {
            let lib = models(cx).library.read(cx);
            (
                lib.playtime_minutes(&id),
                lib.last_played(&id).map(str::to_string),
            )
        };
        let selecting = self.selection.is_active();
        let picked = self.selection.contains(&id);
        let keyboard =
            self.focused.as_ref() == Some(&id) && self.grid_focus.contains_focused(window, cx);
        let active =
            keyboard || self.hovered.as_ref() == Some(&id) || self.menu_open.as_ref() == Some(&id);
        let queue_place = models(cx).downloads.read(cx).queue.queue_place(&id);
        let theme = cx.theme();
        let (muted, border, hover_bg, active_bg, ring) = (
            theme.muted_foreground,
            theme.border,
            theme.list_hover,
            theme.list_active,
            theme.ring,
        );
        let now = crate::format::now();
        let dash = || "—".to_string();

        let status_cell: AnyElement = match status.as_ref() {
            Some(s)
                if s.kind.is_busy()
                    && !matches!(s.kind, StatusKind::Playing | StatusKind::Queued) =>
            {
                let pct = s.progress.and_then(|p| p.percent);
                v_flex()
                    .gap_1()
                    .child(div().text_xs().truncate().child(status_text(s)))
                    .child(gliding_progress(
                        ElementId::Name(format!("row-progress:{key}").into()),
                        pct,
                    ))
                    .into_any_element()
            }
            Some(s) if s.kind == StatusKind::Playing => Tag::success()
                .xsmall()
                .child(t("library.status.playing"))
                .into_any_element(),
            Some(s) if s.kind == StatusKind::Queued => Tag::secondary()
                .xsmall()
                .child(crate::i18n::queued_label(queue_place))
                .into_any_element(),
            _ => div()
                .text_xs()
                .text_color(muted)
                .truncate()
                .child(t(if state.unavailable {
                    "library.caption.files_missing"
                } else if state.installed {
                    if state.has_update && !state.sideload {
                        "library.caption.update_available"
                    } else {
                        "library.caption.installed"
                    }
                } else {
                    "library.caption.not_installed"
                }))
                .into_any_element(),
        };

        let primary = commands::primary(&state).map(|command| {
            let id = id.clone();
            Button::new(ElementId::Name(format!("row-primary:{key}").into()))
                .xsmall()
                .icon(command.icon(&state))
                .label(command.label(&state))
                .loading(state.launching)
                .when(command != GameCommand::Stop, |b| b.primary())
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.run_command(&id, command.clone(), window, cx)
                }))
        });

        let view = cx.entity().downgrade();
        let menu_game = game.clone();
        let hover_id = id.clone();
        let click_id = id.clone();
        let shown = self.frame.list_columns;
        if shown[5] {
            self.request_compat(&game, true, cx);
        }
        let compat_badge = shown[5].then(|| self.compat.get(&id)).flatten();
        // An entry of several stores' copies names every store.
        let stores = shown[0].then(|| {
            irori_ui::copies::stores(&id, cx)
                .iter()
                .map(|r| r.short_name())
                .collect::<Vec<_>>()
                .join(" · ")
        });
        let cell = |w: f32, right: bool, text: String| {
            div()
                .w(rems(w))
                .flex_shrink_0()
                .text_sm()
                .truncate()
                .when(right, |el| el.text_right())
                .child(text)
        };
        let row_size = self.list_row_size;
        let thumb_rem = grid::list_thumb_rem_at(row_size, self.density);
        h_flex()
            .id(ElementId::Name(format!("row:{key}").into()))
            .w_full()
            .h(rems(self.frame.metrics.list_row_rem))
            .px(rems(grid::EDGE_REM))
            .gap_4()
            .border_b_1()
            .border_color(border)
            .relative()
            .hover(|s| s.bg(hover_bg))
            // Rows span the list, where a ring around them would be clipped; the
            // focused one gets a bar in the focus colour on its leading edge.
            .when(keyboard, |el| {
                el.bg(active_bg).child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(3.))
                        .bg(ring),
                )
            })
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.set_hovered(&hover_id, *hovered, cx)
            }))
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                this.click_game(&click_id, e.modifiers(), e.click_count(), window, cx)
            }))
            .when(picked, |el| el.bg(active_bg))
            .when(selecting, |el| {
                el.child(div().flex_shrink_0().child(pick_mark(picked, cx)))
            })
            .child(
                GameCover::new(game.clone())
                    .width(rems(thumb_rem))
                    .radius(cx.theme().radius / 2.),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_1()
                            .min_w_0()
                            .child(
                                div()
                                    .map(|el| match row_size {
                                        ListRowSize::Compact => el.text_sm().font_medium(),
                                        ListRowSize::Medium => el.text_base().font_semibold(),
                                        ListRowSize::Large => el.text_lg().font_semibold(),
                                    })
                                    .truncate()
                                    .child(game.title.clone()),
                            )
                            .when(state.favourite, |el| {
                                el.child(Icon::new(IconName::Star).xsmall().text_color(muted))
                            }),
                    )
                    .when(state.hidden, |el| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .truncate()
                                .child(t("library.caption.hidden")),
                        )
                    }),
            )
            .when_some(stores, |el, stores| {
                el.child(cell(grid::LIST_COLUMNS[0], false, stores))
            })
            .when(shown[1], |el| {
                el.child(cell(
                    grid::LIST_COLUMNS[1],
                    true,
                    crate::format::install_size(&game).unwrap_or_else(dash),
                ))
            })
            .when(shown[2], |el| {
                el.child(cell(
                    grid::LIST_COLUMNS[2],
                    true,
                    crate::format::playtime(minutes).unwrap_or_else(dash),
                ))
            })
            .when(shown[3], |el| {
                el.child(cell(
                    grid::LIST_COLUMNS[3],
                    false,
                    crate::format::last_played(last.as_deref(), now).unwrap_or_else(dash),
                ))
            })
            .when(shown[4], |el| {
                el.child(
                    div()
                        .w(rems(grid::LIST_COLUMNS[4]))
                        .flex_shrink_0()
                        .child(status_cell),
                )
            })
            .when(shown[5], |el| {
                el.child(
                    div()
                        .w(rems(grid::LIST_COLUMNS[5]))
                        .flex_shrink_0()
                        .child(crate::compat_badges::render_list_cell(compat_badge, cx)),
                )
            })
            .child(
                h_flex()
                    .w(rems(grid::LIST_COLUMNS[6]))
                    .flex_shrink_0()
                    .justify_end()
                    .gap_1()
                    .when(active, |el| {
                        el.children(primary)
                            .child(self.more_button(&game, false, cx))
                    }),
            )
            .context_menu(move |menu, window, cx| {
                game_menu(menu, &view, &menu_game, false, window, cx)
            })
            .into_any_element()
    }
}

/// The circle marking a game in selection mode: filled with a tick when
/// picked, empty otherwise. Sits at the top left of a cover, or leads a row.
pub(super) fn pick_mark(picked: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let mark = div()
        .size(rems(1.5))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_2();
    if picked {
        mark.bg(theme.primary)
            .border_color(theme.primary)
            .text_color(theme.primary_foreground)
            .child(Icon::new(IconName::Check).xsmall())
            .into_any_element()
    } else {
        mark.bg(theme.background.opacity(0.6))
            .border_color(theme.foreground.opacity(0.8))
            .into_any_element()
    }
}
