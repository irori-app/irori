//! Library entries and game details built from what Steam knows: installed
//! apps, the account's games, cached and store art, and store pages.

use std::path::{Path, PathBuf};

use irori_core::age_rating;
use irori_core::util::format_bytes;
use irori_core::{
    AgeRating, ExtraInfo, GameInfo, GameMode, HostPlatform, InstallPlatform, InstalledInfo,
    RatingSystem, Requirement, Runner,
};
use irori_steam_api::{AppAssets, AppDetails};

use crate::account::{LibraryGame, RemoteArt};
use crate::scan::{InstalledApp, LocalArt};

/// Fixed art addresses. Only older apps have their art there: newer ones
/// keep it under hashed addresses that Steam's cache or store name.
const STEAM_CDN: &str = "https://cdn.cloudflare.steamstatic.com/steam/apps";

/// A local file as art URL.
fn file_url(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// The four images of an app: Steam's cache first, then the store's
/// addresses, then the fixed ones.
struct Art {
    cover: String,
    hero: String,
    logo: String,
    header: String,
}

impl Art {
    fn of(id: &str, local: Option<&LocalArt>, remote: Option<&RemoteArt>) -> Self {
        let pick = |cached: Option<&PathBuf>, stored: Option<&String>, fixed: &str| {
            cached
                .map(|p| file_url(p))
                .or_else(|| stored.cloned())
                .unwrap_or_else(|| format!("{STEAM_CDN}/{id}/{fixed}"))
        };
        Self {
            cover: pick(
                local.and_then(|a| a.cover.as_ref()),
                remote.and_then(|a| a.cover.as_ref()),
                "library_600x900_2x.jpg",
            ),
            hero: pick(
                local.and_then(|a| a.hero.as_ref()),
                remote.and_then(|a| a.hero.as_ref()),
                "library_hero.jpg",
            ),
            logo: pick(
                local.and_then(|a| a.logo.as_ref()),
                remote.and_then(|a| a.logo.as_ref()),
                "logo.png",
            ),
            header: pick(
                local.and_then(|a| a.header.as_ref()),
                remote.and_then(|a| a.header.as_ref()),
                "header.jpg",
            ),
        }
    }
}

/// Whether Steam's cache lacks the cover of an app, so the store's art is
/// worth asking for.
pub(crate) fn needs_remote_art(local: Option<&LocalArt>) -> bool {
    local.is_none_or(|a| a.cover.is_none())
}

/// The store art worth keeping from Steam's answer.
pub(crate) fn remote_art(assets: &AppAssets) -> RemoteArt {
    RemoteArt {
        cover: assets.cover_2x.clone().or_else(|| assets.cover.clone()),
        hero: assets.hero_2x.clone().or_else(|| assets.hero.clone()),
        logo: assets.logo_2x.clone().or_else(|| assets.logo.clone()),
        header: assets.header.clone(),
    }
}

fn base(id: &str, title: &str, art: Art) -> GameInfo {
    let native = HostPlatform::current().native();
    GameInfo {
        runner: Some(Runner::Steam),
        app_name: id.to_string(),
        title: title.to_string(),
        store_url: Some(format!("https://store.steampowered.com/app/{id}/")),
        art_cover: Some(art.cover),
        art_square: Some(art.header),
        art_background: Some(art.hero),
        art_logo: Some(art.logo),
        can_run_offline: true,
        // Steam runs Windows games through Proton itself, so for Irori every
        // Steam game is native.
        is_linux_native: native == InstallPlatform::Linux,
        is_mac_native: native == InstallPlatform::Mac,
        platforms: vec![native],
        ..Default::default()
    }
}

/// Library entry of an installed Steam app.
pub(crate) fn installed(
    app: &InstalledApp,
    local: Option<&LocalArt>,
    remote: Option<&RemoteArt>,
) -> GameInfo {
    let native = HostPlatform::current().native();
    GameInfo {
        is_installed: true,
        install: Some(InstalledInfo {
            install_path: app.install_path.to_string_lossy().into_owned(),
            install_size: format_bytes(app.size_on_disk),
            install_size_bytes: app.size_on_disk,
            version: app.build_id.clone(),
            platform: native,
            ..Default::default()
        }),
        installable: false,
        ..base(&app.app_id, &app.name, Art::of(&app.app_id, local, remote))
    }
    .finalize()
}

/// Library entry of a game the account owns, or its family shares, that
/// isn't installed. "Install" hands it to Steam.
pub(crate) fn owned(
    game: &LibraryGame,
    local: Option<&LocalArt>,
    remote: Option<&RemoteArt>,
) -> GameInfo {
    GameInfo {
        installable: true,
        ..base(
            &game.app_id,
            &game.name,
            Art::of(&game.app_id, local, remote),
        )
    }
    .finalize()
}

/// Whether two entries show the same thing.
pub(crate) fn same(a: &GameInfo, b: &GameInfo) -> bool {
    a.app_name == b.app_name
        && a.title == b.title
        && a.is_installed == b.is_installed
        && a.store_hidden == b.store_hidden
        && (&a.art_cover, &a.art_background, &a.art_logo, &a.art_square)
            == (&b.art_cover, &b.art_background, &b.art_logo, &b.art_square)
        && a.install
            .as_ref()
            .map(|i| (&i.install_path, &i.version, i.install_size_bytes))
            == b.install
                .as_ref()
                .map(|i| (&i.install_path, &i.version, i.install_size_bytes))
}

/// Steam's name for the store language matching Irori's `uiLanguage`.
pub(crate) fn store_language(ui_language: &str) -> &'static str {
    match ui_language {
        "cs" => "czech",
        "de" => "german",
        "es" => "spanish",
        "fr" => "french",
        "it" => "italian",
        "nl" => "dutch",
        "pl" => "polish",
        "pt" => "portuguese",
        "pt_BR" => "brazilian",
        "ro" => "romanian",
        "sv" => "swedish",
        "tr" => "turkish",
        "ru" => "russian",
        "uk" => "ukrainian",
        "ja" => "japanese",
        "ko" => "koreana",
        "zh_Hans" => "schinese",
        "zh_Hant" => "tchinese",
        _ => "english",
    }
}

