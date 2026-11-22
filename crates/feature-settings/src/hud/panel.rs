//! Performance HUD settings panel (MangoHud on Linux, Metal HUD on macOS).
//!
//! Supports both Global application defaults and Per-Game overrides (including
//! native Linux OpenGL legacy wrapper visibility and Steam Launch Options
//! snippet generation). All strings shown during `render()` are cached in
//! component state and refreshed on settings, language, or theme changes so
//! rendering allocates zero `String`s.

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, Focusable as _, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Task, Window, div, px, rems, rgb,
};
use irori_core::hud::{resolve_effective_hud, sanitize_hex6, sanitize_hud_label};
use irori_core::{
    FpsLimitMethod, GameHudMode, GameHudSettings, GameId, GlobalHudMode, GlobalHudSettings,
    GpuSelection, HudBackend, HudColorPalette, HudOrientation, HudPosition, HudPreset,
    MangoHudProfile, MetalHudAlignment, MetalHudSettings, Paths, ResolvedHud, ThemeHudColors,
};
use irori_runtime::hud::hardware_linux::{
    LinuxHudEnv, cached_linux_hud_env, default_gpu_short_label, default_gpu_vendor, resolve_pci_dev,
};
use irori_runtime::hud::launch_env::steam_launch_snippet;
use irori_runtime::hud::mangohud_conf::{ConfRenderContext, render_mangohud_conf};
use irori_ui::i18n::{t, t_args};
use irori_ui::theme::theme_hud_colors;
use irori_ui::{backend, models};

use crate::hud::preview::HudPreview;
use crate::hud::shortcut_recorder::ShortcutRecorder;
use crate::i18n::LanguageStamp;
use crate::widgets::{RowState, field_row, group_heading, group_panel, muted};

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

#[derive(Debug, Clone)]
struct HudOption {
    key: SharedString,
    label: SharedString,
}

impl HudOption {
    fn new(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
        }
    }
}

impl SelectItem for HudOption {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &SharedString {
        &self.key
    }
}

const FPS_PRESETS: [(u32, &str, &str); 7] = [
    (30, "hud-fps-30", "30"),
    (60, "hud-fps-60", "60"),
    (90, "hud-fps-90", "90"),
    (120, "hud-fps-120", "120"),
    (144, "hud-fps-144", "144"),
    (165, "hud-fps-165", "165"),
    (240, "hud-fps-240", "240"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum ColorSlot {
    Background = 0,
    Text = 1,
    Gpu = 2,
    Cpu = 3,
    Vram = 4,
    Ram = 5,
    Engine = 6,
    Frametime = 7,
    Io = 8,
    Battery = 9,
    Accent = 10,
}

const COLOR_SLOTS: [ColorSlot; 11] = [
    ColorSlot::Background,
    ColorSlot::Text,
    ColorSlot::Gpu,
    ColorSlot::Cpu,
    ColorSlot::Vram,
    ColorSlot::Ram,
    ColorSlot::Engine,
    ColorSlot::Frametime,
    ColorSlot::Io,
    ColorSlot::Battery,
    ColorSlot::Accent,
];

impl ColorSlot {
    const fn id(self) -> &'static str {
        match self {
            Self::Background => "hud-color-background",
            Self::Text => "hud-color-text",
            Self::Gpu => "hud-color-gpu",
            Self::Cpu => "hud-color-cpu",
            Self::Vram => "hud-color-vram",
            Self::Ram => "hud-color-ram",
            Self::Engine => "hud-color-engine",
            Self::Frametime => "hud-color-frametime",
            Self::Io => "hud-color-io",
            Self::Battery => "hud-color-battery",
            Self::Accent => "hud-color-accent",
        }
    }

    fn get(self, c: &ThemeHudColors) -> &str {
        match self {
            Self::Background => &c.background_color,
            Self::Text => &c.text_color,
            Self::Gpu => &c.gpu_color,
            Self::Cpu => &c.cpu_color,
            Self::Vram => &c.vram_color,
            Self::Ram => &c.ram_color,
            Self::Engine => &c.engine_color,
            Self::Frametime => &c.frametime_color,
            Self::Io => &c.io_color,
            Self::Battery => &c.battery_color,
            Self::Accent => &c.accent_color,
        }
    }

    fn set(self, c: &mut ThemeHudColors, value: String) {
        match self {
            Self::Background => c.background_color = value,
            Self::Text => c.text_color = value,
            Self::Gpu => c.gpu_color = value,
            Self::Cpu => c.cpu_color = value,
            Self::Vram => c.vram_color = value,
            Self::Ram => c.ram_color = value,
            Self::Engine => c.engine_color = value,
            Self::Frametime => c.frametime_color = value,
            Self::Io => c.io_color = value,
            Self::Battery => c.battery_color = value,
            Self::Accent => c.accent_color = value,
        }
    }

    fn fallback(self) -> &'static str {
        match self {
            Self::Background => "020202",
            Self::Text => "FFFFFF",
            Self::Gpu => "2E9762",
            Self::Cpu => "2E97CB",
            Self::Vram => "AD64C1",
            Self::Ram => "C26693",
            Self::Engine => "EB5B5B",
            Self::Frametime => "00FF00",
            Self::Io => "A491D3",
            Self::Battery => "FF9078",
            Self::Accent => "EB5B5B",
        }
    }
}

struct HudLabels {
    unsupported: SharedString,
    group_mode: SharedString,
    mode_label: SharedString,
    mode_global_help: SharedString,
    mode_game_help: SharedString,
    mode_off: SharedString,
    mode_irori_profile: SharedString,
    mode_system_default: SharedString,
    mode_inherit_global: SharedString,
    mode_custom: SharedString,
    mode_disabled: SharedString,
    mode_off_note: SharedString,
    mode_system_default_mangohud_note: SharedString,
    mode_system_default_metal_note: SharedString,
    mode_disabled_note: SharedString,
    restore_from_global: SharedString,
    restore_from_global_help: SharedString,

    missing_title: SharedString,
    missing_description: SharedString,

    steam_title: SharedString,
    steam_description: SharedString,
    steam_snippet_label: SharedString,
    steam_copy: SharedString,
    steam_copied: SharedString,
    steam_none_needed: SharedString,

    tab_metrics: SharedString,
    tab_layout: SharedString,
    tab_appearance: SharedString,
    presets_label: SharedString,
    presets_help: SharedString,
    preset_minimal_fps: SharedString,
    preset_horizontal_compact: SharedString,
    preset_balanced_gaming: SharedString,
    preset_full_diagnostics: SharedString,
    preset_custom: SharedString,

    group_hardware: SharedString,
    gpu_selection_label: SharedString,
    gpu_selection_help: SharedString,
    gpu_selection_auto: SharedString,
    gpu_selection_all: SharedString,
    gpu_label_label: SharedString,
    cpu_label_label: SharedString,
    label_reset: SharedString,

    group_metrics_gpu: SharedString,
    metric_gpu_stats: SharedString,
    metric_gpu_stats_help: SharedString,
    metric_gpu_stats_locked: SharedString,
    metric_gpu_temp: SharedString,
    metric_gpu_temp_help: SharedString,
    metric_gpu_junction_temp: SharedString,
    metric_gpu_junction_temp_help: SharedString,
    metric_gpu_mem_temp: SharedString,
    metric_gpu_mem_temp_help: SharedString,
    metric_gpu_core_clock: SharedString,
    metric_gpu_core_clock_help: SharedString,
    metric_gpu_mem_clock: SharedString,
    metric_gpu_mem_clock_help: SharedString,
    metric_gpu_power: SharedString,
    metric_gpu_power_help: SharedString,
    metric_gpu_fan: SharedString,
    metric_gpu_fan_help: SharedString,
    metric_vram: SharedString,
    metric_vram_help: SharedString,

