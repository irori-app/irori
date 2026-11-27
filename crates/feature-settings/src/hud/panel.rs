//! Performance HUD settings panel (MangoHud on Linux, Metal HUD on macOS).
//!
//! Supports both Global application defaults and Per-Game overrides (including
//! native Linux OpenGL legacy wrapper visibility and Steam Launch Options
//! snippet generation). All strings shown during `render()` are cached in
//! component state and refreshed on settings, language, or theme changes so
//! rendering allocates zero `String`s.

mod helpers;
mod labels;
mod position_grid;
mod sections;
mod sections_layout;
mod sections_metrics;
#[cfg(test)]
mod tests;

use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::slider::{SliderEvent, SliderState};
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, Focusable as _, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Task, Window,
    div,
};
use irori_core::hud::{
    DEFAULT_TOGGLE_HUD, DEFAULT_TOGGLE_PRESET, sanitize_hex6, sanitize_hud_label,
};
use irori_core::{
    GameHudMode, GameHudSettings, GameId, GlobalHudMode, GlobalHudSettings, HudBackend,
    HudColorPalette, HudOrientation, MangoHudProfile, MetalHudSettings, ResolvedHud,
    ThemeHudColors,
};
use irori_runtime::hud::hardware_linux::{
    LinuxHudEnv, cached_linux_hud_env, default_gpu_short_label, default_gpu_vendor, resolve_pci_dev,
};
use irori_runtime::hud::mangohud_conf::{
    ConfRenderContext, render_mangohud_conf, render_mangohud_presets,
};
use irori_services::mangohud::LiveMangoHudSync;
use irori_ui::theme::theme_hud_colors;
use irori_ui::{backend, models};

use crate::hud::color_wheel::{ColorWheelLabels, HudColorWheel};
use crate::hud::preview::HudPreview;
use crate::hud::shortcut_recorder::ShortcutRecorder;
use crate::i18n::LanguageStamp;
use crate::widgets::muted;

pub(super) use self::helpers::ColorSlot;
use self::helpers::{
    COLOR_SLOTS, CachedDisplay, DisplayInputs, HudOption, build_cached_display, build_gpu_options,
    custom_fps_text, effective_mangohud_profile, effective_metal_settings, global_hud_on,
    gpu_selection_key, hud_conf_name, is_fps_preset, legacy_hud_toggle, parse_gpu_selection_key,
    read_hud_settings, resolve_hud_for_scope, slider_event_value,
};
use self::labels::HudLabels;

fn color_pair_hex(slot: ColorSlot, colors: &ThemeHudColors) -> Option<String> {
    match slot {
        ColorSlot::Background => Some(sanitize_hex6(
            ColorSlot::Text.get(colors),
            ColorSlot::Text.fallback(),
        )),
        ColorSlot::Text => Some(sanitize_hex6(
            ColorSlot::Background.get(colors),
            ColorSlot::Background.fallback(),
        )),
        _ => None,
    }
}

