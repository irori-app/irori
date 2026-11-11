//! Isolated update throughput contracts. Only synthetic files and loopback HTTP.
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::io::AsyncReadExt;

#[test]
fn streaming_update_pipeline_builds_relocation_index_only_when_needed() {
    let mut old = segment(4 * PAGE_SIZE, true);
    old.data_hashs = vec![[1; 20], [2; 20], [1; 20], [4; 20]];
    let mut index = None;
    assert_eq!(
        lazy_candidates(&old, &old, &mut index, 0, 4),
        vec![Some(0), Some(1), Some(2), Some(3)]
    );
    assert!(
        index.is_none(),
        "same-position data must not index the whole archive"
    );
    let mut changed = segment(4 * PAGE_SIZE, true);
    changed.data_hashs = vec![[1; 20], [1; 20], [1; 20], [9; 20]];
    assert_eq!(
        lazy_candidates(&old, &changed, &mut index, 0, 4),
        vec![Some(0), Some(0), Some(2), None]
    );
    assert_eq!(index.as_ref().unwrap().len(), 3);
    assert_eq!(
        lazy_candidates(&old, &old, &mut index, 0, 4),
        vec![Some(0), Some(1), Some(2), Some(3)]
    );
}

#[tokio::test]
async fn streaming_update_pipeline_overlaps_but_settles_current_write_before_lookahead_failure() {
    let (started, start) = tokio::sync::oneshot::channel();
    let (prepared, ready) = tokio::sync::oneshot::channel();
    let wrote = std::cell::Cell::new(false);
    let writing = async {
        started.send(()).unwrap();
        ready.await.unwrap();
        wrote.set(true);
        Ok(())
    };
    let preparing = async {
        start.await.unwrap();
        prepared.send(()).unwrap();
        Err::<(), failure::Error>(io::Error::other("synthetic lookahead failure").into())
    };
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        prefetch_while_writing(writing, preparing),
    )
    .await
    .unwrap();
    assert!(result.is_err());
    assert!(
        wrote.get(),
        "lookahead failure must not drop the preceding write"
    );
}

