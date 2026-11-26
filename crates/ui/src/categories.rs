//! Categories of both kinds on every screen that lists them: the library's
//! menus and grouping, covers, the command palette and Big Picture. Manual
//! and smart categories are listed together ([`entries`]), pinned ones
//! first, each with its live count from the library model's
//! [`CategoryIndex`], and coloured from the theme ([`colour`]).

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Hsla, SharedString};
use irori_core::{GameId, GameInfo};
use irori_services::{CategoryIndex, CategoryRef, LibrarySnapshot, UserData};

use crate::i18n::t;

/// One category as lists show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryEntry {
    pub category: CategoryRef,
    pub name: String,
    /// Shown games (hidden ones aside).
    pub count: usize,
    pub pinned: bool,
    /// The stored colour name ([`irori_services::smart_categories::COLOURS`]).
    pub colour: Option<String>,
    /// The stored icon slug ([`irori_services::smart_categories::ICONS`]).
    pub icon: Option<String>,
    /// For a smart category: whether this version reads all of it.
    pub readable: bool,
}

impl CategoryEntry {
    pub fn is_smart(&self) -> bool {
        matches!(self.category, CategoryRef::Smart(_))
    }
}

/// Every category of both kinds, pinned first, then by name.
pub fn entries(user: &UserData, index: &CategoryIndex) -> Vec<CategoryEntry> {
    user.categories_in_order()
        .into_iter()
        .map(|(category, name)| {
            let look = user.category_look(&category);
            let readable = match &category {
                CategoryRef::Smart(id) => index.is_readable(id),
                CategoryRef::Manual(_) => true,
            };
            CategoryEntry {
                count: index.count(&category),
                pinned: look.pinned,
                colour: look.colour,
                icon: look.icon,
                readable,
                category,
                name,
            }
        })
        .collect()
}

/// Whether `game` is in `category`.
pub fn contains(
    category: &CategoryRef,
    game: &GameId,
    user: &UserData,
    index: &CategoryIndex,
) -> bool {
    match category {
        CategoryRef::Manual(name) => user
            .categories
            .get(name)
            .is_some_and(|games| games.contains(&game.key())),
        CategoryRef::Smart(id) => index.contains(id, game),
    }
}

/// The library's entries in `category`, hidden games left out, in library
/// order (Big Picture's shelves and filter).
pub fn games_in(
    category: &CategoryRef,
    library: &LibrarySnapshot,
    user: &UserData,
    index: &CategoryIndex,
) -> Vec<Arc<GameInfo>> {
    library
        .games
        .iter()
        .filter(|g| !user.hidden.contains(&g.id().key()))
        .filter(|g| in_entry(category, g, library, user, index))
        .cloned()
        .collect()
}

/// Whether any copy of the entry `game` heads is in `category`.
pub fn in_entry(
    category: &CategoryRef,
    game: &GameInfo,
    library: &LibrarySnapshot,
    user: &UserData,
    index: &CategoryIndex,
) -> bool {
    let id = game.id();
    let copies = library.copies_of(&id);
    if copies.is_empty() {
        contains(category, &id, user, index)
    } else {
        copies.iter().any(|c| contains(category, c, user, index))
    }
}

/// The categories `game` is in, as (name, colour), pinned first.
pub fn badges(
    game: &GameId,
    user: &UserData,
    index: &CategoryIndex,
) -> Vec<(String, Option<String>)> {
    user.categories_in_order()
        .into_iter()
        .filter(|(category, _)| contains(category, game, user, index))
        .map(|(category, name)| (name, user.category_look(&category).colour))
        .collect()
}

/// The theme colour of a stored colour name; `None` for no colour or one
/// this version doesn't know.
pub fn colour(name: Option<&str>, cx: &App) -> Option<Hsla> {
    let theme = cx.theme();
    Some(match name? {
        "red" => theme.red,
        "yellow" => theme.yellow,
        "green" => theme.green,
        "cyan" => theme.cyan,
        "blue" => theme.blue,
        "magenta" => theme.magenta,
        _ => return None,
    })
}

/// The translated name of a stored colour.
pub fn colour_label(name: &str) -> SharedString {
    t(match name {
        "red" => "ui.category.colour.red",
        "yellow" => "ui.category.colour.yellow",
        "green" => "ui.category.colour.green",
        "cyan" => "ui.category.colour.cyan",
        "blue" => "ui.category.colour.blue",
        "magenta" => "ui.category.colour.magenta",
        _ => "ui.category.colour.none",
    })
}