    group_metrics_cpu: SharedString,
    metric_cpu_stats: SharedString,
    metric_cpu_stats_help: SharedString,
    metric_cpu_stats_locked: SharedString,
    metric_cpu_temp: SharedString,
    metric_cpu_temp_help: SharedString,
    metric_cpu_mhz: SharedString,
    metric_cpu_mhz_help: SharedString,
    metric_cpu_power: SharedString,
    metric_cpu_power_help: SharedString,
    metric_ram: SharedString,
    metric_ram_help: SharedString,

    group_metrics_fps: SharedString,
    metric_fps: SharedString,
    metric_fps_help: SharedString,
    metric_fps_use_label: SharedString,
    metric_fps_use_label_help: SharedString,
    metric_fps_avg: SharedString,
    metric_fps_avg_help: SharedString,
    metric_fps_1_low: SharedString,
    metric_fps_1_low_help: SharedString,
    metric_frametime: SharedString,
    metric_frametime_help: SharedString,
    metric_frame_timing: SharedString,
    metric_frame_timing_help: SharedString,
    metric_present_mode: SharedString,
    metric_present_mode_help: SharedString,

    group_metrics_runtime: SharedString,
    metric_wine: SharedString,
    metric_wine_help: SharedString,
    metric_winesync: SharedString,
    metric_winesync_help: SharedString,
    metric_fsr: SharedString,
    metric_fsr_help: SharedString,
    metric_hdr: SharedString,
    metric_hdr_help: SharedString,
    metric_gamemode: SharedString,
    metric_gamemode_help: SharedString,
    metric_throttling_status: SharedString,
    metric_throttling_status_help: SharedString,
    metric_gamepad_battery: SharedString,
    metric_gamepad_battery_help: SharedString,

    fps_limit_label: SharedString,
    fps_limit_help: SharedString,
    fps_limit_off: SharedString,
    fps_limit_placeholder: SharedString,
    fps_limit_method_help: SharedString,
    fps_limit_method_late: SharedString,
    fps_limit_method_early: SharedString,

    position_label: SharedString,
    position_labels: [SharedString; 8],
    orientation_label: SharedString,
    orientation_vertical: SharedString,
    orientation_horizontal: SharedString,
    round_corners_label: SharedString,
    hud_compact_label: SharedString,
    hud_compact_help: SharedString,
    hud_no_margin_label: SharedString,
    hud_no_margin_help: SharedString,
    font_scale_label: SharedString,
    background_alpha_label: SharedString,
    alpha_label: SharedString,
    toggle_hud_label: SharedString,
    toggle_hud_help: SharedString,
    no_display_label: SharedString,
    no_display_help: SharedString,

    group_colors: SharedString,
    palette_label: SharedString,
    palette_help: SharedString,
    palette_mangohud_default: SharedString,
    palette_sync_irori_theme: SharedString,
    palette_monochrome: SharedString,
    palette_custom: SharedString,
    fps_color_change_label: SharedString,
    fps_color_change_help: SharedString,
    color_slot_labels: [SharedString; 11],

    group_opengl: SharedString,
    opengl_wrapper_label: SharedString,
    opengl_wrapper_help: SharedString,
    mangohud_dlsym_label: SharedString,
    mangohud_dlsym_help: SharedString,

    group_metal: SharedString,
    metal_alignment_label: SharedString,
    metal_alignment_help: SharedString,
    metal_note: SharedString,
}

