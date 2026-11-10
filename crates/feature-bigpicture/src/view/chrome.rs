//! The frame around every screen: the top bar (Back on a game's page, tabs
//! between the shoulder buttons, downloads, controller, clock and the button
//! that leaves Big Picture) and the bottom bar of button hints, drawn as the
//! buttons of the controller in hand (Xbox, PlayStation, Nintendo) or as keys
//! when there is none.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, rems,
};
use irori_ui::components::BrandMark;
use irori_ui::i18n::{t, t_args};
use irori_ui::models;
use irori_ui::style::{ICON_CONTRAST, TEXT_CONTRAST, legible};

use super::{BigPictureView, Colors, LibraryFocus, Overlay, Tab, clock_text};
use crate::gamepad::{self, ControllerKind, GamepadInput};
use crate::layout::Metrics;
use crate::model::{Chip, GameAction, main_action, page_rows};

/// A hint: the buttons (one, or a pair such as LB RB) and what they do.
type Hint = (Vec<GamepadInput>, SharedString);

impl BigPictureView {
    pub(super) fn downloads_summary(cx: &App) -> Option<SharedString> {
        let m = models(cx);
        let queue = &m.downloads.read(cx).queue;
        if queue.paused && !queue.active.is_empty() {
            return Some(t_args(
                "bigpicture.downloads.paused",
                &[("count", queue.active.len().to_string().as_str())],
            ));
        }
        let current = queue.current()?;
        let waiting = queue.active.len().saturating_sub(1);
        let pct = m
            .statuses
            .read(cx)
            .get(&current.id)
            .and_then(|s| s.progress)
            .and_then(|p| p.percent)
            .map(|p| format!("{p:.0}"));
        let title = current.title.as_str();
        let count = waiting.to_string();
        Some(match (pct, waiting) {
            (None, 0) => SharedString::from(current.title.clone()),
            (Some(p), 0) => t_args(
                "bigpicture.downloads.progress",
                &[("title", title), ("percent", p.as_str())],
            ),
            (None, n) => t_args(
                if n == 1 {
                    "bigpicture.downloads.queued_one"
                } else {
                    "bigpicture.downloads.queued_other"
                },
                &[("title", title), ("count", count.as_str())],
            ),
            (Some(p), n) => t_args(
                if n == 1 {
                    "bigpicture.downloads.progress_queued_one"
                } else {
                    "bigpicture.downloads.progress_queued_other"
                },
                &[
                    ("title", title),
                    ("percent", p.as_str()),
                    ("count", count.as_str()),
                ],
            ),
        })
    }

