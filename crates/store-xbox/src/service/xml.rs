use crate::service::XodusMessageType;
use irori_msstore_api::models::live::ExchangeUserTokenOutcome;
use irori_msstore_api::models::secrets::Token;
use irori_msstore_api::models::soap;
use irori_msstore_api::models::xgameruntime::xuser::{MSATokenRequest, MSATokenResponse};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::service::XML_MAGIC;
use crate::service::context::SimpleContext;

pub async fn handle(
    socket: &mut tokio::net::UnixStream,
    context: &mut SimpleContext,
) -> tokio::io::Result<()> {
    tracing::debug!("Parsing XML");
    let message_type = socket.read_u16_le().await?;
    let message_size = socket.read_u16_le().await?;
    let mut buffer = vec![0; message_size as usize];
    tracing::debug!("Reading buffer {message_size}");
    socket.read_exact(&mut buffer).await?;
    tracing::debug!("Read buffer");
    let message_type = XodusMessageType::try_from(message_type as i32).unwrap_or_default();

    let out_buf = match parse_message(context, message_type, buffer).await {
        Ok(buf) => buf,
        Err(err) => {
            tracing::error!("Failed parsing message: {err}");
            vec![]
        }
    };

    let data = crate::service::encode_message(XML_MAGIC, message_type as u16 + 1, out_buf);
    socket.write_all(&data).await
}

pub async fn parse_message(
    context: &mut SimpleContext,
    message_type: XodusMessageType,
    buffer: Vec<u8>,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    match message_type {
        XodusMessageType::Ping => Ok(buffer),
        XodusMessageType::StorePackageUpdatesRequest => {
            let result = crate::service::store_updates::handle(context, &buffer).await;
            Ok(quick_xml::se::to_string(&result)?.into_bytes())
        }
        XodusMessageType::StoreLicenseTokenRequest => {
            // Dedicated request, never silently reinterpret it as Xbox sign-in.
            let result = crate::service::store_license::handle(context, &buffer).await;
            Ok(quick_xml::se::to_string(&result)?.into_bytes())
        }
        XodusMessageType::MsaTokenRequest => {
            tracing::debug!("Raw buffer: {buffer:?}");
            let string_buf = std::str::from_utf8(&buffer)?;
            tracing::debug!("String buffer: {string_buf:?}");
            let req = quick_xml::de::from_str::<MSATokenRequest>(string_buf)?;
            let (mut identity_session, store_session) =
                context.tokens().runtime_identity_snapshots()?;
            let request_token = identity_session.token();
            let Token::Legacy(token) = request_token else {
                return Ok(vec![]);
            };
            let scope = if req.msa_full_trust {
                "service::user.auth.xboxlive.com::MBI_SSL"
            } else {
                "xboxlive.signin"
            };
            let device_token = context
                .device_token
                .as_ref()
                .ok_or(irori_msstore_api::models::secrets::InvalidTokenResponse)?;
            let device_token_resp = irori_msstore_api::api::live::exchange_device_token(
                &context.client,
                device_token.clone(),
                "{28C08266-F973-4AE6-FFE4-409B249F138F}".to_string(),
                "scope=service::user.auth.xboxlive.com::MBI_SSL".to_owned(),
                Some(soap::PolicyReference::token_broker()),
            )
            .await;

            let ms_device_rps_token =
                if let Some((Ok(Token::Compact(ms_device_token)), Ok(lifetime))) =
                    device_token_resp.ok().map(|t| {
                        let expiry = chrono::DateTime::parse_from_rfc3339(&t.lifetime.expires);
                        (Token::try_from(t), expiry)
                    })
                {
                    Some((ms_device_token, lifetime.timestamp()))
                } else {
                    None
                };

            let user_token = irori_msstore_api::api::live::exchange_user_token(
                &context.client,
                token,
                "USERNAME".to_string(),
                device_token.clone(),
                None,
                Some("Silent".to_string()),
                req.client_id.clone(),
                &[
                    (
                        format!("scope={scope}&api-version=2.0&clientid={}", req.client_id),
                        Some(soap::PolicyReference::token_broker()),
                    ),
                    ("http://Passport.NET/tb".to_string(), None),
                ],
            )
            .await?;

            let (user_token, expiry) =
                accept_user_tokens(context, &mut identity_session, user_token)?;
            // Store licensing uses this same session facade. Bind the
            // Xbox ticket's MSA principal to its current Store account,
            // never infer either account identifier from an Xbox XUID.
            context
                .tokens()
                .validate_user_token_snapshot(&identity_session)?;
            if let Some(store) = &store_session {
                context.tokens().validate_user_token_snapshot(store)?;
            }
            let (xbox_msa_account_id, store_msa_account_id) =
                crate::service::account_metadata::bound_ids(
                    identity_session.user(),
                    store_session.as_ref().and_then(|session| session.user()),
                );
            let payload = MSATokenResponse {
                token: user_token,
                expiry,
                device_expiry: ms_device_rps_token.as_ref().map(|(_, r)| *r).unwrap_or(0),
                xbox_msa_account_id,
                store_msa_account_id,
                device_rps: ms_device_rps_token
                    .map(|(t, _)| t)
                    .unwrap_or_else(String::new),
            };
            let payload = quick_xml::se::to_string(&payload)?;
            Ok(payload.as_bytes().to_vec())
        }
        _ => Err("Unimplemented".into()),
    }
}

