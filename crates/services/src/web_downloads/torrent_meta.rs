//! Hardened pure-Rust BitTorrent metadata parser (bencode) and download input
//! classifier.
//!
//! Input is treated as completely untrusted: byte size, nesting depth, total
//! element count, UTF-8 validity, path traversal segments, and BitTorrent v2-only
//! torrents are validated strictly before anything reaches `aria2c` or the UI.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use sha1::{Digest as _, Sha1};

use super::model::{DownloadSource, TorrentFileEntry, TorrentPreview};

/// Maximum accepted `.torrent` payload size (16 MiB).
pub const MAX_TORRENT_BYTES: usize = 16 * 1024 * 1024;
/// Maximum bencode container nesting depth.
const MAX_DEPTH: usize = 64;
/// Maximum total bencode nodes parsed from a single torrent file.
const MAX_ITEMS: usize = 200_000;

/// `(key, value, raw_value_slice)` so `info` can be SHA-1 hashed verbatim.
type DictEntry<'a> = (&'a [u8], Bencode<'a>, &'a [u8]);

/// Parsed bencode value borrowing directly from the input slice.
#[derive(Debug)]
enum Bencode<'a> {
    Int(i64),
    Bytes(&'a [u8]),
    List(Vec<Bencode<'a>>),
    Dict(Vec<DictEntry<'a>>),
}

impl<'a> Bencode<'a> {
    fn as_dict(&self) -> Option<&[DictEntry<'a>]> {
        match self {
            Self::Dict(entries) => Some(entries),
            _ => None,
        }
    }

    fn as_list(&self) -> Option<&[Bencode<'a>]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }

    fn as_bytes(&self) -> Option<&'a [u8]> {
        match self {
            Self::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }

    fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(*n),
            _ => None,
        }
    }

    fn get(&self, key: &[u8]) -> Option<&Bencode<'a>> {
        self.as_dict()?
            .iter()
            .find(|(k, _, _)| *k == key)
            .map(|(_, v, _)| v)
    }

    fn get_with_raw(&self, key: &[u8]) -> Option<(&Bencode<'a>, &'a [u8])> {
        self.as_dict()?
            .iter()
            .find(|(k, _, _)| *k == key)
            .map(|(_, v, raw)| (v, *raw))
    }
}

