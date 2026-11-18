//! Reading names and small files out of ISO 9660 disc images, enough to tell
//! which console a disc is for (a PlayStation 3 disc holds `PS3_DISC.SFB` at
//! its root) and to read the title file inside (`PS3_GAME/PARAM.SFO`).
//!
//! Only the primary volume descriptor and the directories on the way are
//! read, each bounded, and every offset is checked: a damaged or hostile
//! image yields nothing rather than a panic or a huge read. Blocking.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;

/// Images use 2048-byte sectors; the volume descriptors start at sector 16.
const SECTOR: u64 = 2048;
const DESCRIPTORS_AT: u64 = 16 * SECTOR;
/// Where the root directory's record sits in the primary volume descriptor.
const ROOT_RECORD_AT: usize = 156;
/// Directories bigger than this are not read (a disc's root holds a handful
/// of names).
const MAX_DIRECTORY: u32 = 256 * 1024;
/// Volume descriptors read before giving up on finding the primary one.
const MAX_DESCRIPTORS: u64 = 16;

/// One name in a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// As recorded, without the `;1` version suffix or a trailing dot.
    pub name: String,
    pub is_dir: bool,
    extent: u32,
    pub size: u32,
}

/// An opened image.
pub struct Image {
    file: File,
    block: u64,
    root: Entry,
}

impl Image {
    /// Open `path` when it is an ISO 9660 image.
    pub fn open(path: &Path) -> Option<Self> {
        let mut file = File::open(path).ok()?;
        for n in 0..MAX_DESCRIPTORS {
            let sector = read_at(&mut file, DESCRIPTORS_AT + n * SECTOR, SECTOR as usize)?;
            if sector.get(1..6)? != b"CD001" {
                return None;
            }
            match sector[0] {
                // The primary volume descriptor.
                1 => {
                    let block =
                        u64::from(u16::from_le_bytes(sector.get(128..130)?.try_into().ok()?));
                    if !matches!(block, 512 | 1024 | 2048) {
                        return None;
                    }
                    let root = record(&sector[ROOT_RECORD_AT..])?;
                    return Some(Self {
                        file,
                        block,
                        root: Entry {
                            name: String::new(),
                            is_dir: true,
                            ..root
                        },
                    });
                }
                // The set terminator: no primary descriptor.
                255 => return None,
                _ => {}
            }
        }
        None
    }

    /// The names at the root of the image.
    pub fn root(&mut self) -> Option<Vec<Entry>> {
        let root = self.root.clone();
        self.list(&root)
    }

    /// The entries of the directory `dir`.
    pub fn list(&mut self, dir: &Entry) -> Option<Vec<Entry>> {
        if !dir.is_dir || dir.size > MAX_DIRECTORY {
            return None;
        }
        let at = u64::from(dir.extent).checked_mul(self.block)?;
        let bytes = read_at(&mut self.file, at, dir.size as usize)?;
        let block = self.block as usize;
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            let len = usize::from(bytes[i]);
            if len == 0 {
                // Records never cross a block: the rest of this one is padding.
                i = (i / block + 1) * block;
                continue;
            }
            let Some(entry) = bytes.get(i..i + len).and_then(record) else {
                break;
            };
            // `\0` and `\1` are the directory itself and its parent.
            if !matches!(entry.name.as_str(), "\0" | "\u{1}") {
                out.push(entry);
            }
            i += len;
        }
        Some(out)
    }

    /// The entry at `path` (`PS3_GAME/PARAM.SFO`), names compared without
    /// letter case.
    pub fn find(&mut self, path: &str) -> Option<Entry> {
        let mut current = self.root.clone();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            current = self
                .list(&current)?
                .into_iter()
                .find(|e| e.name.eq_ignore_ascii_case(part))?;
        }
        Some(current)
    }

    /// The contents of the file `entry`, when it holds at most `max` bytes.
    pub fn read(&mut self, entry: &Entry, max: u64) -> Option<Vec<u8>> {
        if entry.is_dir || u64::from(entry.size) > max {
            return None;
        }
        let at = u64::from(entry.extent).checked_mul(self.block)?;
        read_at(&mut self.file, at, entry.size as usize)
    }
}

