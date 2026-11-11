//! Optional Store membership read using existing credentials, never an auth UI.
use crate::{
    models::{live::ExchangeUserTokenOutcome, secrets::Token, soap},
    tokens::TokenManager,
};
use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
use serde_json::{Value, json};
use std::time::Duration;

/// Answers kept for [`TTL`] per account binding; Xodus keeps them in the
/// system keyring, Irori in memory for the session.
static CACHE: std::sync::Mutex<Option<(String, Value)>> = std::sync::Mutex::new(None);
const TTL: i64 = 600;
const PLANS: &[&str] = &[
    "CFQ7TTC0KGQ8",
    "CFQ7TTC0KHS0",
    "CFQ7TTC0K5DJ",
    "CFQ7TTC0P85B",
    "CFQ7TTC0K6L8",
];
const URL: &str = "https://beige.xboxservices.com/PCGAFD/entitlements/subscriptions?market=US&language=en-US&appVersion=0.2.3";

fn unknown() -> Value {
    json!({"status":"unknown","productIds":[],"checkedAt":chrono::Utc::now().to_rfc3339()})
}

pub fn clear() {
    if let Ok(mut cache) = CACHE.lock() {
        *cache = None;
    }
}

fn summary(body: &Value, now: i64) -> Result<(Value, i64), &'static str> {
    let entries = body["entitlements"].as_object().ok_or("membership-shape")?;
    if entries.len() > PLANS.len() {
        return Err("membership-count");
    }
    let mut ids = Vec::new();
    let mut valid_until = now + TTL;
    for (id, item) in entries {
        if !PLANS.contains(&id.as_str()) {
            return Err("membership-product");
        }
        let status = item["status"].as_str().ok_or("membership-state")?;
        if status != "Active" {
            continue;
        }
        let expires = item["endDate"]
            .as_str()
            .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())
            .map(|date| date.timestamp())
            .ok_or("membership-expiry")?;
        if expires <= now {
            continue;
        }
        valid_until = valid_until.min(expires);
        ids.push(id.clone());
    }
    // Empty/unsupported data is not proof that this account has no subscription.
    if ids.is_empty() {
        return Err("membership-unverified");
    }
    let checked = chrono::DateTime::from_timestamp(now, 0).ok_or("clock")?;
    Ok((
        json!({"status":"active","productIds":ids,"checkedAt":checked.to_rfc3339()}),
        valid_until,
    ))
}

