//! Whole-file streaming downloads with HTTP range resume and connection
//! recycling.

use std::path::Path;
use std::time::Instant;

use futures::StreamExt as _;
use tokio::io::AsyncWriteExt as _;

use super::support::{file_len, inflate_file, redact, stalled};
use super::{Ctx, Error, STALL};
use crate::adapt::Watchdog;
use crate::path;
use crate::plan::{Codec, FileEntry};

impl Ctx {
    /// Download a whole-file object into `<dest>.irori-part`, resuming what
    /// an earlier attempt left, then verify and move it into place. Servers
    /// are tried in `order`, starting at the `attempt`th.
    pub(super) async fn stream_file(
        &self,
        f: &FileEntry,
        dest: &Path,
        order: &[usize],
        attempt: u32,
    ) -> Result<(), Error> {
        let part = path::part_path(dest);
        let chunk = &f.parts[0].chunk;
        if chunk.urls.is_empty() && self.signing.is_none() {
            return Err(Error::Other(anyhow::anyhow!("no download address")));
        }
        let mut counted = 0u64;
        let result = async {
            if let Some(s) = &self.signing {
                let url = s.url(&chunk.key, &self.cancel).await?;
                let res = self
                    .stream_once(&url, &part, chunk.download_size, chunk.range, &mut counted)
                    .await;
                if let Err(Error::Status { status, .. }) = &res
                    && matches!(status.as_u16(), 401 | 403 | 404 | 410)
                {
                    // Likely an expired signature or link: sign again on the
                    // next attempt, and name the answer in case it wasn't.
                    s.forget(&chunk.key);
                    return Err(Error::Corrupt(format!(
                        "the server answered {status}; the download address may have expired"
                    )));
                }
                res?;
            } else {
                // Other mirrors are tried right away when one refuses the request.
                let n = order.len();
                let mut tried = 0;
                loop {
                    let url = &chunk.urls[order[(attempt as usize + tried) % n]];
                    match self
                        .stream_once(url, &part, chunk.download_size, chunk.range, &mut counted)
                        .await
                    {
                        Err(Error::Status { .. }) if tried + 1 < n => {
                            self.stats.sub_done(counted);
                            counted = 0;
                            tried += 1;
                        }
                        other => break other,
                    }
                }?;
            }
            let finished = if matches!(chunk.codec, Codec::Deflate) {
                let inflated = part.with_extension("irori-inflate");
                let (src, dst) = (part.clone(), inflated.clone());
                tokio::task::spawn_blocking(move || inflate_file(&src, &dst))
                    .await
                    .map_err(|e| Error::Other(e.into()))?
                    .map_err(|e| Error::Corrupt(format!("{}: {e}", f.path)))?;
                let _ = tokio::fs::remove_file(&part).await;
                // Progress counted compressed bytes; account for the file.
                self.stats.sub_done(counted);
                self.stats.add_done(f.size);
                counted = f.size;
                inflated
            } else {
                part.clone()
            };
            if !self.intact(&finished, f).await? {
                let _ = tokio::fs::remove_file(&finished).await;
                return Err(Error::Corrupt(format!(
                    "{} does not match its checksum",
                    f.path
                )));
            }
            tokio::fs::rename(&finished, dest).await?;
            Ok(())
        }
        .await;
        if result.is_err() {
            // The next attempt counts whatever of the part file survives.
            self.stats.sub_done(counted);
        }
        result
    }

    async fn stream_once(
        &self,
        url: &str,
        part: &Path,
        size: u64,
        range: Option<(u64, u64)>,
        counted: &mut u64,
    ) -> Result<(), Error> {
        let mut have = file_len(part).await.unwrap_or(0);
        if have > size {
            tokio::fs::remove_file(part).await?;
            have = 0;
        }
        // What an earlier attempt left on disk counts as done.
        self.stats.add_done(have);
        *counted = have;
        if have == size {
            return Ok(());
        }

        let _permit = self
            .requests
            .acquire(&self.cancel)
            .await
            .ok_or(Error::Cancelled)?;
        let started = Instant::now();
        let before = *counted;
        let result = self
            .stream_body(url, part, size, range, have, counted)
            .await;
        self.score(
            url,
            &result,
            counted.saturating_sub(before),
            started.elapsed(),
        );
        result
    }