fn accept_user_tokens(
    context: &SimpleContext,
    snapshot: &mut irori_msstore_api::tokens::manager::UserTokenSnapshot,
    outcome: ExchangeUserTokenOutcome,
) -> Result<(String, i64), Box<dyn std::error::Error + Send + Sync>> {
    // Parse every token and its expiry before any credential-store mutation.
    let selected = irori_msstore_api::auth::issued_user_tokens(outcome)?;
    context
        .tokens()
        .accept_user_token_refresh(snapshot, selected.refreshed)?;
    Ok((selected.token, selected.expiry))
}

#[cfg(test)]
mod token_response_tests {
    use super::*;
    use irori_msstore_api::tokens::{TokenManager, manager::PASSPORT_STS};
    use serde_json::json;
    use std::sync::Arc;

    fn accept_user_tokens(
        context: &SimpleContext,
        outcome: ExchangeUserTokenOutcome,
    ) -> Result<(String, i64), Box<dyn std::error::Error + Send + Sync>> {
        let mut snapshot = context.tokens().user_token_snapshot()?;
        super::accept_user_tokens(context, &mut snapshot, outcome)
    }

    fn response(kind: &str, value: &str) -> soap::RequestSecurityTokenResponse {
        let mut result: soap::RequestSecurityTokenResponse = serde_json::from_value(json!({
            "TokenType": kind, "AppliesTo":{"EndpointReference":{"Address":"http://Passport.NET/tb"}},
            "Lifetime":{"Created":"2026-10-28T00:00:00Z","Expires":"2027-10-29T00:00:00Z"},
            "RequestedSecurityToken":{"BinarySecurityToken":{"@Id":"fixture","$value":value}}
        })).unwrap();
        if kind == "urn:passport:legacy" {
            result.requested_security_token.binary_security_token = None;
            result.requested_security_token.encrypted_data = Some(
                soap::EncryptedData::devicesoftware("new-synthetic-ciphertext".into()),
            );
        }
        result
    }
    fn context() -> SimpleContext {
        let tokens = Arc::new(TokenManager::with_memory());
        tokens
            .save_user_token(
                PASSPORT_STS.into(),
                Token::Compact("previous-synthetic-ticket".into()),
            )
            .unwrap();
        let device = irori_msstore_api::models::secrets::LegacyToken {
            key_name: None,
            token: "unused-synthetic-device".into(),
            binary_secret: None,
            tpm_key: None,
            lifetime: soap::Timestamp {
                id: None,
                created: "2026-01-01T00:00:00Z".into(),
                expires: "2027-01-01T00:00:00Z".into(),
            },
        };
        SimpleContext::new(device, tokens)
    }
    fn collection(values: Vec<soap::RequestSecurityTokenResponse>) -> ExchangeUserTokenOutcome {
        ExchangeUserTokenOutcome::Issued(soap::BodyContent::RequestSecurityTokenResponseCollection(
            soap::RequestSecurityTokenResponseCollection {
                security_tokens: values,
            },
        ))
    }
    #[test]
    fn token_response_selects_types_in_both_orders() {
        for reverse in [false, true] {
            let context = context();
            let mut values = vec![
                response("urn:passport:compact", "t=synthetic&proof=value"),
                response("urn:passport:legacy", ""),
            ];
            if reverse {
                values.reverse();
            }
            let (ticket, expiry) = accept_user_tokens(&context, collection(values)).unwrap();
            assert_eq!(ticket, "t=synthetic&proof=value");
            assert!(expiry > 0);
            assert!(matches!(
                context.tokens().get_user_sts_token().unwrap(),
                Token::Legacy(_)
            ));
        }
    }
    #[test]
    fn token_response_invalid_expiry_does_not_replace_credentials() {
        let context = context();
        let mut compact = response("urn:passport:compact", "synthetic");
        compact.lifetime.expires = "invalid".into();
        assert!(
            accept_user_tokens(
                &context,
                collection(vec![compact, response("urn:passport:legacy", "")])
            )
            .is_err()
        );
        assert!(
            matches!(context.tokens().get_user_sts_token().unwrap(), Token::Compact(value) if value == "previous-synthetic-ticket")
        );
    }
    #[test]
    fn token_response_ambiguous_reply_does_not_replace_credentials() {
        let context = context();
        let values = vec![
            response("urn:passport:compact", "one"),
            response("urn:passport:compact", "two"),
            response("urn:passport:legacy", ""),
        ];
        assert!(accept_user_tokens(&context, collection(values)).is_err());
        assert!(
            matches!(context.tokens().get_user_sts_token().unwrap(), Token::Compact(value) if value == "previous-synthetic-ticket")
        );
    }

