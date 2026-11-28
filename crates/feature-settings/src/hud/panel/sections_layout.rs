use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::select::Select;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable, Sizable as _, StyledExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AnyElement, App, Context, Entity, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, px, rems, rgb,
};
use irori_core::hud::{
    DEFAULT_FPS_COLOR_HIGH, DEFAULT_FPS_COLOR_LOW, DEFAULT_TOGGLE_HUD, DEFAULT_TOGGLE_PRESET,
};
use irori_core::{HudColorPalette, HudOrientation, HudPreset, ThemeHudColors};

use crate::widgets::{RowState, field_row, group_heading, group_panel};

use super::HudPanel;
use super::helpers::{
    ColorSlot, choice_button, compact_slider_cell, metric_switch_row, palette_dots,
};
use super::position_grid::{position_label_index, render_mangohud_position_grid};

type SwitchChange = Box<dyn Fn(&bool, &mut Window, &mut App)>;
type ClickAction = Box<dyn Fn(&gpui_kit::ClickEvent, &mut Window, &mut App)>;

const fn cycle_pill_id(preset: HudPreset) -> &'static str {
    match preset {
        HudPreset::MinimalFps => "hud-cycle-pill-1",
        HudPreset::HorizontalCompact => "hud-cycle-pill-2",
        HudPreset::BalancedGaming => "hud-cycle-pill-3",
        HudPreset::FullDiagnostics => "hud-cycle-pill-4",
        HudPreset::Custom => "hud-cycle-pill-5",
        HudPreset::CustomHorizontal => "hud-cycle-pill-6",
    }
}

/// Clickable swatch + slot label trigger for the HUD color wheel popover so
/// clicking either the swatch or the name opens the picker.
#[derive(IntoElement)]
struct ColorTileTrigger {
    id: SharedString,
    color: Hsla,
    label: SharedString,
    tooltip: SharedString,
    selected: bool,
}

impl ColorTileTrigger {
    fn new(
        id: impl Into<SharedString>,
        color: Hsla,
        label: SharedString,
        tooltip: SharedString,
    ) -> Self {
        Self {
            id: id.into(),
            color,
            label,
            tooltip,
            selected: false,
        }
    }
}

impl Selectable for ColorTileTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for ColorTileTrigger {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let border = if self.selected {
            theme.ring
        } else {
            theme.border
        };
        let hover_fg = theme.foreground;
        let tip = self.tooltip;

        h_flex()
            .id(self.id)
            .w_full()
            .min_w_0()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .child(
                div()
                    .w(px(20.))
                    .h(px(20.))
                    .flex_shrink_0()
                    .rounded(px(5.))
                    .bg(self.color)
                    .when(self.selected, |d| d.border_2())
                    .when(!self.selected, |d| d.border_1())
                    .border_color(border)
                    .hover(move |s| s.border_color(hover_fg)),
            )
            .child(
                div()
                    .text_xs()
                    .font_medium()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(self.label),
            )
    }
}

impl HudPanel {
    pub(super) fn has_shortcut_conflict(&self) -> bool {
        let p = self.active_mangohud();
        let hud = match p.toggle_hud.trim() {
            "" => DEFAULT_TOGGLE_HUD,
            s => s,
        };
        let preset = match p.toggle_preset.trim() {
            "" => DEFAULT_TOGGLE_PRESET,
            s => s,
        };
        hud.eq_ignore_ascii_case(preset)
    }

    pub(super) fn render_layout_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let (border, muted_fg, bg, primary_fg, warning, radius) = {
            let t = cx.theme();
            (
                t.border,
                t.muted_foreground,
                t.background,
                t.primary_foreground,
                t.warning,
                t.radius,
            )
        };
        let p = self.active_mangohud();
        let font_def = (p.font_scale - 1.0).abs() < 0.01;
        let corner_def = p.round_corners == 8;
        let bg_def = (p.background_alpha - 0.5).abs() < 0.01;
        let alpha_def = (p.alpha - 1.0).abs() < 0.01;
        let layout_def =
            font_def && corner_def && bg_def && alpha_def && !p.hud_compact && !p.hud_no_margin;

