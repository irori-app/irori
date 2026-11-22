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
    if key == " " {
        return Some("space".to_string());
    }
    let normalized = key.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }

    // Bare modifier keys cannot be standalone toggle bindings.
    if matches!(
        normalized.as_str(),
        "shift"
            | "shift_l"
            | "shift_r"
            | "control"
            | "ctrl"
            | "control_l"
            | "control_r"
            | "alt"
            | "alt_l"
            | "alt_r"
            | "super"
            | "super_l"
            | "super_r"
            | "meta"
            | "meta_l"
            | "meta_r"
            | "platform"
            | "function"
            | "capslock"
            | "caps_lock"
    ) {
        return None;
    }

    // Function keys F1..F12
    if let Some(num_str) = normalized.strip_prefix('f')
        && let Ok(num) = num_str.parse::<u8>()
        && (1..=12).contains(&num)
    {
        return Some(format!("F{num}"));
    }

    // Special named X11 keysyms (exact case sensitivity required by XStringToKeysym)
    let special = match normalized.as_str() {
        "insert" => Some("Insert"),
        "home" => Some("Home"),
        "end" => Some("End"),
        "delete" => Some("Delete"),
        "pageup" | "page_up" => Some("Prior"),
        "pagedown" | "page_down" => Some("Next"),
        "pause" => Some("Pause"),
        "scrolllock" | "scroll_lock" => Some("Scroll_Lock"),
        "tab" => Some("Tab"),
        "grave" | "`" => Some("grave"),
        "backslash" | "\\" => Some("backslash"),
        "minus" | "-" => Some("minus"),
        "equal" | "=" => Some("equal"),
        "space" => Some("space"),
        _ => None,
    };
    if let Some(sym) = special {
        return Some(sym.to_string());
    }

    // Single ASCII lowercase letters (`a`..`z`) and digits (`0`..`9`)
    if normalized.len() == 1 {
        let b = normalized.as_bytes()[0];
        if b.is_ascii_lowercase() || b.is_ascii_digit() {
            return Some(normalized);
        }
    }

    None
}

/// Format a captured GPUI keystroke into a MangoHud `toggle_hud` binding.
pub fn format_toggle_hud(modifiers: KeyModifiers, key: &str, side: ModifierSide) -> Option<String> {
    let keysym = gpui_key_to_x11_keysym(key)?;
    let suffix = match side {
        ModifierSide::Left => "L",
        ModifierSide::Right => "R",
    };

    let mut parts: Vec<String> = Vec::with_capacity(4);
    if modifiers.shift {
        parts.push(format!("Shift_{suffix}"));
    }
    if modifiers.control {
        parts.push(format!("Control_{suffix}"));
    }
    if modifiers.alt {
        parts.push(format!("Alt_{suffix}"));
    }
    parts.push(keysym);
    Some(parts.join("+"))
}

/// Return true if `combo` contains at least one modifier token (`Shift_*`,
/// `Control_*`, `Alt_*`).
pub fn combo_has_modifiers(combo: &str) -> bool {
    combo.split('+').map(str::trim).any(|tok| {
        matches!(
            tok,
            "Shift_L" | "Shift_R" | "Control_L" | "Control_R" | "Alt_L" | "Alt_R"
        )
    })
}

/// Inspect whether `combo`'s modifiers are currently left-sided or right-sided.
pub fn combo_modifier_side(combo: &str) -> Option<ModifierSide> {
    for tok in combo.split('+').map(str::trim) {
        match tok {
            "Shift_L" | "Control_L" | "Alt_L" => return Some(ModifierSide::Left),
            "Shift_R" | "Control_R" | "Alt_R" => return Some(ModifierSide::Right),
            _ => {}
        }
    }
    None
}

