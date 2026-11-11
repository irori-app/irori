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

/// The index to show for `library` and `user`: `current` while it was
/// built from them (and today), else a new one. HowLongToBeat times are
/// asked from the backend.
pub fn index_for(
    current: Option<&Arc<CategoryIndex>>,
    library: &LibrarySnapshot,
    user: &Arc<UserData>,
    hltb: &irori_services::HltbTimes,
) -> Arc<CategoryIndex> {
    let key = irori_services::smart_categories::IndexKey::new(library, user, hltb);
    match current {
        Some(index) if index.key == key => index.clone(),
        _ => Arc::new(CategoryIndex::build(
            key,
            library,
            user,
            &hltb.hours,
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
        let hltb = irori_services::HltbTimes::default();
        let first = index_for(None, &library, &user, &hltb);
        assert!(Arc::ptr_eq(
            &first,
            &index_for(Some(&first), &library, &user, &hltb)
        ));
        assert_eq!(first.count(&CategoryRef::Smart("s".into())), 1);
        // New HowLongToBeat times or new user data mean a new index.
        let newer = irori_services::HltbTimes {
            revision: 1,
            ..Default::default()
        };
        assert!(!Arc::ptr_eq(
            &first,
            &index_for(Some(&first), &library, &user, &newer)
        ));
        let other_user = Arc::new((*user).clone());
        assert!(!Arc::ptr_eq(
            &first,
            &index_for(Some(&first), &library, &other_user, &hltb)
        ));
    }
}
