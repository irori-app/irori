//! Durable checkpoints for the exact package layout, never CDN URLs/tokens.
use irori_msixvc::streaming::resume_digest;
use irori_msixvc::xvd::SegmentFile;
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{self, BufRead, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
};

pub(super) struct Journal {
    root: PathBuf,
    identity: String,
    entries: serde_json::Map<String, serde_json::Value>,
}

pub(super) struct Identity(String);

/// A data sync completed on this exact output inode before its journal entry.
/// Fields are private so callers cannot claim that unsynced bytes are durable.
pub(super) struct SyncedCheckpoint {
    offset: u64,
    device: u64,
    inode: u64,
}

impl SyncedCheckpoint {
    #[cfg(test)]
    pub(super) fn position(&self) -> u64 {
        self.offset
    }
}

pub(super) async fn sync_output(
    file: &mut tokio::fs::File,
    offset: u64,
) -> io::Result<SyncedCheckpoint> {
    use std::os::unix::fs::MetadataExt;
    use tokio::io::AsyncWriteExt;
    file.flush().await?;
    file.sync_data().await?;
    let meta = file.metadata().await?;
    if !meta.is_file() || meta.len() < offset {
        return Err(io::Error::other("checkpoint output is incomplete"));
    }
    Ok(SyncedCheckpoint {
        offset,
        device: meta.dev(),
        inode: meta.ino(),
    })
}

/// Preserve completed ciphertext pages, trim only uncommitted data. Return the
/// logical offset to resume from, which can rewind an old truncated final page.
pub(super) async fn prepare_output(
    file: &mut tokio::fs::File,
    mut offset: u64,
    content: &SegmentFile,
) -> io::Result<u64> {
    use tokio::io::AsyncSeekExt;
    let stored_length = content.stored_length()?;
    let page_size = irori_msixvc::layout::PAGE_SIZE as u64;
    if offset > content.length || (offset != content.length && !offset.is_multiple_of(page_size)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid download checkpoint offset",
        ));
    }
    let actual = file.metadata().await?.len();
    if actual < offset {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "checkpoint file changed during download preparation",
        ));
    }
    let physical = if offset == content.length && content.keep_encrypted {
        if actual >= stored_length {
            stored_length
        } else {
            // Older resumes trimmed off required ciphertext. Fetch the final
            // full page again; extending with zeroes would corrupt decryption.
            offset = content.length / page_size * page_size;
            offset
        }
    } else {
        offset
    };
    file.set_len(physical).await?;
    file.seek(SeekFrom::Start(physical)).await?;
    Ok(offset)
}

#[tokio::test]
async fn streaming_resume_completed_encrypted_image_preserves_ciphertext_tail() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("game.exe");
    let bytes: Vec<u8> = (0..8192).map(|n| (n % 251) as u8).collect();
    fs::write(&path, &bytes).unwrap();
    let layout = HashMap::from([(
        "game.exe".to_owned(),
        SegmentFile {
            offset: 0,
            length: 5000,
            keep_encrypted: true,
            data_hashs: vec![],
        },
    )]);
    let mut journal = Journal::open(root.path(), "synthetic-package", &layout).unwrap();
    journal.checkpoint("game.exe", 5000).unwrap();
    let logical = journal.offset("game.exe", 5000);
    assert_eq!(logical, 5000);
    let mut output = tokio::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .await
        .unwrap();
    assert_eq!(
        prepare_output(&mut output, logical, &layout["game.exe"])
            .await
            .unwrap(),
        logical
    );
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        8192,
        "completed encrypted files need the full final ciphertext page"
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[tokio::test]
async fn streaming_updated_file_is_trimmed_even_when_its_expected_length_is_zero_or_complete() {
    for offset in [0, 4] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("updated.bin");
        std::fs::write(&path, b"old-content").unwrap();
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .await
            .unwrap();
        let content = SegmentFile {
            offset: 0,
            length: offset,
            keep_encrypted: false,
            data_hashs: vec![],
        };
        assert_eq!(
            prepare_output(&mut file, offset, &content).await.unwrap(),
            offset
        );
        assert_eq!(
            std::fs::read(path).unwrap(),
            &b"old-content"[..offset as usize]
        );
    }
}

