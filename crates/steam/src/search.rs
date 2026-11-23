//! The store's search by name (`/api/storesearch`, keyless), and reading an
//! app id out of what a user pastes: the id itself, or an address of the
//! store, the community, SteamDB or a `steam://` link.

use anyhow::Result;
use serde::Deserialize;

use crate::api::Client;
use crate::library::lenient_u64;

/// One app the store's search found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreHit {
    pub id: u32,
    pub name: String,
    /// A small capsule picture, when the store sent one.
    pub image: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawHit {
    #[serde(rename = "type")]
    kind: String,
    name: String,
    #[serde(deserialize_with = "lenient_u64")]
    id: u64,
    tiny_image: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Answer {
    items: Vec<RawHit>,
}

fn hits(answer: Answer) -> Vec<StoreHit> {
    answer
        .items
        .into_iter()
        // Bundles and packages share the list; only apps are games.
        .filter(|h| h.kind.is_empty() || h.kind == "app")
        .filter_map(|h| {
            let id = u32::try_from(h.id).ok().filter(|&id| id != 0)?;
            let name = h.name.trim().to_owned();
            (!name.is_empty()).then(|| StoreHit {
                id,
                name,
                image: Some(h.tiny_image).filter(|i| i.starts_with("https://")),
            })
        })
        .collect()
}

impl Client {
    /// The store's apps named like `term`, best first, with names in Steam's
    /// `language` (`english`, `german`…).
    pub async fn store_search(&self, term: &str, language: &str) -> Result<Vec<StoreHit>> {
        let term = term.trim();
        if term.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/api/storesearch/", self.endpoints().store);
        let answer: Answer = self
            .get_json(
                url,
                &[
                    ("term", term.to_owned()),
                    ("l", language.to_owned()),
                    ("cc", "US".to_owned()),
                ],
                "the store search",
            )
            .await?;
        Ok(hits(answer))
    }
}

/// The app id in `text`: a bare id, or an address naming one
/// (`store.steampowered.com/app/620/Portal_2/`, `steamcommunity.com/app/620`,
/// `steamdb.info/app/620/`, `steam://store/620`, `steam://rungameid/620`).
pub fn app_id_in(text: &str) -> Option<u32> {
    let text = text.trim();
    let positive = |s: &str| s.parse::<u32>().ok().filter(|&id| id != 0);
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        return positive(text);
    }
    let lower = text.to_ascii_lowercase();
    let rest = ["steam://store/", "steam://rungameid/", "steam://run/"]
        .iter()
        .find_map(|p| lower.strip_prefix(p))
        .or_else(|| {
            let address = lower
                .strip_prefix("https://")
                .or_else(|| lower.strip_prefix("http://"))
                .unwrap_or(&lower);
            let (host, path) = address.split_once('/')?;
            let host = host.strip_prefix("www.").unwrap_or(host);
            matches!(
                host,
                "store.steampowered.com" | "steamcommunity.com" | "steamdb.info"
            )
            .then_some(path)?
            .strip_prefix("app/")
        })?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    positive(&digits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Endpoints;
    use irori_depot::test_server::{Resp, TestServer};
    use serde_json::json;

    #[test]
    fn app_ids_from_what_users_paste() {
        assert_eq!(app_id_in(" 620 "), Some(620));
        assert_eq!(
            app_id_in("https://store.steampowered.com/app/620/Portal_2/"),
            Some(620)
        );
        assert_eq!(
            app_id_in("store.steampowered.com/app/1091500"),
            Some(1091500)
        );
        assert_eq!(
            app_id_in("https://steamcommunity.com/app/620/discussions/"),
            Some(620)
        );
        assert_eq!(app_id_in("https://steamdb.info/app/570/"), Some(570));
        assert_eq!(app_id_in("steam://rungameid/620"), Some(620));
        assert_eq!(app_id_in("steam://store/620"), Some(620));
        assert_eq!(app_id_in("0"), None);
        assert_eq!(app_id_in("Portal 2"), None);
        assert_eq!(app_id_in("https://example.com/app/620"), None);
        assert_eq!(app_id_in("https://store.steampowered.com/sub/620"), None);
        assert_eq!(app_id_in("99999999999"), None);
    }

    #[tokio::test]
    async fn searches_the_store_for_apps() {
        let server = TestServer::start(|req| {
            if req.path.starts_with("/api/storesearch/?") {
                assert!(req.path.contains("term=Portal+2") || req.path.contains("term=Portal%202"));
                assert!(req.path.contains("l=german"));
                Resp::json(json!({"total": 4, "items": [
                    {"type": "app", "name": "Portal 2", "id": 620,
                     "tiny_image": "https://cdn.example/620.jpg"},
                    {"type": "sub", "name": "Portal Bundle", "id": 7932},
                    {"type": "app", "name": " ", "id": 5},
                    {"type": "app", "name": "Portal", "id": "400", "tiny_image": "file:///x"}
                ]}))
            } else {
                Resp::status(404)
            }
        });
        let api = Client::with_endpoints(Endpoints {
            api: server.base(),
            store: server.base(),
            assets: server.base(),
        })
        .unwrap();
        let found = api.store_search("Portal 2", "german").await.unwrap();
        assert_eq!(
            found,
            [
                StoreHit {
                    id: 620,
                    name: "Portal 2".into(),
                    image: Some("https://cdn.example/620.jpg".into()),
                },
                StoreHit {
                    id: 400,
                    name: "Portal".into(),
                    image: None,
                },
            ]
        );
        assert!(api.store_search("  ", "english").await.unwrap().is_empty());
    }
}
