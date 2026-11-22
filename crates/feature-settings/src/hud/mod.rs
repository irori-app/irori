//! Native Performance HUD settings UI (MangoHud on Linux, Metal HUD on macOS).

pub mod panel;
pub mod preview;
pub mod shortcut_recorder;

pub use panel::{HudPanel, HudPanelScope};
pub use preview::HudPreview;
pub use shortcut_recorder::ShortcutRecorder;

/// Temporary debug-only QA hook for visually inspecting the MangoHud or Metal
/// Performance HUD UI on any host (`IRORI_QA_HUD_VIEW=mangohud|metal`).
#[cfg(debug_assertions)]
pub fn qa_hud_backend_override() -> Option<irori_core::HudBackend> {
    match std::env::var("IRORI_QA_HUD_VIEW")
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("mangohud") => Some(irori_core::HudBackend::MangoHud),
        Some("metal") => Some(irori_core::HudBackend::MetalHud),
        _ => None,
    }
}

/// Release builds never override the host HUD backend.
#[cfg(not(debug_assertions))]
pub fn qa_hud_backend_override() -> Option<irori_core::HudBackend> {
    None
}
