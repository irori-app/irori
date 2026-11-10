//! Bounded, atomic per-file updates with authenticated reuse of relocated pages.
use super::{ProgressEvent, failure, resume};
use irori_msixvc::{
    layout::PAGE_SIZE,
    xvd::{SegmentFile, StoredPageVerifier, XvdFile},
};
#[cfg(test)]
use std::io::Read;
use std::{
    collections::HashMap,
    fs,
    io::{self, SeekFrom},
    path::{Path, PathBuf},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWriteExt},
    sync::{mpsc::Sender, watch},
};

#[cfg(test)]
#[path = "update_throughput_tests.rs"]
mod throughput_tests;

pub(super) fn staging(root: &Path) -> io::Result<PathBuf> {
    let path = resume::target(root, ".irori-xbox-update")?;
    match fs::create_dir(&path) {
        Ok(()) => (),
        Err(e)
            if e.kind() == io::ErrorKind::AlreadyExists
                && fs::symlink_metadata(&path)?.is_dir() => {}
        Err(e) => return Err(e),
    }
    Ok(path)
}

pub(super) fn pending<'a>(
    root: &Path,
    remote: &'a HashMap<String, SegmentFile>,
    local: &HashMap<String, SegmentFile>,
    recovering: bool,
) -> io::Result<Vec<(&'a String, &'a SegmentFile)>> {
    let mut files = super::pending_files(root, remote, local)?;
    if recovering {
        let queued: std::collections::HashSet<&String> =
            files.iter().map(|(name, _)| *name).collect();
        let placeholders = crate::install::state::placeholders(root)?;
        // The old final metadata no longer describes every installed file.
        // If a newer package arrives during recovery, even an apparent revert
        // to the old metadata must pass the new package's verification again.
        for (name, file) in remote {
            if queued.contains(name) {
                continue;
            }
            let path = resume::target(root, name)?;
            if placeholders.contains(name)
                && !file.keep_encrypted
                && file.length > 0
                && fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == 0)
            {
                continue; // Preserve deliberately uninstalled optional archives.
            }
            files.push((name, file));
        }
    }
    Ok(files)
}

pub(super) fn begin(
    root: &Path,
    remote: &HashMap<String, SegmentFile>,
    pending: &[(&String, &SegmentFile)],
) -> io::Result<()> {
    let queued: std::collections::HashSet<&String> =
        pending.iter().map(|(name, _)| *name).collect();
    let mut placeholders = Vec::new();
    for (name, file) in remote {
        if queued.contains(name) || file.keep_encrypted || file.length == 0 {
            continue;
        }
        let path = resume::target(root, name)?;
        if fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == 0) {
            placeholders.push(name.clone());
        }
    }
    crate::install::state::begin_with_placeholders(root, &placeholders)
}

/// Existing output is reclaimed only after its staged replacement is complete.
/// Count growth conservatively regardless of download order; never credit hardlinks.
pub(super) fn required_space<'a>(
    root: &Path,
    pending: impl Iterator<Item = (&'a String, &'a SegmentFile)>,
    staged: &resume::Journal,
) -> io::Result<u64> {
    let mut growth = 0u64;
    let mut peak = 0u64;
    for (name, file) in pending {
        let target = resume::target(root, name)?;
        let old = match fs::metadata(target) {
            Ok(m) if m.is_file() => {
                use std::os::unix::fs::MetadataExt;
                if m.nlink() == 1 {
                    m.blocks().saturating_mul(512)
                } else {
                    0
                }
            }
            Ok(_) => return Err(io::Error::other("package target is not a regular file")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e),
        };
        let size = file.stored_length()?;
        growth = growth.saturating_add(size.saturating_sub(old));
        peak = peak.max(size.saturating_sub(staged.offset(name, file.length)));
    }
    Ok(growth.saturating_add(peak).saturating_add(64 * 1024 * 1024))
}

