//! HTTP/HTTPS probing, `Content-Disposition` filename extraction, collision
//! deduplication, and remote `.torrent` fetching for web downloads.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use futures::StreamExt as _;
use irori_core::Paths;
use irori_core::process::CancellationToken;

use super::model::{
    DownloadSource, TorrentPreview, WebDownloadId, WebDownloadKind, WebDownloadRequest,
};
use super::torrent_meta;

/// Resolved metadata derived when enqueuing a download source.
pub(crate) struct PreparedSource {
    pub kind: WebDownloadKind,
    pub source: String,
    pub stored_torrent: Option<PathBuf>,
    pub inferred_title: String,
    pub inferred_total: u64,
    pub inferred_output: Option<PathBuf>,
}

/// Inspect a classified source + optional preview bytes and persist any `.torrent`
/// copy into `state/web-downloads/` while deriving early title, size, and BT path.
pub(crate) fn prepare_enqueued_source(
    id: &WebDownloadId,
    classified: DownloadSource,
    destination: &Path,
    req: &WebDownloadRequest,
) -> Result<PreparedSource> {
    match classified {
        DownloadSource::DirectUrl(url) => {
            let inferred_title = url
                .path_segments()
                .and_then(|mut segs| segs.next_back())
                .filter(|s| !s.is_empty())
                .map(irori_core::fs_part::sanitize_download_name)
                .unwrap_or_else(|| url.host_str().unwrap_or("Download").to_string());
            Ok(PreparedSource {
                kind: WebDownloadKind::DirectUrl,
                source: url.to_string(),
                stored_torrent: None,
                inferred_title,
                inferred_total: 0,
                inferred_output: None,
            })
        }
        DownloadSource::RemoteTorrentUrl(url) => {
            let (stored_torrent, preview_name, inferred_total, inferred_output) =
                match req.torrent_bytes.as_deref() {
                    Some(bytes) => {
                        let preview = torrent_meta::parse_torrent(bytes)?;
                        let stored = persist_torrent_bytes(id, bytes)?;
                        let total = selected_bytes(&preview, &req.selected_files);
                        let out = destination.join(&preview.name);
                        (Some(stored), Some(preview.name), total, Some(out))
                    }
                    None => (None, None, 0, None),
                };
            let inferred_title = preview_name.unwrap_or_else(|| {
                url.path_segments()
                    .and_then(|mut segs| segs.next_back())
                    .filter(|s| !s.is_empty())
                    .map(irori_core::fs_part::sanitize_download_name)
                    .unwrap_or_else(|| "Torrent".to_string())
            });
            Ok(PreparedSource {
                kind: WebDownloadKind::TorrentFile,
                source: url.to_string(),
                stored_torrent,
                inferred_title,
                inferred_total,
                inferred_output,
            })
        }
        DownloadSource::LocalTorrent(path) => {
            let bytes = match req.torrent_bytes.as_ref() {
                Some(b) => b.clone(),
                None => {
                    std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?
                }
            };
            let preview = torrent_meta::parse_torrent(&bytes)?;
            let stored = persist_torrent_bytes(id, &bytes)?;
            let inferred_total = selected_bytes(&preview, &req.selected_files);
            let inferred_output = Some(destination.join(&preview.name));
            let source = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| preview.name.clone());
            Ok(PreparedSource {
                kind: WebDownloadKind::TorrentFile,
                source,
                stored_torrent: Some(stored),
                inferred_title: preview.name,
                inferred_total,
                inferred_output,
            })
        }
        DownloadSource::Magnet {
            uri,
            display_name,
            info_hash,
        } => {
            let (stored_torrent, preview_title, inferred_total, inferred_output) =
                match req.torrent_bytes.as_deref() {
                    Some(bytes) => {
                        let preview = torrent_meta::parse_torrent(bytes)?;
                        let stored = persist_torrent_bytes(id, bytes)?;
                        let total = selected_bytes(&preview, &req.selected_files);
                        let out = destination.join(&preview.name);
                        (Some(stored), Some(preview.name), total, Some(out))
                    }
                    None => (None, None, 0, None),
                };
            let inferred_title = display_name
                .or(preview_title)
                .unwrap_or_else(|| format!("Torrent {}", &info_hash[..8.min(info_hash.len())]));
            Ok(PreparedSource {
                kind: WebDownloadKind::Magnet,
                source: uri,
                stored_torrent,
                inferred_title,
                inferred_total,
                inferred_output,
            })
        }
    }
}

/// Result of probing a direct HTTP/HTTPS URL before handing it to `irori-depot`.
#[derive(Debug)]
pub(crate) enum DirectProbe {
    File {
        final_url: String,
        filename: String,
        size: u64,
    },
    RemoteTorrent {
        bytes: Vec<u8>,
    },
}

pub(crate) fn assets_dir() -> PathBuf {
    Paths::get().state_dir().join("web-downloads")
}

pub(crate) fn torrent_copy_path(id: &WebDownloadId) -> PathBuf {
    assets_dir().join(format!("{}.torrent", id.as_str()))
}

pub(crate) fn journal_path(id: &WebDownloadId) -> PathBuf {
    assets_dir().join(format!("{}.journal", id.as_str()))
}

