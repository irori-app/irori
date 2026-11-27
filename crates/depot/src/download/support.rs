//! Supporting utilities for the download engine: disk space checks, chunk
//! signing, chunk cache, rate limiting, resume journal, and codec helpers.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use super::{Error, SIGN_BATCH, SIGN_TIMEOUT, STALL, UrlSigner};
use crate::adapt;
use crate::hash::Digest;
use crate::path;
use crate::plan::{Codec, FileEntry};

/// Tail window hashed for [`PartCheckpoint::tail_sha256`] validation on resume.
pub(super) const TAIL_HASH_WINDOW: u64 = 64 * 1024;

/// Stable digest identifying a file's size, target hash, and chunk/part layout.
pub(super) fn parts_fingerprint(f: &FileEntry) -> String {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(f.size.to_le_bytes());
    if let Some(hash) = &f.hash {
        h.update([1u8]);
        h.update(hash.to_string().as_bytes());
    } else {
        h.update([0u8]);
    }
    h.update((f.parts.len() as u64).to_le_bytes());
    for p in &f.parts {
        h.update((p.chunk.key.len() as u64).to_le_bytes());
        h.update(p.chunk.key.as_bytes());
        h.update(p.offset.to_le_bytes());
        h.update(p.size.to_le_bytes());
        h.update(p.chunk.size.to_le_bytes());
        h.update(p.chunk.download_size.to_le_bytes());
        match p.chunk.range {
            Some((start, len)) => {
                h.update([1u8]);
                h.update(start.to_le_bytes());
                h.update(len.to_le_bytes());
            }
            None => h.update([0u8]),
        }
    }
    hex::encode(h.finalize())
}

/// SHA-256 hex digest of `[bytes_done - 64 KiB .. bytes_done]` in `path`.
pub(super) fn hash_file_tail(path: &Path, bytes_done: u64) -> std::io::Result<String> {
    use sha2::Digest as _;
    use std::io::{Read as _, Seek as _};
    let start = bytes_done.saturating_sub(TAIL_HASH_WINDOW);
    let len = usize::try_from(bytes_done - start).map_err(std::io::Error::other)?;
    let mut file = std::fs::File::open(path)?;
    file.seek(std::io::SeekFrom::Start(start))?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)?;
    Ok(hex::encode(sha2::Sha256::digest(&buf)))
}

/// Sizes of the files already below `root`, alongside valid partial-file byte
/// credits for free-space checks.
pub(super) async fn disk_sizes(
    root: &Path,
    files: &[FileEntry],
    journal: Option<&Journal>,
) -> Result<(Vec<Option<u64>>, Vec<u64>)> {
    let root = root.to_path_buf();
    let specs: Vec<(String, u64, bool, Option<u64>)> = files
        .iter()
        .map(|f| {
            let cp_bytes = if f.is_stream() {
                None
            } else {
                journal.and_then(|j| {
                    let cp = j.checkpoint(&f.path)?;
                    if cp.size == f.size
                        && cp.bytes_done <= f.size
                        && cp.parts_done <= f.parts.len()
                        && cp.parts_fingerprint == parts_fingerprint(f)
                    {
                        Some(cp.bytes_done)
                    } else {
                        None
                    }
                })
            };
            (f.path.clone(), f.size, f.is_stream(), cp_bytes)
        })
        .collect();
    Ok(tokio::task::spawn_blocking(move || {
        let mut on_disk = Vec::with_capacity(specs.len());
        let mut partial_credit = Vec::with_capacity(specs.len());
        for (rel, size, is_stream, cp_bytes) in specs {
            let dest = path::join(&root, &rel);
            let existing = std::fs::metadata(&dest)
                .ok()
                .filter(|m| m.is_file())
                .map(|m| m.len());
            let credit = if existing == Some(size) {
                0
            } else {
                let part = path::part_path(&dest);
                let part_len = std::fs::metadata(&part)
                    .ok()
                    .filter(|m| m.is_file())
                    .map(|m| m.len())
                    .unwrap_or(0);
                if is_stream {
                    part_len.min(size)
                } else {
                    match cp_bytes {
                        Some(cp) if part_len >= cp => cp,
                        _ => 0,
                    }
                }
            };
            on_disk.push(existing);
            partial_credit.push(credit);
        }
        (on_disk, partial_credit)
    })
    .await?)
}

