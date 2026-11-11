//! MSIXVC installs, updates and repairs, ported from the Nexus backend's
//! streaming command.
//!
//! A package is read over HTTP: [`Remote::open`] fetches its header and the
//! metadata naming every file (the start of the package is cached in
//! `.irori-xbox-tmp.msixvc`), the caller gets the content key from the
//! licence, and [`Remote::install`] writes the files into the game folder.
//! Encrypted files (executables and libraries) stay encrypted on disk, as on
//! Windows; the launch decrypts them. Once every file is written the cached
//! header becomes `.irori-xbox.msixvc`, the package metadata a launch and the
//! next update read.
//!
//! * Resume: `.irori-xbox-journal.jsonl` records synced offsets per file, so
//!   an interrupted download continues where it stopped ([`resume`]).
//! * Update: with an installed package, only changed files are written, each
//!   staged in `.irori-xbox-update` and swapped in when complete; pages whose
//!   hash matches installed data are reused instead of downloaded
//!   ([`update`]). `.irori-xbox-update-incomplete` blocks launches until the
//!   update finishes ([`state`]).
//! * Repair: an installed file is written again when its metadata changed or
//!   its encrypted data is short.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream};
use irori_core::install::{InstallProgress, ProgressSink};
use irori_msixvc::streaming;
use irori_msixvc::xvd::{SegmentFile, XvdFile};
use tokio::fs::OpenOptions;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio_util::sync::CancellationToken;

mod failure;
mod resume;
pub(crate) mod state;
mod update;

#[cfg(test)]
mod preflight_tests;

/// The package metadata a launch and the next update read.
pub const PACKAGE_FILE: &str = ".irori-xbox.msixvc";
/// The same file as Nexus Launcher (Xodus) names it, read when importing a
/// game it installed.
pub const XODUS_PACKAGE_FILE: &str = ".xodus-streaming.msixvc";
/// The start of a package while it is being installed.
const CACHE_FILE: &str = ".irori-xbox-tmp.msixvc";

struct Job {
    name: String,
    content: SegmentFile,
}

/// The journal behind its lock; a writer that panicked leaves it unusable.
fn lock(
    journal: &std::sync::Mutex<resume::Journal>,
) -> Result<std::sync::MutexGuard<'_, resume::Journal>, InstallError> {
    journal
        .lock()
        .map_err(|_| InstallError::Failed("download checkpoints are unusable".into()))
}

fn pending_files<'a>(
    root: &Path,
    remote: &'a HashMap<String, SegmentFile>,
    local: &HashMap<String, SegmentFile>,
) -> std::io::Result<Vec<(&'a String, &'a SegmentFile)>> {
    let mut pending = Vec::new();
    for (name, file) in remote {
        let unchanged = local.get(name).is_some_and(|old| {
            !file.data_hashs.is_empty()
                && file.data_hashs == old.data_hashs
                && file.length == old.length
                && file.keep_encrypted == old.keep_encrypted
        });
        let needs_repair = if unchanged && file.keep_encrypted {
            // Matching package metadata alone does not prove that the on-disk
            // ciphertext still includes the full final page.
            let target = resume::target(root, name)?;
            match std::fs::metadata(target) {
                Ok(metadata) if metadata.is_file() => metadata.len() < file.stored_length()?,
                Ok(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "package image is not a regular file",
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                Err(error) => return Err(error),
            }
        } else {
            false
        };
        if !unchanged || needs_repair {
            pending.push((name, file));
        }
    }
    Ok(pending)
}

enum ProgressEvent {
    Advanced {
        delta: u64,
    },
    Remaining {
        total: u64,
    },
    /// Bytes downloaded and bytes reused from installed files.
    Traffic {
        #[allow(dead_code)] // Tests assert it; progress needs only `reused`.
        network: u64,
        reused: u64,
    },
}

/// Why an install stopped.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("The download was cancelled")]
    Cancelled,
    #[error("Not enough free space on {path}: {needed} needed, {available} free")]
    NoSpace {
        path: String,
        needed: String,
        available: String,
    },
    #[error("{0}")]
    Failed(String),
}