        let pos_name = self.labels.position_labels[position_label_index(p.position)].clone();
        let weak_pos = cx.entity().downgrade();
        let pos_grid = render_mangohud_position_grid(
            p.position,
            p.orientation,
            &self.labels.position_labels,
            move |pos, _, window, cx| {
                let _ = weak_pos.update(cx, |this, cx| {
                    this.edit_mangohud(window, cx, |p| p.position = pos)
                });
            },
            cx,
        );

        let is_vertical = p.orientation == HudOrientation::Vertical;
        let orient_group = ButtonGroup::new("hud-orientation-group")
            .outline()
            .small()
            .child(
                Button::new("hud-orientation-vertical")
                    .icon(IconName::PanelLeft)
                    .label(self.labels.orientation_vertical.clone())
                    .selected(is_vertical),
            )
            .child(
                Button::new("hud-orientation-horizontal")
                    .icon(IconName::PanelTop)
                    .label(self.labels.orientation_horizontal.clone())
                    .selected(!is_vertical),
            )
            .on_click(cx.listener(|this, clicked: &Vec<usize>, window, cx| {
                match clicked.first().copied() {
                    Some(0) => this
                        .edit_mangohud(window, cx, |p| p.set_orientation(HudOrientation::Vertical)),
                    Some(1) => this.edit_mangohud(window, cx, |p| {
                        p.set_orientation(HudOrientation::Horizontal)
                    }),
                    _ => {}
                }
            }));

