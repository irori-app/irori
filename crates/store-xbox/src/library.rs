//! The account's games: its Store entitlements (`ops::library`), each resolved
//! to how it is held (bought, or through a Game Pass plan), described by the
//! display catalog. Ported from Nexus Launcher's account library.

use std::collections::{BTreeMap, HashMap, HashSet};

use irori_core::{AgeRating, GameInfo, InstallPlatform, RatingSystem, Runner};
use irori_msstore_api::catalog::Product;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Store product IDs of the PC and Ultimate Game Pass plans.
const PLANS: &[&str] = &[
    "CFQ7TTC0KGQ8",
    "CFQ7TTC0KHS0",
    "CFQ7TTC0P85B",
    "CFQ7TTC0K5DJ",
    "CFQ7TTC0K6L8",
    "CFQ7TTC10QFD",
];

/// Plans whose members may play every PC Game Pass game: PC Game Pass and
/// Game Pass Ultimate.
const PC_PLANS: &[&str] = &["CFQ7TTC0KGQ8", "CFQ7TTC0KHS0"];

/// Whether a membership answer (`ops::subscription::lookup`) is an active
/// plan that includes PC games.
pub fn has_pc_game_pass(membership: &Value) -> bool {
    membership["status"] == "active"
        && membership["productIds"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|id| id.as_str().is_some_and(|id| PC_PLANS.contains(&id)))
}

/// How the account holds a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    Owned,
    GamePass,
}

/// One entitlement row as `ops::library::entitlements` returns it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entitlement {
    product_id: String,
    status: String,
    is_trial: bool,
    sharing_source: String,
    end_date: String,
    #[serde(default)]
    satisfied_by_product_id: Option<String>,
    special: bool,
    #[serde(default)]
    is_positron: bool,
}

fn product_id(id: &str) -> bool {
    id.len() == 12
        && id
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// The products of an entitlements answer the account may play, with how.
/// A row that is not active, a trial, shared, expired or otherwise special
/// (other than Microsoft's conditional "Positron" access) grants nothing, and
/// neither does any row that depends on it.
pub fn resolve(answer: &Value, now: i64) -> BTreeMap<String, Access> {
    let rows: Vec<Entitlement> = answer["entitlements"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| serde_json::from_value(row.clone()).ok())
        .collect();
    let entries: HashMap<&str, &Entitlement> = rows
        .iter()
        .filter(|row| product_id(&row.product_id))
        .map(|row| (row.product_id.as_str(), row))
        .collect();
    entries
        .keys()
        .filter_map(|id| resolve_one(id, &entries, now).map(|kind| (id.to_string(), kind)))
        .collect()
}

fn resolve_one(id: &str, entries: &HashMap<&str, &Entitlement>, now: i64) -> Option<Access> {
    let mut visited = HashSet::new();
    let (mut current, mut subscription, mut conditional) = (id, false, false);
    for depth in 0..16 {
        if !visited.insert(current) {
            return None;
        }
        subscription |= PLANS.contains(&current);
        let Some(item) = entries.get(current) else {
            // An absent source row: owned only with conditional evidence,
            // a plan's game when the chain went through a plan.
            return match depth {
                0 => None,
                _ if conditional => Some(Access::Owned),
                _ if subscription => Some(Access::GamePass),
                _ => None,
            };
        };
        if item.special && !item.is_positron {
            return None;
        }
        let expired = chrono_seconds(&item.end_date).is_none_or(|end| end <= now);
        if item.status != "Active" || item.is_trial || item.sharing_source != "None" || expired {
            return None;
        }
        conditional |= item.is_positron;
        let Some(parent) = item
            .satisfied_by_product_id
            .as_deref()
            .filter(|p| product_id(p))
        else {
            // The plan itself is not a game.
            if current == id && PLANS.contains(&current) {
                return None;
            }
            return Some(if subscription && !conditional {
                Access::GamePass
            } else {
                Access::Owned
            });
        };
        current = parent;
    }
    None
}

fn chrono_seconds(text: &str) -> Option<i64> {
    time::OffsetDateTime::parse(text.trim(), &time::format_description::well_known::Rfc3339)
        .ok()
        .map(time::OffsetDateTime::unix_timestamp)
}

/// A library entry kept between runs: what the catalog said, and the access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub product: Product,
    pub access: Access,
}

/// The art Irori draws for a product, by the catalog's image purposes.
fn art(product: &Product, purposes: &[&str]) -> Option<String> {
    product.image(purposes).map(str::to_string)
}

/// Whether a product is a game Irori lists (not a plan, app or consumable).
pub fn is_listed(product: &Product) -> bool {
    matches!(
        product.product_type.as_str(),
        "Game" | "Durable" | "GameAddOn" | "Bundle"
    )
}

