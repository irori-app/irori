//! Pure-`std` Windows Virtual-Key (`VK_*`) mapping table for OptiScaler's
//! `[Menu] ShortcutKey` setting.
//!
//! Converts GPUI `Keystroke::key` names (such as `"insert"`, `"delete"`,
//! `"home"`, `"f12"`, `"a"`) into OptiScaler's hexadecimal `VK_*` codes
//! (`"0x2D"`, `"0x2E"`, `"0x24"`, `"0x7B"`, `"0x41"`) and human-readable
//! labels with zero allocations on the static fast path.

use std::borrow::Cow;

/// Special `OptiScaler.ini` value meaning OptiScaler's default key (`Insert`, `0x2D`).
pub const SHORTCUT_AUTO: &str = "auto";
/// Special `OptiScaler.ini` value disabling the menu hotkey completely.
pub const SHORTCUT_DISABLED: &str = "-1";
/// Hexadecimal `VK_INSERT` code (`0x2D`).
pub const VK_INSERT_HEX: &str = "0x2D";
/// Hexadecimal `VK_DELETE` code (`0x2E`).
pub const VK_DELETE_HEX: &str = "0x2E";

/// One entry in the static GPUI key <-> Windows `VK_*` lookup table.
struct VkEntry {
    gpui_key: &'static str,
    vk_hex: &'static str,
    vk_code: u16,
    label: &'static str,
}

