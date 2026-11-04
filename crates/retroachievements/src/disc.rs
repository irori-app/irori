//! Reading files off a disc image the way rcheevos does: the first data
//! track of a `.cue` sheet, a raw `.bin` (2352-byte sectors, Mode 1 or
//! Mode 2) or a plain `.iso` (2048-byte sectors), and the ISO 9660 file
//! system on it. Compressed images (CHD, CSO, PBP) are not read.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

/// Bytes of user data in a sector.
pub const SECTOR: usize = 2048;
/// The CD sync pattern a raw sector starts with.
const SYNC: [u8; 12] = [
    0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0,
];
/// Directory depth followed before a path is given up on.
const MAX_DEPTH: usize = 8;

/// The data track of a disc image.
pub struct Track {
    file: File,
    /// Bytes per sector in the file.
    stride: u64,
    /// Where a sector's user data starts.
    data_offset: u64,
}

impl Track {
    /// Open the first data track of `path`: a `.cue`, `.m3u` (its first
    /// disc), `.bin`/`.img` or `.iso`.
    pub fn open(path: &Path) -> Result<Self> {
        let ext = extension(path);
        match ext.as_str() {
            "m3u" => Self::open(&first_m3u_entry(path)?),
            "cue" => {
                let (file, mode) = first_data_track(path)?;
                Self::raw(&file, Some(mode))
            }
            "iso" => {
                let file = File::open(path)
                    .with_context(|| format!("could not open {}", path.display()))?;
                let len = file.metadata().map(|m| m.len()).unwrap_or(0);
                // Some ".iso" files are raw sector dumps.
                if len % 2352 == 0 && len % 2048 != 0 {
                    return Self::raw(path, None);
                }
                Ok(Self {
                    file,
                    stride: SECTOR as u64,
                    data_offset: 0,
                })
            }
            _ => Self::raw(path, None),
        }
    }

    /// A raw image; `mode` from the cue sheet, else read off sector 16.
    fn raw(path: &Path, mode: Option<TrackMode>) -> Result<Self> {
        let mut file =
            File::open(path).with_context(|| format!("could not open {}", path.display()))?;
        let mode = match mode {
            Some(m) => m,
            None => {
                let mut head = [0u8; 16];
                file.seek(SeekFrom::Start(16 * 2352))?;
                file.read_exact(&mut head)
                    .context("the disc image is too short")?;
                if head[..12] != SYNC {
                    bail!("the disc image is not a raw CD image");
                }
                match head[15] {
                    1 => TrackMode::Mode1Raw,
                    2 => TrackMode::Mode2Raw,
                    m => bail!("unknown CD sector mode {m}"),
                }
            }
        };
        let (stride, data_offset) = match mode {
            TrackMode::Mode1Raw => (2352, 16),
            TrackMode::Mode2Raw => (2352, 24),
            TrackMode::Cooked => (2048, 0),
        };
        Ok(Self {
            file,
            stride,
            data_offset,
        })
    }

    /// The user data of sector `lba`.
    pub fn sector(&mut self, lba: u32) -> Result<[u8; SECTOR]> {
        let mut buf = [0u8; SECTOR];
        self.file
            .seek(SeekFrom::Start(lba as u64 * self.stride + self.data_offset))?;
        self.file
            .read_exact(&mut buf)
            .with_context(|| format!("could not read sector {lba} of the disc"))?;
        Ok(buf)
    }