struct Parser<'a> {
    input: &'a [u8],
    pos: usize,
    items: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            pos: 0,
            items: 0,
        }
    }

    fn bump_items(&mut self) -> Result<()> {
        self.items = self.items.saturating_add(1);
        if self.items > MAX_ITEMS {
            bail!("Invalid torrent: metadata exceeds maximum allowed element count");
        }
        Ok(())
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn parse_value(&mut self, depth: usize) -> Result<Bencode<'a>> {
        if depth > MAX_DEPTH {
            bail!("Invalid torrent: bencode nesting depth exceeds limit ({MAX_DEPTH})");
        }
        self.bump_items()?;
        match self.peek() {
            Some(b'i') => self.parse_int(),
            Some(b'l') => self.parse_list(depth + 1),
            Some(b'd') => self.parse_dict(depth + 1),
            Some(b'0'..=b'9') => self.parse_bytes().map(Bencode::Bytes),
            Some(byte) => bail!("Invalid torrent: unexpected bencode token 0x{byte:02x}"),
            None => bail!("Invalid torrent: unexpected end of metadata"),
        }
    }

    fn parse_int(&mut self) -> Result<Bencode<'a>> {
        self.pos += 1; // consume 'i'
        let start = self.pos;
        let tail = self
            .input
            .get(start..)
            .context("Invalid torrent: truncated integer")?;
        let rel_end = tail
            .iter()
            .position(|&b| b == b'e')
            .context("Invalid torrent: unterminated integer")?;
        let raw = tail
            .get(..rel_end)
            .context("Invalid torrent: truncated integer")?;
        if raw.is_empty() {
            bail!("Invalid torrent: empty bencode integer");
        }
        if raw == b"-0"
            || (raw.len() > 1 && raw[0] == b'0')
            || (raw.len() > 2 && raw[0] == b'-' && raw[1] == b'0')
        {
            bail!("Invalid torrent: non-canonical bencode integer");
        }
        let digits = raw.strip_prefix(b"-").unwrap_or(raw);
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            bail!("Invalid torrent: malformed bencode integer");
        }
        let text = std::str::from_utf8(raw).context("Invalid torrent: integer is not ASCII")?;
        let value = text
            .parse::<i64>()
            .context("Invalid torrent: integer out of range")?;
        self.pos = start + rel_end + 1;
        Ok(Bencode::Int(value))
    }

    fn parse_bytes(&mut self) -> Result<&'a [u8]> {
        let start = self.pos;
        let tail = self
            .input
            .get(start..)
            .context("Invalid torrent: truncated byte string")?;
        let colon = tail
            .iter()
            .position(|&b| b == b':')
            .context("Invalid torrent: missing ':' in byte string")?;
        let len_raw = tail
            .get(..colon)
            .context("Invalid torrent: missing byte string length")?;
        if len_raw.is_empty()
            || (len_raw.len() > 1 && len_raw[0] == b'0')
            || !len_raw.iter().all(u8::is_ascii_digit)
        {
            bail!("Invalid torrent: invalid byte string length");
        }
        let len_text =
            std::str::from_utf8(len_raw).context("Invalid torrent: length is not ASCII")?;
        let len = len_text
            .parse::<usize>()
            .context("Invalid torrent: byte string length overflow")?;
        let data_start = start
            .checked_add(colon + 1)
            .context("Invalid torrent: byte string offset overflow")?;
        let data_end = data_start
            .checked_add(len)
            .context("Invalid torrent: byte string length exceeds input")?;
        let slice = self
            .input
            .get(data_start..data_end)
            .context("Invalid torrent: truncated byte string payload")?;
        self.pos = data_end;
        Ok(slice)
    }

    fn parse_list(&mut self, depth: usize) -> Result<Bencode<'a>> {
        self.pos += 1; // consume 'l'
        let mut items = Vec::new();
        loop {
            match self.peek() {
                Some(b'e') => {
                    self.pos += 1;
                    return Ok(Bencode::List(items));
                }
                Some(_) => items.push(self.parse_value(depth)?),
                None => bail!("Invalid torrent: unterminated bencode list"),
            }
        }
    }

    fn parse_dict(&mut self, depth: usize) -> Result<Bencode<'a>> {
        self.pos += 1; // consume 'd'
        let mut entries = Vec::new();
        loop {
            match self.peek() {
                Some(b'e') => {
                    self.pos += 1;
                    return Ok(Bencode::Dict(entries));
                }
                Some(b'0'..=b'9') => {
                    self.bump_items()?;
                    let key = self.parse_bytes()?;
                    let val_start = self.pos;
                    let value = self.parse_value(depth)?;
                    let val_end = self.pos;
                    let raw_slice = self
                        .input
                        .get(val_start..val_end)
                        .context("Invalid torrent: dict value slice out of bounds")?;
                    entries.push((key, value, raw_slice));
                }
                Some(_) => bail!("Invalid torrent: dictionary key must be a byte string"),
                None => bail!("Invalid torrent: unterminated bencode dictionary"),
            }
        }
    }
}

/// Parse a `.torrent` file from raw bytes into a sanitized [`TorrentPreview`].
pub fn parse_torrent(bytes: &[u8]) -> Result<TorrentPreview> {
    if bytes.is_empty() {
        bail!("Invalid torrent: file is empty");
    }
    if bytes.len() > MAX_TORRENT_BYTES {
        bail!(
            "Invalid torrent: metadata exceeds maximum size ({} MiB)",
            MAX_TORRENT_BYTES >> 20
        );
    }
    let mut parser = Parser::new(bytes);
    let root = parser.parse_value(0)?;
    if parser.pos != bytes.len() {
        bail!("Invalid torrent: trailing garbage after bencode root");
    }
    let (info, info_raw) = root
        .get_with_raw(b"info")
        .context("Invalid torrent: missing 'info' dictionary")?;
    if info.as_dict().is_none() {
        bail!("Invalid torrent: 'info' must be a dictionary");
    }

    // Detect BitTorrent v2-only torrents (BEP 52) cleanly. Hybrid v1+v2
    // torrents still carry v1 `pieces` and `length`/`files` and work normally.
    let meta_version = info.get(b"meta version").and_then(Bencode::as_int);
    let has_file_tree = info.get(b"file tree").is_some();
    let pieces = info.get(b"pieces").and_then(Bencode::as_bytes);
    let has_v1_layout = info.get(b"files").is_some() || info.get(b"length").is_some();
    if (meta_version == Some(2) || has_file_tree) && (pieces.is_none() || !has_v1_layout) {
        bail!(
            "BitTorrent v2-only torrents are not supported yet (this torrent has no v1 pieces metadata)"
        );
    }
    match pieces {
        Some(p) if !p.is_empty() && p.len() % 20 == 0 => {}
        _ => bail!("Invalid torrent: missing or malformed 'pieces' hash table"),
    }

    let info_hash = hex::encode(Sha1::digest(info_raw));
    let name_bytes = info
        .get(b"name.utf-8")
        .or_else(|| info.get(b"name"))
        .and_then(Bencode::as_bytes)
        .context("Invalid torrent: missing 'name'")?;
    let name = strict_utf8_component(name_bytes, "torrent name")?;

    let (files, total_bytes) = if let Some(raw_files) = info.get(b"files") {
        parse_multi_file(raw_files)?
    } else if let Some(len) = info.get(b"length").and_then(Bencode::as_int) {
        let length = non_negative_len(len, &name)?;
        (
            vec![TorrentFileEntry {
                index: 1,
                path: name.clone(),
                length,
            }],
            length,
        )
    } else {
        bail!("Invalid torrent: missing both 'files' and 'length'");
    };

    if files.is_empty() {
        bail!("Invalid torrent: contains no downloadable files");
    }

    Ok(TorrentPreview {
        name,
        info_hash,
        total_bytes,
        files,
        torrent_bytes: None,
    })
}