impl HudLabels {
    fn load() -> Self {
        Self {
            unsupported: t("settings.hud.unsupported"),
            group_mode: t("settings.hud.group.mode"),
            mode_label: t("settings.hud.mode.label"),
            mode_global_help: t("settings.hud.mode.global_help"),
            mode_game_help: t("settings.hud.mode.game_help"),
            mode_off: t("settings.hud.mode.off"),
            mode_irori_profile: t("settings.hud.mode.irori_profile"),
            mode_system_default: t("settings.hud.mode.system_default"),
            mode_inherit_global: t("settings.hud.mode.inherit_global"),
            mode_custom: t("settings.hud.mode.custom"),
            mode_disabled: t("settings.hud.mode.disabled"),
            mode_off_note: t("settings.hud.mode.off_note"),
            mode_system_default_mangohud_note: t("settings.hud.mode.system_default_mangohud_note"),
            mode_system_default_metal_note: t("settings.hud.mode.system_default_metal_note"),
            mode_disabled_note: t("settings.hud.mode.disabled_note"),
            restore_from_global: t("settings.hud.restore_from_global"),
            restore_from_global_help: t("settings.hud.restore_from_global_help"),

            missing_title: t("settings.hud.missing.title"),
            missing_description: t("settings.hud.missing.description"),

            steam_title: t("settings.hud.steam.title"),
            steam_description: t("settings.hud.steam.description"),
            steam_snippet_label: t("settings.hud.steam.snippet_label"),
            steam_copy: t("settings.hud.steam.copy"),
            steam_copied: t("settings.hud.steam.copied"),
            steam_none_needed: t("settings.hud.steam.none_needed"),

            tab_metrics: t("settings.hud.tab.metrics"),
            tab_layout: t("settings.hud.tab.layout"),
            tab_appearance: t("settings.hud.tab.appearance"),
            presets_label: t("settings.hud.presets.label"),
            presets_help: t("settings.hud.presets.help"),
            preset_minimal_fps: t("settings.hud.preset.minimal_fps"),
            preset_horizontal_compact: t("settings.hud.preset.horizontal_compact"),
            preset_balanced_gaming: t("settings.hud.preset.balanced_gaming"),
            preset_full_diagnostics: t("settings.hud.preset.full_diagnostics"),
            preset_custom: t("settings.hud.preset.custom"),

            group_hardware: t("settings.hud.group.hardware"),
            gpu_selection_label: t("settings.hud.gpu_selection.label"),
            gpu_selection_help: t("settings.hud.gpu_selection.help"),
            gpu_selection_auto: t("settings.hud.gpu_selection.auto_dedicated"),
            gpu_selection_all: t("settings.hud.gpu_selection.all_gpus"),
            gpu_label_label: t("settings.hud.gpu_label.label"),
            cpu_label_label: t("settings.hud.cpu_label.label"),
            label_reset: t("settings.hud.label.reset"),

            group_metrics_gpu: t("settings.hud.group.metrics_gpu"),
            metric_gpu_stats: t("settings.hud.metric.gpu_stats"),
            metric_gpu_stats_help: t("settings.hud.metric.gpu_stats_help"),
            metric_gpu_stats_locked: t("settings.hud.metric.gpu_stats_locked"),
            metric_gpu_temp: t("settings.hud.metric.gpu_temp"),
            metric_gpu_temp_help: t("settings.hud.metric.gpu_temp_help"),
            metric_gpu_junction_temp: t("settings.hud.metric.gpu_junction_temp"),
            metric_gpu_junction_temp_help: t("settings.hud.metric.gpu_junction_temp_help"),
            metric_gpu_mem_temp: t("settings.hud.metric.gpu_mem_temp"),
            metric_gpu_mem_temp_help: t("settings.hud.metric.gpu_mem_temp_help"),
            metric_gpu_core_clock: t("settings.hud.metric.gpu_core_clock"),
            metric_gpu_core_clock_help: t("settings.hud.metric.gpu_core_clock_help"),
            metric_gpu_mem_clock: t("settings.hud.metric.gpu_mem_clock"),
            metric_gpu_mem_clock_help: t("settings.hud.metric.gpu_mem_clock_help"),
            metric_gpu_power: t("settings.hud.metric.gpu_power"),
            metric_gpu_power_help: t("settings.hud.metric.gpu_power_help"),
            metric_gpu_fan: t("settings.hud.metric.gpu_fan"),
            metric_gpu_fan_help: t("settings.hud.metric.gpu_fan_help"),
            metric_vram: t("settings.hud.metric.vram"),
            metric_vram_help: t("settings.hud.metric.vram_help"),

            group_metrics_cpu: t("settings.hud.group.metrics_cpu"),
            metric_cpu_stats: t("settings.hud.metric.cpu_stats"),
            metric_cpu_stats_help: t("settings.hud.metric.cpu_stats_help"),
            metric_cpu_stats_locked: t("settings.hud.metric.cpu_stats_locked"),
            metric_cpu_temp: t("settings.hud.metric.cpu_temp"),
            metric_cpu_temp_help: t("settings.hud.metric.cpu_temp_help"),
            metric_cpu_mhz: t("settings.hud.metric.cpu_mhz"),
            metric_cpu_mhz_help: t("settings.hud.metric.cpu_mhz_help"),
            metric_cpu_power: t("settings.hud.metric.cpu_power"),
            metric_cpu_power_help: t("settings.hud.metric.cpu_power_help"),
            metric_ram: t("settings.hud.metric.ram"),
            metric_ram_help: t("settings.hud.metric.ram_help"),

            group_metrics_fps: t("settings.hud.group.metrics_fps"),
            metric_fps: t("settings.hud.metric.fps"),
            metric_fps_help: t("settings.hud.metric.fps_help"),
            metric_fps_use_label: t("settings.hud.metric.fps_use_label"),
            metric_fps_use_label_help: t("settings.hud.metric.fps_use_label_help"),
            metric_fps_avg: t("settings.hud.metric.fps_avg"),
            metric_fps_avg_help: t("settings.hud.metric.fps_avg_help"),
            metric_fps_1_low: t("settings.hud.metric.fps_1_low"),
            metric_fps_1_low_help: t("settings.hud.metric.fps_1_low_help"),
            metric_frametime: t("settings.hud.metric.frametime"),
            metric_frametime_help: t("settings.hud.metric.frametime_help"),
            metric_frame_timing: t("settings.hud.metric.frame_timing"),
            metric_frame_timing_help: t("settings.hud.metric.frame_timing_help"),
            metric_present_mode: t("settings.hud.metric.present_mode"),
            metric_present_mode_help: t("settings.hud.metric.present_mode_help"),

            group_metrics_runtime: t("settings.hud.group.metrics_runtime"),
            metric_wine: t("settings.hud.metric.wine"),
            metric_wine_help: t("settings.hud.metric.wine_help"),
            metric_winesync: t("settings.hud.metric.winesync"),
            metric_winesync_help: t("settings.hud.metric.winesync_help"),
            metric_fsr: t("settings.hud.metric.fsr"),
            metric_fsr_help: t("settings.hud.metric.fsr_help"),
            metric_hdr: t("settings.hud.metric.hdr"),
            metric_hdr_help: t("settings.hud.metric.hdr_help"),
            metric_gamemode: t("settings.hud.metric.gamemode"),
            metric_gamemode_help: t("settings.hud.metric.gamemode_help"),
            metric_throttling_status: t("settings.hud.metric.throttling_status"),
            metric_throttling_status_help: t("settings.hud.metric.throttling_status_help"),
            metric_gamepad_battery: t("settings.hud.metric.gamepad_battery"),
            metric_gamepad_battery_help: t("settings.hud.metric.gamepad_battery_help"),

            fps_limit_label: t("settings.hud.fps_limit.label"),
            fps_limit_help: t("settings.hud.fps_limit.help"),
            fps_limit_off: t("settings.hud.fps_limit.off"),
            fps_limit_placeholder: t("settings.hud.fps_limit.placeholder"),
            fps_limit_method_help: t("settings.hud.fps_limit_method.help"),
            fps_limit_method_late: t("settings.hud.fps_limit_method.late"),
            fps_limit_method_early: t("settings.hud.fps_limit_method.early"),

            position_label: t("settings.hud.position.label"),
            position_labels: [
                t("settings.hud.position.top_left"),
                t("settings.hud.position.top_right"),
                t("settings.hud.position.bottom_left"),
                t("settings.hud.position.bottom_right"),
                t("settings.hud.position.top_center"),
                t("settings.hud.position.bottom_center"),
                t("settings.hud.position.middle_left"),
                t("settings.hud.position.middle_right"),
            ],
            orientation_label: t("settings.hud.orientation.label"),
            orientation_vertical: t("settings.hud.orientation.vertical"),
            orientation_horizontal: t("settings.hud.orientation.horizontal"),
            round_corners_label: t("settings.hud.round_corners.label"),
            hud_compact_label: t("settings.hud.hud_compact.label"),
            hud_compact_help: t("settings.hud.hud_compact.help"),
            hud_no_margin_label: t("settings.hud.hud_no_margin.label"),
            hud_no_margin_help: t("settings.hud.hud_no_margin.help"),
            font_scale_label: t("settings.hud.font_scale.label"),
            background_alpha_label: t("settings.hud.background_alpha.label"),
            alpha_label: t("settings.hud.alpha.label"),
            toggle_hud_label: t("settings.hud.toggle_hud.label"),
            toggle_hud_help: t("settings.hud.toggle_hud.help"),
            no_display_label: t("settings.hud.no_display.label"),
            no_display_help: t("settings.hud.no_display.help"),

            group_colors: t("settings.hud.group.colors"),
            palette_label: t("settings.hud.palette.label"),
            palette_help: t("settings.hud.palette.help"),
            palette_mangohud_default: t("settings.hud.palette.mangohud_default"),
            palette_sync_irori_theme: t("settings.hud.palette.sync_irori_theme"),
            palette_monochrome: t("settings.hud.palette.monochrome"),
            palette_custom: t("settings.hud.palette.custom"),
            fps_color_change_label: t("settings.hud.fps_color_change.label"),
            fps_color_change_help: t("settings.hud.fps_color_change.help"),
            color_slot_labels: [
                t("settings.hud.color.background"),
                t("settings.hud.color.text"),
                t("settings.hud.color.gpu"),
                t("settings.hud.color.cpu"),
                t("settings.hud.color.vram"),
                t("settings.hud.color.ram"),
                t("settings.hud.color.engine"),
                t("settings.hud.color.frametime"),
                t("settings.hud.color.io"),
                t("settings.hud.color.battery"),
                t("settings.hud.color.accent"),
            ],

            group_opengl: t("settings.hud.group.opengl"),
            opengl_wrapper_label: t("settings.hud.opengl_wrapper.label"),
            opengl_wrapper_help: t("settings.hud.opengl_wrapper.help"),
            mangohud_dlsym_label: t("settings.hud.mangohud_dlsym.label"),
            mangohud_dlsym_help: t("settings.hud.mangohud_dlsym.help"),

            group_metal: t("settings.hud.group.metal"),
            metal_alignment_label: t("settings.hud.metal.alignment.label"),
            metal_alignment_help: t("settings.hud.metal.alignment.help"),
            metal_note: t("settings.hud.metal.note"),
        }
    }
}

struct CachedDisplay {
    round_corners: SharedString,
    font_scale: SharedString,
    background_alpha: SharedString,
    alpha: SharedString,
    inherit_summary: SharedString,
    steam_snippet: Option<SharedString>,
    custom_swatches: [Hsla; 11],
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
    active_tab: HudTab,
    global: GlobalHudSettings,
    game: GameHudSettings,
    env: LinuxHudEnv,
    env_loaded: bool,
    labels: HudLabels,
    display: CachedDisplay,
    language: LanguageStamp,