    /// The request of [`Self::stream_once`], appending to the `have` bytes
    /// of `part`.
    async fn stream_body(
        &self,
        url: &str,
        part: &Path,
        size: u64,
        range: Option<(u64, u64)>,
        mut have: u64,
        counted: &mut u64,
    ) -> Result<(), Error> {
        let mut req = self.client.get(url);
        match range {
            Some((start, len)) => {
                req = req.header(
                    reqwest::header::RANGE,
                    format!("bytes={}-{}", start + have, start + len - 1),
                )
            }
            None if have > 0 => req = req.header(reqwest::header::RANGE, format!("bytes={have}-")),
            None => {}
        }
        let resp = self.send(req, url).await?;
        let status = resp.status();
        let append = match status {
            reqwest::StatusCode::PARTIAL_CONTENT => have > 0,
            s if s.is_success() && range.is_some() => {
                return Err(Error::Other(anyhow::anyhow!(
                    "the server does not support partial downloads"
                )));
            }
            s if s.is_success() => false,
            status => {
                return Err(Error::Status {
                    status,
                    url: redact(url),
                });
            }
        };
        let file = if append {
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(part)
                .await?
        } else {
            self.stats.sub_done(*counted);
            *counted = 0;
            have = 0;
            tokio::fs::File::create(part).await?
        };
        let mut out = tokio::io::BufWriter::with_capacity(1 << 20, file);
        let mut body = resp.bytes_stream();
        let mut watch = Watchdog::new(STALL, Instant::now());
        let generation = self.generation();
        loop {
            let waited = Instant::now();
            let next = tokio::select! {
                _ = self.cancel.cancelled() => {
                    out.flush().await?;
                    return Err(Error::Cancelled);
                }
                n = tokio::time::timeout(STALL, body.next()) => n,
            };
            self.stats.add_net_wait(waited.elapsed());
            let bytes = match next {
                Ok(None) => break,
                Ok(Some(Ok(b))) => b,
                Ok(Some(Err(e))) => {
                    // Keep what arrived for the next attempt.
                    out.flush().await?;
                    return Err(e.into());
                }
                Err(_) => {
                    out.flush().await?;
                    return Err(stalled(url));
                }
            };
            let n = bytes.len() as u64;
            if have + n > size {
                drop(out);
                tokio::fs::remove_file(part).await?;
                return Err(Error::Corrupt(
                    "the server sent more data than expected".into(),
                ));
            }
            watch.pause(self.throttle(bytes.len()).await?);
            let writing = Instant::now();
            out.write_all(&bytes).await?;
            self.stats.add_disk_wait(writing.elapsed());
            have += n;
            *counted += n;
            self.stats.add_received(n);
            self.stats.add_written(n);
            self.stats.add_done(n);
            if let Err(e) = watch.feed(n, Instant::now()) {
                out.flush().await?;
                return Err(Error::Interrupted(format!("{}: {e}", redact(url))));
            }
            if have < size && self.generation() != generation {
                // Keep what arrived; the rest comes on a new connection.
                out.flush().await?;
                return Err(Error::Recycled);
            }
        }
        out.flush().await?;
        if have != size {
            return Err(Error::Interrupted(format!(
                "the connection closed after {have} of {size} bytes"
            )));
        }
        Ok(())
    }

    /// Send `req`, giving up on a server that does not answer in time.
    pub(super) async fn send(
        &self,
        mut req: reqwest::RequestBuilder,
        url: &str,
    ) -> Result<reqwest::Response, Error> {
        let close = self
            .recycle
            .to_close
            .try_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |n| n.checked_sub(1),
            )
            .is_ok();
        if close {
            req = req.header(reqwest::header::CONNECTION, "close");
        }
        tokio::select! {
            _ = self.cancel.cancelled() => Err(Error::Cancelled),
            r = tokio::time::timeout(STALL * 2, req.send()) => match r {
                Ok(r) => Ok(r?),
                Err(_) => Err(stalled(url)),
            },
        }
    }
}

#[cfg(test)]
mod recycle_tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering::Relaxed;
    use std::time::Duration;

    use tokio_util::sync::CancellationToken;

    use super::super::support::ChunkCache;
    use super::super::{Ctx, Recycle};
    use crate::adapt::{Gate, Hosts};
    use crate::plan::FileEntry;
    use crate::progress::Stats;
    use crate::test_server::{TestServer, serve_bytes};

    /// Heroic #6018: after a slump, a stream stops and resumes where it was
    /// on a new connection, without counting as a failure, and the next
    /// requests close their connections.
    #[tokio::test]
    async fn streams_continue_on_new_connections() {
        let body: Vec<u8> = (0..512 * 1024u32).map(|i| (i % 251) as u8).collect();
        let served = body.clone();
        let server = TestServer::start(move |req| {
            let mut resp = serve_bytes(req, &served, true);
            resp.chunk_delay = Some(Duration::from_millis(15));
            resp
        });
        let dir = tempfile::tempdir().unwrap();
        let digest = {
            use sha2::Digest as _;
            crate::Digest::new(
                crate::Algorithm::Sha256,
                sha2::Sha256::digest(&body).to_vec(),
            )
        };
        let file = FileEntry::whole(
            "big.bin".into(),
            body.len() as u64,
            Some(digest),
            vec![server.url("/big.bin")],
        );
        let files = [file.clone()];
        let ctx = Arc::new(Ctx {
            client: reqwest::Client::new(),
            root: dir.path().to_path_buf(),
            requests: Gate::new(4),
            assemble_slots: tokio::sync::Semaphore::new(4),
            hosts: Hosts::default(),
            limiter: None,
            cache: ChunkCache::new(&files, 1 << 20),
            stats: Arc::new(Stats::new(body.len() as u64)),
            cancel: CancellationToken::new(),
            journal: None,
            retries: 1,
            signing: None,
            prefetch: 4,
            recycle: Recycle::default(),
        });
        let task = {
            let ctx = ctx.clone();
            tokio::spawn(async move { ctx.process_file(&file).await })
        };
        while ctx.stats.received.load(Relaxed) < 64 * 1024 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        ctx.replace_connections(4);
        task.await.unwrap().unwrap();

        assert_eq!(std::fs::read(dir.path().join("big.bin")).unwrap(), body);
        assert_eq!(ctx.stats.failures.load(Relaxed), 0);
        let requests = server.requests();
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert!(requests[0].header("range").is_none());
        let range = requests[1].header("range").unwrap();
        let from: usize = range["bytes=".len()..range.len() - 1].parse().unwrap();
        assert!(from >= 64 * 1024 && from < body.len(), "{range}");
        assert_eq!(requests[1].header("connection"), Some("close"));
        assert_eq!(ctx.recycle.to_close.load(Relaxed), 3);
    }
}
