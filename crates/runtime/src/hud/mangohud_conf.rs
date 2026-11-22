//! Pure, cross-platform `MangoHud.conf` serializer.

use irori_core::{MangoHudProfile, ThemeHudColors};

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
    let _ = (profile, theme_colors);
    None
}

/// Serialize `profile` and `ctx` into a deterministic `MangoHud.conf` string.
pub fn render_mangohud_conf(profile: &MangoHudProfile, ctx: &ConfRenderContext) -> String {
    let _ = (profile, ctx);
    "legacy_layout=false\n".into()
}