fn parse_multi_file(raw_files: &Bencode<'_>) -> Result<(Vec<TorrentFileEntry>, u64)> {
    let entries = raw_files
        .as_list()
        .context("Invalid torrent: 'files' must be a list")?;
    let mut files = Vec::new();
    let mut total_bytes: u64 = 0;

    for (idx, entry) in entries.iter().enumerate() {
        // aria2 `select-file` uses 1-based indices over ALL entries in `info.files`,
        // including BEP 47 `.pad` alignment files.
        let select_index = idx + 1;
        if entry.as_dict().is_none() {
            bail!("Invalid torrent: file entry #{select_index} must be a dictionary");
        }
        let raw_len = entry
            .get(b"length")
            .and_then(Bencode::as_int)
            .with_context(|| {
                format!("Invalid torrent: file entry #{select_index} missing length")
            })?;
        let length = non_negative_len(raw_len, &format!("file #{select_index}"))?;

        let raw_path = entry
            .get(b"path.utf-8")
            .or_else(|| entry.get(b"path"))
            .and_then(Bencode::as_list)
            .with_context(|| format!("Invalid torrent: file entry #{select_index} missing path"))?;
        if raw_path.is_empty() {
            bail!("Invalid torrent: file entry #{select_index} has an empty path");
        }

        let mut parts = Vec::with_capacity(raw_path.len());
        for raw_part in raw_path {
            let bytes = raw_part.as_bytes().with_context(|| {
                format!("Invalid torrent: non-string path segment in #{select_index}")
            })?;
            parts.push(strict_utf8_component(bytes, "torrent file path")?);
        }

        let attr_pad = entry
            .get(b"attr")
            .and_then(Bencode::as_bytes)
            .is_some_and(|attr| attr.contains(&b'p'));
        let path_pad = parts.iter().any(|p| p == ".pad");
        if attr_pad || path_pad {
            continue;
        }

        total_bytes = total_bytes
            .checked_add(length)
            .context("Invalid torrent: total size overflow")?;
        files.push(TorrentFileEntry {
            index: select_index,
            path: parts.join("/"),
            length,
        });
    }

    Ok((files, total_bytes))
}

fn non_negative_len(raw: i64, label: &str) -> Result<u64> {
    u64::try_from(raw)
        .map_err(|_| anyhow::anyhow!("Invalid torrent: negative file length for {label}"))
}

fn strict_utf8_component(raw: &[u8], label: &str) -> Result<String> {
    let text = std::str::from_utf8(raw)
        .with_context(|| format!("Invalid torrent: {label} is not valid UTF-8"))?;
    validate_safe_component(text, label)?;
    Ok(text.to_string())
}

fn validate_safe_component(component: &str, label: &str) -> Result<()> {
    if component.is_empty() || component == "." || component == ".." {
        bail!("Invalid torrent: unsafe {label} segment '{component}'");
    }
    if component.contains(['/', '\\', '\0']) {
        bail!("Invalid torrent: {label} contains path separator or NUL byte");
    }
    let bytes = component.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        bail!("Invalid torrent: {label} contains drive prefix '{component}'");
    }
    Ok(())
}

/// Classify user input from the unified download box into a supported source.
///
/// Strictly rejects unsafe or unsupported schemes such as `file://`, `ftp://`,
/// `javascript:`, or `data:`.
pub fn classify_source(input: &str) -> Result<DownloadSource> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        bail!("Enter a download link, magnet URI, or .torrent file");
    }

    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("magnet:") {
        return parse_magnet(trimmed);
    }

    if lower.starts_with("http://") || lower.starts_with("https://") {
        let parsed = url::Url::parse(trimmed).context("Invalid download URL")?;
        if parsed.host_str().is_none() {
            bail!("Invalid download URL: missing host");
        }
        if parsed.path().to_ascii_lowercase().ends_with(".torrent") {
            return Ok(DownloadSource::RemoteTorrentUrl(parsed));
        }
        return Ok(DownloadSource::DirectUrl(parsed));
    }

    // Reject any other URI scheme (`file://`, `ftp://`, `javascript:`, `smb://`, …)
    // while still permitting Windows drive-letter paths like `C:\path\game.torrent`.
    if has_non_drive_uri_scheme(trimmed) {
        let scheme = trimmed.split(':').next().unwrap_or("unknown");
        bail!(
            "Unsupported link scheme '{scheme}:'. Use http://, https://, magnet:, or a local .torrent file."
        );
    }

    let expanded = irori_core::paths::expand_home(trimmed);
    if is_torrent_extension(&expanded) {
        return Ok(DownloadSource::LocalTorrent(expanded));
    }

    bail!("Enter an http(s):// link, a magnet: link, or a .torrent file path")
}

