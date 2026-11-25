//! Crash report storage, summary parsing, redaction and retention.
//!
//! Kept in `irori-core` with zero UI dependencies so the panic hook in the
//! shell, the Settings log page and unit tests share one contract.

use std::io::Read as _;
use std::path::{Path, PathBuf};

/// Crash reports kept on disk; older ones are deleted.
pub const KEPT: usize = 5;
/// Filename prefix of every crash report.
pub const PREFIX: &str = "crash-";
/// Suffix of a report the user has already been notified about.
pub const REPORTED: &str = ".reported.txt";

const HEADER_BYTES: u64 = 2048;

/// Key fields parsed from the top of a crash report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CrashSummary {
    pub version: Option<String>,
    pub os: Option<String>,
    pub thread: Option<String>,
    pub location: Option<String>,
    pub message: Option<String>,
}

/// Metadata of one saved crash report in the logs folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashReportMeta {
    pub path: PathBuf,
    pub timestamp: u64,
    pub reported: bool,
    pub summary: CrashSummary,
}

/// The current user's home directory, for redacting personal paths.
pub fn current_home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Write a new crash report file and return its path.
pub fn write_report(dir: &Path, now: u64, text: &str) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join(format!("{PREFIX}{now}.txt"));
    std::fs::write(&path, text).ok()?;
    Some(path)
}

fn is_crash_report_name(name: &str) -> bool {
    name.starts_with(PREFIX) && name.ends_with(".txt")
}

fn parse_filename_timestamp(name: &str) -> u64 {
    let rest = name.strip_prefix(PREFIX).unwrap_or(name);
    let num = rest
        .strip_suffix(REPORTED)
        .or_else(|| rest.strip_suffix(".txt"))
        .unwrap_or(rest);
    num.parse().unwrap_or(0)
}

/// Report files in `dir`, oldest first.
fn report_paths(dir: &Path) -> Vec<PathBuf> {
    let mut list: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_crash_report_name)
        })
        .collect();
    list.sort();
    list
}

/// The newest report the user has not been told about, marked as told; older
/// reports past [`KEPT`] are deleted.
pub fn take_unreported(dir: &Path) -> Option<PathBuf> {
    let all = report_paths(dir);
    for old in all.iter().take(all.len().saturating_sub(KEPT)) {
        let _ = std::fs::remove_file(old);
    }
    let mut newest = None;
    for path in report_paths(dir) {
        let name = path.file_name()?.to_str()?.to_string();
        if name.ends_with(REPORTED) {
            continue;
        }
        let told = path.with_file_name(name.replace(".txt", REPORTED));
        if std::fs::rename(&path, &told).is_ok() {
            newest = Some(told);
        }
    }
    newest
}

