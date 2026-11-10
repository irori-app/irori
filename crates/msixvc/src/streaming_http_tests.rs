use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn policy() -> Policy {
    Policy {
        timeout: Duration::from_millis(80),
        budget: Duration::from_millis(500),
        backoff: Duration::from_millis(2),
        retries: 2,
    }
}

#[tokio::test]
async fn streaming_http_disk_or_consumer_delay_is_not_network_idle_time() {
    let (url, task) = server(vec![
        reply(206, "bytes 0-0/2", "a"),
        reply(206, "bytes 1-1/2", "b"),
    ])
    .await;
    let client = crate::http_client().unwrap();
    let mut reader = RangeReader::new(
        &client,
        &url,
        0,
        1,
        Policy {
            budget: Duration::from_millis(100),
            ..policy()
        },
    );
    assert_eq!(reader.next().await.unwrap().unwrap(), "a");
    sleep(Duration::from_millis(150)).await;
    assert_eq!(reader.next().await.unwrap().unwrap(), "b");
    assert!(reader.next().await.unwrap().is_none());
    assert_eq!(task.await.unwrap().len(), 2);
}
fn reply(status: u16, range: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nContent-Range: {range}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
async fn server(replies: Vec<String>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut requests = vec![];
        for reply in replies {
            let (mut socket, _) = timeout(Duration::from_secs(2), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut request = vec![];
            while !request.ends_with(b"\r\n\r\n") {
                request.push(
                    timeout(Duration::from_secs(1), socket.read_u8())
                        .await
                        .unwrap()
                        .unwrap(),
                );
                assert!(request.len() < 8192);
            }
            requests.push(String::from_utf8(request).unwrap().to_lowercase());
            socket.write_all(reply.as_bytes()).await.ok();
        }
        requests
    });
    (
        format!("http://{address}/private-package?token=PRIVATE-SECRET"),
        task,
    )
}
async fn collect(reader: &mut RangeReader) -> Result<Vec<u8>, TransferError> {
    let mut bytes = vec![];
    while let Some(chunk) = reader.next().await? {
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[tokio::test]
async fn streaming_http_retries_transient_status_and_preserves_exact_range() {
    let (url, task) = server(vec![
        reply(503, "", "private-provider-message"),
        reply(206, "bytes 5-9/20", "abcde"),
    ])
    .await;
    let client = crate::http_client().unwrap();
    let mut reader = RangeReader::new(&client, &url, 5, 9, policy());
    assert_eq!(collect(&mut reader).await.unwrap(), b"abcde");
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|r| r.contains("range: bytes=5-9\r\n") && r.contains("accept-encoding: identity"))
    );
}
#[tokio::test]
async fn streaming_http_truncated_body_resumes_at_received_byte_without_duplication() {
    let first = "HTTP/1.1 206 Test\r\nContent-Length: 10\r\nContent-Range: bytes 0-9/10\r\nConnection: close\r\n\r\nabc".to_string();
    let (url, task) = server(vec![first, reply(206, "bytes 3-9/10", "defghij")]).await;
    let client = crate::http_client().unwrap();
    assert_eq!(
        collect(&mut RangeReader::new(&client, &url, 0, 9, policy()))
            .await
            .unwrap(),
        b"abcdefghij"
    );
    assert!(task.await.unwrap()[1].contains("range: bytes=3-9\r\n"));
}
#[tokio::test]
async fn streaming_http_short_valid_response_reconnects_for_the_remaining_range() {
    let (url, task) = server(vec![
        reply(206, "bytes 0-2/10", "abc"),
        reply(206, "bytes 3-9/10", "defghij"),
    ])
    .await;
    let client = crate::http_client().unwrap();
    assert_eq!(
        collect(&mut RangeReader::new(&client, &url, 0, 9, policy()))
            .await
            .unwrap(),
        b"abcdefghij"
    );
    assert_eq!(task.await.unwrap().len(), 2);
}
#[tokio::test]
async fn streaming_http_denials_rate_limit_and_wrong_status_are_not_retried_or_leaked() {
    for status in [200, 301, 400, 401, 403, 404, 416, 429] {
        // No Location header, so the redirect is a final unexpected response.
        let (url, task) = server(vec![reply(status, "", "private-provider-message")]).await;
        let client = crate::http_client().unwrap();
        let error = collect(&mut RangeReader::new(&client, &url, 0, 9, policy()))
            .await
            .unwrap_err();
        assert_eq!(error, TransferError::new("transfer-http", status));
        assert!(!error.to_string().contains("PRIVATE") && !error.diagnostic().contains("private"));
        assert_eq!(task.await.unwrap().len(), 1);
    }
}
#[tokio::test]
async fn streaming_http_rejects_invalid_range_encoding_and_changed_total() {
    for range in [
        "",
        "bytes 1-9/10",
        "bytes 0-10/20",
        "bytes 0-9/9",
        "bytes 9-0/10",
        "bytes 0-9/*",
    ] {
        let (url, task) = server(vec![reply(206, range, "abcdefghij")]).await;
        let client = crate::http_client().unwrap();
        assert_eq!(
            collect(&mut RangeReader::new(&client, &url, 0, 9, policy()))
                .await
                .unwrap_err()
                .stage,
            "transfer-range"
        );
        task.await.unwrap();
    }
    let encoded = reply(206, "bytes 0-9/10", "abcdefghij").replace(
        "Connection: close",
        "Content-Encoding: gzip\r\nConnection: close",
    );
    let (url, task) = server(vec![encoded]).await;
    let client = crate::http_client().unwrap();
    assert_eq!(
        collect(&mut RangeReader::new(&client, &url, 0, 9, policy()))
            .await
            .unwrap_err()
            .stage,
        "transfer-encoding"
    );
    task.await.unwrap();
    let (url, task) = server(vec![
        reply(206, "bytes 0-2/10", "abc"),
        reply(206, "bytes 3-9/11", "defghij"),
    ])
    .await;
    assert_eq!(
        collect(&mut RangeReader::new(&client, &url, 0, 9, policy()))
            .await
            .unwrap_err()
            .stage,
        "transfer-range"
    );
    task.await.unwrap();
}
#[tokio::test]
async fn streaming_http_retries_are_bounded_and_keep_actual_status() {
    let (url, task) = server(vec![reply(503, "", "private"); 3]).await;
    let client = crate::http_client().unwrap();
    let start = Instant::now();
    let error = collect(&mut RangeReader::new(&client, &url, 0, 9, policy()))
        .await
        .unwrap_err();
    assert_eq!(error, TransferError::new("transfer-http", 503));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert_eq!(task.await.unwrap().len(), 3);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let url = format!("http://{address}/?SECRET");
    assert_eq!(
        collect(&mut RangeReader::new(&client, &url, 0, 9, policy()))
            .await
            .unwrap_err()
            .stage,
        "transfer-request"
    );
}
#[tokio::test]
async fn streaming_http_idle_timeout_is_distinct_from_header_timeout() {
    for headers in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![];
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
            }
            if headers {
                socket.write_all(b"HTTP/1.1 206 Test\r\nContent-Length: 10\r\nContent-Range: bytes 0-9/10\r\n\r\n").await.unwrap();
            }
            sleep(Duration::from_secs(1)).await;
        });
        let client = crate::http_client().unwrap();
        let url = format!("http://{address}/private");
        let mut p = policy();
        p.retries = 0;
        let error = collect(&mut RangeReader::new(&client, &url, 0, 9, p))
            .await
            .unwrap_err();
        assert_eq!(
            error.stage,
            if headers {
                "transfer-idle-timeout"
            } else {
                "transfer-headers-timeout"
            }
        );
        task.abort();
    }
}
#[tokio::test]
async fn streaming_http_production_timeout_tolerates_a_six_second_slow_response() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![];
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        sleep(Duration::from_secs(6)).await;
        socket
            .write_all(reply(206, "bytes 0-2/3", "abc").as_bytes())
            .await
            .unwrap();
    });
    let client = crate::http_client().unwrap();
    let url = format!("http://{address}/package");
    assert_eq!(
        collect(&mut RangeReader::new(
            &client,
            &url,
            0,
            2,
            Policy::default()
        ))
        .await
        .unwrap(),
        b"abc"
    );
    task.await.unwrap();
}
#[test]
fn streaming_http_policy_and_error_metadata_are_bounded_and_url_free() {
    let p = Policy::default();
    assert_eq!(p.timeout, Duration::from_secs(30));
    assert_eq!(p.budget, Duration::from_secs(180));
    assert_eq!(p.retries, 5);
    assert_eq!(
        TransferError::new("transfer-http", 403).diagnostic(),
        "NEXUS_TRANSFER_DIAGNOSTIC_V1 transfer-http 403"
    );
}
