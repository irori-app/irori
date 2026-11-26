//! Native Performance HUD settings UI (MangoHud on Linux, Metal HUD on macOS).

pub mod panel;
pub mod preview;
pub mod shortcut_recorder;
mod steam_guide;

pub use panel::{HudPanel, HudPanelScope};
pub use preview::HudPreview;
pub use shortcut_recorder::ShortcutRecorder;
