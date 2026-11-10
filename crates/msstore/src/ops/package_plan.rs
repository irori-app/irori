//! Public Store metadata only. A plan is not an entitlement or a download URL.
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use uuid::Uuid;

const MAX_PRODUCTS: usize = 48;
const MAX_DEPTH: usize = 6;
const MAX_CHOICES: usize = 128;

#[cfg(test)]
#[path = "package_plan_http_tests.rs"]
mod http_tests;

#[derive(Clone, Debug)]
pub struct Choice {
    pub product_id: String,
    pub content_id: String,
    pub title: String,
    pub platform: String,
    pub format: String,
    pub kind: String,
    pub supported: bool,
    pub parent_product_id: Option<String>,
    pub main_package_family_name: Option<String>,
    pub download_bytes: Option<u64>,
    pub install_bytes: Option<u64>,
}
#[derive(Clone, Debug)]
pub struct Plan {
    pub choices: Vec<Choice>,
}

fn size(package: &Value, key: &str) -> Option<u64> {
    package[key]
        .as_u64()
        .filter(|n| *n > 0 && *n <= 9_007_199_254_740_991)
}

impl Choice {
    fn merge_estimates(&mut self, other: &Self) {
        // Catalog duplicates can disagree. Keep the largest eligible estimate,
        // never a trial/preorder or another content package's advertised size.
        self.download_bytes = self.download_bytes.max(other.download_bytes);
        self.install_bytes = self.install_bytes.max(other.install_bytes);
    }
}

fn product_id(id: &str) -> bool {
    id.len() == 12
        && id
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}
fn text(value: &Value, key: &str) -> String {
    value[key]
        .as_str()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(240)
        .collect()
}
fn list<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn dlc_parent(product: &Value) -> Option<String> {
    let mut parents = HashSet::new();
    for market in list(product, "MarketProperties") {
        for relation in list(market, "RelatedProducts") {
            if relation["RelationshipType"].as_str() != Some("addOnParent") {
                continue;
            }
            let id = relation["RelatedProductId"].as_str()?;
            if !product_id(id) || product["ProductId"].as_str() == Some(id) {
                return None;
            }
            parents.insert(id.to_string());
        }
    }
    if parents.len() == 1 {
        parents.into_iter().next()
    } else {
        None
    }
}
fn package_family(value: &str) -> bool {
    let Some((name, publisher)) = value.split_once('_') else {
        return false;
    };
    value.len() <= 200
        && !name.is_empty()
        && !publisher.is_empty()
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-'))
        && publisher.bytes().all(|c| c.is_ascii_alphanumeric())
}

