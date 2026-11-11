//! Microsoft Store display catalog (`displaycatalog.mp.microsoft.com`):
//! public product metadata (titles, descriptions, artwork, platforms,
//! packages, bundles and add-on relations, Xbox title ID, age ratings), Store
//! search and browsing, and the add-on (DLC) graph of a game.
//!
//! Everything here is anonymous public metadata: no token, and nothing in it
//! proves ownership or grants a download.

mod browse;

pub use browse::{GamePassList, StoreCard, StoreSearch};

use std::collections::BTreeSet;
use std::time::Duration;

use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::http::{self, Result, StoreApi, StoreError, is_product_id};

/// Public Store browsing (search, featured, collections) uses one fixed market
/// so a page never inherits a country from the OS locale. Account operations
/// (library, licences, installs) keep the account's market.
pub const PUBLIC_STORE_MARKET: &str = "US";

/// Display-catalog languages every request asks for.
const LANGUAGES: &str = "en-us,neutral";
/// Products per `bigIds` request.
const BATCH: usize = 20;
/// Parallel display-catalog requests.
const PARALLEL: usize = 4;
/// Body limit of one display-catalog response.
const MAX_BODY: usize = 8_000_000;
/// IDs one `products` call may ask for.
const MAX_IDS: usize = 1_000;
const TIMEOUT: Duration = Duration::from_secs(20);
const IMAGE_HOSTS: &[&str] = &["store-images.s-microsoft.com", "store-images.microsoft.com"];

