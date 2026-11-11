//! Read-only Store entitlements. No interactive auth, provisioning, or cloud writes.
use crate::tokens::TokenManager;
use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    time::Duration,
};

fn product_id(id: &str) -> bool {
    id.len() == 12
        && id
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

fn entry(id: &str, item: &Value) -> Result<Value, &'static str> {
    if !product_id(id) || !item.is_object() {
        return Err("entitlement-product");
    }
    let status = item["status"]
        .as_str()
        .filter(|s| s.len() < 40)
        .ok_or("entitlement-status")?;
    let trial = item["isTrial"].as_bool().ok_or("entitlement-trial")?;
    let sharing = item["sharingSource"]
        .as_str()
        .filter(|s| s.len() < 40)
        .ok_or("entitlement-sharing")?;
    let expiry = item["endDate"]
        .as_str()
        .filter(|s| s.len() < 50)
        .filter(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok())
        .ok_or_else(|| {
            tracing::debug!(
                "NEXUS_LIBRARY_DIAGNOSTIC_V1 end-date-shape {}",
                shape(&item["endDate"])
            );
            "entitlement-expiry"
        })?;
    let parent = match item.get("satisfiedByProductId") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) if product_id(id) => Some(id.as_str()),
        _ => return Err("entitlement-parent"),
    };
    // Preserve typed Positron provenance for conditional library access.
    // Unknown special licensing flags never become permanent ownership.
    let special = item
        .get("isPositron")
        .map(|v| v.as_bool() != Some(false))
        .unwrap_or(false);
    let positron = item.get("isPositron").and_then(Value::as_bool) == Some(true);
    Ok(json!({"productId":id,"status":status,"isTrial":trial,
            "sharingSource":sharing,"endDate":expiry,"satisfiedByProductId":parent,
            "special":special,"isPositron":positron}))
}

// A malformed row is not ownership. Quarantine that ID across all pages and
// omit its dependent rows, while keeping independently verified entitlements.
#[derive(Default)]
struct Entitlements {
    entries: BTreeMap<String, Value>,
    rejected: HashSet<String>,
    reasons: BTreeMap<&'static str, usize>,
    skipped: usize,
    total: usize,
}

