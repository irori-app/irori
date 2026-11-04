//! The tail of a game's log, read for the log viewer, the launch-problem
//! sheet and Big Picture's log panel: lines with their severity.

use std::path::Path;
use std::time::SystemTime;

use gpui_kit::SharedString;

/// Bytes read from the end of the log file.
pub const TAIL_BYTES: u64 = 1024 * 1024;

/// Severity of a log line, for colouring and the errors filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineLevel {
    Error,
    Warning,
    Info,
}

impl LineLevel {
    pub fn of(line: &str) -> Self {
        let lower = line.to_ascii_lowercase();
        if lower.contains("err") || lower.contains("fatal") || lower.contains("panic") {
            LineLevel::Error
        } else if lower.contains("warn") {
            LineLevel::Warning
        } else {
            LineLevel::Info
        }
    }
}

/// A loaded log tail.
#[derive(Debug, Clone, Default)]
pub struct LogTail {
    pub lines: Vec<(LineLevel, SharedString)>,
    /// Earlier content was cut off.
    pub truncated: bool,
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

/// A log file's size and modification time, which change when the game
/// writes to it.
pub type LogStamp = (u64, Option<SystemTime>);

/// The stamp of the log at `path`; `None` when there is none. Runs on the
/// backend runtime.
pub async fn stamp(path: &Path) -> Option<LogStamp> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    Some((meta.len(), meta.modified().ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert_eq!(LineLevel::of("err:module:load failed"), LineLevel::Error);
        assert_eq!(LineLevel::of("WARN something"), LineLevel::Warning);
        assert_eq!(LineLevel::of("Started"), LineLevel::Info);
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