pub(super) fn check_free_space(
    root: &Path,
    files: &[FileEntry],
    on_disk: &[Option<u64>],
    partial_credit: &[u64],
) -> Result<()> {
    let needed: u64 = files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let have = on_disk[i].unwrap_or(0).max(partial_credit[i]);
            f.size.saturating_sub(have)
        })
        .sum();
    match fs4::available_space(root) {
        Ok(free) if free < needed => bail!(
            "Not enough free disk space in {}: {} MiB needed, {} MiB available",
            root.display(),
            needed.div_ceil(1 << 20),
            free >> 20
        ),
        Ok(_) => Ok(()),
        Err(e) => {
            tracing::warn!("free space of {}: {e}", root.display());
            Ok(())
        }
    }
}

/// Whether `err` was caused by a full filesystem (Windows `ERROR_DISK_FULL`
/// or Unix `ENOSPC`).
pub(super) fn is_disk_full(err: &Error) -> bool {
    match err {
        Error::Io(io) => io.kind() == std::io::ErrorKind::StorageFull,
        Error::Other(other) => other.chain().any(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::StorageFull)
        }),
        _ => false,
    }
}

/// The error for a server that stopped sending.
pub(super) fn stalled(url: &str) -> Error {
    Error::Interrupted(format!(
        "{} sent nothing for {} seconds",
        adapt::host(url),
        STALL.as_secs_f32()
    ))
}

/// Counts a task in a gauge while it lives.
pub(super) struct Waiting<'a>(&'a std::sync::atomic::AtomicU64);

impl<'a> Waiting<'a> {
    pub(super) fn new(gauge: &'a std::sync::atomic::AtomicU64) -> Self {
        gauge.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self(gauge)
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

pub(super) fn decode(codec: &Codec, raw: Vec<u8>) -> Result<Vec<u8>> {
    match codec {
        Codec::Raw => Ok(raw),
        Codec::Zlib => {
            use std::io::Read as _;
            let mut out = Vec::new();
            flate2::read::ZlibDecoder::new(raw.as_slice()).read_to_end(&mut out)?;
            Ok(out)
        }
        Codec::Deflate => {
            use std::io::Read as _;
            let mut out = Vec::new();
            flate2::read::DeflateDecoder::new(raw.as_slice()).read_to_end(&mut out)?;
            Ok(out)
        }
        Codec::Custom(d) => d.decode(raw),
    }
}

/// Inflate a raw-deflate file.
pub(super) fn inflate_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    let input = std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(src)?);
    let mut decoder = flate2::read::DeflateDecoder::new(input);
    let mut out = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(dst)?);
    std::io::copy(&mut decoder, &mut out)?;
    std::io::Write::flush(&mut out)
}

/// `size` bytes of `path` from `offset`.
pub(super) fn read_at(path: &Path, offset: u64, size: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read as _, Seek as _};
    let mut file = std::fs::File::open(path)?;
    file.seek(std::io::SeekFrom::Start(offset))?;
    let mut buf = vec![0; usize::try_from(size).map_err(std::io::Error::other)?];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

/// Signed chunk addresses, fetched in batches in download order.
pub(super) struct Signing {
    signer: Arc<dyn UrlSigner>,
    /// Chunk keys in the order files need them.
    order: Vec<String>,
    index: HashMap<String, usize>,
    urls: std::sync::Mutex<HashMap<String, String>>,
    gate: tokio::sync::Mutex<()>,
}

impl Signing {
    pub(super) fn new(signer: Arc<dyn UrlSigner>, files: &[FileEntry]) -> Self {
        let mut order = Vec::new();
        let mut index = HashMap::new();
        for p in files.iter().flat_map(|f| &f.parts) {
            if !index.contains_key(&p.chunk.key) {
                index.insert(p.chunk.key.clone(), order.len());
                order.push(p.chunk.key.clone());
            }
        }
        Self {
            signer,
            order,
            index,
            urls: Default::default(),
            gate: Default::default(),
        }
    }

    fn cached(&self, key: &str) -> Option<String> {
        self.urls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned()
    }

    pub(super) fn forget(&self, key: &str) {
        self.urls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }

