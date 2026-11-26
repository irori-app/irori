use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::select::Select;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::{
    Anchor, AnyElement, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    Styled as _, div, rems,
};
use irori_core::{HudColorPalette, HudOrientation};

use crate::hud::color_wheel::ColorSwatchTrigger;
use crate::widgets::{RowState, field_row, group_heading, group_panel};

use super::HudPanel;
use super::helpers::{
    COLOR_SLOTS, choice_button, compact_slider_cell, metric_chip, metric_switch_row, palette_dots,
};

impl HudPanel {
    pub(super) fn render_layout_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let muted_fg = cx.theme().muted_foreground;
        let p = self.active_mangohud();

        // Row 1: Position + Orientation side-by-side
        let pos_orient_row = h_flex()
            .w_full()
            .flex_wrap()
            .items_end()
            .justify_between()
            .gap_4()
            .px_4()
            .py_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .child(self.labels.position_label.clone()),
                    )
                    .child(
                        Select::new(&self.position_select)
                            .id("hud-position-select")
                            .accessibility_label(self.labels.position_label.clone())
                            .w_48(),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .child(self.labels.orientation_label.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(choice_button(
                                "hud-orientation-vertical",
                                self.labels.orientation_vertical.clone(),
                                p.orientation == HudOrientation::Vertical,
                                cx.listener(|this, _, window, cx| {
                                    this.edit_mangohud(window, cx, |p| {
                                        p.set_orientation(HudOrientation::Vertical);
                                    });
                                }),
                            ))
                            .child(choice_button(
                                "hud-orientation-horizontal",
                                self.labels.orientation_horizontal.clone(),
                                p.orientation == HudOrientation::Horizontal,
                                cx.listener(|this, _, window, cx| {
                                    this.edit_mangohud(window, cx, |p| {
                                        p.set_orientation(HudOrientation::Horizontal);
                                    });
                                }),
                            )),
                    ),
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

        // Row 3: Quick layout toggle chips
        let quick_toggles = h_flex()
            .w_full()
            .flex_wrap()
            .px_4()
            .py_3()
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

        // Row 4: Shortcut recorders (overlay toggle + preset cycling)
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

        group_panel(cx)
            .child(pos_orient_row)
            .child(div().border_t_1().border_color(border).child(sliders_grid))
            .child(div().border_t_1().border_color(border).child(quick_toggles))
            .child(div().border_t_1().border_color(border).child(shortcut_row))
            .child(
                div()
                    .border_t_1()
                    .border_color(border)
                    .child(preset_shortcut_row),
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
        let border = cx.theme().border;
        let p = self.active_mangohud();

        let palette_control = h_flex()
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
                        this.edit_mangohud(window, cx, |p| p.palette = HudColorPalette::Monochrome);
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
                        this.edit_mangohud(window, cx, |p| p.palette = HudColorPalette::Custom);
                    }),
                )
                .child(palette_dots(self.display.palette_dots[3], border)),
            )
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
            let mut grid = h_flex().flex_wrap().gap_3().w_full();
            for slot in COLOR_SLOTS {
                let ix = slot as usize;
                let swatch = self.display.custom_swatches[ix];
                let wheel = self.color_wheel.clone();
                let swatch_popover = Popover::new(slot.popover_id())
                    .anchor(Anchor::TopLeft)
                    .open(self.open_color_slot == Some(slot))
                    .trigger(ColorSwatchTrigger::new(
                        slot.trigger_id(),
                        swatch,
                        self.labels.color_pick_tooltip.clone(),
                    ))
                    .on_open_change(cx.listener(move |this, open: &bool, window, cx| {
                        this.on_color_popover_change(slot, *open, window, cx);
                    }))
                    .content(move |_, _, _| wheel.clone());
                let item = h_flex()
                    .id(slot.id())
                    .gap_2()
                    .items_center()
                    .w(rems(13.5))
                    .child(swatch_popover)
                    .child(
                        div()
                            .text_xs()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(self.labels.color_slot_labels[ix].clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .child(Input::new(&self.color_inputs[ix]).small().w_24()),
                    );
                grid = grid.child(item);
            }
            panel = panel.child(
                div()
                    .border_t_1()
                    .border_color(border)
                    .px_4()
                    .py_3()
                    .child(grid),
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
