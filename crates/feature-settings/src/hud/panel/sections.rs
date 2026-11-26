use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::{
    AnyElement, ClipboardItem, Context, InteractiveElement as _, IntoElement as _,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, div,
};
use irori_core::{
    FpsLimitMethod, GameHudMode, GlobalHudMode, HudBackend, HudPreset, MetalHudAlignment,
};

use crate::widgets::{RowState, field_row, group_heading, group_panel, muted};

use super::helpers::{
    FPS_PRESETS, choice_button, info_button, metric_switch_row, render_divided_group,
};
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

    pub(super) fn render_mode_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let mode_control: AnyElement = match &self.scope {
            HudPanelScope::Global => {
                let on = self.global_on();
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .child(choice_button(
                        "hud-mode-off",
                        self.labels.mode_off.clone(),
                        !on,
                        cx.listener(|this, _, window, cx| {
                            this.set_global_mode(GlobalHudMode::Off, window, cx);
                        }),
                    ))
                    .child(choice_button(
                        "hud-mode-irori",
                        self.labels.mode_irori_profile.clone(),
                        on,
                        cx.listener(|this, _, window, cx| {
                            this.set_global_mode(GlobalHudMode::IroriProfile, window, cx);
                        }),
                    ))
                    .into_any_element()
            }
            HudPanelScope::PerGame { .. } => {
                let current = self.game.mode;
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .child(choice_button(
                        "hud-game-mode-inherit",
                        self.labels.mode_inherit_global.clone(),
                        current == GameHudMode::InheritGlobal,
                        cx.listener(|this, _, window, cx| {
                            this.set_game_mode(GameHudMode::InheritGlobal, window, cx);
                        }),
                    ))
                    .child(choice_button(
                        "hud-game-mode-custom",
                        self.labels.mode_custom.clone(),
                        current == GameHudMode::Custom,
                        cx.listener(|this, _, window, cx| {
                            this.set_game_mode(GameHudMode::Custom, window, cx);
                        }),
                    ))
                    .child(choice_button(
                        "hud-game-mode-disabled",
                        self.labels.mode_disabled.clone(),
                        current == GameHudMode::Disabled,
                        cx.listener(|this, _, window, cx| {
                            this.set_game_mode(GameHudMode::Disabled, window, cx);
                        }),
                    ))
                    .into_any_element()
            }
        };

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
        let border = cx.theme().border;
        let p = self.active_mangohud();

        // Row 1: Mode buttons (+ inline Restore button when PerGame::Custom)
        let mode_buttons: AnyElement = match &self.scope {
            HudPanelScope::Global => {
                let on = self.global_on();
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .child(choice_button(
                        "hud-mode-off",
                        self.labels.mode_off.clone(),
                        !on,
                        cx.listener(|this, _, window, cx| {
                            this.set_global_mode(GlobalHudMode::Off, window, cx);
                        }),
                    ))
                    .child(choice_button(
                        "hud-mode-irori",
                        self.labels.mode_irori_profile.clone(),
                        on,
                        cx.listener(|this, _, window, cx| {
                            this.set_global_mode(GlobalHudMode::IroriProfile, window, cx);
                        }),
                    ))
                    .into_any_element()
            }
            HudPanelScope::PerGame { .. } => {
                let current = self.game.mode;
                let mut row = h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .items_center()
                    .child(choice_button(
                        "hud-game-mode-inherit",
                        self.labels.mode_inherit_global.clone(),
                        current == GameHudMode::InheritGlobal,
                        cx.listener(|this, _, window, cx| {
                            this.set_game_mode(GameHudMode::InheritGlobal, window, cx);
                        }),
                    ))
                    .child(choice_button(
                        "hud-game-mode-custom",
                        self.labels.mode_custom.clone(),
                        current == GameHudMode::Custom,
                        cx.listener(|this, _, window, cx| {
                            this.set_game_mode(GameHudMode::Custom, window, cx);
                        }),
                    ))
                    .child(choice_button(
                        "hud-game-mode-disabled",
                        self.labels.mode_disabled.clone(),
                        current == GameHudMode::Disabled,
                        cx.listener(|this, _, window, cx| {
                            this.set_game_mode(GameHudMode::Disabled, window, cx);
                        }),
                    ));
                if current == GameHudMode::Custom {
                    row = row.child(
                        Button::new("hud-restore-from-global")
                            .ghost()
                            .small()
                            .icon(IconName::RotateCcw)
                            .tooltip(self.labels.restore_from_global_help.clone())
                            .label(self.labels.restore_from_global.clone())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.restore_game_from_global(window, cx);
                            })),
                    );
                }
                row.into_any_element()
            }
        };

        // Row 2: 5 quick Presets
        let current_preset = p.preset;
        let preset_buttons = h_flex()
            .flex_wrap()
            .gap_1p5()
            .child(choice_button(
                "hud-preset-minimal",
                self.labels.preset_minimal_fps.clone(),
                current_preset == HudPreset::MinimalFps,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| p.apply_preset(HudPreset::MinimalFps));
                }),
            ))
            .child(choice_button(
                "hud-preset-horizontal",
                self.labels.preset_horizontal_compact.clone(),
                current_preset == HudPreset::HorizontalCompact,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.apply_preset(HudPreset::HorizontalCompact)
                    });
                }),
            ))
            .child(choice_button(
                "hud-preset-balanced",
                self.labels.preset_balanced_gaming.clone(),
                current_preset == HudPreset::BalancedGaming,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| p.apply_preset(HudPreset::BalancedGaming));
                }),
            ))
            .child(choice_button(
                "hud-preset-full",
                self.labels.preset_full_diagnostics.clone(),
                current_preset == HudPreset::FullDiagnostics,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| p.apply_preset(HudPreset::FullDiagnostics));
                }),
            ))
            .child(choice_button(
                "hud-preset-custom",
                self.labels.preset_custom.clone(),
                current_preset == HudPreset::Custom,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| p.apply_preset(HudPreset::Custom));
                }),
            ))
            .child(choice_button(
                "hud-preset-custom-horizontal",
                self.labels.preset_custom_horizontal.clone(),
                current_preset == HudPreset::CustomHorizontal,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| p.apply_preset(HudPreset::CustomHorizontal));
                }),
            ));

        // Row 3: FPS limit values on top, limiter method below.
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
        let fps_methods = h_flex()
            .flex_wrap()
            .gap_1p5()
            .items_center()
            .child(
                choice_button(
                    "hud-fps-method-late",
                    self.labels.fps_limit_method_late.clone(),
                    p.fps_limit_method == FpsLimitMethod::Late,
                    cx.listener(|this, _, window, cx| {
                        this.edit_mangohud(window, cx, |p| {
                            p.fps_limit_method = FpsLimitMethod::Late
                        });
                    }),
                )
                .tooltip(self.labels.fps_limit_method_help.clone()),
            )
            .child(
                choice_button(
                    "hud-fps-method-early",
                    self.labels.fps_limit_method_early.clone(),
                    p.fps_limit_method == FpsLimitMethod::Early,
                    cx.listener(|this, _, window, cx| {
                        this.edit_mangohud(window, cx, |p| {
                            p.fps_limit_method = FpsLimitMethod::Early
                        });
                    }),
                )
                .tooltip(self.labels.fps_limit_method_help.clone()),
            );
        let fps_row = v_flex().gap_1p5().child(fps_values).child(fps_methods);

        let compact_row =
            |info_id: &'static str, label: SharedString, hint: SharedString, body: AnyElement| {
                v_flex()
                    .w_full()
                    .px_4()
                    .py_2p5()
                    .gap_1p5()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(div().text_xs().font_semibold().child(label))
                            .child(info_button(info_id, hint)),
                    )
                    .child(body)
            };

        let mode_hint = match &self.scope {
            HudPanelScope::Global => self.labels.mode_global_help.clone(),
            HudPanelScope::PerGame { .. } => self.labels.mode_game_help.clone(),
        };

        group_panel(cx)
            .child(compact_row(
                "hud-mode-info",
                self.labels.mode_label.clone(),
                mode_hint,
                mode_buttons,
            ))
            .child(div().border_t_1().border_color(border).child(compact_row(
                "hud-preset-info",
                self.labels.presets_label.clone(),
                self.labels.presets_help.clone(),
                preset_buttons.into_any_element(),
            )))
            .child(div().border_t_1().border_color(border).child(compact_row(
                "hud-fps-info",
                self.labels.fps_limit_label.clone(),
                self.labels.fps_limit_help.clone(),
                fps_row.into_any_element(),
            )))
            .into_any_element()
    }

    pub(super) fn render_tab_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let active = self.active_tab;
        h_flex()
            .w_full()
            .pt_2()
            .gap_1p5()
            .child(choice_button(
                "hud-tab-metrics",
                self.labels.tab_metrics.clone(),
                active == HudTab::Metrics,
                cx.listener(|this, _, _, cx| {
                    this.active_tab = HudTab::Metrics;
                    this.open_color_slot = None;
                    cx.notify();
                }),
            ))
            .child(choice_button(
                "hud-tab-layout",
                self.labels.tab_layout.clone(),
                active == HudTab::Layout,
                cx.listener(|this, _, _, cx| {
                    this.active_tab = HudTab::Layout;
                    this.open_color_slot = None;
                    cx.notify();
                }),
            ))
            .child(choice_button(
                "hud-tab-appearance",
                self.labels.tab_appearance.clone(),
                active == HudTab::Appearance,
                cx.listener(|this, _, _, cx| {
                    this.active_tab = HudTab::Appearance;
                    cx.notify();
                }),
            ))
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

        let buttons = h_flex()
            .flex_wrap()
            .gap_1p5()
            .child(choice_button(
                "hud-metal-top-left",
                self.labels.position_labels[0].clone(),
                current == MetalHudAlignment::TopLeft,
                cx.listener(|this, _, _, cx| {
                    this.edit_metal(cx, |m| m.alignment = MetalHudAlignment::TopLeft);
                }),
            ))
            .child(choice_button(
                "hud-metal-top-right",
                self.labels.position_labels[1].clone(),
                current == MetalHudAlignment::TopRight,
                cx.listener(|this, _, _, cx| {
                    this.edit_metal(cx, |m| m.alignment = MetalHudAlignment::TopRight);
                }),
            ))
            .child(choice_button(
                "hud-metal-bottom-left",
                self.labels.position_labels[2].clone(),
                current == MetalHudAlignment::BottomLeft,
                cx.listener(|this, _, _, cx| {
                    this.edit_metal(cx, |m| m.alignment = MetalHudAlignment::BottomLeft);
                }),
            ))
            .child(choice_button(
                "hud-metal-bottom-right",
                self.labels.position_labels[3].clone(),
                current == MetalHudAlignment::BottomRight,
                cx.listener(|this, _, _, cx| {
                    this.edit_metal(cx, |m| m.alignment = MetalHudAlignment::BottomRight);
                }),
            ))
            .into_any_element();

        let mut panel = group_panel(cx);
        if self.is_editing_profile() {
            panel = panel.child(field_row(
                RowState {
                    label: self.labels.metal_alignment_label.clone(),
                    help: self.labels.metal_alignment_help.clone(),
                    stacked: true,
                    ..Default::default()
                },
                buttons,
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