const VK_TABLE: &[VkEntry] = &[
    // Navigation & editing keys
    VkEntry {
        gpui_key: "insert",
        vk_hex: "0x2D",
        vk_code: 0x2D,
        label: "Insert",
    },
    VkEntry {
        gpui_key: "delete",
        vk_hex: "0x2E",
        vk_code: 0x2E,
        label: "Delete",
    },
    VkEntry {
        gpui_key: "home",
        vk_hex: "0x24",
        vk_code: 0x24,
        label: "Home",
    },
    VkEntry {
        gpui_key: "end",
        vk_hex: "0x23",
        vk_code: 0x23,
        label: "End",
    },
    VkEntry {
        gpui_key: "pageup",
        vk_hex: "0x21",
        vk_code: 0x21,
        label: "Page Up",
    },
    VkEntry {
        gpui_key: "pagedown",
        vk_hex: "0x22",
        vk_code: 0x22,
        label: "Page Down",
    },
    VkEntry {
        gpui_key: "backspace",
        vk_hex: "0x08",
        vk_code: 0x08,
        label: "Backspace",
    },
    VkEntry {
        gpui_key: "tab",
        vk_hex: "0x09",
        vk_code: 0x09,
        label: "Tab",
    },
    VkEntry {
        gpui_key: "enter",
        vk_hex: "0x0D",
        vk_code: 0x0D,
        label: "Enter",
    },
    VkEntry {
        gpui_key: "space",
        vk_hex: "0x20",
        vk_code: 0x20,
        label: "Space",
    },
    VkEntry {
        gpui_key: "pause",
        vk_hex: "0x13",
        vk_code: 0x13,
        label: "Pause",
    },
    VkEntry {
        gpui_key: "capslock",
        vk_hex: "0x14",
        vk_code: 0x14,
        label: "Caps Lock",
    },
    VkEntry {
        gpui_key: "printscreen",
        vk_hex: "0x2C",
        vk_code: 0x2C,
        label: "Print Screen",
    },
    VkEntry {
        gpui_key: "scrolllock",
        vk_hex: "0x91",
        vk_code: 0x91,
        label: "Scroll Lock",
    },
    VkEntry {
        gpui_key: "numlock",
        vk_hex: "0x90",
        vk_code: 0x90,
        label: "Num Lock",
    },
    VkEntry {
        gpui_key: "menu",
        vk_hex: "0x5D",
        vk_code: 0x5D,
        label: "Menu",
    },
    // Arrow keys
    VkEntry {
        gpui_key: "left",
        vk_hex: "0x25",
        vk_code: 0x25,
        label: "Left",
    },
    VkEntry {
        gpui_key: "up",
        vk_hex: "0x26",
        vk_code: 0x26,
        label: "Up",
    },
    VkEntry {
        gpui_key: "right",
        vk_hex: "0x27",
        vk_code: 0x27,
        label: "Right",
    },
    VkEntry {
        gpui_key: "down",
        vk_hex: "0x28",
        vk_code: 0x28,
        label: "Down",
    },
    // Function keys F1-F24
    VkEntry {
        gpui_key: "f1",
        vk_hex: "0x70",
        vk_code: 0x70,
        label: "F1",
    },
    VkEntry {
        gpui_key: "f2",
        vk_hex: "0x71",
        vk_code: 0x71,
        label: "F2",
    },
    VkEntry {
        gpui_key: "f3",
        vk_hex: "0x72",
        vk_code: 0x72,
        label: "F3",
    },
    VkEntry {
        gpui_key: "f4",
        vk_hex: "0x73",
        vk_code: 0x73,
        label: "F4",
    },
    VkEntry {
        gpui_key: "f5",
        vk_hex: "0x74",
        vk_code: 0x74,
        label: "F5",
    },
    VkEntry {
        gpui_key: "f6",
        vk_hex: "0x75",
        vk_code: 0x75,
        label: "F6",
    },
    VkEntry {
        gpui_key: "f7",
        vk_hex: "0x76",
        vk_code: 0x76,
        label: "F7",
    },
    VkEntry {
        gpui_key: "f8",
        vk_hex: "0x77",
        vk_code: 0x77,
        label: "F8",
    },
    VkEntry {
        gpui_key: "f9",
        vk_hex: "0x78",
        vk_code: 0x78,
        label: "F9",
    },
    VkEntry {
        gpui_key: "f10",
        vk_hex: "0x79",
        vk_code: 0x79,
        label: "F10",
    },
    VkEntry {
        gpui_key: "f11",
        vk_hex: "0x7A",
        vk_code: 0x7A,
        label: "F11",
    },
    VkEntry {
        gpui_key: "f12",
        vk_hex: "0x7B",
        vk_code: 0x7B,
        label: "F12",
    },
    VkEntry {
        gpui_key: "f13",
        vk_hex: "0x7C",
        vk_code: 0x7C,
        label: "F13",
    },
    VkEntry {
        gpui_key: "f14",
        vk_hex: "0x7D",
        vk_code: 0x7D,
        label: "F14",
    },
    VkEntry {
        gpui_key: "f15",
        vk_hex: "0x7E",
        vk_code: 0x7E,
        label: "F15",
    },
    VkEntry {
        gpui_key: "f16",
        vk_hex: "0x7F",
        vk_code: 0x7F,
        label: "F16",
    },
    VkEntry {
        gpui_key: "f17",
        vk_hex: "0x80",
        vk_code: 0x80,
        label: "F17",
    },
    VkEntry {
        gpui_key: "f18",
        vk_hex: "0x81",
        vk_code: 0x81,
        label: "F18",
    },
    VkEntry {
        gpui_key: "f19",
        vk_hex: "0x82",
        vk_code: 0x82,
        label: "F19",
    },
    VkEntry {
        gpui_key: "f20",
        vk_hex: "0x83",
        vk_code: 0x83,
        label: "F20",
    },
    VkEntry {
        gpui_key: "f21",
        vk_hex: "0x84",
        vk_code: 0x84,
        label: "F21",
    },
    VkEntry {
        gpui_key: "f22",
        vk_hex: "0x85",
        vk_code: 0x85,
        label: "F22",
    },
    VkEntry {
        gpui_key: "f23",
        vk_hex: "0x86",
        vk_code: 0x86,
        label: "F23",
    },
    VkEntry {
        gpui_key: "f24",
        vk_hex: "0x87",
        vk_code: 0x87,
        label: "F24",
    },
    // Digits 0-9
    VkEntry {
        gpui_key: "0",
        vk_hex: "0x30",
        vk_code: 0x30,
        label: "0",
    },
    VkEntry {
        gpui_key: "1",
        vk_hex: "0x31",
        vk_code: 0x31,
        label: "1",
    },
    VkEntry {
        gpui_key: "2",
        vk_hex: "0x32",
        vk_code: 0x32,
        label: "2",
    },
    VkEntry {
        gpui_key: "3",
        vk_hex: "0x33",
        vk_code: 0x33,
        label: "3",
    },
    VkEntry {
        gpui_key: "4",
        vk_hex: "0x34",
        vk_code: 0x34,
        label: "4",
    },
    VkEntry {
        gpui_key: "5",
        vk_hex: "0x35",
        vk_code: 0x35,
        label: "5",
    },
    VkEntry {
        gpui_key: "6",
        vk_hex: "0x36",
        vk_code: 0x36,
        label: "6",
    },
    VkEntry {
        gpui_key: "7",
        vk_hex: "0x37",
        vk_code: 0x37,
        label: "7",
    },
    VkEntry {
        gpui_key: "8",
        vk_hex: "0x38",
        vk_code: 0x38,
        label: "8",
    },
    VkEntry {
        gpui_key: "9",
        vk_hex: "0x39",
        vk_code: 0x39,
        label: "9",
    },
    // Letters A-Z
    VkEntry {
        gpui_key: "a",
        vk_hex: "0x41",
        vk_code: 0x41,
        label: "A",
    },
    VkEntry {
        gpui_key: "b",
        vk_hex: "0x42",
        vk_code: 0x42,
        label: "B",
    },
    VkEntry {
        gpui_key: "c",
        vk_hex: "0x43",
        vk_code: 0x43,
        label: "C",
    },
    VkEntry {
        gpui_key: "d",
        vk_hex: "0x44",
        vk_code: 0x44,
        label: "D",
    },
    VkEntry {
        gpui_key: "e",
        vk_hex: "0x45",
        vk_code: 0x45,
        label: "E",
    },
    VkEntry {
        gpui_key: "f",
        vk_hex: "0x46",
        vk_code: 0x46,
        label: "F",
    },
    VkEntry {
        gpui_key: "g",
        vk_hex: "0x47",
        vk_code: 0x47,
        label: "G",
    },
    VkEntry {
        gpui_key: "h",
        vk_hex: "0x48",
        vk_code: 0x48,
        label: "H",
    },
    VkEntry {
        gpui_key: "i",
        vk_hex: "0x49",
        vk_code: 0x49,
        label: "I",
    },
    VkEntry {
        gpui_key: "j",
        vk_hex: "0x4A",
        vk_code: 0x4A,
        label: "J",
    },
    VkEntry {
        gpui_key: "k",
        vk_hex: "0x4B",
        vk_code: 0x4B,
        label: "K",
    },
    VkEntry {
        gpui_key: "l",
        vk_hex: "0x4C",
        vk_code: 0x4C,
        label: "L",
    },
    VkEntry {
        gpui_key: "m",
        vk_hex: "0x4D",
        vk_code: 0x4D,
        label: "M",
    },
    VkEntry {
        gpui_key: "n",
        vk_hex: "0x4E",
        vk_code: 0x4E,
        label: "N",
    },
    VkEntry {
        gpui_key: "o",
        vk_hex: "0x4F",
        vk_code: 0x4F,
        label: "O",
    },
    VkEntry {
        gpui_key: "p",
        vk_hex: "0x50",
        vk_code: 0x50,
        label: "P",
    },
    VkEntry {
        gpui_key: "q",
        vk_hex: "0x51",
        vk_code: 0x51,
        label: "Q",
    },
    VkEntry {
        gpui_key: "r",
        vk_hex: "0x52",
        vk_code: 0x52,
        label: "R",
    },
    VkEntry {
        gpui_key: "s",
        vk_hex: "0x53",
        vk_code: 0x53,
        label: "S",
    },
    VkEntry {
        gpui_key: "t",
        vk_hex: "0x54",
        vk_code: 0x54,
        label: "T",
    },
    VkEntry {
        gpui_key: "u",
        vk_hex: "0x55",
        vk_code: 0x55,
        label: "U",
    },
    VkEntry {
        gpui_key: "v",
        vk_hex: "0x56",
        vk_code: 0x56,
        label: "V",
    },
    VkEntry {
        gpui_key: "w",
        vk_hex: "0x57",
        vk_code: 0x57,
        label: "W",
    },
    VkEntry {
        gpui_key: "x",
        vk_hex: "0x58",
        vk_code: 0x58,
        label: "X",
    },
    VkEntry {
        gpui_key: "y",
        vk_hex: "0x59",
        vk_code: 0x59,
        label: "Y",
    },
    VkEntry {
        gpui_key: "z",
        vk_hex: "0x5A",
        vk_code: 0x5A,
        label: "Z",
    },
    // Numpad
    VkEntry {
        gpui_key: "numpad0",
        vk_hex: "0x60",
        vk_code: 0x60,
        label: "Numpad 0",
    },
    VkEntry {
        gpui_key: "numpad1",
        vk_hex: "0x61",
        vk_code: 0x61,
        label: "Numpad 1",
    },
    VkEntry {
        gpui_key: "numpad2",
        vk_hex: "0x62",
        vk_code: 0x62,
        label: "Numpad 2",
    },
    VkEntry {
        gpui_key: "numpad3",
        vk_hex: "0x63",
        vk_code: 0x63,
        label: "Numpad 3",
    },
    VkEntry {
        gpui_key: "numpad4",
        vk_hex: "0x64",
        vk_code: 0x64,
        label: "Numpad 4",
    },
    VkEntry {
        gpui_key: "numpad5",
        vk_hex: "0x65",
        vk_code: 0x65,
        label: "Numpad 5",
    },
    VkEntry {
        gpui_key: "numpad6",
        vk_hex: "0x66",
        vk_code: 0x66,
        label: "Numpad 6",
    },
    VkEntry {
        gpui_key: "numpad7",
        vk_hex: "0x67",
        vk_code: 0x67,
        label: "Numpad 7",
    },
    VkEntry {
        gpui_key: "numpad8",
        vk_hex: "0x68",
        vk_code: 0x68,
        label: "Numpad 8",
    },
    VkEntry {
        gpui_key: "numpad9",
        vk_hex: "0x69",
        vk_code: 0x69,
        label: "Numpad 9",
    },
    VkEntry {
        gpui_key: "numpadmultiply",
        vk_hex: "0x6A",
        vk_code: 0x6A,
        label: "Numpad *",
    },
    VkEntry {
        gpui_key: "numpadadd",
        vk_hex: "0x6B",
        vk_code: 0x6B,
        label: "Numpad +",
    },
    VkEntry {
        gpui_key: "numpadsubtract",
        vk_hex: "0x6D",
        vk_code: 0x6D,
        label: "Numpad -",
    },
    VkEntry {
        gpui_key: "numpaddecimal",
        vk_hex: "0x6E",
        vk_code: 0x6E,
        label: "Numpad .",
    },
    VkEntry {
        gpui_key: "numpaddivide",
        vk_hex: "0x6F",
        vk_code: 0x6F,
        label: "Numpad /",
    },
    // OEM punctuation
    VkEntry {
        gpui_key: ";",
        vk_hex: "0xBA",
        vk_code: 0xBA,
        label: ";",
    },
    VkEntry {
        gpui_key: "=",
        vk_hex: "0xBB",
        vk_code: 0xBB,
        label: "=",
    },
    VkEntry {
        gpui_key: ",",
        vk_hex: "0xBC",
        vk_code: 0xBC,
        label: ",",
    },
    VkEntry {
        gpui_key: "-",
        vk_hex: "0xBD",
        vk_code: 0xBD,
        label: "-",
    },
    VkEntry {
        gpui_key: ".",
        vk_hex: "0xBE",
        vk_code: 0xBE,
        label: ".",
    },
    VkEntry {
        gpui_key: "/",
        vk_hex: "0xBF",
        vk_code: 0xBF,
        label: "/",
    },
    VkEntry {
        gpui_key: "`",
        vk_hex: "0xC0",
        vk_code: 0xC0,
        label: "`",
    },
    VkEntry {
        gpui_key: "[",
        vk_hex: "0xDB",
        vk_code: 0xDB,
        label: "[",
    },
    VkEntry {
        gpui_key: "\\",
        vk_hex: "0xDC",
        vk_code: 0xDC,
        label: "\\",
    },
    VkEntry {
        gpui_key: "]",
        vk_hex: "0xDD",
        vk_code: 0xDD,
        label: "]",
    },
    VkEntry {
        gpui_key: "'",
        vk_hex: "0xDE",
        vk_code: 0xDE,
        label: "'",
    },
];