        let switch_card = |id: &'static str,
                           label: SharedString,
                           help: SharedString,
                           checked: bool,
                           on_change: SwitchChange| {
            h_flex()
                .flex_1()
                .min_w(rems(11.5))
                .items_center()
                .justify_between()
                .gap_3()
                .px_3()
                .py_2()
                .rounded(radius)
                .bg(bg)
                .border_1()
                .border_color(border)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .text_xs()
                                .font_medium()
                                .truncate()
                                .child(label.clone()),
                        )
                        .child(div().text_xs().text_color(muted_fg).truncate().child(help)),
                )
                .child(
                    Switch::new(id)
                        .checked(checked)
                        .accessibility_label(label)
                        .on_change(move |on, window, cx| on_change(on, window, cx)),
                )
        };

        let spacing_cards = h_flex()
            .w_full()
            .flex_wrap()
            .gap_2p5()
            .child(switch_card(
                "hud-layout-compact",
                self.labels.hud_compact_label.clone(),
                self.labels.hud_compact_help.clone(),
                p.hud_compact,
                Box::new(cx.listener(|this, on: &bool, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.hud_compact = *on;
                        p.mark_custom();
                    });
                })),
            ))
            .child(switch_card(
                "hud-layout-no-margin",
                self.labels.hud_no_margin_label.clone(),
                self.labels.hud_no_margin_help.clone(),
                p.hud_no_margin,
                Box::new(cx.listener(|this, on: &bool, window, cx| {
                    this.edit_mangohud(window, cx, |p| p.hud_no_margin = *on);
                })),
            ));

        let pos_orient_row = h_flex()
            .w_full()
            .flex_wrap()
            .items_start()
            .gap_6()
            .px_4()
            .py_3p5()
            .child(
                v_flex()
                    .flex_shrink_0()
                    .gap_2()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .child(self.labels.position_label.clone()),
                            )
                            .child(div().text_xs().text_color(muted_fg).child(pos_name)),
                    )
                    .child(pos_grid),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w(rems(16.))
                    .gap_3()
                    .child(
                        h_flex()
                            .w_full()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_medium()
                                            .child(self.labels.orientation_label.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_fg)
                                            .child(self.labels.orientation_help.clone()),
                                    ),
                            )
                            .child(orient_group),
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .pt_3()
                            .border_t_1()
                            .border_color(border)
                            .gap_2()
                            .child(irori_ui::style::caption(
                                self.labels.layout_spacing_label.clone(),
                                cx,
                            ))
                            .child(spacing_cards),
                    ),
            );

        let rst_tip = self.labels.layout_reset_slider.clone();
        let sliders_grid = v_flex()
            .w_full()
            .px_4()
            .py_3p5()
            .gap_4()
            .child(
                h_flex()
                    .w_full()
                    .gap_8()
                    .child(compact_slider_cell(
                        self.labels.font_scale_label.clone(),
                        self.display.font_scale.clone(),
                        &self.font_scale_slider,
                        font_def,
                        "hud-slider-reset-font-scale",
                        rst_tip.clone(),
                        cx.listener(|this, _, w, cx| {
                            this.edit_mangohud(w, cx, |p| p.font_scale = 1.0)
                        }),
                        cx,
                    ))
                    .child(compact_slider_cell(
                        self.labels.round_corners_label.clone(),
                        self.display.round_corners.clone(),
                        &self.round_corners_slider,
                        corner_def,
                        "hud-slider-reset-round-corners",
                        rst_tip.clone(),
                        cx.listener(|this, _, w, cx| {
                            this.edit_mangohud(w, cx, |p| p.round_corners = 8)
                        }),
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_8()
                    .child(compact_slider_cell(
                        self.labels.background_alpha_label.clone(),
                        self.display.background_alpha.clone(),
                        &self.bg_alpha_slider,
                        bg_def,
                        "hud-slider-reset-bg-alpha",
                        rst_tip.clone(),
                        cx.listener(|this, _, w, cx| {
                            this.edit_mangohud(w, cx, |p| p.background_alpha = 0.5)
                        }),
                        cx,
                    ))
                    .child(compact_slider_cell(
                        self.labels.alpha_label.clone(),
                        self.display.alpha.clone(),
                        &self.alpha_slider,
                        alpha_def,
                        "hud-slider-reset-alpha",
                        rst_tip,
                        cx.listener(|this, _, w, cx| this.edit_mangohud(w, cx, |p| p.alpha = 1.0)),
                        cx,
                    )),
            );

        let layout_section = v_flex()
            .w_full()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .pt_2()
                    .pb_1()
                    .child(irori_ui::style::caption(
                        self.labels.group_overlay_layout.clone(),
                        cx,
                    ))
                    .child(
                        Button::new("hud-layout-reset")
                            .ghost()
                            .xsmall()
                            .icon(IconName::RotateCcw)
                            .label(self.labels.layout_reset.clone())
                            .disabled(layout_def)
                            .on_click(
                                cx.listener(|this, _, w, cx| this.reset_layout_defaults(w, cx)),
                            ),
                    ),
            )
            .child(
                group_panel(cx)
                    .child(pos_orient_row)
                    .child(div().border_t_1().border_color(border).child(sliders_grid)),
            );

        // Card 2: SHORTCUTS AND CYCLING
        let shortcut_row = field_row(
            RowState {
                label: self.labels.toggle_hud_label.clone(),
                help: self.labels.toggle_hud_help.clone(),
                ..Default::default()
            },
            self.shortcut.clone().into_any_element(),
            cx,
        );
        let start_hidden_row = metric_switch_row(
            "hud-layout-no-display",
            self.labels.no_display_label.clone(),
            self.display.start_hidden_help.clone(),
            p.no_display,
            cx.listener(|this, on: &bool, w, cx| this.edit_mangohud(w, cx, |p| p.no_display = *on)),
            cx,
        );
        let preset_shortcut_row = field_row(
            RowState {
                label: self.labels.toggle_preset_label.clone(),
                help: self.labels.toggle_preset_help.clone(),
                ..Default::default()
            },
            self.preset_shortcut.clone().into_any_element(),
            cx,
        );

        let conflict = self.has_shortcut_conflict();
        let mut cycle_pills = h_flex().w_full().flex_wrap().items_center().gap_1p5();
        for (ix, preset) in p.preset_cycle().into_iter().enumerate() {
            if ix > 0 {
                cycle_pills = cycle_pills.child(div().text_xs().text_color(muted_fg).child("→"));
            }
            let is_start = ix == 0;
            let name = self.labels.preset_name(preset).clone();
            let btn = if is_start {
                Button::new(cycle_pill_id(preset)).primary()
            } else {
                Button::new(cycle_pill_id(preset)).outline()
            };
            cycle_pills = cycle_pills.child(
                btn.xsmall()
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.edit_mangohud(w, cx, |p| p.apply_preset(preset));
                    }))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .when(is_start, |r| {
                                r.child(div().size(px(6.)).rounded_full().bg(primary_fg))
                            })
                            .child(div().text_xs().child(name)),
                    ),
            );
        }

        let cycle_footer = v_flex()
            .w_full()
            .px_4()
            .pb_3()
            .gap_2()
            .when(conflict, |v| {
                v.child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .rounded(radius)
                        .bg(bg)
                        .border_1()
                        .border_color(warning)
                        .text_xs()
                        .text_color(warning)
                        .child(Icon::new(IconName::TriangleAlert).xsmall())
                        .child(div().flex_1().child(self.labels.shortcut_conflict.clone())),
                )
            })
            .child(
                v_flex()
                    .w_full()
                    .px_3()
                    .py_2p5()
                    .gap_2()
                    .rounded(radius)
                    .bg(bg)
                    .border_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .w_full()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .child(self.labels.cycle_order_title.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .child(self.labels.cycle_order_hint.clone()),
                            ),
                    )
                    .child(cycle_pills),
            );

        let wayland_footer = h_flex()
            .w_full()
            .items_center()
            .gap_2()
            .px_4()
            .py_2p5()
            .bg(bg.opacity(0.45))
            .text_xs()
            .text_color(muted_fg)
            .child(Icon::new(IconName::Info).xsmall())
            .child(div().flex_1().child(self.labels.wayland_note.clone()));

        let shortcuts_section = v_flex()
            .w_full()
            .child(group_heading(
                self.labels.group_shortcuts_cycling.clone(),
                cx,
            ))
            .child(
                group_panel(cx)
                    .child(shortcut_row)
                    .child(
                        div()
                            .border_t_1()
                            .border_color(border)
                            .child(start_hidden_row),
                    )
                    .child(
                        div().border_t_1().border_color(border).child(
                            v_flex()
                                .w_full()
                                .child(preset_shortcut_row)
                                .child(cycle_footer),
                        ),
                    )
                    .child(
                        div()
                            .border_t_1()
                            .border_color(border)
                            .child(wayland_footer),
                    ),
            );

        v_flex()
            .w_full()
            .gap_2()
            .child(layout_section)
            .child(shortcuts_section)
            .into_any_element()
    }

    pub(super) fn render_hardware_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let (border, muted_fg) = (cx.theme().border, cx.theme().muted_foreground);
        let profile = self.active_mangohud();

        let gpu_select_row = field_row(
            RowState {
                label: self.labels.gpu_selection_label.clone(),
                help: self.labels.gpu_selection_help.clone(),
                ..Default::default()
            },
            Select::new(&self.gpu_select)
                .id("hud-gpu-select")
                .accessibility_label(self.labels.gpu_selection_label.clone())
                .w_64()
                .into_any_element(),
            cx,
        );

        let label_col = |title: SharedString,
                         input: &Entity<InputState>,
                         reset_id: &'static str,
                         empty: bool,
                         on_reset: ClickAction| {
            v_flex()
                .flex_1()
                .min_w(rems(13.))
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_medium()
                        .text_color(muted_fg)
                        .child(title),
                )
                .child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .child(Input::new(input).small().flex_1())
                        .child(
                            Button::new(reset_id)
                                .ghost()
                                .xsmall()
                                .icon(IconName::RotateCcw)
                                .tooltip(self.labels.label_reset.clone())
                                .disabled(empty)
                                .on_click(move |ev, w, cx| on_reset(ev, w, cx)),
                        ),
                )
        };

        let labels_section = v_flex()
            .w_full()
            .px_4()
            .py_3()
            .gap_3()
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .child(self.labels.labels_override_label.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(self.labels.labels_override_help.clone()),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .gap_4()
                    .child(label_col(
                        self.labels.gpu_label_label.clone(),
                        &self.gpu_label_input,
                        "hud-gpu-label-reset",
                        profile.gpu_label_override.is_empty(),
                        Box::new(cx.listener(|this, _, w, cx| {
                            this.gpu_label_input
                                .update(cx, |s, cx| s.set_value("", w, cx));
                            this.edit_mangohud(w, cx, |p| p.gpu_label_override.clear());
                        })),
                    ))
                    .child(label_col(
                        self.labels.cpu_label_label.clone(),
                        &self.cpu_label_input,
                        "hud-cpu-label-reset",
                        profile.cpu_label_override.is_empty(),
                        Box::new(cx.listener(|this, _, w, cx| {
                            this.cpu_label_input
                                .update(cx, |s, cx| s.set_value("", w, cx));
                            this.edit_mangohud(w, cx, |p| p.cpu_label_override.clear());
                        })),
                    )),
            );

        v_flex()
            .w_full()
            .child(group_heading(self.labels.group_hardware.clone(), cx))
            .child(
                group_panel(cx).child(gpu_select_row).child(
                    div()
                        .border_t_1()
                        .border_color(border)
                        .child(labels_section),
                ),
            )
            .into_any_element()
    }

    pub(super) fn render_colors_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let (border, bg, muted_fg, primary, success, warning, radius, mono_font) = {
            let t = cx.theme();
            (
                t.border,
                t.background,
                t.muted_foreground,
                t.primary,
                t.success,
                t.warning,
                t.radius,
                t.mono_font_family.clone(),
            )
        };
        let p = self.active_mangohud();
        let custom_colors_default = p.custom_colors == ThemeHudColors::default();

        let palette_header = h_flex()
            .w_full()
            .flex_wrap()
            .items_start()
            .justify_between()
            .gap_3()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .child(self.labels.palette_label.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(self.labels.palette_help.clone()),
                    ),
            )
            .when(p.palette == HudColorPalette::Custom, |row| {
                row.child(
                    Button::new("hud-colors-reset-all")
                        .ghost()
                        .xsmall()
                        .icon(IconName::RotateCcw)
                        .disabled(custom_colors_default)
                        .label(self.labels.color_reset_all.clone())
                        .on_click(cx.listener(|this, _, w, cx| this.reset_custom_colors(w, cx))),
                )
            });

        let palette_specs = [
            (
                "hud-palette-default",
                &self.labels.palette_mangohud_default,
                HudColorPalette::MangoHudDefault,
                0,
            ),
            (
                "hud-palette-sync",
                &self.labels.palette_sync_irori_theme,
                HudColorPalette::SyncIroriTheme,
                1,
            ),
            (
                "hud-palette-monochrome",
                &self.labels.palette_monochrome,
                HudColorPalette::Monochrome,
                2,
            ),
            (
                "hud-palette-custom",
                &self.labels.palette_custom,
                HudColorPalette::Custom,
                3,
            ),
        ];
        let mut palette_buttons = h_flex().w_full().flex_wrap().gap_2();
        for (id, label, palette, dot_ix) in palette_specs {
            palette_buttons = palette_buttons.child(
                choice_button(
                    id,
                    label.clone(),
                    p.palette == palette,
                    cx.listener(move |this, _, w, cx| {
                        if palette != HudColorPalette::Custom {
                            this.open_color_slot = None;
                        }
                        this.edit_mangohud(w, cx, |p| p.palette = palette);
                    }),
                )
                .flex_1()
                .min_w(rems(9.5))
                .child(palette_dots(self.display.palette_dots[dot_ix], border)),
            );
        }

        let mut panel = group_panel(cx).child(
            v_flex()
                .w_full()
                .px_4()
                .py_3()
                .gap_3()
                .child(palette_header)
                .child(palette_buttons),
        );

        if p.palette == HudColorPalette::Custom {
            let render_tile = |slot: ColorSlot, cx: &mut Context<Self>| {
                let ix = slot as usize;
                let swatch = self.display.custom_swatches[ix];
                let wheel = self.color_wheel.clone();
                let is_open = self.open_color_slot == Some(slot);
                let popover = Popover::new(slot.popover_id())
                    .anchor(Anchor::TopLeft)
                    .open(is_open)
                    .trigger(ColorTileTrigger::new(
                        slot.trigger_id(),
                        swatch,
                        self.labels.color_slot_labels[ix].clone(),
                        self.labels.color_slot_tips[ix].clone(),
                    ))
                    .on_open_change(cx.listener(move |this, open: &bool, w, cx| {
                        this.on_color_popover_change(slot, *open, w, cx);
                    }))
                    .content(move |_, _, _| wheel.clone());

                h_flex()
                    .id(slot.id())
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .px_2p5()
                    .py_1p5()
                    .rounded(radius)
                    .bg(bg)
                    .border_1()
                    .border_color(if is_open { primary } else { border })
                    .child(div().flex_1().min_w_0().child(popover))
                    .child(
                        Input::new(&self.color_inputs[ix])
                            .small()
                            .prefix(
                                div()
                                    .text_xs()
                                    .font_family(mono_font.clone())
                                    .text_color(muted_fg)
                                    .child("#"),
                            )
                            .w(rems(6.2)),
                    )
            };

            let ratio = self.display.wcag_ratio;
            let low_contrast = self.display.low_contrast_notice.is_some();
            let (grade_text, grade_color) = if ratio >= 7.0 {
                (SharedString::new_static("AAA"), success)
            } else if ratio >= 4.5 {
                (SharedString::new_static("AA"), success)
            } else {
                (self.labels.color_contrast_low_badge.clone(), warning)
            };

            let wcag_card = h_flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .justify_between()
                .gap_2()
                .px_3()
                .py_1p5()
                .rounded(radius)
                .bg(bg)
                .border_1()
                .border_color(if low_contrast { warning } else { border })
                .child(
                    v_flex()
                        .min_w_0()
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_fg)
                                .truncate()
                                .child(self.labels.color_contrast_label.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family(mono_font.clone())
                                .font_semibold()
                                .child(self.display.wcag_ratio_label.clone()),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .bg(grade_color.opacity(0.16))
                        .text_xs()
                        .font_semibold()
                        .text_color(grade_color)
                        .child(grade_text),
                );

            let surface_row = h_flex()
                .w_full()
                .gap_2p5()
                .child(render_tile(ColorSlot::Background, cx))
                .child(render_tile(ColorSlot::Text, cx))
                .child(wcag_card);

            let metric_rows = [
                [ColorSlot::Gpu, ColorSlot::Cpu, ColorSlot::Vram],
                [ColorSlot::Ram, ColorSlot::Engine, ColorSlot::Frametime],
                [ColorSlot::Io, ColorSlot::Battery, ColorSlot::Accent],
            ];
            let mut metrics_grid = v_flex().w_full().gap_2();
            for row_slots in metric_rows {
                metrics_grid = metrics_grid.child(
                    h_flex()
                        .w_full()
                        .gap_2p5()
                        .child(render_tile(row_slots[0], cx))
                        .child(render_tile(row_slots[1], cx))
                        .child(render_tile(row_slots[2], cx)),
                );
            }

            panel = panel.child(
                v_flex()
                    .w_full()
                    .border_t_1()
                    .border_color(border)
                    .px_4()
                    .py_3p5()
                    .gap_3p5()
                    .child(
                        v_flex()
                            .w_full()
                            .gap_1p5()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(muted_fg)
                                    .child(self.labels.color_group_surface.clone()),
                            )
                            .child(surface_row),
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .gap_1p5()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(muted_fg)
                                    .child(self.labels.color_group_metrics.clone()),
                            )
                            .child(metrics_grid),
                    ),
            );
        }

        let (fps_low, fps_high) = p.effective_fps_color_thresholds();
        let thresholds_default =
            fps_low == DEFAULT_FPS_COLOR_LOW && fps_high == DEFAULT_FPS_COLOR_HIGH;
        let dot = |hex: u32| div().size(px(10.)).rounded_full().bg(rgb(hex));
        let dim = |text: SharedString| div().text_xs().text_color(muted_fg).child(text);

        let fps_threshold_row = v_flex()
            .w_full()
            .child(metric_switch_row(
                "hud-fps-color-change",
                self.labels.fps_color_change_label.clone(),
                self.display.fps_threshold_summary.clone(),
                p.fps_color_change,
                cx.listener(|this, on: &bool, w, cx| {
                    this.edit_mangohud(w, cx, |p| p.fps_color_change = *on)
                }),
                cx,
            ))
            .when(p.fps_color_change, |col| {
                col.child(
                    div().w_full().px_4().pb_3().child(
                        h_flex()
                            .w_full()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .px_3()
                            .py_2()
                            .rounded(radius)
                            .bg(bg)
                            .border_1()
                            .border_color(border)
                            .child(
                                h_flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_x_4()
                                    .gap_y_2()
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap_1p5()
                                            .child(dot(0xB22222))
                                            .child(dim(SharedString::new_static("<=")))
                                            .child(
                                                Input::new(&self.fps_threshold_low_input)
                                                    .small()
                                                    .w_16(),
                                            )
                                            .child(dim(SharedString::new_static("FPS"))),
                                    )
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap_1p5()
                                            .child(dot(0xFDFD09))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_family(mono_font.clone())
                                                    .text_color(muted_fg)
                                                    .child(self.display.fps_mid_start.clone()),
                                            )
                                            .child(dim(self.labels.fps_color_change_to.clone()))
                                            .child(
                                                Input::new(&self.fps_threshold_high_input)
                                                    .small()
                                                    .w_16(),
                                            )
                                            .child(dim(SharedString::new_static("FPS"))),
                                    )
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap_1p5()
                                            .child(dot(0x39F900))
                                            .child(dim(SharedString::new_static(">")))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_family(mono_font.clone())
                                                    .font_semibold()
                                                    .child(self.display.fps_high_value.clone()),
                                            )
                                            .child(dim(SharedString::new_static("FPS"))),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_1p5()
                                    .flex_shrink_0()
                                    .child(choice_button(
                                        "hud-fps-threshold-30-60",
                                        SharedString::new_static("30 / 60"),
                                        fps_low == 30 && fps_high == 60,
                                        cx.listener(|this, _, w, cx| {
                                            this.edit_mangohud(w, cx, |p| {
                                                p.set_fps_color_thresholds(30, 60)
                                            })
                                        }),
                                    ))
                                    .child(choice_button(
                                        "hud-fps-threshold-60-120",
                                        SharedString::new_static("60 / 120"),
                                        fps_low == 60 && fps_high == 120,
                                        cx.listener(|this, _, w, cx| {
                                            this.edit_mangohud(w, cx, |p| {
                                                p.set_fps_color_thresholds(60, 120)
                                            })
                                        }),
                                    ))
                                    .child(
                                        Button::new("hud-fps-threshold-reset")
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::RotateCcw)
                                            .disabled(thresholds_default)
                                            .accessibility_label(
                                                self.labels.fps_color_change_reset.clone(),
                                            )
                                            .tooltip(self.labels.fps_color_change_reset.clone())
                                            .on_click(cx.listener(|this, _, w, cx| {
                                                this.edit_mangohud(w, cx, |p| {
                                                    p.set_fps_color_thresholds(
                                                        DEFAULT_FPS_COLOR_LOW,
                                                        DEFAULT_FPS_COLOR_HIGH,
                                                    );
                                                });
                                            })),
                                    ),
                            ),
                    ),
                )
            });

        panel = panel.child(
            div()
                .border_t_1()
                .border_color(border)
                .child(fps_threshold_row),
        );

        v_flex()
            .w_full()
            .child(group_heading(self.labels.group_colors.clone(), cx))
            .child(panel)
            .into_any_element()
    }
}
