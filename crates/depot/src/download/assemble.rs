//! Multi-chunk file assembly: prefetches shared chunks, decodes and verifies
//! them off the async workers, and writes files in order.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use futures::{StreamExt as _, TryStreamExt as _};
use tokio::io::AsyncWriteExt as _;

use super::support::{decode, read_at, redact, set_executable, stalled};
use super::{CHECKERS, Ctx, Error, Kind, STALL};
use crate::adapt::Watchdog;
use crate::hash::{Digest, hash_file};
use crate::path;
use crate::plan::{Chunk, FileEntry, Part};
use crate::progress::Stats;

/// Where the bytes of one part come from.
enum PartData {
    Chunk(Arc<Vec<u8>>),
    Local(Vec<u8>),
}

impl Ctx {
    /// Skip files already in place: recorded in the journal, or of the
    /// right size and matching their digest. Returns the indices of the
    /// files still to download.
    pub(super) async fn check_existing(
        &self,
        files: &[FileEntry],
        on_disk: &[Option<u64>],
    ) -> Result<Vec<usize>, Error> {
        let mut pending = Vec::new();
        let mut to_hash = Vec::new();
        for (i, f) in files.iter().enumerate() {
            if on_disk[i] != Some(f.size) {
                pending.push(i);
            } else if self.journal.as_ref().is_some_and(|j| j.has(f)) {
                self.skip(f);
            } else if f.hash.is_none() {
                self.skip(f);
                self.record(f).await;
            } else {
                to_hash.push(i);
            }
        }
        use std::sync::atomic::Ordering::Relaxed;
        self.stats.check_total.store(to_hash.len() as u64, Relaxed);
        let checked: Vec<(usize, bool)> = futures::stream::iter(to_hash)
            .map(|i| async move {
                let f = &files[i];
                let ok = self.intact(&path::join(&self.root, &f.path), f).await?;
                self.stats.checked.fetch_add(1, Relaxed);
                Ok::<_, Error>((i, ok))
            })
            .buffer_unordered(CHECKERS)
            .try_collect()
            .await?;
        for (i, ok) in checked {
            if ok {
                self.skip(&files[i]);
                self.record(&files[i]).await;
            } else {
                pending.push(i);
            }
        }
        pending.sort_unstable();
        Ok(pending)
    }

    pub(super) async fn process_file(&self, f: &FileEntry) -> Result<(), Error> {
        self.check_cancel()?;
        let dest = path::join(&self.root, &f.path);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Chunks retry on their own; whole files retry here, moving to the
        // next server after each failure.
        let order = match f.parts.first() {
            Some(p) if f.is_stream() => self.hosts.order(&p.chunk.urls),
            _ => Vec::new(),
        };
        let mirrors = order.len().max(1) as u32;
        let (mut attempt, mut failures, mut corrupt) = (0, 0, 0);
        loop {
            let result = if f.parts.is_empty() {
                self.write_empty(f, &dest).await
            } else if f.is_stream() {
                self.stream_file(f, &dest, &order, attempt).await
            } else {
                // Local copies are only trusted on the first attempt.
                self.assemble(f, &dest, attempt == 0).await
            };
            let e = match result {
                Ok(()) => break,
                Err(e) => e,
            };
            let kind = e.kind();
            if kind == Kind::Recycled && !self.cancel.is_cancelled() {
                continue;
            }
            if self.cancel.is_cancelled()
                || kind == Kind::Fatal
                || (kind == Kind::Corrupt && corrupt >= self.retries)
            {
                return Err(match e {
                    Error::Cancelled => e,
                    e => Error::Other(anyhow::Error::from(e).context(f.path.clone())),
                });
            }
            tracing::warn!("{} (attempt {}): {e}", f.path, attempt + 1);
            attempt += 1;
            if kind == Kind::Corrupt {
                corrupt += 1;
                self.backoff(corrupt).await?;
            } else {
                failures += 1;
                // Every server gets a try before waiting.
                if failures >= mirrors {
                    self.backoff(failures + 1 - mirrors).await?;
                }
            }
        }
        if f.executable {
            set_executable(&dest).await;
        }
        self.record(f).await;
        Ok(())
    }