pub(super) fn output(stage: &Path, name: &str) -> io::Result<PathBuf> {
    let path = resume::target(stage, name)?;
    match fs::symlink_metadata(&path) {
        Ok(m) => {
            use std::os::unix::fs::MetadataExt;
            if !m.is_file() || m.nlink() != 1 {
                return Err(io::Error::other("unsafe staged output"));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => return Err(e),
    }
    Ok(path)
}

pub(super) fn commit(stage: &Path, root: &Path, name: &str) -> io::Result<()> {
    let from = output(stage, name)?;
    let to = resume::target(root, name)?;
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    if !fs::symlink_metadata(&from)?.is_file() {
        return Err(io::Error::other("staged output is not a regular file"));
    }
    fs::File::open(&from)?.sync_all()?;
    crate::install::state::begin(root)?;
    fs::rename(&from, &to)?;
    fs::File::open(to.parent().unwrap())?.sync_all()?;
    fs::File::open(from.parent().unwrap())?.sync_all()
}

pub(super) fn completed(
    journal: &resume::Journal,
    root: &Path,
    name: &str,
    file: &SegmentFile,
) -> bool {
    // Zero is also the sentinel for a missing checkpoint; empty files need a real file.
    journal.offset(name, file.length) == file.length
        && resume::target(root, name)
            .and_then(fs::metadata)
            .is_ok_and(|m| m.is_file() && m.len() == file.stored_length().unwrap_or(u64::MAX))
}

// Prefer the same position before a duplicate elsewhere. Read adjacent source
// pages together, without a large read-ahead buffer on random relocated pages.
fn local_candidates(
    old: &SegmentFile,
    file: &SegmentFile,
    index: &HashMap<[u8; 20], u64>,
    start: usize,
    pages: usize,
) -> Vec<Option<u64>> {
    (start..start + pages)
        .map(|position| {
            let hash = file.data_hashs.get(position)?;
            if old.data_hashs.get(position) == Some(hash) {
                Some(position as u64)
            } else {
                index.get(hash).copied()
            }
        })
        .collect()
}

async fn read_local_pages(
    input: &mut (impl AsyncRead + AsyncSeek + Unpin),
    data: &mut [u8],
    candidates: &[Option<u64>],
) -> Vec<bool> {
    let mut read = vec![false; candidates.len()];
    let mut n = 0;
    while n < candidates.len() {
        let begin = n;
        n += 1;
        let Some(page) = candidates[begin] else {
            continue;
        };
        while n < candidates.len() && candidates[n] == Some(page + (n - begin) as u64) {
            n += 1;
        }
        if input
            .seek(SeekFrom::Start(page * PAGE_SIZE as u64))
            .await
            .is_ok()
            && input
                .read_exact(&mut data[begin * PAGE_SIZE..n * PAGE_SIZE])
                .await
                .is_ok()
        {
            read[begin..n].fill(true);
        }
    }
    read
}

fn lazy_candidates(
    old: &SegmentFile,
    file: &SegmentFile,
    index: &mut Option<HashMap<[u8; 20], u64>>,
    start: usize,
    pages: usize,
) -> Vec<Option<u64>> {
    let relocated = (start..start + pages).any(|n| {
        file.data_hashs
            .get(n)
            .is_some_and(|hash| old.data_hashs.get(n) != Some(hash))
    });
    if relocated && index.is_none() {
        let mut entries = HashMap::new();
        for (page, hash) in old.data_hashs.iter().enumerate() {
            entries.entry(*hash).or_insert(page as u64);
        }
        *index = Some(entries);
    }
    local_candidates(
        old,
        file,
        index.as_ref().unwrap_or(&HashMap::new()),
        start,
        pages,
    )
}

const REUSE_WINDOW: usize = 8 * 1024 * 1024;

struct LocalWindow {
    start: u64,
    logical: u64,
    data: Vec<u8>,
    reused: Vec<bool>,
}

// A prefetched window is only memory, not completed progress or durable data.
// One producer is borrowed by this future, never a detached thread/task.
async fn prepare_local_window(
    source: &mut Option<tokio::fs::File>,
    old: Option<&SegmentFile>,
    file: &SegmentFile,
    verifier: &StoredPageVerifier<'_>,
    index: &mut Option<HashMap<[u8; 20], u64>>,
    start: u64,
) -> Result<Option<LocalWindow>, failure::Error> {
    if start >= file.length {
        return Ok(None);
    }
    let bytes = (file.stored_length()? - start).min(REUSE_WINDOW as u64) as usize;
    let pages = bytes.div_ceil(PAGE_SIZE);
    let mut data = vec![0u8; pages * PAGE_SIZE];
    let mut reused = vec![false; pages];
    if let (Some(input), Some(old)) = (source.as_mut(), old) {
        let first = (start / PAGE_SIZE as u64) as usize;
        let candidates =
            tokio::task::block_in_place(|| lazy_candidates(old, file, index, first, pages));
        reused = read_local_pages(input, &mut data, &candidates).await;
        tokio::task::block_in_place(|| {
            for (n, page) in data.as_chunks::<PAGE_SIZE>().0.iter().enumerate() {
                if reused[n] {
                    reused[n] = verifier.matches(first + n, page);
                }
            }
        });
    }
    Ok(Some(LocalWindow {
        start,
        logical: (bytes as u64).min(file.length - start),
        data,
        reused,
    }))
}

// A failed lookahead must still settle the current write. A failed write must
// cancel lookahead immediately, without counting or checkpointing those bytes.
async fn prefetch_while_writing<T>(
    writing: impl std::future::Future<Output = Result<(), failure::Error>>,
    preparing: impl std::future::Future<Output = Result<T, failure::Error>>,
) -> Result<T, failure::Error> {
    tokio::pin!(writing, preparing);
    tokio::select! {
        biased;
        done = &mut writing => { done?; preparing.await }
        next = &mut preparing => { writing.await?; next }
    }
}

// Coalesce tiny reused islands as before, then bound each concurrent request's
// memory. Never read ahead beyond the current8MiB reconstruction window.
fn missing_ranges(reused: &[bool]) -> Vec<std::ops::Range<usize>> {
    const MAX_PAGES: usize = 1024 * 1024 / PAGE_SIZE;
    let mut ranges = Vec::new();
    let mut n = 0;
    while n < reused.len() {
        if reused[n] {
            n += 1;
            continue;
        }
        let begin = n;
        while n < reused.len() && !reused[n] && n - begin < MAX_PAGES {
            n += 1;
        }
        while n - begin < MAX_PAGES {
            let mut end = n;
            while end < reused.len() && reused[end] {
                end += 1;
            }
            if end == reused.len() || end - n > 16 || end - begin >= MAX_PAGES {
                break;
            }
            n = end;
            while n < reused.len() && !reused[n] && n - begin < MAX_PAGES {
                n += 1;
            }
        }
        ranges.push(begin..n);
    }
    ranges
}

// Ported as Xodus has it: a transfer needs every one of these.
#[allow(clippy::too_many_arguments)]
pub(super) async fn transfer(
    xvd: &XvdFile,
    client: &reqwest::Client,
    url: &str,
    installed: &Path,
    old: Option<&SegmentFile>,
    file: &SegmentFile,
    out: &mut tokio::fs::File,
    key: [u8; 32],
    mut offset: u64,
    mut pause: watch::Receiver<bool>,
    parallel: usize,
    tx: &Sender<ProgressEvent>,
    mut checkpoint: impl FnMut(resume::SyncedCheckpoint) -> io::Result<()>,
) -> Result<(), failure::Error> {
    if offset >= file.length {
        return Ok(());
    }
    let mut source = None;
    let mut index = None;
    if old.is_some_and(|old| old.keep_encrypted == file.keep_encrypted)
        && let Ok(input) = tokio::fs::File::open(installed).await
        && input.metadata().await?.is_file()
    {
        source = Some(input);
    }
    let verifier = xvd.stored_page_verifier(file, key);
    let mut last_checkpoint = offset;
    pause.wait_for(|paused| !*paused).await.ok();
    let mut next =
        prepare_local_window(&mut source, old, file, &verifier, &mut index, offset).await?;
    while let Some(current) = next {
        if *pause.borrow() {
            checkpoint(resume::sync_output(out, offset).await?)?;
            last_checkpoint = offset;
        }
        pause.wait_for(|paused| !*paused).await.ok();
        let following = current.start + current.logical;
        let preparing =
            prepare_local_window(&mut source, old, file, &verifier, &mut index, following);
        let writing = async {
            let LocalWindow {
                start,
                logical,
                mut data,
                reused,
            } = current;
            use futures_util::{StreamExt, TryStreamExt};
            let fetched: Result<Vec<_>, failure::Error> =
                futures_util::stream::iter(missing_ranges(&reused))
                    .map(|range| {
                        let pause = pause.clone();
                        async move {
                            let base = start + (range.start * PAGE_SIZE) as u64;
                            let length = (range.len() * PAGE_SIZE) as u64;
                            let logical = length.min(file.length - base);
                            let segment = SegmentFile {
                                offset: file.offset + base,
                                length: logical,
                                keep_encrypted: file.keep_encrypted,
                                data_hashs: vec![],
                            };
                            let mut buffer = Vec::with_capacity(length as usize);
                            xvd.download_file_http(
                                client,
                                url,
                                &mut buffer,
                                &segment,
                                key,
                                |_, _| {},
                                Some(pause),
                                0,
                                |_| Ok(()), // Memory is not a durable file checkpoint.
                            )
                            .await?;
                            if buffer.len()
                                != if file.keep_encrypted { length } else { logical } as usize
                            {
                                return Err(io::Error::other("incomplete update range").into());
                            }
                            Ok((range.start * PAGE_SIZE, buffer, logical))
                        }
                    })
                    .buffer_unordered(parallel.clamp(1, 8))
                    .try_collect()
                    .await;
            let fetched = match fetched {
                Ok(fetched) => fetched,
                Err(error) => {
                    checkpoint(resume::sync_output(out, offset).await?)?;
                    return Err(error);
                }
            };
            let mut network = 0;
            for (begin, buffer, logical) in fetched {
                data[begin..begin + buffer.len()].copy_from_slice(&buffer);
                network += logical;
            }
            pause.wait_for(|paused| !*paused).await.ok();
            let physical = if file.keep_encrypted {
                data.len()
            } else {
                logical as usize
            };
            out.write_all(&data[..physical]).await?;
            tx.send(ProgressEvent::Traffic {
                network,
                reused: logical - network,
            })
            .await
            .ok();
            tx.send(ProgressEvent::Advanced { delta: logical })
                .await
                .ok();
            offset = start + logical;
            if offset - last_checkpoint >= 32 * 1024 * 1024 {
                // Data sync can overlap the one-window lookahead. Its opaque proof
                // binds the journal append to this inode and exact written prefix.
                checkpoint(resume::sync_output(out, offset).await?)?;
                last_checkpoint = offset;
            }
            Ok(())
        };
        next = match prefetch_while_writing(writing, preparing).await {
            Ok(next) => next,
            Err(error) => {
                out.flush().await?;
                return Err(error);
            }
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn streaming_update_completed_never_skips_zero_length_or_oversized_replacements() {
        let root = tempfile::tempdir().unwrap();
        let files = HashMap::from([
            (
                "zero.bin".to_owned(),
                SegmentFile {
                    offset: 0,
                    length: 0,
                    keep_encrypted: false,
                    data_hashs: vec![],
                },
            ),
            (
                "short.bin".to_owned(),
                SegmentFile {
                    offset: 4096,
                    length: 3,
                    keep_encrypted: false,
                    data_hashs: vec![],
                },
            ),
        ]);
        fs::write(root.path().join("zero.bin"), b"must be emptied").unwrap();
        fs::write(root.path().join("short.bin"), b"new trailing old bytes").unwrap();
        let mut journal = resume::Journal::open(root.path(), "fixture", &files).unwrap();
        journal.checkpoint("short.bin", 3).unwrap();
        for name in ["zero.bin", "short.bin"] {
            assert!(!completed(&journal, root.path(), name, &files[name]));
        }
        fs::write(root.path().join("zero.bin"), []).unwrap();
        fs::write(root.path().join("short.bin"), b"new").unwrap();
        for name in ["zero.bin", "short.bin"] {
            assert!(completed(&journal, root.path(), name, &files[name]));
        }
    }

    #[test]
    fn streaming_update_stages_and_commits_without_touching_old_file_until_ready() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("game.bin"), b"old valid file").unwrap();
        let stage = staging(root).unwrap();
        fs::write(stage.join("game.bin"), b"new valid file").unwrap();
        assert_eq!(fs::read(root.join("game.bin")).unwrap(), b"old valid file");
        commit(&stage, root, "game.bin").unwrap();
        assert!(crate::install::state::incomplete(root).unwrap());
        assert_eq!(fs::read(root.join("game.bin")).unwrap(), b"new valid file");
        assert!(!stage.join("game.bin").exists());
        assert!(commit(&stage, root, "../outside").is_err());
    }

    #[test]
    fn streaming_update_recovery_rechecks_old_metadata_and_preserves_optional_placeholders() {
        let root = tempfile::tempdir().unwrap();
        let file = |length| SegmentFile {
            offset: 0,
            length,
            keep_encrypted: false,
            data_hashs: vec![[7; 20]; 2],
        };
        let files = HashMap::from([
            ("game.bin".to_owned(), file(8192)),
            ("optional.ucas".to_owned(), file(8192)),
        ]);
        fs::write(root.path().join("game.bin"), b"partly replaced game").unwrap();
        fs::write(root.path().join("optional.ucas"), []).unwrap();
        assert!(
            pending(root.path(), &files, &files, false)
                .unwrap()
                .is_empty()
        );
        begin(root.path(), &files, &[]).unwrap();
        let recovered = pending(root.path(), &files, &files, true).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].0, "game.bin");
        assert_eq!(
            fs::metadata(root.path().join("optional.ucas"))
                .unwrap()
                .len(),
            0
        );
        // A prior target may have deliberately replaced a full file with an
        // empty one. A newer target reverting to old metadata must repair it.
        fs::write(root.path().join("game.bin"), []).unwrap();
        let recovered = pending(root.path(), &files, &files, true).unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].0, "game.bin");
        // A torn/old-format barrier has no trustworthy placeholder snapshot.
        fs::write(root.path().join(crate::install::state::INCOMPLETE), b"{").unwrap();
        assert_eq!(pending(root.path(), &files, &files, true).unwrap().len(), 2);
    }

    #[test]
    fn streaming_update_interrupted_multifile_commit_remains_blocked_until_recovery_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let files = HashMap::from([
            (
                "one.bin".to_owned(),
                SegmentFile {
                    offset: 0,
                    length: 3,
                    keep_encrypted: false,
                    data_hashs: vec![[7; 20]],
                },
            ),
            (
                "two.bin".to_owned(),
                SegmentFile {
                    offset: 4096,
                    length: 3,
                    keep_encrypted: false,
                    data_hashs: vec![[8; 20]],
                },
            ),
        ]);
        for name in files.keys() {
            fs::write(root.join(name), b"old").unwrap();
        }
        let final_path = root.join(".irori-xbox.msixvc");
        let cache = root.join(".irori-xbox-tmp.msixvc");
        fs::write(&final_path, b"old metadata").unwrap();
        let stage = staging(root).unwrap();
        let mut journal = resume::Journal::open(root, "new version", &files).unwrap();
        fs::write(stage.join("one.bin"), b"new").unwrap();
        commit(&stage, root, "one.bin").unwrap();
        journal.checkpoint("one.bin", 3).unwrap();
        drop(journal); // Model cancellation/restart after only the first replacement.
        assert_eq!(fs::read(root.join("one.bin")).unwrap(), b"new");
        assert_eq!(fs::read(root.join("two.bin")).unwrap(), b"old");
        assert!(crate::install::state::launch_guard(root).is_err());
        let reopened = resume::Journal::open(root, "new version", &files).unwrap();
        assert!(completed(&reopened, root, "one.bin", &files["one.bin"]));
        assert!(!completed(&reopened, root, "two.bin", &files["two.bin"]));
        fs::write(stage.join("two.bin"), b"new").unwrap();
        commit(&stage, root, "two.bin").unwrap();
        fs::write(&cache, b"new metadata").unwrap();
        crate::install::state::finish(root, &cache, &final_path).unwrap();
        assert!(crate::install::state::launch_guard(root).is_ok());
        assert_eq!(fs::read(root.join("two.bin")).unwrap(), b"new");
    }
    #[test]
    fn streaming_update_rejects_symlinked_stage_and_commit_targets() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(other.path(), dir.path().join(".irori-xbox-update")).unwrap();
        assert!(staging(dir.path()).is_err());
        fs::remove_file(dir.path().join(".irori-xbox-update")).unwrap();
        let stage = staging(dir.path()).unwrap();
        fs::write(stage.join("game.bin"), b"new").unwrap();
        std::os::unix::fs::symlink(other.path().join("save.bin"), dir.path().join("game.bin"))
            .unwrap();
        assert!(commit(&stage, dir.path(), "game.bin").is_err());
        assert!(stage.join("game.bin").exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn streaming_update_reuses_complete_ciphertext_tail_on_initial_and_resumed_updates() {
        for offset in [0, PAGE_SIZE as u64] {
            let root = tempfile::tempdir().unwrap();
            let bytes = [vec![3u8; PAGE_SIZE], vec![7u8; PAGE_SIZE]].concat();
            let hashes = bytes
                .chunks_exact(PAGE_SIZE)
                .map(|page| {
                    let digest = irori_msixvc::streaming::resume_digest(page);
                    std::array::from_fn(|i| {
                        u8::from_str_radix(&digest[i * 2..i * 2 + 2], 16).unwrap()
                    })
                })
                .collect();
            let file = SegmentFile {
                offset: 0,
                length: 5000,
                keep_encrypted: true,
                data_hashs: hashes,
            };
            let installed = root.path().join("installed.exe");
            fs::write(&installed, &bytes).unwrap();
            let mut header = vec![0u8; 0x3000];
            header[512..520].copy_from_slice(b"msft-xvd");
            header[528..536].copy_from_slice(&116444736000000000u64.to_le_bytes());
            header[652..656].copy_from_slice(&4u32.to_le_bytes());
            let xvd = XvdFile::parse(&mut std::io::Cursor::new(header))
                .await
                .unwrap();
            let target = root.path().join("staged.exe");
            let mut out = tokio::fs::File::create(&target).await.unwrap();
            out.write_all(&bytes[..offset as usize]).await.unwrap();
            let (_sender, pause) = watch::channel(false);
            let (tx, _rx) = tokio::sync::mpsc::channel(16);
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                transfer(
                    &xvd,
                    &reqwest::Client::new(),
                    "http://127.0.0.1:9/must-not-download",
                    &installed,
                    Some(&file),
                    &file,
                    &mut out,
                    [0; 32],
                    offset,
                    pause,
                    4,
                    &tx,
                    |_| Ok(()),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            out.flush().await.unwrap();
            assert_eq!(
                fs::read(&target).unwrap(),
                bytes,
                "the physical ciphertext tail must not be truncated to 5000 logical bytes"
            );
            assert_eq!(fs::read(&installed).unwrap(), bytes);
        }
    }
    #[test]
    fn streaming_update_space_uses_largest_staged_file_not_sum_of_replacements() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let stage = staging(root).unwrap();
        let files = HashMap::from([
            (
                "a".into(),
                SegmentFile {
                    offset: 0,
                    length: 8192,
                    keep_encrypted: false,
                    data_hashs: vec![],
                },
            ),
            (
                "b".into(),
                SegmentFile {
                    offset: 0,
                    length: 8192,
                    keep_encrypted: false,
                    data_hashs: vec![],
                },
            ),
        ]);
        for name in files.keys() {
            fs::write(root.join(name), vec![1; 8192]).unwrap();
        }
        let journal = resume::Journal::open(&stage, "test", &files).unwrap();
        assert_eq!(
            required_space(root, files.iter(), &journal).unwrap(),
            8192 + 64 * 1024 * 1024
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn streaming_update_reuses_relocated_pages_and_downloads_modified_or_corrupt_pages_only()
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (corrupt, initial) in [(false, 0usize), (true, 0), (false, PAGE_SIZE)] {
            let dir = tempfile::tempdir().unwrap();
            let original = [
                vec![1u8; PAGE_SIZE],
                vec![2u8; PAGE_SIZE],
                vec![3u8; PAGE_SIZE],
            ]
            .concat();
            let remote = [
                vec![2u8; PAGE_SIZE],
                vec![7u8; PAGE_SIZE],
                vec![1u8; PAGE_SIZE],
            ]
            .concat();
            let hash = |page: &[u8]| {
                let h = irori_msixvc::streaming::resume_digest(page);
                std::array::from_fn(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).unwrap())
            };
            let old = SegmentFile {
                offset: 0,
                length: original.len() as u64,
                keep_encrypted: false,
                data_hashs: original.chunks_exact(PAGE_SIZE).map(hash).collect(),
            };
            let file = SegmentFile {
                offset: 0,
                length: remote.len() as u64,
                keep_encrypted: false,
                data_hashs: remote.chunks_exact(PAGE_SIZE).map(hash).collect(),
            };
            let mut local = original.clone();
            if corrupt {
                local[PAGE_SIZE + 100] ^= 1;
            }
            let installed = dir.path().join("old.bin");
            fs::write(&installed, &local).unwrap();
            // Minimal unencrypted XVD header, no services, keys or game data.
            let mut header = vec![0u8; 0x3000];
            header[512..520].copy_from_slice(b"msft-xvd");
            header[528..536].copy_from_slice(&116444736000000000u64.to_le_bytes());
            header[652..656].copy_from_slice(&4u32.to_le_bytes());
            let xvd = XvdFile::parse(&mut std::io::Cursor::new(header))
                .await
                .unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let expected_start = if corrupt { 0 } else { PAGE_SIZE };
            let expected_end = PAGE_SIZE * 2 - 1;
            let body = remote.clone();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![];
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(socket.read_u8().await.unwrap());
                    assert!(request.len() < 8192);
                }
                assert!(
                    String::from_utf8(request)
                        .unwrap()
                        .to_lowercase()
                        .contains(&format!("range: bytes={expected_start}-{expected_end}\r\n"))
                );
                socket.write_all(format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {expected_start}-{expected_end}/{}\r\nConnection: close\r\n\r\n",expected_end-expected_start+1,body.len()).as_bytes()).await.unwrap();
                socket
                    .write_all(&body[expected_start..=expected_end])
                    .await
                    .unwrap();
            });
            let output = dir.path().join("new.bin");
            let mut out = tokio::fs::File::create(&output).await.unwrap();
            out.write_all(&remote[..initial]).await.unwrap();
            let (_control, pause) = watch::channel(false);
            let (tx, mut rx) = tokio::sync::mpsc::channel(64);
            let mut positions = vec![];
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                transfer(
                    &xvd,
                    &reqwest::Client::new(),
                    &format!("http://{address}/fixture"),
                    &installed,
                    Some(&old),
                    &file,
                    &mut out,
                    [0; 32],
                    initial as u64,
                    pause,
                    4,
                    &tx,
                    |n| {
                        positions.push(n);
                        Ok(())
                    },
                ),
            )
            .await
            .unwrap()
            .unwrap();
            out.flush().await.unwrap();
            server.await.unwrap();
            assert_eq!(fs::read(&output).unwrap(), remote);
            assert_eq!(fs::read(&installed).unwrap(), local);
            let (mut network, mut reused) = (0, 0);
            drop(tx);
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
            assert_eq!(network, (expected_end - expected_start + 1) as u64);
            assert_eq!(network + reused, (remote.len() - initial) as u64);
        }
    }
}
