//! Tests for multi-chunk pause/resume checkpoints, cooperative cancellation,
//! journal compaction, partial free-space credit, and mixed-file concurrency.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::test_server::{Resp, TestServer, serve_bytes};
use crate::{
    Algorithm, CancellationToken, Chunk, Codec, Digest, FileEntry, Finished, Job, Options, Part,
    Progress, ProgressFn, Signer, UrlSigner, download, ignore_progress, path,
};

fn sha256(data: &[u8]) -> Digest {
    use sha2::Digest as _;
    Digest::new(Algorithm::Sha256, sha2::Sha256::digest(data).to_vec())
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(37).wrapping_add(seed))
        .collect()
}

fn client() -> reqwest::Client {
    reqwest::Client::new()
}

/// Build a multi-chunk `FileEntry` backed by `chunks` served at `/c/<key>`.
fn multi_chunk_file(
    server: &TestServer,
    rel: &str,
    chunks: &[(&str, Vec<u8>)],
) -> (FileEntry, Vec<u8>) {
    let mut full = Vec::new();
    let mut parts = Vec::with_capacity(chunks.len());
    for (key, bytes) in chunks {
        full.extend_from_slice(bytes);
        let chunk = Arc::new(Chunk {
            key: (*key).into(),
            urls: vec![server.url(&format!("/c/{key}"))],
            download_size: bytes.len() as u64,
            size: bytes.len() as u64,
            hash: Some(sha256(bytes)),
            codec: Codec::Raw,
            range: None,
        });
        parts.push(Part::new(chunk, 0, bytes.len() as u64));
    }
    let entry = FileEntry {
        path: rel.into(),
        size: full.len() as u64,
        hash: Some(sha256(&full)),
        parts,
        executable: false,
    };
    (entry, full)
}

/// Start a server serving `/c/<key>` that hangs on `stall_key` while `stall` is true.
fn stalling_chunk_server(
    chunks: &[(&str, Vec<u8>)],
    stall_key: &'static str,
    stall: Arc<AtomicBool>,
) -> TestServer {
    let map: HashMap<String, Vec<u8>> = chunks
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect();
    TestServer::start(move |req| {
        let Some(key) = req.path.strip_prefix("/c/") else {
            return Resp::status(404);
        };
        let Some(body) = map.get(key) else {
            return Resp::status(404);
        };
        let mut resp = serve_bytes(req, body, true);
        if key == stall_key && stall.load(Ordering::SeqCst) {
            resp.hang_after = Some(1);
        }
        resp
    })
}

/// Run `download()` until reported `bytes_done >= threshold`, then cancel and
/// return the cancel drain duration.
async fn pause_after_bytes(
    root: &Path,
    files: Vec<FileEntry>,
    opts: Options,
    threshold: usize,
) -> Duration {
    let cancel = CancellationToken::new();
    let done = Arc::new(AtomicUsize::new(0));
    let sink = {
        let done = done.clone();
        Arc::new(move |p: Progress| done.store(p.bytes_done as usize, Ordering::SeqCst))
            as ProgressFn
    };
    let task = {
        let root = root.to_path_buf();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            download(
                &client(),
                Job {
                    root: &root,
                    files: &files,
                    remove: &[],
                },
                &opts,
                sink,
                &cancel,
            )
            .await
        })
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while done.load(Ordering::SeqCst) < threshold {
            assert!(!task.is_finished(), "download finished before pause");
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await
    .expect("download reached pause threshold");
    let started = Instant::now();
    cancel.cancel();
    let outcome = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("pause drains promptly")
        .unwrap()
        .unwrap();
    assert_eq!(outcome, Finished::Cancelled);
    started.elapsed()
}