#[tokio::test]
async fn streaming_resume_ciphertext_layout_preserves_only_committed_pages_and_never_zero_fills() {
    use tokio::io::AsyncSeekExt;
    for (length, actual, checkpoint, encrypted, expected_offset, expected_len) in [
        (1, 4096, 1, true, 1, 4096),
        (4095, 4096, 4095, true, 4095, 4096),
        (4096, 4096, 4096, true, 4096, 4096),
        (4097, 8192, 4097, true, 4097, 8192),
        (5000, 9000, 5000, true, 5000, 8192),
        (5000, 5000, 5000, true, 4096, 4096),
        (5000, 7000, 5000, true, 4096, 4096),
        (1, 1, 1, true, 0, 0),
        (5000, 8192, 4096, true, 4096, 4096),
        (5000, 8192, 0, true, 0, 0),
        (5000, 8192, 5000, false, 5000, 5000),
        (0, 8192, 0, true, 0, 0),
        (0, 8192, 0, false, 0, 0),
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fixture");
        let bytes: Vec<u8> = (0..actual).map(|n| (n % 251) as u8).collect();
        fs::write(&path, &bytes).unwrap();
        let content = SegmentFile {
            offset: 0,
            length,
            keep_encrypted: encrypted,
            data_hashs: vec![],
        };
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .await
            .unwrap();
        assert_eq!(
            prepare_output(&mut file, checkpoint, &content)
                .await
                .unwrap(),
            expected_offset
        );
        assert_eq!(file.stream_position().await.unwrap(), expected_len);
        assert_eq!(fs::read(&path).unwrap(), bytes[..expected_len as usize]);
    }
}

#[tokio::test]
async fn streaming_resume_invalid_or_changed_checkpoint_does_not_modify_files() {
    for (length, offset, encrypted) in [
        (5000, 1, true),
        (5000, 9000, true),
        (u64::MAX, 0, true),
        (5000, 4096, false),
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fixture");
        let original = b"unchanged";
        fs::write(&path, original).unwrap();
        let content = SegmentFile {
            offset: 0,
            length,
            keep_encrypted: encrypted,
            data_hashs: vec![],
        };
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .await
            .unwrap();
        assert!(prepare_output(&mut file, offset, &content).await.is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}

#[tokio::test]
async fn streaming_resume_repairs_old_encrypted_tail_with_actual_range_download() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for length in [1u64, 4097, 5000, 8193] {
        let content = SegmentFile {
            offset: 0,
            length,
            keep_encrypted: true,
            data_hashs: vec![],
        };
        let stored = content.stored_length().unwrap();
        let ciphertext: Vec<u8> = (0..stored).map(|n| (n % 251) as u8).collect();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("fixture.exe");
        fs::write(&path, &ciphertext[..length as usize]).unwrap();
        let files = HashMap::from([("fixture.exe".to_owned(), content)]);
        let mut journal = Journal::open(root.path(), "synthetic", &files).unwrap();
        journal.checkpoint("fixture.exe", length).unwrap();
        let mut out = tokio::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .await
            .unwrap();
        let offset = prepare_output(
            &mut out,
            journal.offset("fixture.exe", length),
            &files["fixture.exe"],
        )
        .await
        .unwrap();
        assert_eq!(offset, length / 4096 * 4096);
        journal.checkpoint("fixture.exe", offset).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let body = ciphertext.clone();
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
                    .to_ascii_lowercase()
                    .contains(&format!("range: bytes={offset}-{}\r\n", stored - 1))
            );
            socket.write_all(format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {offset}-{}/{stored}\r\nConnection: close\r\n\r\n", stored-offset, stored-1).as_bytes()).await.unwrap();
            socket.write_all(&body[offset as usize..]).await.unwrap();
        });
        let mut header = vec![0; 4096];
        header[512..520].copy_from_slice(b"msft-xvd");
        header[528..536].copy_from_slice(&116444736000000000u64.to_le_bytes());
        header[652..656].copy_from_slice(&4u32.to_le_bytes());
        let xvd = irori_msixvc::xvd::XvdFile::parse(std::io::Cursor::new(header))
            .await
            .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            xvd.download_file_http(
                &reqwest::Client::new(),
                &format!("http://{address}/synthetic"),
                &mut out,
                &files["fixture.exe"],
                [0; 32],
                |_, _| {},
                None,
                offset,
                |pos| journal.checkpoint("fixture.exe", pos),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        server.await.unwrap();
        assert_eq!(fs::read(&path).unwrap(), ciphertext);
        assert_eq!(journal.offset("fixture.exe", length), length);
        assert_eq!(
            prepare_output(&mut out, length, &files["fixture.exe"])
                .await
                .unwrap(),
            length
        );
        assert_eq!(fs::read(&path).unwrap(), ciphertext);
    }
}