    /// The first `size` bytes of the file starting at sector `lba`.
    pub fn read(&mut self, lba: u32, size: usize) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(size);
        let mut lba = lba;
        while out.len() < size {
            let s = self.sector(lba)?;
            let take = (size - out.len()).min(SECTOR);
            out.extend_from_slice(&s[..take]);
            lba += 1;
        }
        Ok(out)
    }

    /// Find a file by its path on the disc (`PSP_GAME\SYSDIR\EBOOT.BIN`,
    /// either slash), ignoring case and version suffixes (`;1`). Returns
    /// its first sector and size.
    pub fn find(&mut self, path: &str) -> Result<Option<(u32, u32)>> {
        let pvd = self.sector(16)?;
        if &pvd[1..6] != b"CD001" {
            bail!("the disc has no ISO 9660 file system");
        }
        let root = &pvd[156..156 + 34];
        let (mut lba, mut size) = (le32(&root[2..]), le32(&root[10..]));
        let parts: Vec<&str> = path.split(['\\', '/']).filter(|p| !p.is_empty()).collect();
        if parts.is_empty() || parts.len() > MAX_DEPTH {
            return Ok(None);
        }
        for (i, part) in parts.iter().enumerate() {
            let last = i + 1 == parts.len();
            match self.find_in_dir(lba, size, part, !last)? {
                Some((l, s)) => (lba, size) = (l, s),
                None => return Ok(None),
            }
        }
        Ok(Some((lba, size)))
    }

    fn find_in_dir(
        &mut self,
        lba: u32,
        size: u32,
        name: &str,
        want_dir: bool,
    ) -> Result<Option<(u32, u32)>> {
        let sectors = (size as usize).div_ceil(SECTOR).min(64);
        for n in 0..sectors as u32 {
            let s = self.sector(lba + n)?;
            let mut at = 0usize;
            while at < SECTOR {
                let len = s[at] as usize;
                if len == 0 || at + len > SECTOR || len < 34 {
                    break;
                }
                let rec = &s[at..at + len];
                let name_len = rec[32] as usize;
                if 33 + name_len <= len {
                    let raw = &rec[33..33 + name_len];
                    let entry = String::from_utf8_lossy(raw);
                    let entry = entry.split(';').next().unwrap_or_default();
                    let entry = entry.strip_suffix('.').unwrap_or(entry);
                    let is_dir = rec[25] & 2 != 0;
                    if is_dir == want_dir && entry.eq_ignore_ascii_case(name) {
                        return Ok(Some((le32(&rec[2..]), le32(&rec[10..]))));
                    }
                }
                at += len;
            }
        }
        Ok(None)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrackMode {
    Mode1Raw,
    Mode2Raw,
    Cooked,
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

pub(crate) fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// The first disc an `.m3u` playlist names.
fn first_m3u_entry(path: &Path) -> Result<PathBuf> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .context("the playlist names no disc")?;
    let dir = path.parent().unwrap_or(Path::new(""));
    let entry = dir.join(line);
    if extension(&entry) == "m3u" {
        bail!("a playlist names another playlist");
    }
    Ok(entry)
}

/// The file and mode of a cue sheet's first data track.
fn first_data_track(cue: &Path) -> Result<(PathBuf, TrackMode)> {
    let text = std::fs::read_to_string(cue)
        .with_context(|| format!("could not read {}", cue.display()))?;
    let dir = cue.parent().unwrap_or(Path::new(""));
    let mut file: Option<PathBuf> = None;
    for line in text.lines().map(str::trim) {
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("FILE ") {
            let rest = &line[5..];
            let name = match rest.strip_prefix('"') {
                Some(r) => r.split('"').next().unwrap_or_default(),
                None => rest.rsplit_once(' ').map_or(rest, |(n, _)| n),
            };
            file = Some(dir.join(name));
        } else if upper.starts_with("TRACK ") {
            let mode = if upper.contains("MODE1/2352") {
                TrackMode::Mode1Raw
            } else if upper.contains("MODE2/2352") {
                TrackMode::Mode2Raw
            } else if upper.contains("MODE1/2048") {
                TrackMode::Cooked
            } else {
                continue;
            };
            let file = file.clone().context("the cue sheet names no file")?;
            return Ok((file, mode));
        }
    }
    bail!("the cue sheet has no data track")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A directory entry of a test image: name, first sector, size, folder.
    type Entry = (String, u32, u32, bool);

    /// Build a 2048-byte-sector ISO image with `files` (path, contents) in
    /// the root or one level of folders; returns its bytes.
    pub fn iso(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut sectors: Vec<[u8; SECTOR]> = vec![[0; SECTOR]; 18];
        // Folders and their entries.
        let mut dirs: Vec<(String, Vec<Entry>)> = vec![(String::new(), vec![])];
        let mut data: Vec<(u32, Vec<u8>)> = Vec::new();
        // Sectors 18.. hold directories (one each), then file data.
        let folders: Vec<String> = {
            let mut f: Vec<String> = files
                .iter()
                .filter_map(|(p, _)| p.rsplit_once('/').map(|(d, _)| d.to_string()))
                .collect();
            f.sort();
            f.dedup();
            f
        };
        for f in &folders {
            dirs.push((f.clone(), vec![]));
        }
        let mut next = 18 + dirs.len() as u32;
        for (p, contents) in files {
            let (dir, name) = p.rsplit_once('/').unwrap_or(("", p));
            let lba = next;
            next += (contents.len().div_ceil(SECTOR)).max(1) as u32;
            data.push((lba, contents.to_vec()));
            let d = dirs.iter_mut().find(|(n, _)| n == dir).unwrap();
            d.1.push((format!("{name};1"), lba, contents.len() as u32, false));
        }
        for (i, f) in folders.iter().enumerate() {
            dirs[0]
                .1
                .push((f.clone(), 19 + i as u32, SECTOR as u32, true));
        }
        let record = |name: &str, lba: u32, size: u32, dir: bool| {
            let mut r = vec![0u8; 33 + name.len() + (name.len() + 1) % 2];
            r[0] = r.len() as u8;
            r[2..6].copy_from_slice(&lba.to_le_bytes());
            r[10..14].copy_from_slice(&size.to_le_bytes());
            r[25] = if dir { 2 } else { 0 };
            r[32] = name.len() as u8;
            r[33..33 + name.len()].copy_from_slice(name.as_bytes());
            r
        };
        let mut pvd = [0u8; SECTOR];
        pvd[0] = 1;
        pvd[1..6].copy_from_slice(b"CD001");
        let root = record("\0", 18, SECTOR as u32, true);
        pvd[156..156 + 34].copy_from_slice(&root[..34]);
        sectors[16] = pvd;
        for (i, (_, entries)) in dirs.iter().enumerate() {
            let mut s = [0u8; SECTOR];
            let mut at = 0;
            for (name, lba, size, dir) in entries {
                let r = record(name, *lba, *size, *dir);
                s[at..at + r.len()].copy_from_slice(&r);
                at += r.len();
            }
            if sectors.len() <= 18 + i {
                sectors.resize(18 + i + 1, [0; SECTOR]);
            }
            sectors[18 + i] = s;
        }
        sectors.resize(next as usize, [0; SECTOR]);
        for (lba, contents) in data {
            for (n, chunk) in contents.chunks(SECTOR).enumerate() {
                sectors[lba as usize + n][..chunk.len()].copy_from_slice(chunk);
            }
        }
        sectors.concat()
    }

    /// The same image as raw Mode 2 sectors.
    pub fn raw_mode2(iso: &[u8]) -> Vec<u8> {
        iso.chunks(SECTOR)
            .flat_map(|chunk| {
                let mut s = vec![0u8; 2352];
                s[..12].copy_from_slice(&SYNC);
                s[15] = 2;
                s[24..24 + chunk.len()].copy_from_slice(chunk);
                s
            })
            .collect()
    }

    #[test]
    fn finds_files_on_iso_and_raw_images() {
        let image = iso(&[
            ("SYSTEM.CNF", b"BOOT = cdrom:\\SLUS_000.01;1\n"),
            ("DATA/A.BIN", b"abc"),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let iso_path = dir.path().join("g.iso");
        std::fs::write(&iso_path, &image).unwrap();
        let bin_path = dir.path().join("g.bin");
        std::fs::write(&bin_path, raw_mode2(&image)).unwrap();
        let cue_path = dir.path().join("g.cue");
        std::fs::write(
            &cue_path,
            "FILE \"g.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n",
        )
        .unwrap();
        let m3u = dir.path().join("g.m3u");
        std::fs::write(&m3u, "# discs\ng.cue\n").unwrap();
        for p in [&iso_path, &bin_path, &cue_path, &m3u] {
            let mut t = Track::open(p).unwrap();
            let (lba, size) = t.find("system.cnf").unwrap().unwrap();
            assert_eq!(size, 28, "{}", p.display());
            assert!(t.read(lba, size as usize).unwrap().starts_with(b"BOOT"));
            let (lba, size) = t.find("\\DATA\\A.BIN").unwrap().unwrap();
            assert_eq!(t.read(lba, size as usize).unwrap(), b"abc");
            assert!(t.find("MISSING.TXT").unwrap().is_none());
        }
    }
}
