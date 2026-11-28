//! Unified Accent colour control: inline quick swatches, a `Default` reset
//! chip, a `Custom` trigger opening Irori's shared circular Hue/Saturation
//! [`ColorWheel`] popover (the same wheel used by Performance HUD), and
//! contextual desktop-accent status/toggle handling.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, StyledExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, App, AppContext as _, Context, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, Window, div, px, rgb,
};
use irori_ui::components::color_wheel::hsla_to_hex6;
use irori_ui::components::{
    ColorWheel, ColorWheelLabels, ColorWheelPick, ColorWheelReset, ColorWheelSwatch,
};
use irori_ui::i18n::t;
use irori_ui::models;
use irori_ui::style::{KeyboardFocusable as _, TEXT_CONTRAST, legible};
use irori_ui::theme::Desktop;

use crate::i18n::LanguageStamp;

use super::{FEATURED_ACCENTS, hex, set_ui, ui_prefs};

fn accent_swatches() -> Vec<ColorWheelSwatch> {
    FEATURED_ACCENTS
        .into_iter()
        .map(|value| {
            let hex = format!("{value:06X}");
            let id = format!("accent-wheel-swatch-{hex}");
            ColorWheelSwatch::from_hex(id, hex, value)
        })
        .collect()
}

fn wheel_labels(cx: &App) -> ColorWheelLabels {
    let desktop = irori_ui::theme::followed_desktop(cx).filter(|d| *d != Desktop::Wallpaper);
    let reset = if desktop.is_some() {
        t("settings.accent.use_desktop")
    } else {
        t("settings.accent.use_theme")
    };
    ColorWheelLabels::standard(t("settings.field.ui_accent_color.label"), reset)
}

fn current_wheel_hex(cx: &App) -> String {
    if let Some(value) = ui_prefs(cx).accent_rgb() {
        format!("{value:06X}")
    } else {
        hsla_to_hex6(cx.theme().primary)
    }
}

/// The unified accent colour control using [`ColorWheel`] for custom picking.
pub(crate) struct AccentPicker {
    wheel: Entity<ColorWheel>,
    popover_open: bool,
    pending_write: Option<Task<()>>,
    language: LanguageStamp,
    _subscriptions: Vec<Subscription>,
}

impl AccentPicker {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let picked = ui_prefs(cx).accent_rgb().is_some();
        let initial_hex = current_wheel_hex(cx);
        let fallback_hex = hsla_to_hex6(cx.theme().primary);
        let bg_hex = hsla_to_hex6(cx.theme().background);
        let labels = wheel_labels(cx);
        let weak = cx.entity().downgrade();

        let wheel = cx.new(|cx| {
            ColorWheel::new(
                &initial_hex,
                &fallback_hex,
                ColorWheelReset::Clear { has_value: picked },
                Some(&bg_hex),
                labels,
                accent_swatches(),
                move |pick, released, window, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.on_wheel_pick(pick, released, window, cx);
                    });
                },
                window,
                cx,
            )
        });

        let settings = models(cx).settings.clone();
        let subscriptions = vec![cx.observe_in(&settings, window, |this, _, window, cx| {
            this.sync(window, cx);
        })];

        Self {
            wheel,
            popover_open: false,
            pending_write: None,
            language: LanguageStamp::new(),
            _subscriptions: subscriptions,
        }
    }

    fn on_wheel_pick(
        &mut self,
        pick: ColorWheelPick,
        released: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match pick {
            ColorWheelPick::Cleared => {
                self.pending_write = None;
                set_ui(window, cx, |ui| ui.accent_color.clear());
                cx.notify();
            }
            ColorWheelPick::Hex(hex6) => {
                let value = format!("#{}", hex6.to_ascii_lowercase());
                if released {
                    self.pending_write = None;
                    set_ui(window, cx, |ui| ui.accent_color = value);
                    cx.notify();
                } else {
                    self.pending_write = Some(cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(80))
                            .await;
                        let _ = this.update_in(cx, |this, window, cx| {
                            this.pending_write = None;
                            set_ui(window, cx, |ui| ui.accent_color = value);
                            cx.notify();
                        });
                    }));
                }
            }
            ColorWheelPick::Named(_) => {}
        }
    }

    /// Follow changes made elsewhere (restored backup, reset, theme switch).
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = ui_prefs(cx).accent_rgb().is_some();
        let current = current_wheel_hex(cx);
        let bg_hex = hsla_to_hex6(cx.theme().background);
        self.wheel.update(cx, |w, cx| {
            w.sync_external_hex(
                &current,
                Some(&bg_hex),
                Some(ColorWheelReset::Clear { has_value: picked }),
                window,
                cx,
            );
        });
        cx.notify();
    }

    fn render_desktop_note(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let prefs = ui_prefs(cx);
        let picked = prefs.accent_rgb().is_some();
        let desktop = irori_ui::theme::followed_desktop(cx);
        let border = cx.theme().border;
        let muted = cx.theme().muted_foreground;

        if !picked
            && prefs.theme_name.is_empty()
            && desktop.is_none()
            && irori_ui::theme::has_system_accent(cx)
        {
            return h_flex()
                .w_full()
                .pt_2()
                .border_t_1()
                .border_color(border)
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_xs()
                                .font_medium()
                                .child(t("settings.field.ui_system_accent.label")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(t("settings.field.ui_system_accent.help")),
                        ),
                )
                .child(
                    Switch::new("field-iroriUi.systemAccent")
                        .checked(prefs.system_accent)
                        .accessibility_label(t("settings.field.ui_system_accent.label"))
                        .on_change(|on: &bool, window, cx| {
                            let on = *on;
                            set_ui(window, cx, |ui| ui.system_accent = on);
                        }),
                )
                .into_any_element();
        }

        let note = if picked {
            t("settings.accent.note_custom")
        } else if desktop == Some(Desktop::Wallpaper) {
            t("settings.accent.note_wallpaper")
        } else if let Some(followed) = desktop
            && prefs.theme_name.is_empty()
        {
            match followed {
                Desktop::Kde => t("settings.accent.note_kde"),
                Desktop::Cosmic => t("settings.accent.note_cosmic"),
                Desktop::Omarchy => t("settings.accent.note_omarchy"),
                Desktop::Wallpaper => t("settings.accent.note_wallpaper"),
            }
        } else {
            t("settings.accent.note_theme")
        };

        div()
            .w_full()
            .pt_2()
            .border_t_1()
            .border_color(border)
            .text_xs()
            .text_color(muted)
            .child(note)
            .into_any_element()
    }
}

