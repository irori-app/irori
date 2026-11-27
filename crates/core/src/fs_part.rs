//! Filesystem helpers shared by download engines (`irori-depot`, `irori-runtime`,
//! and `irori-services`): resilient part-file finalization, safe download
//! filename sanitization, and sparse-file activation on supported volumes.
//!
//! Retry backoff for Windows rename contention (`finalize_part`) and download
//! filename sanitization (`sanitize_download_name`) are adapted from
//! `agalwood/Motrix` (`packages/finalize-fs`, MIT License; see
//! `THIRD-PARTY-NOTICES.md`).

use std::path::Path;
use std::time::Duration;

/// Retry delays (in ms) after the initial rename attempt (5 attempts total).
const RENAME_RETRY_DELAYS_MS: [u64; 4] = [50, 100, 200, 400];

/// Maximum UTF-8 byte length of a sanitized filename component so sibling
/// suffixes like `.irori-part.seg` stay safely within the 255-byte FS limit.
const MAX_FILENAME_BYTES: usize = 200;

/// Windows OS error codes raised when antivirus scanners, search indexers, or
/// transient handles briefly lock a freshly written `.irori-part` file:
/// - `5`: `ERROR_ACCESS_DENIED`
/// - `32`: `ERROR_SHARING_VIOLATION`
/// - `33`: `ERROR_LOCK_VIOLATION`
#[cfg(any(windows, test))]
fn is_retryable_windows_code(code: i32) -> bool {
    matches!(code, 5 | 32 | 33)
}

fn is_retryable_rename_error(err: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        err.raw_os_error().is_some_and(is_retryable_windows_code)
    }
    #[cfg(not(windows))]
    {
        let _ = err;
        false
    }
}