pub(super) fn lock(root: &Path) -> io::Result<File> {
    let path = target(root, ".irori-xbox.lock")?;
    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.try_lock()
        .map_err(|_| io::Error::other("another download is using this folder"))?;
    Ok(file)
}

pub(super) fn target(root: &Path, name: &str) -> io::Result<PathBuf> {
    let relative = name.replace('\\', "/");
    let relative = Path::new(&relative);
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(io::Error::other("unsafe package filename"));
    }
    let mut path = root.to_owned();
    for component in relative.components() {
        path.push(component);
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(io::Error::other("symlink in package download path"));
        }
    }
    Ok(path)
}

impl Journal {
    pub(super) fn identity(
        content: &str,
        files: &HashMap<String, SegmentFile>,
    ) -> io::Result<Identity> {
        let mut layout: Vec<_> = files
            .iter()
            .map(|(name, f)| (name, f.offset, f.length, f.keep_encrypted, &f.data_hashs))
            .collect();
        layout.sort_by(|a, b| a.0.cmp(b.0));
        let mut digest = irori_msixvc::streaming::ResumeDigest::default();
        {
            let mut writer = std::io::BufWriter::with_capacity(64 * 1024, &mut digest);
            serde_json::to_writer(&mut writer, &(content, layout))?;
            writer.flush()?;
        }
        Ok(Identity(digest.finish()))
    }

    #[cfg(test)]
    pub(super) fn open(
        root: &Path,
        content: &str,
        files: &HashMap<String, SegmentFile>,
    ) -> io::Result<Self> {
        Self::open_with_identity(root, &Self::identity(content, files)?)
    }

    pub(super) fn open_with_identity(root: &Path, identity: &Identity) -> io::Result<Self> {
        let identity = identity.0.clone();
        let path = target(root, ".irori-xbox-journal.jsonl")?;
        let mut entries = serde_json::Map::new();
        if let Ok(file) = File::open(&path) {
            // A torn final append is ignored; previously synced records survive.
            for line in std::io::BufReader::new(file.take(64 * 1024 * 1024)).split(b'\n') {
                let Ok(line) = line else {
                    break;
                };
                if line.len() > 4096 {
                    continue;
                }
                if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&line)
                    && value["schema"] == 1
                    && value["identity"].as_str() == Some(identity.as_str())
                    && let Some(name) = value["file"].as_str()
                {
                    entries.insert(name.to_owned(), value["checkpoint"].clone());
                }
            }
        }
        let journal = Self {
            root: root.to_owned(),
            identity,
            entries,
        };
        journal.compact()?;
        Ok(journal)
    }

    pub(super) fn offset(&self, name: &str, length: u64) -> u64 {
        let Some(entry) = self.entries.get(name) else {
            return 0;
        };
        let Some(offset) = entry["offset"]
            .as_u64()
            .filter(|v| *v <= length && (*v == length || *v % 4096 == 0))
        else {
            return 0;
        };
        let valid = target(&self.root, name)
            .and_then(|path| tail(&path, offset))
            .is_ok_and(|digest| entry["tailSha256"].as_str() == Some(digest.as_str()));
        if valid { offset } else { 0 }
    }

    pub(super) fn checkpoint(&mut self, name: &str, offset: u64) -> io::Result<()> {
        let file = target(&self.root, name)?;
        // File data reaches storage before the journal claims it can be resumed.
        File::open(&file)?.sync_data()?;
        self.append_checkpoint(name, &file, offset)
    }

    pub(super) fn checkpoint_synced(
        &mut self,
        name: &str,
        synced: SyncedCheckpoint,
    ) -> io::Result<()> {
        use std::os::unix::fs::MetadataExt;
        let file = target(&self.root, name)?;
        let meta = fs::metadata(&file)?;
        if !meta.is_file()
            || meta.dev() != synced.device
            || meta.ino() != synced.inode
            || meta.len() < synced.offset
        {
            return Err(io::Error::other(
                "checkpoint output changed after data sync",
            ));
        }
        self.append_checkpoint(name, &file, synced.offset)
    }

    fn append_checkpoint(&mut self, name: &str, file: &Path, offset: u64) -> io::Result<()> {
        let digest = tail(file, offset)?;
        let entry = serde_json::json!({"offset": offset, "tailSha256": digest});
        let path = target(&self.root, ".irori-xbox-journal.jsonl")?;
        let mut out = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(out, "{}", self.record(name, &entry))?;
        out.sync_all()?;
        self.entries.insert(name.to_owned(), entry);
        Ok(())
    }

    fn record(&self, name: &str, entry: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({"schema":1, "identity":self.identity, "file":name, "checkpoint":entry})
    }

    fn compact(&self) -> io::Result<()> {
        let path = self.root.join(".irori-xbox-journal.jsonl");
        let temp = self
            .root
            .join(format!(".irori-xbox-journal-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut out = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            for (name, entry) in &self.entries {
                writeln!(out, "{}", self.record(name, entry))?;
            }
            out.sync_all()?;
            fs::rename(&temp, &path)?;
            File::open(&self.root)?.sync_all()
        })();
        if result.is_err() {
            fs::remove_file(&temp).ok();
        }
        result
    }
}

