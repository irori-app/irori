use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, StyledExt as _,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, ClipboardItem, Context, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};
use irori_core::{FpsLimitMethod, GameHudMode, GlobalHudMode, HudBackend, HudPreset};

use crate::widgets::{RowState, field_row, group_heading, group_panel, muted};

use super::helpers::{
    FPS_PRESETS, choice_button, info_button, metric_switch_row, render_divided_group,
};
use super::position_grid::render_metal_position_grid;
use super::{HudPanel, HudPanelScope, HudTab};

impl HudPanel {
    pub(super) fn render_missing_mangohud(&self) -> Option<AnyElement> {
        if self.backend != HudBackend::MangoHud
            || !self.env_loaded
            || self.env.has_vulkan_layer
            || self.env.mangohud_bin_path.is_some()
        {
            return None;
        }
        Some(
            div()
                .pb_2()
                .child(
                    Alert::warning(
                        "hud-missing-mangohud",
                        self.labels.missing_description.clone(),
                    )
                    .title(self.labels.missing_title.clone()),
                )
                .into_any_element(),
        )
    }

    pub(super) fn copy_steam_button(
        &self,
        id: &'static str,
        snippet: SharedString,
        cx: &mut Context<Self>,
    ) -> Button {
        let copied_msg = self.labels.steam_copied.clone();
        Button::new(id)
            .outline()
            .small()
            .icon(IconName::Copy)
            .label(self.labels.steam_copy.clone())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.sync_live_mangohud_config(cx);
                cx.write_to_clipboard(ClipboardItem::new_string(snippet.to_string()));
                window.push_notification(Notification::success(copied_msg.clone()), cx);
            }))
    }

    pub(super) fn render_scope_notice(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, text) = self.scope_notice()?;
        let alert = Alert::info(id, text.clone());
        let snippet_row = if matches!(self.scope, HudPanelScope::Global) {
            self.display.steam_snippet.clone().map(|snippet| {
                let (radius, muted_bg, border_color, mono_font) = {
                    let theme = cx.theme();
                    (
                        theme.radius,
                        theme.muted,
                        theme.border,
                        theme.mono_font_family.clone(),
                    )
                };
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded(radius)
                    .bg(muted_bg)
                    .border_1()
                    .border_color(border_color)
                    .child(
                        div()
                            .id("hud-global-steam-snippet")
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_xs()
                            .font_family(mono_font)
                            .tooltip({
                                let full = snippet.clone();
                                move |window, cx| Tooltip::new(full.clone()).build(window, cx)
                            })
                            .child(snippet.clone()),
                    )
                    .child(self.copy_steam_button("hud-global-steam-copy", snippet, cx))
                    .into_any_element()
            })
        } else {
            None
        };
        Some(
            v_flex()
                .w_full()
                .gap_2()
                .pb_1()
                .child(alert)
                .children(snippet_row)
                .into_any_element(),
        )
    }

    pub(super) fn render_steam_section(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.is_steam() {
            return None;
        }
        let snippet_card = match self.display.steam_snippet.clone() {
            Some(snippet) => {
                let (radius, muted_bg, border_color, mono_font) = {
                    let theme = cx.theme();
                    (
                        theme.radius,
                        theme.muted,
                        theme.border,
                        theme.mono_font_family.clone(),
                    )
                };
                let code_box = div()
                    .w_full()
                    .min_w_0()
                    .p_3()
                    .rounded(radius)
                    .bg(muted_bg)
                    .border_1()
                    .border_color(border_color)
                    .text_xs()
                    .font_family(mono_font)
                    .child(snippet.clone());
                let copy_btn = self.copy_steam_button("hud-steam-copy", snippet, cx);
                v_flex()
                    .w_full()
                    .gap_2()
                    .items_start()
                    .child(code_box)
                    .child(copy_btn)
                    .into_any_element()
            }
            None => muted(self.labels.steam_none_needed.clone(), cx).into_any_element(),
        };

        let row = field_row(
            RowState {
                label: self.labels.steam_snippet_label.clone(),
                stacked: true,
                ..Default::default()
            },
            snippet_card,
            cx,
        );

        Some(
            v_flex()
                .w_full()
                .gap_3()
                .pb_2()
                .child(
                    Alert::info("hud-steam-notice", self.labels.steam_description.clone())
                        .title(self.labels.steam_title.clone()),
                )
                .child(group_panel(cx).child(row))
                .into_any_element(),
        )
    }

    fn render_mode_group(&self, id: &'static str, cx: &mut Context<Self>) -> AnyElement {
        match &self.scope {
            HudPanelScope::Global => {
                let on = self.global_on();
                ButtonGroup::new(id)
                    .outline()
                    .small()
                    .child(
                        Button::new("hud-mode-off")
                            .label(self.labels.mode_off.clone())
                            .selected(!on),
                    )
                    .child(
                        Button::new("hud-mode-irori")
                            .label(self.labels.mode_irori_profile.clone())
                            .selected(on),
                    )
                    .on_click(cx.listener(|this, clicked: &Vec<usize>, window, cx| {
                        match clicked.first().copied() {
                            Some(0) => this.set_global_mode(GlobalHudMode::Off, window, cx),
                            Some(1) => {
                                this.set_global_mode(GlobalHudMode::IroriProfile, window, cx)
                            }
                            _ => {}
                        }
                    }))
                    .into_any_element()
            }
            HudPanelScope::PerGame { .. } => {
                let current = self.game.mode;
                ButtonGroup::new(id)
                    .outline()
                    .small()
                    .child(
                        Button::new("hud-game-mode-inherit")
                            .label(self.labels.mode_inherit_global.clone())
                            .selected(current == GameHudMode::InheritGlobal),
                    )
                    .child(
                        Button::new("hud-game-mode-custom")
                            .label(self.labels.mode_custom.clone())
                            .selected(current == GameHudMode::Custom),
                    )
                    .child(
                        Button::new("hud-game-mode-disabled")
                            .label(self.labels.mode_disabled.clone())
                            .selected(current == GameHudMode::Disabled),
                    )
                    .on_click(cx.listener(|this, clicked: &Vec<usize>, window, cx| {
                        match clicked.first().copied() {
                            Some(0) => this.set_game_mode(GameHudMode::InheritGlobal, window, cx),
                            Some(1) => this.set_game_mode(GameHudMode::Custom, window, cx),
                            Some(2) => this.set_game_mode(GameHudMode::Disabled, window, cx),
                            _ => {}
                        }
                    }))
                    .into_any_element()
            }
        }
    }

    pub(super) fn render_mode_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let mode_control = self.render_mode_group("hud-mode-section-group", cx);

        let help = match &self.scope {
            HudPanelScope::Global => self.labels.mode_global_help.clone(),
            HudPanelScope::PerGame { .. } => self.labels.mode_game_help.clone(),
        };

        let mut panel = group_panel(cx).child(field_row(
            RowState {
                label: self.labels.mode_label.clone(),
                help,
                stacked: true,
                ..Default::default()
            },
            mode_control,
            cx,
        ));

        let status_note: Option<SharedString> = match &self.scope {
            HudPanelScope::Global if self.global_on() => None,
            HudPanelScope::Global => Some(self.labels.mode_off_note.clone()),
            HudPanelScope::PerGame { .. } => match self.game.mode {
                GameHudMode::InheritGlobal => Some(self.display.inherit_summary.clone()),
                GameHudMode::Disabled => Some(self.labels.mode_disabled_note.clone()),
                GameHudMode::Custom => None,
            },
        };

        if let Some(note) = status_note {
            panel = panel.child(
                div()
                    .border_t_1()
                    .border_color(border)
                    .px_4()
                    .py_3()
                    .child(muted(note, cx)),
            );
        }

        if matches!(self.scope, HudPanelScope::PerGame { .. })
            && self.game.mode == GameHudMode::Custom
        {
            let restore_btn = Button::new("hud-restore-from-global")
                .outline()
                .small()
                .icon(IconName::RotateCcw)
                .label(self.labels.restore_from_global.clone())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.restore_game_from_global(window, cx);
                }))
                .into_any_element();
            panel = panel.child(div().border_t_1().border_color(border).child(field_row(
                RowState {
                    label: self.labels.restore_from_global.clone(),
                    help: self.labels.restore_from_global_help.clone(),
                    ..Default::default()
                },
                restore_btn,
                cx,
            )));
        }

        v_flex()
            .w_full()
            .child(group_heading(self.labels.group_mode.clone(), cx))
            .child(panel)
            .into_any_element()
    }

    pub(super) fn render_top_quick_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let (border, muted_fg, primary, primary_fg) = {
            let theme = cx.theme();
            (
                theme.border,
                theme.muted_foreground,
                theme.primary,
                theme.primary_foreground,
            )
        };
        let p = self.active_mangohud();

        // Row 1: Mode segmented ButtonGroup (+ inline Restore button when PerGame::Custom)
        let mode_group = self.render_mode_group("hud-mode-quick-group", cx);
        let mode_row = h_flex()
            .w_full()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .px_4()
            .py_2p5()
            .child(
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2p5()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .child(self.labels.mode_label.clone()),
                            )
                            .child(info_button(
                                "hud-mode-info",
                                match &self.scope {
                                    HudPanelScope::Global => self.labels.mode_global_help.clone(),
                                    HudPanelScope::PerGame { .. } => {
                                        self.labels.mode_game_help.clone()
                                    }
                                },
                            )),
                    )
                    .child(mode_group),
            )
            .when(
                matches!(self.scope, HudPanelScope::PerGame { .. })
                    && self.game.mode == GameHudMode::Custom,
                |row| {
                    row.child(
                        Button::new("hud-restore-from-global")
                            .ghost()
                            .small()
                            .icon(IconName::RotateCcw)
                            .tooltip(self.labels.restore_from_global_help.clone())
                            .label(self.labels.restore_from_global.clone())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.restore_game_from_global(window, cx);
                            })),
                    )
                },
            );

        // Row 2: 6 Quick Presets with semantic Lucide icons + saved-slot indicator dot
        let current_preset = p.preset;
        let has_custom_v = p.has_custom_preset();
        let has_custom_h = p.has_custom_horizontal_preset();
        let preset_btn = |id: &'static str,
                          icon: IconName,
                          label: SharedString,
                          preset: HudPreset,
                          saved_dot: bool,
                          cx: &mut Context<Self>| {
            let active = current_preset == preset;
            let dot_color = if active { primary_fg } else { primary };
            let btn = if active {
                Button::new(id).primary()
            } else {
                Button::new(id).outline()
            };
            btn.small()
                .icon(icon)
                .label(label)
                .when(saved_dot, |b| {
                    b.child(div().size(px(6.)).rounded_full().bg(dot_color))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| p.apply_preset(preset));
                }))
        };

        let preset_buttons = h_flex()
            .flex_wrap()
            .gap_1p5()
            .child(preset_btn(
                "hud-preset-minimal",
                IconName::Gauge,
                self.labels.preset_minimal_fps.clone(),
                HudPreset::MinimalFps,
                false,
                cx,
            ))
            .child(preset_btn(
                "hud-preset-horizontal",
                IconName::PanelTop,
                self.labels.preset_horizontal_compact.clone(),
                HudPreset::HorizontalCompact,
                false,
                cx,
            ))
            .child(preset_btn(
                "hud-preset-balanced",
                IconName::Gamepad2,
                self.labels.preset_balanced_gaming.clone(),
                HudPreset::BalancedGaming,
                false,
                cx,
            ))
            .child(preset_btn(
                "hud-preset-full",
                IconName::Activity,
                self.labels.preset_full_diagnostics.clone(),
                HudPreset::FullDiagnostics,
                false,
                cx,
            ))
            .child(preset_btn(
                "hud-preset-custom",
                IconName::SlidersVertical,
                self.labels.preset_custom.clone(),
                HudPreset::Custom,
                has_custom_v,
                cx,
            ))
            .child(preset_btn(
                "hud-preset-custom-horizontal",
                IconName::SlidersHorizontal,
                self.labels.preset_custom_horizontal.clone(),
                HudPreset::CustomHorizontal,
                has_custom_h,
                cx,
            ));

        let preset_section = v_flex()
            .w_full()
            .px_4()
            .py_2p5()
            .gap_1p5()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .child(self.labels.presets_label.clone()),
                            )
                            .child(info_button(
                                "hud-preset-info",
                                self.labels.presets_help.clone(),
                            )),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(self.display.in_cycle_summary.clone()),
                    ),
            )
            .child(preset_buttons);

        // Row 3: FPS limit values; contextual Pacing ButtonGroup appears only when a limit is active.
        let mut fps_values = h_flex()
            .flex_wrap()
            .gap_1p5()
            .items_center()
            .child(choice_button(
                "hud-fps-off",
                self.labels.fps_limit_off.clone(),
                p.fps_limit.is_none(),
                cx.listener(|this, _, window, cx| {
                    this.fps_limit_input
                        .update(cx, |s, cx| s.set_value("", window, cx));
                    this.edit_mangohud(window, cx, |p| p.fps_limit = None);
                }),
            ));
        for (cap, id, text) in FPS_PRESETS {
            fps_values = fps_values.child(choice_button(
                id,
                SharedString::new_static(text),
                p.fps_limit == Some(cap),
                cx.listener(move |this, _, window, cx| {
                    this.fps_limit_input
                        .update(cx, |s, cx| s.set_value("", window, cx));
                    this.edit_mangohud(window, cx, |p| p.fps_limit = Some(cap));
                }),
            ));
        }
        fps_values = fps_values.child(Input::new(&self.fps_limit_input).small().w_20());

        let has_fps_limit = p.fps_limit.is_some_and(|v| v > 0);
        let fps_method_group = ButtonGroup::new("hud-fps-method-group")
            .outline()
            .xsmall()
            .child(
                Button::new("hud-fps-method-late")
                    .label(self.labels.fps_limit_method_late.clone())
                    .tooltip(self.labels.fps_limit_method_help.clone())
                    .selected(p.fps_limit_method == FpsLimitMethod::Late),
            )
            .child(
                Button::new("hud-fps-method-early")
                    .label(self.labels.fps_limit_method_early.clone())
                    .tooltip(self.labels.fps_limit_method_help.clone())
                    .selected(p.fps_limit_method == FpsLimitMethod::Early),
            )
            .on_click(cx.listener(|this, clicked: &Vec<usize>, window, cx| {
                match clicked.first().copied() {
                    Some(0) => this.edit_mangohud(window, cx, |p| {
                        p.fps_limit_method = FpsLimitMethod::Late;
                    }),
                    Some(1) => this.edit_mangohud(window, cx, |p| {
                        p.fps_limit_method = FpsLimitMethod::Early;
                    }),
                    _ => {}
                }
            }));

        let fps_section = v_flex()
            .w_full()
            .px_4()
            .py_2p5()
            .gap_1p5()
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .child(self.labels.fps_limit_label.clone()),
                            )
                            .child(info_button(
                                "hud-fps-info",
                                self.labels.fps_limit_help.clone(),
                            )),
                    )
                    .when(has_fps_limit, |row| {
                        row.child(
                            h_flex()
                                .items_center()
                                .gap_1p5()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted_fg)
                                        .child(self.labels.fps_limit_method_pacing.clone()),
                                )
                                .child(fps_method_group),
                        )
                    }),
            )
            .child(fps_values);

        group_panel(cx)
            .child(mode_row)
            .child(
                div()
                    .border_t_1()
                    .border_color(border)
                    .child(preset_section),
            )
            .child(div().border_t_1().border_color(border).child(fps_section))
            .into_any_element()
    }

    pub(super) fn render_tab_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let selected = match self.active_tab {
            HudTab::Metrics => 0,
            HudTab::Layout => 1,
            HudTab::Appearance => 2,
        };
        div()
            .w_full()
            .pt_1()
            .child(
                TabBar::new("hud-tabs")
                    .underline()
                    .large()
                    .selected_index(selected)
                    .child(Tab::new().label(self.labels.tab_metrics.clone()))
                    .child(Tab::new().label(self.labels.tab_layout.clone()))
                    .child(Tab::new().label(self.labels.tab_appearance.clone()))
                    .on_click(cx.listener(|this, ix: &usize, _, cx| {
                        let next = match ix {
                            1 => HudTab::Layout,
                            2 => HudTab::Appearance,
                            _ => HudTab::Metrics,
                        };
                        if next != HudTab::Appearance {
                            this.open_color_slot = None;
                        }
                        this.active_tab = next;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    pub(super) fn render_opengl_section(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.show_opengl_legacy() {
            return None;
        }
        if self.game.mode != GameHudMode::Custom {
            return None;
        }
        let border = cx.theme().border;
        let p = self.active_mangohud();

        let rows = [
            metric_switch_row(
                "hud-opengl-wrapper",
                self.labels.opengl_wrapper_label.clone(),
                self.labels.opengl_wrapper_help.clone(),
                p.opengl_legacy_wrapper,
                cx.listener(|this, on: &bool, window, cx| {
                    this.edit_mangohud(window, cx, |p| p.opengl_legacy_wrapper = *on);
                }),
                cx,
            ),
            field_row(
                RowState {
                    label: self.labels.mangohud_dlsym_label.clone(),
                    help: self.labels.mangohud_dlsym_help.clone(),
                    ..Default::default()
                },
                Switch::new("hud-mangohud-dlsym")
                    .checked(p.mangohud_dlsym)
                    .disabled(!p.opengl_legacy_wrapper)
                    .accessibility_label(self.labels.mangohud_dlsym_label.clone())
                    .on_change(cx.listener(|this, on: &bool, window, cx| {
                        this.edit_mangohud(window, cx, |p| p.mangohud_dlsym = *on);
                    }))
                    .into_any_element(),
                cx,
            ),
        ];

        Some(render_divided_group(
            self.labels.group_opengl.clone(),
            rows,
            border,
            cx,
        ))
    }

    pub(super) fn render_metal_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let current = self.active_metal().alignment;
        let weak = cx.entity().downgrade();
        let grid = render_metal_position_grid(
            current,
            &self.labels.position_labels,
            move |align, _, _, cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.edit_metal(cx, |m| m.alignment = align);
                });
            },
            cx,
        );

        let mut panel = group_panel(cx);
        if self.is_editing_profile() {
            panel = panel.child(field_row(
                RowState {
                    label: self.labels.metal_alignment_label.clone(),
                    help: self.labels.metal_alignment_help.clone(),
                    stacked: true,
                    ..Default::default()
                },
                grid,
                cx,
            ));
            panel = panel.child(
                div()
                    .border_t_1()
                    .border_color(border)
                    .px_4()
                    .py_3()
                    .child(muted(self.labels.metal_note.clone(), cx)),
            );
        } else {
            panel = panel.child(
                div()
                    .px_4()
                    .py_3()
                    .child(muted(self.labels.metal_note.clone(), cx)),
            );
        }

        v_flex()
            .w_full()
            .child(group_heading(self.labels.group_metal.clone(), cx))
            .child(panel)
            .into_any_element()
    }
}
