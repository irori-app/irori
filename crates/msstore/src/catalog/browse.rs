//! Public Store browsing: search (`apps.microsoft.com/api/products/search`),
//! the Store's computed game lists (Top paid, Top trending) and the PC Game
//! Pass collections of `catalog.gamepass.com`. Cards only name products; the
//! display catalog has their details. Nothing here proves access.

use serde_json::Value;

use super::{TIMEOUT, normalize_market};
use crate::http::{self, Result, StoreApi, StoreError, is_product_id};

/// Bytes one search or list page may take.
const MAX_PAGE: usize = 4_000_000;
/// Bytes a Game Pass collection may take.
const MAX_COLLECTION: usize = 256 * 1024;
/// Items one computed list page asks for.
const LIST_ITEMS: u32 = 48;

/// One Store result: enough to draw a tile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreCard {
    pub product_id: String,
    pub title: String,
    pub publisher: Option<String>,
    /// A Store image (`store-images.s-microsoft.com`) for the tile.
    pub image: Option<String>,
    /// The Store's price text ("$59.99", "Free"), when it shows one.
    pub price: Option<String>,
    /// Whether the Store marks it as included in Game Pass.
    pub in_game_pass: bool,
}

/// A page of results and the cursor of the next one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StoreSearch {
    pub cards: Vec<StoreCard>,
    /// Pass back to get the next page; `None` at the end.
    pub cursor: Option<String>,
}

/// Which public Game Pass collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GamePassList {
    /// Every PC Game Pass game.
    AllPc,
    /// PC games recently added to Game Pass, newest first.
    RecentlyAddedPc,
}

impl GamePassList {
    fn id(self) -> &'static str {
        match self {
            GamePassList::AllPc => "fdd9e2a7-0fee-49f6-ad69-4354098401ff",
            GamePassList::RecentlyAddedPc => "06323672-b8c8-43cc-b0de-32d5a9834749",
        }
    }
}

/// Only Store image hosts are drawn; anything else in a result is dropped.
fn store_image(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    (parsed.scheme() == "https"
        && matches!(
            parsed.host_str(),
            Some("store-images.s-microsoft.com" | "store-images.microsoft.com")
        ))
    .then(|| url.to_string())
}

fn text(value: &Value, key: &str) -> Option<String> {
    value[key]
        .as_str()
        .map(|s| {
            s.chars()
                .filter(|c| !c.is_control())
                .take(240)
                .collect::<String>()
        })
        .filter(|s| !s.trim().is_empty())
}

