//! Downloads every graphics mod shares: lists refreshed once a day with a
//! stale copy standing in when the network fails, and files fetched with
//! progress and cancellation into a cache folder.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use irori_core::install::ProgressSink;
use irori_core::json_store;
use irori_core::process::CancellationToken;
use sha2::{Digest as _, Sha256};

/// How long a downloaded list is used before asking again.
pub const LIST_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

fn fresh(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < LIST_MAX_AGE)
}

/// A daily-refreshed download kept at `path`: the cached copy while it is
/// fresh, else a new one, else the stale copy. `what` names the data in
/// the log.
pub async fn cached<T>(
    what: &str,
    path: PathBuf,
    fetch: impl AsyncFnOnce() -> Result<Vec<u8>>,
    parse: impl Fn(&[u8]) -> Result<T>,
) -> Result<T> {
    load(what, path, true, fetch, parse).await
}

/// [`cached`], but asking the source even when the cached copy is fresh:
/// for data an install checks downloads against. The cached copy still
/// stands in when the source can't be reached.
pub async fn refreshed<T>(
    what: &str,
    path: PathBuf,
    fetch: impl AsyncFnOnce() -> Result<Vec<u8>>,
    parse: impl Fn(&[u8]) -> Result<T>,
) -> Result<T> {
    load(what, path, false, fetch, parse).await
}

async fn load<T>(
    what: &str,
    path: PathBuf,
    use_fresh: bool,
    fetch: impl AsyncFnOnce() -> Result<Vec<u8>>,
    parse: impl Fn(&[u8]) -> Result<T>,
) -> Result<T> {
    if use_fresh
        && fresh(&path)
        && let Ok(bytes) = tokio::fs::read(&path).await
        && let Ok(value) = parse(&bytes)
    {
        return Ok(value);
    }
    match fetch().await.and_then(|bytes| Ok((parse(&bytes)?, bytes))) {
        Ok((value, bytes)) => {
            let path = path.clone();
            if let Err(e) =
                tokio::task::spawn_blocking(move || json_store::write_bytes(&path, &bytes)).await?
            {
                tracing::warn!("caching {what} data: {e:#}");
            }
            Ok(value)
        }
        Err(e) => match tokio::fs::read(&path).await {
            Ok(bytes) => {
                tracing::warn!("using {what} data cached earlier: {e:#}");
                parse(&bytes)
            }
            Err(_) => Err(e),
        },
    }
}

/// Download `url` through a file in `dir` and return its bytes.
pub async fn fetch_bytes(
    url: &str,
    dir: &Path,
    progress: &ProgressSink,
    cancel: &CancellationToken,
) -> Result<Vec<u8>> {
    // Two games may download the same file at once; each gets its own name.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dest = dir.join(format!(
        "{:x}-{}-{n}",
        Sha256::digest(url.as_bytes()),
        std::process::id()
    ));
    irori_runtime::download::download(url, &dest, progress, cancel).await?;
    let bytes = tokio::fs::read(&dest).await?;
    let _ = tokio::fs::remove_file(&dest).await;
    Ok(bytes)
}
