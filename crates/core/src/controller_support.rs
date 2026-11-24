//! How a game plays with a controller, one fact from every source that
//! says: Steam's store page (`full`, `partial`), GOG's "Controller support"
//! feature and PCGamingWiki's `{{Input}}` (limited, or only with a fix, read
//! as partial). Lists, filters, smart rules and Big Picture read this one
//! fact instead of each source's words.

use serde::{Deserialize, Deserializer, Serialize};

/// How much of a game a controller plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ControllerSupport {
    /// Some of it: menus or modes need a mouse and keyboard.
    Partial,
    /// All of it.
    Full,
}

impl ControllerSupport {
    /// Steam's word for it (`appdetails`' `controller_support`).
    pub fn from_steam(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "full" => Some(Self::Full),
            "partial" => Some(Self::Partial),
            _ => None,
        }
    }

    /// Whether it meets `wanted`: full meets both, partial only partial.
    pub fn meets(self, wanted: ControllerSupport) -> bool {
        self >= wanted
    }
}

/// Read a kept value leniently: a word a newer Irori writes reads as
/// unknown instead of failing the whole file.
pub fn lenient<'de, D: Deserializer<'de>>(d: D) -> Result<Option<ControllerSupport>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(d)?;
    Ok(value.and_then(|v| serde_json::from_value(v).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_steam_and_unknown_words() {
        assert_eq!(
            ControllerSupport::from_steam("full"),
            Some(ControllerSupport::Full)
        );
        assert_eq!(
            ControllerSupport::from_steam(" Partial "),
            Some(ControllerSupport::Partial)
        );
        assert_eq!(ControllerSupport::from_steam("none"), None);
        #[derive(Deserialize)]
        struct Kept {
            #[serde(default, deserialize_with = "lenient")]
            c: Option<ControllerSupport>,
        }
        let kept: Kept = serde_json::from_str(r#"{"c": "gyroOnly"}"#).unwrap();
        assert_eq!(kept.c, None);
        let kept: Kept = serde_json::from_str(r#"{"c": "full"}"#).unwrap();
        assert_eq!(kept.c, Some(ControllerSupport::Full));
        let kept: Kept = serde_json::from_str("{}").unwrap();
        assert_eq!(kept.c, None);
        assert!(ControllerSupport::Full.meets(ControllerSupport::Partial));
        assert!(!ControllerSupport::Partial.meets(ControllerSupport::Full));
    }
}
