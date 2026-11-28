//! Log parsing, filtering, highlighting and event-driven tailing shared by
//! the unified [`crate::log_viewer::LogViewer`], the launch-problem sheet and
//! Big Picture's log panel.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gpui_kit::SharedString;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

/// Bytes read from the end of a log file.
pub const TAIL_BYTES: u64 = 1024 * 1024;

/// Severity of a log line, for colouring and level filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LineLevel {
    Info,
    Warning,
    Error,
}

/// Level filter applied by log viewers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LevelFilter {
    All,
    Warnings,
    Errors,
}

impl LevelFilter {
    pub fn admits(self, level: LineLevel) -> bool {
        match self {
            LevelFilter::All => true,
            LevelFilter::Warnings => level >= LineLevel::Warning,
            LevelFilter::Errors => level == LineLevel::Error,
        }
    }
}

impl LineLevel {
    /// Classify a raw log line from `tracing`, Wine/Proton, DXVK/vkd3d or
    /// generic game output without false positives on words like `override`,
    /// `deferred` or `stderr`.
    pub fn of(line: &str) -> Self {
        let trimmed = line.trim_start();
        if let Some(level) = tracing_level(trimmed) {
            return level;
        }
        if let Some(level) = wine_level(trimmed) {
            return level;
        }
        if let Some(level) = dxvk_level(trimmed) {
            return level;
        }
        let upper = trimmed.to_ascii_uppercase();
        if has_word(&upper, "ERROR")
            || has_word(&upper, "FATAL")
            || has_word(&upper, "PANIC")
            || upper.contains(":ERR:")
            || upper.contains("[ERR")
        {
            LineLevel::Error
        } else if has_word(&upper, "WARN")
            || has_word(&upper, "WARNING")
            || upper.contains("FIXME:")
        {
            LineLevel::Warning
        } else {
            LineLevel::Info
        }
    }
}

/// Structured fields extracted from a single log line.
#[derive(Debug, Clone)]
pub struct ParsedLine {
    /// Zero-based line index within the loaded tail.
    pub index: usize,
    pub raw: SharedString,
    pub ts_full: Option<SharedString>,
    pub ts_short: Option<SharedString>,
    pub level: LineLevel,
    pub module: SharedString,
    pub module_short: SharedString,
    pub message: SharedString,
    /// Stable hash of `(level, module, message)` for collapsing repeated runs.
    pub dedupe_key: u64,
}

/// One row shown in the viewer, possibly collapsing consecutive duplicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayRow {
    pub entry_idx: usize,
    pub last_idx: usize,
    pub count: usize,
}

/// A loaded log tail (kept for Big Picture and launch-problem callers).
#[derive(Debug, Clone, Default)]
pub struct LogTail {
    pub lines: Vec<(LineLevel, SharedString)>,
    /// Earlier content was cut off.
    pub truncated: bool,
}

/// A loaded structured log tail for the unified viewer.
#[derive(Debug, Clone, Default)]
pub struct ParsedTail {
    pub lines: Vec<ParsedLine>,
    pub file_size: u64,
    pub truncated: bool,
}

/// Parse one raw log line into structured fields.
pub fn parse_line(index: usize, raw: &str) -> ParsedLine {
    let trimmed = raw.trim_end_matches('\r');
    if let Some((ts_full, ts_short, level, module, message)) = parse_tracing_line(trimmed) {
        return build_parsed(
            index,
            trimmed,
            Some(ts_full),
            Some(ts_short),
            level,
            module,
            message,
        );
    }
    if let Some((level, module, message)) = parse_wine_line(trimmed) {
        return build_parsed(index, trimmed, None, None, level, &module, &message);
    }
    if let Some((level, module, message)) = parse_dxvk_line(trimmed) {
        return build_parsed(index, trimmed, None, None, level, module, message);
    }
    let level = LineLevel::of(trimmed);
    build_parsed(index, trimmed, None, None, level, "output", trimmed)
}