/// The Store web API's product rows as cards; rows without a valid product
/// ID or title are left out, and each product appears once.
pub fn parse_cards(body: &Value) -> Vec<StoreCard> {
    let mut seen = std::collections::HashSet::new();
    body["productsList"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|row| {
            // The Store web API writes IDs in lower case.
            let product_id = row["productId"]
                .as_str()?
                .to_ascii_uppercase()
                .chars()
                .collect::<String>();
            if !is_product_id(&product_id)
                || row["isGame"].as_bool() == Some(false)
                || !seen.insert(product_id.clone())
            {
                return None;
            }
            let images = row["images"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default();
            let of_type = |kind: &str| {
                images
                    .iter()
                    .filter(|i| i["imageType"].as_str() == Some(kind))
                    .filter_map(|i| i["url"].as_str())
                    .find_map(store_image)
            };
            let image = ["posterArtUrl", "boxArtUrl"]
                .iter()
                .filter_map(|k| row[*k].as_str())
                .find_map(store_image)
                .or_else(|| of_type("Poster"))
                .or_else(|| of_type("BoxArt"))
                .or_else(|| {
                    images
                        .iter()
                        .filter_map(|i| i["url"].as_str())
                        .find_map(store_image)
                });
            let price = row["priceInfo"]["price"]
                .as_str()
                .or_else(|| row["displayPrice"].as_str())
                .map(str::to_string)
                .filter(|p| !p.is_empty() && p.len() < 40);
            Some(StoreCard {
                product_id,
                title: text(row, "title")?,
                publisher: text(row, "publisherName"),
                image,
                price,
                in_game_pass: row["plaintextPassName"]
                    .as_str()
                    .is_some_and(|p| p.to_ascii_lowercase().contains("game pass")),
            })
        })
        .collect()
}

/// Product IDs of a Game Pass collection (`sigls/v3`): metadata first, then
/// one row per product. Any other shape is refused rather than guessed.
pub fn parse_collection(body: &Value, list: GamePassList) -> Result<Vec<String>> {
    let rows = body
        .as_array()
        .ok_or(StoreError::Malformed("gamepass-shape"))?;
    let (meta, items) = rows
        .split_first()
        .ok_or(StoreError::Malformed("gamepass-empty"))?;
    if meta["siglId"].as_str() != Some(list.id()) || items.len() > 2000 {
        return Err(StoreError::Malformed("gamepass-collection"));
    }
    let mut ids = Vec::with_capacity(items.len());
    for item in items {
        let id = item["id"]
            .as_str()
            .filter(|id| is_product_id(id))
            .ok_or(StoreError::Malformed("gamepass-id"))?;
        if ids.iter().any(|known| known == id) {
            return Err(StoreError::Malformed("gamepass-duplicate"));
        }
        ids.push(id.to_string());
    }
    Ok(ids)
}

fn cursor_ok(cursor: &str) -> bool {
    cursor.len() <= 4096
        && cursor
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+/=_-".contains(&b))
}

impl StoreApi {
    /// Search the Store's games. `query` is 2 to 100 characters; `cursor`
    /// continues a previous search. Anonymous.
    pub async fn search(
        &self,
        query: &str,
        market: &str,
        cursor: Option<&str>,
    ) -> Result<StoreSearch> {
        let query = query.trim();
        if !(2..=100).contains(&query.chars().count()) {
            return Err(StoreError::Invalid("search text"));
        }
        if cursor.is_some_and(|c| !cursor_ok(c)) {
            return Err(StoreError::Invalid("search cursor"));
        }
        let market = normalize_market(market);
        let mut params = vec![
            ("query", query),
            ("mediaType", "games"),
            ("age", "all"),
            ("price", "all"),
            ("category", "all"),
            ("subscription", "all"),
            ("gl", market.as_str()),
            ("hl", "en-us"),
        ];
        if let Some(cursor) = cursor {
            params.push(("cursor", cursor));
        }
        let url = format!("{}/api/products/search", self.endpoints.store_web);
        let request = self
            .client
            .get(url)
            .query(&params)
            .header("Accept", "application/json");
        let body: Value =
            http::json_response(http::send(request, TIMEOUT).await?, MAX_PAGE).await?;
        let cursor = (body["hasMorePages"].as_bool() == Some(true))
            .then(|| {
                body["cursor"]
                    .as_str()
                    .filter(|c| cursor_ok(c))
                    .map(str::to_string)
            })
            .flatten();
        Ok(StoreSearch {
            cards: parse_cards(&body),
            cursor,
        })
    }

    /// One page (from 1) of a Store computed list such as `TopPaid` or
    /// `TopTrending`, and whether a next page exists. Anonymous.
    pub async fn computed_list(
        &self,
        list: &str,
        market: &str,
        page: u32,
    ) -> Result<(Vec<StoreCard>, bool)> {
        if !list.bytes().all(|b| b.is_ascii_alphanumeric()) || !(1..=20).contains(&page) {
            return Err(StoreError::Invalid("list"));
        }
        let market = normalize_market(market);
        let (page_text, items) = (page.to_string(), LIST_ITEMS.to_string());
        let url = format!(
            "{}/api/Reco/GetComputedProductsList",
            self.endpoints.store_web
        );
        let request = self
            .client
            .get(url)
            .query(&[
                ("listName", list),
                ("pgNo", page_text.as_str()),
                ("noItems", items.as_str()),
                ("filteredCategories", "AllProducts"),
                ("mediaType", "Games"),
                ("gl", market.as_str()),
                ("hl", "en-us"),
                ("exp", "0"),
            ])
            .header("Accept", "application/json");
        let body: Value =
            http::json_response(http::send(request, TIMEOUT).await?, MAX_PAGE).await?;
        // A next page exists only when the feed names exactly the next one.
        let more = body["hasMorePages"].as_bool() == Some(true)
            && body["nextPageNumber"].as_u64() == Some(u64::from(page) + 1);
        Ok((parse_cards(&body), more))
    }

    /// Product IDs of a PC Game Pass collection, in its order. Anonymous.
    pub async fn game_pass(&self, list: GamePassList, market: &str) -> Result<Vec<String>> {
        let market = normalize_market(market);
        let url = format!("{}/sigls/v3", self.endpoints.gamepass);
        let request = self
            .client
            .get(url)
            .query(&[
                ("id", list.id()),
                ("language", "en-US"),
                ("market", market.as_str()),
                ("platformContext", "pc"),
                ("subscriptionContext", "cfq7ttc0kgq8"),
            ])
            .header("Accept", "application/json");
        let body: Value =
            http::json_response(http::send(request, TIMEOUT).await?, MAX_COLLECTION).await?;
        parse_collection(&body, list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cards_keep_games_with_store_images_once() {
        let body = json!({"productsList": [
            {"productId": "9nblggh4r315", "title": "Halo", "publisherName": "Xbox Game Studios",
             "images": [{"imageType": "Logo", "url": "https://store-images.s-microsoft.com/image/apps.0"},
                        {"imageType": "Poster", "url": "https://store-images.s-microsoft.com/image/apps.1"}],
             "plaintextPassName": "PC Game Pass", "displayPrice": "$59.99"},
            {"productId": "9NBLGGH4R315", "title": "Halo again"},
            {"productId": "bad", "title": "No id"},
            {"productId": "9WZDNCRFJ3TJ", "title": "An app", "isGame": false},
            {"productId": "9P3J32CTXLRZ", "title": "Elsewhere", "posterArtUrl": "https://evil.test/a.png"}
        ]});
        let cards = parse_cards(&body);
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].title, "Halo");
        assert!(cards[0].in_game_pass);
        assert_eq!(cards[0].price.as_deref(), Some("$59.99"));
        assert_eq!(cards[0].product_id, "9NBLGGH4R315");
        assert_eq!(
            cards[0].image.as_deref(),
            Some("https://store-images.s-microsoft.com/image/apps.1")
        );
        assert_eq!(cards[1].image, None);
    }

    #[test]
    fn collections_need_their_own_id_and_unique_products() {
        let list = GamePassList::RecentlyAddedPc;
        let good = json!([{"siglId": list.id()}, {"id": "9NBLGGH4R315"}, {"id": "9P3J32CTXLRZ"}]);
        assert_eq!(
            parse_collection(&good, list).unwrap(),
            ["9NBLGGH4R315", "9P3J32CTXLRZ"]
        );
        for bad in [
            json!([{"siglId": "other"}, {"id": "9NBLGGH4R315"}]),
            json!([{"siglId": list.id()}, {"id": "9NBLGGH4R315"}, {"id": "9NBLGGH4R315"}]),
            json!([{"siglId": list.id()}, {"id": "lower"}]),
            json!([]),
            json!({}),
        ] {
            assert!(parse_collection(&bad, list).is_err(), "{bad}");
        }
    }

    #[test]
    fn search_cursor_is_bounded_to_safe_text() {
        assert!(cursor_ok("abc+/=_-"));
        assert!(!cursor_ok("a&b"));
        assert!(!cursor_ok(&"a".repeat(5000)));
    }
}