/// The game page's details from the store page.
pub(crate) fn extra_info(id: &str, details: &AppDetails) -> ExtraInfo {
    let host = HostPlatform::current().native();
    let mut systems = vec![
        (
            InstallPlatform::Windows,
            "Windows",
            &details.pc_requirements,
        ),
        (InstallPlatform::Linux, "Linux", &details.linux_requirements),
        (InstallPlatform::Mac, "macOS", &details.mac_requirements),
    ];
    // This computer's system first.
    systems.sort_by_key(|(platform, _, _)| *platform != host);
    let requirements = systems
        .into_iter()
        .filter(|(_, _, r)| r.minimum.is_some() || r.recommended.is_some())
        .map(|(_, title, r)| Requirement {
            title: title.to_string(),
            minimum: r.minimum.as_deref().map(plain_text).unwrap_or_default(),
            recommended: r.recommended.as_deref().map(plain_text).unwrap_or_default(),
        })
        .collect();
    let nonempty = |s: &str| (!s.trim().is_empty()).then(|| s.to_string());
    ExtraInfo {
        description: nonempty(details.description()),
        short_description: nonempty(&details.short_description),
        requirements,
        release_date: nonempty(&details.release_date.date),
        store_url: Some(format!("https://store.steampowered.com/app/{id}/")),
        changelog: None,
        genres: details
            .genres
            .iter()
            .map(|g| g.description.clone())
            .filter(|g| !g.is_empty())
            .collect(),
        screenshots: details
            .screenshots
            .iter()
            .map(|s| s.path_full.clone())
            .filter(|p| !p.is_empty())
            .collect(),
        age_ratings: age_ratings(details),
        game_modes: game_modes(details.categories.iter().map(|c| c.id)),
    }
}