fn tail(path: &Path, offset: u64) -> io::Result<String> {
    let mut file = File::open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() < offset {
        return Err(io::Error::other("checkpoint file is incomplete"));
    }
    let start = offset.saturating_sub(65536);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0; (offset - start) as usize];
    file.read_exact(&mut bytes)?;
    Ok(resume_digest(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn streaming_resume_synced_proof_binds_exact_output_and_preserves_legacy_journal() {
        use tokio::io::AsyncWriteExt;
        let root = tempfile::tempdir().unwrap();
        let files = HashMap::from([(
            "a".to_owned(),
            SegmentFile {
                offset: 0,
                length: 8192,
                keep_encrypted: false,
                data_hashs: vec![],
            },
        )]);
        let mut journal = Journal::open(root.path(), "proof", &files).unwrap();
        fs::write(root.path().join("b"), vec![19; 8192]).unwrap();
        let mut out = tokio::fs::File::create(root.path().join("a"))
            .await
            .unwrap();
        out.write_all(&vec![19; 8192]).await.unwrap();
        let wrong = sync_output(&mut out, 4096).await.unwrap();
        assert!(journal.checkpoint_synced("b", wrong).is_err());
        assert_eq!(journal.offset("b", 8192), 0);
        let proof = sync_output(&mut out, 4096).await.unwrap();
        journal.checkpoint_synced("a", proof).unwrap();
        drop(journal);
        let mut journal = Journal::open(root.path(), "proof", &files).unwrap();
        assert_eq!(journal.offset("a", 8192), 4096);
        let replaced = sync_output(&mut out, 8192).await.unwrap();
        fs::rename(root.path().join("a"), root.path().join("old-a")).unwrap();
        fs::write(root.path().join("a"), vec![19; 8192]).unwrap();
        assert!(
            journal.checkpoint_synced("a", replaced).is_err(),
            "same bytes on another inode are not a data-sync proof"
        );
        out.set_len(1024).await.unwrap();
        assert!(sync_output(&mut out, 4096).await.is_err());
    }
    fn legacy_identity(content: &str, files: &HashMap<String, SegmentFile>) -> String {
        let mut layout: Vec<_> = files
            .iter()
            .map(|(name, f)| (name, f.offset, f.length, f.keep_encrypted, &f.data_hashs))
            .collect();
        layout.sort_by(|a, b| a.0.cmp(b.0));
        resume_digest(&serde_json::to_vec(&(content, layout)).unwrap())
    }

    #[test]
    fn streaming_resume_streamed_identity_is_byte_compatible_and_binds_every_layout_field() {
        let mut files = HashMap::from([
            (
                "žluťoučký/🎮\\\"\n.bin".to_owned(),
                SegmentFile {
                    offset: u64::MAX - 100,
                    length: 5000,
                    keep_encrypted: true,
                    data_hashs: (0..256).map(|n| [n as u8; 20]).collect(),
                },
            ),
            (
                "a".to_owned(),
                SegmentFile {
                    offset: 0,
                    length: 0,
                    keep_encrypted: false,
                    data_hashs: vec![],
                },
            ),
        ]);
        let identity = Journal::identity("package\n\"", &files).unwrap().0;
        assert_eq!(identity, legacy_identity("package\n\"", &files));
        assert_eq!(
            Journal::identity("empty", &HashMap::new()).unwrap().0,
            legacy_identity("empty", &HashMap::new())
        );
        assert_ne!(Journal::identity("another", &files).unwrap().0, identity);
        for field in 0..4 {
            let file = files.get_mut("a").unwrap();
            *file = SegmentFile {
                offset: 0,
                length: 0,
                keep_encrypted: false,
                data_hashs: vec![],
            };
            match field {
                0 => file.offset = 1,
                1 => file.length = 1,
                2 => file.keep_encrypted = true,
                _ => file.data_hashs.push([7; 20]),
            }
            assert_ne!(
                Journal::identity("package\n\"", &files).unwrap().0,
                identity
            );
        }
    }

    #[test]
    fn streaming_resume_shared_identity_reopens_legacy_checkpoints_in_both_roots() {
        let roots = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
        let files = HashMap::from([(
            "a.bin".to_owned(),
            SegmentFile {
                offset: 4096,
                length: 8192,
                keep_encrypted: false,
                data_hashs: vec![[3; 20]; 2],
            },
        )]);
        let identity = Journal::identity("package", &files).unwrap();
        for root in roots {
            let path = root.path().join("a.bin");
            fs::write(&path, vec![19; 8192]).unwrap();
            let record = serde_json::json!({"schema":1,"identity":legacy_identity("package",&files),
                "file":"a.bin","checkpoint":{"offset":4096,"tailSha256":tail(&path,4096).unwrap()}});
            fs::write(
                root.path().join(".irori-xbox-journal.jsonl"),
                format!("{record}\n"),
            )
            .unwrap();
            let journal = Journal::open_with_identity(root.path(), &identity).unwrap();
            assert_eq!(journal.offset("a.bin", 8192), 4096);
            let other = Journal::identity("changed", &files).unwrap();
            assert_eq!(
                Journal::open_with_identity(root.path(), &other)
                    .unwrap()
                    .offset("a.bin", 8192),
                0
            );
        }
    }

    #[test]
    #[ignore = "synthetic identity CPU benchmark, no account or installed data"]
    fn streaming_resume_identity_benchmark() {
        let files = HashMap::from([(
            "fixture".to_owned(),
            SegmentFile {
                offset: 4096,
                length: 1638400000,
                keep_encrypted: true,
                data_hashs: (0..400000).map(|n| [n as u8; 20]).collect(),
            },
        )]);
        let start = std::time::Instant::now();
        let before = legacy_identity("fixture", &files);
        let old = start.elapsed().as_secs_f64();
        let start = std::time::Instant::now();
        let after = Journal::identity("fixture", &files).unwrap().0;
        let new = start.elapsed().as_secs_f64();
        assert_eq!(before, after);
        tracing::debug!(
            "IDENTITY_BENCH hashes=400000 legacy_seconds={old:.6} streamed_seconds={new:.6}"
        );
    }
    #[test]
    fn streaming_resume_reopens_checkpoint_and_rejects_changed_data_or_package() {
        let root = tempfile::tempdir().unwrap();
        let files = HashMap::from([(
            "game.bin".to_owned(),
            SegmentFile {
                offset: 0,
                length: 8192,
                keep_encrypted: false,
                data_hashs: vec![],
            },
        )]);
        fs::write(root.path().join("game.bin"), vec![7; 8192]).unwrap();
        let mut journal = Journal::open(root.path(), "package-A", &files).unwrap();
        journal.checkpoint("game.bin", 4096).unwrap();
        assert_eq!(
            Journal::open(root.path(), "package-A", &files)
                .unwrap()
                .offset("game.bin", 8192),
            4096
        );
        fs::write(root.path().join("game.bin"), vec![8; 8192]).unwrap();
        assert_eq!(journal.offset("game.bin", 8192), 0);
        assert_eq!(
            Journal::open(root.path(), "package-B", &files)
                .unwrap()
                .offset("game.bin", 8192),
            0
        );
        assert!(target(root.path(), "../outside").is_err());
        let _guard = lock(root.path()).unwrap();
        assert!(lock(root.path()).is_err());
    }
}
