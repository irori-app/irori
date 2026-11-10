//! Public synthetic data only, no account, keyring or network access.
use super::{Token, licensing_ticket, store_ticket};
use crate::models::soap;
use base64_23::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::json;

fn jwe(header: serde_json::Value, encrypted_key: &str) -> String {
    format!(
        "{}.{}.{}.{}.{}",
        URL_SAFE_NO_PAD.encode(header.to_string()),
        encrypted_key,
        URL_SAFE_NO_PAD.encode(b"synthetic-iv"),
        URL_SAFE_NO_PAD.encode(b"synthetic-ciphertext"),
        URL_SAFE_NO_PAD.encode(b"synthetic-tag")
    )
}

fn delegation(raw: &str) -> Token {
    let response: soap::RequestSecurityTokenResponse = serde_json::from_value(json!({
        "TokenType":"urn:passport:delegationcompact",
        "AppliesTo":{"EndpointReference":{"Address":"www.microsoft.com"}},
        "Lifetime":{"Created":"2026-10-22T00:00:00Z","Expires":"2026-10-23T00:00:00Z"},
        "RequestedSecurityToken":{"BinarySecurityToken":{"@Id":"fixture","$value":raw}}
    }))
    .unwrap();
    Token::try_from(response).unwrap()
}

#[test]
fn content_ticket_jwe_is_bare_only_at_the_licensing_boundary() {
    for raw in [
        jwe(json!({"alg":"dir","enc":"A128CBC-HS256"}), ""),
        jwe(json!({"alg":"RSA-OAEP-256","enc":"A256GCM"}), "a2V5"),
    ] {
        let Token::Compact(original) = delegation(&raw) else {
            panic!("fixture");
        };
        assert_eq!(
            original,
            format!("t={raw}"),
            "Xbox sign-in gets compact JWE tickets as t="
        );
        assert_eq!(
            store_ticket(original.clone()),
            format!("t={raw}"),
            "Store behavior is unchanged"
        );
        assert_eq!(licensing_ticket(original), raw);
        assert_eq!(licensing_ticket(raw.clone()), raw);
        assert_eq!(licensing_ticket(format!("t={raw}")), raw);
    }
}

#[test]
fn content_ticket_opaque_compact_and_delegation_forms_stay_byte_exact() {
    for (input, expected) in [
        ("t=opaque&p=proof", "t=opaque&p=proof"),
        ("d=opaque", "t=opaque"),
        ("opaque", "opaque"),
        ("", ""),
        ("d=", "t="),
        ("d=eyJ.abc", "t=eyJ.abc"),
        (
            "d=eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiIxIn0.c2ln",
            "t=eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiIxIn0.c2ln",
        ),
    ] {
        assert_eq!(licensing_ticket(input.into()), expected);
    }
    assert!(matches!(delegation("opaque"), Token::Compact(value) if value == "d=opaque"));
}

#[test]
fn content_ticket_dotted_or_malformed_values_are_not_reinterpreted_as_jwe() {
    let valid = jwe(json!({"alg":"dir","enc":"A256GCM"}), "");
    let mut invalid = vec![
        "opaque..iv.ciphertext.tag".to_string(),
        "eyJ..a.b.c".into(),
        format!("{valid}.extra"),
        valid.replace("..", ".+."),
        valid.replace("..", ".=."),
    ];
    for header in [
        json!(null),
        json!([]),
        json!({}),
        json!({"alg":"dir"}),
        json!({"alg":"dir","enc":""}),
        json!({"alg":17,"enc":"A256GCM"}),
    ] {
        invalid.push(jwe(header, ""));
    }
    let mut parts: Vec<_> = valid.split('.').map(str::to_string).collect();
    for index in [2, 3, 4] {
        let before = parts[index].clone();
        parts[index].clear();
        invalid.push(parts.join("."));
        parts[index] = before;
    }
    invalid.push(valid.replace(".", ". "));
    for raw in invalid {
        assert_eq!(licensing_ticket(format!("d={raw}")), format!("t={raw}"));
        assert_eq!(licensing_ticket(format!("t={raw}")), format!("t={raw}"));
    }
}

#[test]
fn xbox_delegation_ticket_prefix_follows_the_ticket_encoding() {
    let raw = jwe(json!({"alg":"dir","enc":"A256GCM"}), "");
    assert!(matches!(delegation(&raw), Token::Compact(value) if value == format!("t={raw}")));
    for opaque in [
        "opaque",
        "EwA4A+synthetic/opaque==",
        "eyJ.abc",
        "opaque..iv.ciphertext.tag",
        "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiIxIn0.c2ln",
    ] {
        assert!(
            matches!(delegation(opaque), Token::Compact(value) if value == format!("d={opaque}")),
            "opaque and signed-only tickets keep the exact previous d= form"
        );
    }
}

#[test]
fn content_ticket_formatting_does_not_decode_or_rewrite_ciphertext() {
    let raw = jwe(
        json!({"kid":"public-synthetic","enc":"A256GCM","alg":"dir"}),
        "",
    );
    assert_eq!(licensing_ticket(format!("d={raw}")), raw);
    let long_header = jwe(
        json!({"alg":"dir","enc":"A256GCM","kid":"a".repeat(9000)}),
        "",
    );
    assert_eq!(
        licensing_ticket(format!("d={long_header}")),
        format!("t={long_header}")
    );
}
