//! Synthetic response parsing only, no account, keyring or network access.
use super::*;
use crate::models::secrets::InvalidTokenResponse;
use base64_23::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::json;

fn response(kind: &str, value: &str) -> soap::RequestSecurityTokenResponse {
    let mut response: soap::RequestSecurityTokenResponse = serde_json::from_value(json!({
        "TokenType":kind,
        "AppliesTo":{"EndpointReference":{"Address":"http://Passport.NET/tb"}},
        "Lifetime":{"Created":"2026-10-28T00:00:00Z","Expires":"2026-10-29T00:00:00Z"},
        "RequestedSecurityToken":{"BinarySecurityToken":{"@Id":"fixture","$value":value}}
    }))
    .unwrap();
    if kind == "urn:passport:legacy" {
        response.requested_security_token.binary_security_token = None;
        response.requested_security_token.encrypted_data = Some(
            soap::EncryptedData::devicesoftware("synthetic-ciphertext".into()),
        );
    }
    response
}

fn collection(responses: Vec<soap::RequestSecurityTokenResponse>) -> ExchangeUserTokenOutcome {
    ExchangeUserTokenOutcome::Issued(soap::BodyContent::RequestSecurityTokenResponseCollection(
        soap::RequestSecurityTokenResponseCollection {
            security_tokens: responses,
        },
    ))
}

#[test]
fn credential_sisu_selects_compact_and_refresh_tokens_independently_of_response_order() {
    for reverse in [false, true] {
        let mut responses = vec![
            response("urn:passport:compact", "t=synthetic&p=proof"),
            response("urn:passport:legacy", ""),
        ];
        if reverse {
            responses.reverse();
        }
        let (ticket, refresh) = sisu_user_tokens(collection(responses)).unwrap();
        assert_eq!(ticket, "t=synthetic&p=proof");
        let (address, token) = refresh.unwrap();
        assert_eq!(address, "http://Passport.NET/STS");
        assert!(matches!(token, Token::Legacy(_)));
    }
}

#[test]
fn credential_sisu_keeps_jwe_opaque_and_single_response_ticket_formats() {
    let jwe = format!(
        "{}..{}.{}.{}",
        URL_SAFE_NO_PAD.encode(json!({"alg":"dir","enc":"A256GCM"}).to_string()),
        URL_SAFE_NO_PAD.encode("iv"),
        URL_SAFE_NO_PAD.encode("ciphertext"),
        URL_SAFE_NO_PAD.encode("tag")
    );
    for (raw, expected) in [
        (jwe.clone(), format!("t={jwe}")),
        ("opaque".into(), "d=opaque".into()),
    ] {
        for single in [false, true] {
            let value = response("urn:passport:delegationcompact", &raw);
            let outcome = if single {
                ExchangeUserTokenOutcome::Issued(soap::BodyContent::RequestSecurityTokenResponse(
                    Box::new(value),
                ))
            } else {
                collection(vec![value])
            };
            let (ticket, refresh) = sisu_user_tokens(outcome).unwrap();
            assert_eq!(ticket, expected);
            assert!(refresh.is_none());
        }
    }
}

#[test]
fn credential_sisu_rejects_missing_duplicate_unknown_and_malformed_tokens_without_payload_errors() {
    for responses in [
        vec![],
        vec![response("urn:passport:legacy", "")],
        vec![
            response("urn:passport:compact", "one"),
            response("urn:passport:compact", "two"),
        ],
        vec![
            response("urn:passport:legacy", ""),
            response("urn:passport:compact", "one"),
            response("urn:passport:legacy", ""),
        ],
        vec![response("unexpected-private-value", "private-value")],
        vec![response("urn:passport:compact", "")],
        vec![response("urn:passport:delegationcompact", "")],
    ] {
        assert!(matches!(
            sisu_user_tokens(collection(responses)),
            Err(InvalidTokenResponse)
        ));
    }
    let mut malformed = response("urn:passport:compact", "private-value");
    malformed.requested_security_token.binary_security_token = None;
    assert!(sisu_user_tokens(collection(vec![malformed])).is_err());
    assert!(sisu_user_tokens(ExchangeUserTokenOutcome::Fault(None)).is_err());
    assert!(!InvalidTokenResponse.to_string().contains("private-value"));
}

#[test]
fn credential_sisu_checks_all_expiries_and_selects_the_ticket_lifetime() {
    let compact = response("urn:passport:compact", "synthetic-ticket");
    let expected = chrono::DateTime::parse_from_rfc3339(&compact.lifetime.expires)
        .unwrap()
        .timestamp();
    let mut legacy = response("urn:passport:legacy", "");
    legacy.lifetime.expires = "2029-01-01T00:00:00Z".into();
    let selected = issued_user_tokens(collection(vec![legacy, compact])).unwrap();
    assert_eq!(selected.expiry, expected);
    for bad_legacy in [false, true] {
        let mut compact = response("urn:passport:compact", "synthetic-ticket");
        let mut legacy = response("urn:passport:legacy", "");
        if bad_legacy {
            legacy.lifetime.expires = "invalid".into();
        } else {
            compact.lifetime.expires = "invalid".into();
        }
        assert!(issued_user_tokens(collection(vec![compact, legacy])).is_err());
    }
}
