use crate::models::xbox::{
    UserAuthProperties, UserAuthRequest, XstsPropertyBag, XstsRequest, XstsResponse,
};

#[path = "auth_diagnostics.rs"]
mod diagnostics;

pub async fn authenticate_xbox_user(
    client: &reqwest::Client,
    rps_ticket: String,
) -> reqwest::Result<XstsResponse> {
    let resp = xbox_user_request(client, rps_ticket)
        .send()
        .await
        .map_err(|error| diagnostics::request(diagnostics::Stage::User, error))?;

    diagnostics::response(diagnostics::Stage::User, resp).await
}

fn xbox_user_request(client: &reqwest::Client, rps_ticket: String) -> reqwest::RequestBuilder {
    let body = UserAuthRequest {
        relying_party: "http://auth.xboxlive.com".to_string(),
        token_type: "JWT".to_string(),
        properties: UserAuthProperties {
            auth_method: "RPS".to_string(),
            site_name: "user.auth.xboxlive.com".to_string(),
            rps_ticket,
        },
    };

    client
        .post("https://user.auth.xboxlive.com/user/authenticate")
        .header("Content-Type", "application/json")
        .header("x-xbl-contract-version", "1")
        .json(&body)
}

pub async fn request_xsts_token(
    client: &reqwest::Client,
    token: String,
    relying_party: &str,
) -> reqwest::Result<XstsResponse> {
    let body = XstsRequest {
        relying_party: Some(relying_party.to_string()),
        token_type: Some("JWT".to_string()),
        properties: XstsPropertyBag {
            user_tokens: Some(vec![token]),
            sandbox_id: Some("RETAIL".to_string()),
            delegation_token: None,
            service_token: None,
        },
    };

    let resp = client
        .post("https://xsts.auth.xboxlive.com/xsts/authorize")
        .header("Content-Type", "application/json")
        .header("x-xbl-contract-version", "1")
        .json(&body)
        .send()
        .await
        .map_err(|error| diagnostics::request(diagnostics::Stage::Xsts, error))?;

    diagnostics::response(diagnostics::Stage::Xsts, resp).await
}

pub fn get_xsts_auth_header(
    xsts: XstsResponse,
) -> Result<String, crate::models::secrets::InvalidTokenResponse> {
    use crate::models::secrets::InvalidTokenResponse;
    let uhs = xsts
        .user_hash()
        .filter(|value| !value.is_empty())
        .ok_or(InvalidTokenResponse)?;
    if xsts.token.is_empty() {
        return Err(InvalidTokenResponse);
    }
    Ok(format!("XBL3.0 x={uhs};{}", xsts.token))
}

#[cfg(test)]
mod ticket_tests {
    use super::*;
    use crate::models::{secrets::Token, soap};
    use base64_23::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use serde_json::json;

    #[test]
    fn xbox_delegation_ticket_reaches_the_real_user_request_without_an_extra_prefix() {
        let jwe = format!(
            "{}..{}.{}.{}",
            URL_SAFE_NO_PAD.encode(json!({"alg":"dir","enc":"A256GCM"}).to_string()),
            URL_SAFE_NO_PAD.encode("iv"),
            URL_SAFE_NO_PAD.encode("ciphertext"),
            URL_SAFE_NO_PAD.encode("tag")
        );
        let client = reqwest::Client::new();
        for (kind, raw, expected) in [
            (
                "urn:passport:delegationcompact",
                jwe.clone(),
                format!("t={jwe}"),
            ),
            (
                "urn:passport:delegationcompact",
                "opaque".into(),
                "d=opaque".into(),
            ),
            (
                "urn:passport:compact",
                "t=opaque&p=proof".into(),
                "t=opaque&p=proof".into(),
            ),
        ] {
            let response: soap::RequestSecurityTokenResponse = serde_json::from_value(json!({
                "TokenType":kind,"AppliesTo":{"EndpointReference":{"Address":"user.auth.xboxlive.com"}},
                "Lifetime":{"Created":"2026-10-28T00:00:00Z","Expires":"2026-10-29T00:00:00Z"},
                "RequestedSecurityToken":{"BinarySecurityToken":{"@Id":"fixture","$value":raw}}
            })).unwrap();
            let Token::Compact(ticket) = Token::try_from(response).unwrap() else {
                panic!("fixture");
            };
            let request = xbox_user_request(&client, ticket).build().unwrap();
            assert_eq!(
                request.url().as_str(),
                "https://user.auth.xboxlive.com/user/authenticate"
            );
            assert_eq!(request.headers()["x-xbl-contract-version"], "1");
            let body: serde_json::Value =
                serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
            assert_eq!(body["Properties"]["RpsTicket"], expected);
            assert_eq!(body["Properties"]["AuthMethod"], "RPS");
            assert_eq!(body["RelyingParty"], "http://auth.xboxlive.com");
        }
    }
}
