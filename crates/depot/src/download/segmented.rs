//! Sidecar state, probing, parallel range workers, and 100%-safe positional
//! write helpers for multi-range segmented streams (`<dest>.irori-part.seg`).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::support::{file_len, redact, stalled};
use super::{Ctx, Error, Kind, STALL};
use crate::adapt::Watchdog;
use crate::path;

/// Minimum file size (32 MiB) to attempt multi-range segmented streaming.
pub(super) const SEGMENT_MIN_SIZE: u64 = 32 << 20;
/// Target size per segment (16 MiB), clamped to `2..=8` concurrent streams.
pub(super) const SEGMENT_TARGET_SIZE: u64 = 16 << 20;
/// Buffer size per segment before issuing a blocking positional write (~256 KiB).
const SEGMENT_WRITE_BUF: usize = 256 * 1024;
/// Persist `.irori-part.seg` after this many newly written bytes per segment.
const SEGMENT_PERSIST_BYTES: u64 = 2 << 20;

pub(super) enum SegmentedOutcome {
    Done,
    Fallback,
}

/// Persistent state of a multi-range segmented download (`<dest>.irori-part.seg`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SegmentState {
    pub(super) size: u64,
    pub(super) etag_or_lm: Option<String>,
    pub(super) segments: Vec<SegmentCursor>,
}

/// Byte cursor of one contiguous segment `[start ..= end]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SegmentCursor {
    pub(super) start: u64,
    pub(super) end: u64,
    pub(super) done: u64,
}

impl SegmentCursor {
    pub(super) fn len(self) -> u64 {
        self.end.saturating_sub(self.start).saturating_add(1)
    }

    pub(super) fn is_complete(self) -> bool {
        self.done >= self.len()
    }
}

impl SegmentState {
    /// Partition `size` bytes evenly across `count` segments (`count >= 2`).
    pub(super) fn new(size: u64, etag_or_lm: Option<String>, count: usize) -> Self {
        let count = count.clamp(2, 8) as u64;
        let base = size / count;
        let rem = size % count;
        let mut segments = Vec::with_capacity(count as usize);
        let mut offset = 0u64;
        for i in 0..count {
            let len = base + u64::from(i < rem);
            if len == 0 {
                continue;
            }
            segments.push(SegmentCursor {
                start: offset,
                end: offset + len - 1,
                done: 0,
            });
            offset += len;
        }
        Self {
            size,
            etag_or_lm,
            segments,
        }
    }

    /// Total bytes already written across all segments.
    pub(super) fn downloaded_bytes(&self) -> u64 {
        self.segments
            .iter()
            .fold(0u64, |acc, s| acc.saturating_add(s.done.min(s.len())))
    }

    /// Validate that segments cleanly partition `0..expected_size` and that
    /// `etag_or_lm` matches when both sides provide a validator.
    pub(super) fn is_valid_for(&self, expected_size: u64, validator: Option<&str>) -> bool {
        if self.size != expected_size || expected_size == 0 || self.segments.is_empty() {
            return false;
        }
        if let (Some(saved), Some(current)) = (self.etag_or_lm.as_deref(), validator)
            && saved != current
        {
            return false;
        }
        let mut expected_start = 0u64;
        for seg in &self.segments {
            if seg.start != expected_start || seg.end < seg.start || seg.end >= expected_size {
                return false;
            }
            if seg.done > seg.len() {
                return false;
            }
            expected_start = seg.end.saturating_add(1);
        }
        expected_start == expected_size
    }
}

fn segment_count(size: u64, target_chunk: u64) -> usize {
    let target = target_chunk.max(1);
    (size / target).clamp(2, 8) as usize
}

/// Parse `(start, end, total)` from an HTTP `Content-Range` header (`bytes X-Y/TOTAL`).
fn parse_content_range(header: &str) -> Option<(u64, u64, u64)> {
    let rest = header.trim().strip_prefix("bytes ")?.trim();
    let (span, total_str) = rest.split_once('/')?;
    let (start_str, end_str) = span.split_once('-')?;
    let start: u64 = start_str.trim().parse().ok()?;
    let end: u64 = end_str.trim().parse().ok()?;
    let total: u64 = total_str.trim().parse().ok()?;
    (start <= end && end < total).then_some((start, end, total))
}

