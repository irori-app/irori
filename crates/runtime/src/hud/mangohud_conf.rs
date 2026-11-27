//! Pure, cross-platform `MangoHud.conf` and `presets.conf` serializer.

use std::fmt::Write as _;

use irori_core::hud::{
    DEFAULT_TOGGLE_HUD, DEFAULT_TOGGLE_PRESET, sanitize_hex6, sanitize_hud_label,
    truncate_hud_label,
};
use irori_core::{
    HardwareVendor, HudColorPalette, HudOrientation, HudPreset, MangoHudProfile, ThemeHudColors,
};

/// Runtime context supplied when rendering a `MangoHud.conf` file.
#[derive(Debug, Clone, Default)]
pub struct ConfRenderContext {
    pub detected_gpu_label: String,
    pub detected_cpu_label: String,
    pub gpu_vendor: HardwareVendor,
    pub cpu_vendor: HardwareVendor,
    pub pci_dev: Option<String>,
    pub theme_colors: Option<ThemeHudColors>,
}

/// Resolve the active 6-digit hex color palette for `profile`, returning `None`
/// when `HudColorPalette::MangoHudDefault` is selected without hardware vendor
/// tinting.
pub fn resolve_palette_colors(
    profile: &MangoHudProfile,
    theme_colors: Option<&ThemeHudColors>,
) -> Option<ThemeHudColors> {
    match profile.palette {
        HudColorPalette::MangoHudDefault => None,
        HudColorPalette::Monochrome => Some(ThemeHudColors::monochrome()),
        HudColorPalette::SyncIroriTheme => Some(theme_colors.cloned().unwrap_or_default()),
        HudColorPalette::Custom => Some(profile.custom_colors.clone()),
    }
}

/// Resolve effective colors for `profile` and `ctx`, automatically applying
/// hardware-vendor colors (AMD red, NVIDIA green, Intel blue) when
/// [`HudColorPalette::MangoHudDefault`] is active and a silicon vendor was
/// identified.
pub fn resolve_effective_colors(
    profile: &MangoHudProfile,
    ctx: &ConfRenderContext,
) -> Option<ThemeHudColors> {
    match profile.palette {
        HudColorPalette::MangoHudDefault => {
            if ctx.gpu_vendor.is_known() || ctx.cpu_vendor.is_known() {
                Some(ThemeHudColors::for_hardware(ctx.gpu_vendor, ctx.cpu_vendor))
            } else {
                None
            }
        }
        _ => resolve_palette_colors(profile, ctx.theme_colors.as_ref()),
    }
}

fn sanitize_shortcut(raw: &str, fallback: &'static str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|&c| !matches!(c, '\r' | '\n' | '#' | '=' | ',' | ';'))
        .collect::<String>()
        .trim()
        .to_string();
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    }
}

