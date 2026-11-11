//! Optional account reads: no interactive auth, provisioning or credential deletion.
use crate::{
    models::secrets::{LegacyToken, Token},
    tokens::{
        TokenManager,
        session::{self, State},
    },
};
use std::{error::Error, future::Future, time::Duration};

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
pub fn diagnostic(stage: &'static str) {
    tracing::debug!("NEXUS_SESSION_DIAGNOSTIC_V1 {stage}");
}

/// Which stored account a session is prepared for. `Xbox` is the player
/// (Xbox Live profile, social, saves). `License` owns the games or the
/// subscription (Store ticket, library, membership). Without a separate player
/// account both are the same account.
#[derive(Clone, Copy)]
pub enum Role {
    License,
    Xbox,
}

impl Role {
    fn account_role(self) -> crate::tokens::AccountRole {
        match self {
            Self::License => crate::tokens::AccountRole::License,
            Self::Xbox => crate::tokens::AccountRole::Xbox,
        }
    }
}

pub async fn prepare(
    tokens: &TokenManager,
    client: &reqwest::Client,
) -> Result<
    (
        String,
        LegacyToken,
        LegacyToken,
        crate::tokens::UserTokenSnapshot,
    ),
    &'static str,
> {
    prepare_for(Role::Xbox, tokens, || async {
        crate::tokens::device::refresh_existing_device_credentials(client, tokens).await
    })
    .await
}

/// Same as `prepare`, for the account that owns the games or subscription.
pub async fn prepare_license(
    tokens: &TokenManager,
    client: &reqwest::Client,
) -> Result<
    (
        String,
        LegacyToken,
        LegacyToken,
        crate::tokens::UserTokenSnapshot,
    ),
    &'static str,
> {
    prepare_for(Role::License, tokens, || async {
        crate::tokens::device::refresh_existing_device_credentials(client, tokens).await
    })
    .await
}

#[cfg(test)]
async fn prepare_with<F, Fut>(
    tokens: &TokenManager,
    refresh: F,
) -> Result<
    (
        String,
        LegacyToken,
        LegacyToken,
        crate::tokens::UserTokenSnapshot,
    ),
    &'static str,
>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), crate::tokens::store::TokenStoreError>>,
{
    prepare_for(Role::Xbox, tokens, refresh).await
}

async fn prepare_for<F, Fut>(
    role: Role,
    tokens: &TokenManager,
    mut refresh: F,
) -> Result<
    (
        String,
        LegacyToken,
        LegacyToken,
        crate::tokens::UserTokenSnapshot,
    ),
    &'static str,