    /// The signed address of `key`, signing it together with the next
    /// unsigned keys. Cancels promptly when `cancel` is triggered.
    pub(super) async fn url(&self, key: &str, cancel: &CancellationToken) -> Result<String, Error> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if let Some(url) = self.cached(key) {
            return Ok(url);
        }
        let _gate = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(Error::Cancelled),
            g = self.gate.lock() => g,
        };
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if let Some(url) = self.cached(key) {
            return Ok(url);
        }
        let start = self.index.get(key).copied();
        let mut batch = vec![key.to_string()];
        if let Some(start) = start {
            let urls = self.urls.lock().unwrap_or_else(|e| e.into_inner());
            batch.extend(
                self.order[start + 1..]
                    .iter()
                    .filter(|k| !urls.contains_key(*k))
                    .take(SIGN_BATCH - 1)
                    .cloned(),
            );
        }
        let signed = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(Error::Cancelled),
            res = tokio::time::timeout(SIGN_TIMEOUT, self.signer.sign(&batch)) => res
                .map_err(|_| Error::Interrupted("signing chunk addresses timed out".into()))?
                .map_err(sign_error)?,
        };
        let mut urls = self.urls.lock().unwrap_or_else(|e| e.into_inner());
        urls.extend(signed);
        urls.get(key).cloned().ok_or_else(|| {
            Error::Other(anyhow::anyhow!(
                "no download address was issued for chunk {key}"
            ))
        })
    }
}

/// A failure to sign addresses: retried like a dropped connection when
/// the network was at fault, else final.
fn sign_error(e: anyhow::Error) -> Error {
    let network = e.chain().any(|c| {
        c.downcast_ref::<reqwest::Error>()
            .is_some_and(|r| !r.is_builder() && r.status().is_none_or(|s| s.is_server_error()))
    });
    let e = e.context("Could not get chunk download addresses");
    if network {
        Error::Interrupted(format!("{e:#}"))
    } else {
        Error::Other(e)
    }
}

/// Signed CDN addresses carry credentials, in the query or the path (GOG's
/// `/token=…~hmac=…`); only the server's name goes in logs.
pub(super) fn redact(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("https", url));
    let host = rest.split(['/', '?']).next().unwrap_or(rest);
    format!("{scheme}://{host}")
}

pub(super) async fn file_len(p: &Path) -> Option<u64> {
    tokio::fs::metadata(p)
        .await
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len())
}

#[cfg(unix)]
pub(super) async fn set_executable(p: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    if let Ok(meta) = tokio::fs::metadata(p).await {
        let mut perm = meta.permissions();
        perm.set_mode(perm.mode() | 0o755);
        if let Err(e) = tokio::fs::set_permissions(p, perm).await {
            tracing::warn!("making {} executable: {e}", p.display());
        }
    }
}

#[cfg(not(unix))]
pub(super) async fn set_executable(_: &Path) {}

/// Token bucket over virtual time: each reservation pushes the next free
/// moment forward by its transfer time.
pub(super) struct RateLimiter {
    bytes_per_sec: f64,
    next: std::sync::Mutex<Instant>,
}

impl RateLimiter {
    pub(super) fn new(bytes_per_sec: u64) -> Self {
        Self {
            bytes_per_sec: bytes_per_sec as f64,
            next: std::sync::Mutex::new(Instant::now()),
        }
    }

    /// How long to wait before sending `n` bytes. Unused capacity carries
    /// over for up to one second.
    pub(super) fn reserve(&self, n: u64) -> Duration {
        let mut next = self.next.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let floor = now.checked_sub(Duration::from_secs(1)).unwrap_or(now);
        let start = (*next).max(floor);
        *next = start + Duration::from_secs_f64(n as f64 / self.bytes_per_sec);
        next.saturating_duration_since(now)
    }
}

/// Decoded chunks kept while files that still need them are pending.
pub(super) struct ChunkCache {
    slots: std::sync::Mutex<HashMap<String, Slot>>,
    budget: u64,
    bytes: std::sync::atomic::AtomicU64,
}

struct Slot {
    remaining: usize,
    size: u64,
    cell: Arc<OnceCell<Arc<Vec<u8>>>>,
}