/// Game modes from the store page's categories. Steam marks the venue
/// (online, LAN, shared or split screen) and the kind (PvP, co-op)
/// separately; "Multi-player" and "PvP" alone, without a venue, are taken
/// as network play, which they are on the pages that predate the venues.
/// LAN play counts as online: both are network multiplayer.
pub(crate) fn game_modes(categories: impl IntoIterator<Item = u64>) -> Vec<GameMode> {
    let mut modes = Vec::new();
    let mut unplaced = false;
    for id in categories {
        match id {
            2 => modes.push(GameMode::Single),
            // Multi-player, PvP.
            1 | 49 => unplaced = true,
            // Online PvP, LAN PvP.
            36 | 47 => modes.push(GameMode::Online),
            // Shared/split screen PvP, shared/split screen.
            37 | 24 => modes.push(GameMode::Local),
            9 => modes.push(GameMode::Coop),
            // Online co-op, LAN co-op.
            38 | 48 => modes.extend([GameMode::Coop, GameMode::Online]),
            // Shared/split screen co-op.
            39 => modes.extend([GameMode::Coop, GameMode::Local]),
            20 => modes.extend([GameMode::Mmo, GameMode::Online]),
            _ => {}
        }
    }
    if unplaced
        && !modes
            .iter()
            .any(|m| matches!(m, GameMode::Local | GameMode::Online))
    {
        modes.push(GameMode::Online);
    }
    irori_core::game_mode::normalized(modes)
}

/// The store page's ratings by board. Steam's `steam_germany` entry
/// repeats the USK's verdict for Germany's store and is left out, as are
/// boards Irori doesn't know.
pub(crate) fn age_ratings(details: &AppDetails) -> Vec<AgeRating> {
    details
        .ratings
        .iter()
        .filter_map(|(board, r)| {
            let system = RatingSystem::from_key(board)?;
            let age = u8::try_from(r.required_age).ok().filter(|a| *a > 0);
            AgeRating::new(
                system,
                &r.rating,
                age_rating::split_descriptors(&r.descriptors, false),
                age,
            )
        })
        .collect()
}