fn build_parsed(
    index: usize,
    raw: &str,
    ts_full: Option<&str>,
    ts_short: Option<&str>,
    level: LineLevel,
    module: &str,
    message: &str,
) -> ParsedLine {
    let dedupe_key = fnv1a_hash(level, module, message);
    ParsedLine {
        index,
        raw: SharedString::from(raw.to_string()),
        ts_full: ts_full.map(|s| SharedString::from(s.to_string())),
        ts_short: ts_short.map(|s| SharedString::from(s.to_string())),
        level,
        module: SharedString::from(module.to_string()),
        module_short: SharedString::from(shorten_module(module)),
        message: SharedString::from(message.to_string()),
        dedupe_key,
    }
}

/// Shorten deeply nested Rust module paths (`gpui_linux::linux::platform` ->
/// `gpui_linux::platform`) while keeping short targets intact.
pub fn shorten_module(module: &str) -> String {
    let mut parts = module.split("::");
    let Some(first) = parts.next() else {
        return String::new();
    };
    let rest: Vec<&str> = parts.collect();
    if rest.len() <= 1 {
        return module.to_string();
    }
    match rest.last() {
        Some(last) => format!("{first}::{last}"),
        None => module.to_string(),
    }
}

fn fnv1a_hash(level: LineLevel, module: &str, message: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mix = |h: &mut u64, bytes: &[u8]| {
        for &b in bytes {
            *h ^= u64::from(b);
            *h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    mix(&mut h, &[level as u8, 0xff]);
    mix(&mut h, module.as_bytes());
    mix(&mut h, &[0xff]);
    mix(&mut h, message.as_bytes());
    h
}

/// Stable color bucket (`0..6`) for a module tag.
pub fn module_color_bucket(module: &str) -> usize {
    let mut h: u64 = 5381;
    for b in module.bytes() {
        h = h.wrapping_mul(33).wrapping_add(u64::from(b));
    }
    (h as usize) % 6
}

fn has_word(haystack: &str, needle: &str) -> bool {
    let bytes = haystack.as_bytes();
    let n = needle.len();
    let mut offset = 0;
    while let Some(pos) = haystack[offset..].find(needle) {
        let start = offset + pos;
        let end = start + n;
        let left_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let right_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if left_ok && right_ok {
            return true;
        }
        offset = start + 1;
    }
    false
}

fn tracing_level(line: &str) -> Option<LineLevel> {
    parse_tracing_line(line).map(|(_, _, level, _, _)| level)
}

fn parse_tracing_line(line: &str) -> Option<(&str, &str, LineLevel, &str, &str)> {
    // Expects RFC-3339 timestamp prefix: `YYYY-MM-DDTHH:MM:SS...Z`
    if line.len() < 24 || !line.is_char_boundary(19) {
        return None;
    }
    let b = line.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let ts_end = line.find(char::is_whitespace)?;
    let ts_full = &line[..ts_end];
    if !ts_full.ends_with('Z') && !ts_full.contains('+') {
        return None;
    }
    let ts_short = ts_full
        .get(11..23)
        .filter(|s| s.as_bytes().get(8) == Some(&b'.'))
        .or_else(|| ts_full.get(11..19))?;
    let rest = line[ts_end..].trim_start();
    let lvl_end = rest.find(char::is_whitespace)?;
    let level = match &rest[..lvl_end] {
        "ERROR" => LineLevel::Error,
        "WARN" => LineLevel::Warning,
        "INFO" | "DEBUG" | "TRACE" => LineLevel::Info,
        _ => return None,
    };
    let after_lvl = rest[lvl_end..].trim_start();
    let (module, message) = split_tracing_target(after_lvl)?;
    if module.is_empty() || module.contains(char::is_whitespace) {
        return None;
    }
    Some((ts_full, ts_short, level, module, message.trim_start()))
}

fn split_tracing_target(s: &str) -> Option<(&str, &str)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' {
            if bytes.get(i + 1) == Some(&b':') {
                i += 2;
                continue;
            }
            return Some((&s[..i], &s[i + 1..]));
        }
        if bytes[i].is_ascii_whitespace() {
            return None;
        }
        i += 1;
    }
    None
}

fn wine_level(line: &str) -> Option<LineLevel> {
    parse_wine_line(line).map(|(level, _, _)| level)
}

