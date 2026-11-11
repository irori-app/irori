use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

#[path = "peer.rs"]
mod peer;

use crate::service::context::SimpleContext;

pub async fn route(
    socket: tokio::net::UnixStream,
    token: CancellationToken,
    context: SimpleContext,
) {
    // Leave room for the existing90s licence response inside the runtime95s budget.
    route_with_budget(socket, token, context, std::time::Duration::from_secs(92)).await;
}

async fn route_with_budget(
    mut socket: tokio::net::UnixStream,
    token: CancellationToken,
    mut context: SimpleContext,
    budget: std::time::Duration,
) {
    let cred = socket.peer_cred().ok().and_then(|cred| cred.pid());
    tracing::debug!("Connection from pid {cred:?}");

    let peer = peer::PeerMonitor::new(&socket);
    loop {
        // One budget includes framing, upstream work, body parsing and reply.
        // Cancellation drops the in-flight async I/O rather than waiting for
        // the peer to send another byte before checking the shutdown token.
        let request = async {
            let mut read_magic = [0; 4];
            socket.read_exact(&mut read_magic).await?;
            match u32::from_le_bytes(read_magic) {
                crate::service::XML_MAGIC => {
                    crate::service::xml::handle(&mut socket, &mut context).await
                }
                crate::service::PROTO_MAGIC => {
                    crate::service::proto_handle(&mut socket, &mut context).await
                }
                _ => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "unknown protocol magic",
                )),
            }
        };
        let result = match peer::run_request(&peer, &token, budget, request).await {
            peer::RequestEnd::Stopped => return,
            peer::RequestEnd::Finished(result) => result,
        };
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::debug!("Backend connection ended: {error}");
                return;
            }
            Err(_) => {
                tracing::warn!("Backend request exceeded its deadline");
                return;
            }
        }
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    use irori_msstore_api::models::secrets::LegacyToken;
    use irori_msstore_api::tokens::TokenManager;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn credentials() -> LegacyToken {
        LegacyToken {
            key_name: None,
            token: "synthetic-unused".into(),
            binary_secret: None,
            tpm_key: None,
            lifetime: irori_msstore_api::models::soap::Timestamp {
                id: None,
                created: "2026-01-01T00:00:00Z".into(),
                expires: "2027-01-01T00:00:00Z".into(),
            },
        }
    }
    async fn stalled(bytes: &[u8], cancel: bool) {
        let (mut client, server) = tokio::net::UnixStream::pair().unwrap();
        client.write_all(bytes).await.unwrap();
        let cancellation = CancellationToken::new();
        let trigger = cancellation.clone();
        let cancel_task = tokio::spawn(async move {
            if cancel {
                tokio::time::sleep(Duration::from_millis(20)).await;
                trigger.cancel();
            }
        });
        let result = tokio::time::timeout(
            Duration::from_millis(400),
            route_with_budget(
                server,
                cancellation,
                SimpleContext::new(credentials(), Arc::new(TokenManager::with_memory())),
                Duration::from_millis(80),
            ),
        )
        .await;
        cancel_task.await.unwrap();
        assert!(
            result.is_ok(),
            "stalled request did not release its connection"
        );
        let mut byte = [0];
        assert_eq!(
            client.read(&mut byte).await.unwrap(),
            0,
            "connection remains open after deadline"
        );
    }
    #[tokio::test]
    async fn deadline_covers_partial_magic() {
        stalled(&crate::service::XML_MAGIC.to_le_bytes()[..2], false).await;
    }
    #[tokio::test]
    async fn deadline_covers_partial_header() {
        stalled(&crate::service::XML_MAGIC.to_le_bytes(), false).await;
    }
    #[tokio::test]
    async fn deadline_covers_partial_body() {
        let mut request = crate::service::XML_MAGIC.to_le_bytes().to_vec();
        request.extend(1_u16.to_le_bytes());
        request.extend(4_u16.to_le_bytes());
        request.push(b'p');
        stalled(&request, false).await;
    }
    #[tokio::test]
    async fn cancellation_interrupts_pending_read() {
        stalled(&[], true).await;
    }
    #[tokio::test]
    async fn complete_ping_preserves_framing() {
        let (mut client, server) = tokio::net::UnixStream::pair().unwrap();
        let token = CancellationToken::new();
        let shutdown = token.clone();
        let task = tokio::spawn(route_with_budget(
            server,
            token,
            SimpleContext::new(credentials(), Arc::new(TokenManager::with_memory())),
            Duration::from_secs(2),
        ));
        let request =
            crate::service::encode_message(crate::service::XML_MAGIC, 1, b"ping".to_vec());
        client.write_all(&request).await.unwrap();
        let mut response = [0; 12];
        tokio::time::timeout(Duration::from_secs(1), client.read_exact(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            response.to_vec(),
            crate::service::encode_message(crate::service::XML_MAGIC, 2, b"ping".to_vec())
        );
        shutdown.cancel();
        drop(client);
        task.await.unwrap();
    }
}