/// Sync `part`'s file data to disk and move it into `dest`, retrying briefly
/// on transient Windows sharing/AV-scanner locks (`5`, `32`, `33`).
///
/// Blocking; prefer [`finalize_part_async`] inside async tasks.
pub fn finalize_part(part: &Path, dest: &Path) -> std::io::Result<()> {
    // Windows requires write access for `FlushFileBuffers` (`sync_data`).
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(part) {
        let _ = file.sync_data();
    }
    for (attempt, delay_ms) in RENAME_RETRY_DELAYS_MS.iter().copied().enumerate() {
        match std::fs::rename(part, dest) {
            Ok(()) => return Ok(()),
            Err(e) if is_retryable_rename_error(&e) => {
                tracing::debug!(
                    "retrying rename {} -> {} after Windows lock (attempt {}): {e}",
                    part.display(),
                    dest.display(),
                    attempt + 1
                );
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
            Err(e) => return Err(e),
        }
    }
    std::fs::rename(part, dest)
}

/// Async variant of [`finalize_part`] that yields to the Tokio runtime between
/// retry backoffs instead of blocking a worker thread.
pub async fn finalize_part_async(part: &Path, dest: &Path) -> std::io::Result<()> {
    if let Ok(file) = tokio::fs::OpenOptions::new().write(true).open(part).await {
        let _ = file.sync_data().await;
    }
    for (attempt, delay_ms) in RENAME_RETRY_DELAYS_MS.iter().copied().enumerate() {
        match tokio::fs::rename(part, dest).await {
            Ok(()) => return Ok(()),
            Err(e) if is_retryable_rename_error(&e) => {
                tracing::debug!(
                    "retrying rename {} -> {} after Windows lock (attempt {}): {e}",
                    part.display(),
                    dest.display(),
                    attempt + 1
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            Err(e) => return Err(e),
        }
    }
    tokio::fs::rename(part, dest).await
}

/// Sanitize a remote download filename (from a URL path or
/// `Content-Disposition` header) into a safe, single-component filename that
/// is valid on Windows, macOS, and Linux.
///
/// Strips directory traversal, control characters, Windows-illegal characters,
/// trailing dots/spaces, and Windows reserved device names (`CON`, `PRN`,
/// `AUX`, `NUL`, `COM1`..`COM9`, `LPT1`..`LPT9`), and bounds length while
/// preserving the file extension.
pub fn sanitize_download_name(raw: &str) -> String {
    // Strip query/fragment when a full URL is passed, then take the final path segment.
    let trimmed = raw.trim().trim_matches('"').trim_matches('\'');
    let without_query = if trimmed.contains("://") {
        trimmed.split(['?', '#']).next().unwrap_or(trimmed)
    } else {
        trimmed
    };
    let base = without_query
        .rsplit(['/', '\\'])
        .find(|seg| !seg.is_empty())
        .unwrap_or("");

    let mut cleaned = String::with_capacity(base.len());
    for ch in base.chars() {
        if ch.is_control() {
            continue;
        }
        match ch {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => cleaned.push('_'),
            _ => cleaned.push(ch),
        }
    }

    // Windows silently strips trailing dots and spaces, which breaks path comparisons.
    let mut name = cleaned
        .trim_matches(|c: char| c.is_whitespace())
        .trim_end_matches(['.', ' '])
        .to_string();

    if name.is_empty() || name == "." || name == ".." || name.chars().all(|c| c == '.' || c == '_')
    {
        return "download".to_string();
    }

    if is_windows_reserved_name(&name) {
        name.insert(0, '_');
    }

    truncate_preserving_ext(name, MAX_FILENAME_BYTES)
}

fn is_windows_reserved_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let upper = stem.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn truncate_preserving_ext(name: String, max_bytes: usize) -> String {
    if name.len() <= max_bytes {
        return name;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(idx) if idx > 0 && name.len() - idx <= 24 => (&name[..idx], &name[idx..]),
        _ => (name.as_str(), ""),
    };
    let budget = max_bytes.saturating_sub(ext.len());
    let mut end = 0;
    for (idx, ch) in stem.char_indices() {
        if idx + ch.len_utf8() > budget {
            break;
        }
        end = idx + ch.len_utf8();
    }
    let truncated_stem = stem[..end].trim_end_matches(['.', ' ']);
    if truncated_stem.is_empty() {
        format!("download{ext}")
    } else {
        format!("{truncated_stem}{ext}")
    }
}

/// Mark `file` as sparse when the underlying filesystem supports sparse files.
///
/// On Windows, calls `DeviceIoControl(FSCTL_SET_SPARSE)`, returning `true` on
/// NTFS/ReFS and `false` on FAT32/exFAT so multi-range downloads can fall back
/// to a single sequential stream instead of triggering slow zero-fill stalls.
/// On Unix, returns `true` (POSIX filesystems create sparse holes on positional
/// writes without a special ioctl).
#[cfg(windows)]
pub fn set_sparse_if_supported(file: &std::fs::File) -> bool {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE;

    let handle: HANDLE = file.as_raw_handle();
    let mut bytes_returned: u32 = 0;
    // SAFETY: `handle` is borrowed from a live `std::fs::File` for the duration
    // of this synchronous call; `FSCTL_SET_SPARSE` requires no input/output
    // buffers when passed null pointers with zero lengths, `bytes_returned` is
    // a valid aligned `u32` out-pointer, and `lpOverlapped` is null.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_SET_SPARSE,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &mut bytes_returned,
            std::ptr::null_mut(),
        )
    };
    ok != 0
}

/// Mark `file` as sparse when the underlying filesystem supports sparse files.
#[cfg(not(windows))]
pub fn set_sparse_if_supported(_file: &std::fs::File) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_unsafe_and_reserved_download_names() {
        assert_eq!(sanitize_download_name("game.zip"), "game.zip");
        assert_eq!(
            sanitize_download_name("https://cdn.example/evil/path/archive.tar.gz?token=abc"),
            "archive.tar.gz"
        );
        assert_eq!(
            sanitize_download_name("../../evil/path/archive.tar.gz"),
            "archive.tar.gz"
        );
        assert_eq!(
            sanitize_download_name(r"C:\Temp\my:game*v1?.zip"),
            "my_game_v1_.zip"
        );
        assert_eq!(sanitize_download_name("trailing. . "), "trailing");
        assert_eq!(sanitize_download_name("CON"), "_CON");
        assert_eq!(sanitize_download_name("CON .txt"), "_CON .txt");
        assert_eq!(sanitize_download_name("nul.zip"), "_nul.zip");
        assert_eq!(sanitize_download_name("com1.exe"), "_com1.exe");
        assert_eq!(sanitize_download_name("lpt9"), "_lpt9");
        assert_eq!(sanitize_download_name("..."), "download");
        assert_eq!(sanitize_download_name("   "), "download");
        assert_eq!(sanitize_download_name("\x00\x1f"), "download");
        assert_eq!(
            sanitize_download_name("hello\x07world.bin"),
            "helloworld.bin"
        );
    }

    #[test]
    fn truncates_long_names_while_preserving_extension() {
        let long = format!("{}.tar.gz", "a".repeat(260));
        let sanitized = sanitize_download_name(&long);
        assert!(sanitized.len() <= MAX_FILENAME_BYTES);
        assert!(sanitized.ends_with(".gz"), "{sanitized}");
    }

    #[test]
    fn retryable_windows_codes_match_spec() {
        assert!(is_retryable_windows_code(5));
        assert!(is_retryable_windows_code(32));
        assert!(is_retryable_windows_code(33));
        assert!(!is_retryable_windows_code(2));
        assert!(!is_retryable_windows_code(112));
    }

    #[test]
    fn finalize_part_sync_and_sparse_work() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("file.bin.irori-part");
        let dest = dir.path().join("file.bin");
        let f = std::fs::File::create(&part).unwrap();
        // On NTFS (Windows CI/dev) and Unix, sparse returns true; on FAT32 it returns false.
        let _supported = set_sparse_if_supported(&f);
        drop(f);
        std::fs::write(&part, b"payload").unwrap();
        finalize_part(&part, &dest).unwrap();
        assert!(!part.exists());
        assert_eq!(std::fs::read(&dest).unwrap(), b"payload");
    }

    #[tokio::test]
    async fn finalize_part_async_replaces_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("data.pak.irori-part");
        let dest = dir.path().join("data.pak");
        tokio::fs::write(&dest, b"old").await.unwrap();
        tokio::fs::write(&part, b"new-content").await.unwrap();
        finalize_part_async(&part, &dest).await.unwrap();
        assert!(!part.exists());
        assert_eq!(tokio::fs::read(&dest).await.unwrap(), b"new-content");
    }
}