fn cached(value: &Value, account: &str, now: i64) -> Option<Value> {
    if value["schema"] != 2 || value["account"].as_str() != Some(account) {
        return None;
    }
    let sub = &value["subscription"];
    let checked = sub["checkedAt"]
        .as_str()
        .and_then(|date| chrono::DateTime::parse_from_rfc3339(date).ok())?
        .timestamp();
    let until = value["validUntil"].as_i64()?;
    if checked > now
        || now - checked >= TTL
        || until <= now
        || until > checked + TTL
        || sub["status"] != "active"
    {
        return None;
    }
    let ids = sub["productIds"].as_array()?;
    if ids.is_empty() || ids.len() > PLANS.len() {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    if !ids.iter().all(|id| {
        id.as_str()
            .is_some_and(|id| PLANS.contains(&id) && seen.insert(id))
    }) {
        return None;
    }
    Some(sub.clone())
}

pub(super) async fn store_session(
    tokens: &TokenManager,
    account: &str,
) -> Result<(reqwest::Client, String, crate::tokens::UserTokenSnapshot), &'static str> {
    if tokens
        .get_license_user()
        .map_err(|_| "stored-account")?
        .username
        != account
    {
        return Err("account-changed");
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(super::session::REQUEST_TIMEOUT)
        .user_agent("Xodus-Nexus-membership/0.2.3")
        .build()
        .map_err(|_| "http-client")?;
    let (selected, device, user, snapshot) = super::session::prepare_license(tokens, &client)
        .await
        .inspect_err(|&stage| super::session::diagnostic(stage))?;
    if selected != account {
        return Err("account-changed");
    }
    // Genuine Microsoft Store ticket, not Xbox privileges, browser cookies or
    // a first-party client impersonation. Silent failure never opens another UI.
    let outcome = super::session::exchange(|| async {
        crate::api::live::exchange_user_token(
            &client,
            user.clone(),
            account.into(),
            device.clone(),
            None,
            Some("Silent".into()),
            "{d6d5a677-0872-4ab0-9442-bb792fce85c5}".into(),
            &[(
                "www.microsoft.com".into(),
                Some(soap::PolicyReference::mbi_ssl()),
            )],
        )
        .await
        .map_err(|error| super::session::classify(&error))
    })
    .await?;
    tokens
        .validate_user_token_snapshot(&snapshot)
        .map_err(|_| "account-changed")?;
    if let Some(stage) = super::session::rejection(&outcome) {
        return Err(stage);
    }
    let token: Token = match outcome {
        ExchangeUserTokenOutcome::Issued(
            soap::BodyContent::RequestSecurityTokenResponseCollection(mut collection),
        ) if collection.security_tokens.len() == 1 => {
            Token::try_from(collection.security_tokens.remove(0))
                .map_err(|_| "ticket-response-parse")?
        }
        ExchangeUserTokenOutcome::Issued(soap::BodyContent::RequestSecurityTokenResponse(
            token,
        )) => Token::try_from(*token).map_err(|_| "ticket-response-parse")?,
        _ => return Err("store-ticket-refused"),
    };
    let Token::Compact(ticket) = token else {
        return Err("store-ticket-type");
    };
    let ticket = valid_store_ticket(ticket).ok_or("store-ticket-format")?;
    Ok((client, ticket, snapshot))
}

/// Delegation-compact tickets have no "&p=" part and arrive as "d=..."; both
/// ticket kinds work for the Store once sent as "t=...".
fn valid_store_ticket(ticket: String) -> Option<String> {
    let ticket = crate::models::secrets::store_ticket(ticket);
    (ticket.len() > 2 && ticket.starts_with("t=") && !ticket.contains(char::is_whitespace))
        .then_some(ticket)
}

async fn query(tokens: &TokenManager, account: &str) -> Result<(Value, i64), &'static str> {
    let (client, ticket, snapshot) = store_session(tokens, account).await?;
    let cv = format!(
        "{}.0",
        STANDARD_NO_PAD.encode(uuid::Uuid::new_v4().as_bytes())
    );
    let mut response = client
        .post(URL)
        .header("Authorization", ticket)
        .header("X-MS-API-Version", "1.1")
        .header("MS-CV", cv)
        .header("Calling-App-Name", "Xodus-Nexus")
        .json(&json!({"productIds":PLANS}))
        .send()
        .await
        .map_err(|_| "membership-request")?;
    if !response.status().is_success() {
        return Err("membership-http");
    }
    if response.content_length().is_some_and(|size| size > 262_144) {
        return Err("membership-size");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "membership-read")? {
        if bytes.len() + chunk.len() > 262_144 {
            return Err("membership-size");
        }
        bytes.extend_from_slice(&chunk);
    }
    tokens
        .validate_user_token_snapshot(&snapshot)
        .map_err(|_| "account-changed")?;
    let body: Value = serde_json::from_slice(&bytes).map_err(|_| "membership-json")?;
    summary(&body, chrono::Utc::now().timestamp())
}