>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), crate::tokens::store::TokenStoreError>>,
{
    let snapshot = tokens
        .user_token_snapshot_role(role.account_role())
        .map_err(|_| "stored-account")?;
    let account = snapshot
        .stored_user()
        .ok_or("stored-account")?
        .username
        .clone();
    let user = snapshot.token();
    match session::state(&user, chrono::Utc::now()) {
        State::Expired => return Err("session-user-expired"),
        State::Invalid => return Err("session-user-invalid"),
        State::Current => {}
    }
    let device = tokens.get_device_sts_token();
    let renew = match &device {
        Ok(token) => match session::state(token, chrono::Utc::now()) {
            State::Current => false,
            State::Expired => true,
            State::Invalid => return Err("session-device-invalid"),
        },
        Err(crate::tokens::store::TokenStoreError::NotFound) => true,
        Err(_) => return Err("session-device-unavailable"),
    };
    if renew {
        // Presence is checked before invoking even the injectable renewal step.
        tokens
            .get_device_license()
            .map_err(|_| "session-device-unavailable")?;
        diagnostic("session-device-refresh");
        refresh()
            .await
            .map_err(|_| "session-device-refresh-failed")?;
    }
    tokens
        .validate_user_token_snapshot(&snapshot)
        .map_err(|_| "session-account-changed")?;
    let device = tokens
        .get_device_sts_token()
        .map_err(|_| "session-device-unavailable")?;
    if session::state(&device, chrono::Utc::now()) != State::Current {
        return Err("session-device-invalid");
    }
    let Token::Legacy(device) = device else {
        return Err("session-device-invalid");
    };
    let Token::Legacy(user) = user else {
        return Err("session-user-invalid");
    };
    Ok((account, device, user, snapshot))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Timeout,
    Connect,
    Network,
    Response,
    Build,
    Parse,
    Signature,
    Crypto,
}
impl Failure {
    fn stage(self) -> &'static str {
        match self {
            Self::Timeout => "ticket-timeout",
            Self::Connect => "ticket-connect",
            Self::Network => "ticket-network",
            Self::Response => "ticket-response",
            Self::Build => "ticket-request-build",
            Self::Parse => "ticket-response-parse",
            Self::Signature => "ticket-response-signature",
            Self::Crypto => "ticket-response-crypto",
        }
    }
    fn retry(self) -> bool {
        matches!(self, Self::Timeout | Self::Connect | Self::Network)
    }
}
pub fn classify(error: &(dyn Error + 'static)) -> Failure {
    let mut current = Some(error);
    for _ in 0..8 {
        let Some(error) = current else {
            break;
        };
        match crate::api::live::exchange_failure_stage(error) {
            Some("ticket-request-build") => return Failure::Build,
            Some("ticket-response-parse") => return Failure::Parse,
            Some("ticket-response-signature") => return Failure::Signature,
            Some("ticket-response-crypto") => return Failure::Crypto,
            _ => {}
        }
        if let Some(request) = error.downcast_ref::<reqwest::Error>() {
            return if request.is_timeout() {
                Failure::Timeout
            } else if request.is_connect() {
                Failure::Connect
            } else if request.status().is_none() && (request.is_request() || request.is_body()) {
                Failure::Network
            } else {
                Failure::Response
            };
        }
        current = error.source();
    }
    Failure::Response
}

fn service_code(raw: Option<&str>) -> Option<String> {
    let raw = raw?;
    let raw = raw
        .strip_prefix("0x")
        .or_else(|| raw.strip_prefix("0X"))
        .unwrap_or(raw);
    (raw.len() == 8 && raw.bytes().all(|b| b.is_ascii_hexdigit())).then(|| raw.to_ascii_uppercase())
}

/// A SOAP fault is a refusal, not a transient transport error. Do not retry it,
/// follow its URL or discard credentials. Only its bounded public code is logged.
pub fn rejection(outcome: &crate::models::live::ExchangeUserTokenOutcome) -> Option<&'static str> {
    use crate::models::live::ExchangeUserTokenOutcome;
    let ExchangeUserTokenOutcome::Fault(pp) = outcome else {
        return None;
    };
    let stage = if pp
        .as_ref()
        .and_then(|pp| pp.inline_auth_url.as_deref())
        .is_some_and(|url| !url.is_empty())
    {
        "ticket-interaction-required"
    } else {
        "ticket-refused"
    };
    if let Some(code) = service_code(pp.as_ref().and_then(|pp| pp.req_status.as_deref())) {
        tracing::debug!("NEXUS_TICKET_DIAGNOSTIC_V1 {stage} {code}");
    } else {
        diagnostic(stage);
    }
    Some(stage)
}
pub async fn exchange<T, F, Fut>(mut request: F) -> Result<T, &'static str>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Failure>>,
{
    exchange_with(&mut request, || {
        tokio::time::sleep(Duration::from_millis(250))
    })
    .await
}
async fn exchange_with<T, F, Fut, W, Wait>(request: &mut F, mut wait: W) -> Result<T, &'static str>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, Failure>>,
    W: FnMut() -> Wait,
    Wait: Future<Output = ()>,
{
    for attempt in 0..2 {
        match request().await {
            Ok(value) => {
                diagnostic("ticket-response-received");
                return Ok(value);
            }
            Err(error) => {
                diagnostic(error.stage());
                if attempt != 0 || !error.retry() {
                    return Err(error.stage());
                }
                diagnostic("ticket-retry");
                wait().await;
            }
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ticket_fault_classification_and_provider_codes_are_strictly_bounded() {
        use crate::models::live::ExchangeUserTokenOutcome;
        for raw in ["800488ab", "0x800488AB", "0X800488ab"] {
            assert_eq!(service_code(Some(raw)).as_deref(), Some("800488AB"));
        }
        for raw in [
            "",
            "1234567",
            "123456789",
            "private@example.test",
            "800488AB secret",
            "800488AB\n",
            "0x800488AZ",
        ] {
            assert_eq!(service_code(Some(raw)), None);
        }
        assert_eq!(service_code(None), None);
        assert_eq!(
            rejection(&ExchangeUserTokenOutcome::Fault(None)),
            Some("ticket-refused")
        );
        let pp: crate::models::soap::PP = serde_json::from_value(serde_json::json!({"reqstatus":"0x800488ab", "inlineauthurl":"https://private.example.test/?secret=never-log"})).unwrap();
        assert_eq!(
            rejection(&ExchangeUserTokenOutcome::Fault(Some(pp))),
            Some("ticket-interaction-required")
        );
    }
    use crate::{
        models::{
            secrets::{Device, LegacyToken, User},
            soap::Timestamp,
        },
        tokens::PASSPORT_STS,
    };
    use std::{cell::Cell, future::ready};
    fn never_refresh() -> std::future::Ready<Result<(), crate::tokens::store::TokenStoreError>> {
        panic!("unexpected renewal");
    }
    fn never_wait() -> std::future::Ready<()> {
        panic!("unexpected retry");
    }
    fn token(expires: &str) -> Token {
        Token::Legacy(LegacyToken {
            key_name: None,
            token: "synthetic-unused".into(),
            binary_secret: None,
            tpm_key: None,
            lifetime: Timestamp {
                id: None,
                created: "2000-01-01T00:00:00Z".into(),
                expires: expires.into(),
            },
        })
    }
    fn current() -> Token {
        token("2099-01-01T00:00:00Z")
    }
    fn expired() -> Token {
        token("2001-01-01T00:00:00Z")
    }
    fn manager(device: Token, user: Token, identity: bool) -> TokenManager {
        let tokens = TokenManager::with_memory();
        tokens
            .save_user(&User {
                puid: "fixture".into(),
                username: "one@example.test".into(),
            })
            .unwrap();
        tokens.save_user_token(PASSPORT_STS.into(), user).unwrap();
        tokens
            .save_device_token(PASSPORT_STS.into(), device)
            .unwrap();
        if identity {
            tokens
                .save_device_license(&Device {
                    username: "device-fixture".into(),
                    password: "fixture".into(),
                    puid: "fixture".into(),
                    hwid: "unchanged".into(),
                    device_id: "unchanged".into(),
                    splicense: "fixture".into(),
                })
                .unwrap();
        }
        tokens
    }
    #[tokio::test]
    async fn credential_session_same_name_saved_activation_and_player_aba_are_rejected() {
        use crate::tokens::AccountRole;
        for player in [false, true] {
            let tokens = manager(expired(), current(), true);
            let u = User {
                puid: "same".into(),
                username: "same@example.test".into(),
            };
            tokens
                .save_user_session(
                    u.clone(),
                    std::collections::HashMap::from([(PASSPORT_STS.into(), current())]),
                )
                .unwrap();
            tokens.enable_saved_accounts().unwrap();
            if player {
                tokens
                    .save_player_session(
                        u,
                        std::collections::HashMap::from([(PASSPORT_STS.into(), current())]),
                    )
                    .unwrap();
            }
            let result = prepare_for(
                if player { Role::Xbox } else { Role::License },
                &tokens,
                || {
                    tokens
                        .save_device_token(PASSPORT_STS.into(), current())
                        .unwrap();
                    if player {
                        tokens.remove_player().unwrap();
                        tokens
                            .save_player_session(
                                User {
                                    puid: "same".into(),
                                    username: "same@example.test".into(),
                                },
                                std::collections::HashMap::from([(PASSPORT_STS.into(), current())]),
                            )
                            .unwrap();
                    } else {
                        tokens.activate_saved_account("same@example.test").unwrap();
                    }
                    ready(Ok(()))
                },
            )
            .await;
            assert_eq!(result.err(), Some("session-account-changed"));
            assert_eq!(
                tokens
                    .user_token_snapshot_role(if player {
                        AccountRole::Xbox
                    } else {
                        AccountRole::License
                    })
                    .unwrap()
                    .stored_user()
                    .unwrap()
                    .username,
                "same@example.test"
            );
        }
    }
    #[tokio::test]
    async fn credential_session_valid_signin_has_no_renewal_and_raw_reads_are_unchanged() {
        let tokens = manager(current(), current(), true);
        let result = prepare_with(&tokens, never_refresh).await.unwrap();
        assert_eq!(result.0, "one@example.test");
        assert_eq!(tokens.get_device_license().unwrap().device_id, "unchanged");
        // Interactive login and other raw upstream consumers keep their existing API.
        tokens
            .save_user_token(PASSPORT_STS.into(), expired())
            .unwrap();
        assert!(tokens.get_user_sts_token().is_ok());
        assert!(tokens.get_user().is_ok());
    }
    #[tokio::test]
    async fn credential_session_expired_invalid_user_never_renews_or_deletes_account() {
        for (user, expected) in [
            (expired(), "session-user-expired"),
            (token("bad"), "session-user-invalid"),
        ] {
            let tokens = manager(expired(), user, true);
            assert_eq!(
                prepare_with(&tokens, never_refresh).await.err(),
                Some(expected)
            );
            assert_eq!(tokens.get_user().unwrap().username, "one@example.test");
            assert!(tokens.get_user_sts_token().is_ok());
            assert_eq!(tokens.get_device_license().unwrap().hwid, "unchanged");
        }
    }
    #[tokio::test]
    async fn credential_session_expired_device_renews_once_preserving_identity() {
        let tokens = manager(expired(), current(), true);
        let calls = Cell::new(0);
        let result = prepare_with(&tokens, || {
            calls.set(calls.get() + 1);
            ready(tokens.save_device_token(PASSPORT_STS.into(), current()))
        })
        .await
        .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(result.0, "one@example.test");
        assert_eq!(tokens.get_device_license().unwrap().hwid, "unchanged");
        prepare_with(&tokens, never_refresh).await.unwrap();
    }
    #[tokio::test]
    async fn credential_session_missing_identity_never_provisions() {
        let tokens = manager(expired(), current(), false);
        assert_eq!(
            prepare_with(&tokens, never_refresh).await.err(),
            Some("session-device-unavailable")
        );
        assert!(tokens.get_device_license().is_err());
        assert!(tokens.get_user().is_ok());
    }
    #[tokio::test]
    async fn credential_session_failed_renewal_and_account_switch_publish_no_session() {
        let tokens = manager(expired(), current(), true);
        assert_eq!(
            prepare_with(&tokens, || ready(Err(
                crate::tokens::store::TokenStoreError::DeviceAuthentication
            )))
            .await
            .err(),
            Some("session-device-refresh-failed")
        );
        assert_eq!(
            session::state(&tokens.get_device_sts_token().unwrap(), chrono::Utc::now()),
            State::Expired
        );
        assert_eq!(tokens.get_user().unwrap().username, "one@example.test");
        assert_eq!(tokens.get_device_license().unwrap().device_id, "unchanged");
        assert_eq!(
            prepare_with(&tokens, || {
                tokens
                    .save_device_token(PASSPORT_STS.into(), current())
                    .unwrap();
                tokens
                    .save_user(&User {
                        puid: "other".into(),
                        username: "two@example.test".into(),
                    })
                    .unwrap();
                ready(Ok(()))
            })
            .await
            .err(),
            Some("session-account-changed")
        );
    }
    #[tokio::test]
    async fn credential_session_does_not_accept_a_still_expired_renewal() {
        let tokens = manager(expired(), current(), true);
        assert_eq!(
            prepare_with(&tokens, || ready(Ok(()))).await.err(),
            Some("session-device-invalid")
        );
    }
    #[tokio::test]
    async fn ticket_transient_retry_is_bounded_and_response_errors_are_not_retried() {
        for failure in [
            Failure::Timeout,
            Failure::Connect,
            Failure::Network,
            Failure::Response,
            Failure::Build,
            Failure::Parse,
            Failure::Signature,
            Failure::Crypto,
        ] {
            let calls = Cell::new(0);
            let waits = Cell::new(0);
            let result: Result<(), _> = exchange_with(
                &mut || {
                    calls.set(calls.get() + 1);
                    ready(Err(failure))
                },
                || {
                    waits.set(waits.get() + 1);
                    ready(())
                },
            )
            .await;
            assert_eq!(result, Err(failure.stage()));
            assert_eq!(calls.get(), if failure.retry() { 2 } else { 1 });
            assert_eq!(waits.get(), if failure.retry() { 1 } else { 0 });
        }
        let calls = Cell::new(0);
        let value = exchange_with(
            &mut || {
                calls.set(calls.get() + 1);
                ready(if calls.get() == 1 {
                    Err(Failure::Timeout)
                } else {
                    Ok("issued")
                })
            },
            || ready(()),
        )
        .await;
        assert_eq!(value, Ok("issued"));
        assert_eq!(calls.get(), 2);
        let value = exchange_with(&mut || ready(Ok::<_, Failure>("SOAP-fault")), never_wait).await;
        assert_eq!(value, Ok("SOAP-fault"));
    }
    #[tokio::test]
    async fn ticket_cancelled_future_does_not_spawn_a_background_retry() {
        let calls = Cell::new(0);
        let task = exchange(|| {
            calls.set(calls.get() + 1);
            std::future::pending::<Result<(), Failure>>()
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(5), task)
                .await
                .is_err()
        );
        assert_eq!(calls.get(), 1);
    }
    #[tokio::test]
    async fn ticket_real_transport_timeout_and_http_refusal_classification() {
        use tokio::{io::AsyncWriteExt, net::TcpListener};
        let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (_socket, _) = server.accept().await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        });
        let error = reqwest::Client::builder()
            .timeout(Duration::from_millis(30))
            .build()
            .unwrap()
            .get(format!("http://{address}/fixture"))
            .send()
            .await
            .unwrap_err();
        assert_eq!(classify(&error), Failure::Timeout);
        task.abort();
        for status in [401, 403, 500] {
            let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = server.local_addr().unwrap();
            let task = tokio::spawn(async move {
                let (mut socket, _) = server.accept().await.unwrap();
                // Answer only after the request arrived, or the client may
                // see a reply to a request it hasn't finished sending.
                let mut request = [0u8; 1024];
                let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut request).await;
                socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
            });
            let error = reqwest::get(format!("http://{address}/fixture"))
                .await
                .unwrap()
                .error_for_status()
                .unwrap_err();
            assert_eq!(classify(&error), Failure::Response);
            task.await.unwrap();
        }
    }
}
