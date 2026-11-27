//! Resume journal: tracks finished files and multi-chunk `.irori-part`
//! checkpoints across pauses and crashes, compacting superseded lines on open.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt as _;

use crate::hash::Digest;
use crate::plan::FileEntry;

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
        irori_core::fs_part::finalize_part_async(&tmp, path).await?;
        Ok::<_, std::io::Error>(())
    }
    .await;
    if let Err(e) = write_res {
        tracing::warn!("compacting download journal {}: {e}", path.display());
        let _ = tokio::fs::remove_file(&tmp).await;
    }
}