/// Text of Steam's requirement HTML, one item per line, with the
/// "Minimum:" heading Steam repeats in every block left out.
fn plain_text(html: &str) -> String {
    let mut out = String::new();
    let mut tag = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match (in_tag, c) {
            (false, '<') => {
                in_tag = true;
                tag.clear();
            }
            (true, '>') => {
                in_tag = false;
                let name = tag
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if matches!(name.as_str(), "br" | "li" | "p" | "ul") && !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            (true, c) => tag.push(c),
            (false, c) => out.push(c),
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| !matches!(*l, "Minimum:" | "Recommended:"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_store_page_categories_to_game_modes() {
        // Portal 2's categories, as its store page lists them.
        let portal2: AppDetails = serde_json::from_value(serde_json::json!({
            "categories": [
                {"id": 2, "description": "Single-player"}, {"id": 1, "description": "Multi-player"},
                {"id": 9, "description": "Co-op"}, {"id": 38, "description": "Online Co-op"},
                {"id": 39, "description": "Shared/Split Screen Co-op"},
                {"id": 24, "description": "Shared/Split Screen"},
                {"id": 22, "description": "Steam Achievements"}, {"id": 23, "description": "Steam Cloud"},
                {"id": 44, "description": "Remote Play Together"}
            ]
        }))
        .unwrap();
        assert_eq!(
            extra_info("620", &portal2).game_modes,
            [
                GameMode::Single,
                GameMode::Coop,
                GameMode::Local,
                GameMode::Online
            ]
        );
        // An older page: "Multi-player" alone is network play.
        assert_eq!(game_modes([2, 1]), [GameMode::Single, GameMode::Online]);
        // Split screen PvP only: no online play is claimed.
        assert_eq!(game_modes([1, 49, 37]), [GameMode::Local]);
        assert_eq!(game_modes([20]), [GameMode::Online, GameMode::Mmo]);
        assert_eq!(game_modes([47, 48]), [GameMode::Coop, GameMode::Online]);
        assert!(game_modes([22, 23, 44]).is_empty());
    }

    #[test]
    fn maps_store_page_ratings() {
        let details: AppDetails = serde_json::from_value(serde_json::json!({
            "name": "Rated",
            "ratings": {
                "esrb": {"rating": "e10", "descriptors": "Fantasy Violence\r\nMild Blood",
                         "use_age_gate": "true", "required_age": "10"},
                "pegi": {"rating": "7", "descriptors": "Violence"},
                "cero": {"rating": "b"},
                "dejus": {"rating": "l"},
                "oflc": {"rating": "pg"},
                "steam_germany": {"rating_generated": "1", "rating": "6", "required_age": "6",
                                  "banned": "0", "use_age_gate": "0", "descriptors": ""},
                "unknownboard": {"rating": "x"}
            }
        }))
        .unwrap();
        let ratings = age_ratings(&details);
        let titles: Vec<String> = ratings.iter().map(AgeRating::title).collect();
        assert_eq!(
            titles,
            ["CERO B", "ClassInd L", "ESRB E10+", "ACB PG", "PEGI 7"]
        );
        let esrb = &ratings[2];
        assert_eq!(esrb.age, Some(10));
        assert_eq!(esrb.descriptors, ["Fantasy Violence", "Mild Blood"]);
        assert_eq!(extra_info("1", &details).age_ratings, ratings);
        // Pages without ratings have none.
        let bare: AppDetails =
            serde_json::from_value(serde_json::json!({"ratings": null})).unwrap();
        assert!(age_ratings(&bare).is_empty());
    }

    #[test]
    fn art_prefers_the_cache_then_the_store() {
        let local = LocalArt {
            cover: Some(PathBuf::from(
                "/steam/appcache/librarycache/7/a/library_capsule.jpg",
            )),
            ..Default::default()
        };
        let remote = RemoteArt {
            cover: Some("https://store/cover.jpg".into()),
            hero: Some("https://store/hero.jpg".into()),
            ..Default::default()
        };
        let art = Art::of("7", Some(&local), Some(&remote));
        assert_eq!(
            art.cover,
            "file:///steam/appcache/librarycache/7/a/library_capsule.jpg"
        );
        assert_eq!(art.hero, "https://store/hero.jpg");
        assert!(art.logo.ends_with("/7/logo.png"));
        assert!(needs_remote_art(None));
        assert!(!needs_remote_art(Some(&local)));
    }

    #[test]
    fn owned_games_are_installable_through_steam() {
        let game = LibraryGame {
            app_id: "70".into(),
            name: "Half-Life".into(),
            ..Default::default()
        };
        let info = owned(&game, None, None);
        assert!(!info.is_installed && info.installable);
        assert_eq!(info.runner(), Runner::Steam);
    }

    #[test]
    fn requirements_become_plain_lines() {
        let html = "<strong>Minimum:</strong><br><ul class=\"bb_ul\"><li><strong>OS:</strong> Windows 10<br></li><li><strong>Memory:</strong> 8 GB RAM</li></ul>";
        assert_eq!(plain_text(html), "OS: Windows 10\nMemory: 8 GB RAM");
        assert_eq!(plain_text("A &amp; B"), "A & B");
    }

    #[test]
    fn maps_irori_languages_to_steam_names() {
        assert_eq!(store_language("pt_BR"), "brazilian");
        assert_eq!(store_language("zh_Hans"), "schinese");
        assert_eq!(store_language("en"), "english");
    }
}