fn parse_wine_line(line: &str) -> Option<(LineLevel, String, String)> {
    // Format: `0124:err:module:import_dll ...` or `0124:fixme:ntdll:...`
    let (tid, rest) = line.split_once(':')?;
    if tid.len() != 4 || !tid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let (kind, rest) = rest.split_once(':')?;
    let level = if kind.eq_ignore_ascii_case("err") {
        LineLevel::Error
    } else if kind.eq_ignore_ascii_case("warn") || kind.eq_ignore_ascii_case("fixme") {
        LineLevel::Warning
    } else if kind.eq_ignore_ascii_case("trace") {
        LineLevel::Info
    } else {
        return None;
    };
    let (channel, body) = rest.split_once(':')?;
    if channel.is_empty() || channel.contains(char::is_whitespace) {
        return None;
    }
    let module = format!("wine:{channel}");
    let message = if kind.eq_ignore_ascii_case("fixme") {
        format!("fixme:{body}")
    } else {
        body.trim_start().to_string()
    };
    Some((level, module, message))
}

fn dxvk_level(line: &str) -> Option<LineLevel> {
    parse_dxvk_line(line).map(|(level, _, _)| level)
}

fn parse_dxvk_line(line: &str) -> Option<(LineLevel, &'static str, &str)> {
    let (prefix, body) = line.split_once(':')?;
    if !body.starts_with(' ') {
        return None;
    }
    let level = match prefix.to_ascii_lowercase().as_str() {
        "err" | "error" => LineLevel::Error,
        "warn" => LineLevel::Warning,
        "info" => LineLevel::Info,
        _ => return None,
    };
    let msg = body.trim_start();
    let module = if msg.to_ascii_lowercase().contains("vkd3d") {
        "vkd3d"
    } else {
        "dxvk"
    };
    Some((level, module, msg))
}

/// Decode bytes that may start mid-UTF-8 sequence, dropping a partial first
/// line when the read started mid-file.
pub fn decode_tail(bytes: &[u8], started_mid_file: bool) -> String {
    let skip = bytes
        .iter()
        .take(4)
        .take_while(|b| (**b & 0xC0) == 0x80)
        .count();
    let text = String::from_utf8_lossy(&bytes[skip..]);
    if !started_mid_file {
        return text.into_owned();
    }
    match text.find('\n') {
        Some(i) => text[i + 1..].to_string(),
        None => text.into_owned(),
    }
}

/// Split the tail of a log into lines, dropping a partial first line when
/// the read started mid-file, and keeping at most `max` lines.
pub fn parse_tail(text: &str, started_mid_file: bool, max: usize) -> LogTail {
    let mut lines: Vec<&str> = text.lines().collect();
    let mut truncated = started_mid_file;
    if started_mid_file && !lines.is_empty() {
        lines.remove(0);
    }
    if lines.len() > max {
        lines.drain(..lines.len() - max);
        truncated = true;
    }
    LogTail {
        lines: lines
            .into_iter()
            .map(|l| (LineLevel::of(l), SharedString::from(l.to_string())))
            .collect(),
        truncated,
    }
}

/// Parse the tail of a file into structured lines, keeping at most `max` lines.
pub fn parse_structured_tail(
    text: &str,
    file_size: u64,
    started_mid_file: bool,
    max: usize,
) -> ParsedTail {
    let all: Vec<&str> = text.lines().collect();
    let truncated = started_mid_file || all.len() > max;
    let start = all.len().saturating_sub(max);
    let lines = all[start..]
        .iter()
        .enumerate()
        .map(|(i, l)| parse_line(i, l))
        .collect();
    ParsedTail {
        lines,
        file_size,
        truncated,
    }
}