impl ChunkCache {
    pub(super) fn new(files: &[FileEntry], budget: u64) -> Self {
        let mut slots: HashMap<String, Slot> = HashMap::new();
        for p in files.iter().flat_map(|f| &f.parts) {
            slots
                .entry(p.chunk.key.clone())
                .or_insert_with(|| Slot {
                    remaining: 0,
                    size: p.chunk.size,
                    cell: Arc::default(),
                })
                .remaining += 1;
        }
        Self {
            slots: std::sync::Mutex::new(slots),
            budget,
            bytes: Default::default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Slot>> {
        self.slots.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn cell(&self, key: &str) -> Arc<OnceCell<Arc<Vec<u8>>>> {
        self.lock()
            .entry(key.to_string())
            .or_insert_with(|| Slot {
                remaining: 1,
                size: 0,
                cell: Arc::default(),
            })
            .cell
            .clone()
    }

    pub(super) fn add_bytes(&self, n: u64) {
        self.bytes
            .fetch_add(n, std::sync::atomic::Ordering::Relaxed);
    }

    fn drop_bytes(&self, slot: &Slot) {
        if slot.cell.initialized() {
            let n = slot.cell.get().map_or(slot.size, |d| d.len() as u64);
            let _ = self.bytes.try_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |b| Some(b.saturating_sub(n)),
            );
        }
    }

    /// One use of the chunk is done: forget it when nothing else needs it,
    /// or when the cache is over budget (it is downloaded again if needed).
    pub(super) fn release(&self, key: &str) {
        let mut slots = self.lock();
        let Some(slot) = slots.get_mut(key) else {
            return;
        };
        slot.remaining = slot.remaining.saturating_sub(1);
        if slot.remaining == 0 {
            if let Some(slot) = slots.remove(key) {
                self.drop_bytes(&slot);
            }
        } else if self.bytes.load(std::sync::atomic::Ordering::Relaxed) > self.budget
            && slot.cell.initialized()
        {
            let old = std::mem::take(&mut slot.cell);
            let size = slot.size;
            self.drop_bytes(&Slot {
                remaining: 0,
                size,
                cell: old,
            });
        }
    }
}

/// Persisted progress of an in-flight multi-chunk `.irori-part` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PartCheckpoint {
    pub(super) path: String,
    pub(super) size: u64,
    pub(super) parts_fingerprint: String,
    pub(super) parts_done: usize,
    pub(super) bytes_done: u64,
    pub(super) tail_sha256: String,
}

/// Finished files and partial multi-chunk checkpoints of an interrupted download.
pub(super) struct Journal {
    done: HashMap<String, (u64, Option<Digest>)>,
    checkpoints: std::sync::Mutex<HashMap<String, PartCheckpoint>>,
    file: tokio::sync::Mutex<Option<tokio::fs::File>>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum JournalRecord {
    Done {
        path: String,
        size: u64,
        hash: Option<Digest>,
    },
    Checkpoint(PartCheckpoint),
    ClearCheckpoint {
        path: String,
    },
}

/// Legacy journal line written by older versions (no `kind` tag).
#[derive(Deserialize)]
struct LegacyJournalEntry {
    path: String,
    size: u64,
    hash: Option<Digest>,
}

impl Journal {
    pub(super) async fn open(path: &Path) -> Result<Self> {
        let mut done = HashMap::new();
        let mut checkpoints = HashMap::new();
        let mut raw_lines = 0usize;
        let existed = match tokio::fs::read_to_string(path).await {
            Ok(text) => {
                for line in text.lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    raw_lines += 1;
                    if let Ok(record) = serde_json::from_str::<JournalRecord>(trimmed) {
                        match record {
                            JournalRecord::Done { path, size, hash } => {
                                checkpoints.remove(&path);
                                done.insert(path, (size, hash));
                            }
                            JournalRecord::Checkpoint(cp) => {
                                checkpoints.insert(cp.path.clone(), cp);
                            }
                            JournalRecord::ClearCheckpoint { path } => {
                                checkpoints.remove(&path);
                            }
                        }
                    } else if let Ok(legacy) = serde_json::from_str::<LegacyJournalEntry>(trimmed) {
                        checkpoints.remove(&legacy.path);
                        done.insert(legacy.path, (legacy.size, legacy.hash));
                    }
                }
                true
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        // Compact superseded checkpoints, clears, and truncated trailing lines.
        if existed && raw_lines > done.len() + checkpoints.len() {
            compact_journal(path, &done, &checkpoints).await;
        }
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .with_context(|| format!("opening {}", path.display()))?;
        Ok(Self {
            done,
            checkpoints: std::sync::Mutex::new(checkpoints),
            file: tokio::sync::Mutex::new(Some(file)),
        })
    }

    pub(super) fn has(&self, f: &FileEntry) -> bool {
        self.done
            .get(&f.path)
            .is_some_and(|(size, hash)| *size == f.size && *hash == f.hash)
    }

    pub(super) fn checkpoint(&self, path: &str) -> Option<PartCheckpoint> {
        self.checkpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(path)
            .cloned()
    }

    pub(super) async fn save_checkpoint(&self, cp: PartCheckpoint) {
        self.checkpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(cp.path.clone(), cp.clone());
        self.append(&JournalRecord::Checkpoint(cp), true).await;
    }

    pub(super) async fn clear_checkpoint(&self, path: &str) {
        let removed = self
            .checkpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(path)
            .is_some();
        if removed {
            self.append(
                &JournalRecord::ClearCheckpoint {
                    path: path.to_string(),
                },
                true,
            )
            .await;
        }
    }

    pub(super) async fn record(&self, f: &FileEntry) {
        self.checkpoints
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&f.path);
        let entry = JournalRecord::Done {
            path: f.path.clone(),
            size: f.size,
            hash: f.hash.clone(),
        };
        self.append(&entry, false).await;
    }