/// Serialize shared `MangoHud.conf` settings for `MANGOHUD_CONFIGFILE`.
///
/// Metric toggles and per-preset layout flags (`fps_only`, `horizontal`,
/// `hud_compact`, `hud_no_margin`) live exclusively in
/// [`render_mangohud_presets`] (`MANGOHUD_PRESETSFILE`). MangoHud parses
/// `MANGOHUD_CONFIGFILE` after `[preset N]`, so keeping display elements out of
/// this file prevents duplicated metrics and clipped layouts when cycling
/// presets in-game.
pub fn render_mangohud_conf(profile: &MangoHudProfile, ctx: &ConfRenderContext) -> String {
    let mut out = String::with_capacity(512);

    // (a) Shared layout, preset cycle order, scaling, shortcuts, GPU selector and frame limiter
    out.push_str("legacy_layout=false\n");
    out.push_str("preset=");
    for (ix, preset) in profile.preset_cycle().into_iter().enumerate() {
        if ix > 0 {
            out.push(',');
        }
        let _ = write!(out, "{}", preset.mangohud_preset_id());
    }
    out.push('\n');

    let _ = writeln!(out, "position={}", profile.position.as_mangohud_value());
    let _ = writeln!(out, "round_corners={}", profile.round_corners);
    let font_scale = profile.font_scale.clamp(0.5, 3.0);
    let bg_alpha = profile.background_alpha.clamp(0.0, 1.0);
    let alpha = profile.alpha.clamp(0.0, 1.0);
    let _ = writeln!(out, "font_scale={font_scale:.2}");
    let _ = writeln!(out, "background_alpha={bg_alpha:.2}");
    let _ = writeln!(out, "alpha={alpha:.2}");

    let toggle_hud = sanitize_shortcut(&profile.toggle_hud, DEFAULT_TOGGLE_HUD);
    let toggle_preset = sanitize_shortcut(&profile.toggle_preset, DEFAULT_TOGGLE_PRESET);
    let _ = writeln!(out, "toggle_hud={toggle_hud}");
    let _ = writeln!(out, "toggle_preset={toggle_preset}");

    if profile.no_display {
        out.push_str("no_display\n");
    }
    if let Some(pci) = ctx
        .pci_dev
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let _ = writeln!(out, "pci_dev={pci}");
    }
    if let Some(limit) = profile.fps_limit.filter(|&v| v > 0) {
        let _ = writeln!(out, "fps_limit={limit}");
    }
    let _ = writeln!(
        out,
        "fps_limit_method={}",
        profile.fps_limit_method.as_mangohud_value()
    );

    // (b) Palette colors + optional FPS color change
    if let Some(colors) = resolve_effective_colors(profile, ctx) {
        let defaults = ThemeHudColors::default();
        let _ = writeln!(
            out,
            "background_color={}",
            sanitize_hex6(&colors.background_color, &defaults.background_color)
        );
        let _ = writeln!(
            out,
            "text_color={}",
            sanitize_hex6(&colors.text_color, &defaults.text_color)
        );
        let _ = writeln!(
            out,
            "gpu_color={}",
            sanitize_hex6(&colors.gpu_color, &defaults.gpu_color)
        );
        let _ = writeln!(
            out,
            "cpu_color={}",
            sanitize_hex6(&colors.cpu_color, &defaults.cpu_color)
        );
        let _ = writeln!(
            out,
            "vram_color={}",
            sanitize_hex6(&colors.vram_color, &defaults.vram_color)
        );
        let _ = writeln!(
            out,
            "ram_color={}",
            sanitize_hex6(&colors.ram_color, &defaults.ram_color)
        );
        let _ = writeln!(
            out,
            "engine_color={}",
            sanitize_hex6(&colors.engine_color, &defaults.engine_color)
        );
        let _ = writeln!(
            out,
            "frametime_color={}",
            sanitize_hex6(&colors.frametime_color, &defaults.frametime_color)
        );
        let _ = writeln!(
            out,
            "io_color={}",
            sanitize_hex6(&colors.io_color, &defaults.io_color)
        );
        let _ = writeln!(
            out,
            "battery_color={}",
            sanitize_hex6(&colors.battery_color, &defaults.battery_color)
        );
    }
    if profile.fps_color_change {
        out.push_str("fps_color_change\n");
        out.push_str("fps_value=30,60\n");
        out.push_str("fps_color=B22222,FDFD09,39F900\n");
    }

    // (c) Labels (parameter values only; never add display elements on their own)
    let raw_gpu = if profile.gpu_label_override.trim().is_empty() {
        &ctx.detected_gpu_label
    } else {
        &profile.gpu_label_override
    };
    let gpu_label = truncate_hud_label(&sanitize_hud_label(raw_gpu));
    if !gpu_label.is_empty() {
        let _ = writeln!(out, "gpu_text={gpu_label}");
    }
    let raw_cpu = if profile.cpu_label_override.trim().is_empty() {
        &ctx.detected_cpu_label
    } else {
        &profile.cpu_label_override
    };
    let cpu_label = truncate_hud_label(&sanitize_hud_label(raw_cpu));
    if !cpu_label.is_empty() {
        let _ = writeln!(out, "cpu_text={cpu_label}");
    }
    if profile.fps_use_label {
        out.push_str("fps_text=FPS\n");
    }

    out
}

