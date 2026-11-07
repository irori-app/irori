//! How a game is played: alone, in co-op, on one screen with others, over a
//! network, or in a massively multiplayer world. Stores and wikis each name
//! these their own way; their mappings (Steam categories, GOG features,
//! PCGamingWiki's multiplayer table) turn into [`GameMode`]s here or in the
//! store crates, and the library filters, groups and searches by them.
//!
//! A list of modes is kept sorted in [`GameMode::ALL`] order without
//! repeats ([`normalize`]). Lists are read leniently ([`lenient`]): a mode
//! a newer Irori knows and this one doesn't is skipped, never an error, so
//! user data and caches it wrote still load.

use serde::{Deserialize, Deserializer, Serialize};

/// One way of playing a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameMode {
    /// One player.
    Single,
    /// Players on the same side, wherever they sit.
    Coop,
    /// Several players at one computer: split or shared screen, couch play.
    Local,
    /// Several players over the internet or a local network.
    Online,
    /// A massively multiplayer world.
    Mmo,
}

impl GameMode {
    /// Every mode, in the order lists, filters and chips show them.
    pub const ALL: [GameMode; 5] = [
        GameMode::Single,
        GameMode::Coop,
        GameMode::Local,
        GameMode::Online,
        GameMode::Mmo,
    ];

    /// The mode's id in files and in search (`mode:coop`).
    pub fn id(self) -> &'static str {
        match self {
            GameMode::Single => "single",
            GameMode::Coop => "coop",
            GameMode::Local => "local",
            GameMode::Online => "online",
            GameMode::Mmo => "mmo",
        }
    }

    /// Whether more than one person plays.
    pub fn is_multiplayer(self) -> bool {
        self != GameMode::Single
    }

    /// The modes a search term names (`mode:<term>`), any of which a game
    /// must have: the ids and the usual ways of writing them (`co-op`,
    /// `splitscreen`, `couch`, `solo`…), and `multiplayer` for every mode
    /// with more than one player. Case and dashes don't matter. `None` for
    /// a term that names no mode.
    pub fn from_search(term: &str) -> Option<Vec<GameMode>> {
        let folded: String = term
            .chars()
            .filter(|c| !matches!(c, '-' | '_' | ' '))
            .flat_map(char::to_lowercase)
            .collect();
        let one = |m| Some(vec![m]);
        match folded.as_str() {
            "single" | "singleplayer" | "solo" | "sp" => one(GameMode::Single),
            "coop" | "cooperative" => one(GameMode::Coop),
            "local" | "split" | "splitscreen" | "shared" | "sharedscreen" | "couch" => {
                one(GameMode::Local)
            }
            "online" | "lan" | "network" => one(GameMode::Online),
            "mmo" | "mmorpg" => one(GameMode::Mmo),
            "multi" | "multiplayer" | "mp" => Some(
                GameMode::ALL
                    .into_iter()
                    .filter(|m| m.is_multiplayer())
                    .collect(),
            ),
            _ => None,
        }
    }
}

/// Sort `modes` in [`GameMode::ALL`] order and drop repeats.
pub fn normalize(modes: &mut Vec<GameMode>) {
    modes.sort();
    modes.dedup();
}

/// `modes`, normalized.
pub fn normalized(mut modes: Vec<GameMode>) -> Vec<GameMode> {
    normalize(&mut modes);
    modes
}

/// One list entry: a mode, or something this Irori doesn't know.
#[derive(Deserialize)]
#[serde(untagged)]
enum Entry {
    Known(GameMode),
    Other(serde::de::IgnoredAny),
}

/// Read a list of modes, skipping entries that name no mode known here
/// (and a value that isn't a list), normalized.
pub fn lenient<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<GameMode>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum List {
        List(Vec<Entry>),
        Other(serde::de::IgnoredAny),
    }
    Ok(match List::deserialize(d)? {
        List::List(entries) => normalized(
            entries
                .into_iter()
                .filter_map(|e| match e {
                    Entry::Known(m) => Some(m),
                    Entry::Other(_) => None,
                })
                .collect(),
        ),
        List::Other(_) => Vec::new(),
    })
}

/// [`lenient`] for an optional list (`null` or missing is `None`).
pub fn lenient_option<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<GameMode>>, D::Error> {
    #[derive(Deserialize)]
    struct Wrap(#[serde(deserialize_with = "lenient")] Vec<GameMode>);
    Ok(Option::<Wrap>::deserialize(d)?.map(|w| w.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_terms_name_modes() {
        assert_eq!(GameMode::from_search("coop"), Some(vec![GameMode::Coop]));
        assert_eq!(GameMode::from_search("Co-op"), Some(vec![GameMode::Coop]));
        assert_eq!(
            GameMode::from_search("split-screen"),
            Some(vec![GameMode::Local])
        );
        assert_eq!(GameMode::from_search("SOLO"), Some(vec![GameMode::Single]));
        assert_eq!(
            GameMode::from_search("multiplayer"),
            Some(vec![
                GameMode::Coop,
                GameMode::Local,
                GameMode::Online,
                GameMode::Mmo
            ])
        );
        assert_eq!(GameMode::from_search("racing"), None);
        for mode in GameMode::ALL {
            assert_eq!(GameMode::from_search(mode.id()), Some(vec![mode]));
        }
    }

    #[test]
    fn lists_read_leniently_and_normalized() {
        #[derive(Deserialize)]
        struct Holder {
            #[serde(default, deserialize_with = "lenient")]
            modes: Vec<GameMode>,
            #[serde(default, deserialize_with = "lenient_option")]
            chosen: Option<Vec<GameMode>>,
        }
        let h: Holder = serde_json::from_str(
            r#"{"modes": ["online", "vr", "single", 3, "online"], "chosen": ["mmo", "battle_royale"]}"#,
        )
        .unwrap();
        assert_eq!(h.modes, [GameMode::Single, GameMode::Online]);
        assert_eq!(h.chosen, Some(vec![GameMode::Mmo]));
        let h: Holder = serde_json::from_str(r#"{"modes": "co-op", "chosen": null}"#).unwrap();
        assert!(h.modes.is_empty());
        assert_eq!(h.chosen, None);
        assert_eq!(
            serde_json::to_string(&GameMode::ALL).unwrap(),
            r#"["single","coop","local","online","mmo"]"#
        );
    }
}
