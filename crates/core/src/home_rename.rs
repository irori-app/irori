//! Paths written under a macOS home folder that has since been renamed.
//!
//! Renaming a Mac account (System Settings → Users & Groups) moves the home
//! folder from `/Users/<old>` to `/Users/<new>`, but every saved path still
//! names `/Users/<old>`: install folders, prefixes, Wine builds, art. When
//! the app turns this on ([`enable`], only `main.rs` does, so tests never
//! rewrite their fixtures), [`crate::json_store`] reads rewrite such paths to
//! the current home as they load, and the next save keeps them.
//!
//! Only a home that no longer exists is rewritten: another user's home, the
//! shared folder and paths on other disks (`/Volumes/…`) stay as written.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Rewrite paths of a renamed home in what [`crate::json_store`] reads from
/// now on. macOS only; nothing happens elsewhere.
pub fn enable() {
    ENABLED.store(cfg!(target_os = "macos"), Ordering::Relaxed);
}

const USERS: &[u8] = b"/Users/";

/// `bytes` with every `/Users/<name>` path whose home doesn't exist
/// (`exists` says) pointed at `home` instead, or `None` when nothing
/// changes. A path counts only where a string or value starts (after `"`,
/// `'`, `=`, `:` or `file://`), and `home` must itself be a `/Users/<me>`
/// folder.
pub fn remap(bytes: &[u8], home: &Path, exists: impl Fn(&Path) -> bool) -> Option<Vec<u8>> {
    let home_text = home.to_str()?.trim_end_matches('/');
    let me = home_text.strip_prefix("/Users/")?;
    if me.is_empty() || me.contains('/') {
        return None;
    }
    let mut missing: HashMap<&[u8], bool> = HashMap::new();
    let mut out: Option<Vec<u8>> = None;
    let mut copied = 0;
    let mut at = 0;
    while let Some(found) = find(&bytes[at..], USERS) {
        let start = at + found;
        at = start + USERS.len();
        let starts_value = match start.checked_sub(1).map(|i| bytes[i]) {
            None => true,
            Some(b'"' | b'\'' | b'=' | b':') => true,
            Some(b'/') => start >= 7 && &bytes[start - 7..start] == b"file://",
            _ => false,
        };
        if !starts_value {
            continue;
        }
        let name_end = bytes[at..]
            .iter()
            .position(|b| matches!(b, b'/' | b'"' | b'\'' | b'\\' | b':' | b'\n'))
            .map_or(bytes.len(), |i| at + i);
        let name = &bytes[at..name_end];
        if name.is_empty() || name == me.as_bytes() || name == b"Shared" {
            continue;
        }
        let gone = *missing.entry(name).or_insert_with(|| {
            std::str::from_utf8(name).is_ok_and(|n| !exists(&Path::new("/Users").join(n)))
        });
        if !gone {
            continue;
        }
        let buf = out.get_or_insert_with(|| Vec::with_capacity(bytes.len()));
        buf.extend_from_slice(&bytes[copied..start]);
        buf.extend_from_slice(home_text.as_bytes());
        copied = name_end;
        at = name_end;
    }
    let mut buf = out?;
    buf.extend_from_slice(&bytes[copied..]);
    Some(buf)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// [`remap`] against this Mac's home, when [`enable`]d; `None` otherwise or
/// when nothing changes.
pub(crate) fn remap_loaded(bytes: &[u8], path: &Path) -> Option<Vec<u8>> {
    if !ENABLED.load(Ordering::Relaxed) || find(bytes, USERS).is_none() {
        return None;
    }
    let home: PathBuf = dirs::home_dir()?;
    let remapped = remap(bytes, &home, |p| p.exists())?;
    tracing::info!(
        "{}: pointed paths of a renamed home folder at {}",
        path.display(),
        home.display()
    );
    Some(remapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, existing: &[&str]) -> Option<String> {
        remap(text.as_bytes(), Path::new("/Users/new"), |p| {
            existing.iter().any(|e| p == Path::new(e))
        })
        .map(|b| String::from_utf8(b).unwrap())
    }

    #[test]
    fn points_a_renamed_home_at_the_current_one() {
        let json = r#"{"installPath":"/Users/old/Games/Hades","winePrefix":"/Users/old","art":"file:///Users/old/Pictures/a.png","env":"WINEPREFIX=/Users/old/p","list":"/usr/bin:/Users/old/bin"}"#;
        assert_eq!(
            run(json, &[]).unwrap(),
            r#"{"installPath":"/Users/new/Games/Hades","winePrefix":"/Users/new","art":"file:///Users/new/Pictures/a.png","env":"WINEPREFIX=/Users/new/p","list":"/usr/bin:/Users/new/bin"}"#
        );
    }

    #[test]
    fn leaves_other_homes_and_disks_alone() {
        // Another user's home exists; the shared folder and this home are
        // never touched; a path on another disk only looks alike.
        let json = r#"{"a":"/Users/friend/Games","b":"/Users/Shared/Games","c":"/Users/new/Games","d":"/Volumes/Disk/Users/old/Games","e":"C:\\Users\\old"}"#;
        assert_eq!(run(json, &["/Users/friend"]), None);
    }

    #[test]
    fn needs_a_home_under_users() {
        let json = br#"{"a":"/Users/old/x"}"#;
        assert_eq!(remap(json, Path::new("/home/new"), |_| false), None);
        assert_eq!(remap(json, Path::new("/Users"), |_| false), None);
        assert!(remap(json, Path::new("/Users/new/"), |_| false).is_some());
    }

    #[test]
    fn off_until_enabled() {
        assert!(!ENABLED.load(Ordering::Relaxed));
        assert_eq!(
            remap_loaded(br#"{"a":"/Users/nobody-here/x"}"#, Path::new("x.json")),
            None
        );
    }
}