    fn render_tab(&self, tab: Tab, c: Colors, cx: &mut Context<Self>) -> AnyElement {
        let active = self.tab == tab && self.page.is_none();
        let current = self.tab == tab;
        v_flex()
            .id(match tab {
                Tab::Home => "bp-tab-home",
                Tab::Library => "bp-tab-library",
            })
            .role(Role::Tab)
            .aria_selected(current)
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .pt_1()
            .cursor_pointer()
            .text_xl()
            .map(|d| {
                if current {
                    d.font_bold().text_color(c.foreground)
                } else {
                    d.text_color(c.muted_foreground)
                }
            })
            .child(tab.label())
            .child(
                div()
                    .h(rems(0.2))
                    .w_full()
                    .rounded_full()
                    .when(active, |d| {
                        d.child(irori_ui::motion::fade_in(
                            SharedString::from(format!("bp-tab-line-{tab:?}")),
                            div().size_full().rounded_full().bg(c.foreground),
                        ))
                    }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.page = None;
                this.set_tab(tab, cx);
                this.focus.focus(window, cx);
            }))
            .into_any_element()
    }

    pub(super) fn render_top_bar(
        &self,
        c: Colors,
        metrics: Metrics,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let kind = active_kind(cx);
        let controller = gamepad::model(cx).and_then(|p| p.read(cx).primary().cloned());
        h_flex()
            .w_full()
            .flex_shrink_0()
            .h(rems(metrics.top_bar))
            .px(rems(metrics.gutter))
            .gap_5()
            .items_center()
            .map(|d| {
                if self.page.is_some() {
                    // The mouse's way off a page; Back and Esc do the same.
                    d.child(
                        Button::new("bp-page-back")
                            .ghost()
                            .large()
                            .flex_shrink_0()
                            .icon(IconName::ArrowLeft)
                            .label(t("bigpicture.hint.back"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.settings.is_some() {
                                    this.close_settings(window, cx);
                                    return;
                                }
                                this.overlay = None;
                                this.page = None;
                                this.after_focus_change(cx);
                                this.focus.focus(window, cx);
                            })),
                    )
                } else {
                    d.child(BrandMark::new(rems(1.75)))
                }
            })
            .child(
                h_flex()
                    .id("bp-tabs")
                    .role(Role::TabList)
                    .flex_shrink_0()
                    .gap_4()
                    .items_center()
                    .child(glyph(GamepadInput::PrevPage, kind, c, cx))
                    .children(Tab::ALL.map(|tab| self.render_tab(tab, c, cx)))
                    .child(glyph(GamepadInput::NextPage, kind, c, cx)),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .justify_end()
                    .gap_6()
                    .text_base()
                    .when_some(Self::downloads_summary(cx), |d, s| {
                        d.child(
                            h_flex()
                                .id("bp-downloads")
                                .role(Role::Status)
                                .min_w_0()
                                .gap_2()
                                .text_color(c.muted_foreground)
                                .child(Icon::new(IconName::Download))
                                .child(div().min_w_0().truncate().child(s)),
                        )
                    })
                    .when_some(controller, |d, info| {
                        let battery_icon = match info.battery {
                            gamepad::Battery::Charging(_) => Some(IconName::BatteryCharging),
                            gamepad::Battery::Full => Some(IconName::BatteryFull),
                            b if b.is_low() => Some(IconName::BatteryLow),
                            gamepad::Battery::Discharging(_) => Some(IconName::BatteryMedium),
                            _ => None,
                        };
                        let name = info.kind.label();
                        let aria = match info.battery.label() {
                            Some(battery) => t_args(
                                "bigpicture.controller.connected_battery",
                                &[("controller", name.as_str()), ("battery", battery.as_str())],
                            ),
                            None => t_args(
                                "bigpicture.controller.connected",
                                &[("controller", name.as_str())],
                            ),
                        };
                        let low = info.battery.is_low();
                        d.child(
                            h_flex()
                                .id("bp-controller")
                                .role(Role::Status)
                                .aria_label(aria)
                                .flex_shrink_0()
                                .gap_2()
                                .text_color(if low { c.danger } else { c.muted_foreground })
                                .child(Icon::new(IconName::Gamepad2))
                                .when_some(battery_icon, |d, icon| d.child(Icon::new(icon)))
                                .when_some(info.battery.label(), |d, l| d.child(l)),
                        )
                    })
                    .child({
                        let clock = clock_text();
                        div()
                            .id("bp-clock")
                            .role(Role::Timer)
                            .aria_label(clock.clone())
                            .flex_shrink_0()
                            .text_xl()
                            .font_semibold()
                            .child(clock)
                    })
                    .child(
                        Button::new("bp-exit")
                            .ghost()
                            .large()
                            .flex_shrink_0()
                            .icon(IconName::X)
                            .tooltip(t("bigpicture.exit"))
                            .accessibility_label(t("bigpicture.exit"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.overlay = None;
                                crate::exit_big_picture(cx);
                            })),
                    ),
            )
    }

    /// The hints for what is focused now.
    pub(super) fn hints(&self, cx: &App) -> Vec<Hint> {
        use GamepadInput::*;
        let one = |input: GamepadInput, key: &str| (vec![input], t(key));
        let other_tab = match self.tab {
            Tab::Home => Tab::Library,
            Tab::Library => Tab::Home,
        };
        let main_hint = |out: &mut Vec<Hint>, game: Option<&irori_core::GameInfo>| {
            if let Some(game) = game {
                let state = Self::game_state(game, cx);
                match main_action(state) {
                    GameAction::Play => out.push(one(Alt, "bigpicture.hint.play")),
                    GameAction::Install if state.installable => {
                        out.push(one(Alt, "bigpicture.hint.install"))
                    }
                    _ => {}
                }
            }
        };
        if self.library.searching || self.log_filter.editing {
            return vec![one(Confirm, "bigpicture.hint.done")];
        }
        if self.settings.is_some() {
            return vec![
                (vec![Up, Down], t("bigpicture.hint.choose")),
                one(Confirm, "bigpicture.hint.select"),
                one(Back, "bigpicture.hint.back"),
            ];
        }
        match &self.overlay {
            Some(
                Overlay::Uninstall { .. } | Overlay::DeleteTheme { .. } | Overlay::Power { .. },
            ) => {
                return vec![
                    one(Confirm, "bigpicture.hint.select"),
                    one(Back, "bigpicture.hint.cancel"),
                ];
            }
            Some(Overlay::Menu { .. }) => {
                return vec![
                    one(Confirm, "bigpicture.hint.select"),
                    one(Back, "bigpicture.hint.close"),
                ];
            }
            None => {}
        }
        if let Some(editor) = &self.look.editor {
            if editor.naming {
                return vec![one(Confirm, "bigpicture.hint.done")];
            }
            let rows = editor.rows();
            let row = rows.get(editor.ix.min(rows.len().saturating_sub(1)));
            let mut out = vec![(vec![Up, Down], t("bigpicture.hint.choose"))];
            if row.is_some_and(|r| r.has_value()) {
                out.push((vec![Left, Right], t("bigpicture.hint.change")));
            } else {
                out.push(one(Confirm, "bigpicture.hint.select"));
            }
            out.push(one(Back, "bigpicture.hint.done"));
            return out;
        }
        let mut out = Vec::new();
        if let Some(page) = &self.page {
            out.push(one(Confirm, "bigpicture.hint.select"));
            if let Some(game) = self.page_game(cx) {
                let state = Self::game_state(&game, cx);
                let rows = page_rows(
                    state,
                    crate::model::install_platforms(&game).len(),
                    self.show_log,
                );
                let row = rows.get(page.ix.min(rows.len().saturating_sub(1)));
                if row.is_some_and(|r| r.has_value()) {
                    out.push((vec![Left, Right], t("bigpicture.hint.change")));
                }
                main_hint(&mut out, Some(&game));
            }
            if self.log.is_some() {
                out.push((vec![PrevPage, NextPage], t("bigpicture.hint.scroll_log")));
            }
            out.push(one(Back, "bigpicture.hint.back"));
            out.push(one(Menu, "bigpicture.hint.menu"));
            return out;
        }
        match (self.tab, self.library.focus) {
            (Tab::Library, LibraryFocus::Chips(ix)) => {
                let search = Chip::ALL.get(ix) == Some(&Chip::Search);
                out.push(one(
                    Confirm,
                    if search {
                        "bigpicture.hint.search"
                    } else {
                        "bigpicture.hint.change"
                    },
                ));
            }
            _ => {
                out.push(one(Confirm, "bigpicture.hint.details"));
                main_hint(&mut out, self.tab_game().map(|g| &**g));
            }
        }
        out.push((vec![PrevPage, NextPage], other_tab.label()));
        out.push(one(Menu, "bigpicture.hint.menu"));
        out.push(match self.tab {
            Tab::Home => one(Back, "bigpicture.hint.exit"),
            Tab::Library => one(Back, "bigpicture.hint.back"),
        });
        out
    }

    pub(super) fn render_hints(
        &self,
        c: Colors,
        metrics: Metrics,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let kind = active_kind(cx);
        let hints = self.hints(cx);
        h_flex()
            .id("bp-hints")
            .w_full()
            .flex_shrink_0()
            .h(rems(metrics.hints))
            .px(rems(metrics.gutter))
            .gap_6()
            .items_center()
            .overflow_hidden()
            .border_t_1()
            .border_color(c.border.opacity(0.5))
            .bg(c.background.opacity(0.55))
            .text_base()
            .children(hints.into_iter().enumerate().map(|(i, (inputs, label))| {
                let first = inputs[0];
                h_flex()
                    .id(("bp-hint", i))
                    .role(Role::Button)
                    .aria_label(label.clone())
                    .flex_shrink_0()
                    .gap_2()
                    .items_center()
                    .cursor_pointer()
                    .children(inputs.into_iter().map(|input| glyph(input, kind, c, cx)))
                    .child(div().text_color(c.foreground.opacity(0.85)).child(label))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.library.searching {
                            this.close_search(window, cx);
                        } else if this.log_filter.editing {
                            this.close_log_filter(window, cx);
                        } else {
                            this.on_input(first, window, cx);
                        }
                    }))
            }))
    }
}

