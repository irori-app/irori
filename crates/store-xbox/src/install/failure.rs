//! Settles active writes before a failed batch releases its installation lock.
use irori_msixvc::streaming_http::TransferError;
use std::future::Future;
#[cfg(test)]
use std::path::Path;
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::watch,
};
pub(super) type Error = Box<dyn std::error::Error + Send + Sync>;
pub(super) fn cancelled() -> Error {
    Box::new(TransferError::new("transfer-cancelled", 0))
}
pub(super) async fn run_or_cancel<F>(
    mut stop: watch::Receiver<bool>,
    operation: F,
) -> Result<(), Error>
where
    F: Future<Output = Result<(), Error>>,
{
    tokio::select! {
        biased;
        _ = stop.wait_for(|v| *v) => Err(cancelled()),
        result = operation => result,
    }
}
pub(super) async fn settle<W: AsyncWrite + Unpin>(
    out: &mut W,
    result: Result<(), Error>,
    completed: impl FnOnce() -> std::io::Result<()>,
) -> Result<(), Error> {
    out.flush().await?;
    result?;
    completed()?;
    Ok(())
}
/// A user-facing message for a failed transfer: the transfer stage when one
/// is known (it never holds addresses or tokens), else `stage`.
pub(super) fn message(
    mut error: &(dyn std::error::Error + 'static),
    stage: &'static str,
) -> String {
    let mut found = None;
    for _ in 0..8 {
        if let Some(value) = error.downcast_ref::<TransferError>() {
            found = Some(*value);
            break;
        }
        if let Some(inner) = error
            .downcast_ref::<std::io::Error>()
            .and_then(|error| error.get_ref())
        {
            error = inner;
        } else if let Some(inner) = error.source() {
            error = inner;
        } else {
            break;
        }
    }
    let error = found.unwrap_or(TransferError::new(stage, 0));
    tracing::debug!("{}", error.diagnostic());
    format!("{error} Retry this installation to get a fresh package link and resume safely.")
}

/// The checkpoint journal's lock was poisoned by a panicking writer.
pub(super) fn poisoned() -> Error {
    Box::new(std::io::Error::other("download checkpoints are unusable"))
}

#[cfg(test)]
pub(super) fn finalize(cache: &Path, final_path: &Path, failed: bool) -> std::io::Result<bool> {
    if failed {
        return Ok(false);
    }
    // Atomic replacement: never delete the previous completed package marker first.
    std::fs::rename(cache, final_path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    #[tokio::test]
    async fn streaming_failure_cancels_and_settles_parallel_workers_without_completing_failed_jobs()
    {
        let (stop, rx) = watch::channel(false);
        let complete = Arc::new(AtomicBool::new(false));
        let wrote = Arc::new(AtomicBool::new(false));
        let (started, wait) = tokio::sync::oneshot::channel();
        let task = {
            let complete = complete.clone();
            let wrote = wrote.clone();
            async move {
                let mut out = Vec::new();
                let result = run_or_cancel(rx, async {
                    out.write_all(b"partial").await?;
                    wrote.store(true, Ordering::SeqCst);
                    started.send(()).ok();
                    std::future::pending::<()>().await;
                    Ok(())
                })
                .await;
                let failed = settle(&mut out, result, || {
                    complete.store(true, Ordering::SeqCst);
                    Ok(())
                })
                .await
                .is_err();
                (failed, out)
            }
        };
        let cancel = async {
            wait.await.unwrap();
            stop.send(true).unwrap();
        };
        let ((failed, out), ()) = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            tokio::join!(task, cancel)
        })
        .await
        .unwrap();
        assert!(failed && wrote.load(Ordering::SeqCst));
        assert!(!complete.load(Ordering::SeqCst));
        assert_eq!(out, b"partial");
    }
    #[tokio::test]
    async fn streaming_failure_settle_commits_only_success_and_propagates_checkpoint_errors() {
        let mut out = Vec::new();
        let mut committed = false;
        settle(&mut out, Ok(()), || {
            committed = true;
            Ok(())
        })
        .await
        .unwrap();
        assert!(committed);
        assert!(
            settle(&mut out, Ok(()), || Err(std::io::Error::other(
                "synthetic checkpoint failure"
            )))
            .await
            .is_err()
        );
    }
    #[test]
    fn streaming_failure_preserves_existing_game_and_dlc_files_and_completed_marker() {
        let root = std::env::temp_dir().join(format!("irori-xbox-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let cache = root.join(".irori-xbox-tmp.msixvc");
        let final_path = root.join(".irori-xbox.msixvc");
        for file in ["game.exe", "dlc.bin"] {
            std::fs::write(root.join(file), b"existing game data").unwrap();
        }
        std::fs::write(&final_path, b"old completed package").unwrap();
        std::fs::write(&cache, b"partial").unwrap();
        assert!(!finalize(&cache, &final_path, true).unwrap());
        assert_eq!(
            std::fs::read(&final_path).unwrap(),
            b"old completed package"
        );
        assert!(cache.exists());
        assert!(finalize(&root.join("missing"), &final_path, false).is_err());
        assert_eq!(
            std::fs::read(&final_path).unwrap(),
            b"old completed package"
        );
        assert!(finalize(&cache, &final_path, false).unwrap());
        assert_eq!(std::fs::read(&final_path).unwrap(), b"partial");
        for file in ["game.exe", "dlc.bin"] {
            assert_eq!(
                std::fs::read(root.join(file)).unwrap(),
                b"existing game data"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