/// Where [`HudPanel`] is mounted and which capabilities apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HudPanelScope {
    Global,
    PerGame {
        id: GameId,
        is_native_linux: bool,
        is_steam: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum HudTab {
    #[default]
    Metrics,
    Layout,
    Appearance,
}

/// Complete Performance HUD configuration panel for Global or Per-Game settings.
pub struct HudPanel {
    scope: HudPanelScope,
    backend: HudBackend,
    pinned_preview: bool,
    active_tab: HudTab,
    global: GlobalHudSettings,
    game: GameHudSettings,
    env: LinuxHudEnv,
    env_loaded: bool,
    /// Legacy `showMangohud` (Linux) or `showFps` (macOS) toggle, which
    /// counts as "On" while the global mode is Off.
    legacy_on: bool,
    labels: HudLabels,
    display: CachedDisplay,
    language: LanguageStamp,
    theme_stamp: (Hsla, Hsla, Hsla),

    preview: Entity<HudPreview>,
    shortcut: Entity<ShortcutRecorder>,
    preset_shortcut: Entity<ShortcutRecorder>,
    gpu_select: Entity<SelectState<Vec<HudOption>>>,
    gpu_label_input: Entity<InputState>,
    cpu_label_input: Entity<InputState>,
    fps_limit_input: Entity<InputState>,
    round_corners_slider: Entity<SliderState>,
    font_scale_slider: Entity<SliderState>,
    bg_alpha_slider: Entity<SliderState>,
    alpha_slider: Entity<SliderState>,
    color_inputs: [Entity<InputState>; 11],
    color_wheel: Entity<HudColorWheel>,
    open_color_slot: Option<ColorSlot>,
    slider_debounce: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl HudPanel {
    pub fn new(
        scope: HudPanelScope,
        backend_override: Option<HudBackend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let hud_backend = backend_override.unwrap_or_else(HudBackend::current);
        let (global, game) = read_hud_settings(&scope, cx);
        let active_profile = effective_mangohud_profile(&scope, &global, &game);
        let active_metal = effective_metal_settings(&scope, &global, &game);
        let labels = HudLabels::load();
        let cached_env = if hud_backend == HudBackend::MangoHud {
            cached_linux_hud_env().cloned()
        } else {
            None
        };
        let env_loaded = hud_backend != HudBackend::MangoHud || cached_env.is_some();
        let env = cached_env.unwrap_or_default();

        let preview = cx.new(|cx| {
            HudPreview::new(
                hud_backend,
                active_profile.clone(),
                active_metal.clone(),
                env.clone(),
                cx,
            )
        });

        let weak = cx.entity().downgrade();
        let shortcut = cx.new(|cx| {
            ShortcutRecorder::with_options(
                "hud-shortcut",
                DEFAULT_TOGGLE_HUD,
                active_profile.toggle_hud.clone(),
                false,
                move |combo, window, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.edit_mangohud(window, cx, |p| {
                            p.toggle_hud = combo.to_string();
                        });
                    });
                },
                window,
                cx,
            )
        });

        let weak_preset = cx.entity().downgrade();
        let preset_shortcut = cx.new(|cx| {
            ShortcutRecorder::with_options(
                "hud-preset-shortcut",
                DEFAULT_TOGGLE_PRESET,
                active_profile.toggle_preset.clone(),
                true,
                move |combo, window, cx| {
                    let _ = weak_preset.update(cx, |this, cx| {
                        this.edit_mangohud(window, cx, |p| {
                            p.toggle_preset = combo.to_string();
                        });
                    });
                },
                window,
                cx,
            )
        });

        let gpu_items = build_gpu_options(&labels, &env, &active_profile.gpu_selection);
        let gpu_key = gpu_selection_key(&active_profile.gpu_selection, &env);
        let gpu_select = cx.new(|cx| {
            let mut s = SelectState::new(gpu_items, None, window, cx);
            s.set_selected_value(&gpu_key, window, cx);
            s
        });

        let gpu_placeholder = default_gpu_short_label(&active_profile.gpu_selection, &env.gpus);
        let gpu_label_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder(gpu_placeholder);
            s.set_value(active_profile.gpu_label_override.clone(), window, cx);
            s
        });

        let cpu_placeholder = if env.cpu_label.trim().is_empty() {
            "CPU".to_string()
        } else {
            env.cpu_label.clone()
        };
        let cpu_label_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder(cpu_placeholder);
            s.set_value(active_profile.cpu_label_override.clone(), window, cx);
            s
        });

        let fps_limit_text = custom_fps_text(active_profile.fps_limit);
        let fps_limit_placeholder = labels.fps_limit_placeholder.clone();
        let fps_limit_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder(fps_limit_placeholder);
            s.set_value(fps_limit_text, window, cx);
            s
        });

        let round_corners_slider = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(24.)
                .step(1.)
                .default_value(f32::from(active_profile.round_corners))
        });
        let font_scale_slider = cx.new(|_| {
            SliderState::new()
                .min(0.5)
                .max(2.0)
                .step(0.1)
                .default_value(active_profile.font_scale.clamp(0.5, 2.0))
        });
        let bg_alpha_slider = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(1.0)
                .step(0.05)
                .default_value(active_profile.background_alpha.clamp(0.0, 1.0))
        });
        let alpha_slider = cx.new(|_| {
            SliderState::new()
                .min(0.1)
                .max(1.0)
                .step(0.05)
                .default_value(active_profile.alpha.clamp(0.1, 1.0))
        });

        let color_inputs: [Entity<InputState>; 11] = std::array::from_fn(|ix| {
            let slot = COLOR_SLOTS[ix];
            let initial = sanitize_hex6(slot.get(&active_profile.custom_colors), slot.fallback());
            cx.new(|cx| {
                let mut s = InputState::new(window, cx).placeholder(slot.fallback());
                s.set_value(initial, window, cx);
                s
            })
        });

        let initial_slot = ColorSlot::Background;
        let initial_wheel_hex = sanitize_hex6(
            initial_slot.get(&active_profile.custom_colors),
            initial_slot.fallback(),
        );
        let initial_pair_hex = color_pair_hex(initial_slot, &active_profile.custom_colors);
        let weak_color = cx.entity().downgrade();
        let color_wheel = cx.new(|cx| {
            HudColorWheel::new(
                initial_slot,
                &initial_wheel_hex,
                initial_pair_hex.as_deref(),
                ColorWheelLabels {
                    slot: labels.color_slot_labels[initial_slot as usize].clone(),
                    brightness: labels.color_brightness.clone(),
                    quick_swatches: labels.color_quick_swatches.clone(),
                    reset_slot: labels.color_reset_slot.clone(),
                    copy_hex: labels.color_copy_hex.clone(),
                    copied_hex: labels.color_copied_hex.clone(),
                },
                move |slot, hex, released, window, cx| {
                    let _ = weak_color.update(cx, |this, cx| {
                        this.edit_mangohud_color(slot, hex, released, window, cx);
                    });
                },
                window,
                cx,
            )
        });

        let mut subscriptions = Vec::new();

        subscriptions.push(cx.subscribe_in(
            &gpu_select,
            window,
            |this, _, event: &SelectEvent<Vec<HudOption>>, window, cx| {
                if let SelectEvent::Confirm(Some(key)) = event {
                    let selection = parse_gpu_selection_key(key, &this.env);
                    this.edit_mangohud(window, cx, |p| p.gpu_selection = selection);
                    this.sync_placeholders(window, cx);
                }
            },
        ));

        subscriptions.push(cx.subscribe_in(
            &gpu_label_input,
            window,
            |this, state, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    let clean = sanitize_hud_label(state.read(cx).value().as_ref());
                    if state.read(cx).value().as_ref() != clean {
                        state.update(cx, |s, cx| s.set_value(clean.clone(), window, cx));
                    }
                    this.edit_mangohud(window, cx, |p| p.gpu_label_override = clean);
                }
            },
        ));

        subscriptions.push(cx.subscribe_in(
            &cpu_label_input,
            window,
            |this, state, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    let clean = sanitize_hud_label(state.read(cx).value().as_ref());
                    if state.read(cx).value().as_ref() != clean {
                        state.update(cx, |s, cx| s.set_value(clean.clone(), window, cx));
                    }
                    this.edit_mangohud(window, cx, |p| p.cpu_label_override = clean);
                }
            },
        ));

        subscriptions.push(cx.subscribe_in(
            &fps_limit_input,
            window,
            |this, state, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    let raw = state.read(cx).value().trim().to_string();
                    let current = this.active_mangohud().fps_limit;
                    let parsed = raw
                        .parse::<u32>()
                        .ok()
                        .filter(|&v| v > 0)
                        .map(|v| v.min(1000));
                    // The field only holds values outside the shortcut chips, so an
                    // empty field clears a custom limit but keeps a chip selection.
                    let next = match parsed {
                        Some(v) => Some(v),
                        None if raw.is_empty() => current.filter(|&v| is_fps_preset(v)),
                        None => current,
                    };
                    let normalized = custom_fps_text(next);
                    if state.read(cx).value().as_ref() != normalized {
                        state.update(cx, |s, cx| s.set_value(normalized, window, cx));
                    }
                    if next != current {
                        this.edit_mangohud(window, cx, |p| p.fps_limit = next);
                    }
                }
            },
        ));

        subscriptions.push(cx.subscribe_in(
            &round_corners_slider,
            window,
            |this, _, event: &SliderEvent, window, cx| {
                let (v, released) = slider_event_value(event);
                let rounded = v.round().clamp(0.0, 24.0) as u8;
                this.edit_mangohud_slider(released, window, cx, |p| p.round_corners = rounded);
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &font_scale_slider,
            window,
            |this, _, event: &SliderEvent, window, cx| {
                let (v, released) = slider_event_value(event);
                let scaled = ((v.clamp(0.5, 2.0) * 10.0).round()) / 10.0;
                this.edit_mangohud_slider(released, window, cx, |p| p.font_scale = scaled);
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &bg_alpha_slider,
            window,
            |this, _, event: &SliderEvent, window, cx| {
                let (v, released) = slider_event_value(event);
                let alpha = ((v.clamp(0.0, 1.0) * 20.0).round()) / 20.0;
                this.edit_mangohud_slider(released, window, cx, |p| p.background_alpha = alpha);
            },
        ));
        subscriptions.push(cx.subscribe_in(
            &alpha_slider,
            window,
            |this, _, event: &SliderEvent, window, cx| {
                let (v, released) = slider_event_value(event);
                let alpha = ((v.clamp(0.1, 1.0) * 20.0).round()) / 20.0;
                this.edit_mangohud_slider(released, window, cx, |p| p.alpha = alpha);
            },
        ));

        for slot in COLOR_SLOTS {
            let input = &color_inputs[slot as usize];
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                move |this, state, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
                        let raw = state.read(cx).value().to_string();
                        let hex = sanitize_hex6(&raw, slot.fallback());
                        if state.read(cx).value().as_ref() != hex {
                            state.update(cx, |s, cx| s.set_value(hex.clone(), window, cx));
                        }
                        this.edit_mangohud(window, cx, |p| {
                            slot.set(&mut p.custom_colors, hex.clone());
                        });
                        if this.open_color_slot == Some(slot) {
                            let pair = color_pair_hex(slot, &this.active_mangohud().custom_colors);
                            this.color_wheel.update(cx, |w, cx| {
                                w.set_hex_if_slot(slot, &hex, pair.as_deref(), window, cx);
                            });
                        }
                    }
                },
            ));
        }

        let settings_model = models(cx).settings.clone();
        subscriptions.push(
            cx.observe_in(&settings_model, window, |this, _, window, cx| {
                this.reload_from_settings(window, cx)
            }),
        );

        let legacy_on = legacy_hud_toggle(&scope, hud_backend, cx);
        let display = build_cached_display(
            DisplayInputs {
                scope: &scope,
                hud_backend,
                global: &global,
                game: &game,
                legacy_on,
                env: &env,
                labels: &labels,
            },
            cx,
        );
        let theme = cx.theme();
        let theme_stamp = (theme.background, theme.foreground, theme.primary);

        let panel = Self {
            scope,
            backend: hud_backend,
            pinned_preview: false,
            active_tab: HudTab::Metrics,
            global,
            game,
            env,
            env_loaded,
            legacy_on,
            labels,
            display,
            language: LanguageStamp::new(),
            theme_stamp,
            preview,
            shortcut,
            preset_shortcut,
            gpu_select,
            gpu_label_input,
            cpu_label_input,
            fps_limit_input,
            round_corners_slider,
            font_scale_slider,
            bg_alpha_slider,
            alpha_slider,
            color_inputs,
            color_wheel,
            open_color_slot: None,
            slider_debounce: None,
            _subscriptions: subscriptions,
        };

        if hud_backend == HudBackend::MangoHud && !env_loaded {
            let detect_task = irori_ui::state::run_infallible(cx, async {
                tokio::task::spawn_blocking(
                    irori_runtime::hud::hardware_linux::detect_linux_hud_env,
                )
                .await
                .cloned()
                .unwrap_or_else(|_| {
                    irori_runtime::hud::hardware_linux::detect_linux_hud_env().clone()
                })
            });
            cx.spawn_in(window, async move |this, cx| {
                if let Some(env) = detect_task.await {
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.env = env;
                        this.env_loaded = true;
                        this.rebuild_gpu_options(window, cx);
                        this.sync_placeholders(window, cx);
                        this.sync_preview_and_display(cx);
                        this.sync_live_mangohud_config(cx);
                        cx.notify();
                    });
                }
            })
            .detach();
        } else if hud_backend == HudBackend::MangoHud {
            panel.sync_live_mangohud_config(cx);
        }

        panel
    }

    /// Active backend rendered by this panel.
    pub fn backend(&self) -> HudBackend {
        self.backend
    }

    /// Pin the HUD preview at the top of the panel with an inner scrollable
    /// body below (used by the standalone Global settings page).
    pub fn with_pinned_preview(mut self, pinned: bool) -> Self {
        self.pinned_preview = pinned;
        self
    }

    fn is_steam(&self) -> bool {
        matches!(self.scope, HudPanelScope::PerGame { is_steam: true, .. })
    }

    fn is_third_party_launcher(&self) -> bool {
        matches!(
            self.scope,
            HudPanelScope::PerGame { ref id, .. } if id.runner == irori_core::Runner::Launchers
        )
    }

    fn scope_notice(&self) -> Option<(&'static str, &SharedString)> {
        match self.scope {
            HudPanelScope::Global => Some((
                "hud-global-stores-notice",
                &self.labels.global_stores_notice,
            )),
            HudPanelScope::PerGame { .. } if self.is_third_party_launcher() => Some((
                "hud-launcher-restart-notice",
                &self.labels.launcher_restart_notice,
            )),
            HudPanelScope::PerGame { .. } => None,
        }
    }

    fn show_opengl_legacy(&self) -> bool {
        self.backend == HudBackend::MangoHud
            && matches!(
                self.scope,
                HudPanelScope::PerGame {
                    is_native_linux: true,
                    is_steam: false,
                    ..
                }
            )
    }

    fn is_editing_profile(&self) -> bool {
        match &self.scope {
            HudPanelScope::Global => self.global_on(),
            HudPanelScope::PerGame { .. } => self.game.mode == GameHudMode::Custom,
        }
    }

    /// Whether the global mode counts as "On", including the legacy toggle.
    fn global_on(&self) -> bool {
        global_hud_on(&self.global, self.legacy_on)
    }

    fn active_mangohud(&self) -> &MangoHudProfile {
        match &self.scope {
            HudPanelScope::Global => &self.global.mangohud,
            HudPanelScope::PerGame { .. } => self
                .game
                .custom_mangohud
                .as_ref()
                .unwrap_or(&self.global.mangohud),
        }
    }

    fn active_metal(&self) -> &MetalHudSettings {
        match &self.scope {
            HudPanelScope::Global => &self.global.metal,
            HudPanelScope::PerGame { .. } => self
                .game
                .custom_metal
                .as_ref()
                .unwrap_or(&self.global.metal),
        }
    }

    fn persist_current(&mut self, cx: &mut Context<Self>) {
        self.persist_current_with_updates(None, None, cx);
    }

    fn persist_current_with_updates(
        &mut self,
        fps_limit_update: Option<Option<u32>>,
        no_display_update: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        let mut sync_global_too = false;
        match &self.scope {
            HudPanelScope::Global => {
                let mut next = self.global.clone();
                if let Some(mut v) = next.mangohud.saved_custom.clone() {
                    v.orientation = HudOrientation::Vertical;
                    next.mangohud.saved_custom = Some(v.clone());
                    next.custom_preset = Some(v);
                }
                if let Some(mut h) = next.mangohud.saved_custom_horizontal.clone() {
                    h.orientation = HudOrientation::Horizontal;
                    next.mangohud.saved_custom_horizontal = Some(h.clone());
                    next.custom_horizontal_preset = Some(h);
                }
                if next.mangohud.palette == HudColorPalette::SyncIroriTheme {
                    next.cached_theme_colors = Some(theme_hud_colors(cx.theme()));
                }
                self.global = next.clone();
                backend(cx).settings.update_app(|a| {
                    a.hud = next;
                });
            }
            HudPanelScope::PerGame { id, .. } => {
                if let Some(profile) = self.game.custom_mangohud.as_mut() {
                    if let Some(mut v) = profile.saved_custom.clone() {
                        v.orientation = HudOrientation::Vertical;
                        profile.saved_custom = Some(v.clone());
                        if self.global.custom_preset.as_ref() != Some(&v) {
                            self.global.custom_preset = Some(v);
                            sync_global_too = true;
                        }
                    }
                    if let Some(mut h) = profile.saved_custom_horizontal.clone() {
                        h.orientation = HudOrientation::Horizontal;
                        profile.saved_custom_horizontal = Some(h.clone());
                        if self.global.custom_horizontal_preset.as_ref() != Some(&h) {
                            self.global.custom_horizontal_preset = Some(h);
                            sync_global_too = true;
                        }
                    }
                }
                if sync_global_too {
                    let mut global_mangohud = self.global.mangohud.clone();
                    self.global.hydrate_profile(&mut global_mangohud);
                    self.global.mangohud = global_mangohud;
                    let next_global = self.global.clone();
                    backend(cx).settings.update_app(|a| {
                        a.hud = next_global;
                    });
                }
                let next = self.game.clone();
                let value = if next == GameHudSettings::default() {
                    None
                } else {
                    serde_json::to_value(&next).ok()
                };
                backend(cx)
                    .settings
                    .set_game_value(id, "iroriGameHud", value);
            }
        }
        self.sync_live_mangohud_config_with_updates(fps_limit_update, no_display_update, cx);
        if sync_global_too {
            self.sync_global_mangohud_if_active(cx);
        }
    }

    /// Rewrite `<data>/mangohud/<scope>_presets.conf` and `<scope>.conf` in
    /// place and sync matching running sessions so Steam, external launchers,
    /// gamescope and Irori-launched games pick up edits live via `inotify`.
    pub(super) fn sync_live_mangohud_config(&self, cx: &App) {
        self.sync_live_mangohud_config_with_updates(None, None, cx);
    }

    fn sync_live_mangohud_config_with_updates(
        &self,
        fps_limit_update: Option<Option<u32>>,
        no_display_update: Option<bool>,
        cx: &App,
    ) {
        if self.backend != HudBackend::MangoHud {
            return;
        }
        let resolved =
            resolve_hud_for_scope(&self.scope, self.backend, &self.global, &self.game, cx);
        let ResolvedHud::MangoHudProfile(profile) = resolved else {
            return;
        };
        let conf_name = hud_conf_name(&self.scope, &self.game);
        self.dispatch_live_mangohud_sync(
            conf_name,
            &profile,
            fps_limit_update,
            no_display_update,
            cx,
        );
    }

    fn sync_global_mangohud_if_active(&self, cx: &App) {
        if self.backend != HudBackend::MangoHud {
            return;
        }
        let resolved = resolve_hud_for_scope(
            &HudPanelScope::Global,
            self.backend,
            &self.global,
            &GameHudSettings::default(),
            cx,
        );
        let ResolvedHud::MangoHudProfile(profile) = resolved else {
            return;
        };
        self.dispatch_live_mangohud_sync("global".to_string(), &profile, None, None, cx);
    }

    fn dispatch_live_mangohud_sync(
        &self,
        conf_name: String,
        profile: &MangoHudProfile,
        fps_limit_update: Option<Option<u32>>,
        no_display_update: Option<bool>,
        cx: &App,
    ) {
        let ctx = ConfRenderContext {
            detected_gpu_label: default_gpu_short_label(&profile.gpu_selection, &self.env.gpus),
            detected_cpu_label: self.env.cpu_label.clone(),
            gpu_vendor: default_gpu_vendor(&profile.gpu_selection, &self.env.gpus),
            cpu_vendor: self.env.cpu_vendor,
            pci_dev: resolve_pci_dev(&profile.gpu_selection, &self.env.gpus),
            theme_colors: Some(theme_hud_colors(cx.theme())),
        };
        let sync = LiveMangoHudSync {
            conf_name: conf_name.clone(),
            config_body: render_mangohud_conf(profile, &ctx),
            presets_body: render_mangohud_presets(profile),
            fps_limit_update,
            no_display_update,
        };
        let backend = backend(cx).clone();
        irori_ui::run(cx, async move {
            if let Err(err) = backend.sync_live_mangohud(sync).await {
                tracing::warn!("writing live MangoHud config {conf_name}: {err:#}");
            }
            Ok(())
        })
        .detach();
    }

    fn edit_mangohud(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut MangoHudProfile),
    ) {
        let prev = self.active_mangohud();
        let prev_fps_limit = prev.fps_limit;
        let prev_no_display = prev.no_display;
        match &self.scope {
            HudPanelScope::Global => {
                f(&mut self.global.mangohud);
            }
            HudPanelScope::PerGame { .. } => {
                self.game.ensure_custom_from(&self.global);
                if let Some(profile) = self.game.custom_mangohud.as_mut() {
                    f(profile);
                }
            }
        }
        let next = self.active_mangohud();
        let fps_limit_update = (next.fps_limit != prev_fps_limit).then_some(next.fps_limit);
        let no_display_update = (next.no_display != prev_no_display).then_some(next.no_display);
        self.persist_current_with_updates(fps_limit_update, no_display_update, cx);
        self.sync_controls(window, cx);
        self.sync_preview_and_display(cx);
        cx.notify();
    }

    fn edit_mangohud_slider(
        &mut self,
        released: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut MangoHudProfile),
    ) {
        match &self.scope {
            HudPanelScope::Global => {
                f(&mut self.global.mangohud);
            }
            HudPanelScope::PerGame { .. } => {
                self.game.ensure_custom_from(&self.global);
                if let Some(profile) = self.game.custom_mangohud.as_mut() {
                    f(profile);
                }
            }
        }
        self.sync_preview_and_display(cx);
        cx.notify();

        self.slider_debounce = None;
        if released {
            self.persist_current(cx);
        } else {
            self.slider_debounce = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.slider_debounce = None;
                    this.persist_current(cx);
                });
            }));
        }
    }

    pub(super) fn on_color_popover_change(
        &mut self,
        slot: ColorSlot,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if open {
            self.open_color_slot = Some(slot);
            let colors = &self.active_mangohud().custom_colors;
            let hex = sanitize_hex6(slot.get(colors), slot.fallback());
            let pair = color_pair_hex(slot, colors);
            let slot_label = self.labels.color_slot_labels[slot as usize].clone();
            self.color_wheel.update(cx, |w, cx| {
                w.set_slot(slot, slot_label, &hex, pair.as_deref(), window, cx);
            });
            cx.notify();
        } else if self.open_color_slot == Some(slot) {
            self.open_color_slot = None;
            cx.notify();
        }
    }

    pub(super) fn reset_custom_colors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.edit_mangohud(window, cx, |p| {
            p.custom_colors = ThemeHudColors::default();
        });
    }

    fn edit_mangohud_color(
        &mut self,
        slot: ColorSlot,
        hex: String,
        released: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let clean = sanitize_hex6(&hex, slot.fallback());
        let input = &self.color_inputs[slot as usize];
        if input.read(cx).value().as_ref() != clean {
            input.update(cx, |s, cx| s.set_value(clean.clone(), window, cx));
        }
        self.edit_mangohud_slider(released, window, cx, |p| {
            slot.set(&mut p.custom_colors, clean);
        });
    }

    fn edit_metal(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut MetalHudSettings)) {
        match &self.scope {
            HudPanelScope::Global => {
                f(&mut self.global.metal);
            }
            HudPanelScope::PerGame { .. } => {
                self.game.ensure_custom_from(&self.global);
                if let Some(metal) = self.game.custom_metal.as_mut() {
                    f(metal);
                }
            }
        }
        self.persist_current(cx);
        self.sync_preview_and_display(cx);
        cx.notify();
    }

    fn set_global_mode(
        &mut self,
        mode: GlobalHudMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let clears_legacy = mode == GlobalHudMode::Off && self.legacy_on;
        if self.global.mode == mode && !clears_legacy {
            return;
        }
        self.global.mode = mode;
        if clears_legacy {
            // Off must match what games show, so the legacy toggle goes off too.
            let metal = self.backend == HudBackend::MetalHud;
            backend(cx).settings.update_app(|a| {
                a.game.show_mangohud = false;
                if metal {
                    a.game.show_fps = false;
                }
            });
            self.legacy_on = false;
        }
        self.persist_current(cx);
        self.sync_controls(window, cx);
        self.sync_preview_and_display(cx);
        cx.notify();
    }

    fn set_game_mode(&mut self, mode: GameHudMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.game.mode == mode {
            return;
        }
        self.game.mode = mode;
        if mode == GameHudMode::Custom {
            self.game.ensure_custom_from(&self.global);
        }
        self.persist_current(cx);
        self.sync_controls(window, cx);
        self.sync_preview_and_display(cx);
        cx.notify();
    }

    fn restore_game_from_global(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.scope, HudPanelScope::PerGame { .. }) {
            return;
        }
        let prev = self.active_mangohud();
        let prev_fps_limit = prev.fps_limit;
        let prev_no_display = prev.no_display;
        self.game.restore_from_global(&self.global);
        let next = self.active_mangohud();
        let fps_limit_update = (next.fps_limit != prev_fps_limit).then_some(next.fps_limit);
        let no_display_update = (next.no_display != prev_no_display).then_some(next.no_display);
        self.persist_current_with_updates(fps_limit_update, no_display_update, cx);
        self.sync_controls(window, cx);
        self.sync_preview_and_display(cx);
        cx.notify();
    }

    fn reload_from_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (global, game) = read_hud_settings(&self.scope, cx);
        let legacy_on = legacy_hud_toggle(&self.scope, self.backend, cx);
        if self.global == global && self.game == game && self.legacy_on == legacy_on {
            return;
        }
        self.global = global;
        self.game = game;
        self.legacy_on = legacy_on;
        self.sync_controls(window, cx);
        self.sync_preview_and_display(cx);
        cx.notify();
    }

    fn rebuild_gpu_options(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self.active_mangohud().gpu_selection.clone();
        let items = build_gpu_options(&self.labels, &self.env, &selection);
        let key = gpu_selection_key(&selection, &self.env);
        self.gpu_select.update(cx, |s, cx| {
            s.set_items(items, window, cx);
            s.set_selected_value(&key, window, cx);
        });
    }

    fn sync_placeholders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let gpu_ph = default_gpu_short_label(&self.active_mangohud().gpu_selection, &self.env.gpus);
        let cpu_ph = if self.env.cpu_label.trim().is_empty() {
            "CPU".to_string()
        } else {
            self.env.cpu_label.clone()
        };
        let fps_ph = self.labels.fps_limit_placeholder.clone();
        self.gpu_label_input
            .update(cx, |s, cx| s.set_placeholder(gpu_ph, window, cx));
        self.cpu_label_input
            .update(cx, |s, cx| s.set_placeholder(cpu_ph, window, cx));
        self.fps_limit_input
            .update(cx, |s, cx| s.set_placeholder(fps_ph, window, cx));
    }

    fn sync_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let profile = self.active_mangohud().clone();

        self.shortcut
            .update(cx, |s, cx| s.set_combo(profile.toggle_hud.clone(), cx));
        self.preset_shortcut
            .update(cx, |s, cx| s.set_combo(profile.toggle_preset.clone(), cx));

        let gpu_key = gpu_selection_key(&profile.gpu_selection, &self.env);
        self.gpu_select
            .update(cx, |s, cx| s.set_selected_value(&gpu_key, window, cx));

        if !self
            .gpu_label_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
            && self.gpu_label_input.read(cx).value().as_ref() != profile.gpu_label_override
        {
            let val = profile.gpu_label_override.clone();
            self.gpu_label_input
                .update(cx, |s, cx| s.set_value(val, window, cx));
        }

        if !self
            .cpu_label_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
            && self.cpu_label_input.read(cx).value().as_ref() != profile.cpu_label_override
        {
            let val = profile.cpu_label_override.clone();
            self.cpu_label_input
                .update(cx, |s, cx| s.set_value(val, window, cx));
        }

        let fps_text = custom_fps_text(profile.fps_limit);
        if !self
            .fps_limit_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
            && self.fps_limit_input.read(cx).value().as_ref() != fps_text
        {
            self.fps_limit_input
                .update(cx, |s, cx| s.set_value(fps_text, window, cx));
        }

        self.round_corners_slider.update(cx, |s, cx| {
            s.set_value(f32::from(profile.round_corners), window, cx);
        });
        self.font_scale_slider.update(cx, |s, cx| {
            s.set_value(profile.font_scale.clamp(0.5, 2.0), window, cx);
        });
        self.bg_alpha_slider.update(cx, |s, cx| {
            s.set_value(profile.background_alpha.clamp(0.0, 1.0), window, cx);
        });
        self.alpha_slider.update(cx, |s, cx| {
            s.set_value(profile.alpha.clamp(0.1, 1.0), window, cx);
        });

        for slot in COLOR_SLOTS {
            let input = &self.color_inputs[slot as usize];
            if !input.read(cx).focus_handle(cx).is_focused(window) {
                let hex = sanitize_hex6(slot.get(&profile.custom_colors), slot.fallback());
                if input.read(cx).value().as_ref() != hex {
                    input.update(cx, |s, cx| s.set_value(hex, window, cx));
                }
            }
        }
        if let Some(slot) = self.open_color_slot
            && !self.color_wheel.read(cx).is_dragging()
        {
            let hex = sanitize_hex6(slot.get(&profile.custom_colors), slot.fallback());
            let pair = color_pair_hex(slot, &profile.custom_colors);
            self.color_wheel.update(cx, |w, cx| {
                w.set_hex_if_slot(slot, &hex, pair.as_deref(), window, cx);
            });
        }
    }

    fn sync_preview_and_display(&mut self, cx: &mut Context<Self>) {
        let profile = self.active_mangohud().clone();
        let metal = self.active_metal().clone();
        let env = self.env.clone();
        let backend = self.backend;
        self.preview
            .update(cx, |p, cx| p.set_state(backend, profile, metal, env, cx));
        self.rebuild_display(cx);
    }

    fn rebuild_display(&mut self, cx: &App) {
        let theme = cx.theme();
        self.theme_stamp = (theme.background, theme.foreground, theme.primary);
        self.display = build_cached_display(
            DisplayInputs {
                scope: &self.scope,
                hud_backend: self.backend,
                global: &self.global,
                game: &self.game,
                legacy_on: self.legacy_on,
                env: &self.env,
                labels: &self.labels,
            },
            cx,
        );
    }

    fn retranslate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.labels = HudLabels::load();
        self.rebuild_gpu_options(window, cx);
        let slot_label = self
            .open_color_slot
            .map(|s| self.labels.color_slot_labels[s as usize].clone());
        let wheel_labels = ColorWheelLabels {
            slot: slot_label.clone().unwrap_or_default(),
            brightness: self.labels.color_brightness.clone(),
            quick_swatches: self.labels.color_quick_swatches.clone(),
            reset_slot: self.labels.color_reset_slot.clone(),
            copy_hex: self.labels.color_copy_hex.clone(),
            copied_hex: self.labels.color_copied_hex.clone(),
        };
        self.color_wheel.update(cx, |w, cx| {
            w.set_labels(wheel_labels, slot_label.is_some(), cx);
        });
        self.sync_placeholders(window, cx);
        self.sync_preview_and_display(cx);
        cx.notify();
    }
}