impl Entitlements {
    fn add(&mut self, body: &Value) -> Result<(), &'static str> {
        let rows = body["entitlements"]
            .as_object()
            .ok_or("entitlement-shape")?;
        self.total += rows.len();
        if self.total > 10_000 {
            return Err("entitlement-count");
        }
        for (id, item) in rows {
            match entry(id, item) {
                Ok(row) if !self.rejected.contains(id) => {
                    if self.entries.get(id).is_some_and(|old| old != &row) {
                        self.entries.remove(id);
                        self.rejected.insert(id.clone());
                        self.skipped += 2;
                        *self.reasons.entry("entitlements-inconsistent").or_default() += 1;
                    } else {
                        self.entries.insert(id.clone(), row);
                    }
                }
                Ok(_) => self.skipped += 1,
                Err(reason) => {
                    self.skipped += 1;
                    *self.reasons.entry(reason).or_default() += 1;
                    if product_id(id) {
                        self.rejected.insert(id.clone());
                        if self.entries.remove(id).is_some() {
                            self.skipped += 1;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn finish(&mut self) {
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        for (id, row) in &self.entries {
            if let Some(parent) = row["satisfiedByProductId"].as_str() {
                children.entry(parent.into()).or_default().push(id.clone());
            }
        }
        let mut queue: VecDeque<String> = self.rejected.iter().cloned().collect();
        while let Some(parent) = queue.pop_front() {
            for child in children.remove(&parent).unwrap_or_default() {
                if self.rejected.insert(child.clone()) {
                    self.entries.remove(&child);
                    self.skipped += 1;
                    queue.push_back(child);
                }
            }
        }
    }
}

/// The form of a value with its content masked (digits as 9, letters as a),
/// for diagnostics that must not carry account data.
fn shape(value: &Value) -> String {
    match value {
        Value::String(s) => s
            .chars()
            .take(50)
            .map(|c| match c {
                '0'..='9' => '9',
                c if c.is_ascii_alphabetic() => 'a',
                c if c.is_ascii_punctuation() || c == ' ' => c,
                _ => '?',
            })
            .collect(),
        Value::Null => "null".into(),
        Value::Bool(_) => "bool".into(),
        Value::Number(_) => "number".into(),
        Value::Array(_) => "array".into(),
        Value::Object(_) => "object".into(),
    }
}

fn diagnostic(stage: &'static str, status: u16, count: usize) {
    // Static stage + bounded numeric metadata only, never a response or account.
    tracing::debug!("NEXUS_LIBRARY_DIAGNOSTIC_V1 {stage} {status} {count}");
}

async fn query(tokens: &TokenManager, market: &str) -> Result<Value, &'static str> {
    if market.len() != 2 || !market.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err("market");
    }
    let account = tokens
        .get_license_user()
        .map_err(|_| "stored-account")?
        .username;
    let (client, ticket, snapshot) = super::subscription::store_session(tokens, &account).await?;
    let mut all = Entitlements::default();
    let mut continuation: Option<String> = None;
    let mut seen = HashSet::new();
    let mut total_bytes = 0;
    for _ in 0..20 {
        let cv = format!(
            "{}.0",
            STANDARD_NO_PAD.encode(uuid::Uuid::new_v4().as_bytes())
        );
        let url = format!(
            "https://beige.xboxservices.com/PCGAFD/entitlements?market={market}&language=en-US&appVersion=0.2.3"
        );
        let mut request = client
            .get(url)
            .header("Authorization", &ticket)
            .header("X-MS-API-Version", "1.1")
            .header("Calling-App-Name", "Xodus-Nexus")
            .header("MS-CV", cv);
        if let Some(ref cursor) = continuation {
            request = request.header("x-ms-ct", cursor);
        }
        let mut response = request.send().await.map_err(|_| "entitlements-request")?;
        if !response.status().is_success() {
            diagnostic("entitlements-http", response.status().as_u16(), 0);
            return Err("entitlements-http");
        }
        continuation = match response.headers().get("x-ms-ct") {
            None => None,
            Some(value) => {
                let value = value.to_str().map_err(|_| "entitlements-cursor")?;
                if value.is_empty() {
                    None
                } else if value.len() > 4096 || !seen.insert(value.to_owned()) {
                    return Err("entitlements-pagination");
                } else {
                    Some(value.to_owned())
                }
            }
        };
        if response.content_length().is_some_and(|n| n > 4_194_304) {
            return Err("entitlements-size");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| "entitlements-read")? {
            total_bytes += chunk.len();
            if bytes.len() + chunk.len() > 4_194_304 || total_bytes > 16_777_216 {
                return Err("entitlements-size");
            }
            bytes.extend_from_slice(&chunk);
        }
        let page: Value = serde_json::from_slice(&bytes).map_err(|_| "entitlements-json")?;
        all.add(&page)?;
        if continuation.is_none() {
            tokens
                .validate_user_token_snapshot(&snapshot)
                .map_err(|_| "account-changed")?;
            all.finish();
            for (reason, count) in &all.reasons {
                diagnostic(reason, 0, *count);
            }
            diagnostic(
                if all.skipped > 0 {
                    "partial"
                } else {
                    "complete"
                },
                0,
                all.skipped,
            );
            return Ok(json!({"schema":1,"account":account,"market":market,
                "checkedAt":chrono::Utc::now().to_rfc3339(),"skippedEntitlements":all.skipped,
                "entitlements":all.entries.into_values().collect::<Vec<_>>()}));
        }
    }
    Err("entitlements-pagination")
}

/// The Store account's entitlements in `market` (two upper-case letters):
/// schema 1 with the account, market, check time, the verified rows and how
/// many were skipped. Bounded by a 45-second deadline. Errors are fixed stage
/// codes, never response text.
pub async fn entitlements(tokens: &TokenManager, market: &str) -> Result<Value, &'static str> {
    tokio::time::timeout(
        Duration::from_secs(45),
        query(tokens, &market.to_ascii_uppercase()),
    )
    .await
    .unwrap_or(Err("entitlements-timeout"))
}

#[cfg(test)]
mod tests {
    use super::*;
    const A: &str = "9AAAAAAAAAAA";
    const B: &str = "9BBBBBBBBBBB";
    const C: &str = "9CCCCCCCCCCC";
    fn valid() -> Value {
        json!({"status":"Active","isTrial":false,"sharingSource":"None","endDate":"2099-01-01T00:00:00Z"})
    }

    #[test]
    fn positron_provenance_is_explicit_boolean_and_never_copied_from_untrusted_fields() {
        for flag in [
            None,
            Some(json!(false)),
            Some(json!(true)),
            Some(Value::Null),
            Some(json!("true")),
            Some(json!(1)),
            Some(json!({"value":true})),
        ] {
            let mut raw = valid();
            if let Some(ref value) = flag {
                raw["isPositron"] = value.clone();
            }
            raw["special"] = json!(false);
            raw["licenseSource"] = json!("owned");
            let parsed = entry(A, &raw).unwrap();
            assert_eq!(
                parsed["isPositron"],
                flag.as_ref().and_then(Value::as_bool) == Some(true)
            );
            assert_eq!(
                parsed["special"],
                flag.as_ref().is_some_and(|v| v.as_bool() != Some(false))
            );
            assert!(parsed.get("licenseSource").is_none());
        }
    }

    #[test]
    fn conflicting_positron_and_unknown_special_rows_quarantine_their_children() {
        for changed in [json!(false), json!("true"), Value::Null] {
            let mut first = valid();
            first["isPositron"] = json!(true);
            let mut second = valid();
            second["isPositron"] = changed;
            let mut child = valid();
            child["satisfiedByProductId"] = json!(A);
            let mut result = Entitlements::default();
            result
                .add(&page(vec![(A, first), (B, child), (C, valid())]))
                .unwrap();
            result.add(&page(vec![(A, second)])).unwrap();
            result.finish();
            assert_eq!(
                result.entries.keys().cloned().collect::<Vec<_>>(),
                vec![C.to_owned()]
            );
            assert_eq!(result.skipped, 3);
        }
    }

    #[test]
    fn positron_normalized_protocol_fixture() {
        let mut rows = Vec::new();
        for (id, flag, status, parent) in [
            (A, json!(true), "Active", None),
            (B, json!(false), "Active", Some(A)),
            (C, json!(false), "Active", None),
            ("9DDDDDDDDDDD", json!("true"), "Active", None),
            ("9EEEEEEEEEEE", json!(true), "Revoked", None),
        ] {
            let mut raw = valid();
            raw["isPositron"] = flag;
            raw["status"] = json!(status);
            if let Some(parent) = parent {
                raw["satisfiedByProductId"] = json!(parent);
            }
            rows.push(entry(id, &raw).unwrap());
        }
        assert_eq!(rows[0]["isPositron"], true);
        assert_eq!(rows[3]["isPositron"], false);
        // Synthetic account only. Also exercises the compiled launcher reader.
        println!(
            "{}",
            json!({"schema":1,"account":"fixture@example.test","market":"US",
            "checkedAt":chrono::Utc::now().to_rfc3339(),"skippedEntitlements":0,"entitlements":rows})
        );
    }
    fn page(rows: Vec<(&str, Value)>) -> Value {
        json!({"entitlements": rows.into_iter().map(|(id, row)| (id.to_owned(), row)).collect::<serde_json::Map<String, Value>>()})
    }

    #[test]
    fn malformed_rows_do_not_hide_independent_licenses_or_become_owned() {
        for field in ["status", "isTrial", "sharingSource", "endDate"] {
            let mut bad = valid();
            bad.as_object_mut().unwrap().remove(field);
            let mut result = Entitlements::default();
            result
                .add(&page(vec![
                    (A, valid()),
                    (B, bad),
                    ("console-variant-id", valid()),
                ]))
                .unwrap();
            result.finish();
            assert_eq!(
                result.entries.keys().cloned().collect::<Vec<_>>(),
                vec![A.to_owned()]
            );
            assert_eq!(result.skipped, 2);
            assert!(result.reasons.values().sum::<usize>() >= 2);
        }
    }

    #[test]
    fn invalid_policy_types_and_parent_are_omitted_not_defaulted() {
        for (field, value) in [
            ("isTrial", json!("false")),
            ("sharingSource", Value::Null),
            ("endDate", json!("not-a-date")),
            ("satisfiedByProductId", json!("bad")),
        ] {
            let mut bad = valid();
            bad[field] = value;
            let mut result = Entitlements::default();
            result.add(&page(vec![(A, bad), (B, valid())])).unwrap();
            assert!(!result.entries.contains_key(A));
            assert!(result.entries.contains_key(B));
            assert_eq!(result.skipped, 1);
        }
    }

    #[test]
    fn conflicting_or_invalid_duplicate_is_quarantined_across_every_page() {
        for mut bad in [valid(), Value::Null] {
            if bad.is_object() {
                bad["isTrial"] = json!(true);
            }
            let mut result = Entitlements::default();
            result.add(&page(vec![(A, valid()), (B, valid())])).unwrap();
            result.add(&page(vec![(A, bad)])).unwrap();
            result.add(&page(vec![(A, valid())])).unwrap();
            result.finish();
            assert_eq!(result.entries.len(), 1);
            assert!(result.entries.contains_key(B));
            assert_eq!(result.skipped, 3);
        }
    }

    #[test]
    fn identical_repeated_row_is_not_a_conflict() {
        let mut result = Entitlements::default();
        result.add(&page(vec![(A, valid())])).unwrap();
        result.add(&page(vec![(A, valid())])).unwrap();
        result.finish();
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.skipped, 0);
    }

    #[test]
    fn rejects_envelopes_and_bounds_total_rows_across_pages() {
        for body in [
            Value::Null,
            json!({}),
            json!({"entitlements":[]}),
            json!({"entitlements":null}),
        ] {
            assert_eq!(Entitlements::default().add(&body), Err("entitlement-shape"));
        }
        let mut result = Entitlements {
            total: 10_000,
            ..Default::default()
        };
        assert_eq!(
            result.add(&page(vec![(A, valid())])),
            Err("entitlement-count")
        );
    }

    #[test]
    fn quarantines_dependent_chains_including_known_subscription_parents() {
        let plan = "CFQ7TTC0KGQ8";
        let mut child = valid();
        child["satisfiedByProductId"] = json!(plan);
        let mut descendant = valid();
        descendant["satisfiedByProductId"] = json!(A);
        let mut result = Entitlements::default();
        result
            .add(&page(vec![
                (A, child),
                (B, descendant),
                (C, valid()),
                (plan, Value::Null),
            ]))
            .unwrap();
        result.finish();
        assert_eq!(
            result.entries.keys().cloned().collect::<Vec<_>>(),
            vec![C.to_owned()]
        );
        assert_eq!(result.skipped, 3);
    }

    #[test]
    fn expired_trial_shared_and_unknown_state_rows_preserve_their_restrictions() {
        for (field, value) in [
            ("isTrial", json!(true)),
            ("sharingSource", json!("Family")),
            ("status", json!("FutureState")),
            ("endDate", json!("2000-01-01T00:00:00Z")),
            ("isPositron", json!(true)),
            ("isPositron", json!("unknown")),
        ] {
            let mut row = valid();
            row[field] = value.clone();
            let mut result = Entitlements::default();
            result.add(&page(vec![(A, row)])).unwrap();
            let actual = &result.entries[A];
            if field == "isPositron" {
                assert_eq!(actual["special"], true);
            } else {
                assert_eq!(actual[field], value);
            }
        }
    }
    #[test]
    fn keeps_only_license_fields_no_account_tokens_or_renewal_details() {
        let row = json!({"status":"Active","isTrial":false,"sharingSource":"None","endDate":"2099-01-01T00:00:00Z","satisfiedByProductId":"CFQ7TTC0KGQ8","secret":"not-output"});
        let mut parsed = Entitlements::default();
        parsed
            .add(&json!({"entitlements":{"9AAAAAAAAAAA":row}}))
            .unwrap();
        let result = parsed.entries;
        assert_eq!(
            result["9AAAAAAAAAAA"]["satisfiedByProductId"],
            "CFQ7TTC0KGQ8"
        );
        assert!(result["9AAAAAAAAAAA"].get("secret").is_none());
        assert!(parsed.skipped == 0);
        assert!(Entitlements::default().add(&json!({})).is_err());
        let mut invalid = Entitlements::default();
        invalid.add(&json!({"entitlements":{"bad":{}}})).unwrap();
        assert!(invalid.entries.is_empty());
        assert_eq!(invalid.skipped, 1);
    }
}
