//! The icon a Windows program carries: its first icon group
//! (`RT_GROUP_ICON`) and, of that group, the largest picture (`RT_ICON`),
//! read from the program's resources as an `.ico` file. Only the headers,
//! the resource tree and the chosen picture are read, each bounded, so a
//! game's multi-gigabyte program or a malformed one costs little and never
//! fails worse than "no icon".

#![forbid(unsafe_code)]

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// `RT_ICON` and `RT_GROUP_ICON`.
const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;
/// Bounds on what one program may make Irori read.
const MAX_SECTIONS: usize = 96;
const MAX_ENTRIES: usize = 4096;
const MAX_ICON_BYTES: u32 = 4 * 1024 * 1024;
const MAX_GROUP_BYTES: u32 = 64 * 1024;

/// The program's largest icon as `.ico` bytes (one picture, PNG or bitmap
/// as the program holds it); `None` when it has none or can't be read.
pub fn icon_of(path: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut reader = Bounded {
        inner: &mut file,
        len,
    };
    read_icon(&mut reader)
}

/// Reads ranges of a file that are inside it.
struct Bounded<'a, R> {
    inner: &'a mut R,
    len: u64,
}

impl<R: Read + Seek> Bounded<'_, R> {
    fn bytes(&mut self, offset: u64, len: usize) -> Option<Vec<u8>> {
        if offset.checked_add(len as u64)? > self.len {
            return None;
        }
        self.inner.seek(SeekFrom::Start(offset)).ok()?;
        let mut buf = vec![0; len];
        self.inner.read_exact(&mut buf).ok()?;
        Some(buf)
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// One section: where its RVAs are in the file.
struct Section {
    rva: u32,
    size: u32,
    raw: u32,
    raw_size: u32,
}

struct Image {
    sections: Vec<Section>,
    /// RVA of the resource directory.
    resources: u32,
}

impl Image {
    /// The file offset of `rva`, with `len` bytes available from there.
    fn offset(&self, rva: u32, len: u32) -> Option<u64> {
        let s = self.sections.iter().find(|s| {
            rva >= s.rva && u64::from(rva) < u64::from(s.rva) + u64::from(s.size.max(s.raw_size))
        })?;
        let within = rva - s.rva;
        (u64::from(within) + u64::from(len) <= u64::from(s.raw_size))
            .then(|| u64::from(s.raw) + u64::from(within))
    }
}

fn read_image<R: Read + Seek>(r: &mut Bounded<'_, R>) -> Option<Image> {
    let dos = r.bytes(0, 64)?;
    if &dos[..2] != b"MZ" {
        return None;
    }
    let pe = u64::from(u32_at(&dos, 0x3c)?);
    let head = r.bytes(pe, 24)?;
    if &head[..4] != b"PE\0\0" {
        return None;
    }
    let sections = usize::from(u16_at(&head, 6)?);
    let optional_size = usize::from(u16_at(&head, 20)?);
    if sections > MAX_SECTIONS || optional_size > 4096 {
        return None;
    }
    let optional = r.bytes(pe + 24, optional_size)?;
    let (count_at, dirs_at) = match u16_at(&optional, 0)? {
        0x10b => (92, 96),
        0x20b => (108, 112),
        _ => return None,
    };
    if u32_at(&optional, count_at)? <= 2 {
        return None;
    }
    let resources = u32_at(&optional, dirs_at + 2 * 8)?;
    if resources == 0 {
        return None;
    }
    let table = r.bytes(pe + 24 + optional_size as u64, sections * 40)?;
    let sections = table
        .as_chunks::<40>()
        .0
        .iter()
        .map(|s| s.as_slice())
        .filter_map(|s| {
            Some(Section {
                size: u32_at(s, 8)?,
                rva: u32_at(s, 12)?,
                raw_size: u32_at(s, 16)?,
                raw: u32_at(s, 20)?,
            })
        })
        .collect();
    Some(Image {
        sections,
        resources,
    })
}

/// The entries of the resource directory at `dir` (relative to the
/// resource section): (id or name flag, offset with its directory flag).
fn entries<R: Read + Seek>(
    r: &mut Bounded<'_, R>,
    image: &Image,
    dir: u32,
) -> Option<Vec<(u32, u32)>> {
    let at = image.offset(image.resources.checked_add(dir)?, 16)?;
    let head = r.bytes(at, 16)?;
    let count = usize::from(u16_at(&head, 12)?) + usize::from(u16_at(&head, 14)?);
    if count > MAX_ENTRIES {
        return None;
    }
    let list = r.bytes(at + 16, count * 8)?;
    Some(
        list.as_chunks::<8>()
            .0
            .iter()
            .filter_map(|e| Some((u32_at(e.as_slice(), 0)?, u32_at(e.as_slice(), 4)?)))
            .collect(),
    )
}

const SUBDIR: u32 = 0x8000_0000;

/// The first leaf under `entry` (a subdirectory's first item, down to the
/// data): its bytes, at most `max` long.
fn first_data<R: Read + Seek>(
    r: &mut Bounded<'_, R>,
    image: &Image,
    mut offset: u32,
    max: u32,
) -> Option<Vec<u8>> {
    // A name, then a language: at most two levels more.
    for _ in 0..3 {
        if offset & SUBDIR == 0 {
            let at = image.offset(image.resources.checked_add(offset)?, 16)?;
            let leaf = r.bytes(at, 16)?;
            let (rva, size) = (u32_at(&leaf, 0)?, u32_at(&leaf, 4)?);
            if size == 0 || size > max {
                return None;
            }
            let at = image.offset(rva, size)?;
            return r.bytes(at, size as usize);
        }
        offset = entries(r, image, offset & !SUBDIR)?.first()?.1;
    }
    None
}

/// The type directory's entries for `kind`, by id.
fn of_type<R: Read + Seek>(
    r: &mut Bounded<'_, R>,
    image: &Image,
    kind: u32,
) -> Option<Vec<(u32, u32)>> {
    let root = entries(r, image, 0)?;
    let (_, dir) = root.into_iter().find(|(id, _)| *id == kind)?;
    if dir & SUBDIR == 0 {
        return None;
    }
    entries(r, image, dir & !SUBDIR)
}

fn read_icon<R: Read + Seek>(r: &mut Bounded<'_, R>) -> Option<Vec<u8>> {
    let image = read_image(r)?;
    let groups = of_type(r, &image, RT_GROUP_ICON)?;
    let (_, group) = groups.first()?;
    let group = first_data(r, &image, *group, MAX_GROUP_BYTES)?;
    let count = usize::from(u16_at(&group, 4)?);
    // Each GRPICONDIRENTRY: width, height, colours, reserved, planes,
    // bit count, size, then the RT_ICON id.
    let best = group
        .get(6..6 + count * 14)?
        .as_chunks::<14>()
        .0
        .iter()
        .map(|e| e.as_slice())
        .max_by_key(|e| {
            let side = |b: u8| if b == 0 { 256 } else { u32::from(b) };
            (side(e[0]) * side(e[1]), u16_at(e, 6).unwrap_or(0))
        })?
        .to_vec();
    let id = u32::from(u16_at(&best, 12)?);
    let icons = of_type(r, &image, RT_ICON)?;
    let (_, icon) = icons.into_iter().find(|(i, _)| *i == id)?;
    let data = first_data(r, &image, icon, MAX_ICON_BYTES)?;
    Some(ico_file(&best[..8], &data))
}

/// An `.ico` file holding one picture, described by the group entry's first
/// eight bytes.
fn ico_file(entry: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(22 + data.len());
    out.extend_from_slice(&[0, 0, 1, 0, 1, 0]);
    out.extend_from_slice(entry);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&22u32.to_le_bytes());
    out.extend_from_slice(data);
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;

    /// A minimal PE32+ program whose resources hold one icon group with two
    /// pictures (16 px and 48 px), the larger a PNG.
    pub(crate) fn program_with_icon(png: &[u8]) -> Vec<u8> {
        let small = vec![0xAAu8; 40];
        // Resource section, at RVA 0x1000 and file offset 0x200.
        let mut rsrc = Vec::new();
        let dir = |named: u16, ids: u16| {
            let mut d = vec![0u8; 12];
            d.extend_from_slice(&named.to_le_bytes());
            d.extend_from_slice(&ids.to_le_bytes());
            d
        };
        let entry = |id: u32, off: u32| {
            let mut e = id.to_le_bytes().to_vec();
            e.extend_from_slice(&off.to_le_bytes());
            e
        };
        // Layout (offsets within the section):
        // 0x000 root: RT_ICON -> 0x100, RT_GROUP_ICON -> 0x200
        // 0x100 icons: id 1 -> 0x120, id 2 -> 0x140
        // 0x120/0x140 languages -> leaves 0x300/0x310
        // 0x200 groups: id 1 -> 0x220 -> leaf 0x320
        // 0x400 data
        rsrc.resize(0x600 + small.len() + png.len(), 0);
        let mut put = |at: usize, bytes: &[u8]| rsrc[at..at + bytes.len()].copy_from_slice(bytes);
        let mut root = dir(0, 2);
        root.extend(entry(RT_ICON, SUBDIR | 0x100));
        root.extend(entry(RT_GROUP_ICON, SUBDIR | 0x200));
        put(0, &root);
        let mut icons = dir(0, 2);
        icons.extend(entry(1, SUBDIR | 0x120));
        icons.extend(entry(2, SUBDIR | 0x140));
        put(0x100, &icons);
        for (at, leaf) in [(0x120, 0x300), (0x140, 0x310), (0x220, 0x320)] {
            let mut lang = dir(0, 1);
            lang.extend(entry(1033, leaf));
            put(at, &lang);
        }
        let mut groups = dir(0, 1);
        groups.extend(entry(1, SUBDIR | 0x220));
        put(0x200, &groups);
        let group_at = 0x400u32;
        let small_at = 0x500u32;
        let png_at = 0x600u32;
        let leaf = |rva: u32, size: usize| {
            let mut l = rva.to_le_bytes().to_vec();
            l.extend_from_slice(&(size as u32).to_le_bytes());
            l.extend_from_slice(&[0; 8]);
            l
        };
        let mut group = vec![0, 0, 1, 0, 2, 0];
        for (side, bits, size, id) in [(16u8, 32u16, small.len(), 1u16), (48, 32, png.len(), 2)] {
            group.extend_from_slice(&[side, side, 0, 0, 1, 0]);
            group.extend_from_slice(&bits.to_le_bytes());
            group.extend_from_slice(&(size as u32).to_le_bytes());
            group.extend_from_slice(&id.to_le_bytes());
        }
        put(0x300, &leaf(0x1000 + small_at, small.len()));
        put(0x310, &leaf(0x1000 + png_at, png.len()));
        put(0x320, &leaf(0x1000 + group_at, group.len()));
        put(group_at as usize, &group);
        put(small_at as usize, &small);
        put(png_at as usize, png);

        let mut file = vec![0u8; 0x200];
        file[..2].copy_from_slice(b"MZ");
        file[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        file[0x40..0x44].copy_from_slice(b"PE\0\0");
        file[0x46..0x48].copy_from_slice(&1u16.to_le_bytes());
        let optional_size = 112 + 16 * 8;
        file[0x54..0x56].copy_from_slice(&(optional_size as u16).to_le_bytes());
        let opt = 0x58;
        file[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        file[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes());
        file[opt + 112 + 16..opt + 112 + 20].copy_from_slice(&0x1000u32.to_le_bytes());
        file[opt + 112 + 20..opt + 112 + 24].copy_from_slice(&(rsrc.len() as u32).to_le_bytes());
        let sec = opt + optional_size;
        file[sec..sec + 5].copy_from_slice(b".rsrc");
        file[sec + 8..sec + 12].copy_from_slice(&(rsrc.len() as u32).to_le_bytes());
        file[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        file[sec + 16..sec + 20].copy_from_slice(&(rsrc.len() as u32).to_le_bytes());
        file[sec + 20..sec + 24].copy_from_slice(&0x200u32.to_le_bytes());
        file.extend(rsrc);
        file
    }

    fn icon(bytes: &[u8]) -> Option<Vec<u8>> {
        let mut c = Cursor::new(bytes.to_vec());
        let len = bytes.len() as u64;
        read_icon(&mut Bounded { inner: &mut c, len })
    }

    #[test]
    fn takes_the_largest_picture_of_the_first_group() {
        let png = b"\x89PNG\r\n\x1a\nrest".to_vec();
        let ico = icon(&program_with_icon(&png)).unwrap();
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 1, 0]);
        assert_eq!(ico[6], 48, "the 48 px picture");
        assert_eq!(u32_at(&ico, 14), Some(png.len() as u32));
        assert_eq!(&ico[22..], &png[..]);
    }

    #[test]
    fn broken_or_cut_programs_have_no_icon() {
        let program = program_with_icon(b"\x89PNG\r\n\x1a\n");
        for cut in [0, 2, 0x40, 0x100, 0x210, 0x700] {
            assert!(icon(&program[..cut.min(program.len())]).is_none(), "{cut}");
        }
        assert!(icon(b"not a program").is_none());
        let mut wrong = program.clone();
        wrong[0x40] = b'X';
        assert!(icon(&wrong).is_none());
    }
}