impl Render for HudPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.language.changed() {
            cx.defer_in(window, |this, window, cx| this.retranslate(window, cx));
        }
        let theme = cx.theme();
        if (theme.background, theme.foreground, theme.primary) != self.theme_stamp {
            // Palette sample colors follow the theme; rebuild only when it changes.
            self.rebuild_display(cx);
        }
        if self.backend == HudBackend::Unsupported {
            return div()
                .py_3()
                .child(muted(self.labels.unsupported.clone(), cx))
                .into_any_element();
        }

        let editing = self.is_editing_profile();

        let render_mangohud_tab_body = |this: &Self, cx: &mut Context<Self>| match this.active_tab {
            HudTab::Metrics => this.render_metrics_tab(cx),
            HudTab::Layout => this.render_layout_tab(cx),
            HudTab::Appearance => v_flex()
                .w_full()
                .gap_2()
                .child(this.render_colors_section(cx))
                .child(this.render_hardware_section(cx))
                .children(this.render_opengl_section(cx))
                .into_any_element(),
        };

        if self.pinned_preview {
            return match self.backend {
                HudBackend::MangoHud if editing => {
                    let tab_body = render_mangohud_tab_body(self, cx);
                    v_flex()
                        .size_full()
                        .min_h_0()
                        .gap_2()
                        .child(
                            v_flex()
                                .w_full()
                                .flex_shrink_0()
                                .gap_2()
                                .children(self.render_missing_mangohud())
                                .child(self.preview.clone()),
                        )
                        .child(
                            div()
                                .id("hud-panel-scroll")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scrollbar()
                                .child(
                                    v_flex()
                                        .w_full()
                                        .pb_8()
                                        .gap_2()
                                        .children(self.render_scope_notice(cx))
                                        .children(self.render_steam_section(cx))
                                        .child(self.render_top_quick_card(cx))
                                        .child(self.render_tab_bar(cx))
                                        .child(tab_body),
                                ),
                        )
                        .into_any_element()
                }
                HudBackend::MangoHud => div()
                    .id("hud-panel-scroll")
                    .size_full()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(
                        v_flex()
                            .w_full()
                            .pb_8()
                            .gap_2()
                            .children(self.render_missing_mangohud())
                            .children(self.render_scope_notice(cx))
                            .children(self.render_steam_section(cx))
                            .child(self.render_mode_section(cx))
                            .children(self.render_opengl_section(cx)),
                    )
                    .into_any_element(),
                HudBackend::MetalHud => div()
                    .id("hud-panel-scroll")
                    .size_full()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(
                        v_flex()
                            .w_full()
                            .pb_8()
                            .gap_2()
                            .children(self.render_scope_notice(cx))
                            .children(self.render_steam_section(cx))
                            .child(self.render_mode_section(cx))
                            .child(self.render_metal_section(cx))
                            .when(editing, |v| {
                                v.child(div().pt_4().child(self.preview.clone()))
                            }),
                    )
                    .into_any_element(),
                HudBackend::Unsupported => div().into_any_element(),
            };
        }

        match self.backend {
            HudBackend::MangoHud => v_flex()
                .w_full()
                .gap_2()
                .children(self.render_missing_mangohud())
                .children(self.render_scope_notice(cx))
                .children(self.render_steam_section(cx))
                .when(!editing, |v| {
                    v.child(self.render_mode_section(cx))
                        .children(self.render_opengl_section(cx))
                })
                .when(editing, |v| {
                    let tab_body = render_mangohud_tab_body(self, cx);
                    v.child(self.preview.clone())
                        .child(self.render_top_quick_card(cx))
                        .child(self.render_tab_bar(cx))
                        .child(tab_body)
                })
                .into_any_element(),
            HudBackend::MetalHud => v_flex()
                .w_full()
                .gap_2()
                .children(self.render_scope_notice(cx))
                .children(self.render_steam_section(cx))
                .child(self.render_mode_section(cx))
                .child(self.render_metal_section(cx))
                .when(editing, |v| {
                    v.child(div().pt_4().child(self.preview.clone()))
                })
                .into_any_element(),
            HudBackend::Unsupported => div().into_any_element(),
        }
    }
}
