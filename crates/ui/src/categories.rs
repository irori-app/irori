//! Categories of both kinds on every screen that lists them: the library's
//! menus and grouping, covers, the command palette and Big Picture. Manual
//! and smart categories are listed together ([`entries`]), pinned ones
//! first, each with its live count from the library model's
//! [`CategoryIndex`], and coloured from the theme ([`colour`]).

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, Hsla, SharedString, rgb};
use irori_core::{GameId, GameInfo};
use irori_services::smart_categories::{format_hex_colour, parse_hex_colour};
use irori_services::{CategoryIndex, CategoryRef, LibrarySnapshot, UserData};

use crate::components::color_wheel::hsla_to_hex6;
use crate::i18n::t;
use crate::style::{ICON_CONTRAST, legible};

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

/// Derive a theme-harmonized named hue from the active theme's base palette
/// saturation and lightness, keeping contrast readable on `theme.background`.
fn extended_hue(hue_deg: f32, cx: &App) -> Hsla {
    let theme = cx.theme();
    let bases = [
        theme.red,
        theme.green,
        theme.yellow,
        theme.blue,
        theme.magenta,
        theme.cyan,
    ];
    let avg_s = bases.iter().map(|c| c.s).sum::<f32>() / bases.len() as f32;
    let avg_l = bases.iter().map(|c| c.l).sum::<f32>() / bases.len() as f32;
    let (s, l) = if theme.mode.is_dark() {
        (avg_s.clamp(0.58, 0.88), avg_l.clamp(0.58, 0.72))
    } else {
        (avg_s.clamp(0.58, 0.85), avg_l.clamp(0.36, 0.48))
    };
    let raw = Hsla {
        h: hue_deg.rem_euclid(360.0) / 360.0,
        s,
        l,
        a: 1.0,
    };
    legible(raw, &[theme.background], ICON_CONTRAST, cx)
}

/// The theme colour of a stored colour name or `#rrggbb` hex value; `None` for
/// no colour or one this version doesn't know.
pub fn colour(name: Option<&str>, cx: &App) -> Option<Hsla> {
    let name = name?;
    let theme = cx.theme();
    if let Some(val) = parse_hex_colour(name) {
        let raw: Hsla = rgb(val).into();
        return Some(legible(raw, &[theme.background], ICON_CONTRAST, cx));
    }
    Some(match name {
        "red" => theme.red,
        "orange" => extended_hue(24.0, cx),
        "amber" => extended_hue(38.0, cx),
        "yellow" => theme.yellow,
        "lime" => extended_hue(86.0, cx),
        "green" => theme.green,
        "teal" => extended_hue(170.0, cx),
        "cyan" => theme.cyan,
        "sky" => extended_hue(198.0, cx),
        "blue" => theme.blue,
        "indigo" => extended_hue(238.0, cx),
        "purple" => extended_hue(272.0, cx),
        "magenta" => theme.magenta,
        "pink" => extended_hue(334.0, cx),
        _ => return None,
    })
}

/// Raw uppercase 6-digit `RRGGBB` hex for initializing the Hue wheel without
/// losing the exact custom colour the user dragged to.
pub fn raw_colour_hex(name: Option<&str>, cx: &App) -> Option<String> {
    let name = name?;
    if let Some(val) = parse_hex_colour(name) {
        return Some(format!("{val:06X}"));
    }
    colour(Some(name), cx).map(hsla_to_hex6)
}

/// Whether `name` is a custom `#rrggbb` hex colour rather than a named palette swatch.
pub fn is_custom_hex_colour(name: Option<&str>) -> bool {
    name.and_then(parse_hex_colour).is_some()
}