/// A product as the display catalog describes it, reduced to the fields Irori
/// acts on. Malformed fields are dropped, never guessed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Product {
    pub product_id: String,
    /// `Game`, `Durable`, `GameAddOn`, `Bundle`, `Consumable`, ...
    pub product_type: String,
    /// Localized title; empty when the catalog gave none.
    pub title: String,
    pub description: String,
    pub publisher: Option<String>,
    pub developer: Option<String>,
    pub images: Vec<Image>,
    /// The single `XboxTitleId` alternate ID (decimal), when unambiguous.
    pub xbox_title_id: Option<String>,
    /// Editions and resale variants of one game share this group.
    pub product_group_id: Option<String>,
    pub categories: Vec<String>,
    pub release_date: Option<String>,
    pub age_ratings: Vec<AgeRating>,
    /// `MarketProperties[].RelatedProducts[]`.
    pub related: Vec<Relation>,
    pub skus: Vec<Sku>,
    pub is_demo: Option<bool>,
    pub is_trial: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Image {
    /// `BoxArt`, `Poster`, `SuperHeroArt`, `Screenshot`, `Logo`, ...
    pub purpose: String,
    /// An `https` URL on Microsoft's image hosts.
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgeRating {
    /// `ESRB`, `PEGI`, `USK`, ...
    pub system: String,
    /// `ESRB:M`, `PEGI:18`, ...
    pub rating: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Relation {
    /// As the catalog wrote it; check with [`is_product_id`] before use.
    pub product_id: String,
    /// `addOnParent`, `Bundle`, `Parent`, ...
    pub kind: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Sku {
    pub sku_id: String,
    /// `full`, `trial`, `demo`, ... (lower-case)
    pub sku_type: Option<String>,
    pub is_trial: Option<bool>,
    pub is_pre_order: Option<bool>,
    pub is_demo: Option<bool>,
    /// Whether the SKU declares a `Packages` list at all.
    pub packages_declared: bool,
    pub packages: Vec<Package>,
    pub bundled: Vec<BundledSku>,
    /// One per availability (`OrderManagementData.Price`).
    pub prices: Vec<Price>,
}

impl Sku {
    /// A retail SKU: not a trial, demo or pre-order.
    pub fn is_retail(&self) -> bool {
        self.is_trial != Some(true)
            && self.is_demo != Some(true)
            && self.is_pre_order != Some(true)
            && !matches!(self.sku_type.as_deref(), Some("trial" | "demo"))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Package {
    /// As the catalog wrote it (normally a GUID).
    pub content_id: Option<String>,
    /// Upper-case: `MSIXVC`, `XVC`, `APPX`, `MSIX`, ...
    pub package_format: String,
    /// `Windows.Desktop`, `Windows.Xbox`, ...
    pub platforms: Vec<String>,
    pub architectures: Vec<String>,
    pub package_family_name: Option<String>,
    /// For add-ons: the base game's package family name.
    pub main_package_family_name_for_dlc: Option<String>,
    pub max_download_size: Option<u64>,
    pub max_install_size: Option<u64>,
}

impl Package {
    pub fn is_desktop(&self) -> bool {
        self.platforms.iter().any(|p| p == "Windows.Desktop")
    }

    /// A PC MSIXVC package for x64, the only kind Irori installs.
    pub fn is_pc_msixvc(&self) -> bool {
        self.package_format == "MSIXVC"
            && self.is_desktop()
            && self.architectures.iter().any(|a| a == "x64")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BundledSku {
    /// As the catalog wrote it (upper-cased); check with [`is_product_id`].
    pub product_id: String,
    pub primary: bool,
}

/// One availability's price. Each amount is missing, a non-negative number,
/// or malformed (present but not a non-negative number).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Price {
    pub msrp: Amount,
    pub list_price: Amount,
    pub base_price: Amount,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub enum Amount {
    #[default]
    Missing,
    Value(f64),
    Malformed,
}

impl Amount {
    fn from_json(value: Option<&Value>) -> Self {
        match value {
            None => Self::Missing,
            Some(value) => match value.as_f64() {
                Some(number) if number.is_finite() && number >= 0.0 => Self::Value(number),
                _ => Self::Malformed,
            },
        }
    }
}

impl Product {
    /// Reads one display-catalog product. `None` when it has no product ID.
    pub fn from_json(value: &Value) -> Option<Self> {
        let product_id = value
            .get("ProductId")?
            .as_str()?
            .trim()
            .to_ascii_uppercase();
        if product_id.is_empty() || product_id.len() > 64 {
            return None;
        }
        let localized = list(value, "LocalizedProperties").first();
        let localized = localized.unwrap_or(&Value::Null);
        let properties = value.get("Properties").unwrap_or(&Value::Null);
        let market = list(value, "MarketProperties")
            .first()
            .unwrap_or(&Value::Null);
        let images = list(localized, "Images")
            .iter()
            .take(200)
            .filter_map(Image::from_json)
            .collect();
        let related = list(value, "MarketProperties")
            .iter()
            .flat_map(|market| list(market, "RelatedProducts"))
            .take(512)
            .map(|relation| Relation {
                product_id: text(relation, "RelatedProductId", 64).to_ascii_uppercase(),
                kind: text(relation, "RelationshipType", 64),
            })
            .collect();
        let age_ratings = list(market, "ContentRatings")
            .iter()
            .take(32)
            .filter_map(|rating| {
                let system = text(rating, "RatingSystem", 32);
                let rating = text(rating, "RatingId", 64);
                (!system.is_empty() && !rating.is_empty()).then_some(AgeRating { system, rating })
            })
            .collect();
        let categories = list(properties, "Categories")
            .iter()
            .filter_map(Value::as_str)
            .map(|c| clean(c, 80))
            .filter(|c| !c.is_empty())
            .take(12)
            .collect::<Vec<_>>();
        let categories = if categories.is_empty() {
            text_opt(properties, "Category", 80).into_iter().collect()
        } else {
            categories
        };
        Some(Self {
            product_id,
            product_type: text(value, "ProductType", 64),
            title: text(localized, "ProductTitle", 512),
            description: text(localized, "ProductDescription", 64_000),
            publisher: text_opt(localized, "PublisherName", 512),
            developer: text_opt(localized, "DeveloperName", 512),
            images,
            xbox_title_id: xbox_title_id(value.get("AlternateIds")),
            product_group_id: text_opt(properties, "ProductGroupId", 128),
            categories,
            release_date: text_opt(market, "OriginalReleaseDate", 64),
            age_ratings,
            related,
            skus: list(value, "DisplaySkuAvailabilities")
                .iter()
                .take(128)
                .map(Sku::from_json)
                .collect(),
            is_demo: properties.get("IsDemo").and_then(Value::as_bool),
            is_trial: properties.get("IsTrial").and_then(Value::as_bool),
        })
    }

    /// The first image for the first purpose that has one.
    pub fn image(&self, purposes: &[&str]) -> Option<&str> {
        purposes.iter().find_map(|purpose| {
            self.images
                .iter()
                .find(|image| image.purpose == *purpose)
                .map(|image| image.url.as_str())
        })
    }

    /// Square cover art with the title's branding, then textless fallbacks.
    pub fn cover_url(&self) -> Option<&str> {
        self.image(&[
            "BoxArt",
            "TitledPoster",
            "Poster",
            "FeaturePromotionalSquareArt",
        ])
    }

    /// Landscape hero art.
    pub fn hero_url(&self) -> Option<&str> {
        self.image(&["TitledHeroArt", "SuperHeroArt", "Hero", "Screenshot"])
    }

    pub fn screenshots(&self) -> Vec<&str> {
        let mut seen = BTreeSet::new();
        self.images
            .iter()
            .filter(|image| image.purpose == "Screenshot" && seen.insert(image.url.as_str()))
            .map(|image| image.url.as_str())
            .take(24)
            .collect()
    }

    /// Platforms any package of the product runs on.
    pub fn platforms(&self) -> BTreeSet<&str> {
        self.skus
            .iter()
            .flat_map(|sku| &sku.packages)
            .flat_map(|package| &package.platforms)
            .map(String::as_str)
            .collect()
    }

    /// Valid product IDs related with `kind` (`addOnParent`, `Bundle`, ...).
    pub fn related_ids(&self, kind: &str) -> Vec<&str> {
        self.related
            .iter()
            .filter(|relation| relation.kind == kind && is_product_id(&relation.product_id))
            .map(|relation| relation.product_id.as_str())
            .collect()
    }

    /// Valid bundle members of the retail SKUs, with whether any SKU marks
    /// the member primary.
    pub fn bundle_members(&self) -> Vec<BundledSku> {
        let mut members: Vec<BundledSku> = Vec::new();
        for sku in self.skus.iter().filter(|sku| sku.is_retail()) {
            for member in sku.bundled.iter().filter(|m| is_product_id(&m.product_id)) {
                match members
                    .iter_mut()
                    .find(|m| m.product_id == member.product_id)
                {
                    Some(existing) => existing.primary |= member.primary,
                    None => members.push(member.clone()),
                }
            }
        }
        members
    }
}

impl Image {
    fn from_json(value: &Value) -> Option<Self> {
        let uri = value.get("Uri")?.as_str()?;
        Some(Self {
            purpose: text(value, "ImagePurpose", 64),
            url: image_url(uri)?,
            width: value
                .get("Width")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok()),
            height: value
                .get("Height")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok()),
        })
    }
}

impl Sku {
    fn from_json(availability: &Value) -> Self {
        let sku = availability.get("Sku").unwrap_or(&Value::Null);
        let properties = sku.get("Properties").unwrap_or(&Value::Null);
        Self {
            sku_id: text(sku, "SkuId", 32),
            sku_type: text_opt(sku, "SkuType", 32).map(|kind| kind.to_ascii_lowercase()),
            is_trial: properties.get("IsTrial").and_then(Value::as_bool),
            is_pre_order: properties.get("IsPreOrder").and_then(Value::as_bool),
            is_demo: properties.get("IsDemo").and_then(Value::as_bool),
            packages_declared: properties.get("Packages").is_some(),
            packages: list(properties, "Packages")
                .iter()
                .take(256)
                .map(Package::from_json)
                .collect(),
            bundled: list(properties, "BundledSkus")
                .iter()
                .take(256)
                .filter_map(|member| {
                    Some(BundledSku {
                        product_id: member.get("BigId")?.as_str()?.trim().to_ascii_uppercase(),
                        primary: member.get("IsPrimary").and_then(Value::as_bool) == Some(true),
                    })
                })
                .collect(),
            prices: list(availability, "Availabilities")
                .iter()
                .take(64)
                .map(|offer| {
                    let price = offer
                        .get("OrderManagementData")
                        .and_then(|data| data.get("Price"))
                        .unwrap_or(&Value::Null);
                    Price {
                        msrp: Amount::from_json(price.get("MSRP")),
                        list_price: Amount::from_json(price.get("ListPrice")),
                        base_price: Amount::from_json(price.get("BasePrice")),
                    }
                })
                .collect(),
        }
    }
}

impl Package {
    fn from_json(value: &Value) -> Self {
        Self {
            content_id: text_opt(value, "ContentId", 64),
            package_format: text(value, "PackageFormat", 32).to_ascii_uppercase(),
            platforms: list(value, "PlatformDependencies")
                .iter()
                .take(32)
                .map(|platform| text(platform, "PlatformName", 64))
                .filter(|name| !name.is_empty())
                .collect(),
            architectures: list(value, "Architectures")
                .iter()
                .take(16)
                .filter_map(Value::as_str)
                .map(|arch| clean(arch, 16))
                .collect(),
            package_family_name: text_opt(value, "PackageFamilyName", 256),
            main_package_family_name_for_dlc: text_opt(value, "MainPackageFamilyNameForDlc", 256),
            max_download_size: size(value.get("MaxDownloadSizeInBytes")),
            max_install_size: size(value.get("MaxInstallSizeInBytes")),
        }
    }
}

/// A byte count the catalog advertises: positive and exactly representable
/// as a JavaScript number, otherwise unknown.
fn size(value: Option<&Value>) -> Option<u64> {
    value?
        .as_u64()
        .filter(|n| *n > 0 && *n <= 9_007_199_254_740_991)
}

pub(crate) fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// Text without control characters, trimmed and capped at `max` characters.
pub(crate) fn clean(text: &str, max: usize) -> String {
    text.trim()
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(max)
        .collect()
}

pub(crate) fn text(value: &Value, key: &str, max: usize) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(|text| clean(text, max))
        .unwrap_or_default()
}

pub(crate) fn text_opt(value: &Value, key: &str, max: usize) -> Option<String> {
    Some(text(value, key, max)).filter(|text| !text.is_empty())
}

/// An `https` URL on `hosts`, without credentials or port. Protocol-relative
/// `//host/...` URLs (common in the catalog) become `https:`.
pub(crate) fn safe_url(raw: &str, hosts: &[&str]) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 8192 {
        return None;
    }
    let owned;
    let raw = if raw.starts_with("//") {
        owned = format!("https:{raw}");
        owned.as_str()
    } else {
        raw
    };
    let url = url::Url::parse(raw).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    (url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && hosts.contains(&host.as_str()))
    .then(|| url.to_string())
}

fn image_url(raw: &str) -> Option<String> {
    safe_url(raw, IMAGE_HOSTS)
}

/// The one Xbox title ID among `AlternateIds`; none when absent, malformed or
/// ambiguous.
fn xbox_title_id(value: Option<&Value>) -> Option<String> {
    let rows = value?.as_array()?;
    if rows.len() > 100 {
        return None;
    }
    let mut titles = BTreeSet::new();
    for row in rows {
        if row.get("IdType").and_then(Value::as_str) != Some("XboxTitleId") {
            continue;
        }
        let text = row.get("Value")?.as_str()?;
        if text.is_empty() || text.len() > 10 || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let id: u64 = text.parse().ok()?;
        if id == 0 || id > u64::from(u32::MAX) {
            return None;
        }
        titles.insert(id);
    }
    (titles.len() == 1)
        .then(|| titles.first().map(u64::to_string))
        .flatten()
}

/// A market as the Store expects it: two upper-case letters. A locale
/// (`en-GB`) gives its region; anything else falls back to `US`.
pub fn normalize_market(input: &str) -> String {
    let market = input.trim().to_ascii_uppercase();
    if http::is_market(&market) {
        return market;
    }
    match market.rsplit_once('-') {
        Some((_, region)) if http::is_market(region) => region.to_string(),
        _ => PUBLIC_STORE_MARKET.to_string(),
    }
}

impl StoreApi {
    /// Products by ID from the display catalog, in batches. IDs the catalog
    /// does not know are simply absent. Anonymous: no token.
    pub async fn products(&self, ids: &[String], market: &str) -> Result<Vec<Product>> {
        if ids.len() > MAX_IDS || ids.iter().any(|id| !is_product_id(id)) {
            return Err(StoreError::Invalid("product ID list"));
        }
        let market = normalize_market(market);
        let mut unique: Vec<&str> = Vec::new();
        for id in ids {
            if !unique.contains(&id.as_str()) {
                unique.push(id);
            }
        }
        // Owned batches: borrowed ones make the stream's future lifetime-generic,
        // which callers that must be `Send` (store refreshes) cannot hold.
        let batches: Vec<Vec<String>> = unique
            .chunks(BATCH)
            .map(|chunk| chunk.iter().map(|id| id.to_string()).collect())
            .collect();
        let market = &market;
        let mut results = futures::stream::iter(batches)
            .map(|batch| async move { self.product_batch(&batch, market).await })
            .buffered(PARALLEL);
        let mut products = Vec::new();
        while let Some(batch) = results.next().await {
            products.extend(batch?);
        }
        Ok(products)
    }

    async fn product_batch(&self, batch: &[String], market: &str) -> Result<Vec<Product>> {
        let url = format!("{}/v7.0/products", self.endpoints.displaycatalog);
        let request = self
            .client
            .get(url)
            .query(&[
                ("bigIds", batch.join(",").as_str()),
                ("market", market),
                ("languages", LANGUAGES),
            ])
            .header("Accept", "application/json");
        let response = http::send(request, TIMEOUT).await?;
        let body: Value = http::json_response(response, MAX_BODY).await?;
        let rows = body
            .get("Products")
            .and_then(Value::as_array)
            .ok_or(StoreError::Malformed("catalog-products"))?;
        if rows.len() > batch.len() * 2 {
            return Err(StoreError::Malformed("catalog-count"));
        }
        let mut seen = BTreeSet::new();
        Ok(rows
            .iter()
            .filter_map(Product::from_json)
            .filter(|product| batch.contains(&product.product_id))
            .filter(|product| seen.insert(product.product_id.clone()))
            .collect())
    }

    /// One product (`/v7.0/products/{id}`), `None` when the catalog has none.
    /// Anonymous: no token.
    pub async fn product(&self, id: &str, market: &str) -> Result<Option<Product>> {
        if !is_product_id(id) {
            return Err(StoreError::Invalid("product ID"));
        }
        let url = format!("{}/v7.0/products/{id}", self.endpoints.displaycatalog);
        let request = self
            .client
            .get(url)
            .query(&[("market", market), ("languages", "en,neutral")])
            .header("Accept", "application/json");
        let response = http::send(request, TIMEOUT).await?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        let body: Value = http::json_response(response, MAX_BODY).await?;
        let product = body
            .get("Product")
            .and_then(Product::from_json)
            .ok_or(StoreError::Malformed("catalog-product"))?;
        if product.product_id != id {
            return Err(StoreError::Malformed("catalog-identity"));
        }
        Ok(Some(product))
    }

    /// The game whose `AlternateIds` hold exactly this Xbox title ID (as
    /// title history and achievements identify games). Anonymous: no token.
    pub async fn product_for_title_id(
        &self,
        title_id: u32,
        market: &str,
    ) -> Result<Option<Product>> {
        if title_id == 0 {
            return Err(StoreError::Invalid("title ID"));
        }
        let title = title_id.to_string();
        let url = format!("{}/v7.0/products/lookup", self.endpoints.displaycatalog);
        let market = normalize_market(market);
        let request = self
            .client
            .get(url)
            .query(&[
                ("alternateId", "XboxTitleID"),
                ("value", title.as_str()),
                ("market", market.as_str()),
                ("languages", LANGUAGES),
                ("fieldsTemplate", "Details"),
            ])
            .header("Accept", "application/json");
        let response = http::send(request, Duration::from_secs(6)).await?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        let body: Value = http::json_response(response, MAX_BODY).await?;
        let rows = list(&body, "Products");
        if rows.len() > 25 {
            return Err(StoreError::Malformed("catalog-count"));
        }
        // A similar name never stands in: the title ID must match exactly.
        Ok(rows.iter().filter_map(Product::from_json).find(|product| {
            product.product_type == "Game"
                && product.xbox_title_id.as_deref() == Some(title.as_str())
        }))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use irori_depot::test_server::{Resp, TestServer};
    use serde_json::json;

    pub(crate) const GAME: &str = "9AAAAAAAAAAA";
    pub(crate) const CID: &str = "11111111-1111-4111-8111-111111111111";

    pub(crate) fn game_json(id: &str) -> Value {
        json!({
            "ProductId": id,
            "ProductType": "Game",
            "AlternateIds": [{"IdType": "XboxTitleId", "Value": "1234567"}, {"IdType": "LegacyXboxProductId", "Value": "x"}],
            "LocalizedProperties": [{
                "ProductTitle": "Synthetic\u{0007} Game",
                "ProductDescription": "A game.",
                "PublisherName": "Publisher",
                "DeveloperName": "Developer",
                "Images": [
                    {"ImagePurpose": "Poster", "Uri": "//store-images.s-microsoft.com/image/poster", "Width": 720, "Height": 1080},
                    {"ImagePurpose": "BoxArt", "Uri": "https://store-images.s-microsoft.com/image/box"},
                    {"ImagePurpose": "SuperHeroArt", "Uri": "https://evil.example/hero"},
                    {"ImagePurpose": "Screenshot", "Uri": "https://store-images.microsoft.com/image/shot"},
                    {"ImagePurpose": "Screenshot", "Uri": "https://store-images.microsoft.com/image/shot"},
                    {"ImagePurpose": "Logo", "Uri": 7}
                ]
            }],
            "MarketProperties": [{
                "OriginalReleaseDate": "2024-05-01T00:00:00.0000000Z",
                "ContentRatings": [{"RatingSystem": "ESRB", "RatingId": "ESRB:T"}, {"RatingSystem": "PEGI"}],
                "RelatedProducts": [{"RelatedProductId": "9BBBBBBBBBBB", "RelationshipType": "Bundle"}]
            }],
            "Properties": {"Categories": ["Action", 3, "Shooter"], "ProductGroupId": "group-1"},
            "DisplaySkuAvailabilities": [{
                "Sku": {"SkuId": "0010", "SkuType": "Full", "Properties": {
                    "IsTrial": false,
                    "Packages": [{"ContentId": CID, "PackageFormat": "msixvc", "Architectures": ["x64"],
                        "PlatformDependencies": [{"PlatformName": "Windows.Desktop"}],
                        "PackageFamilyName": "Synthetic.Game_8wekyb3d8bbwe",
                        "MaxDownloadSizeInBytes": 2_000_000_000u64, "MaxInstallSizeInBytes": -1}],
                    "BundledSkus": [{"BigId": "9ccccccccccc", "IsPrimary": true}]
                }},
                "Availabilities": [{"OrderManagementData": {"Price": {"MSRP": 59.99, "ListPrice": "free"}}}]
            }]
        })
    }

    #[test]
    fn product_parsing_keeps_valid_fields_and_drops_malformed_ones() {
        let product = Product::from_json(&game_json(GAME)).unwrap();
        assert_eq!(product.product_id, GAME);
        assert_eq!(product.title, "Synthetic Game");
        assert_eq!(product.xbox_title_id.as_deref(), Some("1234567"));
        assert_eq!(
            product.cover_url(),
            Some("https://store-images.s-microsoft.com/image/box")
        );
        assert_eq!(
            product.image(&["Poster"]),
            Some("https://store-images.s-microsoft.com/image/poster")
        );
        assert_eq!(
            product.hero_url(),
            Some("https://store-images.microsoft.com/image/shot")
        );
        assert_eq!(product.screenshots().len(), 1);
        assert_eq!(product.categories, vec!["Action", "Shooter"]);
        assert_eq!(
            product.age_ratings,
            vec![AgeRating {
                system: "ESRB".into(),
                rating: "ESRB:T".into()
            }]
        );
        assert_eq!(product.related_ids("Bundle"), vec!["9BBBBBBBBBBB"]);
        assert_eq!(
            product.release_date.as_deref(),
            Some("2024-05-01T00:00:00.0000000Z")
        );
        let sku = &product.skus[0];
        assert_eq!(sku.sku_type.as_deref(), Some("full"));
        assert!(sku.is_retail());
        let package = &sku.packages[0];
        assert!(package.is_pc_msixvc());
        assert_eq!(package.max_download_size, Some(2_000_000_000));
        assert_eq!(package.max_install_size, None);
        assert_eq!(sku.prices[0].msrp, Amount::Value(59.99));
        assert_eq!(sku.prices[0].list_price, Amount::Malformed);
        assert_eq!(sku.prices[0].base_price, Amount::Missing);
        assert_eq!(
            product.bundle_members(),
            vec![BundledSku {
                product_id: "9CCCCCCCCCCC".into(),
                primary: true
            }]
        );
        assert_eq!(
            product.platforms().into_iter().collect::<Vec<_>>(),
            vec!["Windows.Desktop"]
        );

        for bad in [
            json!(null),
            json!({}),
            json!({"ProductId": 5}),
            json!([1, 2]),
        ] {
            assert!(Product::from_json(&bad).is_none());
        }
        // Wrong types everywhere still yield a product without panicking.
        let odd = json!({"ProductId": "9AAAAAAAAAAA", "LocalizedProperties": "x", "DisplaySkuAvailabilities": [{"Sku": 1}, null],
            "MarketProperties": {"a": 1}, "AlternateIds": [{"IdType": "XboxTitleId", "Value": "0"}]});
        let product = Product::from_json(&odd).unwrap();
        assert_eq!(product.skus.len(), 2);
        assert!(product.xbox_title_id.is_none());
    }

    #[test]
    fn ambiguous_title_ids_are_not_chosen() {
        let two = json!([{"IdType": "XboxTitleId", "Value": "1"}, {"IdType": "XboxTitleId", "Value": "2"}]);
        assert_eq!(xbox_title_id(Some(&two)), None);
        let same = json!([{"IdType": "XboxTitleId", "Value": "01"}, {"IdType": "XboxTitleId", "Value": "1"}]);
        assert_eq!(xbox_title_id(Some(&same)).as_deref(), Some("1"));
        let large = json!([{"IdType": "XboxTitleId", "Value": "4294967296"}]);
        assert_eq!(xbox_title_id(Some(&large)), None);
    }

    #[test]
    fn markets_normalize() {
        assert_eq!(normalize_market("us"), "US");
        assert_eq!(normalize_market("en-GB"), "GB");
        assert_eq!(normalize_market("neutral"), "US");
    }

    #[tokio::test]
    async fn products_are_fetched_in_batches_and_filtered_to_the_request() {
        let server = TestServer::start(|req| {
            let ids: Vec<String> = req
                .path
                .split("bigIds=")
                .nth(1)
                .and_then(|rest| rest.split('&').next())
                .map(|ids| ids.split("%2C").map(str::to_string).collect())
                .unwrap_or_default();
            let mut products: Vec<Value> = ids.iter().map(|id| game_json(id)).collect();
            products.push(game_json("9ZZZZZZZZZZZ"));
            Resp::json(json!({"Products": products}))
        });
        let api = StoreApi::with_endpoints(
            reqwest::Client::new(),
            http::Endpoints::local(&server.base()),
        );
        let ids: Vec<String> = (0..25).map(|n| format!("9AAAAAAAAA{n:02}")).collect();
        let products = api.products(&ids, "us").await.unwrap();
        assert_eq!(products.len(), 25);
        assert!(products.iter().all(|p| p.product_id != "9ZZZZZZZZZZZ"));
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].path.starts_with("/v7.0/products?bigIds="));
        assert!(requests[0].path.contains("market=US"));
        assert!(api.products(&["bad".into()], "US").await.is_err());
    }

    #[tokio::test]
    async fn single_product_and_title_lookup() {
        let server = TestServer::start(|req| {
            if req.path.starts_with("/v7.0/products/lookup") {
                assert!(req.path.contains("alternateId=XboxTitleID&value=1234567"));
                let mut other = game_json("9BBBBBBBBBBB");
                other["AlternateIds"] = json!([{"IdType": "XboxTitleId", "Value": "7654321"}]);
                Resp::json(json!({"Products": [other, game_json(GAME)]}))
            } else if req.path.starts_with(&format!("/v7.0/products/{GAME}"))
                // Answers another product's ID: the identity check refuses it.
                || req.path.starts_with("/v7.0/products/9BBBBBBBBBBB")
            {
                Resp::json(json!({"Product": game_json(GAME)}))
            } else {
                Resp::status(404)
            }
        });
        let api = StoreApi::with_endpoints(
            reqwest::Client::new(),
            http::Endpoints::local(&server.base()),
        );
        assert_eq!(
            api.product(GAME, "US").await.unwrap().unwrap().product_id,
            GAME
        );
        assert_eq!(api.product("9CCCCCCCCCCC", "US").await.unwrap(), None);
        assert_eq!(
            api.product("9BBBBBBBBBBB", "US").await,
            Err(StoreError::Malformed("catalog-identity"))
        );
        let found = api
            .product_for_title_id(1_234_567, "US")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.product_id, GAME);
    }
}
