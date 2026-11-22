//! Case-sensitive X11 keysym mapping for GPUI `Keystroke` -> MangoHud `toggle_hud`.

use irori_core::ModifierSide;
pub use irori_core::hud::DEFAULT_TOGGLE_HUD;

/// Platform-neutral modifier flags passed from a GPUI `Keystroke`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

/// Convert a lowercase GPUI `Keystroke.key` into MangoHud's exact case-sensitive
/// X11 keysym name (`XStringToKeysym`). Returns `None` for bare modifiers or
/// unsupported keys.
pub fn gpui_key_to_x11_keysym(key: &str) -> Option<String> {
    let _ = key;
    None
}

/// Format a captured GPUI keystroke into a MangoHud `toggle_hud` binding.
pub fn format_toggle_hud(modifiers: KeyModifiers, key: &str, side: ModifierSide) -> Option<String> {
    let _ = (modifiers, key, side);
    None
}

/// Return true if `combo` contains at least one modifier token (`Shift_*`,
/// `Control_*`, `Alt_*`).
pub fn combo_has_modifiers(combo: &str) -> bool {
    let _ = combo;
    false
}

/// Inspect whether `combo`'s modifiers are currently left-sided or right-sided.
pub fn combo_modifier_side(combo: &str) -> Option<ModifierSide> {
    let _ = combo;
    None
}

/// Rewrite modifier tokens in `combo` to use `side` (`*_L` vs `*_R`).
pub fn set_modifier_side(combo: &str, side: ModifierSide) -> String {
    let _ = side;
    combo.to_string()
}