/// Serialize Irori's MangoHud preset definitions for `MANGOHUD_PRESETSFILE`.
///
/// Always emits `[preset 1]` through `[preset 4]` (Minimal, Horizontal Compact,
/// Balanced Gaming, Full Diagnostics), appends `[preset 5]` whenever the
/// profile has a vertical custom selection (`Custom` active or `saved_custom`
/// present), and appends `[preset 6]` whenever it has a horizontal custom
/// selection (`CustomHorizontal` active or `saved_custom_horizontal` present).
pub fn render_mangohud_presets(profile: &MangoHudProfile) -> String {
    let mut out = String::with_capacity(1280);
    for (ix, preset) in HudPreset::BUILTIN.into_iter().enumerate() {
        if ix > 0 {
            out.push('\n');
        }
        let mut effective = profile.clone();
        effective.apply_preset(preset);
        let _ = writeln!(out, "[preset {}]", preset.mangohud_preset_id());
        render_preset_body(&mut out, &effective);
    }
    if profile.has_custom_preset() {
        out.push('\n');
        let mut custom = profile.clone();
        if custom.preset != HudPreset::Custom
            && let Some(snapshot) = custom.saved_custom.clone()
        {
            custom.preset = HudPreset::Custom;
            custom.restore_custom_snapshot(&snapshot);
        }
        custom.orientation = HudOrientation::Vertical;
        let _ = writeln!(out, "[preset {}]", HudPreset::Custom.mangohud_preset_id());
        render_preset_body(&mut out, &custom);
    }
    if profile.has_custom_horizontal_preset() {
        out.push('\n');
        let mut custom_h = profile.clone();
        if custom_h.preset != HudPreset::CustomHorizontal
            && let Some(snapshot) = custom_h.saved_custom_horizontal.clone()
        {
            custom_h.preset = HudPreset::CustomHorizontal;
            custom_h.restore_custom_snapshot(&snapshot);
        }
        custom_h.orientation = HudOrientation::Horizontal;
        let _ = writeln!(
            out,
            "[preset {}]",
            HudPreset::CustomHorizontal.mangohud_preset_id()
        );
        render_preset_body(&mut out, &custom_h);
    }
    out
}

fn render_preset_body(out: &mut String, profile: &MangoHudProfile) {
    if profile.is_minimal_fps_only() {
        out.push_str("fps_only=1\n");
        out.push_str("frametime=0\n");
        return;
    }

    // Per-preset layout flags
    if profile.orientation == HudOrientation::Horizontal {
        out.push_str("horizontal\n");
        out.push_str("horizontal_stretch=0\n");
    }
    if profile.hud_compact {
        out.push_str("hud_compact\n");
    }
    if profile.hud_no_margin {
        out.push_str("hud_no_margin\n");
    }

    // GPU block
    if profile.effective_gpu_stats() {
        out.push_str("gpu_stats\n");
        if profile.gpu_temp {
            out.push_str("gpu_temp\n");
        }
        if profile.gpu_junction_temp {
            out.push_str("gpu_junction_temp\n");
        }
        if profile.gpu_mem_temp {
            out.push_str("gpu_mem_temp\n");
        }
        if profile.gpu_core_clock {
            out.push_str("gpu_core_clock\n");
        }
        if profile.gpu_mem_clock {
            out.push_str("gpu_mem_clock\n");
        }
        if profile.gpu_power {
            out.push_str("gpu_power\n");
        }
        if profile.gpu_fan {
            out.push_str("gpu_fan\n");
        }
    }
    if profile.vram {
        out.push_str("vram\n");
    }

    // CPU & RAM block
    if profile.effective_cpu_stats() {
        out.push_str("cpu_stats\n");
        if profile.cpu_temp {
            out.push_str("cpu_temp\n");
        }
        if profile.cpu_mhz {
            out.push_str("cpu_mhz\n");
        }
        if profile.cpu_power {
            out.push_str("cpu_power\n");
        }
    }
    if profile.ram {
        out.push_str("ram\n");
    }

    // FPS & Frame Pacing block
    if profile.fps {
        out.push_str("fps\n");
    }
    match (profile.fps_avg, profile.fps_1_low) {
        (true, true) => out.push_str("fps_metrics=avg,0.01\n"),
        (true, false) => out.push_str("fps_metrics=avg\n"),
        (false, true) => out.push_str("fps_metrics=0.01\n"),
        (false, false) => {}
    }
    // MangoHud defaults `frametime=true`; emit `frametime=0` explicitly when off.
    if profile.frametime {
        out.push_str("frametime\n");
    } else {
        out.push_str("frametime=0\n");
    }
    if profile.frame_timing {
        out.push_str("frame_timing\n");
    }
    if profile.present_mode {
        out.push_str("present_mode\n");
    }

    // Runtime block
    if profile.wine {
        out.push_str("wine\n");
    }
    if profile.winesync {
        out.push_str("winesync\n");
    }
    if profile.fsr {
        out.push_str("fsr\n");
    }
    if profile.hdr {
        out.push_str("hdr\n");
    }
    if profile.gamemode {
        out.push_str("gamemode\n");
    }
    if profile.throttling_status {
        out.push_str("throttling_status\n");
    }
    if profile.gamepad_battery {
        out.push_str("device_battery=gamepad\n");
    }
}