/// The translated name of a stored colour (or uppercase `#RRGGBB` for a custom hex).
pub fn colour_label(name: &str) -> SharedString {
    if let Some(hex) = format_hex_colour(name) {
        return SharedString::from(hex.to_ascii_uppercase());
    }
    t(match name {
        "red" => "ui.category.colour.red",
        "orange" => "ui.category.colour.orange",
        "amber" => "ui.category.colour.amber",
        "yellow" => "ui.category.colour.yellow",
        "lime" => "ui.category.colour.lime",
        "green" => "ui.category.colour.green",
        "teal" => "ui.category.colour.teal",
        "cyan" => "ui.category.colour.cyan",
        "sky" => "ui.category.colour.sky",
        "blue" => "ui.category.colour.blue",
        "indigo" => "ui.category.colour.indigo",
        "purple" => "ui.category.colour.purple",
        "magenta" => "ui.category.colour.magenta",
        "pink" => "ui.category.colour.pink",
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

/// Metadata for a curated collection icon used for lookup and keyword search.
#[derive(Clone, Copy, Debug)]
pub struct IconEntry {
    pub slug: &'static str,
    pub icon: IconName,
    pub keywords: &'static str,
}

/// Quick-pick icons shown directly on the main row of the collection dialog.
pub const QUICK_ICONS: [&str; 18] = [
    "sparkles",
    "flame",
    "swords",
    "shield",
    "skull",
    "ghost",
    "wand-sparkles",
    "crosshair",
    "rocket",
    "castle",
    "dices",
    "puzzle",
    "compass",
    "pickaxe",
    "coffee",
    "trophy",
    "gamepad-2",
    "heart",
];

/// Full curated icon catalog (all 64 entries of [`irori_services::smart_categories::ICONS`])
/// with searchable keywords across common game genres and organization terms.
pub const ICON_CATALOG: [IconEntry; 64] = [
    // Combat & action
    IconEntry {
        slug: "flame",
        icon: IconName::Flame,
        keywords: "fire fogo souls soulslike bonfire hot action acao",
    },
    IconEntry {
        slug: "swords",
        icon: IconName::Swords,
        keywords: "sword espada combat combate action rpg fighting hack slash",
    },
    IconEntry {
        slug: "shield",
        icon: IconName::Shield,
        keywords: "defense defesa knight cavaleiro tank paladin tower",
    },
    IconEntry {
        slug: "axe",
        icon: IconName::Axe,
        keywords: "machado viking barbarian survival melee",
    },
    IconEntry {
        slug: "bow-arrow",
        icon: IconName::BowArrow,
        keywords: "archery arco ranger hunter stealth",
    },
    IconEntry {
        slug: "crosshair",
        icon: IconName::Crosshair,
        keywords: "fps shooter tiro sniper aim tactical mira",
    },
    IconEntry {
        slug: "bomb",
        icon: IconName::Bomb,
        keywords: "explosive bomba boomer shooter demolition",
    },
    IconEntry {
        slug: "zap",
        icon: IconName::Zap,
        keywords: "lightning raio electric fast speedrun cyberpunk arcade",
    },
    IconEntry {
        slug: "biohazard",
        icon: IconName::Biohazard,
        keywords: "zombie zumbi virus outbreak toxic postapocalyptic",
    },
    // Fantasy, horror & sci-fi
    IconEntry {
        slug: "sparkles",
        icon: IconName::Sparkles,
        keywords: "default smart brilho destaque especial",
    },
    IconEntry {
        slug: "wand-sparkles",
        icon: IconName::WandSparkles,
        keywords: "magic magia wizard mage fantasy fantasia",
    },
    IconEntry {
        slug: "castle",
        icon: IconName::Castle,
        keywords: "castelo medieval kingdom metroidvania city",
    },
    IconEntry {
        slug: "skull",
        icon: IconName::Skull,
        keywords: "caveira hardcore dark fantasy gore death roguelike",
    },
    IconEntry {
        slug: "ghost",
        icon: IconName::Ghost,
        keywords: "fantasma horror terror spooky halloween",
    },
    IconEntry {
        slug: "moon-star",
        icon: IconName::MoonStar,
        keywords: "lua noite night anime jrpg atmospheric",
    },
    IconEntry {
        slug: "rocket",
        icon: IconName::Rocket,
        keywords: "foguete scifi space espaco launch",
    },
    IconEntry {
        slug: "orbit",
        icon: IconName::Orbit,
        keywords: "planet planeta cosmic space astronomy galaxy",
    },
    IconEntry {
        slug: "bot",
        icon: IconName::Bot,
        keywords: "robot robo mecha cyberpunk automation ai",
    },
    IconEntry {
        slug: "flask-conical",
        icon: IconName::FlaskConical,
        keywords: "potion pocao alchemy alquimia science ciencia lab",
    },
    // Strategy, puzzle & adventure
    IconEntry {
        slug: "chess-knight",
        icon: IconName::ChessKnight,
        keywords: "xadrez strategy estrategia turnbased rts 4x board tabletop",
    },
    IconEntry {
        slug: "dices",
        icon: IconName::Dices,
        keywords: "dados dice roguelike roguelite deckbuilder card rng tabletop",
    },
    IconEntry {
        slug: "puzzle",
        icon: IconName::Puzzle,
        keywords: "quebra cabeca logic enigma mystery detective",
    },
    IconEntry {
        slug: "brain",
        icon: IconName::Brain,
        keywords: "cerebro psychological trivia mind smart puzzle",
    },
    IconEntry {
        slug: "compass",
        icon: IconName::Compass,
        keywords: "bussola adventure aventura open world exploration",
    },
    IconEntry {
        slug: "map",
        icon: IconName::Map,
        keywords: "mapa cartography world journey quest",
    },
    IconEntry {
        slug: "pickaxe",
        icon: IconName::Pickaxe,
        keywords: "picareta mining survival crafting sandbox base building",
    },
    IconEntry {
        slug: "wrench",
        icon: IconName::Wrench,
        keywords: "chave engenharia engineering factory automation mods",
    },
    IconEntry {
        slug: "key",
        icon: IconName::Key,
        keywords: "chave unlock escape secret metroidvania dungeon",
    },
    IconEntry {
        slug: "eye",
        icon: IconName::Eye,
        keywords: "olho stealth furtivo mystery lovecraft watch",
    },
    // Genres, vehicles & media
    IconEntry {
        slug: "gamepad-2",
        icon: IconName::Gamepad2,
        keywords: "controle controller game platformer indie console",
    },
    IconEntry {
        slug: "joystick",
        icon: IconName::Joystick,
        keywords: "fliperama arcade retro pixel classic fightstick",
    },
    IconEntry {
        slug: "tv",
        icon: IconName::Tv,
        keywords: "television couch console emulator retro big picture",
    },
    IconEntry {
        slug: "car",
        icon: IconName::Car,
        keywords: "carro racing corrida driving simulation drift",
    },
    IconEntry {
        slug: "plane",
        icon: IconName::Plane,
        keywords: "aviao flight voar aviation simulator",
    },
    IconEntry {
        slug: "anchor",
        icon: IconName::Anchor,
        keywords: "ancora pirate pirata ship naval sea ocean",
    },
    IconEntry {
        slug: "music",
        icon: IconName::Music,
        keywords: "musica rhythm ritmo soundtrack ost",
    },
    IconEntry {
        slug: "headphones",
        icon: IconName::Headphones,
        keywords: "fone audio podcast chill immersion",
    },
    IconEntry {
        slug: "book-open",
        icon: IconName::BookOpen,
        keywords: "livro story historia visual novel narrative reading",
    },
    IconEntry {
        slug: "scroll-text",
        icon: IconName::ScrollText,
        keywords: "pergaminho lore crpg quest classic rpg",
    },
    IconEntry {
        slug: "clapperboard",
        icon: IconName::Clapperboard,
        keywords: "cinema movie filme cinematic interactive fmv",
    },
    IconEntry {
        slug: "drama",
        icon: IconName::Drama,
        keywords: "teatro theater masks roleplay persona visual novel",
    },
    // Cozy, nature & creatures
    IconEntry {
        slug: "coffee",
        icon: IconName::Coffee,
        keywords: "cafe cozy aconchegante relaxing chill casual",
    },
    IconEntry {
        slug: "utensils",
        icon: IconName::Utensils,
        keywords: "comida cooking cozinha food restaurant sim",
    },
    IconEntry {
        slug: "trees",
        icon: IconName::Trees,
        keywords: "arvores forest floresta nature natureza hunting outdoor",
    },
    IconEntry {
        slug: "sprout",
        icon: IconName::Sprout,
        keywords: "planta farming fazenda garden vida lifesim cozy",
    },
    IconEntry {
        slug: "flower-2",
        icon: IconName::Flower2,
        keywords: "flor wholesome floral cozy relaxing spring",
    },
    IconEntry {
        slug: "mountain",
        icon: IconName::Mountain,
        keywords: "montanha climb hike wilderness adventure",
    },
    IconEntry {
        slug: "tent",
        icon: IconName::Tent,
        keywords: "barraca camp acampamento outdoor survival",
    },
    IconEntry {
        slug: "snowflake",
        icon: IconName::Snowflake,
        keywords: "neve winter inverno ice gelo frost cold",
    },
    IconEntry {
        slug: "sun",
        icon: IconName::Sun,
        keywords: "sol summer verao bright day light",
    },
    IconEntry {
        slug: "paw-print",
        icon: IconName::PawPrint,
        keywords: "pata animal pet creature collector monster tamer",
    },
    IconEntry {
        slug: "cat",
        icon: IconName::Cat,
        keywords: "gato feline stray pet cozy animal",
    },
    // Social, awards & organization
    IconEntry {
        slug: "users",
        icon: IconName::Users,
        keywords: "amigos friends coop multiplayer mmo party",
    },
    IconEntry {
        slug: "trophy",
        icon: IconName::Trophy,
        keywords: "trofeu sports esportes competitive pvp 100 platinum",
    },
    IconEntry {
        slug: "medal",
        icon: IconName::Medal,
        keywords: "medalha ranked award mastery achievements conquistas",
    },
    IconEntry {
        slug: "crown",
        icon: IconName::Crown,
        keywords: "coroa king top best classics favoritos favoritos",
    },
    IconEntry {
        slug: "gem",
        icon: IconName::Gem,
        keywords: "joia diamante indie hidden gems rare treasure",
    },
    IconEntry {
        slug: "heart",
        icon: IconName::Heart,
        keywords: "coracao love romance favorite favorito dating sim",
    },
    IconEntry {
        slug: "star",
        icon: IconName::Star,
        keywords: "estrela top rated best 5star wishlist",
    },
    IconEntry {
        slug: "bookmark",
        icon: IconName::Bookmark,
        keywords: "marcador backlog play next saved queue fila",
    },
    IconEntry {
        slug: "clock",
        icon: IconName::Clock,
        keywords: "relogio short curto quick session time tempo",
    },
    IconEntry {
        slug: "hourglass",
        icon: IconName::Hourglass,
        keywords: "ampulheta long longo epic waiting backlog",
    },
    IconEntry {
        slug: "circle-check",
        icon: IconName::CircleCheck,
        keywords: "concluido completed beaten zerado done finished",
    },
    IconEntry {
        slug: "flag",
        icon: IconName::Flag,
        keywords: "bandeira milestone campaign goal objetivo",
    },
];

/// Filter [`ICON_CATALOG`] by a free-text search query matching slug or keywords.
pub fn filter_icons(query: &str) -> Vec<&'static IconEntry> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return ICON_CATALOG.iter().collect();
    }
    let terms: Vec<&str> = needle.split_whitespace().collect();
    ICON_CATALOG
        .iter()
        .filter(|entry| {
            terms
                .iter()
                .all(|term| entry.slug.contains(term) || entry.keywords.contains(term))
        })
        .collect()
}

/// Resolve a stored collection icon slug ([`irori_services::smart_categories::ICONS`])
/// into its Lucide [`IconName`], falling back to `fallback` when unset or unknown.
pub fn icon_by_name(name: Option<&str>, fallback: IconName) -> IconName {
    let Some(slug) = name else {
        return fallback;
    };
    if let Some(entry) = ICON_CATALOG.iter().find(|e| e.slug == slug) {
        return entry.icon;
    }
    match slug {
        // Legacy alias kept so older saved collections continue to render.
        "chess-pawn" => IconName::ChessPawn,
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
            "darkfantasy" | "hardcore" | "gore" => Some("skull"),
            "zombie" | "zombies" | "postapocalyptic" => Some("biohazard"),
            "horror" | "survivalhorror" | "psychologicalhorror" => Some("ghost"),
            "anime" | "jrpg" | "atmospheric" => Some("moon-star"),
            "fantasy" | "magic" => Some("wand-sparkles"),
            "fps" | "shooter" | "thirdpersonshooter" | "lootershooter" | "tactical" => {
                Some("crosshair")
            }
            "stealth" | "lovecraftian" => Some("eye"),
            "cyberpunk" | "speedrun" | "fastpaced" => Some("zap"),
            "scifi" | "vr" => Some("rocket"),
            "space" | "astronomy" => Some("orbit"),
            "mecha" | "automation" | "robots" => Some("bot"),
            "factory" | "engineering" => Some("wrench"),
            "alchemy" | "science" => Some("flask-conical"),
            "metroidvania" | "medieval" | "citybuilder" | "grandstrategy" => Some("castle"),
            "towerdefense" | "paladin" | "knight" => Some("shield"),
            "strategy" | "rts" | "4x" | "turnbased" | "management" => Some("chess-knight"),
            "roguelike" | "roguelite" | "deckbuilder" | "cardgame" => Some("dices"),
            "puzzle" | "mystery" | "detective" | "pointclick" => Some("puzzle"),
            "psychological" | "trivia" => Some("brain"),
            "adventure" | "openworld" | "exploration" => Some("compass"),
            "survival" | "crafting" | "basebuilding" | "sandbox" => Some("pickaxe"),
            "cozy" | "relaxing" | "casual" => Some("coffee"),
            "farmingsim" | "lifesim" | "gardening" => Some("sprout"),
            "nature" | "hunting" => Some("trees"),
            "cooking" => Some("utensils"),
            "creaturecollector" | "pets" => Some("paw-print"),
            "romance" | "datingsim" => Some("heart"),
            "visualnovel" | "narrative" | "storyrich" | "choicesmatter" => Some("book-open"),
            "crpg" | "lore" => Some("scroll-text"),
            "cinematic" | "fmv" => Some("clapperboard"),
            "racing" | "driving" | "simulation" => Some("car"),
            "flight" | "aviation" => Some("plane"),
            "pirates" | "pirate" | "naval" | "sailing" => Some("anchor"),
            "sports" | "pvp" | "competitive" | "esports" => Some("trophy"),
            "coop" | "localcoop" | "onlinecoop" | "multiplayer" | "mmo" => Some("users"),
            "rhythm" | "music" | "soundtrack" => Some("music"),
            "action" | "rpg" | "arpg" | "hackandslash" | "beatemup" | "fighting" => Some("swords"),
            "indie" => Some("gem"),
            "retro" | "pixelgraphics" | "arcade" => Some("joystick"),
            "platformer" | "2dplatformer" | "3dplatformer" | "isometric" | "topdown"
            | "singleplayer" | "freetoplay" => Some("gamepad-2"),
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
        assert_eq!(
            ICON_CATALOG.len(),
            irori_services::smart_categories::ICONS.len()
        );
        for slug in irori_services::smart_categories::ICONS {
            let resolved = icon_by_name(Some(slug), IconName::Tag);
            assert_ne!(
                resolved,
                IconName::Tag,
                "slug {slug} must map to a known Lucide IconName"
            );
        }
        for quick in QUICK_ICONS {
            assert!(
                irori_services::smart_categories::ICONS.contains(&quick),
                "quick icon {quick} must be present in ICONS"
            );
        }
        // Legacy alias remains readable for older saved collections.
        assert_eq!(
            icon_by_name(Some("chess-pawn"), IconName::Tag),
            IconName::ChessPawn
        );
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
        assert_eq!(
            suggest_icon_for_terms(&["Strategy".into()]),
            Some("chess-knight")
        );
        assert_eq!(suggest_icon_for_terms(&["Retro".into()]), Some("joystick"));
        assert_eq!(suggest_icon_for_terms(&["Romance".into()]), Some("heart"));
        assert_eq!(suggest_icon_for_terms(&["Unknown Tag".into()]), None);

        // Keyword search matches both English and Portuguese hints.
        assert_eq!(filter_icons("").len(), ICON_CATALOG.len());
        assert!(filter_icons("zumbi").iter().any(|e| e.slug == "biohazard"));
        assert!(filter_icons("gato").iter().any(|e| e.slug == "cat"));
        assert!(filter_icons("backlog").iter().any(|e| e.slug == "bookmark"));
        assert!(filter_icons("xyznonexistent").is_empty());

        // Custom hex colour labels format consistently.
        assert_eq!(colour_label("#ff6b2c").as_ref(), "#FF6B2C");
        assert!(is_custom_hex_colour(Some("#ff6b2c")));
        assert!(!is_custom_hex_colour(Some("teal")));
        assert!(!is_custom_hex_colour(None));
    }
}
