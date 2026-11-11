//! What every request of the account service shares: the HTTP client, the
//! device token and the account token store.

use std::{sync::Arc, time::Duration};

use irori_msstore_api::models::secrets::LegacyToken;
use irori_msstore_api::tokens::TokenManager;

// Clones share the HTTP transport pool and token manager, not mutable per-peer
// protocol state. Authentication remains attached to individual requests.
#[derive(Clone)]
pub struct SimpleContext {
    pub client: reqwest::Client,
    pub device_token: Option<LegacyToken>,
    tokens: Arc<TokenManager>,
}

impl SimpleContext {
    pub fn new(device_token: LegacyToken, tokens: Arc<TokenManager>) -> Self {
        let client = service_client(Duration::from_secs(60));

        Self {
            client,
            device_token: Some(device_token),
            tokens,
        }
    }

    pub fn tokens(&self) -> &Arc<TokenManager> {
        &self.tokens
    }
}

fn service_client(timeout: Duration) -> reqwest::Client {
    reqwest::ClientBuilder::new()
        .user_agent(format!("Irori/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(timeout)
        .build()
        .unwrap_or_default()
}

#[cfg(test)]
mod pooling_tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_util::sync::CancellationToken;

    fn context() -> SimpleContext {
        SimpleContext::new(
            LegacyToken {
                key_name: None,
                token: "synthetic-device".into(),
                binary_secret: None,
                tpm_key: None,
                lifetime: irori_msstore_api::models::soap::Timestamp {
                    id: None,
                    created: "2026-01-01T00:00:00Z".into(),
                    expires: "2027-01-01T00:00:00Z".into(),
                },
            },
            Arc::new(TokenManager::with_memory()),
        )
    }

    async fn requests(shared: bool) -> (usize, Vec<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let headers = captured.clone();
        let stop = CancellationToken::new();
        let shutdown = stop.clone();
        let server = tokio::spawn(async move {
            let mut connections = 0;
            let mut tasks = tokio::task::JoinSet::new();
            loop {
                let (mut socket, _) = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    accepted = listener.accept() => accepted.unwrap(),
                };
                connections += 1;
                let headers = headers.clone();
                tasks.spawn(async move {
                    loop {
                        let mut request = Vec::new();
                        loop {
                            let mut byte = [0];
                            if socket.read(&mut byte).await.unwrap_or(0) == 0 { return; }
                            request.push(byte[0]);
                            assert!(request.len() < 16_384);
                            if request.ends_with(b"\r\n\r\n") { break; }
                        }
                        headers.lock().unwrap().push(String::from_utf8(request).unwrap().to_lowercase());
                        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nSet-Cookie: fixture=previous-account\r\n\r\nok").await.unwrap();
                    }
                });
            }
            tasks.abort_all();
            while let Some(result) = tasks.join_next().await {
                if let Err(error) = result {
                    assert!(error.is_cancelled(), "HTTP fixture panicked: {error}");
                }
            }
            connections
        });
        let base = context();
        for authorization in [Some("Bearer synthetic-a"), Some("Bearer synthetic-b"), None] {
            let mut peer = if shared { base.clone() } else { context() };
            peer.device_token = None;
            assert!(
                base.device_token.is_some(),
                "per-peer protocol state leaked into the listener"
            );
            let mut request = peer.client.get(format!("http://{address}/fixture"));
            if let Some(value) = authorization {
                request = request.header("Authorization", value);
            }
            assert_eq!(request.send().await.unwrap().text().await.unwrap(), "ok");
        }
        stop.cancel();
        let connections = server.await.unwrap();
        let headers = captured.lock().unwrap().clone();
        (connections, headers)
    }

    #[tokio::test]
    async fn pooled_transport_reuses_connections_without_carrying_credentials() {
        let (connections, headers) = tokio::time::timeout(Duration::from_secs(3), requests(true))
            .await
            .unwrap();
        assert_eq!(connections, 1);
        assert_eq!(headers.len(), 3);
        assert!(headers[0].contains("authorization: bearer synthetic-a\r\n"));
        assert!(headers[1].contains("authorization: bearer synthetic-b\r\n"));
        assert!(!headers[1].contains("synthetic-a"));
        assert!(!headers[2].contains("authorization:"));
        for header in headers {
            assert!(!header.contains("\r\ncookie:"));
        }
    }

    #[tokio::test]
    async fn separate_transports_reproduce_the_previous_connection_overhead() {
        let (connections, headers) = tokio::time::timeout(Duration::from_secs(3), requests(false))
            .await
            .unwrap();
        assert_eq!(connections, 3);
        assert_eq!(headers.len(), 3);
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn stalled_http(mode: u8) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(1), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut request = [0; 1024];
            let _ = socket.read(&mut request).await.unwrap();
            if mode == 0 {
                tokio::time::sleep(Duration::from_millis(250)).await;
                return;
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            if mode == 1 {
                tokio::time::sleep(Duration::from_millis(250)).await;
                return;
            }
            for _ in 0..10 {
                tokio::time::sleep(Duration::from_millis(30)).await;
                if socket.write_all(b"x").await.is_err() {
                    break;
                }
            }
        });
        let client = service_client(Duration::from_millis(80));
        let response = client.get(format!("http://{address}/fixture")).send().await;
        let result = match response {
            Ok(response) => response.text().await,
            Err(error) => Err(error),
        };
        let error = result.expect_err("stalled response unexpectedly completed");
        assert!(error.is_timeout(), "expected total deadline, got {error}");
        server.await.unwrap();
    }
    #[tokio::test]
    async fn http_deadline_covers_headers() {
        stalled_http(0).await;
    }
    #[tokio::test]
    async fn http_deadline_covers_body() {
        stalled_http(1).await;
    }
    #[tokio::test]
    async fn http_deadline_does_not_restart_for_each_chunk() {
        stalled_http(2).await;
    }
}