#[tokio::test]
async fn streaming_update_pipeline_write_failure_cancels_lookahead_without_detached_work() {
    struct DropFlag(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let guard = DropFlag(dropped.clone());
    let preparing = async move {
        let _guard = guard;
        std::future::pending::<Result<(), failure::Error>>().await
    };
    let result = prefetch_while_writing(
        async { Err(io::Error::other("synthetic output failure").into()) },
        preparing,
    )
    .await;
    assert!(result.is_err());
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streaming_update_pipeline_checkpoint_failure_is_not_retried() {
    let root = tempfile::tempdir().unwrap();
    let length = 33 * 1024 * 1024;
    let source = root.path().join("installed");
    let body = vec![37; length];
    fs::write(&source, &body).unwrap();
    let digest = irori_msixvc::streaming::resume_digest(&body[..PAGE_SIZE]);
    let hash = std::array::from_fn(|n| u8::from_str_radix(&digest[n * 2..n * 2 + 2], 16).unwrap());
    let file = SegmentFile {
        offset: 0,
        length: length as u64,
        keep_encrypted: true,
        data_hashs: vec![hash; length / PAGE_SIZE],
    };
    let target = root.path().join("staged");
    let mut out = tokio::fs::File::create(&target).await.unwrap();
    let (_control, pause) = watch::channel(false);
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let mut calls = 0;
    let result = transfer(
        &xvd().await,
        &reqwest::Client::new(),
        "http://127.0.0.1:1/not-requested",
        &source,
        Some(&file),
        &file,
        &mut out,
        [0; 32],
        0,
        pause,
        8,
        &tx,
        |proof| {
            assert_eq!(proof.position(), 32 * 1024 * 1024);
            calls += 1;
            Err(io::Error::other("synthetic journal failure"))
        },
    )
    .await;
    assert!(result.is_err());
    assert_eq!(
        calls, 1,
        "journal failures must not trigger an implicit retry"
    );
    out.flush().await.unwrap();
    assert_eq!(fs::metadata(target).unwrap().len(), 32 * 1024 * 1024);
    let mut reused = 0;
    while let Ok(event) = rx.try_recv() {
        if let ProgressEvent::Traffic { network, reused: r } = event {
            assert_eq!(network, 0);
            reused += r;
        }
    }
    assert_eq!(reused, 32 * 1024 * 1024);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streaming_update_pipeline_pause_keeps_lookahead_out_of_progress_and_resume_checkpoint() {
    let root = tempfile::tempdir().unwrap();
    let length = 33 * 1024 * 1024;
    let source = root.path().join("installed");
    let body = vec![37; length];
    fs::write(&source, &body).unwrap();
    let digest = irori_msixvc::streaming::resume_digest(&body[..PAGE_SIZE]);
    let hash = std::array::from_fn(|n| u8::from_str_radix(&digest[n * 2..n * 2 + 2], 16).unwrap());
    let file = SegmentFile {
        offset: 0,
        length: length as u64,
        keep_encrypted: true,
        data_hashs: vec![hash; length / PAGE_SIZE],
    };
    let files = HashMap::from([("staged".to_owned(), segment(length, true))]);
    let mut journal = resume::Journal::open(root.path(), "pipeline-pause", &files).unwrap();
    let target = root.path().join("staged");
    let mut out = tokio::fs::File::create(&target).await.unwrap();
    let (control, pause) = watch::channel(false);
    let (stop, stopped) = watch::channel(false);
    let (done, wait) = tokio::sync::oneshot::channel();
    let mut done = Some(done);
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let client = reqwest::Client::new();
    let xvd = xvd().await;
    let operation = failure::run_or_cancel(
        stopped,
        transfer(
            &xvd,
            &client,
            "http://127.0.0.1:1/not-requested",
            &source,
            Some(&file),
            &file,
            &mut out,
            [0; 32],
            0,
            pause.clone(),
            8,
            &tx,
            |proof| {
                let position = proof.position();
                journal.checkpoint_synced("staged", proof)?;
                if position == 32 * 1024 * 1024 {
                    control.send(true).unwrap();
                    if let Some(done) = done.take() {
                        done.send(()).unwrap();
                    }
                }
                Ok(())
            },
        ),
    );
    let driver = async {
        wait.await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert_eq!(fs::metadata(&target).unwrap().len(), 32 * 1024 * 1024);
        stop.send(true).unwrap();
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(operation, driver)
    })
    .await
    .unwrap();
    assert!(result.is_err());
    out.flush().await.unwrap();
    assert_eq!(journal.offset("staged", file.length), 32 * 1024 * 1024);
    let mut reused = 0;
    while let Ok(event) = rx.try_recv() {
        if let ProgressEvent::Traffic { network, reused: r } = event {
            assert_eq!(network, 0);
            reused += r;
        }
    }
    assert_eq!(
        reused,
        32 * 1024 * 1024,
        "prefetched final MiB is not progress"
    );
    control.send(false).unwrap();
    let offset = journal.offset("staged", file.length);
    assert_eq!(
        resume::prepare_output(&mut out, offset, &file)
            .await
            .unwrap(),
        offset
    );
    transfer(
        &xvd,
        &client,
        "http://127.0.0.1:1/not-requested",
        &source,
        Some(&file),
        &file,
        &mut out,
        [0; 32],
        offset,
        pause,
        8,
        &tx,
        |proof| journal.checkpoint_synced("staged", proof),
    )
    .await
    .unwrap();
    out.flush().await.unwrap();
    journal.checkpoint("staged", file.length).unwrap();
    assert_eq!(fs::read(target).unwrap(), body);
    assert_eq!(journal.offset("staged", file.length), file.length);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "synthetic reused-data disk benchmark, no game or account"]
async fn streaming_update_reused_disk_benchmark() {
    use std::io::Write;
    let parent =
        std::env::var_os("NEXUS_BENCH_ROOT").expect("explicit fresh fixture parent required");
    let root = tempfile::Builder::new()
        .prefix("reuse-")
        .tempdir_in(parent)
        .unwrap();
    let length = 512 * 1024 * 1024;
    let source = root.path().join("installed.bin");
    let mut disk =
        std::io::BufWriter::with_capacity(1024 * 1024, fs::File::create(&source).unwrap());
    let mut hashes = Vec::with_capacity(length / PAGE_SIZE);
    for n in 0..length / PAGE_SIZE {
        let mut page = [0u8; PAGE_SIZE];
        let mut seed = n as u64 + 1;
        for word in page.chunks_exact_mut(8) {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            word.copy_from_slice(&seed.to_le_bytes());
        }
        disk.write_all(&page).unwrap();
        let digest = irori_msixvc::streaming::resume_digest(&page);
        hashes.push(std::array::from_fn(|n| {
            u8::from_str_radix(&digest[n * 2..n * 2 + 2], 16).unwrap()
        }));
    }
    disk.flush().unwrap();
    disk.get_ref().sync_all().unwrap();
    drop(disk);
    let file = SegmentFile {
        offset: 0,
        length: length as u64,
        keep_encrypted: true,
        data_hashs: hashes,
    };
    let files = HashMap::from([(
        "staged".to_owned(),
        SegmentFile {
            offset: 0,
            length: file.length,
            keep_encrypted: true,
            data_hashs: file.data_hashs.clone(),
        },
    )]);
    let mut journal = resume::Journal::open(root.path(), "synthetic-reuse", &files).unwrap();
    let target = root.path().join("staged");
    let mut out = tokio::fs::File::create(&target).await.unwrap();
    let xvd = xvd().await;
    let client = reqwest::Client::new();
    let (_control, pause) = watch::channel(false);
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let started = std::time::Instant::now();
    let mut checkpoints = 0;
    transfer(
        &xvd,
        &client,
        "http://127.0.0.1:1/not-requested",
        &source,
        Some(&file),
        &file,
        &mut out,
        [0; 32],
        0,
        pause,
        8,
        &tx,
        |p| {
            checkpoints += 1;
            journal.checkpoint_synced("staged", p)
        },
    )
    .await
    .unwrap();
    out.flush().await.unwrap();
    journal.checkpoint("staged", file.length).unwrap();
    let elapsed = started.elapsed().as_secs_f64();
    drop(tx);
    let mut network = 0;
    let mut reused = 0;
    while let Some(event) = rx.recv().await {
        if let ProgressEvent::Traffic {
            network: n,
            reused: r,
        } = event
        {
            network += n;
            reused += r;
        }
    }
    assert_eq!((network, reused), (0, file.length));
    assert_eq!(checkpoints, 16);
    assert_eq!(journal.offset("staged", file.length), file.length);
    let mut a = fs::File::open(source).unwrap();
    let mut b = fs::File::open(target).unwrap();
    let mut aa = vec![0; 1024 * 1024];
    let mut bb = vec![0; 1024 * 1024];
    for _ in 0..512 {
        a.read_exact(&mut aa).unwrap();
        b.read_exact(&mut bb).unwrap();
        assert_eq!(aa, bb);
    }
    tracing::debug!(
        "REUSED_DISK_BENCH bytes={length} seconds={elapsed:.6} MiBps={:.2} checkpoints={checkpoints}",
        512.0 / elapsed
    );
}

fn segment(length: usize, encrypted: bool) -> SegmentFile {
    SegmentFile {
        offset: 0,
        length: length as u64,
        keep_encrypted: encrypted,
        data_hashs: vec![],
    }
}
async fn xvd() -> XvdFile {
    let mut header = vec![0u8; 0x3000];
    header[512..520].copy_from_slice(b"msft-xvd");
    header[528..536].copy_from_slice(&116444736000000000u64.to_le_bytes());
    header[652..656].copy_from_slice(&4u32.to_le_bytes());
    XvdFile::parse(&mut std::io::Cursor::new(header))
        .await
        .unwrap()
}

#[tokio::test]
async fn streaming_update_throughput_prefers_same_position_and_reads_contiguous_pages_once() {
    struct Count {
        data: std::io::Cursor<Vec<u8>>,
        bytes: usize,
        reads: usize,
    }
    impl tokio::io::AsyncRead for Count {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
            data: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            self.reads += 1;
            let before = data.filled().len();
            let result = std::pin::Pin::new(&mut self.data).poll_read(cx, data);
            self.bytes += data.filled().len() - before;
            result
        }
    }
    impl tokio::io::AsyncSeek for Count {
        fn start_seek(mut self: std::pin::Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
            std::pin::Pin::new(&mut self.data).start_seek(position)
        }
        fn poll_complete(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<io::Result<u64>> {
            std::pin::Pin::new(&mut self.data).poll_complete(cx)
        }
    }
    // Every page shares a hash, the old first-match index would select page0
    // repeatedly and invalidate its1MiB read-ahead buffer for each4KiB page.
    let mut old = segment(8 * 1024 * 1024, true);
    old.data_hashs = vec![[7; 20]; 2048];
    let index = HashMap::from([([7; 20], 0)]);
    let candidates = local_candidates(&old, &old, &index, 0, 2048);
    assert_eq!(candidates, (0..2048).map(Some).collect::<Vec<_>>());
    let data = vec![9; 8 * 1024 * 1024];
    let mut reader = Count {
        data: std::io::Cursor::new(data.clone()),
        bytes: 0,
        reads: 0,
    };
    let mut output = vec![0; data.len()];
    assert!(
        read_local_pages(&mut reader, &mut output, &candidates)
            .await
            .into_iter()
            .all(|read| read)
    );
    assert_eq!(output, data);
    assert_eq!((reader.bytes, reader.reads), (data.len(), 1));
    let partial = read_local_pages(
        &mut reader,
        &mut [0; PAGE_SIZE * 4],
        &[Some(2), None, Some(0), Some(1)],
    )
    .await;
    assert_eq!(partial, [true, false, true, true]);
    assert!(!read_local_pages(&mut reader, &mut [0; PAGE_SIZE], &[Some(9999)]).await[0]);
}

#[test]
fn streaming_update_throughput_ranges_cover_all_gaps_once_with_bounded_memory() {
    for mask in 0u32..1024 {
        let reused: Vec<_> = (0..10).map(|i| mask & (1 << i) != 0).collect();
        let ranges = missing_ranges(&reused);
        let mut fetched = vec![0; reused.len()];
        for range in ranges {
            assert!(range.len() <= 256);
            assert!(!range.is_empty());
            for n in range {
                fetched[n] += 1;
            }
        }
        for (n, local) in reused.into_iter().enumerate() {
            assert!(fetched[n] <= 1);
            if !local {
                assert_eq!(fetched[n], 1);
            }
        }
    }
    assert_eq!(missing_ranges(&[false; 2048]).len(), 8);
    let mut islands = vec![true; 2048];
    islands[0] = false;
    islands[100] = false;
    assert_eq!(missing_ranges(&islands), vec![0..1, 100..101]);
}

async fn server(
    body: Arc<Vec<u8>>,
    count: usize,
    concurrency: usize,
    fail_after: Option<usize>,
) -> (String, tokio::task::JoinHandle<usize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let barrier = Arc::new(tokio::sync::Barrier::new(concurrency.min(count)));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..count {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (body, barrier, active, peak) =
                (body.clone(), barrier.clone(), active.clone(), peak.clone());
            tasks.spawn(async move {
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") { request.push(socket.read_u8().await.unwrap()); assert!(request.len() < 8192); }
                let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
                let range = request.lines().find_map(|line| line.strip_prefix("range: bytes=")).unwrap();
                let (start, end) = range.split_once('-').unwrap();
                let (start,end) = (start.parse::<usize>().unwrap(),end.parse::<usize>().unwrap());
                assert_eq!(start % PAGE_SIZE, 0);
                assert!(end - start < 1024 * 1024);
                assert!(end < body.len());
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                // The first group cannot finish unless requests really overlap.
                if index < concurrency.min(count) { barrier.wait().await; }
                active.fetch_sub(1, Ordering::SeqCst);
                if fail_after.is_some_and(|limit| start >= limit) {
                    socket.write_all(b"HTTP/1.1 403 Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                } else {
                    socket.write_all(format!("HTTP/1.1 206 Test\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nConnection: close\r\n\r\n",end-start+1,body.len()).as_bytes()).await.unwrap();
                    socket.write_all(&body[start..=end]).await.unwrap();
                }
            });
        }
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }
        peak.load(Ordering::SeqCst)
    });
    (format!("http://{address}/fixture"), task)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streaming_update_throughput_parallel_ranges_preserve_plain_and_ciphertext_tails() {
    for encrypted in [false, true] {
        for parallel in [1usize, 2, 8] {
            let root = tempfile::tempdir().unwrap();
            let length: usize = 4 * 1024 * 1024 + 123;
            let body = Arc::new(
                (0..length.div_ceil(PAGE_SIZE) * PAGE_SIZE)
                    .map(|i| (i % 251) as u8)
                    .collect::<Vec<_>>(),
            );
            let (url, task) = server(body.clone(), 5, parallel, None).await;
            let file = segment(length, encrypted);
            let target = root.path().join("staged");
            let mut out = tokio::fs::File::create(&target).await.unwrap();
            let (_control, pause) = watch::channel(false);
            let (tx, mut rx) = tokio::sync::mpsc::channel(16);
            let mut checkpoints = Vec::new();
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                transfer(
                    &xvd().await,
                    &reqwest::Client::new(),
                    &url,
                    &root.path().join("missing"),
                    None,
                    &file,
                    &mut out,
                    [0; 32],
                    0,
                    pause,
                    parallel,
                    &tx,
                    |p| {
                        checkpoints.push(p.position());
                        Ok(())
                    },
                ),
            )
            .await
            .unwrap()
            .unwrap();
            out.flush().await.unwrap();
            assert_eq!(task.await.unwrap(), parallel.min(5));
            let physical = if encrypted { body.len() } else { length };
            assert_eq!(fs::read(target).unwrap(), body[..physical]);
            assert!(
                checkpoints.is_empty(),
                "memory fetches must not commit filesystem checkpoints"
            );
            drop(tx);
            let mut network = 0;
            while let Some(event) = rx.recv().await {
                if let ProgressEvent::Traffic { network: n, reused } = event {
                    network += n;
                    assert_eq!(reused, 0);
                }
            }
            assert_eq!(network, length as u64);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streaming_update_throughput_failed_window_resumes_only_durable_contiguous_output() {
    use tokio::io::AsyncSeekExt;
    let root = tempfile::tempdir().unwrap();
    let body = Arc::new(
        (0..9 * 1024 * 1024)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<_>>(),
    );
    let file = segment(body.len(), false);
    let files = HashMap::from([("staged".to_owned(), segment(body.len(), false))]);
    let mut journal = resume::Journal::open(root.path(), "fixture", &files).unwrap();
    let target = root.path().join("staged");
    let mut out = tokio::fs::File::create(&target).await.unwrap();
    let (_control, pause) = watch::channel(false);
    let (tx, _rx) = tokio::sync::mpsc::channel(32);
    let client = reqwest::Client::new();
    let xvd = xvd().await;
    let (url, task) = server(body.clone(), 9, 4, Some(8 * 1024 * 1024)).await;
    let failed = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        transfer(
            &xvd,
            &client,
            &url,
            &root.path().join("missing"),
            None,
            &file,
            &mut out,
            [0; 32],
            0,
            pause.clone(),
            4,
            &tx,
            |p| journal.checkpoint_synced("staged", p),
        ),
    )
    .await
    .unwrap();
    assert!(failed.is_err());
    task.await.unwrap();
    assert_eq!(fs::read(&target).unwrap(), body[..8 * 1024 * 1024]);
    drop(journal);
    let mut journal = resume::Journal::open(root.path(), "fixture", &files).unwrap();
    let offset = journal.offset("staged", file.length);
    assert_eq!(offset, 8 * 1024 * 1024);
    out.seek(SeekFrom::Start(offset)).await.unwrap();
    let (url, task) = server(body.clone(), 1, 1, None).await;
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        transfer(
            &xvd,
            &client,
            &url,
            &root.path().join("missing"),
            None,
            &file,
            &mut out,
            [0; 32],
            offset,
            pause,
            4,
            &tx,
            |p| journal.checkpoint_synced("staged", p),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    out.flush().await.unwrap();
    journal.checkpoint("staged", file.length).unwrap();
    task.await.unwrap();
    assert_eq!(fs::read(target).unwrap(), *body);
    assert_eq!(journal.offset("staged", file.length), file.length);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streaming_update_throughput_pause_and_cancellation_never_claim_prefetched_bytes() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("staged");
    let mut out = tokio::fs::File::create(&target).await.unwrap();
    let file = segment(1024 * 1024, false);
    let xvd = xvd().await;
    let client = reqwest::Client::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (control, pause) = watch::channel(true);
    let (stop, stopped) = watch::channel(false);
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let mut checkpoints = Vec::new();
    let driver = async {
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
                .await
                .is_err(),
            "paused update must not open a range request"
        );
        control.send(false).unwrap();
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        socket.write_all(b"HTTP/1.1 206 Test\r\nContent-Length: 1048576\r\nContent-Range: bytes 0-1048575/1048576\r\nConnection: close\r\n\r\n").await.unwrap();
        socket.write_all(&vec![7; 8192]).await.unwrap();
        stop.send(true).unwrap();
        let mut ignored = Vec::new();
        let closed = socket.read_to_end(&mut ignored).await;
        assert!(
            closed.is_ok() || closed.is_err_and(|e| e.kind() == io::ErrorKind::ConnectionReset)
        );
    };
    let url = format!("http://{address}/fixture");
    let missing = root.path().join("missing");
    let operation = failure::run_or_cancel(
        stopped,
        transfer(
            &xvd,
            &client,
            &url,
            &missing,
            None,
            &file,
            &mut out,
            [0; 32],
            0,
            pause,
            4,
            &tx,
            |p| {
                checkpoints.push(p.position());
                Ok(())
            },
        ),
    );
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(operation, driver)
    })
    .await
    .unwrap();
    assert!(result.is_err());
    let mut complete = false;
    assert!(
        failure::settle(&mut out, result, || {
            complete = true;
            Ok(())
        })
        .await
        .is_err()
    );
    assert!(!complete);
    assert!(checkpoints.iter().all(|p| *p == 0));
    assert!(fs::read(target).unwrap().is_empty());
}
