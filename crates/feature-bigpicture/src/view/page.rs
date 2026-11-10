//! A game's page: its cover and title, a list of actions to move through
//! (rows with a value change it with Left and Right), and its details.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, ClickEvent, Context, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Role, SharedString, SpringAnimation,
    StatefulInteractiveElement as _, Styled as _, Window, div, rems,
};
use irori_core::GameInfo;
use irori_ui::components::GameCover;
use irori_ui::i18n::{t, t_args};
use irori_ui::models;
use irori_ui::motion::{QUICK, rise_in};

use super::home::SCROLL_SPRING;
use super::{BigPictureView, Colors};
use crate::layout::{Metrics, end_scroll, list_scroll};
use crate::model::{
    GameAction, GameState, PageRow, click_acts, default_platform, disabled_reason,
    install_platforms, page_rows,
};
use crate::text::{self, store_name};

/// Height of a row and of the "Manage" caption, in rem.
const ROW_H: f32 = 3.25;
const COMPACT_ROW_H: f32 = 2.75;
const CAPTION_H: f32 = 2.25;
/// Pages narrower than this (in rem) leave out the details panel.
const DETAILS_FROM: f32 = 56.0;

/// `a` moved toward `b` by `t` (0…1).
fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    Hsla {
        h: a.h + (b.h - a.h) * t,
        s: a.s + (b.s - a.s) * t,
        l: a.l + (b.l - a.l) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

impl BigPictureView {
    pub(super) fn render_page(
        &mut self,
        c: Colors,
        m: Metrics,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(game) = self.page_game(cx) else {
            return div().into_any_element();
        };
        let state = Self::game_state(&game, cx);
        let platforms = install_platforms(&game);
        let rows = page_rows(state, platforms.len(), self.show_log);
        let show_log = self.show_log && rows.contains(&PageRow::Log);
        self.sync_log(show_log.then(|| game.id()).as_ref(), cx);
        let focused = self
            .page
            .as_ref()
            .map_or(0, |p| p.ix.min(rows.len().saturating_sub(1)));
        let row_h = if m.compact { COMPACT_ROW_H } else { ROW_H };
        let cover_w = if m.compact { 5.0 } else { 8.5 };
        let header_h = cover_w * 1.5;
        // Padding and gaps around the header and list.
        let list_h = m.height - m.top_bar - m.hints - header_h - 2.75;
        let caption_before = rows.iter().position(|r| r.starts_manage());
        let content = rows.len() as f32 * row_h + caption_before.map_or(0.0, |_| CAPTION_H);
        let scroll = end_scroll(
            list_scroll(focused, caption_before, row_h, CAPTION_H, list_h),
            content,
            list_h,
        );

        let mut list = Vec::with_capacity(rows.len() + 1);
        let mut captioned = false;
        for (ix, row) in rows.iter().copied().enumerate() {
            if row.starts_manage() && !captioned {
                captioned = true;
                list.push(
                    div()
                        .h(rems(CAPTION_H))
                        .px_4()
                        .flex()
                        .items_end()
                        .pb_1()
                        .text_sm()
                        .font_semibold()
                        .text_color(c.foreground.opacity(0.6))
                        .child(t("bigpicture.page.manage"))
                        .into_any_element(),
                );
            }
            list.push(self.render_row(row, ix, ix == focused, &game, state, row_h, c, cx));
        }

        let key = game.id().key();
        let header = h_flex()
            .h(rems(header_h))
            .flex_shrink_0()
            .gap_6()
            .items_end()
            .child(
                GameCover::new(game.clone())
                    .width(rems(cover_w))
                    .radius(c.radius_lg),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .pb_1()
                    .child(
                        div()
                            .text_sm()
                            .font_bold()
                            .text_color(c.foreground.opacity(0.65))
                            .child(SharedString::from(store_name(game.runner()).to_uppercase())),
                    )
                    .child(
                        div()
                            .id("bp-page-title")
                            .role(Role::Heading)
                            .text_size(rems(if m.compact { 2.0 } else { 3.25 }))
                            .line_height(rems(if m.compact { 2.6 } else { 4.1 }))
                            .font_extrabold()
                            .line_clamp(if m.compact { 1 } else { 2 })
                            .child(game.title.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .min_w_0()
                            .items_center()
                            .text_lg()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(c.foreground.opacity(0.8))
                                    .child(Self::meta_line(&game, state, false, cx)),
                            )
                            .when_some(Self::status_pill(&game, state, cx), |d, pill| {
                                d.child(div().flex_shrink_0().child(pill))
                            }),
                    )
                    .when_some(facts_line(&game).filter(|_| !m.compact), |d, facts| {
                        d.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(c.foreground.opacity(0.6))
                                .child(facts),
                        )
                    }),
            );

        // The log shows on narrow pages too: it is what the user asked for.
        let details = if show_log {
            Some(self.render_log(c, window, cx))
        } else {
            (m.width >= DETAILS_FROM).then(|| self.render_details(&game, c, m, cx))
        };
        let body = h_flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .gap_8()
            .items_start()
            .child(
                div()
                    .id("bp-page-actions")
                    .role(Role::Menu)
                    .w(rems(if details.is_some() {
                        (m.width * 0.4).clamp(24.0, 36.0)
                    } else {
                        m.width - 2.0 * m.gutter
                    }))
                    .flex_shrink_0()
                    .h_full()
                    .relative()
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .absolute()
                            .left_0()
                            .right_0()
                            .children(list)
                            .with_spring(
                                "bp-page-scroll",
                                SpringAnimation::new(SCROLL_SPRING).to(scroll),
                                |el, v: f32| el.top(rems(-v)),
                            ),
                    ),
            )
            .when_some(details, |d, details| d.child(details));

        rise_in(
            SharedString::from(format!("bp-page-{key}")),
            v_flex()
                .size_full()
                .px(rems(m.gutter))
                .pt_2()
                .pb_4()
                .gap_5()
                .child(header)
                .child(body),
            16.,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_row(
        &self,
        row: PageRow,
        ix: usize,
        focused: bool,
        game: &Arc<GameInfo>,
        state: GameState,
        row_h: f32,
        c: Colors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focused = focused && self.overlay.is_none();
        let (icon, label) = match row {
            PageRow::Action(action) => (action.icon(), action.label()),
            PageRow::Platform => (IconName::Monitor, t("bigpicture.page.platform")),
            PageRow::Favourite => (
                if state.favourite {
                    IconName::StarFill
                } else {
                    IconName::Star
                },
                t("bigpicture.page.favourite"),
            ),
            PageRow::Settings => (IconName::SlidersHorizontal, t("bigpicture.page.settings")),
            PageRow::Log => (IconName::ScrollText, t("bigpicture.page.log")),
            PageRow::LogErrors => (IconName::TriangleAlert, t("bigpicture.page.log_errors")),
            PageRow::LogFilter => (IconName::ListFilter, t("bigpicture.page.log_filter")),
            PageRow::Playtime => (IconName::Clock, t("bigpicture.page.playtime")),
            PageRow::Hidden => (IconName::EyeOff, t("bigpicture.page.hidden")),
        };
        let disabled = match row {
            PageRow::Action(action) => disabled_reason(action, state).is_some(),
            _ => false,
        };
        let pending =
            matches!(row, PageRow::Action(_)) && ix == 0 && self.pending.contains(&game.id());
        let (fg, bg) = (c.foreground, c.background);

        // On the focused row's light highlight the "Off" pill takes the
        // highlight's own text colour.
        let ink = if focused { bg } else { fg };
        let toggle = |on: bool| {
            div()
                .flex_shrink_0()
                .px_3()
                .py_0p5()
                .rounded_full()
                .text_sm()
                .font_bold()
                .map(|d| {
                    if on {
                        d.bg(c.primary).text_color(c.primary_foreground)
                    } else {
                        d.bg(ink.opacity(0.15)).text_color(ink.opacity(0.8))
                    }
                })
                .child(t(if on {
                    "bigpicture.toggle.on"
                } else {
                    "bigpicture.toggle.off"
                }))
                .into_any_element()
        };
        let trailing: Option<AnyElement> = match row {
            PageRow::Action(_) if pending => Some(Spinner::new().into_any_element()),
            PageRow::Action(GameAction::Uninstall) | PageRow::Settings => {
                Some(Icon::new(IconName::ChevronRight).into_any_element())
            }
            PageRow::Action(_) => None,
            PageRow::Platform => {
                let current = self
                    .page
                    .as_ref()
                    .and_then(|p| p.platform)
                    .unwrap_or_else(|| default_platform(game));
                Some(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(Icon::new(IconName::ChevronLeft).small())
                        .child(div().font_bold().child(current.label()))
                        .child(Icon::new(IconName::ChevronRight).small())
                        .into_any_element(),
                )
            }
            PageRow::Favourite => Some(toggle(state.favourite)),
            PageRow::Hidden => Some(toggle(state.hidden)),
            PageRow::Log => Some(toggle(self.show_log)),
            PageRow::LogErrors => Some(toggle(self.log_filter.errors_only)),
            PageRow::LogFilter => {
                let query = self.log_filter.query(cx);
                Some(if query.is_empty() {
                    Icon::new(IconName::ChevronRight).into_any_element()
                } else {
                    div()
                        .min_w_0()
                        .max_w(rems(12.))
                        .truncate()
                        .font_bold()
                        .child(t_args("bigpicture.log.filter_value", &[("text", &query)]))
                        .into_any_element()
                })
            }
            PageRow::Playtime => {
                let library = models(cx).library.read(cx);
                let minutes = library.playtime_minutes(&game.id());
                Some(
                    div()
                        .flex_shrink_0()
                        .child(if minutes > 0 {
                            text::playtime(minutes)
                        } else {
                            t("bigpicture.info.never_played")
                        })
                        .into_any_element(),
                )
            }
        };

        let element = h_flex()
            .id(row.id())
            .role(Role::MenuItem)
            .aria_label(label.clone())
            .aria_selected(focused)
            .when(focused, |d| d.aria_active_descendant())
            .w_full()
            .h(rems(row_h))
            .px_4()
            .gap_4()
            .items_center()
            .rounded(c.radius_lg)
            .cursor_pointer()
            .text_xl()
            .text_color(fg)
            .when(!focused, |d| d.hover(|s| s.bg(c.foreground.opacity(0.08))))
            .when(disabled, |d| d.opacity(0.5))
            .child(Icon::new(icon))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .when_some(trailing, |d, t| d.child(t))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if !click_acts(event.click_count()) {
                    return;
                }
                if let Some(page) = &mut this.page {
                    page.ix = ix;
                }
                if let Some(game) = this.page_game(cx) {
                    this.activate_row(row, game, window, cx);
                }
                this.focus.focus(window, cx);
                cx.notify();
            }));
        if focused {
            // The highlight comes up quickly, its text turning with it.
            element
                .with_animation(
                    SharedString::from(format!("{}-focus", row.id())),
                    Animation::new(QUICK).with_easing(gpui_kit::ease_in_out),
                    move |el, t| el.bg(fg.opacity(t)).text_color(mix(fg, bg, t)),
                )
                .into_any_element()
        } else {
            element.into_any_element()
        }
    }

    fn render_details(
        &self,
        game: &GameInfo,
        c: Colors,
        m: Metrics,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let library = models(cx).library.read(cx);
        let id = game.id();
        let mut facts: Vec<(SharedString, SharedString)> = Vec::new();
        let mut push = |key: &str, value: Option<String>| {
            if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
                facts.push((t(key), value.into()));
            }
        };
        push(
            "bigpicture.details.store",
            Some(store_name(game.runner()).to_string()),
        );
        push(
            "bigpicture.details.system",
            game.system
                .as_deref()
                .and_then(irori_engines::systems::get)
                .map(|s| s.name.clone()),
        );
        push("bigpicture.details.developer", game.developer.clone());
        push("bigpicture.details.publisher", game.publisher.clone());
        push(
            "bigpicture.details.released",
            game.release_date
                .clone()
                .or_else(|| game.extra.as_ref().and_then(|e| e.release_date.clone()))
                .map(|d| d.chars().take(10).collect()),
        );
        let genres = if game.genres.is_empty() {
            game.extra
                .as_ref()
                .map(|e| e.genres.clone())
                .unwrap_or_default()
        } else {
            game.genres.clone()
        };
        push("bigpicture.details.genres", Some(genres.join(", ")));
        push(
            "bigpicture.details.last_played",
            library
                .last_played(&id)
                .and_then(text::last_played)
                .map(|s| s.to_string()),
        );
        let minutes = library.playtime_minutes(&id);
        push(
            "bigpicture.details.playtime",
            (minutes > 0).then(|| text::playtime(minutes).to_string()),
        );
        if let Some(install) = &game.install {
            push(
                "bigpicture.details.size",
                Some(install.install_size.clone()),
            );
            push("bigpicture.details.version", Some(install.version.clone()));
            push(
                "bigpicture.details.platform",
                Some(install.platform.label().to_string()),
            );
        }
        let description = text::description(game);
        let label_w = 9.0_f32.min(m.width * 0.12);

        v_flex()
            .id("bp-page-details")
            .role(Role::Region)
            .flex_1()
            .min_w_0()
            .max_h_full()
            .p_6()
            .gap_4()
            .rounded(c.radius_lg)
            .bg(c.background.opacity(0.6))
            .border_1()
            .border_color(c.foreground.opacity(0.08))
            .overflow_y_scroll()
            .when_some(description, |d, text| {
                d.child(
                    div()
                        .text_base()
                        .line_clamp(if m.compact { 3 } else { 5 })
                        .text_color(c.foreground.opacity(0.85))
                        .child(text),
                )
            })
            .child(
                v_flex()
                    .gap_2()
                    .children(facts.into_iter().map(|(label, value)| {
                        h_flex()
                            .gap_4()
                            .items_start()
                            .min_w_0()
                            .child(
                                div()
                                    .w(rems(label_w))
                                    .flex_shrink_0()
                                    .truncate()
                                    .text_color(c.foreground.opacity(0.6))
                                    .child(label),
                            )
                            .child(div().flex_1().min_w_0().line_clamp(2).child(value))
                    })),
            )
            .into_any_element()
    }
}

/// Release year and genres under the title ("2022 · Role-playing · Action").
fn facts_line(game: &GameInfo) -> Option<SharedString> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(date) = game
        .release_date
        .as_deref()
        .or_else(|| game.extra.as_ref().and_then(|e| e.release_date.as_deref()))
        .filter(|d| d.len() >= 4)
    {
        parts.push(date.chars().take(4).collect());
    }
    parts.extend(game.genres.iter().take(3).cloned());
    (!parts.is_empty()).then(|| parts.join(" · ").into())
}
