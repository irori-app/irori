//! The download engine: builds files from chunks with bounded parallelism,
//! resumes interrupted work and verifies everything it writes.

mod assemble;
mod stream;
mod support;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use futures::future::BoxFuture;
use futures::{StreamExt as _, TryStreamExt as _};
use tokio_util::sync::CancellationToken;

use self::support::{
    ChunkCache, Journal, RateLimiter, Signing, Waiting, check_free_space, disk_sizes, is_disk_full,
};
use crate::adapt::{self, Gate, Hosts, Signals, Slump, Tuner, Window, classify};
use crate::path;
use crate::plan::FileEntry;
use crate::progress::{INTERVAL, Meter, ProgressFn, Stats};

/// Supplies short-lived download addresses for chunks (stores whose CDN
/// needs per-chunk signatures). Called with up to [`SIGN_BATCH`] chunk keys
/// in download order; returns key → URL.
pub trait UrlSigner: Send + Sync {
    fn sign<'a>(&'a self, keys: &'a [String]) -> BoxFuture<'a, Result<HashMap<String, String>>>;
}

/// Chunk keys signed per [`UrlSigner::sign`] call.
pub const SIGN_BATCH: usize = 50;

/// A shared [`UrlSigner`].
#[derive(Clone)]
pub struct Signer(pub Arc<dyn UrlSigner>);

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Signer")
    }
}

/// Tuning of a download.
#[derive(Debug, Clone)]
pub struct Options {
    /// Most HTTP requests at once (0: up to 32). The engine measures how
    /// many make the download fastest and stays below this.
    pub workers: usize,
    /// Bandwidth cap in bytes per second (0: unlimited).
    pub max_bytes_per_sec: u64,
    /// Records finished files so a resumed download skips them without
    /// hashing. Deleted when the download completes.
    pub journal: Option<PathBuf>,
    /// Memory for chunks shared by several files (0: 256 MiB).
    pub cache_bytes: u64,
    /// Attempts after the first when a server sends damaged data or
    /// refuses a signed address. Dropped connections, stalls and busy
    /// servers are retried until the download is cancelled.
    pub retries: u32,
    /// Signs chunk addresses instead of using [`Chunk::urls`].
    pub signer: Option<Signer>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            workers: 0,
            max_bytes_per_sec: 0,
            journal: None,
            cache_bytes: 0,
            retries: 5,
            signer: None,
        }
    }
}

/// What to put where.
#[derive(Debug, Clone, Copy)]
pub struct Job<'a> {
    /// Install directory.
    pub root: &'a Path,
    /// Files to write (files already intact on disk are skipped).
    pub files: &'a [FileEntry],
    /// Paths deleted once every file is in place (files an update dropped).
    pub remove: &'a [String],
}

/// How a download ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finished {
    Done,
    Cancelled,
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("cancelled")]
    Cancelled,
    #[error("the server answered {status} for {url}")]
    Status {
        status: reqwest::StatusCode,
        url: String,
    },
    #[error("network error: {0}")]
    Network(reqwest::Error),
    /// The connection stalled or closed early.
    #[error("{0}")]
    Interrupted(String),
    /// Stopped to continue on a new connection ([`Ctx::replace_connections`]).
    #[error("continuing on a new connection")]
    Recycled,
    #[error("{0}")]
    Corrupt(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<reqwest::Error> for Error {
    /// Without the address: signed CDN addresses are credentials.
    fn from(e: reqwest::Error) -> Self {
        Self::Network(e.without_url())
    }
}

/// How a failed request is retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The connection or a busy server: retried until cancelled.
    Transient,
    /// Damaged data: retried [`Options::retries`] times.
    Corrupt,
    /// Retrying cannot help.
    Fatal,
    /// Stopped on purpose: continued right away, not the server's fault.
    Recycled,
}

impl Error {
    fn kind(&self) -> Kind {
        match self {
            Error::Status { status, .. }
                if status.is_server_error()
                    || *status == reqwest::StatusCode::REQUEST_TIMEOUT
                    || *status == reqwest::StatusCode::TOO_MANY_REQUESTS =>
            {
                Kind::Transient
            }
            Error::Network(e) if e.is_builder() || e.is_redirect() => Kind::Fatal,
            Error::Network(_) | Error::Interrupted(_) => Kind::Transient,
            Error::Corrupt(_) => Kind::Corrupt,
            Error::Recycled => Kind::Recycled,
            Error::Status { .. } | Error::Cancelled | Error::Io(_) | Error::Other(_) => Kind::Fatal,
        }
    }
}

/// A connection that delivers nothing (or a trickle) for this long is
/// dropped for another server.
pub(crate) const STALL: Duration = if cfg!(test) {
    Duration::from_millis(400)
} else {
    Duration::from_secs(8)
};
/// How long one batch of chunk addresses may take to sign before it is
/// retried like a dropped connection.
const SIGN_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_millis(400)
} else {
    Duration::from_secs(30)
};
/// The first wait after a failure; it doubles per failure up to a minute.
const BACKOFF: Duration = if cfg!(test) {
    Duration::from_millis(10)
} else {
    Duration::from_millis(500)
};
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// How often the number of requests at once is tuned.
const TUNE_EVERY: Duration = if cfg!(test) {
    Duration::from_millis(100)
} else {
    Duration::from_secs(2)
};
/// Files hashed at once while checking what is already on disk.
const CHECKERS: usize = 4;

