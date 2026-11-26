//! Native Performance HUD settings UI (MangoHud on Linux, Metal HUD on macOS).

mod color_wheel;
pub mod panel;
pub mod preview;
pub mod shortcut_recorder;

pub use panel::{HudPanel, HudPanelScope};
pub use preview::HudPreview;
pub use shortcut_recorder::ShortcutRecorder;