// Only explicit bundle membership describes downloadable children. Licensing
// keys name offers that grant this product, not products contained in it.
fn inspect(product: &Value, id: &str) -> Result<(Vec<Choice>, Vec<String>), String> {
    if product["ProductId"].as_str() != Some(id) {
        return Err("Store product identity mismatch".into());
    }
    let localized = list(product, "LocalizedProperties")
        .first()
        .unwrap_or(&Value::Null);
    let title = text(localized, "ProductTitle");
    let title = if title.is_empty() {
        id.to_string()
    } else {
        title
    };
    let kind = text(product, "ProductType");
    let parent_product_id = if kind == "Durable" {
        dlc_parent(product)
    } else {
        None
    };
    let mut choices: BTreeMap<(String, String, String), Choice> = BTreeMap::new();
    let mut bundled = HashSet::new();
    for availability in list(product, "DisplaySkuAvailabilities") {
        let properties = &availability["Sku"]["Properties"];
        for package in list(properties, "Packages") {
            let Some(raw) = package["ContentId"].as_str() else {
                continue;
            };
            let Ok(cid) = Uuid::parse_str(raw) else {
                continue;
            };
            let content_id = cid.to_string();
            let platforms: Vec<_> = list(package, "PlatformDependencies")
                .iter()
                .map(|v| text(v, "PlatformName"))
                .filter(|s| !s.is_empty())
                .collect();
            let desktop = platforms.iter().any(|p| p == "Windows.Desktop");
            let format = text(package, "PackageFormat").to_uppercase();
            let main_package_family_name = package["MainPackageFamilyNameForDlc"]
                .as_str()
                .filter(|name| package_family(name))
                .map(str::to_string);
            let durable = kind == "Durable"
                && parent_product_id.is_some()
                && main_package_family_name.is_some()
                && list(package, "Architectures")
                    .iter()
                    .any(|arch| arch.as_str() == Some("x64"));
            let supported = desktop
                && format == "MSIXVC"
                && (kind == "Game" || kind == "GameAddOn" || durable)
                && properties["IsTrial"].as_bool() != Some(true)
                && properties["IsPreOrder"].as_bool() != Some(true);
            let platform = if desktop {
                "Windows.Desktop".to_string()
            } else {
                platforms.join(", ")
            };
            let key = (content_id.clone(), platform.clone(), format.clone());
            let choice = Choice {
                product_id: id.to_string(),
                content_id,
                title: title.clone(),
                platform,
                format,
                kind: kind.clone(),
                supported,
                parent_product_id: parent_product_id.clone(),
                main_package_family_name,
                download_bytes: size(package, "MaxDownloadSizeInBytes"),
                install_bytes: size(package, "MaxInstallSizeInBytes"),
            };
            // The same package can appear in preorder, retail and trial SKUs.
            // Preserve an eligible retail SKU regardless of catalog ordering.
            if let Some(old) = choices.get_mut(&key).filter(|old| old.supported) {
                if choice.supported {
                    old.merge_estimates(&choice);
                }
            } else {
                choices.insert(key, choice);
            }
        }
        for reference in list(properties, "BundledSkus") {
            if let Some(child) = reference["BigId"].as_str().filter(|v| product_id(v)) {
                bundled.insert(child.to_string());
            }
        }
    }
    if choices.len() > MAX_CHOICES {
        return Err("Too many Store packages".into());
    }
    // Direct packages define this product. Do not follow unrelated licensing keys.
    let has_desktop = choices.values().any(|c| c.platform == "Windows.Desktop");
    let mut children: Vec<_> = if has_desktop {
        vec![]
    } else {
        bundled.into_iter().collect()
    };
    children.sort();
    if children.len() > MAX_PRODUCTS {
        return Err("Store bundle contains too many products".into());
    }
    Ok((choices.into_values().collect(), children))
}

pub fn from_products(root: &str, products: &BTreeMap<String, Value>) -> Result<Plan, String> {
    fn walk(
        id: &str,
        products: &BTreeMap<String, Value>,
        path: &mut HashSet<String>,
        seen: &mut HashSet<String>,
        choices: &mut BTreeMap<String, Choice>,
        depth: usize,
    ) -> Result<(), String> {
        if depth > MAX_DEPTH || path.contains(id) {
            return Err("Circular or excessively deep Store bundle".into());
        }
        if seen.contains(id) {
            return Ok(());
        }
        if seen.len() + path.len() >= MAX_PRODUCTS {
            return Err("Store bundle contains too many products".into());
        }
        path.insert(id.to_string());
        let product = products.get(id).ok_or("Incomplete Store bundle metadata")?;
        let (direct, children) = inspect(product, id)?;
        for choice in direct {
            let key = choice.content_id.clone();
            if let Some(old) = choices.get_mut(&key).filter(|old| old.supported) {
                if choice.supported {
                    old.merge_estimates(&choice);
                }
            } else {
                choices.insert(key, choice);
            }
        }
        if choices.len() > MAX_CHOICES {
            return Err("Too many Store packages".into());
        }
        for child in children {
            walk(&child, products, path, seen, choices, depth + 1)?;
        }
        path.remove(id);
        seen.insert(id.to_string());
        Ok(())
    }
    let mut choices = BTreeMap::new();
    walk(
        root,
        products,
        &mut HashSet::new(),
        &mut HashSet::new(),
        &mut choices,
        0,
    )?;
    Ok(Plan {
        choices: choices.into_values().collect(),
    })
}