#[cfg(test)]
mod tests {
    use irori_core::{FpsLimitMethod, HudPosition, HudPreset};

    use super::*;

    fn lines(s: &str) -> Vec<&str> {
        s.lines().collect()
    }

    fn preset_section(presets_text: &str, id: u8) -> Vec<&str> {
        let header = format!("[preset {id}]");
        let mut inside = false;
        let mut section = Vec::new();
        for line in presets_text.lines() {
            let trimmed = line.trim();
            if trimmed == header {
                inside = true;
                continue;
            }
            if inside {
                if trimmed.starts_with('[') && trimmed.ends_with(']') {
                    break;
                }
                if !trimmed.is_empty() {
                    section.push(trimmed);
                }
            }
        }
        section
    }

    #[test]
    fn shared_conf_omits_per_preset_metrics_and_emits_cycle_and_shortcuts() {
        let mut p = MangoHudProfile::from_preset(HudPreset::MinimalFps);
        p.fps_color_change = true;
        p.palette = HudColorPalette::Monochrome;
        let conf = render_mangohud_conf(&p, &ConfRenderContext::default());
        let l = lines(&conf);
        assert_eq!(l[0], "legacy_layout=false");
        assert_eq!(l[1], "preset=1,2,3,4");
        assert!(l.contains(&"toggle_hud=Shift_R+F12"));
        assert!(l.contains(&"toggle_preset=Shift_R+F10"));
        assert!(l.contains(&"fps_color_change"));
        assert!(l.contains(&"fps_value=30,60"));
        assert!(l.contains(&"fps_color=B22222,FDFD09,39F900"));
        assert!(l.contains(&"background_color=0B0B0C"));
        // Display elements and preset-specific layout toggles must not appear in
        // MANGOHUD_CONFIGFILE, otherwise MangoHud duplicates them on preset cycle.
        for forbidden in [
            "fps_only=1",
            "fps",
            "gpu_stats",
            "cpu_stats",
            "vram",
            "ram",
            "horizontal",
            "hud_compact",
            "frame_timing",
        ] {
            assert!(
                !l.contains(&forbidden),
                "shared conf must not contain {forbidden}"
            );
        }
        assert!(!conf.contains("table_columns"));
        assert!(!l.iter().any(|line| line.starts_with("fps_limit=")));
        assert!(l.contains(&"fps_limit_method=late"));

        let presets = render_mangohud_presets(&p);
        let p1 = preset_section(&presets, 1);
        assert_eq!(p1, vec!["fps_only=1", "frametime=0"]);
        assert!(preset_section(&presets, 5).is_empty());
        assert!(preset_section(&presets, 6).is_empty());
    }