impl InstallError {
    fn from_transfer(error: &(dyn std::error::Error + 'static), stage: &'static str) -> Self {
        let message = failure::message(error, stage);
        if message.contains("transfer-cancelled") {
            InstallError::Cancelled
        } else {
            InstallError::Failed(message)
        }
    }
}

/// A package on Microsoft's servers with its header and file list read.
pub struct Remote {
    client: reqwest::Client,
    url: String,
    destination: PathBuf,
    xvd: XvdFile,
    files: HashMap<String, SegmentFile>,
    _lock: std::fs::File,
}

impl Remote {
    /// Read the package at `url` (a CDN address from the package lookup)
    /// for installing into `destination`, which is created and locked.
    pub async fn open(
        client: &reqwest::Client,
        url: &str,
        destination: &Path,
    ) -> Result<Self, InstallError> {
        std::fs::create_dir_all(destination).map_err(|e| {
            InstallError::Failed(format!("Could not create {}: {e}", destination.display()))
        })?;
        let lock = resume::lock(destination)
            .map_err(|e| InstallError::Failed(format!("Cannot start this download: {e}")))?;
        let http = streaming::HttpRead::open(client.clone(), url, None::<fn(u64, u64)>)
            .await
            .map_err(|e| InstallError::from_transfer(&e, "transfer-io"))?;
        let length = http.len();
        let mut remote =
            streaming::PrefixCacheFile::new(http, length, destination.join(CACHE_FILE))
                .await
                .map_err(|e| InstallError::from_transfer(&e, "transfer-io"))?;
        let metadata = |e: Box<dyn std::error::Error + Send + Sync>| {
            InstallError::from_transfer(e.as_ref(), "metadata-invalid")
        };
        let xvd = XvdFile::parse(&mut remote).await.map_err(metadata)?;
        let mut files = HashMap::new();
        for (name, entry) in &xvd
            .parse_user_package_files(&mut remote)
            .await
            .map_err(metadata)?
        {
            if name == "SegmentMetadata.bin" {
                files = xvd
                    .parse_segment_metadata(&mut remote, entry)
                    .await
                    .map_err(metadata)?;
            }
        }
        let ntfs = xvd
            .parse_ntfs_segment_metadata(&mut remote, !files.is_empty())
            .await
            .map_err(metadata)?;
        files.extend(ntfs);
        Ok(Self {
            client: client.clone(),
            url: url.to_string(),
            destination: destination.to_path_buf(),
            xvd,
            files,
            _lock: lock,
        })
    }

    /// The content ID the licence is asked for.
    pub fn content_id(&self) -> uuid::Uuid {
        self.xvd.content_id()
    }

    /// Bytes of every file the package installs.
    pub fn size(&self) -> u64 {
        self.files.values().map(|f| f.length).sum()
    }

    /// Write the package's files into the destination (resuming, updating
    /// or repairing what is there) with `key` from its licence.
    pub async fn install(
        self,
        key: [u8; 32],
        parallel: usize,
        progress: &ProgressSink,
        cancel: &CancellationToken,
    ) -> Result<(), InstallError> {
        let (tx, rx) = tokio::sync::mpsc::channel::<ProgressEvent>(256);
        let reporter = tokio::spawn(report(rx, progress.clone()));
        let (failed, failure_signal) = tokio::sync::watch::channel(false);
        let failed = Arc::new(failed);
        let watcher = {
            let (cancel, failed) = (cancel.clone(), failed.clone());
            tokio::spawn(async move {
                cancel.cancelled().await;
                failed.send_replace(true);
            })
        };
        let result = self
            .write_files(key, parallel, &tx, failed.clone(), failure_signal)
            .await;
        watcher.abort();
        drop(tx);
        let _ = reporter.await;
        if cancel.is_cancelled() {
            return Err(InstallError::Cancelled);
        }
        result
    }