impl Plan {
    pub fn without_durable_selection(mut self) -> Self {
        for choice in &mut self.choices {
            if choice.kind == "Durable" {
                choice.supported = false;
            }
        }
        self
    }
    pub fn automatic(&self) -> Option<&Choice> {
        let eligible: Vec<_> = self.choices.iter().filter(|c| c.supported).collect();
        if eligible.len() == 1 && eligible[0].kind == "Game" {
            Some(eligible[0])
        } else {
            None
        }
    }
    pub fn select(&self, selected: Option<&str>) -> Result<String, String> {
        if let Some(raw) = selected {
            let cid = Uuid::parse_str(raw)
                .map_err(|_| "Invalid selected Content ID")?
                .to_string();
            let matches: Vec<_> = self
                .choices
                .iter()
                .filter(|c| c.supported && c.content_id == cid)
                .collect();
            if matches.len() != 1 {
                return Err(
                    "Selected package is not a unique supported member of this Store offer".into(),
                );
            }
            return Ok(cid);
        }
        self.automatic()
            .map(|c| c.content_id.clone())
            .ok_or("Select an installable package in Nexus before starting this download".into())
    }
    pub fn json(&self, root: &str, market: &str) -> Value {
        json!({"schema":1,"rootProductId":root,"market":market,
            "status":if self.automatic().is_some() {"ready"} else if self.choices.iter().any(|c| c.supported) {"choice"} else {"unavailable"},
            "choices":self.choices.iter().map(|c| json!({"productId":c.product_id,"contentId":c.content_id,"title":c.title,
                "platform":c.platform,"packageFormat":c.format,"kind":c.kind,"supported":c.supported,
                "parentProductId":c.parent_product_id,"mainPackageFamilyName":c.main_package_family_name,
                "downloadBytes":c.download_bytes,"installBytes":c.install_bytes})).collect::<Vec<_>>()})
    }
}

pub async fn resolve(client: &reqwest::Client, root: &str, market: &str) -> Result<Plan, String> {
    resolve_from(
        client,
        root,
        market,
        "https://displaycatalog.mp.microsoft.com/v7.0/products",
    )
    .await
}

