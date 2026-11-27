use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::select::Select;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Anchor, AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    SharedString, Styled as _, div, px, rems,
};
use irori_core::hud::{DEFAULT_TOGGLE_HUD, DEFAULT_TOGGLE_PRESET};
use irori_core::{HudColorPalette, HudOrientation, HudPreset};

use crate::hud::color_wheel::ColorSwatchTrigger;
use crate::widgets::{RowState, field_row, group_heading, group_panel};

use super::HudPanel;
use super::helpers::{
    COLOR_SLOTS, ColorSlot, choice_button, compact_slider_cell, metric_chip, metric_switch_row,
    palette_dots,
};
use super::position_grid::{position_label_index, render_mangohud_position_grid};

const fn preset_num_badge(preset: HudPreset) -> &'static str {
    match preset {
        HudPreset::MinimalFps => "[1]",
        HudPreset::HorizontalCompact => "[2]",
        HudPreset::BalancedGaming => "[3]",
        HudPreset::FullDiagnostics => "[4]",
        HudPreset::Custom => "[5]",
        HudPreset::CustomHorizontal => "[6]",
    }
}

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

impl HudPanel {
    pub(super) fn has_shortcut_conflict(&self) -> bool {
        let p = self.active_mangohud();
        let hud = p.toggle_hud.trim();
        let hud = if hud.is_empty() {
            DEFAULT_TOGGLE_HUD
        } else {
            hud
        };
        let preset = p.toggle_preset.trim();
        let preset = if preset.is_empty() {
            DEFAULT_TOGGLE_PRESET
        } else {
            preset
        };
        hud.eq_ignore_ascii_case(preset)
    }

    pub(super) fn render_layout_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let (border, muted_fg, bg, warning, radius) = {
            let theme = cx.theme();
            (
                theme.border,
                theme.muted_foreground,
                theme.background,
                theme.warning,
                theme.radius,
            )
        };
        let p = self.active_mangohud();

        // Row 1: 3x3 Visual Position Picker + Orientation ButtonGroup + Quick layout chips
        let pos_name = self.labels.position_labels[position_label_index(p.position)].clone();
        let weak_pos = cx.entity().downgrade();
        let pos_grid = render_mangohud_position_grid(
            p.position,
            &self.labels.position_labels,
            move |pos, _, window, cx| {
                let _ = weak_pos.update(cx, |this, cx| {
                    this.edit_mangohud(window, cx, |p| p.position = pos);
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
                    Some(0) => this.edit_mangohud(window, cx, |p| {
                        p.set_orientation(HudOrientation::Vertical);
                    }),
                    Some(1) => this.edit_mangohud(window, cx, |p| {
                        p.set_orientation(HudOrientation::Horizontal);
                    }),
                    _ => {}
                }
            }));

