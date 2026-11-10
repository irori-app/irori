//! Loopback bytes and counting writers only, never an account or installed game.
use super::*;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

#[derive(Default)]
struct CountingWriter {
    data: Vec<u8>,
    calls: usize,
    largest: usize,
    short: bool,
}
impl AsyncWrite for CountingWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.calls += 1;
        self.largest = self.largest.max(bytes.len());
        let len = if self.short {
            bytes.len().min(3001)
        } else {
            bytes.len()
        };
        self.data.extend_from_slice(&bytes[..len]);
        Poll::Ready(Ok(len))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn streaming_throughput_batches_page_writes_without_changing_plain_bytes_or_short_writes() {
    for short in [false, true] {
        let length: usize = 4 * 1024 * 1024 + 107;
        let stored = length.div_ceil(PAGE_SIZE) * PAGE_SIZE;
        let body: Vec<u8> = (0..stored).map(|i| (i % 251) as u8).collect();
        let expected = body[..length].to_vec();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
            }
            socket.write_all(format!("HTTP/1.1 206 Test\r\nContent-Length: {stored}\r\nContent-Range: bytes 0-{}/{stored}\r\nConnection: close\r\n\r\n",stored-1).as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
        });
        let file = SegmentFile {
            offset: 0,
            length: length as u64,
            keep_encrypted: false,
            data_hashs: vec![],
        };
        let mut writer = CountingWriter {
            short,
            ..Default::default()
        };
        let mut positions = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            XvdFile::download_segment_http(
                &[],
                &crate::http_client().unwrap(),
                &format!("http://{address}/fixture"),
                &mut writer,
                &file,
                [0; 32],
                |_, _| {},
                None,
                0,
                |n| {
                    positions.push(n);
                    Ok(())
                },
            ),
        )
        .await
        .unwrap()
        .unwrap();
        server.await.unwrap();
        assert_eq!(writer.data, expected);
        assert_eq!(positions.last(), Some(&(length as u64)));
        assert!(writer.largest <= 256 * 1024);
        if !short {
            assert!(
                writer.calls <= 17,
                "4MiB should require at most17 writes, got {}",
                writer.calls
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "local disk/loopback benchmark, not an Internet bandwidth claim"]
async fn streaming_throughput_loopback_file_benchmark() {
    let length = 64 * 1024 * 1024;
    let body = vec![37u8; length];
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        socket.write_all(format!("HTTP/1.1 206 Test\r\nContent-Length: {length}\r\nContent-Range: bytes 0-{}/{length}\r\nConnection: close\r\n\r\n",length-1).as_bytes()).await.unwrap();
        socket.write_all(&body).await.unwrap();
    });
    let target = std::env::temp_dir().join(format!("nexus-throughput-{}.bin", Uuid::new_v4()));
    let mut output = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&target)
        .await
        .unwrap();
    let file = SegmentFile {
        offset: 0,
        length: length as u64,
        keep_encrypted: false,
        data_hashs: vec![],
    };
    let start = std::time::Instant::now();
    XvdFile::download_segment_http(
        &[],
        &crate::http_client().unwrap(),
        &format!("http://{address}/fixture"),
        &mut output,
        &file,
        [0; 32],
        |_, _| {},
        None,
        0,
        |_| Ok(()),
    )
    .await
    .unwrap();
    output.sync_all().await.unwrap();
    let elapsed = start.elapsed().as_secs_f64();
    server.await.unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), vec![37u8; length]);
    std::fs::remove_file(&target).unwrap();
    eprintln!(
        "SYNTHETIC_FILE_BENCH bytes={length} seconds={elapsed:.6} MiBps={:.2}",
        64.0 / elapsed
    );
}
