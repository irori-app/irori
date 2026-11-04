//! The themes Irori ships, kept as theme files in `themes/` so they go
//! through the same parser as the ones people share.

use once_cell::sync::Lazy;

use super::Theme;

/// A theme from the gallery.
#[derive(Debug, Clone)]
pub struct BuiltIn {
    pub id: &'static str,
    pub theme: Theme,
}

/// Irori's own look: the interface theme's colours over the focused
/// game's art.
pub const DEFAULT_ID: &str = "irori";

const FILES: &[(&str, &str)] = &[
    (
        DEFAULT_ID,
        include_str!("../../themes/irori.irori-theme.json"),
    ),
    (
        "hearth",
        include_str!("../../themes/hearth.irori-theme.json"),
    ),
    (
        "neon-arcade",
        include_str!("../../themes/neon-arcade.irori-theme.json"),
    ),
    (
        "paperback",
        include_str!("../../themes/paperback.irori-theme.json"),
    ),
    ("moss", include_str!("../../themes/moss.irori-theme.json")),
    ("noir", include_str!("../../themes/noir.irori-theme.json")),
    (
        "glacier",
        include_str!("../../themes/glacier.irori-theme.json"),
    ),
    (
        "handheld",
        include_str!("../../themes/handheld.irori-theme.json"),
    ),
];

static GALLERY: Lazy<Vec<BuiltIn>> = Lazy::new(|| {
    FILES
        .iter()
        .filter_map(|(id, json)| match Theme::from_json(json) {
            Ok(theme) => Some(BuiltIn { id, theme }),
            Err(e) => {
                tracing::error!("built-in Big Picture theme {id}: {e:?}");
                None
            }
        })
        .collect()
});

/// Every built-in theme, Irori's own first.
pub fn all() -> &'static [BuiltIn] {
    &GALLERY
}

/// The built-in theme `id`.
pub fn get(id: &str) -> Option<&'static BuiltIn> {
    all().iter().find(|b| b.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_theme_parses_and_round_trips() {
        assert_eq!(all().len(), FILES.len());
        assert_eq!(all()[0].id, DEFAULT_ID);
        assert_eq!(all()[0].theme, Theme::default());
        for b in all() {
            let again = Theme::from_json(&b.theme.to_json()).unwrap();
            assert_eq!(again, b.theme, "{}", b.id);
        }
    }
}