fn is_torrent_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("torrent"))
}

fn has_non_drive_uri_scheme(text: &str) -> bool {
    let Some((prefix, rest)) = text.split_once(':') else {
        return false;
    };
    if prefix.is_empty() {
        return false;
    }
    // Single ASCII letter followed by `\` or `/` is a Windows drive path (`C:\...`).
    if prefix.len() == 1
        && prefix.as_bytes()[0].is_ascii_alphabetic()
        && (rest.starts_with('\\') || rest.starts_with('/'))
    {
        return false;
    }
    let mut chars = prefix.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn parse_magnet(uri: &str) -> Result<DownloadSource> {
    let parsed = url::Url::parse(uri).context("Invalid magnet URI")?;
    let mut v1_hash: Option<String> = None;
    let mut saw_v2_only = false;
    let mut display_name: Option<String> = None;

    for (key, value) in parsed.query_pairs() {
        if key == "xt" {
            let val = value.trim();
            if let Some(raw_hash) = val
                .strip_prefix("urn:btih:")
                .or_else(|| val.strip_prefix("URN:BTIH:"))
            {
                if let Some(hex_hash) = normalize_btih(raw_hash) {
                    v1_hash = Some(hex_hash);
                }
            } else if val.to_ascii_lowercase().starts_with("urn:btmh:") {
                saw_v2_only = true;
            }
        } else if key == "dn" && display_name.is_none() {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                display_name = Some(trimmed.to_string());
            }
        }
    }

    let Some(info_hash) = v1_hash else {
        if saw_v2_only {
            bail!(
                "BitTorrent v2-only magnet links (urn:btmh without urn:btih) are not supported yet"
            );
        }
        bail!("Invalid magnet link: missing valid BitTorrent v1 info hash (xt=urn:btih:...)");
    };

    Ok(DownloadSource::Magnet {
        uri: uri.to_string(),
        display_name,
        info_hash,
    })
}

/// Normalize a BitTorrent v1 info hash (40-char hex or 32-char RFC 4648 base32)
/// into 40-char lowercase hex.
fn normalize_btih(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.len() == 40 && trimmed.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Some(trimmed.to_ascii_lowercase());
    }
    if trimmed.len() == 32 {
        let decoded = decode_base32_20(trimmed)?;
        return Some(hex::encode(decoded));
    }
    None
}

fn decode_base32_20(input: &str) -> Option<[u8; 20]> {
    let mut out = [0u8; 20];
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    let mut idx = 0usize;
    for byte in input.bytes() {
        let val = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a',
            b'2'..=b'7' => byte - b'2' + 26,
            _ => return None,
        };
        buffer = (buffer << 5) | u32::from(val);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            let slot = out.get_mut(idx)?;
            *slot = ((buffer >> bits) & 0xFF) as u8;
            idx += 1;
        }
    }
    (idx == 20 && bits == 0).then_some(out)
}

/// Format 1-based aria2 `select-file` indices sorted and deduplicated.
pub fn format_select_file(indices: &[usize]) -> Option<String> {
    let mut valid: Vec<usize> = indices.iter().copied().filter(|&i| i > 0).collect();
    if valid.is_empty() {
        return None;
    }
    valid.sort_unstable();
    valid.dedup();
    Some(
        valid
            .into_iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(","),
    )
}

/// Redact credentials and query tokens from URLs or magnet trackers before logging.
pub fn redact_source(source: &str) -> String {
    if source.to_ascii_lowercase().starts_with("magnet:") {
        return match parse_magnet(source) {
            Ok(DownloadSource::Magnet { info_hash, .. }) => {
                format!("magnet:?xt=urn:btih:{info_hash}")
            }
            _ => "magnet:<redacted>".to_string(),
        };
    }
    if let Ok(url) = url::Url::parse(source)
        && matches!(url.scheme(), "http" | "https")
    {
        let host = url.host_str().unwrap_or("unknown");
        return format!("{}://{host}", url.scheme());
    }
    PathBuf::from(source)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<source>".to_string())
}