/// Filter structured lines and optionally collapse consecutive duplicate rows.
pub fn build_rows(
    lines: &[ParsedLine],
    level: LevelFilter,
    query: &str,
    exclude: bool,
    modules: &HashSet<SharedString>,
    collapse_dups: bool,
) -> Vec<DisplayRow> {
    let q = query.trim().to_lowercase();
    let mut out: Vec<DisplayRow> = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if !level.admits(line.level) {
            continue;
        }
        if !modules.is_empty() && !modules.contains(&line.module) {
            continue;
        }
        if !q.is_empty() && (line.raw.to_lowercase().contains(&q) == exclude) {
            continue;
        }
        if collapse_dups
            && let Some(prev) = out.last_mut()
            && lines[prev.entry_idx].dedupe_key == line.dedupe_key
        {
            prev.count += 1;
            prev.last_idx = idx;
            continue;
        }
        out.push(DisplayRow {
            entry_idx: idx,
            last_idx: idx,
            count: 1,
        });
    }
    out
}

/// Count occurrences per module, sorted descending by frequency then name.
pub fn module_counts(lines: &[ParsedLine]) -> Vec<(SharedString, usize)> {
    let mut counts: HashMap<SharedString, usize> = HashMap::new();
    for l in lines {
        *counts.entry(l.module.clone()).or_insert(0) += 1;
    }
    let mut list: Vec<_> = counts.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    list
}

/// Case-insensitive UTF-8 byte ranges in `text` matching `query` for
/// [`gpui_kit::StyledText::with_highlights`].
pub fn find_highlight_ranges(text: &str, query: &str) -> Vec<Range<usize>> {
    let q = query.trim().to_lowercase();
    if q.is_empty() || text.is_empty() {
        return Vec::new();
    }
    // Map lowercase character offsets safely only when byte lengths match
    // (ASCII case folding covers nearly all log identifiers; skip highlighting
    // safely if Unicode folding shifts byte offsets).
    let lower = text.to_lowercase();
    if lower.len() != text.len() {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut start = 0;
    while let Some(pos) = lower[start..].find(&q) {
        let from = start + pos;
        let to = from + q.len();
        if text.is_char_boundary(from) && text.is_char_boundary(to) {
            ranges.push(from..to);
        }
        start = to.max(from + 1);
    }
    ranges
}

/// Human-readable file size (`0 B`, `2.4 KB`, `1.0 MiB`).
pub fn format_file_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / KIB)
    } else {
        format!("{:.1} MiB", bytes as f64 / MIB)
    }
}

/// Indices of the lines to show: warnings and errors only when
/// `errors_only`, then those containing `query` (ignoring case), or with
/// `hide_matching` those that don't. An empty query keeps every line.
pub fn visible_lines(
    lines: &[(LineLevel, SharedString)],
    errors_only: bool,
    query: &str,
    hide_matching: bool,
) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    lines
        .iter()
        .enumerate()
        .filter(|(_, (level, _))| !errors_only || *level != LineLevel::Info)
        .filter(|(_, (_, text))| {
            query.is_empty() || text.to_lowercase().contains(&query) != hide_matching
        })
        .map(|(ix, _)| ix)
        .collect()
}

/// The last [`TAIL_BYTES`] of the log at `path`, as at most `max` lines;
/// `None` when there is no log yet. Runs on the backend runtime.
pub async fn read_tail(path: &Path, max: usize) -> std::io::Result<Option<LogTail>> {
    use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};
    let mut file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let len = file.metadata().await?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut buf).await?;
    let text = String::from_utf8_lossy(&buf);
    Ok(Some(parse_tail(&text, start > 0, max)))
}

/// Read the last [`TAIL_BYTES`] of `path` as structured lines unless `previous`
/// stamp matches. Returns `Ok(None)` when unchanged, `Ok(Some((stamp, None)))`
/// when the file does not exist yet.
pub async fn read_structured_tail(
    path: &Path,
    previous: Option<LogStamp>,
    max: usize,
) -> std::io::Result<Option<(LogStamp, Option<ParsedTail>)>> {
    use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};
    let meta = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let stamp = (0, None);
            return Ok((previous != Some(stamp)).then_some((stamp, None)));
        }
        Err(e) => return Err(e),
    };
    let len = meta.len();
    let stamp = (len, meta.modified().ok());
    if previous == Some(stamp) {
        return Ok(None);
    }
    let mut file = tokio::fs::File::open(path).await?;
    let start = len.saturating_sub(TAIL_BYTES);
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut buf).await?;
    let text = decode_tail(&buf, start > 0);
    Ok(Some((
        stamp,
        Some(parse_structured_tail(&text, len, start > 0, max)),
    )))
}