pub(crate) fn persist_torrent_bytes(id: &WebDownloadId, bytes: &[u8]) -> Result<PathBuf> {
    let dir = assets_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = torrent_copy_path(id);
    std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

pub(crate) fn selected_bytes(preview: &TorrentPreview, selected: &[usize]) -> u64 {
    if selected.is_empty() {
        return preview.total_bytes;
    }
    preview
        .files
        .iter()
        .filter(|f| selected.contains(&f.index))
        .fold(0u64, |acc, f| acc.saturating_add(f.length))
}

pub(crate) async fn fetch_remote_torrent_bytes(
    url: &str,
    cancel: &CancellationToken,
) -> Result<Vec<u8>> {
    let resp = tokio::select! {
        biased;
        () = cancel.cancelled() => bail!("cancelled"),
        res = irori_core::http::download_client()
            .get(url)
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send() => res.context("Could not fetch .torrent file")?,
    }
    .error_for_status()
    .context("Server refused .torrent download")?;
    if let Some(len) = resp.content_length()
        && len as usize > torrent_meta::MAX_TORRENT_BYTES
    {
        bail!("Remote .torrent file exceeds maximum allowed size (16 MiB)");
    }
    let mut out = Vec::new();
    let mut stream = resp.bytes_stream();
    loop {
        let next = tokio::select! {
            biased;
            () = cancel.cancelled() => bail!("cancelled"),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = next else {
            break;
        };
        let chunk = chunk.context("Reading remote .torrent file failed")?;
        if out.len().saturating_add(chunk.len()) > torrent_meta::MAX_TORRENT_BYTES {
            bail!("Remote .torrent file exceeds maximum allowed size (16 MiB)");
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

pub(crate) async fn probe_direct_url(url: &str, cancel: &CancellationToken) -> Result<DirectProbe> {
    let client = irori_core::http::download_client();
    let resp = tokio::select! {
        biased;
        () = cancel.cancelled() => bail!("cancelled"),
        res = client
            .head(url)
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send() => res.context("Could not reach download URL")?,
    };
    let resp = if matches!(resp.status().as_u16(), 403 | 405 | 501) {
        tokio::select! {
            biased;
            () = cancel.cancelled() => bail!("cancelled"),
            res = client
                .get(url)
                .header(reqwest::header::ACCEPT_ENCODING, "identity")
                .header(reqwest::header::RANGE, "bytes=0-0")
                .send() => {
                res.context("Could not reach download URL")?
            }
        }
    } else {
        resp
    };

    if !resp.status().is_success() {
        bail!(
            "Server returned HTTP {} for download link",
            resp.status().as_u16()
        );
    }

    let final_url = resp.url().to_string();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    if content_type.starts_with("application/x-bittorrent") {
        let bytes = fetch_remote_torrent_bytes(&final_url, cancel).await?;
        return Ok(DirectProbe::RemoteTorrent { bytes });
    }

    let content_disp = resp
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .map(ToString::to_string);

    let is_attachment = content_disp
        .as_deref()
        .is_some_and(|d| d.to_ascii_lowercase().contains("attachment"));
    let path_lower = resp.url().path().to_ascii_lowercase();
    let url_is_html = path_lower.ends_with(".html") || path_lower.ends_with(".htm");
    if content_type.starts_with("text/html") && !is_attachment && !url_is_html {
        bail!(
            "This link returned a web page (HTML) instead of a direct file download. Open it in your browser to copy the direct file link."
        );
    }

    let raw_name = content_disp
        .as_deref()
        .and_then(parse_content_disposition_filename)
        .or_else(|| {
            resp.url()
                .path_segments()
                .and_then(|mut s| s.next_back())
                .filter(|s| !s.is_empty())
                .map(percent_decode_lossy)
        })
        .unwrap_or_else(|| "download".to_string());
    let filename = irori_core::fs_part::sanitize_download_name(&raw_name);
    let size = parse_total_size(&resp);

    Ok(DirectProbe::File {
        final_url,
        filename,
        size,
    })
}

fn parse_total_size(resp: &reqwest::Response) -> u64 {
    if resp.status() == reqwest::StatusCode::PARTIAL_CONTENT
        && let Some(cr) = resp
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
        && let Some((_, total)) = cr.rsplit_once('/')
        && let Ok(n) = total.trim().parse::<u64>()
    {
        return n;
    }
    resp.headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

/// Extract `filename*=` (RFC 5987) or `filename=` from a `Content-Disposition` header.
pub(crate) fn parse_content_disposition_filename(header: &str) -> Option<String> {
    let mut fallback: Option<String> = None;
    for part in header.split(';').skip(1) {
        let trimmed = part.trim();
        let Some((key, val)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let val = val.trim().trim_matches('"');
        if key == "filename*" {
            let encoded = val.rsplit_once("''").map_or(val, |(_, rest)| rest);
            let decoded = percent_decode_lossy(encoded);
            if !decoded.trim().is_empty() {
                return Some(decoded);
            }
        } else if key == "filename" && !val.trim().is_empty() {
            fallback = Some(val.to_string());
        }
    }
    fallback
}

fn percent_decode_lossy(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex_str) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(val) = u8::from_str_radix(hex_str, 16)
        {
            out.push(val);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Avoid overwriting an existing unrelated file in `dir` by appending ` (2)`, ` (3)`, …
pub(crate) fn unique_file_name(dir: &Path, filename: &str) -> String {
    if !dir.join(filename).exists() {
        return filename.to_string();
    }
    let (stem, ext) = match filename.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && e.len() <= 16 => (s, Some(e)),
        _ => (filename, None),
    };
    for n in 2..1000 {
        let candidate = match ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        if !dir.join(&candidate).exists() {
            return candidate;
        }
    }
    format!("{filename}-{}", uuid::Uuid::new_v4().simple())
}