#[tokio::test]
async fn pause_and_resume_preserves_multi_chunk_progress() {
    let chunks: Vec<(&str, Vec<u8>)> = vec![
        ("p0", pattern(40_000, 1)),
        ("p1", pattern(40_000, 2)),
        ("p2", pattern(40_000, 3)),
        ("p3", pattern(40_000, 4)),
        ("p4", pattern(40_000, 5)),
    ];
    let stall_p3 = Arc::new(AtomicBool::new(true));
    let server = stalling_chunk_server(&chunks, "p3", stall_p3.clone());
    let (file, expected) = multi_chunk_file(&server, "pak/big.pak", &chunks);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let journal = dir.path().join("resume.jsonl");
    let opts = Options {
        workers: 1,
        retries: 1,
        journal: Some(journal.clone()),
        ..Default::default()
    };

    let elapsed = pause_after_bytes(&root, vec![file.clone()], opts.clone(), 120_000).await;
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");

    let part_file = path::part_path(&root.join("pak/big.pak"));
    assert_eq!(std::fs::metadata(&part_file).unwrap().len(), 120_000);
    let journal_text = std::fs::read_to_string(&journal).unwrap();
    assert!(
        journal_text.contains("\"kind\":\"checkpoint\""),
        "{journal_text}"
    );

    // Unblock p3 and resume: p0..p2 must not be fetched again.
    stall_p3.store(false, Ordering::SeqCst);
    let before = server.requests().len();
    let res = download(
        &client(),
        Job {
            root: &root,
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
    assert_eq!(std::fs::read(root.join("pak/big.pak")).unwrap(), expected);
    assert!(!part_file.exists());
    assert!(!journal.exists());

    let resumed_reqs: Vec<String> = server.requests()[before..]
        .iter()
        .map(|r| r.path.clone())
        .collect();
    assert!(
        !resumed_reqs
            .iter()
            .any(|p| p == "/c/p0" || p == "/c/p1" || p == "/c/p2"),
        "already checkpointed chunks were re-fetched: {resumed_reqs:?}"
    );
    assert!(resumed_reqs.iter().any(|p| p == "/c/p3"));
    assert!(resumed_reqs.iter().any(|p| p == "/c/p4"));
}

#[tokio::test]
async fn resumes_when_part_file_has_uncheckpointed_trailing_bytes() {
    let chunks: Vec<(&str, Vec<u8>)> = vec![
        ("c0", pattern(40_000, 10)),
        ("c1", pattern(40_000, 11)),
        ("c2", pattern(40_000, 12)),
    ];
    let stall_c2 = Arc::new(AtomicBool::new(true));
    let server = stalling_chunk_server(&chunks, "c2", stall_c2.clone());
    let (file, expected) = multi_chunk_file(&server, "asset.pak", &chunks);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let journal = dir.path().join("resume.jsonl");
    let opts = Options {
        workers: 1,
        retries: 1,
        journal: Some(journal),
        ..Default::default()
    };

    pause_after_bytes(&root, vec![file.clone()], opts.clone(), 80_000).await;

    // Simulate a crash leaving 32 KiB of uncheckpointed garbage after bytes_done.
    let part_file = path::part_path(&root.join("asset.pak"));
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&part_file)
        .unwrap();
    f.write_all(&vec![0xEE; 32 * 1024]).unwrap();
    drop(f);
    assert_eq!(
        std::fs::metadata(&part_file).unwrap().len(),
        80_000 + 32 * 1024
    );

    stall_c2.store(false, Ordering::SeqCst);
    let before = server.requests().len();
    let res = download(
        &client(),
        Job {
            root: &root,
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
    assert_eq!(std::fs::read(root.join("asset.pak")).unwrap(), expected);
    let resumed_reqs: Vec<String> = server.requests()[before..]
        .iter()
        .map(|r| r.path.clone())
        .collect();
    assert_eq!(resumed_reqs, vec!["/c/c2"]);
}

#[tokio::test]
async fn full_disk_hash_catches_pre_tail_corruption_and_retries_cleanly() {
    // 50 KiB + 50 KiB = 100 KiB checkpointed -> byte 0 lies BEFORE the 64 KiB tail window.
    let chunks: Vec<(&str, Vec<u8>)> = vec![
        ("k0", pattern(50_000, 20)),
        ("k1", pattern(50_000, 21)),
        ("k2", pattern(20_000, 22)),
    ];
    let stall_k2 = Arc::new(AtomicBool::new(true));
    let server = stalling_chunk_server(&chunks, "k2", stall_k2.clone());
    let (file, expected) = multi_chunk_file(&server, "corrupt.pak", &chunks);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let journal = dir.path().join("resume.jsonl");
    let opts = Options {
        workers: 1,
        retries: 1,
        journal: Some(journal),
        ..Default::default()
    };

    pause_after_bytes(&root, vec![file.clone()], opts.clone(), 100_000).await;

    // Flip byte 0 (outside the 64 KiB tail window): resume proceeds, full-file
    // intact() catches it, clears checkpoint, and retry recovers cleanly.
    let part_file = path::part_path(&root.join("corrupt.pak"));
    let mut bytes = std::fs::read(&part_file).unwrap();
    bytes[0] ^= 0xFF;
    std::fs::write(&part_file, &bytes).unwrap();

    stall_k2.store(false, Ordering::SeqCst);
    let before = server.requests().len();
    let res = download(
        &client(),
        Job {
            root: &root,
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
    assert_eq!(std::fs::read(root.join("corrupt.pak")).unwrap(), expected);

    let resumed_reqs: Vec<String> = server.requests()[before..]
        .iter()
        .map(|r| r.path.clone())
        .collect();
    assert!(
        resumed_reqs.iter().any(|p| p == "/c/k0"),
        "{resumed_reqs:?}"
    );
    assert!(
        resumed_reqs.iter().any(|p| p == "/c/k1"),
        "{resumed_reqs:?}"
    );
    assert_eq!(
        resumed_reqs.iter().filter(|p| *p == "/c/k2").count(),
        2,
        "{resumed_reqs:?}"
    );
}

#[tokio::test]
async fn journal_compacts_on_open_and_ignores_truncated_lines() {
    let body = pattern(20_000, 30);
    let chunks = vec![
        ("j0", body[..10_000].to_vec()),
        ("j1", body[10_000..].to_vec()),
    ];
    let server = stalling_chunk_server(&chunks, "", Arc::new(AtomicBool::new(false)));
    let (file, expected) = multi_chunk_file(&server, "f.bin", &chunks);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    std::fs::create_dir_all(&root).unwrap();
    let journal = dir.path().join("resume.jsonl");
    let done_other = pattern(16, 31);
    std::fs::write(root.join("already.bin"), &done_other).unwrap();
    let already = FileEntry::whole(
        "already.bin".into(),
        done_other.len() as u64,
        Some(sha256(&done_other)),
        vec![server.url("/c/never")],
    );
    let raw = format!(
        "{{\"path\":\"already.bin\",\"size\":16,\"hash\":\"{}\"}}\n\
         {{\"kind\":\"checkpoint\",\"path\":\"f.bin\",\"size\":20000,\"parts_fingerprint\":\"old\",\"parts_done\":1,\"bytes_done\":10000,\"tail_sha256\":\"bad\"}}\n\
         {{\"kind\":\"clear_checkpoint\",\"path\":\"f.bin\"}}\n\
         {{\"kind\":\"checkpoint\",\"path\":\"truncated",
        sha256(&done_other)
    );
    std::fs::write(&journal, raw).unwrap();

    let cancel = CancellationToken::new();
    cancel.cancel();
    let opts = Options {
        journal: Some(journal.clone()),
        ..Default::default()
    };
    let res = download(
        &client(),
        Job {
            root: &root,
            files: &[already.clone(), file.clone()],
            remove: &[],
        },
        &opts,
        ignore_progress(),
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(res, Finished::Cancelled);
    let compacted = std::fs::read_to_string(&journal).unwrap();
    let lines: Vec<&str> = compacted.lines().collect();
    assert_eq!(lines.len(), 1, "expected 1 compacted record: {compacted}");
    assert!(lines[0].contains("already.bin"));

    let res = download(
        &client(),
        Job {
            root: &root,
            files: &[already, file],
            remove: &[],
        },
        &opts,
        ignore_progress(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(res, Finished::Done);
    assert_eq!(std::fs::read(root.join("f.bin")).unwrap(), expected);
    assert!(!server.requests().iter().any(|r| r.path == "/c/never"));
}

#[tokio::test]
async fn mixed_game_keeps_small_stream_files_running_alongside_slow_multi_chunk() {
    let slow_chunks: Vec<(&str, Vec<u8>)> =
        vec![("s0", pattern(8_000, 40)), ("s1", pattern(8_000, 41))];
    let small_body = pattern(2_000, 42);
    let slow_map: HashMap<String, Vec<u8>> = slow_chunks
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect();
    let small_clone = small_body.clone();
    let server = TestServer::start(move |req| {
        if let Some(key) = req.path.strip_prefix("/c/")
            && let Some(b) = slow_map.get(key)
        {
            let mut r = serve_bytes(req, b, true);
            r.hang_after = Some(1);
            return r;
        }
        if req.path.starts_with("/small/") {
            return serve_bytes(req, &small_clone, true);
        }
        Resp::status(404)
    });

    // 10 slow multi-chunk files (above the Semaphore(8) cap) BEFORE 12 small stream files.
    let mut files = Vec::new();
    for i in 0..10 {
        let (f, _) = multi_chunk_file(&server, &format!("big_{i}.pak"), &slow_chunks);
        files.push(f);
    }
    for i in 0..12 {
        files.push(FileEntry::whole(
            format!("small_{i}.txt"),
            small_body.len() as u64,
            Some(sha256(&small_body)),
            vec![server.url(&format!("/small/{i}"))],
        ));
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let cancel = CancellationToken::new();
    let task = {
        let root = root.clone();
        let files = files.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let opts = Options {
                workers: 32,
                retries: 0,
                ..Default::default()
            };
            download(
                &client(),
                Job {
                    root: &root,
                    files: &files,
                    remove: &[],
                },
                &opts,
                ignore_progress(),
                &cancel,
            )
            .await
        })
    };

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let done_small = (0..12)
                .filter(|i| root.join(format!("small_{i}.txt")).is_file())
                .count();
            if done_small == 12 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("small stream files completed without starving behind multi-chunk files");

    cancel.cancel();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        Finished::Cancelled
    );
}

#[tokio::test]
async fn skipped_parts_release_shared_chunk_cache_on_resume() {
    let shared_bytes = pattern(70_000, 50);
    let tail_a = pattern(20_000, 51);
    let tail_b = pattern(20_000, 52);
    let all = vec![
        ("shared", shared_bytes.clone()),
        ("tail_a", tail_a.clone()),
        ("tail_b", tail_b.clone()),
    ];
    let stall_tail_a = Arc::new(AtomicBool::new(true));
    let server = stalling_chunk_server(&all, "tail_a", stall_tail_a.clone());
    let mk_chunk = |key: &str, data: &[u8]| {
        Arc::new(Chunk {
            key: key.into(),
            urls: vec![server.url(&format!("/c/{key}"))],
            download_size: data.len() as u64,
            size: data.len() as u64,
            hash: Some(sha256(data)),
            codec: Codec::Raw,
            range: None,
        })
    };
    let c_shared = mk_chunk("shared", &shared_bytes);
    let c_tail_a = mk_chunk("tail_a", &tail_a);
    let c_tail_b = mk_chunk("tail_b", &tail_b);
    let exp_a: Vec<u8> = [shared_bytes.as_slice(), tail_a.as_slice()].concat();
    let exp_b: Vec<u8> = [shared_bytes.as_slice(), tail_b.as_slice()].concat();
    let file_a = FileEntry {
        path: "a.pak".into(),
        size: exp_a.len() as u64,
        hash: Some(sha256(&exp_a)),
        parts: vec![
            Part::new(c_shared.clone(), 0, shared_bytes.len() as u64),
            Part::new(c_tail_a, 0, tail_a.len() as u64),
        ],
        executable: false,
    };
    let file_b = FileEntry {
        path: "b.pak".into(),
        size: exp_b.len() as u64,
        hash: Some(sha256(&exp_b)),
        parts: vec![
            Part::new(c_shared, 0, shared_bytes.len() as u64),
            Part::new(c_tail_b, 0, tail_b.len() as u64),
        ],
        executable: false,
    };

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("game");
    let journal = dir.path().join("resume.jsonl");
    let opts = Options {
        workers: 1,
        retries: 1,
        cache_bytes: 64 * 1024,
        journal: Some(journal),
        ..Default::default()
    };

    pause_after_bytes(
        &root,
        vec![file_a.clone()],
        opts.clone(),
        shared_bytes.len(),
    )
    .await;

    stall_tail_a.store(false, Ordering::SeqCst);
    let before = server.requests().len();
    let res = download(
        &client(),
        Job {
            root: &root,
            files: &[file_a, file_b],
            remove: &[],
        },
        &opts,
        ignore_progress(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(res, Finished::Done);
    assert_eq!(std::fs::read(root.join("a.pak")).unwrap(), exp_a);
    assert_eq!(std::fs::read(root.join("b.pak")).unwrap(), exp_b);
    let shared_fetches = server.requests()[before..]
        .iter()
        .filter(|r| r.path == "/c/shared")
        .count();
    assert_eq!(
        shared_fetches, 1,
        "shared chunk fetched once for b.pak only"
    );
}

struct BlockingSigner;

impl UrlSigner for BlockingSigner {
    fn sign<'a>(
        &'a self,
        _: &'a [String],
    ) -> futures::future::BoxFuture<'a, anyhow::Result<HashMap<String, String>>> {
        Box::pin(futures::future::pending())
    }
}

#[tokio::test]
async fn signing_cancel_does_not_block_multi_chunk_workers() {
    let chunk0 = Arc::new(Chunk {
        key: "signed0".into(),
        download_size: 100,
        size: 100,
        ..Default::default()
    });
    let chunk1 = Arc::new(Chunk {
        key: "signed1".into(),
        download_size: 100,
        size: 100,
        ..Default::default()
    });
    let file = FileEntry {
        path: "signed.bin".into(),
        size: 200,
        hash: None,
        parts: vec![Part::new(chunk0, 0, 100), Part::new(chunk1, 0, 100)],
        executable: false,
    };
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let root = dir.path().to_path_buf();
    let task = tokio::spawn(async move {
        let opts = Options {
            workers: 4,
            signer: Some(Signer(Arc::new(BlockingSigner))),
            ..Default::default()
        };
        download(
            &client(),
            Job {
                root: &root,
                files: &[file],
                remove: &[],
            },
            &opts,
            ignore_progress(),
            &token,
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let started = Instant::now();
    cancel.cancel();
    let res = tokio::time::timeout(Duration::from_millis(200), task)
        .await
        .expect("Signing::url cancelled immediately")
        .unwrap()
        .unwrap();
    assert_eq!(res, Finished::Cancelled);
    assert!(started.elapsed() < Duration::from_millis(200));
}