    #[test]
    fn vertical_and_horizontal_custom_presets_coexist_as_preset_5_and_6() {
        let mut p = MangoHudProfile::from_preset(HudPreset::BalancedGaming);
        p.gpu_fan = true;
        p.mark_custom();

        p.apply_preset(HudPreset::HorizontalCompact);
        p.cpu_power = true;
        p.mark_custom();

        let conf = render_mangohud_conf(&p, &ConfRenderContext::default());
        assert!(lines(&conf).contains(&"preset=6,1,2,3,4,5"));

        let presets = render_mangohud_presets(&p);
        let p5 = preset_section(&presets, 5);
        assert!(p5.contains(&"gpu_fan"));
        assert!(!p5.contains(&"horizontal"));
        assert!(!p5.contains(&"cpu_power"));

        let p6 = preset_section(&presets, 6);
        assert!(p6.contains(&"horizontal"));
        assert!(p6.contains(&"horizontal_stretch=0"));
        assert!(p6.contains(&"cpu_power"));
        assert!(!p6.contains(&"gpu_fan"));
    }

    #[test]
    fn custom_preset_and_gpu_cpu_master_locks_and_labels() {
        let mut p = MangoHudProfile::from_preset(HudPreset::Custom);
        p.gpu_stats = false;
        p.gpu_junction_temp = true;
        p.gpu_mem_temp = true;
        p.vram = false;
        p.cpu_stats = false;
        p.cpu_temp = true;
        p.ram = false;
        p.fps = true;
        p.frametime = false;
        p.toggle_preset = "Shift_L+F9\n=bad".into();

        let ctx = ConfRenderContext {
            detected_gpu_label: "RX #6=6,0;+0\n".into(),
            detected_cpu_label: "R7=7800X3D".into(),
            ..Default::default()
        };
        let conf = render_mangohud_conf(&p, &ctx);
        let l = lines(&conf);
        assert!(l.contains(&"preset=5,1,2,3,4"));
        assert!(l.contains(&"toggle_preset=Shift_L+F9bad"));
        assert!(l.contains(&"gpu_text=RX 6600"));
        assert!(l.contains(&"cpu_text=R77800X3D"));

        let presets = render_mangohud_presets(&p);
        let p5 = preset_section(&presets, 5);
        assert!(!p5.contains(&"fps_only=1"));
        assert!(p5.contains(&"gpu_stats"));
        assert!(p5.contains(&"gpu_junction_temp"));
        assert!(p5.contains(&"gpu_mem_temp"));
        assert!(p5.contains(&"cpu_stats"));
        assert!(p5.contains(&"cpu_temp"));
        assert!(p5.contains(&"fps"));
        assert!(p5.contains(&"frametime=0"));

        // Switching to a built-in preset preserves saved_custom in [preset 5].
        p.mark_custom();
        p.apply_preset(HudPreset::BalancedGaming);
        let conf_after = render_mangohud_conf(&p, &ctx);
        assert!(lines(&conf_after).contains(&"preset=3,4,5,1,2"));
        let presets_after = render_mangohud_presets(&p);
        let p5_after = preset_section(&presets_after, 5);
        assert!(p5_after.contains(&"gpu_junction_temp"));
        assert!(p5_after.contains(&"frametime=0"));

        // VRAM and RAM work independently even when gpu_stats and cpu_stats are off.
        let mut p2 = MangoHudProfile::from_preset(HudPreset::Custom);
        p2.gpu_stats = false;
        p2.cpu_stats = false;
        p2.vram = true;
        p2.ram = true;
        p2.fps = true;
        let presets2 = render_mangohud_presets(&p2);
        let l2 = preset_section(&presets2, 5);
        assert!(!l2.contains(&"gpu_stats"));
        assert!(!l2.contains(&"cpu_stats"));
        assert!(l2.contains(&"vram"));
        assert!(l2.contains(&"ram"));
    }