// The production endpoint is fixed above. A private parameter permits local
// HTTP regression fixtures without accounts, licensing or external requests.
async fn resolve_from(
    client: &reqwest::Client,
    root: &str,
    market: &str,
    catalog: &str,
) -> Result<Plan, String> {
    if !product_id(root) || !(market.len() == 2 && market.bytes().all(|c| c.is_ascii_uppercase())) {
        return Err("Invalid Store product or market".into());
    }
    let fetch = async {
        let mut pending = vec![(root.to_string(), 0)];
        let mut products = BTreeMap::new();
        while let Some((id, depth)) = pending.pop() {
            if products.contains_key(&id) {
                continue;
            }
            if depth > MAX_DEPTH || products.len() >= MAX_PRODUCTS {
                return Err("Store bundle exceeds resolver limits".into());
            }
            let response = client
                .get(format!(
                    "{catalog}/{id}?market={market}&languages=en,neutral"
                ))
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
                .map_err(|_| {
                    format!("Store metadata request failed for product {id} (market {market})")
                })?;
            if !response.status().is_success() {
                return Err(format!(
                    "Store metadata returned HTTP {} for product {id} (market {market})",
                    response.status().as_u16()
                ));
            }
            let mut response = response;
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| "Store metadata read failed")?
            {
                if bytes.len() + chunk.len() > 4_000_000 {
                    return Err("Store metadata is too large".into());
                }
                bytes.extend_from_slice(&chunk);
            }
            let document: Value =
                serde_json::from_slice(&bytes).map_err(|_| "Invalid Store metadata")?;
            let product = document["Product"].clone();
            let (_, children) = inspect(&product, &id)?;
            for child in children {
                pending.push((child, depth + 1));
            }
            products.insert(id, product);
        }
        from_products(root, &products)
    };
    tokio::time::timeout(std::time::Duration::from_secs(35), fetch)
        .await
        .map_err(|_| "Store package resolution timed out".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_storage_estimates_are_optional_positive_and_bound_to_the_selected_content() {
        let mut product = package(ROOT, "Windows.Desktop", "MSIXVC");
        let p = &mut product["DisplaySkuAvailabilities"][0]["Sku"]["Properties"]["Packages"][0];
        p["MaxDownloadSizeInBytes"] = json!(2_069_975_040u64);
        p["MaxInstallSizeInBytes"] = json!(3_000_000_000u64);
        let plan = from_products(ROOT, &BTreeMap::from([(ROOT.into(), product.clone())])).unwrap();
        let choice = &plan.json(ROOT, "US")["choices"][0];
        assert_eq!(choice["downloadBytes"], 2_069_975_040u64);
        assert_eq!(choice["installBytes"], 3_000_000_000u64);
        assert_eq!(plan.select(None).unwrap(), CID);
        for value in [
            Value::Null,
            json!(0),
            json!(-1),
            json!(1.5),
            json!("100"),
            json!(9_007_199_254_740_992u64),
        ] {
            let p = &mut product["DisplaySkuAvailabilities"][0]["Sku"]["Properties"]["Packages"][0];
            p["MaxDownloadSizeInBytes"] = value.clone();
            p["MaxInstallSizeInBytes"] = value;
            let plan =
                from_products(ROOT, &BTreeMap::from([(ROOT.into(), product.clone())])).unwrap();
            let choice = &plan.json(ROOT, "US")["choices"][0];
            assert!(choice["downloadBytes"].is_null());
            assert!(choice["installBytes"].is_null());
            assert_eq!(plan.select(None).unwrap(), CID);
        }
    }

    #[test]
    fn package_storage_estimates_merge_retail_duplicates_without_using_trial_or_foreign_sizes() {
        let mut product = package(ROOT, "Windows.Desktop", "MSIXVC");
        product["DisplaySkuAvailabilities"][0]["Sku"]["Properties"]["Packages"][0]["MaxDownloadSizeInBytes"] =
            json!(100);
        let mut retail = product["DisplaySkuAvailabilities"][0].clone();
        retail["Sku"]["Properties"]["Packages"][0]["MaxDownloadSizeInBytes"] = json!(200);
        let mut trial = retail.clone();
        trial["Sku"]["Properties"]["IsTrial"] = json!(true);
        trial["Sku"]["Properties"]["Packages"][0]["MaxDownloadSizeInBytes"] = json!(999);
        let mut other = retail.clone();
        other["Sku"]["Properties"]["Packages"][0]["ContentId"] =
            json!("22222222-2222-4222-8222-222222222222");
        other["Sku"]["Properties"]["Packages"][0]["MaxDownloadSizeInBytes"] = json!(500);
        for reversed in [false, true] {
            let base = product["DisplaySkuAvailabilities"][0].clone();
            let mut skus = vec![base, retail.clone(), trial.clone(), other.clone()];
            if reversed {
                skus.reverse();
            }
            product["DisplaySkuAvailabilities"] = json!(skus);
            let plan =
                from_products(ROOT, &BTreeMap::from([(ROOT.into(), product.clone())])).unwrap();
            let choices = plan.json(ROOT, "US")["choices"].as_array().unwrap().clone();
            assert_eq!(choices.len(), 2);
            assert_eq!(
                choices.iter().find(|c| c["contentId"] == CID).unwrap()["downloadBytes"],
                200
            );
            assert_eq!(
                choices.iter().find(|c| c["contentId"] != CID).unwrap()["downloadBytes"],
                500
            );
        }
    }
    const ROOT: &str = "9AAAAAAAAAAA";
    const CHILD: &str = "9BBBBBBBBBBB";
    const CID: &str = "11111111-1111-4111-8111-111111111111";
    fn package(id: &str, platform: &str, format: &str) -> Value {
        json!({"ProductId":id,"ProductType":"Game","LocalizedProperties":[{"ProductTitle":"Test game"}],
            "DisplaySkuAvailabilities":[{"Sku":{"Properties":{"Packages":[{"ContentId":CID,"PackageFormat":format,"PlatformDependencies":[{"PlatformName":platform}]}]}}}]})
    }
    fn bundle(id: &str, children: &[&str]) -> Value {
        json!({"ProductId":id,"DisplaySkuAvailabilities":[{"Sku":{"Properties":{"BundledSkus":children.iter().map(|id| json!({"BigId":id})).collect::<Vec<_>>()}}}]})
    }
    fn durable() -> Value {
        let mut p = package(CHILD, "Windows.Desktop", "MSIXVC");
        p["ProductType"] = json!("Durable");
        p["MarketProperties"] = json!([{ "RelatedProducts": [{ "RelationshipType": "addOnParent", "RelatedProductId": ROOT }] }]);
        p["DisplaySkuAvailabilities"][0]["Sku"]["Properties"]["IsTrial"] = json!(false);
        let pkg = &mut p["DisplaySkuAvailabilities"][0]["Sku"]["Properties"]["Packages"][0];
        pkg["Architectures"] = json!(["x64"]);
        pkg["MainPackageFamilyNameForDlc"] = json!("Example.Base_hash");
        p
    }
    #[test]
    fn durable_pc_package_requires_explicit_selection_and_verified_parent_metadata() {
        let plan = from_products(CHILD, &BTreeMap::from([(CHILD.into(), durable())])).unwrap();
        assert_eq!(plan.json(CHILD, "US")["status"], "choice");
        assert!(plan.select(None).is_err());
        assert_eq!(plan.select(Some(CID)).unwrap(), CID);
        let value = plan.json(CHILD, "US");
        assert_eq!(value["choices"][0]["parentProductId"], ROOT);
        assert_eq!(
            value["choices"][0]["mainPackageFamilyName"],
            "Example.Base_hash"
        );
        let legacy = from_products(CHILD, &BTreeMap::from([(CHILD.into(), durable())]))
            .unwrap()
            .without_durable_selection();
        assert_eq!(legacy.json(CHILD, "US")["status"], "unavailable");
        assert!(legacy.select(Some(CID)).is_err());
    }
    #[test]
    fn durable_missing_foreign_or_ambiguous_parent_and_non_pc_formats_stay_unsupported() {
        for variant in 0..8 {
            let mut p = durable();
            match variant {
                0 => p["MarketProperties"] = json!([]),
                1 => {
                    p["MarketProperties"][0]["RelatedProducts"][0]["RelatedProductId"] =
                        json!("bad-id")
                }
                2 => p["MarketProperties"][0]["RelatedProducts"]
                    .as_array_mut()
                    .unwrap()
                    .push(
                        json!({"RelationshipType":"addOnParent","RelatedProductId":"9CCCCCCCCCCC"}),
                    ),
                3 => {
                    p["DisplaySkuAvailabilities"][0]["Sku"]["Properties"]["Packages"][0]["MainPackageFamilyNameForDlc"] =
                        json!("../outside")
                }
                4 => {
                    p["DisplaySkuAvailabilities"][0]["Sku"]["Properties"]["Packages"][0]["Architectures"] =
                        json!(["arm64"])
                }
                5 => {
                    p["DisplaySkuAvailabilities"][0]["Sku"]["Properties"]["Packages"][0]["PackageFormat"] =
                        json!("APPX")
                }
                6 => p["DisplaySkuAvailabilities"][0]["Sku"]["Properties"]["IsTrial"] = json!(true),
                _ => {
                    p["DisplaySkuAvailabilities"][0]["Sku"]["Properties"]["IsPreOrder"] =
                        json!(true)
                }
            }
            let plan = from_products(CHILD, &BTreeMap::from([(CHILD.into(), p)])).unwrap();
            assert!(plan.select(Some(CID)).is_err(), "variant {variant}");
        }
    }
    #[test]
    fn direct_and_selected_membership() {
        let plan = from_products(
            ROOT,
            &BTreeMap::from([(ROOT.into(), package(ROOT, "Windows.Desktop", "MSIXVC"))]),
        )
        .unwrap();
        assert_eq!(plan.select(None).unwrap(), CID);
        assert!(
            plan.select(Some("22222222-2222-4222-8222-222222222222"))
                .is_err()
        );
        assert_eq!(plan.json(ROOT, "US")["status"], "ready");
    }
    #[test]
    fn unsupported_and_ambiguous_are_not_guessed() {
        for (platform, format) in [("Xbox.One", "MSIXVC"), ("Windows.Desktop", "APPX")] {
            let plan = from_products(
                ROOT,
                &BTreeMap::from([(ROOT.into(), package(ROOT, platform, format))]),
            )
            .unwrap();
            assert!(plan.select(None).is_err());
            assert!(plan.select(Some(CID)).is_err());
        }
        let mut second = package(CHILD, "Windows.Desktop", "MSIXVC");
        second["DisplaySkuAvailabilities"][0]["Sku"]["Properties"]["Packages"][0]["ContentId"] =
            json!("22222222-2222-4222-8222-222222222222");
        let plan = from_products(
            "9CCCCCCCCCCC",
            &BTreeMap::from([
                (
                    "9CCCCCCCCCCC".into(),
                    bundle("9CCCCCCCCCCC", &[ROOT, CHILD]),
                ),
                (ROOT.into(), package(ROOT, "Windows.Desktop", "MSIXVC")),
                (CHILD.into(), second),
            ]),
        )
        .unwrap();
        assert!(plan.select(None).is_err());
        assert!(plan.select(Some(CID)).is_ok());
        assert_eq!(plan.json("9CCCCCCCCCCC", "US")["status"], "choice");
    }
    #[test]
    fn durable_entitlement_back_references_do_not_form_bundle_cycles() {
        const DURABLE: &str = "9DDDDDDDDDDD";
        let durable = json!({"ProductId":DURABLE,"ProductType":"Durable",
        "DisplaySkuAvailabilities":[{
            "Sku":{"Properties":{"Packages":[]}},
            "Availabilities":[{"LicensingData":{"SatisfyingEntitlementKeys":[{
                "EntitlementKeys":[format!("big:{ROOT}:0010")]
            }]}}]
        }]});
        let plan = from_products(
            ROOT,
            &BTreeMap::from([
                (ROOT.into(), bundle(ROOT, &[CHILD, DURABLE])),
                (CHILD.into(), package(CHILD, "Windows.Desktop", "MSIXVC")),
                (DURABLE.into(), durable.clone()),
            ]),
        )
        .unwrap();
        assert_eq!(plan.select(None).unwrap(), CID);

        let standalone =
            from_products(DURABLE, &BTreeMap::from([(DURABLE.into(), durable)])).unwrap();
        assert!(standalone.select(None).is_err());
    }
    #[test]
    fn cycles_missing_children_and_identity_fail_closed() {
        assert!(
            from_products(
                ROOT,
                &BTreeMap::from([(ROOT.into(), bundle(ROOT, &[ROOT]))])
            )
            .is_err()
        );
        assert!(
            from_products(
                ROOT,
                &BTreeMap::from([(ROOT.into(), bundle(ROOT, &[CHILD]))])
            )
            .is_err()
        );
        assert!(
            from_products(
                ROOT,
                &BTreeMap::from([(ROOT.into(), package(CHILD, "Windows.Desktop", "MSIXVC"))])
            )
            .is_err()
        );
    }
    #[test]
    fn shared_children_deduplicate_and_addons_need_explicit_selection() {
        let mut child = package(CHILD, "Windows.Desktop", "MSIXVC");
        child["ProductType"] = json!("GameAddOn");
        let plan = from_products(
            ROOT,
            &BTreeMap::from([
                (ROOT.into(), bundle(ROOT, &[CHILD, CHILD])),
                (CHILD.into(), child),
            ]),
        )
        .unwrap();
        assert_eq!(plan.choices.len(), 1);
        assert!(plan.select(None).is_err());
        assert!(plan.select(Some(CID)).is_ok());
    }
    #[test]
    fn retail_preorder_and_trial_sku_order_never_hides_retail_package() {
        let mut product = package(ROOT, "Windows.Desktop", "MSIXVC");
        let retail = product["DisplaySkuAvailabilities"][0].clone();
        let mut trial = retail.clone();
        trial["Sku"]["Properties"]["IsTrial"] = json!(true);
        let mut preorder = retail.clone();
        preorder["Sku"]["Properties"]["IsPreOrder"] = json!(true);
        for skus in [
            vec![preorder.clone(), retail.clone(), trial.clone()],
            vec![trial.clone(), preorder.clone(), retail.clone()],
        ] {
            product["DisplaySkuAvailabilities"] = json!(skus);
            assert!(
                from_products(ROOT, &BTreeMap::from([(ROOT.into(), product.clone())]))
                    .unwrap()
                    .select(None)
                    .is_ok()
            );
        }
        product["DisplaySkuAvailabilities"] = json!([trial, preorder]);
        assert!(
            from_products(ROOT, &BTreeMap::from([(ROOT.into(), product)]))
                .unwrap()
                .select(None)
                .is_err()
        );
    }
    #[test]
    fn console_root_can_expose_declared_pc_child() {
        let mut root = package(ROOT, "Xbox.One", "XVC");
        root["DisplaySkuAvailabilities"][0]["Sku"]["Properties"]["Packages"][0]["ContentId"] =
            json!("22222222-2222-4222-8222-222222222222");
        root["DisplaySkuAvailabilities"][0]["Sku"]["Properties"]["BundledSkus"] =
            json!([{"BigId":CHILD}]);
        let plan = from_products(
            ROOT,
            &BTreeMap::from([
                (ROOT.into(), root),
                (CHILD.into(), package(CHILD, "Windows.Desktop", "MSIXVC")),
            ]),
        )
        .unwrap();
        assert_eq!(plan.choices.len(), 2);
        assert!(plan.select(None).is_ok());
    }

    fn license_only_addon(id: &str, offers: &[&str]) -> Value {
        json!({"ProductId":id,"ProductType":"GameAddOn",
        "DisplaySkuAvailabilities":[{"Sku":{"Properties":{}},"Availabilities":[{
            "LicensingData":{"SatisfyingEntitlementKeys":[{"EntitlementKeys":offers.iter()
                .map(|offer| format!("big:{offer}:0010")).collect::<Vec<_>>()}]}
        }]}]})
    }

    #[test]
    fn premium_entitlement_links_do_not_walk_back_to_parent_or_foreign_games() {
        let addon = "9CCCCCCCCCCC";
        // A bundle grants its license-only VIP upgrade, not the other way round.
        // The other offer can be a delisted cross-game bundle (HTTP 404).
        let plan = from_products(
            ROOT,
            &BTreeMap::from([
                (ROOT.into(), bundle(ROOT, &[CHILD, addon])),
                (CHILD.into(), package(CHILD, "Windows.Desktop", "MSIXVC")),
                (
                    addon.into(),
                    license_only_addon(addon, &[ROOT, "9DDDDDDDDDDD"]),
                ),
            ]),
        )
        .unwrap();
        assert_eq!(plan.choices.len(), 1);
        assert_eq!(plan.select(None).unwrap(), CID);
        assert_eq!(plan.choices[0].product_id, CHILD);
    }

    #[test]
    fn premium_license_only_addon_never_selects_a_game_that_grants_it() {
        let plan = from_products(
            ROOT,
            &BTreeMap::from([
                (ROOT.into(), license_only_addon(ROOT, &[CHILD])),
                (CHILD.into(), package(CHILD, "Windows.Desktop", "MSIXVC")),
            ]),
        )
        .unwrap();
        assert!(plan.choices.is_empty());
        assert!(plan.select(None).is_err());
        assert!(plan.select(Some(CID)).is_err());
    }

    #[test]
    fn premium_game_entitlement_is_not_bundle_membership_either() {
        let mut offer = license_only_addon(ROOT, &[CHILD]);
        offer["ProductType"] = json!("Game");
        let plan = from_products(
            ROOT,
            &BTreeMap::from([
                (ROOT.into(), offer),
                (CHILD.into(), package(CHILD, "Windows.Desktop", "MSIXVC")),
            ]),
        )
        .unwrap();
        assert!(plan.choices.is_empty());
        assert!(plan.select(None).is_err());
        assert!(plan.select(Some(CID)).is_err());
    }
}
