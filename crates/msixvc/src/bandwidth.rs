//! One aggregate payload limit for all HTTP streams in this downloader process.
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::{Mutex, watch};
static SHARED: OnceLock<Arc<Limiter>> = OnceLock::new();
fn shared() -> &'static Arc<Limiter> {
    SHARED.get_or_init(|| Arc::new(Limiter::new(0)))
}
pub fn set_limit_kib(kib: u32) {
    shared()
        .speed
        .send_replace(u64::from(kib.min(1_048_576)) * 1024);
}
pub(crate) async fn acquire(bytes: usize) {
    shared().acquire(bytes).await;
}
struct Limiter {
    lane: Mutex<()>,
    speed: watch::Sender<u64>,
}
impl Limiter {
    fn new(bytes: u64) -> Self {
        let (speed, _) = watch::channel(bytes);
        Self {
            lane: Mutex::new(()),
            speed,
        }
    }
    async fn acquire(&self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        let mut speed = self.speed.subscribe();
        let _lane = self.lane.lock().await;
        loop {
            let rate = *speed.borrow_and_update();
            if rate == 0 {
                return;
            }
            // Serial pacing avoids per-connection multiplication and unused
            // future reservations when Pause/cancellation drops this future.
            let wait = Duration::from_secs_f64(bytes as f64 / rate as f64);
            tokio::select! {
                biased;
                _=speed.changed()=>continue,
                _=tokio::time::sleep(wait)=>return,
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "global limiter integration, run alone with --ignored --test-threads=1"]
    async fn streaming_bandwidth_cached_prefix_handoff_does_not_park_the_limiter() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut request = vec![];
                    while !request.ends_with(b"\r\n\r\n") {
                        request.push(socket.read_u8().await.unwrap());
                    }
                    let ranged = String::from_utf8(request)
                        .unwrap()
                        .to_lowercase()
                        .contains("range:");
                    let (header, length) = if ranged {
                        (
                            "HTTP/1.1 206 Test\r\nContent-Range: bytes 0-4095/65536\r\n",
                            4096,
                        )
                    } else {
                        ("HTTP/1.1 200 OK\r\n", 65536)
                    };
                    socket
                        .write_all(
                            format!(
                                "{header}Content-Length: {length}\r\nConnection: close\r\n\r\n"
                            )
                            .as_bytes(),
                        )
                        .await
                        .unwrap();
                    socket.write_all(&vec![b'a'; length]).await.unwrap();
                });
            }
        });
        let folder = std::env::temp_dir().join(format!("nexus-handoff-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&folder).unwrap();
        set_limit_kib(1024);
        let client = crate::http_client().unwrap();
        let url = format!("http://{address}/package");
        let input =
            crate::streaming::HttpRead::open(client.clone(), url.clone(), None::<fn(u64, u64)>)
                .await
                .unwrap();
        let mut prefix =
            crate::streaming::PrefixCacheFile::new(input, 65536, folder.join("prefix"))
                .await
                .unwrap();
        let mut header = [0u8; 32];
        prefix.read_exact(&mut header).await.unwrap();
        assert_eq!(header, [b'a'; 32]);
        // Retain the metadata reader as the real downloader does while starting
        // payload ranges. An unpolled read-ahead must not own the global lane.
        let mut range = crate::streaming_http::RangeReader::new(
            &client,
            &url,
            0,
            4095,
            crate::streaming_http::Policy::default(),
        );
        let result = tokio::time::timeout(Duration::from_secs(2), range.next()).await;
        drop(prefix);
        set_limit_kib(0);
        server.abort();
        std::fs::remove_dir_all(folder).unwrap();
        assert!(
            result.is_ok(),
            "metadata read-ahead parked the aggregate limiter"
        );
        assert_eq!(
            result.unwrap().unwrap().unwrap(),
            bytes::Bytes::from(vec![b'a'; 4096])
        );
    }
    #[tokio::test]
    #[ignore = "global limiter integration, run alone with --ignored --test-threads=1"]
    async fn streaming_bandwidth_limited_http_metadata_and_parallel_ranges_keep_bytes_and_do_not_timeout()
     {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..4 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![];
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(socket.read_u8().await.unwrap());
                }
                let range = String::from_utf8(request)
                    .unwrap()
                    .to_lowercase()
                    .contains("range:");
                let header = if range {
                    "HTTP/1.1 206 Test\r\nContent-Range: bytes 0-8191/8192\r\n"
                } else {
                    "HTTP/1.1 200 OK\r\n"
                };
                socket
                    .write_all(
                        format!("{header}Content-Length: 8192\r\nConnection: close\r\n\r\n")
                            .as_bytes(),
                    )
                    .await
                    .unwrap();
                socket.write_all(&vec![b'a'; 8192]).await.unwrap();
            }
        });
        set_limit_kib(16);
        let start = tokio::time::Instant::now();
        let client = crate::http_client().unwrap();
        let url = format!("http://{address}/package");
        let metadata = async {
            let mut input =
                crate::streaming::HttpRead::open(client.clone(), url.clone(), None::<fn(u64, u64)>)
                    .await
                    .unwrap();
            let mut bytes = vec![];
            input.read_to_end(&mut bytes).await.unwrap();
            bytes
        };
        let range = || async {
            let policy = crate::streaming_http::Policy {
                timeout: Duration::from_millis(100),
                budget: Duration::from_millis(200),
                backoff: Duration::from_millis(2),
                retries: 2,
            };
            let mut input = crate::streaming_http::RangeReader::new(&client, &url, 0, 8191, policy);
            let mut bytes = vec![];
            while let Some(chunk) = input.next().await.unwrap() {
                bytes.extend_from_slice(&chunk);
            }
            bytes
        };
        let (a, b, c, d) = tokio::join!(metadata, range(), range(), range());
        for bytes in [a, b, c, d] {
            assert_eq!(bytes, vec![b'a'; 8192]);
        }
        assert!(start.elapsed() >= Duration::from_millis(1950));
        assert!(start.elapsed() < Duration::from_secs(8));
        set_limit_kib(0);
        server.await.unwrap();
    }
    #[tokio::test]
    async fn streaming_bandwidth_is_aggregate_across_parallel_streams() {
        let limiter = Arc::new(Limiter::new(100_000));
        let start = tokio::time::Instant::now();
        let tasks = (0..4).map(|_| {
            let limiter = limiter.clone();
            tokio::spawn(async move {
                limiter.acquire(5000).await;
            })
        });
        for task in futures_util::future::join_all(tasks).await {
            task.unwrap();
        }
        assert!(start.elapsed() >= Duration::from_millis(195));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[tokio::test]
    async fn streaming_bandwidth_update_and_unlimited_wake_active_waiters() {
        let limiter = Arc::new(Limiter::new(1));
        let active = tokio::spawn({
            let limiter = limiter.clone();
            async move {
                limiter.acquire(16384).await;
            }
        });
        tokio::task::yield_now().await;
        limiter.speed.send_replace(0);
        tokio::time::timeout(Duration::from_secs(1), active)
            .await
            .unwrap()
            .unwrap();
        let start = tokio::time::Instant::now();
        limiter.acquire(1000000).await;
        assert!(start.elapsed() < Duration::from_millis(100));
    }
    #[tokio::test]
    async fn streaming_bandwidth_cancellation_leaves_no_reserved_delay() {
        let limiter = Arc::new(Limiter::new(100_000));
        let cancelled =
            tokio::time::timeout(Duration::from_millis(5), limiter.acquire(100_000)).await;
        assert!(cancelled.is_err());
        let start = tokio::time::Instant::now();
        limiter.acquire(1000).await;
        assert!(start.elapsed() < Duration::from_millis(200));
    }
}
