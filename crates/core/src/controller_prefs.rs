//! Controller preferences (`iroriController`): how controller input behaves
//! everywhere in Irori. Kept apart from `settings.rs` because Big Picture,
//! the in-game overlay and the desktop screens all read it.

use serde::{Deserialize, Serialize};

/// Controller preferences (`iroriController`). Irori-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ControllerPrefs {
    /// Hide the mouse pointer on controller input until the mouse moves
    /// (`hidePointer`, on unless turned off).
    pub hide_pointer: bool,
}

impl Default for ControllerPrefs {
    fn default() -> Self {
        Self { hide_pointer: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_keys_read_as_the_defaults() {
        let prefs: ControllerPrefs = serde_json::from_str("{}").unwrap();
        assert!(prefs.hide_pointer);
        let prefs: ControllerPrefs = serde_json::from_str(r#"{"hidePointer":false}"#).unwrap();
        assert!(!prefs.hide_pointer);
    }
}