/// The family of the controller in hand, `None` for the keyboard.
fn active_kind(cx: &App) -> Option<ControllerKind> {
    gamepad::model(cx).and_then(|m| m.read(cx).active_kind())
}

/// A button as printed on the controller of `kind`, or a key without one.
pub(super) fn glyph(
    input: GamepadInput,
    kind: Option<ControllerKind>,
    c: Colors,
    cx: &App,
) -> AnyElement {
    let size = rems(1.75);
    let Some(kind) = kind else {
        return keycap(input, c);
    };
    let face = matches!(
        input,
        GamepadInput::Confirm | GamepadInput::Back | GamepadInput::Alt | GamepadInput::Menu
    );
    if !face {
        return match input {
            GamepadInput::Left | GamepadInput::Right | GamepadInput::Up | GamepadInput::Down => {
                arrow(input, c)
            }
            _ => div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .h(rems(1.5))
                .min_w(rems(2.25))
                .px_1p5()
                .rounded(c.radius)
                .bg(c.foreground.opacity(0.14))
                .text_xs()
                .font_bold()
                .text_color(c.foreground)
                .child(kind.glyph(input))
                .into_any_element(),
        };
    }
    let playstation = matches!(
        kind,
        ControllerKind::PlayStation4 | ControllerKind::PlayStation5
    );
    let colour = face_colour(input, kind, c);
    let on = c.foreground.opacity(0.14);
    // PlayStation prints coloured symbols on dark buttons; Xbox-style pads
    // colour the button itself; Nintendo's are plain.
    let (fill, text) = match colour {
        Some(colour) if playstation => (on, legible(colour, &[c.background], ICON_CONTRAST, cx)),
        Some(colour) => (colour, legible(c.background, &[colour], TEXT_CONTRAST, cx)),
        None => (c.foreground.opacity(0.85), c.background),
    };
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(size)
        .rounded_full()
        .bg(fill)
        .text_sm()
        .font_extrabold()
        .text_color(text)
        .child(kind.glyph(input))
        .into_any_element()
}