/// A log file's size and modification time, which change when the game or app
/// writes to it.
pub type LogStamp = (u64, Option<SystemTime>);

/// The stamp of the log at `path`; `None` when there is none. Runs on the
/// backend runtime.
pub async fn stamp(path: &Path) -> Option<LogStamp> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    Some((meta.len(), meta.modified().ok()))
}

/// Handle keeping a `notify` filesystem watcher alive for one log file.
/// Dropping the handle stops OS watching cleanly with zero idle CPU.
pub struct LogWatcher {
    _watcher: RecommendedWatcher,
}

/// Watch `path`'s parent directory with OS notifications and signal `tx`
/// whenever `path` is created, modified or renamed.
pub fn watch_log_file(
    path: &Path,
) -> Option<(LogWatcher, tokio::sync::mpsc::UnboundedReceiver<()>)> {
    let dir = path.parent()?.to_path_buf();
    if !dir.exists() {
        return None;
    }
    let target_name = path.file_name()?.to_os_string();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| match res {
        Ok(ev) => {
            if matches!(
                ev.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            ) && (ev.paths.is_empty()
                || ev
                    .paths
                    .iter()
                    .any(|p| p.file_name() == Some(target_name.as_os_str())))
            {
                let _ = tx.send(());
            }
        }
        Err(e) => tracing::debug!("log watcher error: {e}"),
    })
    .ok()?;
    watcher.watch(&dir, RecursiveMode::NonRecursive).ok()?;
    Some((LogWatcher { _watcher: watcher }, rx))
}

/// Quick severity tally for sidebar source badges without keeping full lines.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LogSeveritySummary {
    pub lines: usize,
    pub warnings: usize,
    pub errors: usize,
}

/// Count lines, warnings and errors in the tail of `path`.
pub fn summarize_log_file(path: &Path) -> LogSeveritySummary {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return LogSeveritySummary::default();
    };
    let Ok(meta) = file.metadata() else {
        return LogSeveritySummary::default();
    };
    let start = meta.len().saturating_sub(TAIL_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return LogSeveritySummary::default();
    }
    let mut buf = Vec::with_capacity((meta.len() - start).min(TAIL_BYTES) as usize);
    if file.read_to_end(&mut buf).is_err() {
        return LogSeveritySummary::default();
    }
    let text = decode_tail(&buf, start > 0);
    let mut out = LogSeveritySummary::default();
    for line in text.lines() {
        out.lines += 1;
        match LineLevel::of(line) {
            LineLevel::Error => out.errors += 1,
            LineLevel::Warning => out.warnings += 1,
            LineLevel::Info => {}
        }
    }
    out
}