    /// Whether the file on disk (of the right size) matches its digest.
    pub(super) async fn intact(&self, dest: &Path, f: &FileEntry) -> Result<bool, Error> {
        let Some(hash) = f.hash.clone() else {
            return Ok(true);
        };
        let dest = dest.to_path_buf();
        let cancel = self.cancel.clone();
        let res = tokio::task::spawn_blocking(move || {
            hash_file(&dest, &hash, |_| !cancel.is_cancelled())
        })
        .await
        .map_err(|e| Error::Other(e.into()))?;
        match res {
            Ok(ok) => Ok(ok),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Err(Error::Cancelled),
            Err(e) => Err(e.into()),
        }
    }

    fn skip(&self, f: &FileEntry) {
        self.stats.add_done(f.size);
        for p in &f.parts {
            self.cache.release(&p.chunk.key);
        }
    }

    async fn record(&self, f: &FileEntry) {
        if let Some(j) = &self.journal {
            j.record(f).await;
        }
    }

    async fn write_empty(&self, f: &FileEntry, dest: &Path) -> Result<(), Error> {
        if f.size != 0 {
            return Err(Error::Other(anyhow::anyhow!(
                "the manifest lists no content for a {}-byte file",
                f.size
            )));
        }
        tokio::fs::File::create(dest).await?;
        Ok(())
    }