/// Returns true if `gpui_key` is a standalone modifier key that should be
/// ignored while waiting for a real shortcut key.
pub fn is_modifier_key(gpui_key: &str) -> bool {
    matches!(
        gpui_key.trim().to_ascii_lowercase().as_str(),
        "shift"
            | "control"
            | "ctrl"
            | "alt"
            | "platform"
            | "meta"
            | "super"
            | "fn"
            | "function"
            | "altgraph"
    )
}

/// Map a GPUI `Keystroke::key` string (case-insensitive) to OptiScaler's
/// hexadecimal `VK_*` value (for example `"delete"` -> `Some("0x2E")`).
pub fn gpui_key_to_vk(gpui_key: &str) -> Option<&'static str> {
    let trimmed = gpui_key.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = match trimmed.to_ascii_lowercase().as_str() {
        " " => "space",
        "return" => "enter",
        "del" => "delete",
        "ins" => "insert",
        "prior" => "pageup",
        "next" => "pagedown",
        _ => "",
    };
    let needle = if normalized.is_empty() {
        trimmed
    } else {
        normalized
    };
    VK_TABLE
        .iter()
        .find(|e| e.gpui_key.eq_ignore_ascii_case(needle))
        .map(|e| e.vk_hex)
}

/// Normalize a user-provided or stored shortcut string into canonical
/// `OptiScaler.ini` form:
/// - `""` or `"auto"` -> `Some("auto")`
/// - `"-1"` -> `Some("-1")`
/// - `"0x2e"` / `"0x2E"` / `"46"` -> `Some("0x2E")`
/// - `"delete"` -> `Some("0x2E")`
///
/// Returns `None` for invalid or out-of-range values.
pub fn normalize_shortcut_key(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case(SHORTCUT_AUTO) {
        return Some(SHORTCUT_AUTO.to_string());
    }
    if trimmed == SHORTCUT_DISABLED {
        return Some(SHORTCUT_DISABLED.to_string());
    }
    if let Some(hex) = gpui_key_to_vk(trimmed) {
        return Some(hex.to_string());
    }
    if let Some(code) = parse_vk_code(trimmed) {
        return Some(format!("0x{code:02X}"));
    }
    None
}