/// Exactly `len` bytes at `at`, or nothing.
fn read_at(file: &mut File, at: u64, len: usize) -> Option<Vec<u8>> {
    file.seek(SeekFrom::Start(at)).ok()?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// A directory record: length, extent (little-endian half of a both-endian
/// pair at 2), data length (at 10), flags (at 25), name length (at 32) and
/// the name (at 33).
fn record(bytes: &[u8]) -> Option<Entry> {
    let len = usize::from(*bytes.first()?);
    if len < 34 || bytes.len() < len {
        return None;
    }
    let extent = u32::from_le_bytes(bytes.get(2..6)?.try_into().ok()?);
    let size = u32::from_le_bytes(bytes.get(10..14)?.try_into().ok()?);
    let is_dir = bytes[25] & 0x02 != 0;
    let name_len = usize::from(bytes[32]);
    let raw = bytes.get(33..33 + name_len)?;
    if 33 + name_len > len {
        return None;
    }
    let name = String::from_utf8_lossy(raw);
    let name = name.split(';').next().unwrap_or_default();
    let name = if name.len() > 1 {
        name.trim_end_matches('.')
    } else {
        name
    };
    Some(Entry {
        name: name.to_string(),
        is_dir,
        extent,
        size,
    })
}

/// Builds small ISO 9660 images for tests: `(path, contents)` pairs, folders
/// made from the paths. One sector per directory, so keep them small.
#[cfg(any(test, feature = "test-support"))]
pub fn build(files: &[(&str, &[u8])]) -> Vec<u8> {
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Dir {
        dirs: BTreeMap<String, Dir>,
        files: BTreeMap<String, Vec<u8>>,
        extent: u32,
    }
    let mut root = Dir::default();
    for (path, data) in files {
        let mut parts: Vec<&str> = path.split('/').collect();
        let file = parts.pop().unwrap_or_default();
        let mut dir = &mut root;
        for p in parts {
            dir = dir.dirs.entry(p.to_string()).or_default();
        }
        dir.files.insert(file.to_string(), data.to_vec());
    }
    // Sectors: 16 system area, 17 primary descriptor, 18 terminator, then
    // one per directory and the files' data.
    let mut next = 19u32;
    fn place_dirs(dir: &mut Dir, next: &mut u32) {
        dir.extent = *next;
        *next += 1;
        for d in dir.dirs.values_mut() {
            place_dirs(d, next);
        }
    }
    place_dirs(&mut root, &mut next);
    let mut image = vec![0u8; next as usize * SECTOR as usize];
    fn rec(extent: u32, size: u32, dir: bool, name: &[u8]) -> Vec<u8> {
        let mut len = 33 + name.len();
        if len % 2 == 1 {
            len += 1;
        }
        let mut r = vec![0u8; len];
        r[0] = len as u8;
        r[2..6].copy_from_slice(&extent.to_le_bytes());
        r[6..10].copy_from_slice(&extent.to_be_bytes());
        r[10..14].copy_from_slice(&size.to_le_bytes());
        r[14..18].copy_from_slice(&size.to_be_bytes());
        r[25] = if dir { 2 } else { 0 };
        r[32] = name.len() as u8;
        r[33..33 + name.len()].copy_from_slice(name);
        r
    }
    fn write_dir(dir: &Dir, parent: u32, image: &mut Vec<u8>) {
        let sector = SECTOR as usize;
        let mut records = rec(dir.extent, sector as u32, true, b"\0");
        records.extend(rec(parent, sector as u32, true, b"\x01"));
        for (name, d) in &dir.dirs {
            records.extend(rec(d.extent, sector as u32, true, name.as_bytes()));
        }
        for (name, data) in &dir.files {
            let extent = (image.len() / sector) as u32;
            let mut padded = data.clone();
            padded.resize(data.len().div_ceil(sector).max(1) * sector, 0);
            image.extend(padded);
            let name = format!("{name};1");
            records.extend(rec(extent, data.len() as u32, false, name.as_bytes()));
        }
        assert!(records.len() <= sector, "test directory too big");
        let at = dir.extent as usize * sector;
        image[at..at + records.len()].copy_from_slice(&records);
        for d in dir.dirs.values() {
            write_dir(d, dir.extent, image);
        }
    }
    write_dir(&root, root.extent, &mut image);
    let pvd = 16 * SECTOR as usize;
    image[pvd] = 1;
    image[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
    image[pvd + 6] = 1;
    image[pvd + 128..pvd + 130].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    let root_rec = rec(root.extent, SECTOR as u32, true, b"\0");
    image[pvd + ROOT_RECORD_AT..pvd + ROOT_RECORD_AT + root_rec.len()].copy_from_slice(&root_rec);
    let term = 17 * SECTOR as usize;
    image[term] = 255;
    image[term + 1..term + 6].copy_from_slice(b"CD001");
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_reads_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("disc.iso");
        std::fs::write(
            &path,
            build(&[
                ("PS3_DISC.SFB", b".SFB"),
                ("PS3_GAME/PARAM.SFO", b"title file"),
                ("PS3_GAME/USRDIR/EBOOT.BIN", b"SCE"),
            ]),
        )
        .unwrap();
        let mut image = Image::open(&path).unwrap();
        let names: Vec<String> = image.root().unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["PS3_GAME", "PS3_DISC.SFB"]);
        let sfo = image.find("ps3_game/param.sfo").unwrap();
        assert_eq!(image.read(&sfo, 1024).unwrap(), b"title file");
        assert!(image.read(&sfo, 4).is_none());
        assert!(image.find("PS3_GAME/USRDIR/EBOOT.BIN").is_some());
        assert!(image.find("PS3_GAME/NOPE").is_none());
    }

    #[test]
    fn refuses_other_and_damaged_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.iso");
        std::fs::write(&path, [0u8; 64]).unwrap();
        assert!(Image::open(&path).is_none());

        let good = build(&[("PS3_DISC.SFB", b"x")]);
        // Every truncation fails cleanly.
        for len in [0, 1000, 32 * 1024, 34 * 1024, good.len() - 1] {
            std::fs::write(&path, &good[..len]).unwrap();
            if let Some(mut image) = Image::open(&path) {
                let _ = image.root();
                let _ = image.find("PS3_DISC.SFB").map(|e| image.read(&e, 10));
            }
        }
        // A root directory claiming gigabytes is not read.
        let mut huge = good.clone();
        let at = 16 * SECTOR as usize + ROOT_RECORD_AT + 10;
        huge[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(&path, &huge).unwrap();
        assert!(Image::open(&path).unwrap().root().is_none());
        // An extent past the end reads nothing.
        let mut far = good;
        let at = 16 * SECTOR as usize + ROOT_RECORD_AT + 2;
        far[at..at + 4].copy_from_slice(&0x00ff_ffffu32.to_le_bytes());
        std::fs::write(&path, &far).unwrap();
        assert!(Image::open(&path).unwrap().root().is_none());
    }
}