        let quick_toggles = h_flex()
            .w_full()
            .flex_wrap()
            .gap_1p5()
            .child(metric_chip(
                "hud-layout-compact",
                self.labels.hud_compact_label.clone(),
                self.labels.hud_compact_help.clone(),
                p.hud_compact,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.hud_compact = !p.hud_compact;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-layout-no-margin",
                self.labels.hud_no_margin_label.clone(),
                self.labels.hud_no_margin_help.clone(),
                p.hud_no_margin,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| p.hud_no_margin = !p.hud_no_margin);
                }),
            ))
            .child(metric_chip(
                "hud-layout-no-display",
                self.labels.no_display_label.clone(),
                self.labels.no_display_help.clone(),
                p.no_display,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| p.no_display = !p.no_display);
                }),
            ));

        let pos_orient_row = h_flex()
            .w_full()
            .flex_wrap()
            .items_start()
            .justify_between()
            .gap_6()
            .px_4()
            .py_3()
            .child(
                v_flex()
                    .gap_1p5()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .child(self.labels.position_label.clone()),
                            )
                            .child(div().text_xs().text_color(muted_fg).child(pos_name)),
                    )
                    .child(pos_grid),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w(rems(15.))
                    .gap_3()
                    .child(
                        v_flex()
                            .gap_1p5()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .child(self.labels.orientation_label.clone()),
                            )
                            .child(h_flex().child(orient_group)),
                    )
                    .child(quick_toggles),
            );

        // Row 2: 2x2 grid of compact sliders
        let sliders_grid = v_flex()
            .w_full()
            .px_4()
            .py_3()
            .gap_3()
            .child(
                h_flex()
                    .w_full()
                    .gap_4()
                    .child(compact_slider_cell(
                        self.labels.font_scale_label.clone(),
                        self.display.font_scale.clone(),
                        &self.font_scale_slider,
                        muted_fg,
                    ))
                    .child(compact_slider_cell(
                        self.labels.round_corners_label.clone(),
                        self.display.round_corners.clone(),
                        &self.round_corners_slider,
                        muted_fg,
                    )),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_4()
                    .child(compact_slider_cell(
                        self.labels.background_alpha_label.clone(),
                        self.display.background_alpha.clone(),
                        &self.bg_alpha_slider,
                        muted_fg,
                    ))
                    .child(compact_slider_cell(
                        self.labels.alpha_label.clone(),
                        self.display.alpha.clone(),
                        &self.alpha_slider,
                        muted_fg,
                    )),
            );

        // Row 3: Shortcut recorders (overlay toggle + preset cycling + conflict + cycle strip)
        let shortcut_row = field_row(
            RowState {
                label: self.labels.toggle_hud_label.clone(),
                help: self.labels.toggle_hud_help.clone(),
                ..Default::default()
            },
            self.shortcut.clone().into_any_element(),
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
        let cycle = p.preset_cycle();
        let mut cycle_pills = h_flex().w_full().flex_wrap().items_center().gap_1p5();
        for (ix, preset) in cycle.into_iter().enumerate() {
            if ix > 0 {
                cycle_pills = cycle_pills.child(
                    div()
                        .text_xs()
                        .text_color(muted_fg)
                        .child(SharedString::new_static("→")),
                );
            }
            let is_start = ix == 0;
            let num = SharedString::new_static(preset_num_badge(preset));
            let name = self.labels.preset_name(preset).clone();
            let btn = if is_start {
                Button::new(cycle_pill_id(preset)).primary()
            } else {
                Button::new(cycle_pill_id(preset)).outline()
            };
            cycle_pills = cycle_pills.child(
                btn.xsmall()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.edit_mangohud(window, cx, |p| p.apply_preset(preset));
                    }))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(div().text_xs().child(num))
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
                    .py_2()
                    .gap_1p5()
                    .rounded(radius)
                    .bg(bg)
                    .border_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(self.labels.cycle_order_label.clone()),
                    )
                    .child(cycle_pills),
            );

        group_panel(cx)
            .child(pos_orient_row)
            .child(div().border_t_1().border_color(border).child(sliders_grid))
            .child(div().border_t_1().border_color(border).child(shortcut_row))
            .child(
                div().border_t_1().border_color(border).child(
                    v_flex()
                        .w_full()
                        .child(preset_shortcut_row)
                        .child(cycle_footer),
                ),
            )
            .into_any_element()
    }

    pub(super) fn render_hardware_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
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

        // GPU label & CPU label side-by-side in one compact row
        let labels_row = h_flex()
            .w_full()
            .flex_wrap()
            .gap_4()
            .px_4()
            .py_3()
            .child(
                v_flex()
                    .flex_1()
                    .min_w(rems(13.))
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .child(self.labels.gpu_label_label.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(Input::new(&self.gpu_label_input).small().flex_1())
                            .child(
                                Button::new("hud-gpu-label-reset")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::RotateCcw)
                                    .tooltip(self.labels.label_reset.clone())
                                    .disabled(profile.gpu_label_override.is_empty())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.gpu_label_input
                                            .update(cx, |s, cx| s.set_value("", window, cx));
                                        this.edit_mangohud(window, cx, |p| {
                                            p.gpu_label_override.clear()
                                        });
                                    })),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w(rems(13.))
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .child(self.labels.cpu_label_label.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .child(Input::new(&self.cpu_label_input).small().flex_1())
                            .child(
                                Button::new("hud-cpu-label-reset")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::RotateCcw)
                                    .tooltip(self.labels.label_reset.clone())
                                    .disabled(profile.cpu_label_override.is_empty())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.cpu_label_input
                                            .update(cx, |s, cx| s.set_value("", window, cx));
                                        this.edit_mangohud(window, cx, |p| {
                                            p.cpu_label_override.clear()
                                        });
                                    })),
                            ),
                    ),
            );

        v_flex()
            .w_full()
            .child(group_heading(self.labels.group_hardware.clone(), cx))
            .child(
                group_panel(cx)
                    .child(gpu_select_row)
                    .child(div().border_t_1().border_color(border).child(labels_row)),
            )
            .into_any_element()
    }

    pub(super) fn render_colors_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let (border, bg, muted_fg, primary, warning, radius, mono_font) = {
            let theme = cx.theme();
            (
                theme.border,
                theme.background,
                theme.muted_foreground,
                theme.primary,
                theme.warning,
                theme.radius,
                theme.mono_font_family.clone(),
            )
        };
        let p = self.active_mangohud();

        let palette_control = h_flex()
            .w_full()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .child(
                        choice_button(
                            "hud-palette-default",
                            self.labels.palette_mangohud_default.clone(),
                            p.palette == HudColorPalette::MangoHudDefault,
                            cx.listener(|this, _, window, cx| {
                                this.open_color_slot = None;
                                this.edit_mangohud(window, cx, |p| {
                                    p.palette = HudColorPalette::MangoHudDefault
                                });
                            }),
                        )
                        .child(palette_dots(self.display.palette_dots[0], border)),
                    )
                    .child(
                        choice_button(
                            "hud-palette-sync",
                            self.labels.palette_sync_irori_theme.clone(),
                            p.palette == HudColorPalette::SyncIroriTheme,
                            cx.listener(|this, _, window, cx| {
                                this.open_color_slot = None;
                                this.edit_mangohud(window, cx, |p| {
                                    p.palette = HudColorPalette::SyncIroriTheme
                                });
                            }),
                        )
                        .child(palette_dots(self.display.palette_dots[1], border)),
                    )
                    .child(
                        choice_button(
                            "hud-palette-monochrome",
                            self.labels.palette_monochrome.clone(),
                            p.palette == HudColorPalette::Monochrome,
                            cx.listener(|this, _, window, cx| {
                                this.open_color_slot = None;
                                this.edit_mangohud(window, cx, |p| {
                                    p.palette = HudColorPalette::Monochrome
                                });
                            }),
                        )
                        .child(palette_dots(self.display.palette_dots[2], border)),
                    )
                    .child(
                        choice_button(
                            "hud-palette-custom",
                            self.labels.palette_custom.clone(),
                            p.palette == HudColorPalette::Custom,
                            cx.listener(|this, _, window, cx| {
                                this.edit_mangohud(window, cx, |p| {
                                    p.palette = HudColorPalette::Custom
                                });
                            }),
                        )
                        .child(palette_dots(self.display.palette_dots[3], border)),
                    ),
            )
            .when(p.palette == HudColorPalette::Custom, |row| {
                row.child(
                    Button::new("hud-colors-reset-all")
                        .ghost()
                        .small()
                        .icon(IconName::RotateCcw)
                        .label(self.labels.color_reset_all.clone())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.reset_custom_colors(window, cx);
                        })),
                )
            })
            .into_any_element();

        let mut panel = group_panel(cx).child(field_row(
            RowState {
                label: self.labels.palette_label.clone(),
                help: self.labels.palette_help.clone(),
                stacked: true,
                ..Default::default()
            },
            palette_control,
            cx,
        ));

        if p.palette == HudColorPalette::Custom {
            let render_tile = |slot: ColorSlot, cx: &mut Context<Self>| {
                let ix = slot as usize;
                let swatch = self.display.custom_swatches[ix];
                let wheel = self.color_wheel.clone();
                let is_open = self.open_color_slot == Some(slot);
                let swatch_popover = Popover::new(slot.popover_id())
                    .anchor(Anchor::TopLeft)
                    .open(is_open)
                    .trigger(ColorSwatchTrigger::new(
                        slot.trigger_id(),
                        swatch,
                        self.labels.color_pick_tooltip.clone(),
                    ))
                    .on_open_change(cx.listener(move |this, open: &bool, window, cx| {
                        this.on_color_popover_change(slot, *open, window, cx);
                    }))
                    .content(move |_, _, _| wheel.clone());

                h_flex()
                    .id(slot.id())
                    .gap_2()
                    .items_center()
                    .w(rems(14.5))
                    .px_2p5()
                    .py_1p5()
                    .rounded(radius)
                    .bg(bg)
                    .border_1()
                    .border_color(if is_open { primary } else { border })
                    .child(swatch_popover)
                    .child(
                        div()
                            .text_xs()
                            .font_medium()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(self.labels.color_slot_labels[ix].clone()),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(2.))
                            .flex_shrink_0()
                            .child(
                                div()
                                    .text_xs()
                                    .font_family(mono_font.clone())
                                    .text_color(muted_fg)
                                    .child(SharedString::new_static("#")),
                            )
                            .child(Input::new(&self.color_inputs[ix]).small().w_20()),
                    )
            };

            let mut surface_grid = h_flex().flex_wrap().gap_2().w_full();
            let mut metrics_grid = h_flex().flex_wrap().gap_2().w_full();
            for slot in COLOR_SLOTS {
                let tile = render_tile(slot, cx);
                if matches!(slot, ColorSlot::Background | ColorSlot::Text) {
                    surface_grid = surface_grid.child(tile);
                } else {
                    metrics_grid = metrics_grid.child(tile);
                }
            }

            panel = panel.child(
                v_flex()
                    .w_full()
                    .border_t_1()
                    .border_color(border)
                    .px_4()
                    .py_3()
                    .gap_3()
                    .when_some(self.display.low_contrast_notice.clone(), |v, notice| {
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
                                .child(div().flex_1().child(notice)),
                        )
                    })
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
                            .child(surface_grid),
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

        panel = panel.child(
            div()
                .border_t_1()
                .border_color(border)
                .child(metric_switch_row(
                    "hud-fps-color-change",
                    self.labels.fps_color_change_label.clone(),
                    self.labels.fps_color_change_help.clone(),
                    p.fps_color_change,
                    cx.listener(|this, on: &bool, window, cx| {
                        this.edit_mangohud(window, cx, |p| p.fps_color_change = *on);
                    }),
                    cx,
                )),
        );

        v_flex()
            .w_full()
            .child(group_heading(self.labels.group_colors.clone(), cx))
            .child(panel)
            .into_any_element()
    }
}