/// The icon marking a category's kind: smart ones are filled from rules.
pub fn icon(category: &CategoryRef) -> IconName {
    match category {
        CategoryRef::Manual(_) => IconName::Tag,
        CategoryRef::Smart(_) => IconName::Sparkles,
    }
}

/// Resolve a stored collection icon slug ([`irori_services::smart_categories::ICONS`])
/// into its Lucide [`IconName`], falling back to `fallback` when unset or unknown.
pub fn icon_by_name(name: Option<&str>, fallback: IconName) -> IconName {
    match name {
        Some("sparkles") => IconName::Sparkles,
        Some("flame") => IconName::Flame,
        Some("swords") => IconName::Swords,
        Some("skull") => IconName::Skull,
        Some("ghost") => IconName::Ghost,
        Some("wand-sparkles") => IconName::WandSparkles,
        Some("moon-star") => IconName::MoonStar,
        Some("crosshair") => IconName::Crosshair,
        Some("rocket") => IconName::Rocket,
        Some("castle") => IconName::Castle,
        Some("chess-pawn") => IconName::ChessPawn,
        Some("dices") => IconName::Dices,
        Some("puzzle") => IconName::Puzzle,
        Some("compass") => IconName::Compass,
        Some("pickaxe") => IconName::Pickaxe,
        Some("coffee") => IconName::Coffee,
        Some("book-open") => IconName::BookOpen,
        Some("car") => IconName::Car,
        Some("trophy") => IconName::Trophy,
        Some("users") => IconName::Users,
        Some("music") => IconName::Music,
        Some("gamepad-2") => IconName::Gamepad2,
        Some("gem") => IconName::Gem,
        Some("crown") => IconName::Crown,
        _ => fallback,
    }
}

/// The icon for a category, honoring any custom icon stored in its look.
pub fn icon_for(category: &CategoryRef, icon_name: Option<&str>) -> IconName {
    icon_by_name(icon_name, icon(category))
}

/// Suggest a curated collection icon slug from the first matching tag/genre
/// term in `terms`. Returns `None` when no curated tag matches.
pub fn suggest_icon_for_terms(terms: &[String]) -> Option<&'static str> {
    for raw in terms {
        let key = irori_services::search::tag_key(raw);
        let suggested = match key.as_str() {
            "soulslike" | "souls" | "bonfire" | "bullethell" => Some("flame"),
            "darkfantasy" | "hardcore" | "postapocalyptic" | "gore" => Some("skull"),
            "horror" | "survivalhorror" | "psychologicalhorror" => Some("ghost"),
            "anime" | "jrpg" | "atmospheric" => Some("moon-star"),
            "fantasy" | "magic" => Some("wand-sparkles"),
            "fps" | "shooter" | "thirdpersonshooter" | "lootershooter" | "stealth" | "tactical" => {
                Some("crosshair")
            }
            "scifi" | "cyberpunk" | "space" | "mecha" | "vr" => Some("rocket"),
            "metroidvania" | "medieval" | "citybuilder" | "towerdefense" | "grandstrategy" => {
                Some("castle")
            }
            "strategy" | "rts" | "4x" | "turnbased" | "management" => Some("chess-pawn"),
            "roguelike" | "roguelite" | "deckbuilder" | "cardgame" => Some("dices"),
            "puzzle" | "mystery" | "detective" | "pointclick" => Some("puzzle"),
            "adventure" | "openworld" | "exploration" => Some("compass"),
            "survival" | "crafting" | "basebuilding" | "sandbox" => Some("pickaxe"),
            "cozy" | "relaxing" | "farmingsim" | "lifesim" | "casual" => Some("coffee"),
            "visualnovel" | "narrative" | "storyrich" | "choicesmatter" => Some("book-open"),
            "racing" | "driving" | "simulation" => Some("car"),
            "sports" | "pvp" | "competitive" | "esports" => Some("trophy"),
            "coop" | "localcoop" | "onlinecoop" | "multiplayer" | "mmo" => Some("users"),
            "rhythm" | "music" | "soundtrack" => Some("music"),
            "action" | "rpg" | "crpg" | "arpg" | "hackandslash" | "beatemup" | "fighting" => {
                Some("swords")
            }
            "indie" => Some("gem"),
            "platformer" | "2dplatformer" | "3dplatformer" | "retro" | "pixelgraphics"
            | "arcade" | "isometric" | "topdown" | "singleplayer" | "freetoplay" => {
                Some("gamepad-2")
            }
            _ => None,
        };
        if suggested.is_some() {
            return suggested;
        }
    }
    None
}