/// Whether a product is an add-on of another game.
pub fn parent_of(product: &Product) -> Option<&str> {
    if product.product_type == "Game" || product.product_type == "Bundle" {
        return None;
    }
    product
        .related
        .iter()
        .find(|r| r.kind == "addOnParent" && product_id(&r.product_id))
        .map(|r| r.product_id.as_str())
}

/// Whether the catalog offers a PC package Irori can install.
pub fn installable(product: &Product) -> bool {
    product
        .skus
        .iter()
        .filter(|s| s.is_retail())
        .flat_map(|s| s.packages.iter())
        .any(|p| p.is_pc_msixvc())
        || product.skus.iter().any(|s| !s.bundled.is_empty())
}

/// A library entry as Irori shows it.
pub fn game_info(entry: &Entry) -> GameInfo {
    let product = &entry.product;
    let ratings = product
        .age_ratings
        .iter()
        .filter_map(|r| {
            let system = RatingSystem::from_key(&r.system)?;
            let label = r
                .rating
                .split_once(':')
                .map_or(r.rating.as_str(), |(_, l)| l);
            AgeRating::new(system, label, Vec::new(), None)
        })
        .collect();
    let is_dlc = parent_of(product).is_some();
    GameInfo {
        runner: Some(Runner::Xbox),
        app_name: product.product_id.clone(),
        title: if product.title.is_empty() {
            product.product_id.clone()
        } else {
            product.title.clone()
        },
        developer: product.developer.clone(),
        publisher: product.publisher.clone(),
        description: (!product.description.is_empty()).then(|| product.description.clone()),
        store_url: Some(format!(
            "https://www.xbox.com/games/store/p/{}",
            product.product_id
        )),
        art_cover: art(product, &["Poster", "BoxArt"]),
        art_square: art(product, &["BoxArt", "Tile", "Poster"]),
        art_logo: art(product, &["Logo"]),
        art_background: art(
            product,
            &["SuperHeroArt", "TitledHeroArt", "Hero", "Screenshot"],
        ),
        art_icon: art(product, &["Logo", "Tile"]),
        installable: installable(product),
        platforms: vec![InstallPlatform::Windows],
        is_dlc,
        genres: product.categories.clone(),
        release_date: product.release_date.clone(),
        age_ratings: ratings,
        store_tags: match entry.access {
            Access::GamePass => vec!["Game Pass".to_string()],
            Access::Owned => Vec::new(),
        },
        ..Default::default()
    }
    .finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(id: &str, parent: Option<&str>) -> Value {
        json!({"productId": id, "status": "Active", "isTrial": false, "sharingSource": "None",
               "endDate": "2999-01-01T00:00:00Z", "satisfiedByProductId": parent,
               "special": false, "isPositron": false})
    }

    #[test]
    fn owned_game_pass_and_refused_rows() {
        let mut expired = row("9EXPIRED0000", None);
        expired["endDate"] = json!("2000-01-01T00:00:00Z");
        let mut trial = row("9TRIAL000000", None);
        trial["isTrial"] = json!(true);
        let mut special = row("9SPECIAL0000", None);
        special["special"] = json!(true);
        let answer = json!({"entitlements": [
            row("9OWNED000000", None),
            row("CFQ7TTC0KGQ8", None),
            row("9PASSGAME000", Some("CFQ7TTC0KGQ8")),
            expired,
            row("9CHILDOFEXP0", Some("9EXPIRED0000")),
            trial,
            special,
            row("9ORPHAN00000", Some("9UNKNOWN0000")),
        ]});
        let resolved = resolve(&answer, 1_800_000_000);
        assert_eq!(resolved.get("9OWNED000000"), Some(&Access::Owned));
        assert_eq!(resolved.get("9PASSGAME000"), Some(&Access::GamePass));
        for none in [
            "CFQ7TTC0KGQ8",
            "9EXPIRED0000",
            "9CHILDOFEXP0",
            "9TRIAL000000",
            "9SPECIAL0000",
            "9ORPHAN00000",
        ] {
            assert!(!resolved.contains_key(none), "{none}");
        }
    }

    #[test]
    fn only_active_pc_plans_open_the_catalog() {
        assert!(has_pc_game_pass(
            &json!({"status": "active", "productIds": ["CFQ7TTC0KHS0"]})
        ));
        assert!(!has_pc_game_pass(
            &json!({"status": "active", "productIds": ["CFQ7TTC0K5DJ"]})
        ));
        assert!(!has_pc_game_pass(
            &json!({"status": "unknown", "productIds": ["CFQ7TTC0KGQ8"]})
        ));
        assert!(!has_pc_game_pass(&json!({})));
    }

    #[test]
    fn cycles_grant_nothing() {
        let answer = json!({"entitlements": [row("9AAAAAAAAAAA", Some("9BBBBBBBBBBB")), row("9BBBBBBBBBBB", Some("9AAAAAAAAAAA"))]});
        assert!(resolve(&answer, 0).is_empty());
    }
}