/// Resolve the sibling `.old` rotated log file path (`irori.log` -> `irori.log.old`).
pub fn rotated_log_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_avoid_substring_false_positives() {
        assert_eq!(
            LineLevel::of("0124:err:module:load failed"),
            LineLevel::Error
        );
        assert_eq!(
            LineLevel::of("2026-11-28T11:32:01.000Z  ERROR irori: boom"),
            LineLevel::Error
        );
        assert_eq!(LineLevel::of("[ERROR] launch failed"), LineLevel::Error);
        assert_eq!(LineLevel::of("0124:fixme:ntdll:stub"), LineLevel::Warning);
        assert_eq!(LineLevel::of("WARN something slow"), LineLevel::Warning);
        // Must not false-positive on "override", "deferred", or "stderr".
        assert_eq!(
            LineLevel::of("Prefix override=d3d12 (stderr attached)"),
            LineLevel::Info
        );
        assert_eq!(
            LineLevel::of("info: VK_KHR_surface not deferred"),
            LineLevel::Info
        );
    }

    #[test]
    fn parses_tracing_wine_and_dxvk_lines() {
        let t = parse_line(
            0,
            "2026-11-28T11:32:01.386469Z  INFO irori_services::updates: Registered tools",
        );
        assert_eq!(t.level, LineLevel::Info);
        assert_eq!(t.ts_short.as_deref(), Some("11:32:01.386"));
        assert_eq!(t.module.as_ref(), "irori_services::updates");
        assert_eq!(t.message.as_ref(), "Registered tools");

        let w = parse_line(
            1,
            "0124:fixme:ntdll:NtQuerySystemInformation info_class 0x10",
        );
        assert_eq!(w.level, LineLevel::Warning);
        assert_eq!(w.module.as_ref(), "wine:ntdll");
        assert!(w.message.starts_with("fixme:"));

        let d = parse_line(2, "info:  DXVK: v2.5.3");
        assert_eq!(d.level, LineLevel::Info);
        assert_eq!(d.module.as_ref(), "dxvk");
        assert_eq!(d.message.as_ref(), "DXVK: v2.5.3");
    }

    #[test]
    fn collapses_consecutive_duplicates_and_filters_modules() {
        let raw = [
            "2026-11-28T11:32:05.596151Z  INFO irori_epic: Epic library: 659 apps",
            "2026-11-28T11:32:43.799630Z  INFO irori_epic: Epic library: 659 apps",
            "2026-11-28T11:32:44.000000Z  ERROR irori_gog: Auth failed",
        ];
        let lines: Vec<ParsedLine> = raw
            .iter()
            .enumerate()
            .map(|(i, l)| parse_line(i, l))
            .collect();
        let all = build_rows(&lines, LevelFilter::All, "", false, &HashSet::new(), true);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].count, 2);
        assert_eq!(all[1].count, 1);

        let mut mods = HashSet::new();
        mods.insert(SharedString::from("irori_gog"));
        let only_gog = build_rows(&lines, LevelFilter::All, "", false, &mods, true);
        assert_eq!(only_gog.len(), 1);
        assert_eq!(only_gog[0].entry_idx, 2);
    }

    #[test]
    fn finds_case_insensitive_highlight_ranges() {
        let ranges = find_highlight_ranges("Library VCRUNTIME140_1.dll not found", "vcruntime");
        assert_eq!(ranges, vec![8..17]);
    }

    #[test]
    fn tail_parsing() {
        let t = parse_tail("partial\na\nb\nc", true, 10);
        assert!(t.truncated);
        assert_eq!(t.lines.len(), 3);
        assert_eq!(t.lines[0].1.as_ref(), "a");
        let t = parse_tail("a\nb\nc\nd", false, 2);
        assert!(t.truncated);
        assert_eq!(
            t.lines.iter().map(|l| l.1.as_ref()).collect::<Vec<_>>(),
            vec!["c", "d"]
        );
        let t = parse_tail("a\nb", false, 10);
        assert!(!t.truncated);
    }

    #[test]
    fn filters_lines_by_level_and_text() {
        let lines = parse_tail(
            "Descriptor heap\nerr: descriptor lost\nStarted\nwarn: slow",
            false,
            10,
        )
        .lines;
        assert_eq!(visible_lines(&lines, false, "", false), vec![0, 1, 2, 3]);
        assert_eq!(
            visible_lines(&lines, false, "DESCRIPTOR", false),
            vec![0, 1]
        );
        assert_eq!(visible_lines(&lines, false, "descriptor", true), vec![2, 3]);
        assert_eq!(visible_lines(&lines, true, "", false), vec![1, 3]);
        assert_eq!(visible_lines(&lines, true, "descriptor", false), vec![1]);
        assert_eq!(visible_lines(&lines, true, "descriptor", true), vec![3]);
        assert_eq!(visible_lines(&lines, false, "  ", true), vec![0, 1, 2, 3]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reads_missing_and_present_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("game.log");
        assert!(read_tail(&path, 10).await.unwrap().is_none());
        assert!(stamp(&path).await.is_none());
        std::fs::write(&path, "one\nerr: two\n").unwrap();
        assert_eq!(stamp(&path).await.map(|s| s.0), Some(13));
        let tail = read_tail(&path, 10).await.unwrap().unwrap();
        assert_eq!(tail.lines.len(), 2);
        assert_eq!(tail.lines[1].0, LineLevel::Error);
    }
}