impl Render for AccentPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.language.changed() {
            let labels = wheel_labels(cx);
            cx.defer_in(window, move |this, _, cx| {
                this.wheel
                    .update(cx, |w, cx| w.set_labels(labels, true, cx));
            });
        }

        let prefs = ui_prefs(cx);
        let picked_rgb = prefs.accent_rgb();
        let picked = picked_rgb.is_some();
        let desktop = irori_ui::theme::followed_desktop(cx).filter(|d| *d != Desktop::Wallpaper);
        let reset_label = if desktop.is_some() {
            t("settings.accent.use_desktop")
        } else {
            t("settings.accent.use_theme")
        };

        let theme = cx.theme();
        let (primary, bg, sec, border, fg, muted) = (
            theme.primary,
            theme.background,
            theme.secondary,
            theme.border,
            theme.foreground,
            theme.muted_foreground,
        );
        let active_fg = legible(primary, &[sec, bg], TEXT_CONTRAST, cx);
        let hover_bg = theme.secondary_hover;

        let custom_label: SharedString = match picked_rgb {
            Some(v) => SharedString::from(format!("#{v:06X}")),
            None => t("settings.accent.custom"),
        };

        let default_chip = h_flex()
            .id("accent-default-chip")
            .debug_selector(|| "accent-default-chip".into())
            .items_center()
            .px_2p5()
            .py_1()
            .rounded_md()
            .border_1()
            .cursor_pointer()
            .keyboard_focusable(cx)
            .text_xs()
            .when(!picked, |d| {
                d.bg(primary.opacity(0.18))
                    .border_color(primary)
                    .text_color(active_fg)
                    .font_semibold()
            })
            .when(picked, |d| {
                d.bg(bg)
                    .border_color(border)
                    .text_color(muted)
                    .hover(move |s| s.bg(hover_bg).text_color(fg))
            })
            .child(t("settings.accent.default"))
            .on_click(|_, window, cx| {
                set_ui(window, cx, |ui| ui.accent_color.clear());
            });

        let wheel = self.wheel.clone();
        let custom_popover = Popover::new("accent-color-wheel-popover")
            .anchor(Anchor::TopLeft)
            .open(self.popover_open)
            .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                if this.popover_open != *open {
                    this.popover_open = *open;
                    cx.notify();
                }
            }))
            .trigger(
                Button::new("accent-custom-trigger")
                    .debug_selector(|| "accent-custom-trigger".into())
                    .small()
                    .outline()
                    .selected(picked || self.popover_open)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(14.))
                                    .h(px(14.))
                                    .flex_shrink_0()
                                    .rounded(px(4.))
                                    .bg(primary)
                                    .border_1()
                                    .border_color(border),
                            )
                            .child(custom_label),
                    ),
            )
            .content(move |_, _, _| wheel.clone());

        let swatches = FEATURED_ACCENTS.into_iter().map(|value| {
            let swatch_color = rgb(value);
            let is_selected = picked_rgb == Some(value);
            let hex_tip = SharedString::from(format!("#{value:06X}"));
            let id = ElementId::Name(format!("accent-swatch-{value:06x}").into());
            div()
                .id(id)
                .w(px(22.))
                .h(px(22.))
                .flex_shrink_0()
                .rounded_full()
                .bg(swatch_color)
                .cursor_pointer()
                .keyboard_focusable(cx)
                .when(is_selected, |d| d.border_2().border_color(fg))
                .when(!is_selected, |d| {
                    d.border_1()
                        .border_color(border)
                        .hover(move |s| s.border_color(fg))
                })
                .tooltip(move |window, cx| Tooltip::new(hex_tip.clone()).build(window, cx))
                .on_click(move |_, window, cx| {
                    let chosen = hex(swatch_color.into());
                    set_ui(window, cx, |ui| ui.accent_color = chosen);
                })
        });

        v_flex()
            .w_full()
            .gap_2p5()
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(default_chip)
                    .child(custom_popover)
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .flex_wrap()
                            .items_center()
                            .gap_1p5()
                            .children(swatches),
                    )
                    .child(
                        Button::new("accent-default")
                            .ghost()
                            .xsmall()
                            .icon(IconName::RotateCcw)
                            .tooltip(reset_label)
                            .disabled(!picked)
                            .on_click(|_, window, cx| {
                                set_ui(window, cx, |ui| ui.accent_color.clear());
                            }),
                    ),
            )
            .child(self.render_desktop_note(cx))
    }
}