fn parse_vk_code(s: &str) -> Option<u16> {
    let code = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16).ok()?
    } else {
        s.parse::<u16>().ok()?
    };
    (1..=0xFE).contains(&code).then_some(code)
}

/// Human-readable short key name for a stored shortcut value (for example
/// `"0x2E"` -> `"Delete"`, `"auto"` / `""` -> `"Insert"`, `"-1"` -> `"Disabled"`).
pub fn vk_display_name(raw: &str) -> Cow<'static, str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case(SHORTCUT_AUTO) {
        return Cow::Borrowed("Insert");
    }
    if trimmed == SHORTCUT_DISABLED {
        return Cow::Borrowed("Disabled");
    }
    if let Some(code) = parse_vk_code(trimmed) {
        if let Some(entry) = VK_TABLE.iter().find(|e| e.vk_code == code) {
            return Cow::Borrowed(entry.label);
        }
        return Cow::Owned(format!("0x{code:02X}"));
    }
    if let Some(entry) = VK_TABLE
        .iter()
        .find(|e| e.gpui_key.eq_ignore_ascii_case(trimmed))
    {
        return Cow::Borrowed(entry.label);
    }
    Cow::Owned(trimmed.to_string())
}

/// Display badge combining human key name and hex code when a custom key is
/// set (for example `"Delete (0x2E)"`, `"Insert (auto)"`, `"Disabled"`).
pub fn vk_display_badge(raw: &str) -> Cow<'static, str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case(SHORTCUT_AUTO) {
        return Cow::Borrowed("Insert");
    }
    if trimmed == SHORTCUT_DISABLED {
        return Cow::Borrowed("Disabled");
    }
    let name = vk_display_name(trimmed);
    if let Some(code) = parse_vk_code(trimmed) {
        let hex = format!("0x{code:02X}");
        if name.as_ref() == hex {
            return Cow::Owned(hex);
        }
        return Cow::Owned(format!("{name} ({hex})"));
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_gpui_keys_to_windows_vk_codes() {
        assert_eq!(gpui_key_to_vk("insert"), Some("0x2D"));
        assert_eq!(gpui_key_to_vk("delete"), Some("0x2E"));
        assert_eq!(gpui_key_to_vk("Delete"), Some("0x2E"));
        assert_eq!(gpui_key_to_vk("home"), Some("0x24"));
        assert_eq!(gpui_key_to_vk("end"), Some("0x23"));
        assert_eq!(gpui_key_to_vk("pageup"), Some("0x21"));
        assert_eq!(gpui_key_to_vk("pagedown"), Some("0x22"));
        assert_eq!(gpui_key_to_vk("backspace"), Some("0x08"));
        assert_eq!(gpui_key_to_vk("f1"), Some("0x70"));
        assert_eq!(gpui_key_to_vk("f12"), Some("0x7B"));
        assert_eq!(gpui_key_to_vk("a"), Some("0x41"));
        assert_eq!(gpui_key_to_vk("0"), Some("0x30"));
        assert_eq!(gpui_key_to_vk("numpad5"), Some("0x65"));
        assert_eq!(gpui_key_to_vk("escape"), None);
    }

    #[test]
    fn normalizes_and_formats_shortcuts() {
        assert_eq!(normalize_shortcut_key(""), Some("auto".into()));
        assert_eq!(normalize_shortcut_key("auto"), Some("auto".into()));
        assert_eq!(normalize_shortcut_key("-1"), Some("-1".into()));
        assert_eq!(normalize_shortcut_key("0x2e"), Some("0x2E".into()));
        assert_eq!(normalize_shortcut_key("46"), Some("0x2E".into()));
        assert_eq!(normalize_shortcut_key("delete"), Some("0x2E".into()));
        assert_eq!(normalize_shortcut_key("0x00"), None);
        assert_eq!(normalize_shortcut_key("not-a-key"), None);

        assert_eq!(vk_display_name(""), "Insert");
        assert_eq!(vk_display_name("auto"), "Insert");
        assert_eq!(vk_display_name("0x2E"), "Delete");
        assert_eq!(vk_display_name("0x2e"), "Delete");
        assert_eq!(vk_display_name("-1"), "Disabled");
        assert_eq!(vk_display_badge("0x2E"), "Delete (0x2E)");
        assert_eq!(vk_display_badge("auto"), "Insert");
        assert!(is_modifier_key("shift"));
        assert!(is_modifier_key("Control"));
        assert!(!is_modifier_key("delete"));
    }
}