    preview: Entity<HudPreview>,
    shortcut: Entity<ShortcutRecorder>,
    gpu_select: Entity<SelectState<Vec<HudOption>>>,
    position_select: Entity<SelectState<Vec<HudOption>>>,
    gpu_label_input: Entity<InputState>,
    cpu_label_input: Entity<InputState>,
    fps_limit_input: Entity<InputState>,
    round_corners_slider: Entity<SliderState>,
    font_scale_slider: Entity<SliderState>,
    bg_alpha_slider: Entity<SliderState>,
    alpha_slider: Entity<SliderState>,
    color_inputs: [Entity<InputState>; 11],
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
            ShortcutRecorder::new(
                active_profile.toggle_hud.clone(),
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

        let gpu_items = build_gpu_options(&labels, &env, &active_profile.gpu_selection);
        let gpu_key = gpu_selection_key(&active_profile.gpu_selection, &env);
        let gpu_select = cx.new(|cx| {
            let mut s = SelectState::new(gpu_items, None, window, cx);
            s.set_selected_value(&gpu_key, window, cx);
            s
        });

        let pos_items = build_position_options(&labels);
        let pos_key = SharedString::new_static(active_profile.position.as_mangohud_value());
        let position_select = cx.new(|cx| {
            let mut s = SelectState::new(pos_items, None, window, cx);
            s.set_selected_value(&pos_key, window, cx);
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

        let fps_limit_text = active_profile
            .fps_limit
            .map(|v| v.to_string())
            .unwrap_or_default();
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
            &position_select,
            window,
            |this, _, event: &SelectEvent<Vec<HudOption>>, window, cx| {
                if let SelectEvent::Confirm(Some(key)) = event
                    && let Some(pos) = parse_position_key(key)
                {
                    this.edit_mangohud(window, cx, |p| p.position = pos);
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
                    let parsed = raw
                        .parse::<u32>()
                        .ok()
                        .filter(|&v| v > 0)
                        .map(|v| v.min(1000));
                    let normalized = parsed.map(|v| v.to_string()).unwrap_or_default();
                    if state.read(cx).value().as_ref() != normalized {
                        state.update(cx, |s, cx| s.set_value(normalized, window, cx));
                    }
                    this.edit_mangohud(window, cx, |p| p.fps_limit = parsed);
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
                            slot.set(&mut p.custom_colors, hex);
                        });
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

        let display = build_cached_display(&scope, hud_backend, &global, &game, &labels, cx);

        let panel = Self {
            scope,
            backend: hud_backend,
            active_tab: HudTab::Metrics,
            global,
            game,
            env,
            env_loaded,
            labels,
            display,
            language: LanguageStamp::new(),
            preview,
            shortcut,
            gpu_select,
            position_select,
            gpu_label_input,
            cpu_label_input,
            fps_limit_input,
            round_corners_slider,
            font_scale_slider,
            bg_alpha_slider,
            alpha_slider,
            color_inputs,
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

    fn is_steam(&self) -> bool {
        matches!(self.scope, HudPanelScope::PerGame { is_steam: true, .. })
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
            HudPanelScope::Global => self.global.mode == GlobalHudMode::IroriProfile,
            HudPanelScope::PerGame { .. } => self.game.mode == GameHudMode::Custom,
        }
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
        match &self.scope {
            HudPanelScope::Global => {
                let mut next = self.global.clone();
                if next.mangohud.palette == HudColorPalette::SyncIroriTheme {
                    next.cached_theme_colors = Some(theme_hud_colors(cx.theme()));
                }
                self.global = next.clone();
                backend(cx).settings.update_app(|a| {
                    a.hud = next;
                });
            }
            HudPanelScope::PerGame { id, .. } => {
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
        self.sync_live_mangohud_config(cx);
    }

    /// Rewrite `<data>/mangohud/global.conf` or `<runner>_<app>.conf` in-place
    /// on a background task so running games and Steam launches pick up edits
    /// (such as FPS limit or metric changes) live via MangoHud's `inotify`.
    fn sync_live_mangohud_config(&self, cx: &App) {
        if self.backend != HudBackend::MangoHud {
            return;
        }
        let resolved =
            resolve_hud_for_scope(&self.scope, self.backend, &self.global, &self.game, cx);
        let ResolvedHud::MangoHudProfile(profile) = resolved else {
            return;
        };
        let conf_path = steam_conf_path(&self.scope, &self.game);
        let ctx = ConfRenderContext {
            detected_gpu_label: default_gpu_short_label(&profile.gpu_selection, &self.env.gpus),
            detected_cpu_label: self.env.cpu_label.clone(),
            gpu_vendor: default_gpu_vendor(&profile.gpu_selection, &self.env.gpus),
            cpu_vendor: self.env.cpu_vendor,
            pci_dev: resolve_pci_dev(&profile.gpu_selection, &self.env.gpus),
            theme_colors: Some(theme_hud_colors(cx.theme())),
        };
        let body = render_mangohud_conf(&profile, &ctx);
        irori_ui::run(cx, async move {
            if let Err(err) = irori_runtime::hud::write_config_live(&conf_path, &body).await {
                tracing::warn!(
                    "writing live MangoHud config {}: {err:#}",
                    conf_path.display()
                );
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
        self.persist_current(cx);
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
        if self.global.mode == mode {
            return;
        }
        self.global.mode = mode;
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
        self.game.restore_from_global(&self.global);
        self.persist_current(cx);
        self.sync_controls(window, cx);
        self.sync_preview_and_display(cx);
        cx.notify();
    }

    fn reload_from_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (global, game) = read_hud_settings(&self.scope, cx);
        if self.global == global && self.game == game {
            return;
        }
        self.global = global;
        self.game = game;
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

        let gpu_key = gpu_selection_key(&profile.gpu_selection, &self.env);
        self.gpu_select
            .update(cx, |s, cx| s.set_selected_value(&gpu_key, window, cx));

        let pos_key = SharedString::new_static(profile.position.as_mangohud_value());
        self.position_select
            .update(cx, |s, cx| s.set_selected_value(&pos_key, window, cx));

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

        let fps_text = profile.fps_limit.map(|v| v.to_string()).unwrap_or_default();
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
    }

    fn sync_preview_and_display(&mut self, cx: &mut Context<Self>) {
        let profile = self.active_mangohud().clone();
        let metal = self.active_metal().clone();
        let env = self.env.clone();
        let backend = self.backend;
        self.preview
            .update(cx, |p, cx| p.set_state(backend, profile, metal, env, cx));
        self.display = build_cached_display(
            &self.scope,
            self.backend,
            &self.global,
            &self.game,
            &self.labels,
            cx,
        );
    }

    fn retranslate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.labels = HudLabels::load();
        self.rebuild_gpu_options(window, cx);
        let pos_items = build_position_options(&self.labels);
        let pos_key = SharedString::new_static(self.active_mangohud().position.as_mangohud_value());
        self.position_select.update(cx, |s, cx| {
            s.set_items(pos_items, window, cx);
            s.set_selected_value(&pos_key, window, cx);
        });
        self.sync_placeholders(window, cx);
        self.sync_preview_and_display(cx);
        cx.notify();
    }

    fn render_missing_mangohud(&self) -> Option<AnyElement> {
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

    fn render_steam_section(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.is_steam() {
            return None;
        }
        let theme = cx.theme();
        let snippet_card = match &self.display.steam_snippet {
            Some(snippet) => {
                let text = snippet.clone();
                let copied_msg = self.labels.steam_copied.clone();
                let code_box = div()
                    .w_full()
                    .min_w_0()
                    .p_3()
                    .rounded(theme.radius)
                    .bg(theme.muted)
                    .border_1()
                    .border_color(theme.border)
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .child(snippet.clone());
                let copy_btn = Button::new("hud-steam-copy")
                    .outline()
                    .small()
                    .icon(IconName::Copy)
                    .label(self.labels.steam_copy.clone())
                    .on_click(move |_, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                        window.push_notification(Notification::success(copied_msg.clone()), cx);
                    });
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

    fn render_mode_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let mode_control: AnyElement = match &self.scope {
            HudPanelScope::Global => {
                let current = self.global.mode;
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .child(choice_button(
                        "hud-mode-off",
                        self.labels.mode_off.clone(),
                        current == GlobalHudMode::Off,
                        cx.listener(|this, _, window, cx| {
                            this.set_global_mode(GlobalHudMode::Off, window, cx);
                        }),
                    ))
                    .child(choice_button(
                        "hud-mode-irori",
                        self.labels.mode_irori_profile.clone(),
                        current == GlobalHudMode::IroriProfile,
                        cx.listener(|this, _, window, cx| {
                            this.set_global_mode(GlobalHudMode::IroriProfile, window, cx);
                        }),
                    ))
                    .child(choice_button(
                        "hud-mode-system",
                        self.labels.mode_system_default.clone(),
                        current == GlobalHudMode::SystemDefault,
                        cx.listener(|this, _, window, cx| {
                            this.set_global_mode(GlobalHudMode::SystemDefault, window, cx);
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
                        "hud-game-mode-system",
                        self.labels.mode_system_default.clone(),
                        current == GameHudMode::SystemDefault,
                        cx.listener(|this, _, window, cx| {
                            this.set_game_mode(GameHudMode::SystemDefault, window, cx);
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
            HudPanelScope::Global => match self.global.mode {
                GlobalHudMode::Off => Some(self.labels.mode_off_note.clone()),
                GlobalHudMode::SystemDefault => Some(match self.backend {
                    HudBackend::MetalHud => self.labels.mode_system_default_metal_note.clone(),
                    _ => self.labels.mode_system_default_mangohud_note.clone(),
                }),
                GlobalHudMode::IroriProfile => None,
            },
            HudPanelScope::PerGame { .. } => match self.game.mode {
                GameHudMode::InheritGlobal => Some(self.display.inherit_summary.clone()),
                GameHudMode::SystemDefault => Some(match self.backend {
                    HudBackend::MetalHud => self.labels.mode_system_default_metal_note.clone(),
                    _ => self.labels.mode_system_default_mangohud_note.clone(),
                }),
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

    fn render_top_quick_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let muted_fg = cx.theme().muted_foreground;
        let p = self.active_mangohud();

        // Row 1: Mode buttons (+ inline Restore button when PerGame::Custom)
        let mode_buttons: AnyElement = match &self.scope {
            HudPanelScope::Global => {
                let current = self.global.mode;
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .child(choice_button(
                        "hud-mode-off",
                        self.labels.mode_off.clone(),
                        current == GlobalHudMode::Off,
                        cx.listener(|this, _, window, cx| {
                            this.set_global_mode(GlobalHudMode::Off, window, cx);
                        }),
                    ))
                    .child(choice_button(
                        "hud-mode-irori",
                        self.labels.mode_irori_profile.clone(),
                        current == GlobalHudMode::IroriProfile,
                        cx.listener(|this, _, window, cx| {
                            this.set_global_mode(GlobalHudMode::IroriProfile, window, cx);
                        }),
                    ))
                    .child(choice_button(
                        "hud-mode-system",
                        self.labels.mode_system_default.clone(),
                        current == GlobalHudMode::SystemDefault,
                        cx.listener(|this, _, window, cx| {
                            this.set_global_mode(GlobalHudMode::SystemDefault, window, cx);
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
                        "hud-game-mode-system",
                        self.labels.mode_system_default.clone(),
                        current == GameHudMode::SystemDefault,
                        cx.listener(|this, _, window, cx| {
                            this.set_game_mode(GameHudMode::SystemDefault, window, cx);
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
                    this.edit_mangohud(window, cx, |p| p.mark_custom());
                }),
            ));

        // Row 3: FPS Limit + Method inline right at the top
        let mut fps_row = h_flex()
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
            fps_row = fps_row.child(choice_button(
                id,
                SharedString::new_static(text),
                p.fps_limit == Some(cap),
                cx.listener(move |this, _, window, cx| {
                    this.fps_limit_input
                        .update(cx, |s, cx| s.set_value(text, window, cx));
                    this.edit_mangohud(window, cx, |p| p.fps_limit = Some(cap));
                }),
            ));
        }
        fps_row = fps_row
            .child(Input::new(&self.fps_limit_input).small().w_20())
            .child(div().w(px(1.)).h_4().mx_1().bg(border))
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

        let compact_row = |label: SharedString, hint: SharedString, body: AnyElement| {
            v_flex()
                .w_full()
                .px_4()
                .py_2p5()
                .gap_1p5()
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(div().text_xs().font_semibold().child(label))
                        .child(div().text_xs().text_color(muted_fg).truncate().child(hint)),
                )
                .child(body)
        };

        let mode_hint = match &self.scope {
            HudPanelScope::Global => self.labels.mode_global_help.clone(),
            HudPanelScope::PerGame { .. } => self.labels.mode_game_help.clone(),
        };

        group_panel(cx)
            .child(compact_row(
                self.labels.mode_label.clone(),
                mode_hint,
                mode_buttons,
            ))
            .child(div().border_t_1().border_color(border).child(compact_row(
                self.labels.presets_label.clone(),
                self.labels.presets_help.clone(),
                preset_buttons.into_any_element(),
            )))
            .child(div().border_t_1().border_color(border).child(compact_row(
                self.labels.fps_limit_label.clone(),
                self.labels.fps_limit_help.clone(),
                fps_row.into_any_element(),
            )))
            .into_any_element()
    }

    fn render_tab_bar(&self, cx: &mut Context<Self>) -> AnyElement {
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
                    cx.notify();
                }),
            ))
            .child(choice_button(
                "hud-tab-layout",
                self.labels.tab_layout.clone(),
                active == HudTab::Layout,
                cx.listener(|this, _, _, cx| {
                    this.active_tab = HudTab::Layout;
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

    fn render_metrics_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let muted_fg = cx.theme().muted_foreground;
        let p = self.active_mangohud();
        let gpu_locked = p.gpu_stats_locked();
        let cpu_locked = p.cpu_stats_locked();

        let gpu_chips = h_flex()
            .flex_wrap()
            .gap_1p5()
            .child(if gpu_locked {
                locked_metric_chip(
                    "hud-metric-gpu-stats",
                    self.labels.metric_gpu_stats.clone(),
                    self.labels.metric_gpu_stats_locked.clone(),
                )
            } else {
                metric_chip(
                    "hud-metric-gpu-stats",
                    self.labels.metric_gpu_stats.clone(),
                    self.labels.metric_gpu_stats_help.clone(),
                    p.gpu_stats,
                    cx.listener(|this, _, window, cx| {
                        this.edit_mangohud(window, cx, |p| {
                            p.gpu_stats = !p.gpu_stats;
                            p.mark_custom();
                        });
                    }),
                )
            })
            .child(metric_chip(
                "hud-metric-gpu-temp",
                self.labels.metric_gpu_temp.clone(),
                self.labels.metric_gpu_temp_help.clone(),
                p.gpu_temp,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gpu_temp = !p.gpu_temp;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-gpu-junction",
                self.labels.metric_gpu_junction_temp.clone(),
                self.labels.metric_gpu_junction_temp_help.clone(),
                p.gpu_junction_temp,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gpu_junction_temp = !p.gpu_junction_temp;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-gpu-mem-temp",
                self.labels.metric_gpu_mem_temp.clone(),
                self.labels.metric_gpu_mem_temp_help.clone(),
                p.gpu_mem_temp,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gpu_mem_temp = !p.gpu_mem_temp;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-gpu-clock",
                self.labels.metric_gpu_core_clock.clone(),
                self.labels.metric_gpu_core_clock_help.clone(),
                p.gpu_core_clock,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gpu_core_clock = !p.gpu_core_clock;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-gpu-mem-clock",
                self.labels.metric_gpu_mem_clock.clone(),
                self.labels.metric_gpu_mem_clock_help.clone(),
                p.gpu_mem_clock,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gpu_mem_clock = !p.gpu_mem_clock;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-gpu-power",
                self.labels.metric_gpu_power.clone(),
                self.labels.metric_gpu_power_help.clone(),
                p.gpu_power,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gpu_power = !p.gpu_power;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-gpu-fan",
                self.labels.metric_gpu_fan.clone(),
                self.labels.metric_gpu_fan_help.clone(),
                p.gpu_fan,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gpu_fan = !p.gpu_fan;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-vram",
                self.labels.metric_vram.clone(),
                self.labels.metric_vram_help.clone(),
                p.vram,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.vram = !p.vram;
                        p.mark_custom();
                    });
                }),
            ));

        let cpu_chips = h_flex()
            .flex_wrap()
            .gap_1p5()
            .child(if cpu_locked {
                locked_metric_chip(
                    "hud-metric-cpu-stats",
                    self.labels.metric_cpu_stats.clone(),
                    self.labels.metric_cpu_stats_locked.clone(),
                )
            } else {
                metric_chip(
                    "hud-metric-cpu-stats",
                    self.labels.metric_cpu_stats.clone(),
                    self.labels.metric_cpu_stats_help.clone(),
                    p.cpu_stats,
                    cx.listener(|this, _, window, cx| {
                        this.edit_mangohud(window, cx, |p| {
                            p.cpu_stats = !p.cpu_stats;
                            p.mark_custom();
                        });
                    }),
                )
            })
            .child(metric_chip(
                "hud-metric-cpu-temp",
                self.labels.metric_cpu_temp.clone(),
                self.labels.metric_cpu_temp_help.clone(),
                p.cpu_temp,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.cpu_temp = !p.cpu_temp;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-cpu-mhz",
                self.labels.metric_cpu_mhz.clone(),
                self.labels.metric_cpu_mhz_help.clone(),
                p.cpu_mhz,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.cpu_mhz = !p.cpu_mhz;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-cpu-power",
                self.labels.metric_cpu_power.clone(),
                self.labels.metric_cpu_power_help.clone(),
                p.cpu_power,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.cpu_power = !p.cpu_power;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-ram",
                self.labels.metric_ram.clone(),
                self.labels.metric_ram_help.clone(),
                p.ram,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.ram = !p.ram;
                        p.mark_custom();
                    });
                }),
            ));

        let fps_chips = h_flex()
            .flex_wrap()
            .gap_1p5()
            .child(metric_chip(
                "hud-metric-fps",
                self.labels.metric_fps.clone(),
                self.labels.metric_fps_help.clone(),
                p.fps,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.fps = !p.fps;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-fps-avg",
                self.labels.metric_fps_avg.clone(),
                self.labels.metric_fps_avg_help.clone(),
                p.fps_avg,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.fps_avg = !p.fps_avg;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-fps-1-low",
                self.labels.metric_fps_1_low.clone(),
                self.labels.metric_fps_1_low_help.clone(),
                p.fps_1_low,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.fps_1_low = !p.fps_1_low;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-frametime",
                self.labels.metric_frametime.clone(),
                self.labels.metric_frametime_help.clone(),
                p.frametime,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.frametime = !p.frametime;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-frame-timing",
                self.labels.metric_frame_timing.clone(),
                self.labels.metric_frame_timing_help.clone(),
                p.frame_timing,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.frame_timing = !p.frame_timing;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-present-mode",
                self.labels.metric_present_mode.clone(),
                self.labels.metric_present_mode_help.clone(),
                p.present_mode,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.present_mode = !p.present_mode;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-fps-use-label",
                self.labels.metric_fps_use_label.clone(),
                self.labels.metric_fps_use_label_help.clone(),
                p.fps_use_label,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.fps_use_label = !p.fps_use_label;
                        p.mark_custom();
                    });
                }),
            ));

        let runtime_chips = h_flex()
            .flex_wrap()
            .gap_1p5()
            .child(metric_chip(
                "hud-metric-wine",
                self.labels.metric_wine.clone(),
                self.labels.metric_wine_help.clone(),
                p.wine,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.wine = !p.wine;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-winesync",
                self.labels.metric_winesync.clone(),
                self.labels.metric_winesync_help.clone(),
                p.winesync,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.winesync = !p.winesync;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-fsr",
                self.labels.metric_fsr.clone(),
                self.labels.metric_fsr_help.clone(),
                p.fsr,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.fsr = !p.fsr;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-hdr",
                self.labels.metric_hdr.clone(),
                self.labels.metric_hdr_help.clone(),
                p.hdr,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.hdr = !p.hdr;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-gamemode",
                self.labels.metric_gamemode.clone(),
                self.labels.metric_gamemode_help.clone(),
                p.gamemode,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gamemode = !p.gamemode;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-throttling",
                self.labels.metric_throttling_status.clone(),
                self.labels.metric_throttling_status_help.clone(),
                p.throttling_status,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.throttling_status = !p.throttling_status;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-battery",
                self.labels.metric_gamepad_battery.clone(),
                self.labels.metric_gamepad_battery_help.clone(),
                p.gamepad_battery,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gamepad_battery = !p.gamepad_battery;
                        p.mark_custom();
                    });
                }),
            ));

        let band = |title: SharedString, chips: gpui_kit::Div| {
            v_flex()
                .w_full()
                .px_4()
                .py_3()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(muted_fg)
                        .child(title),
                )
                .child(chips)
        };

        group_panel(cx)
            .child(band(self.labels.group_metrics_gpu.clone(), gpu_chips))
            .child(
                div()
                    .border_t_1()
                    .border_color(border)
                    .child(band(self.labels.group_metrics_cpu.clone(), cpu_chips)),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(border)
                    .child(band(self.labels.group_metrics_fps.clone(), fps_chips)),
            )
            .child(div().border_t_1().border_color(border).child(band(
                self.labels.group_metrics_runtime.clone(),
                runtime_chips,
            )))
            .into_any_element()
    }

    fn render_layout_tab(&self, cx: &mut Context<Self>) -> AnyElement {
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
                                        p.orientation = HudOrientation::Vertical;
                                        p.mark_custom();
                                    });
                                }),
                            ))
                            .child(choice_button(
                                "hud-orientation-horizontal",
                                self.labels.orientation_horizontal.clone(),
                                p.orientation == HudOrientation::Horizontal,
                                cx.listener(|this, _, window, cx| {
                                    this.edit_mangohud(window, cx, |p| {
                                        p.orientation = HudOrientation::Horizontal;
                                        p.mark_custom();
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

        // Row 4: Shortcut recorder
        let shortcut_row = field_row(
            RowState {
                label: self.labels.toggle_hud_label.clone(),
                help: self.labels.toggle_hud_help.clone(),
                ..Default::default()
            },
            self.shortcut.clone().into_any_element(),
            cx,
        );

        group_panel(cx)
            .child(pos_orient_row)
            .child(div().border_t_1().border_color(border).child(sliders_grid))
            .child(div().border_t_1().border_color(border).child(quick_toggles))
            .child(div().border_t_1().border_color(border).child(shortcut_row))
            .into_any_element()
    }

    fn render_hardware_section(&self, cx: &mut Context<Self>) -> AnyElement {
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

    fn render_colors_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let p = self.active_mangohud();

        let palette_control = h_flex()
            .flex_wrap()
            .gap_1p5()
            .child(choice_button(
                "hud-palette-default",
                self.labels.palette_mangohud_default.clone(),
                p.palette == HudColorPalette::MangoHudDefault,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.palette = HudColorPalette::MangoHudDefault
                    });
                }),
            ))
            .child(choice_button(
                "hud-palette-sync",
                self.labels.palette_sync_irori_theme.clone(),
                p.palette == HudColorPalette::SyncIroriTheme,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| p.palette = HudColorPalette::SyncIroriTheme);
                }),
            ))
            .child(choice_button(
                "hud-palette-monochrome",
                self.labels.palette_monochrome.clone(),
                p.palette == HudColorPalette::Monochrome,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| p.palette = HudColorPalette::Monochrome);
                }),
            ))
            .child(choice_button(
                "hud-palette-custom",
                self.labels.palette_custom.clone(),
                p.palette == HudColorPalette::Custom,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| p.palette = HudColorPalette::Custom);
                }),
            ))
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
                let item = h_flex()
                    .id(slot.id())
                    .gap_2()
                    .items_center()
                    .w(rems(13.5))
                    .child(
                        div()
                            .w(px(16.))
                            .h(px(16.))
                            .flex_shrink_0()
                            .rounded(px(4.))
                            .bg(swatch)
                            .border_1()
                            .border_color(border),
                    )
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

    fn render_opengl_section(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.show_opengl_legacy() {
            return None;
        }
        let mode_supports_wrapper = matches!(
            self.game.mode,
            GameHudMode::Custom | GameHudMode::SystemDefault
        );
        if !mode_supports_wrapper {
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

    fn render_metal_section(&self, cx: &mut Context<Self>) -> AnyElement {
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

impl Render for HudPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.language.changed() {
            cx.defer_in(window, |this, window, cx| this.retranslate(window, cx));
        }
        if self.backend == HudBackend::Unsupported {
            return div()
                .py_3()
                .child(muted(self.labels.unsupported.clone(), cx))
                .into_any_element();
        }

        let editing = self.is_editing_profile();

        match self.backend {
            HudBackend::MangoHud => v_flex()
                .w_full()
                .gap_2()
                .children(self.render_missing_mangohud())
                .children(self.render_steam_section(cx))
                .when(!editing, |v| {
                    v.child(self.render_mode_section(cx))
                        .children(self.render_opengl_section(cx))
                })
                .when(editing, |v| {
                    let tab_body = match self.active_tab {
                        HudTab::Metrics => self.render_metrics_tab(cx),
                        HudTab::Layout => self.render_layout_tab(cx),
                        HudTab::Appearance => v_flex()
                            .w_full()
                            .gap_2()
                            .child(self.render_colors_section(cx))
                            .child(self.render_hardware_section(cx))
                            .children(self.render_opengl_section(cx))
                            .into_any_element(),
                    };
                    v.child(self.preview.clone())
                        .child(self.render_top_quick_card(cx))
                        .child(self.render_tab_bar(cx))
                        .child(tab_body)
                })
                .into_any_element(),
            HudBackend::MetalHud => v_flex()
                .w_full()
                .gap_2()
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

fn choice_button(
    id: &'static str,
    label: SharedString,
    active: bool,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    let btn = if active {
        Button::new(id).primary()
    } else {
        Button::new(id).outline()
    };
    btn.small().label(label).on_click(on_click)
}

fn metric_chip(
    id: &'static str,
    label: SharedString,
    help: SharedString,
    active: bool,
    on_click: impl Fn(&gpui_kit::ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    let btn = if active {
        Button::new(id).primary()
    } else {
        Button::new(id).outline()
    };
    btn.small().label(label).tooltip(help).on_click(on_click)
}

fn locked_metric_chip(id: &'static str, label: SharedString, reason: SharedString) -> Button {
    Button::new(id)
        .primary()
        .small()
        .disabled(true)
        .label(label)
        .tooltip(reason)
}

fn compact_slider_cell(
    label: SharedString,
    value_label: SharedString,
    state: &Entity<SliderState>,
    muted_fg: Hsla,
) -> gpui_kit::Div {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .truncate()
                        .child(label.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_fg)
                        .flex_shrink_0()
                        .child(value_label),
                ),
        )
        .child(Slider::new(state).accessibility_label(label))
}

fn metric_switch_row(
    id: &'static str,
    label: SharedString,
    help: SharedString,
    checked: bool,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
    cx: &App,
) -> gpui_kit::Div {
    field_row(
        RowState {
            label: label.clone(),
            help,
            ..Default::default()
        },
        Switch::new(id)
            .checked(checked)
            .accessibility_label(label)
            .on_change(on_change)
            .into_any_element(),
        cx,
    )
}

fn render_divided_group<const N: usize>(
    heading: SharedString,
    rows: [gpui_kit::Div; N],
    border: Hsla,
    cx: &App,
) -> AnyElement {
    let mut panel = group_panel(cx);
    for (ix, row) in rows.into_iter().enumerate() {
        panel = panel.child(
            div()
                .when(ix > 0, |d| d.border_t_1().border_color(border))
                .child(row),
        );
    }
    v_flex()
        .w_full()
        .child(group_heading(heading, cx))
        .child(panel)
        .into_any_element()
}

fn slider_event_value(event: &SliderEvent) -> (f32, bool) {
    match event {
        SliderEvent::Change(v) => (v.start(), false),
        SliderEvent::Release(v) => (v.start(), true),
    }
}

fn read_hud_settings(scope: &HudPanelScope, cx: &App) -> (GlobalHudSettings, GameHudSettings) {
    let settings = &backend(cx).settings;
    let global = settings.app().hud.clone();
    let game = match scope {
        HudPanelScope::Global => GameHudSettings::default(),
        HudPanelScope::PerGame { id, .. } => settings.game(id).game_hud.clone(),
    };
    (global, game)
}

fn effective_mangohud_profile(
    scope: &HudPanelScope,
    global: &GlobalHudSettings,
    game: &GameHudSettings,
) -> MangoHudProfile {
    match scope {
        HudPanelScope::Global => global.mangohud.clone(),
        HudPanelScope::PerGame { .. } => game
            .custom_mangohud
            .clone()
            .unwrap_or_else(|| global.mangohud.clone()),
    }
}

fn effective_metal_settings(
    scope: &HudPanelScope,
    global: &GlobalHudSettings,
    game: &GameHudSettings,
) -> MetalHudSettings {
    match scope {
        HudPanelScope::Global => global.metal.clone(),
        HudPanelScope::PerGame { .. } => game
            .custom_metal
            .clone()
            .unwrap_or_else(|| global.metal.clone()),
    }
}

fn resolve_hud_for_scope(
    scope: &HudPanelScope,
    hud_backend: HudBackend,
    global: &GlobalHudSettings,
    game: &GameHudSettings,
    cx: &App,
) -> ResolvedHud {
    let (legacy_mangohud, legacy_fps) = match scope {
        HudPanelScope::Global => {
            let app = backend(cx).settings.app();
            (app.game.show_mangohud, app.game.show_fps)
        }
        HudPanelScope::PerGame { id, .. } => {
            let g = backend(cx).settings.game(id);
            (g.show_mangohud, g.show_fps)
        }
    };
    resolve_effective_hud(global, game, legacy_mangohud, legacy_fps, hud_backend)
}

fn steam_conf_path(scope: &HudPanelScope, game: &GameHudSettings) -> std::path::PathBuf {
    let paths = Paths::get();
    match scope {
        HudPanelScope::PerGame { id, .. } if game.mode == GameHudMode::Custom => {
            paths.mangohud_config_file(&format!("{}_{}", id.runner.id(), id.app_name))
        }
        _ => paths.mangohud_config_file("global"),
    }
}

fn build_gpu_options(
    labels: &HudLabels,
    env: &LinuxHudEnv,
    selection: &GpuSelection,
) -> Vec<HudOption> {
    let mut items = vec![
        HudOption::new("auto", labels.gpu_selection_auto.clone()),
        HudOption::new("all", labels.gpu_selection_all.clone()),
    ];
    let mut matched = !matches!(selection, GpuSelection::ByHardwareId { .. });
    for (ix, gpu) in env.gpus.iter().enumerate() {
        let key = format!("gpu:{ix}");
        if let GpuSelection::ByHardwareId {
            vendor_id,
            device_id,
            subvendor_id,
            subdevice_id,
        } = selection
            && &gpu.vendor_id == vendor_id
            && &gpu.device_id == device_id
            && &gpu.subvendor_id == subvendor_id
            && &gpu.subdevice_id == subdevice_id
        {
            matched = true;
        }
        items.push(HudOption::new(key, gpu.display_name.clone()));
    }
    if !matched
        && let GpuSelection::ByHardwareId {
            vendor_id,
            device_id,
            ..
        } = selection
    {
        items.push(HudOption::new(
            format!("hw:{vendor_id}:{device_id}"),
            format!("GPU ({vendor_id}:{device_id})"),
        ));
    }
    items
}

fn gpu_selection_key(selection: &GpuSelection, env: &LinuxHudEnv) -> SharedString {
    match selection {
        GpuSelection::AutoDedicated => SharedString::new_static("auto"),
        GpuSelection::AllGpus => SharedString::new_static("all"),
        GpuSelection::ByHardwareId {
            vendor_id,
            device_id,
            subvendor_id,
            subdevice_id,
        } => {
            for (ix, gpu) in env.gpus.iter().enumerate() {
                if &gpu.vendor_id == vendor_id
                    && &gpu.device_id == device_id
                    && &gpu.subvendor_id == subvendor_id
                    && &gpu.subdevice_id == subdevice_id
                {
                    return SharedString::from(format!("gpu:{ix}"));
                }
            }
            SharedString::from(format!("hw:{vendor_id}:{device_id}"))
        }
    }
}

fn parse_gpu_selection_key(key: &str, env: &LinuxHudEnv) -> GpuSelection {
    if key == "all" {
        return GpuSelection::AllGpus;
    }
    if let Some(rest) = key.strip_prefix("gpu:")
        && let Ok(ix) = rest.parse::<usize>()
        && let Some(gpu) = env.gpus.get(ix)
    {
        return GpuSelection::ByHardwareId {
            vendor_id: gpu.vendor_id.clone(),
            device_id: gpu.device_id.clone(),
            subvendor_id: gpu.subvendor_id.clone(),
            subdevice_id: gpu.subdevice_id.clone(),
        };
    }
    GpuSelection::AutoDedicated
}

fn build_position_options(labels: &HudLabels) -> Vec<HudOption> {
    HudPosition::ALL
        .iter()
        .enumerate()
        .map(|(ix, pos)| {
            HudOption::new(
                SharedString::new_static(pos.as_mangohud_value()),
                labels.position_labels[ix].clone(),
            )
        })
        .collect()
}

fn parse_position_key(key: &str) -> Option<HudPosition> {
    HudPosition::ALL
        .into_iter()
        .find(|p| p.as_mangohud_value() == key)
}

fn build_cached_display(
    scope: &HudPanelScope,
    hud_backend: HudBackend,
    global: &GlobalHudSettings,
    game: &GameHudSettings,
    labels: &HudLabels,
    cx: &App,
) -> CachedDisplay {
    let profile = effective_mangohud_profile(scope, global, game);
    let global_mode_label = match global.mode {
        GlobalHudMode::Off => labels.mode_off.as_ref(),
        GlobalHudMode::IroriProfile => labels.mode_irori_profile.as_ref(),
        GlobalHudMode::SystemDefault => labels.mode_system_default.as_ref(),
    };
    let inherit_summary = t_args(
        "settings.hud.mode.inherit_summary",
        &[("mode", global_mode_label)],
    );

    let steam_snippet = if matches!(scope, HudPanelScope::PerGame { is_steam: true, .. }) {
        let resolved = resolve_hud_for_scope(scope, hud_backend, global, game, cx);
        let conf_path = steam_conf_path(scope, game);
        steam_launch_snippet(hud_backend, &resolved, &conf_path).map(SharedString::from)
    } else {
        None
    };

    let custom_swatches = std::array::from_fn(|ix| {
        let slot = COLOR_SLOTS[ix];
        let hex = sanitize_hex6(slot.get(&profile.custom_colors), slot.fallback());
        let val = u32::from_str_radix(&hex, 16).unwrap_or(0xFFFFFF);
        rgb(val).into()
    });

    CachedDisplay {
        round_corners: SharedString::from(format!("{} px", profile.round_corners)),
        font_scale: SharedString::from(format!("{}%", (profile.font_scale * 100.0).round() as u32)),
        background_alpha: SharedString::from(format!(
            "{}%",
            (profile.background_alpha * 100.0).round() as u32
        )),
        alpha: SharedString::from(format!("{}%", (profile.alpha * 100.0).round() as u32)),
        inherit_summary,
        steam_snippet,
        custom_swatches,
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{TestAppContext, WindowOptions};
    use irori_core::{GameId, HudBackend, Paths, Runner};
    use irori_services::Backend;

    use super::*;

    fn test_backend() -> Backend {
        static ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
        ROOT.get_or_init(|| {
            let root = tempfile::tempdir().expect("temp dir").keep();
            Paths::init(Paths::under(&root));
            root
        });
        Backend::start_for_tests(|_, _| Vec::new()).expect("backend")
    }

    fn open_hud(
        cx: &mut TestAppContext,
        scope: HudPanelScope,
        backend_override: Option<HudBackend>,
    ) -> (gpui_kit::AnyWindowHandle, Entity<HudPanel>) {
        cx.executor().allow_parking();
        let backend = test_backend();
        cx.update(|cx| {
            gpui_kit::init(cx);
            irori_ui::init(backend, cx);
            crate::init(cx);
        });
        let opened = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| HudPanel::new(scope, backend_override, window, cx))
            })
            .expect("open hud window")
        });
        cx.run_until_parked();
        opened
    }

    #[gpui_kit::test]
    fn mangohud_global_and_metal_panels_render_without_panicking(cx: &mut TestAppContext) {
        let (handle, panel) = open_hud(cx, HudPanelScope::Global, Some(HudBackend::MangoHud));
        cx.update_window(handle, |_, window, cx| {
            panel.update(cx, |p, cx| {
                p.set_global_mode(GlobalHudMode::IroriProfile, window, cx);
                p.edit_mangohud(window, cx, |m| {
                    m.palette = HudColorPalette::Custom;
                    m.apply_preset(HudPreset::FullDiagnostics);
                });
            });
            window.refresh();
            let _ = window.draw(cx);
        })
        .unwrap();

        let (m_handle, m_panel) = open_hud(cx, HudPanelScope::Global, Some(HudBackend::MetalHud));
        cx.update_window(m_handle, |_, window, cx| {
            m_panel.update(cx, |p, cx| {
                p.set_global_mode(GlobalHudMode::IroriProfile, window, cx);
                p.edit_metal(cx, |m| m.alignment = MetalHudAlignment::BottomLeft);
            });
            window.refresh();
            let _ = window.draw(cx);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn per_game_custom_clones_global_and_hides_opengl_wrapper_for_non_native_and_steam(
        cx: &mut TestAppContext,
    ) {
        let native_id = GameId::new(Runner::Sideload, "hud-native-test");
        let (handle, panel) = open_hud(
            cx,
            HudPanelScope::PerGame {
                id: native_id.clone(),
                is_native_linux: true,
                is_steam: false,
            },
            Some(HudBackend::MangoHud),
        );
        cx.update_window(handle, |_, window, cx| {
            irori_ui::backend(cx).settings.update_app(|a| {
                a.hud.mode = GlobalHudMode::IroriProfile;
                a.hud.mangohud.fps_limit = Some(90);
            });
            panel.update(cx, |p, cx| {
                p.reload_from_settings(window, cx);
                assert!(p.show_opengl_legacy());
                p.set_game_mode(GameHudMode::Custom, window, cx);
                assert_eq!(p.active_mangohud().fps_limit, Some(90));
            });
            let _ = window.draw(cx);
        })
        .unwrap();

        let steam_id = GameId::new(Runner::Steam, "620");
        let (s_handle, s_panel) = open_hud(
            cx,
            HudPanelScope::PerGame {
                id: steam_id,
                is_native_linux: true,
                is_steam: true,
            },
            Some(HudBackend::MangoHud),
        );
        cx.update_window(s_handle, |_, window, cx| {
            s_panel.update(cx, |p, _| {
                assert!(
                    !p.show_opengl_legacy(),
                    "Steam games must never show LD_PRELOAD wrapper"
                );
            });
            let _ = window.draw(cx);
        })
        .unwrap();

        let wine_id = GameId::new(Runner::Sideload, "hud-wine-test");
        let (w_handle, w_panel) = open_hud(
            cx,
            HudPanelScope::PerGame {
                id: wine_id,
                is_native_linux: false,
                is_steam: false,
            },
            Some(HudBackend::MangoHud),
        );
        cx.update_window(w_handle, |_, window, cx| {
            w_panel.update(cx, |p, _| {
                assert!(
                    !p.show_opengl_legacy(),
                    "Wine/Proton games must never show LD_PRELOAD wrapper"
                );
            });
            let _ = window.draw(cx);
        })
        .unwrap();
    }
}
