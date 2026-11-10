//! Fixed authentication metadata only. Never format a request, error chain,
//! response body, account identifier, token or server-provided message.
use crate::models::xbox::XstsResponse;

const MAX_ERROR_BYTES: usize = 8192;

#[derive(Clone, Copy)]
pub(super) enum Stage {
    User,
    Xsts,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::User => "xbox-user",
            Self::Xsts => "xbox-xsts",
        }
    }
}

fn frame(stage: Stage, category: &'static str, status: u16, code: Option<u32>) -> String {
    format!(
        "NEXUS_XBOX_AUTH_DIAGNOSTIC_V1 {}-{category} {status} {:08X}",
        stage.name(),
        code.unwrap_or(0)
    )
}

pub(super) fn request(stage: Stage, error: reqwest::Error) -> reqwest::Error {
    let category = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else {
        "request"
    };
    eprintln!("{}", frame(stage, category, 0, None));
    error.without_url()
}

fn service_code(bytes: &[u8]) -> Option<u32> {
    if bytes.len() > MAX_ERROR_BYTES {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    u32::try_from(value.as_object()?.get("XErr")?.as_u64()?).ok()
}

async fn error_code(response: &mut reqwest::Response) -> Option<u32> {
    if response
        .content_length()
        .is_some_and(|size| size > MAX_ERROR_BYTES as u64)
    {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len() + chunk.len() > MAX_ERROR_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    service_code(&bytes)
}

pub(super) async fn response(
    stage: Stage,
    mut response: reqwest::Response,
) -> reqwest::Result<XstsResponse> {
    let status = response.status().as_u16();
    if let Err(error) = response.error_for_status_ref() {
        let code = error_code(&mut response).await;
        eprintln!("{}", frame(stage, "http", status, code));
        return Err(error.without_url());
    }
    response.json().await.map_err(|error| {
        eprintln!("{}", frame(stage, "response", status, None));
        error.without_url()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    async fn reply(status: u16, body: String) -> reqwest::Response {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .unwrap()
            .get(format!("http://{address}/private?token=PRIVATE_SECRET"))
            .send()
            .await
            .unwrap()
    }

    #[test]
    fn xbox_auth_diagnostic_retains_only_unsigned_service_code() {
        assert_eq!(service_code(br#"{"XErr":2148916233,"Identity":"PRIVATE_ID","Message":"PRIVATE_SECRET","Redirect":"https://private.test/"}"#), Some(2148916233));
        for bytes in [
            br#"{"XErr":"2148916233"}"#.as_slice(),
            br#"{"XErr":-1}"#,
            br#"{"XErr":4294967296}"#,
            br#"{"XErr":1.5}"#,
            br#"{"XErr":null}"#,
            b"PRIVATE_SECRET",
            br#"[2148916233]"#,
        ] {
            assert_eq!(service_code(bytes), None);
        }
        assert_eq!(service_code(&vec![b' '; MAX_ERROR_BYTES + 1]), None);
        assert_eq!(
            frame(Stage::Xsts, "http", 401, Some(2148916233)),
            "NEXUS_XBOX_AUTH_DIAGNOSTIC_V1 xbox-xsts-http 401 8015DC09"
        );
    }

    #[tokio::test]
    async fn xbox_auth_diagnostic_preserves_rejections_without_body_or_url_in_error() {
        for status in [400, 401, 403, 429, 500, 503] {
            let result = response(
                Stage::User,
                reply(
                    status,
                    r#"{"XErr":2148916233,"Message":"PRIVATE_SECRET"}"#.into(),
                )
                .await,
            )
            .await;
            let error = result.unwrap_err();
            assert_eq!(error.status().unwrap().as_u16(), status);
            assert!(error.url().is_none());
            assert!(!error.to_string().contains("PRIVATE_SECRET"));
        }
    }

    #[tokio::test]
    async fn xbox_auth_diagnostic_does_not_change_success_or_accept_invalid_responses() {
        let body = r#"{"NotAfter":"2099-01-01T00:00:00Z","Token":"SYNTHETIC_TOKEN","DisplayClaims":{"xui":[{"uhs":"123"}]}}"#;
        let value = response(Stage::User, reply(200, body.into()).await)
            .await
            .unwrap();
        assert_eq!(value.token, "SYNTHETIC_TOKEN");
        assert_eq!(value.user_hash(), Some("123"));
        let error = response(
            Stage::Xsts,
            reply(200, "PRIVATE_SECRET invalid JSON".into()).await,
        )
        .await
        .unwrap_err();
        assert!(error.is_decode());
        assert!(error.url().is_none());
        assert!(!error.to_string().contains("PRIVATE_SECRET"));
        assert!(
            response(
                Stage::Xsts,
                reply(401, "x".repeat(MAX_ERROR_BYTES + 1)).await
            )
            .await
            .unwrap_err()
            .is_status()
        );
    }
}