/// The index to show for `library` and `user`: `current` while it was
/// built from them, `inputs` (`Backend::category_inputs`) and today, else a
/// new one.
pub fn index_for(
    current: Option<&Arc<CategoryIndex>>,
    library: &LibrarySnapshot,
    user: &Arc<UserData>,
    inputs: &irori_services::smart_categories::Inputs,
) -> Arc<CategoryIndex> {
    let key = irori_services::smart_categories::IndexKey::new(library, user, inputs);
    match current {
        Some(index) if index.key == key => index.clone(),
        _ => Arc::new(CategoryIndex::build(
            key,
            library,
            user,
            inputs,
            &chrono::Local,
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use irori_core::Runner;
    use irori_services::{Match, Rule, SmartCategory};

    use super::*;

    #[test]
    fn the_index_is_reused_until_its_inputs_change() {
        let mut user = UserData::default();
        user.smart_categories.insert(
            "s".into(),
            SmartCategory::new(
                "Installed",
                Match::All,
                vec![Rule::Installed { installed: true }],
                vec![],
            ),
        );
        let user = Arc::new(user);
        let library = LibrarySnapshot::new(
            3,
            irori_services::copies::Grouped {
                entries: vec![Arc::new(
                    GameInfo {
                        runner: Some(Runner::Gog),
                        app_name: "1".into(),
                        is_installed: true,
                        ..Default::default()
                    }
                    .finalize(),
                )],
                ..Default::default()
            },
            HashMap::new(),
        );
        let inputs = irori_services::smart_categories::Inputs::default();
        let first = index_for(None, &library, &user, &inputs);
        assert!(Arc::ptr_eq(
            &first,
            &index_for(Some(&first), &library, &user, &inputs)
        ));
        assert_eq!(first.count(&CategoryRef::Smart("s".into())), 1);
        // New HowLongToBeat times, plays, updates or user data mean a new
        // index.
        let mut newer = inputs.clone();
        newer.hltb.revision = 1;
        assert!(!Arc::ptr_eq(
            &first,
            &index_for(Some(&first), &library, &user, &newer)
        ));
        let mut played = inputs.clone();
        let mut stats = irori_services::sessions::SessionStats::default();
        stats.revision = 5;
        played.plays = Arc::new(stats);
        assert!(!Arc::ptr_eq(
            &first,
            &index_for(Some(&first), &library, &user, &played)
        ));
        let mut updated = inputs.clone();
        updated.updates = Arc::default();
        assert!(!Arc::ptr_eq(
            &first,
            &index_for(Some(&first), &library, &user, &updated)
        ));
        let other_user = Arc::new((*user).clone());
        assert!(!Arc::ptr_eq(
            &first,
            &index_for(Some(&first), &library, &other_user, &inputs)
        ));
    }

    #[test]
    fn curated_collection_icons_and_tag_suggestions_cover_all_slugs() {
        for slug in irori_services::smart_categories::ICONS {
            let resolved = icon_by_name(Some(slug), IconName::Tag);
            assert_ne!(
                resolved,
                IconName::Tag,
                "slug {slug} must map to a known Lucide IconName"
            );
        }
        assert_eq!(
            icon_by_name(Some("future-icon"), IconName::Sparkles),
            IconName::Sparkles
        );
        assert_eq!(
            icon_for(&CategoryRef::Smart("s".into()), Some("flame")),
            IconName::Flame
        );
        assert_eq!(
            suggest_icon_for_terms(&["Souls-like".into()]),
            Some("flame")
        );
        assert_eq!(suggest_icon_for_terms(&["Horror".into()]), Some("ghost"));
        assert_eq!(suggest_icon_for_terms(&["Anime".into()]), Some("moon-star"));
        assert_eq!(suggest_icon_for_terms(&["Sci-Fi".into()]), Some("rocket"));
        assert_eq!(suggest_icon_for_terms(&["Cozy".into()]), Some("coffee"));
        assert_eq!(suggest_icon_for_terms(&["Unknown Tag".into()]), None);
    }
}