    #[test]
    fn horizontal_mode_fps_limit_omission_and_device_battery_key() {
        let mut p = MangoHudProfile::from_preset(HudPreset::HorizontalCompact);
        p.fps_limit = Some(0);
        p.position = HudPosition::BottomRight;
        let conf = render_mangohud_conf(&p, &ConfRenderContext::default());
        let l = lines(&conf);
        assert_eq!(l[0], "legacy_layout=false");
        assert_eq!(l[1], "preset=2,3,4,1");
        assert!(l.contains(&"position=bottom-right"));
        assert!(!l.iter().any(|line| line.starts_with("fps_limit=")));
        assert!(l.contains(&"fps_limit_method=late"));

        p.fps_limit = None;
        p.fps_limit_method = FpsLimitMethod::Early;
        let conf_uncapped_early = render_mangohud_conf(&p, &ConfRenderContext::default());
        let l_uncapped_early = lines(&conf_uncapped_early);
        assert!(
            !l_uncapped_early
                .iter()
                .any(|line| line.starts_with("fps_limit="))
        );
        assert!(l_uncapped_early.contains(&"fps_limit_method=early"));

        let presets = render_mangohud_presets(&p);
        let p2 = preset_section(&presets, 2);
        assert!(p2.contains(&"horizontal"));
        assert!(p2.contains(&"horizontal_stretch=0"));
        assert!(p2.contains(&"hud_compact"));
        let p4 = preset_section(&presets, 4);
        assert!(p4.contains(&"device_battery=gamepad"));
        assert!(!presets.contains("gamepad_battery"));

        p.fps_limit = Some(120);
        p.fps_limit_method = FpsLimitMethod::Early;
        let conf_lim = render_mangohud_conf(&p, &ConfRenderContext::default());
        let l_lim = lines(&conf_lim);
        assert!(l_lim.contains(&"fps_limit=120"));
        assert!(l_lim.contains(&"fps_limit_method=early"));
    }

    #[test]
    fn palette_strips_hash_and_alpha_and_preserves_block_order() {
        let mut p = MangoHudProfile::from_preset(HudPreset::FullDiagnostics);
        p.palette = HudColorPalette::Custom;
        p.custom_colors.background_color = "#112233aa".into();
        p.custom_colors.gpu_color = "#2e9762ff".into();
        p.background_alpha = 0.65;
        p.alpha = 0.9;
        let ctx = ConfRenderContext {
            detected_gpu_label: "RX 9070 XT".into(),
            detected_cpu_label: "i5-14600K".into(),
            pci_dev: Some("0000:03:00.0".into()),
            theme_colors: None,
            ..Default::default()
        };
        let conf = render_mangohud_conf(&p, &ctx);
        let cl = lines(&conf);
        assert!(cl.contains(&"preset=4,1,2,3"));
        assert!(cl.contains(&"background_color=112233"));
        assert!(cl.contains(&"gpu_color=2E9762"));
        assert!(cl.contains(&"background_alpha=0.65"));
        assert!(cl.contains(&"alpha=0.90"));
        assert!(cl.contains(&"pci_dev=0000:03:00.0"));

        let cpos = |k: &str| cl.iter().position(|&x| x == k).unwrap();
        assert!(cpos("legacy_layout=false") < cpos("background_color=112233"));
        assert!(cpos("background_color=112233") < cpos("gpu_text=RX 9070 XT"));

        let presets = render_mangohud_presets(&p);
        let p4 = preset_section(&presets, 4);
        let pos = |k: &str| p4.iter().position(|&x| x == k).unwrap();
        assert!(pos("gpu_stats") < pos("vram"));
        assert!(pos("vram") < pos("cpu_stats"));
        assert!(pos("cpu_stats") < pos("ram"));
        assert!(pos("ram") < pos("fps"));
        assert!(pos("fps") < pos("wine"));
        assert!(pos("wine") < pos("device_battery=gamepad"));
    }