/// Rewrite modifier tokens in `combo` to use `side` (`*_L` vs `*_R`).
pub fn set_modifier_side(combo: &str, side: ModifierSide) -> String {
    let suffix = match side {
        ModifierSide::Left => "L",
        ModifierSide::Right => "R",
    };
    combo
        .split('+')
        .map(|raw| {
            let tok = raw.trim();
            match tok {
                "Shift_L" | "Shift_R" => format!("Shift_{suffix}"),
                "Control_L" | "Control_R" => format!("Control_{suffix}"),
                "Alt_L" | "Alt_R" => format!("Alt_{suffix}"),
                other => other.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x11_keysym_case_sensitivity() {
        assert_eq!(gpui_key_to_x11_keysym("f1"), Some("F1".into()));
        assert_eq!(gpui_key_to_x11_keysym("f12"), Some("F12".into()));
        assert_eq!(gpui_key_to_x11_keysym("F12"), Some("F12".into()));
        assert_eq!(gpui_key_to_x11_keysym("insert"), Some("Insert".into()));
        assert_eq!(gpui_key_to_x11_keysym("home"), Some("Home".into()));
        assert_eq!(gpui_key_to_x11_keysym("end"), Some("End".into()));
        assert_eq!(gpui_key_to_x11_keysym("delete"), Some("Delete".into()));
        assert_eq!(gpui_key_to_x11_keysym("pageup"), Some("Prior".into()));
        assert_eq!(gpui_key_to_x11_keysym("pagedown"), Some("Next".into()));
        assert_eq!(gpui_key_to_x11_keysym("pause"), Some("Pause".into()));
        assert_eq!(
            gpui_key_to_x11_keysym("scrolllock"),
            Some("Scroll_Lock".into())
        );
        assert_eq!(
            gpui_key_to_x11_keysym("scroll_lock"),
            Some("Scroll_Lock".into())
        );
        assert_eq!(gpui_key_to_x11_keysym("tab"), Some("Tab".into()));
        assert_eq!(gpui_key_to_x11_keysym("grave"), Some("grave".into()));
        assert_eq!(
            gpui_key_to_x11_keysym("backslash"),
            Some("backslash".into())
        );
        assert_eq!(gpui_key_to_x11_keysym("minus"), Some("minus".into()));
        assert_eq!(gpui_key_to_x11_keysym("equal"), Some("equal".into()));
        assert_eq!(gpui_key_to_x11_keysym("space"), Some("space".into()));
        assert_eq!(gpui_key_to_x11_keysym(" "), Some("space".into()));
        assert_eq!(gpui_key_to_x11_keysym("h"), Some("h".into()));
        assert_eq!(gpui_key_to_x11_keysym("H"), Some("h".into()));
        assert_eq!(gpui_key_to_x11_keysym("9"), Some("9".into()));

        for bare in ["shift", "control", "alt", "super", "meta", "f13", "unknown"] {
            assert_eq!(gpui_key_to_x11_keysym(bare), None, "{bare}");
        }
    }

    #[test]
    fn toggle_hud_capture_and_modifier_side_flip() {
        assert_eq!(DEFAULT_TOGGLE_HUD, "Shift_R+F12");
        assert!(combo_has_modifiers(DEFAULT_TOGGLE_HUD));
        assert_eq!(
            combo_modifier_side(DEFAULT_TOGGLE_HUD),
            Some(ModifierSide::Right)
        );

        let left = format_toggle_hud(
            KeyModifiers {
                shift: true,
                control: false,
                alt: false,
            },
            "f12",
            ModifierSide::Left,
        )
        .unwrap();
        assert_eq!(left, "Shift_L+F12");
        assert_eq!(combo_modifier_side(&left), Some(ModifierSide::Left));

        let flipped = set_modifier_side(&left, ModifierSide::Right);
        assert_eq!(flipped, "Shift_R+F12");
        assert_eq!(combo_modifier_side(&flipped), Some(ModifierSide::Right));

        let multi = format_toggle_hud(
            KeyModifiers {
                shift: true,
                control: true,
                alt: true,
            },
            "insert",
            ModifierSide::Left,
        )
        .unwrap();
        assert_eq!(multi, "Shift_L+Control_L+Alt_L+Insert");
        assert_eq!(
            set_modifier_side(&multi, ModifierSide::Right),
            "Shift_R+Control_R+Alt_R+Insert"
        );

        let plain = format_toggle_hud(KeyModifiers::default(), "f10", ModifierSide::Left).unwrap();
        assert_eq!(plain, "F10");
        assert!(!combo_has_modifiers(&plain));
        assert_eq!(combo_modifier_side(&plain), None);
    }
}
