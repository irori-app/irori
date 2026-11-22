//! Pure, cross-platform `MangoHud.conf` serializer.

use std::fmt::Write as _;

use irori_core::hud::{DEFAULT_TOGGLE_HUD, sanitize_hex6, sanitize_hud_label};
use irori_core::{HudColorPalette, HudOrientation, MangoHudProfile, ThemeHudColors};

/// Runtime context supplied when rendering a `MangoHud.conf` file.
#[derive(Debug, Clone, Default)]
pub struct ConfRenderContext {
    pub detected_gpu_label: String,
    pub detected_cpu_label: String,
    pub pci_dev: Option<String>,
    pub theme_colors: Option<ThemeHudColors>,
}

/// Resolve the active 6-digit hex color palette for `profile`, returning `None`
/// when `HudColorPalette::MangoHudDefault` is selected.
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

/// Serialize `profile` and `ctx` into a deterministic `MangoHud.conf` string.
pub fn render_mangohud_conf(profile: &MangoHudProfile, ctx: &ConfRenderContext) -> String {
    let mut out = String::with_capacity(512);
    let minimal = profile.is_minimal_fps_only();

    // (a) Layout, window, scaling, toggle, GPU selector and frame limiter
    out.push_str("legacy_layout=false\n");
    if minimal {
        out.push_str("fps_only=1\n");
    }
    let _ = writeln!(out, "position={}", profile.position.as_mangohud_value());
    if !minimal && profile.orientation == HudOrientation::Horizontal {
        out.push_str("horizontal\n");
        out.push_str("horizontal_stretch=0\n");
    }
    let _ = writeln!(out, "round_corners={}", profile.round_corners);
    if !minimal {
        if profile.hud_compact {
            out.push_str("hud_compact\n");
        }
        if profile.hud_no_margin {
            out.push_str("hud_no_margin\n");
        }
    }
    let font_scale = profile.font_scale.clamp(0.5, 3.0);
    let bg_alpha = profile.background_alpha.clamp(0.0, 1.0);
    let alpha = profile.alpha.clamp(0.0, 1.0);
    let _ = writeln!(out, "font_scale={font_scale:.2}");
    let _ = writeln!(out, "background_alpha={bg_alpha:.2}");
    let _ = writeln!(out, "alpha={alpha:.2}");

    let toggle: String = profile
        .toggle_hud
        .chars()
        .filter(|&c| !matches!(c, '\r' | '\n' | '#' | '=' | ',' | ';'))
        .collect::<String>()
        .trim()
        .to_string();
    let toggle = if toggle.is_empty() {
        DEFAULT_TOGGLE_HUD
    } else {
        toggle.as_str()
    };
    let _ = writeln!(out, "toggle_hud={toggle}");

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
        let _ = writeln!(
            out,
            "fps_limit_method={}",
            profile.fps_limit_method.as_mangohud_value()
        );
    }

    // (b) Palette colors + optional FPS color change
    if let Some(colors) = resolve_palette_colors(profile, ctx.theme_colors.as_ref()) {
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

    if minimal {
        return out;
    }

    // (c) GPU block
    if profile.effective_gpu_stats() {
        out.push_str("gpu_stats\n");
        let gpu_label = if profile.gpu_label_override.trim().is_empty() {
            sanitize_hud_label(&ctx.detected_gpu_label)
        } else {
            sanitize_hud_label(&profile.gpu_label_override)
        };
        if !gpu_label.is_empty() {
            let _ = writeln!(out, "gpu_text={gpu_label}");
        }
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

    // (d) CPU & RAM block
    if profile.effective_cpu_stats() {
        out.push_str("cpu_stats\n");
        let cpu_label = if profile.cpu_label_override.trim().is_empty() {
            sanitize_hud_label(&ctx.detected_cpu_label)
        } else {
            sanitize_hud_label(&profile.cpu_label_override)
        };
        if !cpu_label.is_empty() {
            let _ = writeln!(out, "cpu_text={cpu_label}");
        }
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

    // (e) FPS & Frame Pacing block
    if profile.fps {
        out.push_str("fps\n");
    }
    if profile.frametime {
        out.push_str("frametime\n");
    }
    if profile.frame_timing {
        out.push_str("frame_timing\n");
    }
    if profile.present_mode {
        out.push_str("present_mode\n");
    }

    // (f) Runtime block
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

    out
}

#[cfg(test)]
mod tests {
    use irori_core::{FpsLimitMethod, HudPosition, HudPreset};

    use super::*;

    fn lines(s: &str) -> Vec<&str> {
        s.lines().collect()
    }

    #[test]
    fn minimal_fps_only_emits_fps_only_and_skips_blocks() {
        let mut p = MangoHudProfile::from_preset(HudPreset::MinimalFps);
        p.fps_color_change = true;
        p.palette = HudColorPalette::Monochrome;
        let out = render_mangohud_conf(&p, &ConfRenderContext::default());
        let l = lines(&out);
        assert_eq!(l[0], "legacy_layout=false");
        assert_eq!(l[1], "fps_only=1");
        assert!(l.contains(&"fps_color_change"));
        assert!(l.contains(&"fps_value=30,60"));
        assert!(l.contains(&"fps_color=B22222,FDFD09,39F900"));
        assert!(l.contains(&"background_color=0B0B0C"));
        assert!(!l.contains(&"fps"));
        assert!(!l.contains(&"gpu_stats"));
        assert!(!out.contains("table_columns"));
        assert!(!out.contains("fps_limit"));
    }

    #[test]
    fn gpu_and_cpu_master_locks_and_vram_ram_independence() {
        let mut p = MangoHudProfile::from_preset(HudPreset::Custom);
        p.gpu_stats = false;
        p.gpu_junction_temp = true;
        p.gpu_mem_temp = true;
        p.vram = false;
        p.cpu_stats = false;
        p.cpu_temp = true;
        p.ram = false;
        p.fps = true;

        let ctx = ConfRenderContext {
            detected_gpu_label: "RX 9070 XT #1=fast,ok;+yes\n".into(),
            detected_cpu_label: "Ryzen 7=7800X3D".into(),
            ..Default::default()
        };
        let out = render_mangohud_conf(&p, &ctx);
        let l = lines(&out);
        assert!(!l.contains(&"fps_only=1"));
        assert!(l.contains(&"gpu_stats"));
        assert!(l.contains(&"gpu_text=RX 9070 XT 1fastokyes"));
        assert!(l.contains(&"gpu_junction_temp"));
        assert!(l.contains(&"gpu_mem_temp"));
        assert!(l.contains(&"cpu_stats"));
        assert!(l.contains(&"cpu_text=Ryzen 77800X3D"));
        assert!(l.contains(&"cpu_temp"));

        // VRAM and RAM work independently even when gpu_stats and cpu_stats are off.
        let mut p2 = MangoHudProfile::from_preset(HudPreset::Custom);
        p2.gpu_stats = false;
        p2.cpu_stats = false;
        p2.vram = true;
        p2.ram = true;
        p2.fps = true;
        let out2 = render_mangohud_conf(&p2, &ConfRenderContext::default());
        let l2 = lines(&out2);
        assert!(!l2.contains(&"gpu_stats"));
        assert!(!l2.contains(&"cpu_stats"));
        assert!(l2.contains(&"vram"));
        assert!(l2.contains(&"ram"));
    }

    #[test]
    fn horizontal_mode_fps_limit_omission_and_device_battery_key() {
        let mut p = MangoHudProfile::from_preset(HudPreset::HorizontalCompact);
        p.fps_limit = Some(0);
        p.gamepad_battery = true;
        p.position = HudPosition::BottomRight;
        let out = render_mangohud_conf(&p, &ConfRenderContext::default());
        let l = lines(&out);
        assert_eq!(l[0], "legacy_layout=false");
        assert!(l.contains(&"position=bottom-right"));
        assert!(l.contains(&"horizontal"));
        assert!(l.contains(&"horizontal_stretch=0"));
        assert!(!out.contains("fps_limit"));
        assert!(l.contains(&"device_battery=gamepad"));
        assert!(!out.contains("gamepad_battery"));

        p.fps_limit = Some(120);
        p.fps_limit_method = FpsLimitMethod::Early;
        let out_lim = render_mangohud_conf(&p, &ConfRenderContext::default());
        let l_lim = lines(&out_lim);
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
        };
        let out = render_mangohud_conf(&p, &ctx);
        let l = lines(&out);
        assert!(l.contains(&"background_color=112233"));
        assert!(l.contains(&"gpu_color=2E9762"));
        assert!(l.contains(&"background_alpha=0.65"));
        assert!(l.contains(&"alpha=0.90"));
        assert!(l.contains(&"pci_dev=0000:03:00.0"));

        let pos = |k: &str| l.iter().position(|&x| x == k).unwrap();
        assert!(pos("legacy_layout=false") < pos("background_color=112233"));
        assert!(pos("background_color=112233") < pos("gpu_stats"));
        assert!(pos("gpu_stats") < pos("vram"));
        assert!(pos("vram") < pos("cpu_stats"));
        assert!(pos("cpu_stats") < pos("ram"));
        assert!(pos("ram") < pos("fps"));
        assert!(pos("fps") < pos("wine"));
        assert!(pos("wine") < pos("device_battery=gamepad"));
    }
}