    #[test]
    fn token_response_rejects_incomplete_replies_before_persistence() {
        let mut invalid_refresh = response("urn:passport:legacy", "");
        invalid_refresh.lifetime.expires = "invalid".into();
        for values in [
            vec![],
            vec![response("urn:passport:legacy", "")],
            vec![
                response("urn:passport:compact", ""),
                response("urn:passport:legacy", ""),
            ],
            vec![
                response("unknown-synthetic-type", "private-fixture"),
                response("urn:passport:legacy", ""),
            ],
            vec![response("urn:passport:compact", "valid"), invalid_refresh],
        ] {
            let context = context();
            let error = accept_user_tokens(&context, collection(values)).unwrap_err();
            assert!(!error.to_string().contains("private-fixture"));
            assert!(
                matches!(context.tokens().get_user_sts_token().unwrap(), Token::Compact(value) if value == "previous-synthetic-ticket")
            );
        }
    }

    #[test]
    fn token_response_single_ticket_keeps_existing_refresh_credential() {
        let context = context();
        let outcome =
            ExchangeUserTokenOutcome::Issued(soap::BodyContent::RequestSecurityTokenResponse(
                Box::new(response("urn:passport:compact", "synthetic-ticket")),
            ));
        let (ticket, _) = accept_user_tokens(&context, outcome).unwrap();
        assert_eq!(ticket, "synthetic-ticket");
        assert!(
            matches!(context.tokens().get_user_sts_token().unwrap(), Token::Compact(value) if value == "previous-synthetic-ticket")
        );
    }
    #[test]
    fn token_response_stale_session_never_returns_a_ticket_or_changes_credentials() {
        for replace in [false, true] {
            for with_refresh in [false, true] {
                let context = context();
                let session = |name: &str| {
                    context
                        .tokens()
                        .save_user_session(
                            irori_msstore_api::models::secrets::User {
                                puid: name.into(),
                                username: name.into(),
                            },
                            std::collections::HashMap::from([(
                                PASSPORT_STS.into(),
                                Token::Compact(name.into()),
                            )]),
                        )
                        .unwrap()
                };
                session("a");
                let mut snapshot = context.tokens().user_token_snapshot().unwrap();
                if replace {
                    session("b");
                } else {
                    context.tokens().remove_persistent().unwrap();
                }
                let mut values = vec![response("urn:passport:compact", "old-account-ticket")];
                if with_refresh {
                    values.push(response("urn:passport:legacy", ""));
                }
                let result = super::accept_user_tokens(&context, &mut snapshot, collection(values));
                assert!(result.is_err(), "a stale account response was accepted");
                match context.tokens().get_user_token_for(PASSPORT_STS).unwrap() {
                    Some(Token::Compact(value)) if replace => assert_eq!(value, "b"),
                    None if !replace => {}
                    _ => panic!("old response changed the current credentials"),
                }
            }
        }
    }
}