fn extract_validator(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get(reqwest::header::ETAG)
        .or_else(|| headers.get(reqwest::header::LAST_MODIFIED))
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Read `<dest>.irori-part.seg` synchronously, returning `None` if absent or malformed.
pub(super) fn load_segment_state_sync(seg_path: &Path) -> Option<SegmentState> {
    let bytes = std::fs::read(seg_path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Atomically persist `SegmentState` to `seg_path` via a temp sibling file.
async fn save_segment_state(seg_path: &Path, state: &SegmentState) -> std::io::Result<()> {
    let payload = serde_json::to_vec(state).map_err(std::io::Error::other)?;
    let mut tmp_name = seg_path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp = seg_path.with_file_name(tmp_name);
    tokio::fs::write(&tmp, &payload).await?;
    if let Err(e) = irori_core::fs_part::finalize_part_async(&tmp, seg_path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    Ok(())
}

/// Real downloaded bytes currently on disk for an in-flight `<dest>.irori-part`
/// file. Uses `<dest>.irori-part.seg` when present (so sparse pre-sized files
/// report actual downloaded bytes rather than logical file length).
pub fn partial_bytes(dest: &Path) -> Option<u64> {
    let part = path::part_path(dest);
    let meta = std::fs::metadata(&part).ok().filter(|m| m.is_file())?;
    let seg = path::seg_path(dest);
    if let Some(state) = load_segment_state_sync(&seg)
        && state.is_valid_for(state.size, None)
        && meta.len() == state.size
    {
        return Some(state.downloaded_bytes());
    }
    Some(meta.len())
}

/// Safe positional write of all `data` bytes at exact `offset` into `file`,
/// without touching `unsafe`. Call from `tokio::task::spawn_blocking`.
fn write_all_at(file: &std::fs::File, mut offset: u64, mut data: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::FileExt as _;
    #[cfg(windows)]
    use std::os::windows::fs::FileExt as _;

    while !data.is_empty() {
        #[cfg(windows)]
        let n = file.seek_write(data, offset)?;
        #[cfg(unix)]
        let n = file.write_at(data, offset)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "failed to write whole buffer at offset",
            ));
        }
        offset += n as u64;
        data = &data[n..];
    }
    Ok(())
}

impl Ctx {
    /// Probe `Range: bytes=0-0` + `Accept-Encoding: identity`. Returns `Ok(None)`
    /// to fall back to single-stream when the server ignores Range or reports a
    /// different total size.
    pub(super) async fn probe_segmented(
        &self,
        key: &str,
        expected_size: u64,
        shared_url: &tokio::sync::RwLock<String>,
        resign_lock: &tokio::sync::Mutex<()>,
        resigns: &AtomicU32,
    ) -> Result<Option<Option<String>>, Error> {
        loop {
            let url = shared_url.read().await.clone();
            let resp = {
                let _permit = self
                    .requests
                    .acquire(&self.cancel)
                    .await
                    .ok_or(Error::Cancelled)?;
                let req = self
                    .client
                    .get(&url)
                    .header(reqwest::header::RANGE, "bytes=0-0")
                    .header(reqwest::header::ACCEPT_ENCODING, "identity");
                self.send(req, &url).await?
            };
            let status = resp.status();
            if self.signing.is_some() && matches!(status.as_u16(), 401 | 403 | 404 | 410) {
                self.refresh_signed_url(key, &url, shared_url, resign_lock, resigns, &self.cancel)
                    .await?;
                continue;
            }
            if status == reqwest::StatusCode::PARTIAL_CONTENT {
                let total = resp
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(parse_content_range)
                    .map(|(_, _, total)| total);
                if total != Some(expected_size) {
                    return Ok(None);
                }
                return Ok(Some(extract_validator(resp.headers())));
            }
            if status.is_success() {
                return Ok(None);
            }
            return Err(Error::Status {
                status,
                url: redact(&url),
            });
        }
    }

    pub(super) async fn open_segmented_files(
        &self,
        part: &Path,
        seg_path: &Path,
        size: u64,
        validator: Option<String>,
        allow_resume: bool,
    ) -> Result<Option<(Arc<std::fs::File>, SegmentState)>, Error> {
        let part_buf = part.to_path_buf();
        let seg_buf = seg_path.to_path_buf();
        let sparse_fn = self.sparse_fn;
        if allow_resume {
            let existing_len = file_len(part).await;
            let existing_state =
                tokio::task::spawn_blocking(move || load_segment_state_sync(&seg_buf))
                    .await
                    .map_err(|e| Error::Other(e.into()))?;

            if let Some(state) = existing_state
                && existing_len == Some(size)
                && state.is_valid_for(size, validator.as_deref())
            {
                let opened = tokio::task::spawn_blocking(
                    move || -> std::io::Result<Option<std::fs::File>> {
                        let file = std::fs::OpenOptions::new()
                            .read(true)
                            .write(true)
                            .open(&part_buf)?;
                        if !sparse_fn(&file) {
                            return Ok(None);
                        }
                        Ok(Some(file))
                    },
                )
                .await
                .map_err(|e| Error::Other(e.into()))??;
                return match opened {
                    Some(file) => Ok(Some((Arc::new(file), state))),
                    None => {
                        let _ = tokio::fs::remove_file(part).await;
                        let _ = tokio::fs::remove_file(seg_path).await;
                        Ok(None)
                    }
                };
            }
        }

        // Fresh segmented transfer (or stale/mismatched ETag, or retry after corruption):
        // Create empty sparse file and persist `.seg` (`done: 0`) BEFORE `set_len(size)`
        // so a crash can never leave an all-zero `len == size` file without `.seg`.
        let _ = tokio::fs::remove_file(seg_path).await;
        let part_create = part.to_path_buf();
        let created =
            tokio::task::spawn_blocking(move || -> std::io::Result<Option<std::fs::File>> {
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .open(&part_create)?;
                if !sparse_fn(&file) {
                    drop(file);
                    let _ = std::fs::remove_file(&part_create);
                    return Ok(None);
                }
                Ok(Some(file))
            })
            .await
            .map_err(|e| Error::Other(e.into()))??;

        let Some(file) = created else {
            return Ok(None);
        };
        let count = segment_count(size, self.segment_target_size);
        let state = SegmentState::new(size, validator, count);
        if let Err(e) = save_segment_state(seg_path, &state).await {
            drop(file);
            let _ = tokio::fs::remove_file(part).await;
            return Err(e.into());
        }
        let file = Arc::new(file);
        let prealloc = file.clone();
        let set_len_res = tokio::task::spawn_blocking(move || prealloc.set_len(size))
            .await
            .map_err(|e| Error::Other(e.into()))?;
        if let Err(e) = set_len_res {
            drop(file);
            let _ = tokio::fs::remove_file(part).await;
            let _ = tokio::fs::remove_file(seg_path).await;
            return Err(e.into());
        }
        Ok(Some((file, state)))
    }

    async fn refresh_signed_url(
        &self,
        key: &str,
        failed_url: &str,
        shared_url: &tokio::sync::RwLock<String>,
        resign_lock: &tokio::sync::Mutex<()>,
        resigns: &AtomicU32,
        cancel: &CancellationToken,
    ) -> Result<String, Error> {
        let Some(signing) = &self.signing else {
            return Err(Error::Other(anyhow::anyhow!("no signer available")));
        };
        let _guard = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(Error::Cancelled),
            g = resign_lock.lock() => g,
        };
        {
            let cur = shared_url.read().await;
            if *cur != failed_url {
                return Ok(cur.clone());
            }
        }
        let used = resigns.fetch_add(1, Ordering::Relaxed);
        if used >= self.retries {
            return Err(Error::Corrupt(
                "the server rejected the download address after re-signing".into(),
            ));
        }
        signing.forget(key);
        let fresh = signing.url(key, cancel).await?;
        *shared_url.write().await = fresh.clone();
        Ok(fresh)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn stream_segment_worker(
        &self,
        idx: usize,
        key: &str,
        total_size: u64,
        file: Arc<std::fs::File>,
        seg_path: &Path,
        state: &std::sync::Mutex<SegmentState>,
        persist_lock: &tokio::sync::Mutex<()>,
        shared_url: &tokio::sync::RwLock<String>,
        resign_lock: &tokio::sync::Mutex<()>,
        resigns: &AtomicU32,
        counted: &AtomicU64,
        seg_cancel: &CancellationToken,
    ) -> Result<(), Error> {
        let res = self
            .stream_segment_worker_inner(
                idx,
                key,
                total_size,
                file,
                seg_path,
                state,
                persist_lock,
                shared_url,
                resign_lock,
                resigns,
                counted,
                seg_cancel,
            )
            .await;
        if let Err(ref e) = res
            && !matches!(e, Error::Cancelled)
        {
            seg_cancel.cancel();
        }
        res
    }

    #[allow(clippy::too_many_arguments)]
    async fn stream_segment_worker_inner(
        &self,
        idx: usize,
        key: &str,
        total_size: u64,
        file: Arc<std::fs::File>,
        seg_path: &Path,
        state: &std::sync::Mutex<SegmentState>,
        persist_lock: &tokio::sync::Mutex<()>,
        shared_url: &tokio::sync::RwLock<String>,
        resign_lock: &tokio::sync::Mutex<()>,
        resigns: &AtomicU32,
        counted: &AtomicU64,
        seg_cancel: &CancellationToken,
    ) -> Result<(), Error> {
        let mut failures = 0u32;
        let mut since_persist = 0u64;
        let mut buf = Vec::with_capacity(SEGMENT_WRITE_BUF);

        loop {
            let cursor = {
                let s = state.lock().unwrap_or_else(|e| e.into_inner());
                s.segments[idx]
            };
            if cursor.is_complete() {
                return Ok(());
            }
            if seg_cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }

            let cur_start = cursor.start + cursor.done;
            let seg_end = cursor.end;
            let url = shared_url.read().await.clone();
            let permit = self
                .requests
                .acquire(seg_cancel)
                .await
                .ok_or(Error::Cancelled)?;
            let started = Instant::now();
            let req = self
                .client
                .get(&url)
                .header(
                    reqwest::header::RANGE,
                    format!("bytes={cur_start}-{seg_end}"),
                )
                .header(reqwest::header::ACCEPT_ENCODING, "identity");
            let resp = match self.send_cancellable(req, &url, seg_cancel).await {
                Ok(r) => r,
                Err(e) => {
                    drop(permit);
                    self.score_err(&url, &e);
                    if matches!(e, Error::Cancelled) || seg_cancel.is_cancelled() {
                        return Err(Error::Cancelled);
                    }
                    if e.kind() == Kind::Transient {
                        failures += 1;
                        self.backoff_cancellable(failures, seg_cancel).await?;
                        continue;
                    }
                    return Err(e);
                }
            };

            let status = resp.status();
            if self.signing.is_some() && matches!(status.as_u16(), 401 | 403 | 404 | 410) {
                drop(permit);
                self.refresh_signed_url(key, &url, shared_url, resign_lock, resigns, seg_cancel)
                    .await?;
                continue;
            }
            if status != reqwest::StatusCode::PARTIAL_CONTENT {
                drop(permit);
                let err = if status.is_success() {
                    Error::Other(anyhow::anyhow!(
                        "the server does not support partial downloads"
                    ))
                } else {
                    Error::Status {
                        status,
                        url: redact(&url),
                    }
                };
                self.score_err(&url, &err);
                if err.kind() == Kind::Transient {
                    failures += 1;
                    self.backoff_cancellable(failures, seg_cancel).await?;
                    continue;
                }
                return Err(err);
            }
            if let Some(cr) = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                && !parse_content_range(cr)
                    .is_some_and(|(s, _, t)| s == cur_start && t == total_size)
            {
                drop(permit);
                return Err(Error::Corrupt(format!(
                    "unexpected Content-Range {cr:?} for segment {idx}"
                )));
            }

            let mut write_offset = cur_start;
            let mut got_this_req = 0u64;
            let mut body = resp.bytes_stream();
            let mut watch = Watchdog::new(STALL, Instant::now());
            let generation = self.generation();
            let seg_limit = seg_end + 1;
            let mut stream_err: Option<Error> = None;

            loop {
                let waited = Instant::now();
                let next = tokio::select! {
                    biased;
                    () = seg_cancel.cancelled() => {
                        stream_err = Some(Error::Cancelled);
                        break;
                    }
                    n = tokio::time::timeout(STALL, body.next()) => n,
                };
                self.stats.add_net_wait(waited.elapsed());
                let bytes = match next {
                    Ok(None) => break,
                    Ok(Some(Ok(b))) => b,
                    Ok(Some(Err(e))) => {
                        stream_err = Some(e.into());
                        break;
                    }
                    Err(_) => {
                        stream_err = Some(stalled(&url));
                        break;
                    }
                };
                let n = bytes.len() as u64;
                if write_offset + (buf.len() as u64) + n > seg_limit {
                    stream_err = Some(Error::Corrupt(
                        "the server sent more data than expected".into(),
                    ));
                    break;
                }
                match self.throttle_cancellable(bytes.len(), seg_cancel).await {
                    Ok(d) => watch.pause(d),
                    Err(e) => {
                        stream_err = Some(e);
                        break;
                    }
                }
                self.stats.add_received(n);
                got_this_req += n;
                buf.extend_from_slice(&bytes);

                if buf.len() >= SEGMENT_WRITE_BUF {
                    let chunk = std::mem::take(&mut buf);
                    let len = chunk.len() as u64;
                    self.flush_segment_chunk(
                        &file,
                        idx,
                        cursor.start,
                        write_offset,
                        chunk,
                        state,
                        counted,
                    )
                    .await?;
                    write_offset += len;
                    since_persist += len;
                    if since_persist >= SEGMENT_PERSIST_BYTES {
                        self.persist_segment_state(seg_path, state, persist_lock)
                            .await?;
                        since_persist = 0;
                    }
                }
                if let Err(e) = watch.feed(n, Instant::now()) {
                    stream_err = Some(Error::Interrupted(format!("{}: {e}", redact(&url))));
                    break;
                }
                if write_offset + (buf.len() as u64) < seg_limit && self.generation() != generation
                {
                    stream_err = Some(Error::Recycled);
                    break;
                }
            }

            if !buf.is_empty() {
                let chunk = std::mem::take(&mut buf);
                let len = chunk.len() as u64;
                self.flush_segment_chunk(
                    &file,
                    idx,
                    cursor.start,
                    write_offset,
                    chunk,
                    state,
                    counted,
                )
                .await?;
                write_offset += len;
                since_persist += len;
            }
            if since_persist > 0 {
                self.persist_segment_state(seg_path, state, persist_lock)
                    .await?;
                since_persist = 0;
            }
            drop(permit);

            match stream_err {
                None => {
                    if write_offset == seg_limit {
                        self.score(&url, &Ok(()), got_this_req, started.elapsed());
                        return Ok(());
                    }
                    let err = Error::Interrupted(format!(
                        "segment {idx} closed after {} of {} bytes",
                        write_offset - cursor.start,
                        cursor.len()
                    ));
                    self.score_err(&url, &err);
                    failures += 1;
                    self.backoff_cancellable(failures, seg_cancel).await?;
                }
                Some(Error::Recycled) if !seg_cancel.is_cancelled() => {
                    self.score(&url, &Ok(()), got_this_req, started.elapsed());
                }
                Some(Error::Cancelled) => return Err(Error::Cancelled),
                Some(e) if e.kind() == Kind::Transient && !seg_cancel.is_cancelled() => {
                    self.score_err(&url, &e);
                    failures += 1;
                    self.backoff_cancellable(failures, seg_cancel).await?;
                }
                Some(e) => {
                    self.score_err(&url, &e);
                    return Err(e);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn flush_segment_chunk(
        &self,
        file: &Arc<std::fs::File>,
        idx: usize,
        seg_start: u64,
        write_offset: u64,
        chunk: Vec<u8>,
        state: &std::sync::Mutex<SegmentState>,
        counted: &AtomicU64,
    ) -> Result<(), Error> {
        let len = chunk.len() as u64;
        let file = file.clone();
        let writing = Instant::now();
        tokio::task::spawn_blocking(move || write_all_at(&file, write_offset, &chunk))
            .await
            .map_err(|e| Error::Other(e.into()))??;
        self.stats.add_disk_wait(writing.elapsed());
        self.stats.add_written(len);
        self.stats.add_done(len);
        counted.fetch_add(len, Ordering::Relaxed);
        let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
        guard.segments[idx].done = (write_offset + len).saturating_sub(seg_start);
        Ok(())
    }

    async fn persist_segment_state(
        &self,
        seg_path: &Path,
        state: &std::sync::Mutex<SegmentState>,
        persist_lock: &tokio::sync::Mutex<()>,
    ) -> Result<(), Error> {
        let _guard = persist_lock.lock().await;
        let snapshot = state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        save_segment_state(seg_path, &snapshot).await?;
        Ok(())
    }
}