/// Download `job`. Files already intact are skipped, partial whole-file
/// downloads resume, and every file is checked against its digest before it
/// replaces the installed one.
pub async fn download(
    client: &reqwest::Client,
    job: Job<'_>,
    opts: &Options,
    progress: ProgressFn,
    cancel: &CancellationToken,
) -> Result<Finished> {
    for f in job.files {
        if path::normalize(&f.path)? != f.path {
            bail!("file path {:?} is not normalised", f.path);
        }
    }
    tokio::fs::create_dir_all(job.root)
        .await
        .with_context(|| format!("Could not create {}", job.root.display()))?;
    let on_disk = disk_sizes(job.root, job.files).await?;
    check_free_space(job.root, job.files, &on_disk)?;

    let total = job.files.iter().map(|f| f.size).sum();
    let stats = Arc::new(Stats::new(total));
    let max = if opts.workers == 0 {
        adapt::DEFAULT_MAX
    } else {
        opts.workers
    };
    let tuner = Tuner::new(max);
    let cancel = cancel.child_token();
    let ctx = Arc::new(Ctx {
        client: client.clone(),
        root: job.root.to_path_buf(),
        requests: Gate::new(tuner.limit()),
        hosts: Hosts::default(),
        limiter: (opts.max_bytes_per_sec > 0).then(|| RateLimiter::new(opts.max_bytes_per_sec)),
        cache: ChunkCache::new(
            job.files,
            if opts.cache_bytes == 0 {
                256 << 20
            } else {
                opts.cache_bytes
            },
        ),
        stats: stats.clone(),
        cancel: cancel.clone(),
        journal: match &opts.journal {
            Some(p) => Some(Journal::open(p).await?),
            None => None,
        },
        retries: opts.retries,
        signing: opts
            .signer
            .as_ref()
            .map(|s| Signing::new(s.0.clone(), job.files)),
        prefetch: (max / 4).clamp(4, 8),
        recycle: Recycle::default(),
    });

    // Owned items keep the future `Send` (borrowing closures trip up
    // higher-ranked lifetime inference).
    let files: Arc<[FileEntry]> = job.files.into();
    let work = {
        let ctx = ctx.clone();
        async move {
            let pending = ctx.check_existing(&files, &on_disk).await?;
            futures::stream::iter(pending)
                .map(move |i| {
                    let (ctx, files) = (ctx.clone(), files.clone());
                    async move { ctx.process_file(&files[i]).await }
                })
                .buffer_unordered(max)
                .try_for_each(|()| async { Ok(()) })
                .await
        }
    };
    let limit = (opts.max_bytes_per_sec > 0).then_some(opts.max_bytes_per_sec);
    let result = with_progress(stats, progress, async {
        tokio::select! {
            biased;
            r = work => r,
            // Dropping the work stops every request at once, including ones
            // waiting for a free slot or for addresses to be signed.
            () = cancel.cancelled() => Err(Error::Cancelled),
            () = tune(&ctx, tuner, limit) => unreachable!("tuning never ends"),
        }
    })
    .await;
    match result {
        Ok(()) => {}
        Err(_) if cancel.is_cancelled() => return Ok(Finished::Cancelled),
        Err(e) => {
            cancel.cancel();
            if is_disk_full(&e) {
                bail!(
                    "Not enough free disk space in {}: free up space and retry",
                    job.root.display()
                );
            }
            return Err(anyhow::Error::from(e).context("Download failed"));
        }
    }

    for rel in job.remove {
        let rel = path::normalize(rel)?;
        let p = path::join(job.root, &rel);
        match tokio::fs::remove_file(&p).await {
            Ok(()) => path::prune_empty_parents(job.root, &rel),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("removing {}: {e}", p.display()),
        }
    }
    if let Some(journal) = &opts.journal
        && let Err(e) = tokio::fs::remove_file(journal).await
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("removing {}: {e}", journal.display());
    }
    Ok(Finished::Done)
}

/// Run `fut`, reporting progress on a timer and once at the end.
pub(crate) async fn with_progress<F: std::future::Future>(
    stats: Arc<Stats>,
    sink: ProgressFn,
    fut: F,
) -> F::Output {
    let mut meter = Meter::new(stats);
    let mut tick = tokio::time::interval(INTERVAL);
    tokio::pin!(fut);
    loop {
        tokio::select! {
            out = &mut fut => {
                sink(meter.sample());
                return out;
            }
            _ = tick.tick() => sink(meter.sample()),
        }
    }
}

