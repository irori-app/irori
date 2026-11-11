use super::*;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn policy() -> crate::streaming_http::Policy {
    crate::streaming_http::Policy {
        timeout: Duration::from_millis(40),
        budget: Duration::from_millis(250),
        backoff: Duration::from_millis(2),
        retries: 2,
    }
}
async fn server(replies: Vec<String>) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let task = tokio::spawn(async move {
        for reply in replies {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![];
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
            }
            calls.fetch_add(1, Ordering::SeqCst);
            if reply == "stall" {
                tokio::time::sleep(Duration::from_secs(2)).await;
            } else if reply == "body-stall" {
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n")
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_secs(2)).await;
            } else {
                socket.write_all(reply.as_bytes()).await.ok();
            }
        }
    });
    (
        format!("http://{address}/package?token=PRIVATE-SECRET"),
        count,
        task,
    )
}
#[tokio::test]
async fn streaming_metadata_http_failures_are_bounded_and_do_not_disclose_urls() {
    for (status, requests) in [(403, 1), (429, 1), (503, 3)] {
        let response =
            format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let (url, count, task) = server(vec![response; requests]).await;
        let result = HttpRead::open_with_policy(
            crate::http_client().unwrap(),
            &url,
            None::<fn(u64, u64)>,
            policy(),
        )
        .await;
        let error = result.err().expect("HTTP failure accepted");
        assert!(error.to_string().contains(&status.to_string()));
        assert!(!error.to_string().contains("PRIVATE-SECRET"));
        assert_eq!(count.load(Ordering::SeqCst), requests);
        task.abort();
    }
}
#[tokio::test]
async fn streaming_metadata_header_and_body_waits_have_deadlines() {
    for response in ["stall", "body-stall"] {
        let (url, _, task) = server(vec![response.into()]).await;
        let result = tokio::time::timeout(Duration::from_secs(1), async {
            let mut file = HttpRead::open_with_policy(
                crate::http_client().unwrap(),
                &url,
                None::<fn(u64, u64)>,
                crate::streaming_http::Policy {
                    retries: 0,
                    ..policy()
                },
            )
            .await?;
            file.read_to_end(&mut vec![]).await
        })
        .await
        .expect("metadata operation did not finish in bounded time");
        assert!(result.is_err());
        task.abort();
    }
}
#[tokio::test]
async fn streaming_metadata_empty_reconnects_stop_after_retry_limit() {
    let initial = "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n";
    let partial = "HTTP/1.1 206 OK\r\nContent-Length: 3\r\nContent-Range: bytes 0-2/3\r\nConnection: close\r\n\r\n";
    let (url, count, task) = server(vec![initial.into(), partial.into(), partial.into()]).await;
    let mut file = HttpRead::open_with_policy(
        crate::http_client().unwrap(),
        &url,
        None::<fn(u64, u64)>,
        policy(),
    )
    .await
    .unwrap();
    let error = tokio::time::timeout(Duration::from_secs(1), file.read_to_end(&mut vec![]))
        .await
        .unwrap()
        .unwrap_err();
    assert!(!error.to_string().contains("PRIVATE-SECRET"));
    assert_eq!(count.load(Ordering::SeqCst), 3);
    task.abort();
}
#[tokio::test]
async fn streaming_metadata_recovers_transient_headers_and_preserves_bytes() {
    let (url, count, task) = server(vec![
        "HTTP/1.1 503 Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
        "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc".into(),
    ])
    .await;
    let mut file = HttpRead::open_with_policy(
        crate::http_client().unwrap(),
        &url,
        None::<fn(u64, u64)>,
        policy(),
    )
    .await
    .unwrap();
    let mut bytes = vec![];
    file.read_to_end(&mut bytes).await.unwrap();
    assert_eq!(bytes, b"abc");
    assert_eq!(count.load(Ordering::SeqCst), 2);
    task.abort();
}