pub async fn lookup(tokens: &TokenManager, account: &str) -> Value {
    let Ok(snapshot) = tokens.user_token_snapshot_role(crate::tokens::AccountRole::License) else {
        return unknown();
    };
    if snapshot.stored_user().is_none_or(|u| u.username != account) {
        return unknown();
    }
    let binding = snapshot.cache_binding();
    let stored = CACHE.lock().ok().and_then(|cache| {
        cache
            .as_ref()
            .filter(|(b, _)| *b == binding)
            .map(|(_, v)| v.clone())
    });
    if let Some(value) =
        stored.and_then(|value| cached(&value, account, chrono::Utc::now().timestamp()))
    {
        return if tokens.validate_user_token_snapshot(&snapshot).is_ok() {
            value
        } else {
            unknown()
        };
    }
    match tokio::time::timeout(Duration::from_secs(40), query(tokens, account)).await {
        Ok(Ok((sub, until))) => {
            if tokens.validate_user_token_snapshot(&snapshot).is_err() {
                return unknown();
            }
            if let Ok(mut cache) = CACHE.lock() {
                *cache = Some((
                    binding,
                    json!({"schema":2,"account":account,"subscription":sub,"validUntil":until}),
                ));
            }
            sub
        }
        _ => unknown(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const NOW: i64 = 1_800_000_000;
    fn active() -> Value {
        json!({"entitlements":{"CFQ7TTC0KGQ8":{"status":"Active","endDate":chrono::DateTime::from_timestamp(NOW+3600,0).unwrap().to_rfc3339(),"autoRenew":true}}})
    }
    #[test]
    fn store_ticket_accepts_compact_and_delegation_compact_forms() {
        assert_eq!(
            valid_store_ticket("t=abc&p=".into()).as_deref(),
            Some("t=abc&p=")
        );
        assert_eq!(valid_store_ticket("d=abc".into()).as_deref(), Some("t=abc"));
        for bad in ["", "t=", "d=", "abc", "t=a b", "x=abc"] {
            assert_eq!(valid_store_ticket(bad.into()), None, "{bad}");
        }
    }
    #[test]
    fn real_membership_shape_gives_pc_plan() {
        let (sub, until) = summary(&active(), NOW).unwrap();
        assert_eq!(sub["productIds"], json!(["CFQ7TTC0KGQ8"]));
        assert_eq!(until, NOW + TTL);
    }
    #[test]
    fn stale_cancelled_unsupported_and_malformed_are_not_active() {
        for value in [
            json!({}),
            json!({"entitlements":{}}),
            json!({"entitlements":{"unknown":{"status":"Active"}}}),
            json!({"entitlements":{"CFQ7TTC0KGQ8":{"status":"Expired"}}}),
        ] {
            assert!(summary(&value, NOW).is_err());
        }
        let mut value = active();
        value["entitlements"]["CFQ7TTC0KGQ8"]["endDate"] = json!("bad");
        assert!(summary(&value, NOW).is_err());
        assert!(summary(&active(), NOW + 3601).is_err());
    }
    #[test]
    fn cache_is_account_bound_short_lived_and_strict() {
        let (sub, until) = summary(&active(), NOW).unwrap();
        let mut value =
            json!({"schema":2,"account":"one@example.test","subscription":sub,"validUntil":until});
        assert!(cached(&value, "one@example.test", NOW + 1).is_some());
        assert!(cached(&value, "other@example.test", NOW + 1).is_none());
        assert!(cached(&value, "one@example.test", NOW - 1).is_none());
        assert!(cached(&value, "one@example.test", NOW + TTL).is_none());
        value["subscription"]["productIds"] = json!(["CFQ7TTC0KGQ8", "CFQ7TTC0KGQ8"]);
        assert!(cached(&value, "one@example.test", NOW + 1).is_none());
    }
    #[test]
    fn cache_does_not_outlive_actual_expiry() {
        let mut value = active();
        value["entitlements"]["CFQ7TTC0KGQ8"]["endDate"] = json!(
            chrono::DateTime::from_timestamp(NOW + 30, 0)
                .unwrap()
                .to_rfc3339()
        );
        let (sub, until) = summary(&value, NOW).unwrap();
        assert_eq!(until, NOW + 30);
        let cache = json!({"schema":2,"account":"one","subscription":sub,"validUntil":until});
        assert!(cached(&cache, "one", NOW + 30).is_none());
    }
}