    async fn append(&self, record: &JournalRecord, sync: bool) {
        let Ok(mut line) = serde_json::to_vec(record) else {
            return;
        };
        line.push(b'\n');
        let mut guard = self.file.lock().await;
        if let Some(file) = guard.as_mut() {
            let res = async {
                file.write_all(&line).await?;
                file.flush().await?;
                if sync {
                    file.sync_data().await?;
                }
                Ok::<_, std::io::Error>(())
            }
            .await;
            if let Err(e) = res {
                tracing::warn!("download journal: {e}");
                *guard = None;
            }
        }
    }
}

async fn compact_journal(
    path: &Path,
    done: &HashMap<String, (u64, Option<Digest>)>,
    checkpoints: &HashMap<String, PartCheckpoint>,
) {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".compact");
    let tmp = path.with_file_name(name);
    let mut buf = Vec::new();
    for (p, (size, hash)) in done {
        if let Ok(line) = serde_json::to_vec(&JournalRecord::Done {
            path: p.clone(),
            size: *size,
            hash: hash.clone(),
        }) {
            buf.extend_from_slice(&line);
            buf.push(b'\n');
        }
    }
    for cp in checkpoints.values() {
        if let Ok(line) = serde_json::to_vec(&JournalRecord::Checkpoint(cp.clone())) {
            buf.extend_from_slice(&line);
            buf.push(b'\n');
        }
    }
    let write_res = async {
        let mut f = tokio::fs::File::create(&tmp).await?;
        f.write_all(&buf).await?;
        f.flush().await?;
        f.sync_data().await?;
        drop(f);
        tokio::fs::rename(&tmp, path).await?;
        Ok::<_, std::io::Error>(())
    }
    .await;
    if let Err(e) = write_res {
        tracing::warn!("compacting download journal {}: {e}", path.display());
        let _ = tokio::fs::remove_file(&tmp).await;
    }
}

#[cfg(test)]
mod redact_tests {
    use super::{Error, is_disk_full, redact};

    #[test]
    fn keeps_only_the_server() {
        for url in [
            "https://cdn.example.org/path/file?sig=secret",
            "https://cdn.example.org/token=nva=1~hmac=secret/file",
            "https://cdn.example.org",
        ] {
            assert_eq!(redact(url), "https://cdn.example.org");
        }
    }

    #[test]
    fn detects_storage_full_errors() {
        let direct = Error::Io(std::io::Error::from(std::io::ErrorKind::StorageFull));
        assert!(is_disk_full(&direct));
        let wrapped = Error::Other(
            anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::StorageFull))
                .context("Data/game.pak"),
        );
        assert!(is_disk_full(&wrapped));
        let other = Error::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert!(!is_disk_full(&other));
    }
}
