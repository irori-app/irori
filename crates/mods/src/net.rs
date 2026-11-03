//! Downloading mod archives with progress and cancellation.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context as _, Result, bail};
use futures::StreamExt as _;
use irori_core::process::CancellationToken;
use tokio::io::AsyncWriteExt as _;

/// Bytes received and expected, readable from any thread.
#[derive(Debug, Default)]
pub struct DownloadProgress {
    pub done: AtomicU64,
    pub total: AtomicU64,
}

impl DownloadProgress {
    /// Fraction done in `0.0..=1.0`, when the size is known.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total.load(Ordering::Relaxed);
        (total > 0).then(|| (self.done.load(Ordering::Relaxed) as f32 / total as f32).min(1.0))
    }
}

/// Download `url` to `dest`. A response larger than `max_bytes` stops the
/// download: mods are never that big, and the server may be misbehaving.
pub async fn download_to(
    url: &str,
    dest: &Path,
    max_bytes: u64,
    progress: Option<Arc<DownloadProgress>>,
    cancel: &CancellationToken,
) -> Result<()> {
    let response = irori_core::http::download_client()
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("the download could not start: {}", e.without_url()))?
        .error_for_status()
        .map_err(|e| anyhow::anyhow!("the server refused the download: {}", e.without_url()))?;
    if let Some(len) = response.content_length() {
        if len > max_bytes {
            bail!("the download is larger than expected");
        }
        if let Some(p) = &progress {
            p.total.store(len, Ordering::Relaxed);
        }
    }
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("creating {}", dest.display()))?;
    let mut stream = response.bytes_stream();
    let mut received = 0u64;
    loop {
        let chunk = tokio::select! {
            () = cancel.cancelled() => bail!("cancelled"),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = chunk else {
            break;
        };
        let chunk =
            chunk.map_err(|e| anyhow::anyhow!("the download broke off: {}", e.without_url()))?;
        received += chunk.len() as u64;
        if received > max_bytes {
            bail!("the download is larger than expected");
        }
        file.write_all(&chunk).await?;
        if let Some(p) = &progress {
            p.done.store(received, Ordering::Relaxed);
        }
    }
    file.flush().await?;
    Ok(())
}

/// Largest mod archive Irori downloads (some texture packs pass 10 GB).
pub const MAX_MOD_BYTES: u64 = 64 * 1024 * 1024 * 1024;