    /// Build a file from chunk slices (or local copies of them), verify it
    /// and move it into place.
    async fn assemble(&self, f: &FileEntry, dest: &Path, use_local: bool) -> Result<(), Error> {
        // Tracks bytes already added to `stats.done` for this in-flight file
        // so both errors and future-drop cancellations (from `tokio::select!`)
        // roll back uncommitted progress before a final sample is read.
        struct Uncommitted<'a> {
            stats: &'a Stats,
            written: u64,
        }
        impl Drop for Uncommitted<'_> {
            fn drop(&mut self) {
                if self.written > 0 {
                    self.stats.sub_done(self.written);
                }
            }
        }

        let part = path::part_path(dest);
        let mut out =
            tokio::io::BufWriter::with_capacity(1 << 20, tokio::fs::File::create(&part).await?);
        let mut hasher = f.hash.as_ref().map(Digest::hasher);
        let mut uncommitted = Uncommitted {
            stats: &self.stats,
            written: 0,
        };
        let result = async {
            let mut parts = futures::stream::iter(f.parts.clone())
                .map(|p| async move {
                    let data = self.part_data(&p, use_local).await?;
                    Ok::<_, Error>((p, data))
                })
                .buffered(self.prefetch);
            loop {
                let waited = Instant::now();
                let Some(next) = parts.next().await else {
                    break;
                };
                self.stats.add_net_wait(waited.elapsed());
                let (p, data) = next?;
                let slice = match &data {
                    PartData::Local(bytes) => bytes.as_slice(),
                    PartData::Chunk(chunk) => usize::try_from(p.offset)
                        .ok()
                        .zip(usize::try_from(p.offset + p.size).ok())
                        .and_then(|(start, end)| chunk.get(start..end))
                        .ok_or_else(|| {
                            Error::Corrupt("a file part lies outside its chunk".into())
                        })?,
                };
                let writing = Instant::now();
                out.write_all(slice).await?;
                self.stats.add_disk_wait(writing.elapsed());
                if let Some(h) = &mut hasher {
                    h.update(slice);
                }
                self.cache.release(&p.chunk.key);
                let n = slice.len() as u64;
                uncommitted.written += n;
                self.stats.add_written(n);
                self.stats.add_done(n);
            }
            out.flush().await?;
            drop(out);
            if uncommitted.written != f.size {
                return Err(Error::Corrupt(format!(
                    "{} has {} bytes instead of {}",
                    f.path, uncommitted.written, f.size
                )));
            }
            if let (Some(h), Some(expected)) = (hasher, &f.hash)
                && h.finish() != expected.value
            {
                return Err(Error::Corrupt(format!(
                    "{} does not match its checksum",
                    f.path
                )));
            }
            Ok(())
        }
        .await;
        if let Err(e) = result {
            if !matches!(e, Error::Cancelled) {
                let _ = tokio::fs::remove_file(&part).await;
            }
            return Err(e);
        }
        tokio::fs::rename(&part, dest).await?;
        uncommitted.written = 0;
        Ok(())
    }

    /// The bytes of a part: read from the local copy when allowed and
    /// readable, else from its (shared, cached) chunk.
    async fn part_data(&self, p: &Part, use_local: bool) -> Result<PartData, Error> {
        if use_local && let Some(reuse) = &p.reuse {
            let src = path::join(&self.root, &reuse.path);
            let (offset, size) = (reuse.offset, p.size);
            let read = tokio::task::spawn_blocking(move || read_at(&src, offset, size))
                .await
                .map_err(|e| Error::Other(e.into()))?;
            match read {
                Ok(bytes) => return Ok(PartData::Local(bytes)),
                Err(e) => tracing::debug!("reusing {}: {e}; downloading instead", reuse.path),
            }
        }
        Ok(PartData::Chunk(self.chunk(&p.chunk).await?))
    }

    /// A decoded chunk, downloaded once while other files still need it.
    async fn chunk(&self, chunk: &Arc<Chunk>) -> Result<Arc<Vec<u8>>, Error> {
        let cell = self.cache.cell(&chunk.key);
        cell.get_or_try_init(|| async {
            let data = self.fetch_chunk(chunk).await?;
            self.cache.add_bytes(data.len() as u64);
            Ok::<_, Error>(Arc::new(data))
        })
        .await
        .cloned()
    }

    /// A chunk from the best server that has it. Failing servers are left
    /// for the next one right away; once every server failed, the engine
    /// waits a little longer each round and keeps trying.
    async fn fetch_chunk(&self, chunk: &Chunk) -> Result<Vec<u8>, Error> {
        if chunk.urls.is_empty() && self.signing.is_none() {
            return Err(Error::Other(anyhow::anyhow!(
                "chunk {} has no address",
                chunk.key
            )));
        }
        let order = match &self.signing {
            Some(_) => Vec::new(),
            None => self.hosts.order(&chunk.urls),
        };
        let mirrors = order.len().max(1);
        let (mut next, mut refused) = (0, 0);
        let (mut failures, mut corrupt, mut resigned) = (0u32, 0u32, 0u32);
        loop {
            let (url, alt) = match &self.signing {
                Some(s) => {
                    let url = s.url(&chunk.key).await?;
                    (url.clone(), url)
                }
                None => (
                    chunk.urls[order[next % mirrors]].clone(),
                    chunk.urls[order[(next + 1) % mirrors]].clone(),
                ),
            };
            let e = match self.fetch_raced(chunk, &url, &alt).await {
                Ok(data) => return Ok(data),
                Err(e) => e,
            };
            if self.cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            match e.kind() {
                // An expired signature: sign again.
                Kind::Fatal
                    if self.signing.is_some()
                        && matches!(&e, Error::Status { status, .. } if matches!(status.as_u16(), 401 | 403))
                        && resigned < self.retries =>
                {
                    if let Some(s) = &self.signing {
                        s.forget(&chunk.key);
                    }
                    resigned += 1;
                }
                // Other servers may have what this one refuses.
                Kind::Fatal if matches!(e, Error::Status { .. }) && refused + 1 < mirrors => {
                    refused += 1;
                    next += 1;
                }
                Kind::Fatal => return Err(e),
                Kind::Corrupt => {
                    corrupt += 1;
                    if corrupt > self.retries.max(mirrors as u32 - 1) {
                        return Err(e);
                    }
                    tracing::warn!("chunk {} (attempt {corrupt}): {e}", chunk.key);
                    next += 1;
                    self.backoff(corrupt).await?;
                }
                // Chunk requests finish on their connection; never stopped.
                Kind::Recycled => {}
                Kind::Transient => {
                    failures += 1;
                    tracing::warn!("chunk {} (failure {failures}): {e}", chunk.key);
                    next += 1;
                    if failures as usize >= mirrors {
                        self.backoff(failures + 1 - mirrors as u32).await?;
                    }
                }
            }
        }
    }

    /// Fetch from `url`; when that lags while request slots sit idle (the
    /// end of a download), race the same chunk from `alt` and take
    /// whichever arrives first.
    async fn fetch_raced(&self, chunk: &Chunk, url: &str, alt: &str) -> Result<Vec<u8>, Error> {
        let first = self.fetch_chunk_once(chunk, url);
        tokio::pin!(first);
        let patience = self
            .hosts
            .chunk_time()
            .map_or(STALL, |t| (t * 3).clamp(STALL / 4, STALL * 2));
        tokio::select! {
            r = &mut first => return r,
            () = tokio::time::sleep(patience) => {}
        }
        if !self.requests.has_spare() {
            return first.await;
        }
        tracing::debug!("chunk {} is slow; racing a second request", chunk.key);
        let second = self.fetch_chunk_once(chunk, alt);
        tokio::pin!(second);
        tokio::select! {
            r = &mut first => match r {
                Err(e) if !matches!(e, Error::Cancelled) => second.await,
                r => r,
            },
            r = &mut second => match r {
                Err(e) if !matches!(e, Error::Cancelled) => first.await,
                r => r,
            },
        }
    }

    async fn fetch_chunk_once(&self, chunk: &Chunk, url: &str) -> Result<Vec<u8>, Error> {
        let raw = {
            let _permit = self.requests.acquire().await;
            let started = Instant::now();
            let raw = self.request_chunk(chunk, url).await;
            let bytes = raw.as_ref().map_or(0, |r| r.len() as u64);
            self.score(url, &raw, bytes, started.elapsed());
            raw?
        };
        // Decoding and hashing are CPU-bound: keep them off the async workers.
        let (codec, size, hash) = (chunk.codec.clone(), chunk.size, chunk.hash.clone());
        let decoded = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
            let data = decode(&codec, raw).map_err(|e| format!("{e:#}"))?;
            if data.len() as u64 != size {
                return Err(format!("{} bytes instead of {size}", data.len()));
            }
            if hash.is_some_and(|h| !h.matches(&data)) {
                return Err("does not match its checksum".into());
            }
            Ok(data)
        })
        .await
        .map_err(|e| Error::Other(e.into()))?
        .map_err(|e| Error::Corrupt(format!("chunk {}: {e}", chunk.key)));
        if decoded.is_err() {
            self.hosts.record_failure(url);
        }
        decoded
    }

    /// The raw bytes of `chunk` from `url`.
    async fn request_chunk(&self, chunk: &Chunk, url: &str) -> Result<Vec<u8>, Error> {
        let mut req = self.client.get(url);
        if let Some((start, len)) = chunk.range {
            req = req.header(
                reqwest::header::RANGE,
                format!("bytes={start}-{}", start + len - 1),
            );
        }
        let resp = self.send(req, url).await?;
        if !resp.status().is_success() {
            return Err(Error::Status {
                status: resp.status(),
                url: redact(url),
            });
        }
        if chunk.range.is_some() && resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(Error::Other(anyhow::anyhow!(
                "the server does not support partial downloads"
            )));
        }
        let mut raw = Vec::with_capacity(chunk.download_size as usize);
        let mut body = resp.bytes_stream();
        let mut watch = Watchdog::new(STALL, Instant::now());
        loop {
            let next = tokio::select! {
                _ = self.cancel.cancelled() => return Err(Error::Cancelled),
                n = tokio::time::timeout(STALL, body.next()) => n.map_err(|_| stalled(url))?,
            };
            let Some(bytes) = next else { break };
            let bytes = bytes?;
            watch.pause(self.throttle(bytes.len()).await?);
            self.stats.add_received(bytes.len() as u64);
            raw.extend_from_slice(&bytes);
            if chunk.download_size > 0 && raw.len() as u64 > chunk.download_size {
                return Err(Error::Corrupt(format!(
                    "{} sent more than the chunk's {} bytes",
                    redact(url),
                    chunk.download_size
                )));
            }
            watch
                .feed(bytes.len() as u64, Instant::now())
                .map_err(|e| Error::Interrupted(format!("{}: {e}", redact(url))))?;
        }
        Ok(raw)
    }
}