/// Parse the header fields (`version`, `os`, `thread`, `location`, `message`)
/// from the top of a crash report.
pub fn parse_summary(text: &str) -> CrashSummary {
    let mut summary = CrashSummary::default();
    for (idx, line) in text.lines().take(24).enumerate() {
        let line = line.trim_end();
        if idx == 0
            && let Some(ver) = line
                .strip_prefix("Irori ")
                .and_then(|s| s.strip_suffix(" crashed"))
        {
            let ver = ver.trim();
            if !ver.is_empty() {
                summary.version = Some(ver.to_string());
            }
            continue;
        }
        if line == "Backtrace:" {
            break;
        }
        if let Some(v) = line.strip_prefix("OS: ") {
            summary.os = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("Thread: ") {
            summary.thread = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("Location: ") {
            summary.location = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("Message: ") {
            summary.message = Some(v.trim().to_string());
        }
    }
    summary
}

fn read_header_summary(path: &Path) -> CrashSummary {
    let Ok(file) = std::fs::File::open(path) else {
        return CrashSummary::default();
    };
    let mut buf = Vec::new();
    if file.take(HEADER_BYTES).read_to_end(&mut buf).is_err() {
        return CrashSummary::default();
    }
    parse_summary(&String::from_utf8_lossy(&buf))
}

/// Saved crash reports in `dir`, newest first.
/// Reads only the small header at the top of each file so listing stays fast.
pub fn list_reports(dir: &Path) -> Vec<CrashReportMeta> {
    let mut out: Vec<CrashReportMeta> = report_paths(dir)
        .into_iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?;
            Some(CrashReportMeta {
                timestamp: parse_filename_timestamp(name),
                reported: name.ends_with(REPORTED),
                summary: read_header_summary(&path),
                path,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.timestamp
            .cmp(&a.timestamp)
            .then_with(|| b.path.cmp(&a.path))
    });
    out
}

/// Replace occurrences of `home` in `text` with `~` (Unix) or `%USERPROFILE%`
/// (Windows) so shared crash reports do not leak the user's account name.
pub fn redact_home_paths(text: &str, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return text.to_string();
    };
    let raw = home.to_string_lossy();
    let trimmed = raw.trim_end_matches(['/', '\\']);
    let is_windows_root = trimmed.len() == 2 && trimmed.as_bytes().get(1) == Some(&b':');
    if trimmed.is_empty() || is_windows_root {
        return text.to_string();
    }
    let is_windows = trimmed.contains('\\') || trimmed.as_bytes().get(1) == Some(&b':');
    let replacement = if is_windows { "%USERPROFILE%" } else { "~" };
    let mut redacted = text.replace(trimmed, replacement);
    if is_windows && trimmed.contains('\\') {
        let forward = trimmed.replace('\\', "/");
        redacted = redacted.replace(&forward, replacement);
    }
    redacted
}

/// Delete a saved crash report, verifying that it sits directly inside
/// `logs_dir` and has a valid `crash-*.txt` filename.
pub fn delete_report(logs_dir: &Path, path: &Path) -> std::io::Result<()> {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid crash report filename",
        ));
    };
    if !is_crash_report_name(name)
        || name.contains('/')
        || name.contains('\\')
        || path != logs_dir.join(name)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path is not a crash report inside logs_dir",
        ));
    }
    std::fs::remove_file(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_about_the_newest_crash_once_and_keeps_a_few() {
        let dir = tempfile::tempdir().unwrap();
        assert!(take_unreported(dir.path()).is_none());
        for t in 1..=7 {
            write_report(dir.path(), 1_000 + t, "x").unwrap();
        }
        let told = take_unreported(dir.path()).unwrap();
        assert!(told.ends_with(format!("crash-1007{REPORTED}")));
        assert!(take_unreported(dir.path()).is_none(), "only once");
        assert_eq!(report_paths(dir.path()).len(), KEPT);
    }

    #[test]
    fn lists_reports_newest_first_with_summary() {
        let dir = tempfile::tempdir().unwrap();
        write_report(
            dir.path(),
            100,
            "Irori 1.2.0 crashed\nOS: linux x86_64\nThread: main\nLocation: a.rs:1:2\nMessage: first\n\nBacktrace:\n...",
        )
        .unwrap();
        write_report(
            dir.path(),
            200,
            "Irori 1.3.0 crashed\nOS: linux x86_64\nThread: worker\nLocation: b.rs:9:4\nMessage: second\n\nBacktrace:\n...",
        )
        .unwrap();
        take_unreported(dir.path());

        let list = list_reports(dir.path());
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].timestamp, 200);
        assert!(list[0].reported);
        assert_eq!(list[0].summary.version.as_deref(), Some("1.3.0"));
        assert_eq!(list[0].summary.location.as_deref(), Some("b.rs:9:4"));
        assert_eq!(list[0].summary.message.as_deref(), Some("second"));
        assert_eq!(list[1].timestamp, 100);
    }

    #[test]
    fn redacts_unix_and_windows_home_paths() {
        let unix_sample =
            "Location: /home/alice/src/irori/main.rs:10\nPath: /home/alice/.local/share/irori/logs";
        assert_eq!(
            redact_home_paths(unix_sample, Some(Path::new("/home/alice/"))),
            "Location: ~/src/irori/main.rs:10\nPath: ~/.local/share/irori/logs"
        );

        let win_sample =
            "File: C:\\Users\\alice\\AppData\\Local\\irori\\log.txt and C:/Users/alice/work";
        assert_eq!(
            redact_home_paths(win_sample, Some(Path::new("C:\\Users\\alice"))),
            "File: %USERPROFILE%\\AppData\\Local\\irori\\log.txt and %USERPROFILE%/work"
        );

        assert_eq!(
            redact_home_paths("/usr/bin/irori", Some(Path::new("/"))),
            "/usr/bin/irori"
        );
    }

    #[test]
    fn delete_report_rejects_unrelated_or_escaped_paths() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let valid = write_report(dir.path(), 500, "boom").unwrap();
        let wrong_name = dir.path().join("irori.log");
        std::fs::write(&wrong_name, "keep").unwrap();
        let outside = other.path().join("crash-999.txt");
        std::fs::write(&outside, "keep").unwrap();

        assert!(delete_report(dir.path(), &wrong_name).is_err());
        assert!(wrong_name.exists());
        assert!(delete_report(dir.path(), &outside).is_err());
        assert!(outside.exists());

        delete_report(dir.path(), &valid).unwrap();
        assert!(!valid.exists());
    }
}