    #[test]
    fn emits_fps_label_and_fps_metrics_combinations() {
        let mut p = MangoHudProfile::from_preset(HudPreset::Custom);
        p.fps_use_label = true;
        p.fps_avg = true;
        p.fps_1_low = true;
        let conf = render_mangohud_conf(&p, &ConfRenderContext::default());
        assert!(lines(&conf).contains(&"fps_text=FPS"));
        let presets = render_mangohud_presets(&p);
        assert!(preset_section(&presets, 5).contains(&"fps_metrics=avg,0.01"));
        assert!(
            crate::hud::launch_env::format_mangohud_inline_config(&presets)
                .contains("fps_metrics=avg+0.01")
        );

        p.fps_1_low = false;
        let presets_avg = render_mangohud_presets(&p);
        assert!(preset_section(&presets_avg, 5).contains(&"fps_metrics=avg"));

        p.fps_avg = false;
        p.fps_1_low = true;
        p.fps_use_label = false;
        let conf_low = render_mangohud_conf(&p, &ConfRenderContext::default());
        assert!(!lines(&conf_low).contains(&"fps_text=FPS"));
        let presets_low = render_mangohud_presets(&p);
        assert!(preset_section(&presets_low, 5).contains(&"fps_metrics=0.01"));
    }

    #[test]
    fn default_palette_applies_hardware_vendor_colors_and_emits_gpu_mem_clock() {
        let mut p = MangoHudProfile::from_preset(HudPreset::BalancedGaming);
        p.gpu_mem_clock = true;
        p.mark_custom();
        let ctx = ConfRenderContext {
            detected_gpu_label: "RX 9070 XT".into(),
            detected_cpu_label: "i5-14600K".into(),
            gpu_vendor: HardwareVendor::Amd,
            cpu_vendor: HardwareVendor::Intel,
            ..Default::default()
        };
        let conf = render_mangohud_conf(&p, &ctx);
        let l = lines(&conf);
        assert!(l.contains(&"gpu_color=ED1C24"));
        assert!(l.contains(&"vram_color=ED1C24"));
        assert!(l.contains(&"cpu_color=0095FF"));
        assert!(l.contains(&"ram_color=00C23D"));
        assert!(l.contains(&"engine_color=FFFFFF"));
        let presets = render_mangohud_presets(&p);
        assert!(preset_section(&presets, 5).contains(&"gpu_mem_clock"));

        let ctx_nv = ConfRenderContext {
            gpu_vendor: HardwareVendor::Nvidia,
            cpu_vendor: HardwareVendor::Amd,
            ..Default::default()
        };
        let conf_nv = render_mangohud_conf(&p, &ctx_nv);
        let l_nv = lines(&conf_nv);
        assert!(l_nv.contains(&"gpu_color=76B900"));
        assert!(l_nv.contains(&"vram_color=76B900"));
        assert!(l_nv.contains(&"cpu_color=ED1C24"));
    }

    #[test]
    fn long_gpu_and_cpu_labels_truncate_with_ascii_ellipsis() {
        let mut p = MangoHudProfile::from_preset(HudPreset::BalancedGaming);
        let ctx = ConfRenderContext {
            detected_gpu_label: "RX 6600/6600 XT/6600M".into(),
            detected_cpu_label: "R7 7800X3D".into(),
            ..Default::default()
        };
        let conf = render_mangohud_conf(&p, &ctx);
        let l = lines(&conf);
        assert!(l.contains(&"gpu_text=RX 6600..."));
        assert!(l.contains(&"cpu_text=R7 7800X3D"));

        p.gpu_label_override = "RX 6600 - TEST TEST TEST".into();
        p.cpu_label_override = "Ryzen 7 7800X3D".into();
        let conf_override = render_mangohud_conf(&p, &ctx);
        let lo = lines(&conf_override);
        assert!(lo.contains(&"gpu_text=RX 6600..."));
        assert!(lo.contains(&"cpu_text=Ryzen 7..."));
    }
}