    async fn write_files(
        &self,
        key: [u8; 32],
        parallel: usize,
        tx: &Sender<ProgressEvent>,
        failed: Arc<tokio::sync::watch::Sender<bool>>,
        failure_signal: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), InstallError> {
        let out = self.destination.as_path();
        let io = |e: std::io::Error| InstallError::Failed(format!("{e}"));
        let final_path = out.join(PACKAGE_FILE);
        let cache_path = out.join(CACHE_FILE);
        let lfiles = match installed_files(&final_path).await {
            Ok(files) => files,
            Err(e) => return Err(InstallError::from_transfer(e.as_ref(), "metadata-invalid")),
        };
        let rfiles = &self.files;
        let recovering = state::incomplete(out).map_err(io)?;
        let mut pending = update::pending(out, rfiles, &lfiles, recovering).map_err(|e| {
            InstallError::Failed(format!("Unable to inspect installed package files: {e}"))
        })?;
        let identity =
            resume::Journal::identity(&self.xvd.resume_identity(), rfiles).map_err(io)?;
        let installed_journal = resume::Journal::open_with_identity(out, &identity)
            .map(|j| Arc::new(std::sync::Mutex::new(j)))
            .map_err(|e| {
                InstallError::Failed(format!("Unable to prepare download checkpoints: {e}"))
            })?;
        let stage = if !final_path.exists() && !recovering {
            None
        } else {
            Some(update::staging(out).map_err(io)?)
        };
        if stage.is_some() {
            let installed = lock(&installed_journal)?;
            pending.retain(|(name, file)| !update::completed(&installed, out, name, file));
        }
        let journal = match &stage {
            Some(stage) => resume::Journal::open_with_identity(stage, &identity)
                .map(|j| Arc::new(std::sync::Mutex::new(j)))
                .map_err(io)?,
            None => installed_journal.clone(),
        };
        let total_size: u64 = pending.iter().map(|(_, file)| file.length).sum();
        let (saved, required) = {
            let journal = lock(&journal)?;
            let saved: u64 = pending
                .iter()
                .map(|(name, file)| journal.offset(name, file.length))
                .sum();
            let required = match &stage {
                Some(_) => {
                    update::required_space(out, pending.iter().copied(), &journal).map_err(io)?
                }
                None => total_size.saturating_sub(saved),
            };
            (saved, required)
        };
        let available = available_space(out).map_err(io)?;
        if available < required {
            return Err(InstallError::NoSpace {
                path: out.display().to_string(),
                needed: irori_core::util::format_bytes(required),
                available: irori_core::util::format_bytes(available),
            });
        }
        if stage.is_some() {
            update::begin(out, rfiles, &pending).map_err(io)?;
        }
        tx.send(ProgressEvent::Remaining { total: total_size })
            .await
            .ok();
        tx.send(ProgressEvent::Advanced { delta: 0 }).await.ok();
        let _ = saved;

        let xvd = &self.xvd;
        let url = self.url.as_str();
        let concurrency = if stage.is_some() { 1 } else { parallel.max(1) };
        let first_error: std::sync::Mutex<Option<InstallError>> = std::sync::Mutex::new(None);
        let (_pause_tx, pause) = tokio::sync::watch::channel(false);
        stream::iter(pending.into_iter().map(|(n, v)| Job {
            name: n.clone(),
            content: SegmentFile {
                offset: v.offset,
                length: v.length,
                data_hashs: vec![],
                keep_encrypted: v.keep_encrypted,
            },
        }))
        .for_each_concurrent(concurrency, |job| {
            let tx = tx.clone();
            let client = self.client.clone();
            let pause = pause.clone();
            let journal = journal.clone();
            let installed_journal = installed_journal.clone();
            let stage = stage.as_deref();
            let old = lfiles.get(&job.name);
            let full_content = &rfiles[&job.name];
            let failed = failed.clone();
            let failure_signal = failure_signal.clone();
            let first_error = &first_error;
            async move {
                if *failure_signal.borrow() {
                    return;
                }
                let fail = |error: InstallError| {
                    if !failed.send_replace(true)
                        && let Ok(mut slot) = first_error.lock()
                    {
                        slot.get_or_insert(error);
                    }
                };
                let opened = async {
                    let target_file = match stage {
                        Some(stage) => update::output(stage, &job.name)?,
                        None => resume::target(out, &job.name)?,
                    };
                    if let Some(folder) = target_file.parent() {
                        std::fs::create_dir_all(folder)?;
                    }
                    OpenOptions::new()
                        .create(true)
                        .write(true)
                        .truncate(false)
                        .open(&target_file)
                        .await
                }
                .await;
                let mut fout = match opened {
                    Ok(file) => file,
                    Err(error) => {
                        return fail(InstallError::Failed(format!("{}: {error}", job.name)));
                    }
                };
                let operation = async {
                    let mut offset = journal
                        .lock()
                        .map_err(|_| failure::poisoned())?
                        .offset(&job.name, job.content.length);
                    offset = resume::prepare_output(&mut fout, offset, &job.content).await?;
                    let mut reported = offset;
                    tx.send(ProgressEvent::Advanced { delta: offset })
                        .await
                        .ok();
                    if offset == job.content.length {
                        return Ok::<(), failure::Error>(());
                    }
                    journal
                        .lock()
                        .map_err(|_| failure::poisoned())?
                        .checkpoint(&job.name, offset)?;
                    let progress = |position: u64, _| {
                        if tx
                            .try_send(ProgressEvent::Advanced {
                                delta: position.saturating_sub(reported),
                            })
                            .is_ok()
                        {
                            reported = position;
                        }
                    };
                    if stage.is_some() {
                        update::transfer(
                            xvd,
                            &client,
                            url,
                            &resume::target(out, &job.name)?,
                            old,
                            full_content,
                            &mut fout,
                            key,
                            offset,
                            pause.clone(),
                            parallel,
                            &tx,
                            |synced| {
                                journal
                                    .lock()
                                    .map_err(|_| std::io::Error::other("checkpoints unusable"))?
                                    .checkpoint_synced(&job.name, synced)
                            },
                        )
                        .await?;
                    } else {
                        xvd.download_file_http(
                            &client,
                            url,
                            &mut fout,
                            &job.content,
                            key,
                            progress,
                            Some(pause.clone()),
                            offset,
                            |position| {
                                journal
                                    .lock()
                                    .map_err(|_| std::io::Error::other("checkpoints unusable"))?
                                    .checkpoint(&job.name, position)
                            },
                        )
                        .await?;
                    }
                    Ok(())
                };
                let result = failure::run_or_cancel(failure_signal, operation).await;
                let result = failure::settle(&mut fout, result, || {
                    journal
                        .lock()
                        .map_err(|_| std::io::Error::other("checkpoints unusable"))?
                        .checkpoint(&job.name, job.content.length)
                })
                .await;
                let result = result.and_then(|()| {
                    if let Some(stage) = stage {
                        update::commit(stage, out, &job.name)?;
                        installed_journal
                            .lock()
                            .map_err(|_| failure::poisoned())?
                            .checkpoint(&job.name, job.content.length)?;
                    }
                    Ok(())
                });
                if let Err(error) = result {
                    fail(InstallError::from_transfer(error.as_ref(), "transfer-io"));
                }
            }
        })
        .await;

        if *failure_signal.borrow() {
            let error = first_error.lock().ok().and_then(|mut slot| slot.take());
            return Err(error.unwrap_or(InstallError::Cancelled));
        }
        state::finish(out, &cache_path, &final_path).map_err(io)?;
        Ok(())
    }
}

/// The files of the package already installed at `package` (none when it is
/// missing).
async fn installed_files(
    package: &Path,
) -> Result<HashMap<String, SegmentFile>, Box<dyn std::error::Error + Send + Sync>> {
    let Ok(mut file) = OpenOptions::new().read(true).open(package).await else {
        return Ok(HashMap::new());
    };
    launch_files_of(&mut file).await
}

/// Every file of the package whose metadata `file` holds, with the files of
/// its plain (unencrypted) region added where the encrypted segment metadata
/// leaves them out.
pub async fn launch_files_of(
    file: &mut tokio::fs::File,
) -> Result<HashMap<String, SegmentFile>, Box<dyn std::error::Error + Send + Sync>> {
    // Callers may have read the header already; parsing starts at the file's
    // position, so a second parse without this would read past the header.
    tokio::io::AsyncSeekExt::seek(file, std::io::SeekFrom::Start(0)).await?;
    let xvd = XvdFile::parse(&mut *file).await?;
    let mut files = HashMap::new();
    for (name, entry) in &xvd.parse_user_package_files(&mut *file).await? {
        if name == "SegmentMetadata.bin" {
            files = xvd.parse_segment_metadata(&mut *file, entry).await?;
        }
    }
    if files.is_empty() {
        files = xvd.parse_ntfs_segment_metadata(&mut *file, false).await?;
    } else if let Ok(plain) = xvd.parse_ntfs_segment_metadata(&mut *file, true).await {
        let known: std::collections::HashSet<String> =
            files.keys().map(|n| n.to_ascii_lowercase()).collect();
        for (name, content) in plain {
            // A plain-region read never overrides an encrypted declaration.
            if !content.keep_encrypted && !known.contains(&name.to_ascii_lowercase()) {
                files.insert(name, content);
            }
        }
    }
    Ok(files)
}

fn available_space(path: &Path) -> std::io::Result<u64> {
    #[cfg(unix)]
    {
        let stat = nix::sys::statvfs::statvfs(path).map_err(std::io::Error::from)?;
        Ok(stat.blocks_available() as u64 * stat.fragment_size() as u64)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(u64::MAX)
    }
}

/// Turn transfer events into Irori's progress, twice a second. Pages an
/// update reuses from the installed files count as written, not downloaded.
async fn report(mut events: Receiver<ProgressEvent>, sink: ProgressSink) {
    let started = Instant::now();
    let (mut done, mut total, mut reused) = (0u64, 0u64, 0u64);
    let mut last = Instant::now() - Duration::from_secs(1);
    while let Some(event) = events.recv().await {
        match event {
            ProgressEvent::Advanced { delta } => done = done.saturating_add(delta),
            ProgressEvent::Remaining { total: t } => total = t,
            ProgressEvent::Traffic {
                network: _,
                reused: r,
            } => reused = reused.saturating_add(r),
        }
        if last.elapsed() < Duration::from_millis(500) {
            continue;
        }
        last = Instant::now();
        let elapsed = started.elapsed().as_secs_f64().max(0.001);
        let written = (done as f64 / elapsed) as u64;
        let downloaded = (done.saturating_sub(reused) as f64 / elapsed) as u64;
        sink(InstallProgress {
            percent: (total > 0).then(|| (done.min(total) as f64 * 100.0 / total as f64) as f32),
            bytes_done: Some(done),
            bytes_total: (total > 0).then_some(total),
            eta_secs: (written > 0 && total > done).then(|| (total - done) / written),
            download_speed: Some(downloaded),
            disk_speed: Some(written),
            ..Default::default()
        });
    }
}

#[cfg(test)]
mod launch_files_tests {
    use super::*;

    /// A launch reads the header first and then the files from the same
    /// handle; the second read must not start where the first stopped.
    #[tokio::test]
    async fn reading_the_files_rewinds_the_package() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PACKAGE_FILE);
        let mut header = vec![0u8; 0x3000];
        header[512..520].copy_from_slice(b"msft-xvd");
        header[528..536].copy_from_slice(&116444736000000000u64.to_le_bytes());
        header[652..656].copy_from_slice(&4u32.to_le_bytes());
        std::fs::write(&path, &header).unwrap();
        let mut file = tokio::fs::File::open(&path).await.unwrap();
        XvdFile::parse(&mut file).await.unwrap();
        let error = launch_files_of(&mut file)
            .await
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(!error.contains("magic"), "read past the header: {error}");
    }
}
