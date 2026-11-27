//! Tests for multi-range `stream_segmented`, FAT32/exFAT sparse fallback,
//! `Accept-Encoding: identity`, dynamic `SharedRateLimiter`, unknown-size
//! downloads, and `.irori-part.seg` resume/space accounting.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::io::AsyncWriteExt as _;

use crate::download::{TestSegmentConfig, download_with_test_config};
use crate::test_server::{Resp, TestServer, serve_bytes};
use crate::{
    Algorithm, CancellationToken, Chunk, Codec, Digest, FileEntry, Finished, Job, Options, Part,
    Progress, ProgressFn, SharedRateLimiter, Signer, UrlSigner, download, ignore_progress,
    partial_bytes, path,
};

fn sha256(data: &[u8]) -> Digest {
    use sha2::Digest as _;
    Digest::new(Algorithm::Sha256, sha2::Sha256::digest(data).to_vec())
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| ((i as u32).wrapping_mul(251).wrapping_add(seed as u32) ^ ((i >> 8) as u32)) as u8)
        .collect()
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

fn crc32(data: &[u8]) -> Digest {
    let mut c = flate2::Crc::new();
    c.update(data);
    Digest::new(Algorithm::Crc32, c.sum().to_be_bytes().to_vec())
}

/// Real 32 MiB threshold test through the public `download()` entry point.
#[tokio::test]
async fn downloads_32mib_file_with_parallel_ranges_and_identity_encoding() {
    // Tile a 4 KiB varied block + CRC-32 so unoptimized `profile = ci` builds
    // verify the full 32 MiB transfer rapidly without starving sibling tests.
    let tile = pattern(4096, 1);
    let mut raw = vec![0u8; 32 << 20];
    for (idx, chunk) in raw.chunks_exact_mut(4096).enumerate() {
        chunk.copy_from_slice(&tile);
        chunk[0] = (idx & 0xFF) as u8;
        chunk[1] = ((idx >> 8) & 0xFF) as u8;
    }
    let body = Arc::new(raw);
    let digest = crc32(&body);
    let served = body.clone();
    let server = TestServer::start(move |req| {
        let mut resp = serve_bytes(req, &served, true);
        resp.headers.push(("ETag".into(), "\"v1-32m\"".into()));
        resp
    });
    let file = FileEntry::whole(
        "big32m.bin".into(),
        body.len() as u64,
        Some(digest),
        vec![server.url("/big32m.bin")],
    );
    let dir = tempfile::tempdir().unwrap();
    let opts = Options {
        workers: 8,
        retries: 1,
        ..Default::default()
    };
    let res = download(
        &client(),
        Job {
            root: dir.path(),
            files: &[file],
            remove: &[],
        },
        &opts,
        ignore_progress(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(res, Finished::Done);
    let dest = dir.path().join("big32m.bin");
    assert_eq!(std::fs::read(&dest).unwrap(), *body);
    assert!(!path::part_path(&dest).exists());
    assert!(!path::seg_path(&dest).exists());

    let reqs = server.requests();
    assert!(
        reqs.iter()
            .all(|r| r.header("accept-encoding") == Some("identity")),
        "every request must send Accept-Encoding: identity: {reqs:?}"
    );
    // On NTFS / Unix where sparse is supported, we expect probe `bytes=0-0` + 2 segments.
    let probe_count = reqs
        .iter()
        .filter(|r| r.header("range") == Some("bytes=0-0"))
        .count();
    assert_eq!(probe_count, 1, "expected 1 Range probe: {reqs:?}");
    let range_reqs = reqs
        .iter()
        .filter(|r| r.header("range").is_some_and(|h| h != "bytes=0-0"))
        .count();
    assert!(
        range_reqs >= 2,
        "expected parallel range requests: {reqs:?}"
    );
}

#[tokio::test]
async fn segmented_pause_resume_tracks_real_partial_bytes_and_skips_completed_ranges() {
    let body = Arc::new(pattern(1 << 20, 2)); // 1 MiB across 4 segments of 256 KiB
    let digest = sha256(&body);
    let served = body.clone();
    let stall_tail = Arc::new(AtomicBool::new(true));
    let stall_flag = stall_tail.clone();
    let server = TestServer::start(move |req| {
        let mut resp = serve_bytes(req, &served, true);
        resp.headers.push(("ETag".into(), "\"etag-seg\"".into()));
        // Stall the last segment (>= 768 KiB) during pass 1 so earlier segments finish.
        if stall_flag.load(Ordering::SeqCst)
            && let Some(r) = req.header("range").and_then(|h| h.strip_prefix("bytes="))
            && let Some((start, _)) = r.split_once('-')
            && start.parse::<usize>().is_ok_and(|s| s >= 768 * 1024)
        {
            resp.hang_after = Some(1);
        }
        resp
    });
    let file = FileEntry::whole(
        "seg.bin".into(),
        body.len() as u64,
        Some(digest),
        vec![server.url("/seg.bin")],
    );
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("seg.bin");
    let opts = Options {
        workers: 8,
        retries: 1,
        ..Default::default()
    };

    let cancel = CancellationToken::new();
    let done = Arc::new(AtomicUsize::new(0));
    let sink = {
        let done = done.clone();
        Arc::new(move |p: Progress| done.store(p.bytes_done as usize, Ordering::SeqCst))
            as ProgressFn
    };
    let task = {
        let root = dir.path().to_path_buf();
        let files = [file.clone()];
        let opts = opts.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            download_with_test_config(
                &client(),
                Job {
                    root: &root,
                    files: &files,
                    remove: &[],
                },
                &opts,
                sink,
                &cancel,
                TestSegmentConfig {
                    min_size: 512 * 1024,
                    target_size: 256 * 1024,
                    sparse_fn: irori_core::fs_part::set_sparse_if_supported,
                },
            )
            .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while done.load(Ordering::SeqCst) < 768 * 1024 {
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("first 3 segments completed");
    cancel.cancel();
    assert_eq!(task.await.unwrap().unwrap(), Finished::Cancelled);

    // Sparse `.irori-part` has logical size 1 MiB, while `partial_bytes` reports real progress (< 1 MiB).
    assert!(path::part_path(&dest).is_file());
    assert!(path::seg_path(&dest).is_file());
    let real_partial = partial_bytes(&dest).unwrap();
    assert!(
        real_partial >= 768 * 1024 && real_partial < body.len() as u64,
        "partial_bytes={real_partial}"
    );

    // Resume: unblock the 4th segment; only probe + 4th segment should be requested.
    stall_tail.store(false, Ordering::SeqCst);
    let before = server.requests().len();
    let res = download_with_test_config(
        &client(),
        Job {
            root: dir.path(),
            files: &[file],
            remove: &[],
        },
        &opts,
        ignore_progress(),
        &CancellationToken::new(),
        TestSegmentConfig {
            min_size: 512 * 1024,
            target_size: 256 * 1024,
            sparse_fn: irori_core::fs_part::set_sparse_if_supported,
        },
    )
    .await
    .unwrap();
    assert_eq!(res, Finished::Done);
    assert_eq!(std::fs::read(&dest).unwrap(), *body);
    assert!(!path::part_path(&dest).exists());
    assert!(!path::seg_path(&dest).exists());

    let resumed = &server.requests()[before..];
    // 1 probe (`bytes=0-0`) + 1 remaining segment request starting at exact `real_partial`.
    assert_eq!(resumed.len(), 2, "{resumed:?}");
    assert_eq!(resumed[0].header("range"), Some("bytes=0-0"));
    let expected_range = format!("bytes={real_partial}-{}", body.len() - 1);
    assert_eq!(
        resumed[1].header("range"),
        Some(expected_range.as_str()),
        "{resumed:?}"
    );
}

#[tokio::test]
async fn falls_back_to_single_stream_when_sparse_unsupported_fat32() {
    let body = pattern(600 * 1024, 3);
    let served = body.clone();
    let server = TestServer::start(move |req| serve_bytes(req, &served, true));
    let file = FileEntry::whole(
        "fat32.bin".into(),
        body.len() as u64,
        Some(sha256(&body)),
        vec![server.url("/fat32.bin")],
    );
    let dir = tempfile::tempdir().unwrap();
    let res = download_with_test_config(
        &client(),
        Job {
            root: dir.path(),
            files: &[file],
            remove: &[],
        },
        &Options {
            workers: 4,
            retries: 1,
            ..Default::default()
        },
        ignore_progress(),
        &CancellationToken::new(),
        TestSegmentConfig {
            min_size: 256 * 1024,
            target_size: 128 * 1024,
            sparse_fn: |_| false, // Simulate FAT32/exFAT returning false from FSCTL_SET_SPARSE
        },
    )
    .await
    .unwrap();
    assert_eq!(res, Finished::Done);
    let dest = dir.path().join("fat32.bin");
    assert_eq!(std::fs::read(&dest).unwrap(), body);
    assert!(!path::seg_path(&dest).exists());

    let reqs = server.requests();
    // 1 probe (`bytes=0-0`), then 1 un-ranged single-stream GET after sparse fallback.
    assert_eq!(reqs.len(), 2, "{reqs:?}");
    assert_eq!(reqs[0].header("range"), Some("bytes=0-0"));
    assert!(reqs[1].header("range").is_none());
}

#[tokio::test]
async fn falls_back_when_server_ignores_range_and_skips_subrange_chunks() {
    let body = pattern(600 * 1024, 4);
    let served = body.clone();
    let no_range_server = TestServer::start(move |req| serve_bytes(req, &served, false));
    let file = FileEntry::whole(
        "norange.bin".into(),
        body.len() as u64,
        Some(sha256(&body)),
        vec![no_range_server.url("/norange.bin")],
    );
    let dir = tempfile::tempdir().unwrap();
    let res = download_with_test_config(
        &client(),
        Job {
            root: dir.path(),
            files: &[file],
            remove: &[],
        },
        &Options {
            workers: 4,
            ..Default::default()
        },
        ignore_progress(),
        &CancellationToken::new(),
        TestSegmentConfig {
            min_size: 256 * 1024,
            target_size: 128 * 1024,
            sparse_fn: irori_core::fs_part::set_sparse_if_supported,
        },
    )
    .await
    .unwrap();
    assert_eq!(res, Finished::Done);
    assert_eq!(std::fs::read(dir.path().join("norange.bin")).unwrap(), body);
    assert_eq!(no_range_server.requests().len(), 2);

    // Sub-range chunk (`chunk.range.is_some()`, like GOG v1/Linux) must NEVER probe `bytes=0-0`.
    let archive = [vec![0u8; 64], body.clone()].concat();
    let ranged_server = TestServer::start(move |req| serve_bytes(req, &archive, true));
    let chunk = Arc::new(Chunk {
        key: "gog-sub".into(),
        urls: vec![ranged_server.url("/archive")],
        download_size: body.len() as u64,
        size: body.len() as u64,
        hash: None,
        codec: Codec::Raw,
        range: Some((64, body.len() as u64)),
    });
    let sub_entry = FileEntry {
        path: "subrange.bin".into(),
        size: body.len() as u64,
        hash: Some(sha256(&body)),
        parts: vec![Part::new(chunk, 0, body.len() as u64)],
        executable: false,
    };
    let res = download_with_test_config(
        &client(),
        Job {
            root: dir.path(),
            files: &[sub_entry],
            remove: &[],
        },
        &Options {
            workers: 4,
            ..Default::default()
        },
        ignore_progress(),
        &CancellationToken::new(),
        TestSegmentConfig {
            min_size: 256 * 1024,
            target_size: 128 * 1024,
            sparse_fn: irori_core::fs_part::set_sparse_if_supported,
        },
    )
    .await
    .unwrap();
    assert_eq!(res, Finished::Done);
    assert_eq!(
        std::fs::read(dir.path().join("subrange.bin")).unwrap(),
        body
    );
    let reqs = ranged_server.requests();
    assert_eq!(reqs.len(), 1, "sub-range file used single ranged GET");
    assert_eq!(
        reqs[0].header("range"),
        Some(format!("bytes=64-{}", 64 + body.len() - 1).as_str())
    );
}

struct ExpiringSigner {
    base: String,
    rounds: AtomicUsize,
}

impl UrlSigner for ExpiringSigner {
    fn sign<'a>(
        &'a self,
        keys: &'a [String],
    ) -> futures::future::BoxFuture<'a, anyhow::Result<HashMap<String, String>>> {
        Box::pin(async move {
            let round = self.rounds.fetch_add(1, Ordering::SeqCst);
            let sig = if round == 0 { "expired" } else { "fresh" };
            Ok(keys
                .iter()
                .map(|k| (k.clone(), format!("{}/signed?sig={sig}", self.base)))
                .collect())
        })
    }
}

#[tokio::test]
async fn segmented_resigns_on_403_and_invalidates_changed_etag() {
    let body = pattern(600 * 1024, 5);
    let served = body.clone();
    let server = TestServer::start(move |req| {
        if !req.path.ends_with("?sig=fresh") {
            return Resp::status(403);
        }
        let mut r = serve_bytes(req, &served, true);
        r.headers.push(("ETag".into(), "\"v2\"".into()));
        r
    });
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("signed.bin");
    // Seed a stale `.irori-part` + `.irori-part.seg` with an old ETag `"v1"`.
    std::fs::write(path::part_path(&dest), vec![0u8; body.len()]).unwrap();
    std::fs::write(
        path::seg_path(&dest),
        format!(
            "{{\"size\":{},\"etag_or_lm\":\"\\\"v1\\\"\",\"segments\":[{{\"start\":0,\"end\":{},\"done\":{}}}]}}",
            body.len(),
            body.len() - 1,
            body.len()
        ),
    )
    .unwrap();

    let signer = Arc::new(ExpiringSigner {
        base: server.base(),
        rounds: AtomicUsize::new(0),
    });
    let file = FileEntry::whole(
        "signed.bin".into(),
        body.len() as u64,
        Some(sha256(&body)),
        vec![],
    );
    let res = download_with_test_config(
        &client(),
        Job {
            root: dir.path(),
            files: &[file],
            remove: &[],
        },
        &Options {
            workers: 4,
            retries: 2,
            signer: Some(Signer(signer.clone())),
            ..Default::default()
        },
        ignore_progress(),
        &CancellationToken::new(),
        TestSegmentConfig {
            min_size: 256 * 1024,
            target_size: 128 * 1024,
            sparse_fn: irori_core::fs_part::set_sparse_if_supported,
        },
    )
    .await
    .unwrap();
    assert_eq!(res, Finished::Done);
    assert_eq!(std::fs::read(&dest).unwrap(), body);
    assert_eq!(signer.rounds.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn downloads_unknown_size_stream_without_content_length() {
    let payload = pattern(150_000, 6);
    let sent = payload.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut req_buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut req_buf).await;
            let head = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n";
            let _ = sock.write_all(head).await;
            for chunk in sent.chunks(16 * 1024) {
                let _ = sock.write_all(chunk).await;
            }
            let _ = sock.shutdown().await;
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let file = FileEntry::whole(
        "unknown.bin".into(),
        0,
        None,
        vec![format!("http://{addr}/stream")],
    );
    let seen = Arc::new(Mutex::new(Vec::<Progress>::new()));
    let sink = {
        let seen = seen.clone();
        Arc::new(move |p: Progress| seen.lock().push(p)) as ProgressFn
    };
    let opts = Options {
        allow_unknown_size: true,
        ..Default::default()
    };
    let res = download(
        &client(),
        Job {
            root: dir.path(),
            files: &[file],
            remove: &[],
        },
        &opts,
        sink,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(res, Finished::Done);
    assert_eq!(
        std::fs::read(dir.path().join("unknown.bin")).unwrap(),
        payload
    );
    let last = *seen.lock().last().unwrap();
    assert_eq!(last.bytes_done, payload.len() as u64);
    assert_eq!(last.bytes_total, 0);
}

#[tokio::test]
async fn dynamic_shared_rate_limiter_updates_bottleneck_live() {
    let body = pattern(1_200_000, 7);
    let served = body.clone();
    let server = TestServer::start(move |req| serve_bytes(req, &served, true));
    let limiter = SharedRateLimiter::new(400_000);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let seen = seen.clone();
        Arc::new(move |p: Progress| seen.lock().push(p.bottleneck)) as ProgressFn
    };
    let dir = tempfile::tempdir().unwrap();
    let file = FileEntry::whole(
        "throttled.bin".into(),
        body.len() as u64,
        Some(sha256(&body)),
        vec![server.url("/throttled.bin")],
    );
    let opts = Options {
        rate_limiter: Some(limiter.clone()),
        ..Default::default()
    };
    let l = limiter.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(600)).await;
        // Lift the cap dynamically mid-download so it finishes quickly.
        l.set_limit(0);
    });
    let res = download(
        &client(),
        Job {
            root: dir.path(),
            files: &[file],
            remove: &[],
        },
        &opts,
        sink,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(res, Finished::Done);
    assert_eq!(
        std::fs::read(dir.path().join("throttled.bin")).unwrap(),
        body
    );
    assert!(
        seen.lock()
            .contains(&Some(irori_core::Bottleneck::SpeedLimit)),
        "{:?}",
        seen.lock()
    );
}

#[tokio::test]
async fn segmented_fails_over_across_mirrors_on_status_error() {
    let body = pattern(600 * 1024, 8);
    let bad = TestServer::start(|_| Resp::status(404));
    let served = body.clone();
    let good = TestServer::start(move |req| serve_bytes(req, &served, true));
    let file = FileEntry::whole(
        "mirror.bin".into(),
        body.len() as u64,
        Some(sha256(&body)),
        vec![bad.url("/mirror.bin"), good.url("/mirror.bin")],
    );
    let dir = tempfile::tempdir().unwrap();
    let res = download_with_test_config(
        &client(),
        Job {
            root: dir.path(),
            files: &[file],
            remove: &[],
        },
        &Options {
            workers: 4,
            retries: 0,
            ..Default::default()
        },
        ignore_progress(),
        &CancellationToken::new(),
        TestSegmentConfig {
            min_size: 256 * 1024,
            target_size: 128 * 1024,
            sparse_fn: irori_core::fs_part::set_sparse_if_supported,
        },
    )
    .await
    .unwrap();
    assert_eq!(res, Finished::Done);
    assert_eq!(std::fs::read(dir.path().join("mirror.bin")).unwrap(), body);
    assert_eq!(bad.requests().len(), 1);
    assert!(good.requests().len() >= 3);
}

#[tokio::test]
async fn segmented_corrupt_cancels_siblings_immediately_and_retries_cleanly() {
    let body = Arc::new(pattern(512 * 1024, 9));
    let served = body.clone();
    let corrupt_once = Arc::new(AtomicBool::new(true));
    let corrupt_flag = corrupt_once.clone();
    let server = TestServer::start(move |req| {
        if req.header("range") == Some("bytes=0-0") {
            return serve_bytes(req, &served, true);
        }
        if corrupt_flag.load(Ordering::SeqCst)
            && let Some(r) = req.header("range").and_then(|h| h.strip_prefix("bytes="))
            && let Some((start, _)) = r.split_once('-')
        {
            let start = start.parse::<usize>().unwrap_or(0);
            if start == 0 {
                // Segment 0 returns a corrupt Content-Range once; then flip flag so retry succeeds.
                corrupt_flag.store(false, Ordering::SeqCst);
                return Resp {
                    status: 206,
                    headers: vec![(
                        "Content-Range".into(),
                        format!("bytes 99-100/{}", served.len()),
                    )],
                    body: vec![0; 2],
                    ..Default::default()
                };
            }
            // Sibling segments sit in transient 503 backoff — must wake immediately on seg_cancel.
            return Resp::status(503);
        }
        serve_bytes(req, &served, true)
    });
    let file = FileEntry::whole(
        "corrupt_seg.bin".into(),
        body.len() as u64,
        Some(sha256(&body)),
        vec![server.url("/corrupt_seg.bin")],
    );
    let dir = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    let res = tokio::time::timeout(
        Duration::from_secs(5),
        download_with_test_config(
            &client(),
            Job {
                root: dir.path(),
                files: &[file],
                remove: &[],
            },
            &Options {
                workers: 4,
                retries: 2,
                ..Default::default()
            },
            ignore_progress(),
            &CancellationToken::new(),
            TestSegmentConfig {
                min_size: 256 * 1024,
                target_size: 128 * 1024,
                sparse_fn: irori_core::fs_part::set_sparse_if_supported,
            },
        ),
    )
    .await
    .expect("sibling segments did not hang on corrupt error")
    .unwrap();
    assert_eq!(res, Finished::Done);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    let dest = dir.path().join("corrupt_seg.bin");
    assert_eq!(std::fs::read(&dest).unwrap(), *body);
    assert!(!path::part_path(&dest).exists());
    assert!(!path::seg_path(&dest).exists());
}