/// Colour of a face button, `None` for Nintendo's plain ones.
fn face_colour(input: GamepadInput, kind: ControllerKind, c: Colors) -> Option<Hsla> {
    if kind == ControllerKind::Nintendo {
        return None;
    }
    let playstation = matches!(
        kind,
        ControllerKind::PlayStation4 | ControllerKind::PlayStation5
    );
    Some(match input {
        GamepadInput::Confirm if playstation => c.blue,
        GamepadInput::Alt if playstation => c.magenta,
        GamepadInput::Menu if playstation => c.green,
        GamepadInput::Confirm => c.green,
        GamepadInput::Back => c.red,
        GamepadInput::Alt => c.blue,
        _ => c.yellow,
    })
}

fn arrow(input: GamepadInput, c: Colors) -> AnyElement {
    let icon = match input {
        GamepadInput::Left => IconName::ChevronLeft,
        GamepadInput::Right => IconName::ChevronRight,
        GamepadInput::Up => IconName::ChevronUp,
        _ => IconName::ChevronDown,
    };
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(rems(1.5))
        .rounded(c.radius)
        .bg(c.foreground.opacity(0.14))
        .text_color(c.foreground)
        .child(Icon::new(icon).small())
        .into_any_element()
}

/// A keyboard key for `input` (the keys bound in [`crate::bind_keys`]).
fn keycap(input: GamepadInput, c: Colors) -> AnyElement {
    let label = match input {
        GamepadInput::Confirm => "Enter",
        GamepadInput::Back => "Esc",
        GamepadInput::Alt => "Space",
        GamepadInput::Menu => "M",
        GamepadInput::PrevPage => "PgUp",
        GamepadInput::NextPage => "PgDn",
        _ => return arrow(input, c),
    };
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .h(rems(1.5))
        .min_w(rems(1.75))
        .px_1p5()
        .rounded(c.radius)
        .border_1()
        .border_color(c.foreground.opacity(0.3))
        .bg(c.foreground.opacity(0.1))
        .text_xs()
        .font_bold()
        .text_color(c.foreground)
        .child(label)
        .into_any_element()
}