/// Size the request gate to what the connection takes, and tell what
/// holds the download back. Runs until dropped.
async fn tune(ctx: &Ctx, mut tuner: Tuner, limit: Option<u64>) {
    use std::sync::atomic::Ordering::Relaxed;
    let stats = &ctx.stats;
    let snapshot = || {
        (
            stats.received.load(Relaxed),
            stats.failures.load(Relaxed),
            stats.net_wait_us.load(Relaxed),
            stats.disk_wait_us.load(Relaxed),
        )
    };
    let mut last = (Instant::now(), snapshot());
    // A verdict shows once two windows in a row agree.
    let mut candidate = None;
    let mut slump = Slump::default();
    loop {
        tokio::time::sleep(TUNE_EVERY).await;
        let now = (Instant::now(), snapshot());
        let secs = now.0.duration_since(last.0).as_secs_f64().max(0.001);
        let (received, failures, net_us, disk_us) = (
            now.1.0 - last.1.0,
            now.1.1 - last.1.1,
            now.1.2 - last.1.2,
            now.1.3 - last.1.3,
        );
        last = now;
        let rate = received as f64 / secs;
        let contended = ctx.requests.take_contended();
        let throttled =
            stats.throttling.load(Relaxed) > 0 || limit.is_some_and(|l| rate >= l as f64 * 0.85);
        let n = tuner.step(Window {
            rate,
            failures,
            contended,
            throttled,
        });
        ctx.requests.set_limit(n);
        let verdict = if throttled {
            Some(irori_core::Bottleneck::SpeedLimit)
        } else {
            classify(Signals {
                rate,
                failures,
                backing_off: stats.backing_off.load(Relaxed),
                limit,
                net_wait: Duration::from_micros(net_us),
                disk_wait: Duration::from_micros(disk_us),
                contended,
                capped: tuner.capped(),
            })
        };
        let steady = !throttled && verdict != Some(irori_core::Bottleneck::Disk);
        if slump.observe(now.0, rate, ctx.requests.active(), steady) {
            tracing::info!(
                "the download slowed down to {:.0} KiB/s; continuing on new connections",
                rate / 1024.0
            );
            ctx.replace_connections(n);
        }
        if verdict == candidate {
            stats.set_bottleneck(verdict);
        }
        candidate = verdict;
    }
}

struct Ctx {
    client: reqwest::Client,
    root: PathBuf,
    /// Requests at once, sized by [`tune`].
    requests: Gate,
    hosts: Hosts,
    limiter: Option<RateLimiter>,
    cache: ChunkCache,
    stats: Arc<Stats>,
    cancel: CancellationToken,
    journal: Option<Journal>,
    retries: u32,
    signing: Option<Signing>,
    /// Chunks fetched ahead of the one being written, per file.
    prefetch: usize,
    recycle: Recycle,
}

/// Connections being replaced after the download slumped ([`Slump`]).
#[derive(Default)]
struct Recycle {
    /// Bumped per replacement: a stream begun before it stops and resumes.
    generation: std::sync::atomic::AtomicU64,
    /// Requests still to ask the server to close their connection after
    /// answering, so the pool opens new ones.
    to_close: std::sync::atomic::AtomicU64,
}

impl Ctx {
    /// Move the download to new connections: streams stop and resume on
    /// one, the next `connections` requests close theirs after answering
    /// (HTTP/2 keeps its one connection: the header doesn't apply there),
    /// and every server is measured again. Requests in flight finish.
    fn replace_connections(&self, connections: usize) {
        use std::sync::atomic::Ordering::Relaxed;
        self.recycle.to_close.store(connections as u64, Relaxed);
        self.recycle.generation.fetch_add(1, Relaxed);
        self.hosts.forget_rates();
    }

    fn generation(&self) -> u64 {
        self.recycle
            .generation
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn check_cancel(&self) -> Result<(), Error> {
        if self.cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Wait out failure number `failures` (1-based) of a request.
    async fn backoff(&self, failures: u32) -> Result<(), Error> {
        let delay = BACKOFF
            .saturating_mul(1 << failures.min(8))
            .min(MAX_BACKOFF);
        let _waiting = Waiting::new(&self.stats.backing_off);
        tokio::select! {
            _ = self.cancel.cancelled() => Err(Error::Cancelled),
            _ = tokio::time::sleep(delay) => Ok(()),
        }
    }

    /// Wait for the speed limit to allow `n` more bytes; returns the wait.
    async fn throttle(&self, n: usize) -> Result<Duration, Error> {
        let Some(limiter) = &self.limiter else {
            return Ok(Duration::ZERO);
        };
        let wait = limiter.reserve(n as u64);
        if wait.is_zero() {
            return Ok(wait);
        }
        let _waiting = Waiting::new(&self.stats.throttling);
        tokio::select! {
            _ = self.cancel.cancelled() => Err(Error::Cancelled),
            _ = tokio::time::sleep(wait) => Ok(wait),
        }
    }

    /// Note how a request to `url` that ran for `elapsed` ended.
    fn score<T>(&self, url: &str, result: &Result<T, Error>, bytes: u64, elapsed: Duration) {
        match result {
            Ok(_) => self.hosts.record_ok(url, bytes, elapsed),
            Err(e) if e.kind() == Kind::Transient => {
                self.hosts.record_failure(url);
                self.stats.add_failure();
            }
            Err(e) if e.kind() == Kind::Corrupt => self.hosts.record_failure(url),
            Err(_) => {}
        }
    }
}
