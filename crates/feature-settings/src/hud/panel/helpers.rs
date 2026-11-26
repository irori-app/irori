use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::select::SelectItem;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Entity, Hsla, IntoElement as _, ParentElement as _, SharedString, Styled as _,
    Window, div, px, rgb,
};
use irori_core::hud::{resolve_effective_hud, sanitize_hex6};
use irori_core::{
    GameHudMode, GameHudSettings, GlobalHudMode, GlobalHudSettings, GpuSelection, HudBackend,
    HudColorPalette, HudPosition, MangoHudProfile, MetalHudSettings, Paths, ResolvedHud,
    ThemeHudColors,
};
use irori_runtime::hud::hardware_linux::{LinuxHudEnv, default_gpu_vendor};
use irori_runtime::hud::launch_env::steam_launch_snippet;
use irori_runtime::hud::mangohud_conf::{ConfRenderContext, resolve_effective_colors};
use irori_ui::backend;
use irori_ui::i18n::t_args;
use irori_ui::theme::theme_hud_colors;

use crate::widgets::{RowState, field_row, group_heading, group_panel};

use super::HudPanelScope;
use super::labels::HudLabels;

#[derive(Debug, Clone)]
pub(super) struct HudOption {
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

pub(super) const FPS_PRESETS: [(u32, &str, &str); 7] = [
    (30, "hud-fps-30", "30"),
    (60, "hud-fps-60", "60"),
    (90, "hud-fps-90", "90"),
    (120, "hud-fps-120", "120"),
    (144, "hud-fps-144", "144"),
    (165, "hud-fps-165", "165"),
    (240, "hud-fps-240", "240"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorSlot {
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

pub(super) const COLOR_SLOTS: [ColorSlot; 11] = [
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
    pub(super) const fn id(self) -> &'static str {
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

    pub(super) const fn popover_id(self) -> &'static str {
        match self {
            Self::Background => "hud-color-popover-background",
            Self::Text => "hud-color-popover-text",
            Self::Gpu => "hud-color-popover-gpu",
            Self::Cpu => "hud-color-popover-cpu",
            Self::Vram => "hud-color-popover-vram",
            Self::Ram => "hud-color-popover-ram",
            Self::Engine => "hud-color-popover-engine",
            Self::Frametime => "hud-color-popover-frametime",
            Self::Io => "hud-color-popover-io",
            Self::Battery => "hud-color-popover-battery",
            Self::Accent => "hud-color-popover-accent",
        }
    }

    pub(super) const fn trigger_id(self) -> &'static str {
        match self {
            Self::Background => "hud-color-trigger-background",
            Self::Text => "hud-color-trigger-text",
            Self::Gpu => "hud-color-trigger-gpu",
            Self::Cpu => "hud-color-trigger-cpu",
            Self::Vram => "hud-color-trigger-vram",
            Self::Ram => "hud-color-trigger-ram",
            Self::Engine => "hud-color-trigger-engine",
            Self::Frametime => "hud-color-trigger-frametime",
            Self::Io => "hud-color-trigger-io",
            Self::Battery => "hud-color-trigger-battery",
            Self::Accent => "hud-color-trigger-accent",
        }
    }

    pub(super) fn get(self, c: &ThemeHudColors) -> &str {
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

    pub(super) fn set(self, c: &mut ThemeHudColors, value: String) {
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

    pub(crate) fn fallback(self) -> &'static str {
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

pub(super) struct CachedDisplay {
    pub(super) round_corners: SharedString,
    pub(super) font_scale: SharedString,
    pub(super) background_alpha: SharedString,
    pub(super) alpha: SharedString,
    pub(super) inherit_summary: SharedString,
    pub(super) steam_snippet: Option<SharedString>,
    pub(super) custom_swatches: [Hsla; 11],
    /// Metric band titles with the enabled count (GPU, CPU, FPS, runtime).
    pub(super) band_titles: [SharedString; 4],
    /// GPU, CPU and RAM sample colors for each palette button, in the order
    /// hardware, Irori theme, monochrome, custom.
    pub(super) palette_dots: [[Hsla; 3]; 4],
}

pub(super) fn choice_button(
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

pub(super) fn metric_chip(
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

pub(super) fn locked_metric_chip(
    id: &'static str,
    label: SharedString,
    reason: SharedString,
) -> Button {
    Button::new(id)
        .primary()
        .small()
        .disabled(true)
        .label(label)
        .tooltip(reason)
}

/// Focusable info icon that shows `help` in its tooltip.
pub(super) fn info_button(id: &'static str, help: SharedString) -> Button {
    Button::new(id)
        .ghost()
        .xsmall()
        .icon(IconName::Info)
        .accessibility_label(help.clone())
        .tooltip(help)
}

/// Three small color samples (GPU, CPU, RAM) shown inside a palette button.
pub(super) fn palette_dots(colors: [Hsla; 3], border: Hsla) -> gpui_kit::Div {
    h_flex()
        .gap(px(2.))
        .children(colors.into_iter().map(|color| {
            div()
                .size(px(8.))
                .rounded_full()
                .bg(color)
                .border_1()
                .border_color(border)
        }))
}

/// Text for the custom FPS field: empty when the limit matches a shortcut chip.
pub(super) fn custom_fps_text(limit: Option<u32>) -> String {
    match limit {
        Some(v) if !is_fps_preset(v) => v.to_string(),
        _ => String::new(),
    }
}

pub(super) fn is_fps_preset(value: u32) -> bool {
    FPS_PRESETS.iter().any(|&(cap, _, _)| cap == value)
}

/// Whether the global mode counts as "On" (the legacy toggle turns Off into On).
pub(super) fn global_hud_on(global: &GlobalHudSettings, legacy_on: bool) -> bool {
    match global.mode {
        GlobalHudMode::IroriProfile => true,
        GlobalHudMode::Off => legacy_on,
    }
}

/// Legacy HUD toggle for `scope`: `showMangohud` on Linux, `showFps` on macOS.
pub(super) fn legacy_hud_toggle(scope: &HudPanelScope, hud_backend: HudBackend, cx: &App) -> bool {
    let settings = &backend(cx).settings;
    let (show_mangohud, show_fps) = match scope {
        HudPanelScope::Global => {
            let app = settings.app();
            (app.game.show_mangohud, app.game.show_fps)
        }
        HudPanelScope::PerGame { id, .. } => {
            let g = settings.game(id);
            (g.show_mangohud, g.show_fps)
        }
    };
    match hud_backend {
        HudBackend::MangoHud => show_mangohud,
        HudBackend::MetalHud => show_fps,
        HudBackend::Unsupported => false,
    }
}

pub(super) fn compact_slider_cell(
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

pub(super) fn metric_switch_row(
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

pub(super) fn render_divided_group<const N: usize>(
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

pub(super) fn slider_event_value(event: &SliderEvent) -> (f32, bool) {
    match event {
        SliderEvent::Change(v) => (v.start(), false),
        SliderEvent::Release(v) => (v.start(), true),
    }
}

pub(super) fn read_hud_settings(
    scope: &HudPanelScope,
    cx: &App,
) -> (GlobalHudSettings, GameHudSettings) {
    let settings = &backend(cx).settings;
    let mut global = settings.app().hud.clone();
    let mut mangohud = global.mangohud.clone();
    global.hydrate_profile(&mut mangohud);
    global.mangohud = mangohud;
    let mut game = match scope {
        HudPanelScope::Global => GameHudSettings::default(),
        HudPanelScope::PerGame { id, .. } => settings.game(id).game_hud.clone(),
    };
    if let Some(profile) = game.custom_mangohud.as_mut() {
        global.hydrate_profile(profile);
    }
    (global, game)
}

pub(super) fn effective_mangohud_profile(
    scope: &HudPanelScope,
    global: &GlobalHudSettings,
    game: &GameHudSettings,
) -> MangoHudProfile {
    let mut profile = match scope {
        HudPanelScope::Global => global.mangohud.clone(),
        HudPanelScope::PerGame { .. } => game
            .custom_mangohud
            .clone()
            .unwrap_or_else(|| global.mangohud.clone()),
    };
    global.hydrate_profile(&mut profile);
    profile
}

pub(super) fn effective_metal_settings(
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

pub(super) fn resolve_hud_for_scope(
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

pub(super) fn hud_conf_name(scope: &HudPanelScope, game: &GameHudSettings) -> String {
    match scope {
        HudPanelScope::PerGame { id, .. } if game.mode == GameHudMode::Custom => {
            format!("{}_{}", id.runner.id(), id.app_name)
        }
        _ => "global".to_string(),
    }
}

pub(super) fn steam_conf_path(scope: &HudPanelScope, game: &GameHudSettings) -> std::path::PathBuf {
    Paths::get().mangohud_config_file(&hud_conf_name(scope, game))
}

pub(super) fn steam_presets_path(
    scope: &HudPanelScope,
    game: &GameHudSettings,
) -> std::path::PathBuf {
    Paths::get().mangohud_presets_file(&hud_conf_name(scope, game))
}

pub(super) fn build_gpu_options(
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

pub(super) fn gpu_selection_key(selection: &GpuSelection, env: &LinuxHudEnv) -> SharedString {
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

pub(super) fn parse_gpu_selection_key(key: &str, env: &LinuxHudEnv) -> GpuSelection {
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

pub(super) fn build_position_options(labels: &HudLabels) -> Vec<HudOption> {
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

pub(super) fn parse_position_key(key: &str) -> Option<HudPosition> {
    HudPosition::ALL
        .into_iter()
        .find(|p| p.as_mangohud_value() == key)
}

/// Panel state that [`build_cached_display`] reads.
pub(super) struct DisplayInputs<'a> {
    pub(super) scope: &'a HudPanelScope,
    pub(super) hud_backend: HudBackend,
    pub(super) global: &'a GlobalHudSettings,
    pub(super) game: &'a GameHudSettings,
    pub(super) legacy_on: bool,
    pub(super) env: &'a LinuxHudEnv,
    pub(super) labels: &'a HudLabels,
}

pub(super) fn build_cached_display(inputs: DisplayInputs<'_>, cx: &App) -> CachedDisplay {
    let DisplayInputs {
        scope,
        hud_backend,
        global,
        game,
        legacy_on,
        env,
        labels,
    } = inputs;
    let profile = effective_mangohud_profile(scope, global, game);
    let global_mode_label = if global_hud_on(global, legacy_on) {
        labels.mode_irori_profile.as_ref()
    } else {
        labels.mode_off.as_ref()
    };
    let inherit_summary = t_args(
        "settings.hud.mode.inherit_summary",
        &[("mode", global_mode_label)],
    );

    let steam_snippet = if matches!(
        scope,
        HudPanelScope::Global | HudPanelScope::PerGame { is_steam: true, .. }
    ) {
        let resolved = resolve_hud_for_scope(scope, hud_backend, global, game, cx);
        let conf_path = steam_conf_path(scope, game);
        let presets_path = steam_presets_path(scope, game);
        steam_launch_snippet(hud_backend, &resolved, &conf_path, &presets_path)
            .map(SharedString::from)
    } else {
        None
    };

    let custom_swatches = std::array::from_fn(|ix| {
        let slot = COLOR_SLOTS[ix];
        let hex = sanitize_hex6(slot.get(&profile.custom_colors), slot.fallback());
        let val = u32::from_str_radix(&hex, 16).unwrap_or(0xFFFFFF);
        rgb(val).into()
    });

    let p = &profile;
    let counts = [
        tally(&[
            p.effective_gpu_stats(),
            p.gpu_temp,
            p.gpu_junction_temp,
            p.gpu_mem_temp,
            p.gpu_core_clock,
            p.gpu_mem_clock,
            p.gpu_power,
            p.gpu_fan,
            p.vram,
        ]),
        tally(&[
            p.effective_cpu_stats(),
            p.cpu_temp,
            p.cpu_mhz,
            p.cpu_power,
            p.ram,
        ]),
        tally(&[
            p.fps,
            p.fps_avg,
            p.fps_1_low,
            p.frametime,
            p.frame_timing,
            p.present_mode,
            p.fps_use_label,
        ]),
        tally(&[
            p.wine,
            p.winesync,
            p.fsr,
            p.hdr,
            p.gamemode,
            p.throttling_status,
            p.gamepad_battery,
        ]),
    ];
    let groups = [
        &labels.group_metrics_gpu,
        &labels.group_metrics_cpu,
        &labels.group_metrics_fps,
        &labels.group_metrics_runtime,
    ];
    let band_titles = std::array::from_fn(|ix| {
        let (on, total) = counts[ix];
        let on = on.to_string();
        let total = total.to_string();
        t_args(
            "settings.hud.metrics.band_count",
            &[
                ("group", groups[ix].as_ref()),
                ("on", on.as_str()),
                ("total", total.as_str()),
            ],
        )
    });

    let ctx = ConfRenderContext {
        gpu_vendor: default_gpu_vendor(&profile.gpu_selection, &env.gpus),
        cpu_vendor: env.cpu_vendor,
        theme_colors: Some(theme_hud_colors(cx.theme())),
        ..Default::default()
    };
    let mut sample = profile.clone();
    let palette_dots = [
        HudColorPalette::MangoHudDefault,
        HudColorPalette::SyncIroriTheme,
        HudColorPalette::Monochrome,
        HudColorPalette::Custom,
    ]
    .map(|palette| {
        sample.palette = palette;
        let colors = resolve_effective_colors(&sample, &ctx).unwrap_or_default();
        let d = ThemeHudColors::default();
        [
            hex_hsla(&colors.gpu_color, &d.gpu_color),
            hex_hsla(&colors.cpu_color, &d.cpu_color),
            hex_hsla(&colors.ram_color, &d.ram_color),
        ]
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
        band_titles,
        palette_dots,
    }
}

/// Count of enabled flags and the total, for a metric band title.
fn tally(flags: &[bool]) -> (usize, usize) {
    (flags.iter().filter(|&&f| f).count(), flags.len())
}

fn hex_hsla(hex: &str, fallback: &str) -> Hsla {
    let clean = sanitize_hex6(hex, fallback);
    rgb(u32::from_str_radix(&clean, 16).unwrap_or(0xFFFFFF)).into()
}
