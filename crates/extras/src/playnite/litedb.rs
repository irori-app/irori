//! A read-only reader of LiteDB 4 data files (file format version 7, the
//! format of LiteDB 4.1, which Playnite 9 and 10 use).
//!
//! The file is a run of 4096-byte pages, each starting with a 25-byte
//! header (page id, type, previous and next page, item count, free bytes).
//! Page 0 holds the file's header and the collection names, each pointing at
//! its collection page; a collection page holds its indexes, the first of
//! which is the `_id` index. Index pages hold skip-list nodes, which point at
//! data blocks in data pages; a document too large for one block continues
//! in a chain of extend pages. Documents are read by walking the `_id`
//! index's bottom level from its head to its tail node, as LiteDB's own
//! `FindAll` does.
//!
//! Nothing is ever written. Every offset, length and page link is checked
//! against the file, and link loops are refused, so a damaged or half-written
//! file is an error, never a panic or an endless walk.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context as _, Result, bail};

use super::bson::{self, BsonError, Cursor, Document};

pub(crate) const PAGE_SIZE: usize = 4096;
pub(crate) const PAGE_HEADER: usize = 25;
pub(crate) const HEADER_INFO: &[u8] = b"** This is a LiteDB file **";
pub(crate) const FILE_VERSION: u8 = 7;
/// "No page" in page links.
pub(crate) const NO_PAGE: u32 = u32::MAX;
/// Indexes per collection page (the reader needs only the first).
#[cfg(test)]
pub(crate) const INDEXES: usize = 16;

/// Files larger than this are not a game library; refused rather than read.
const MAX_FILE: u64 = 2 << 30;

pub(crate) const HEADER_PAGE: u8 = 1;
pub(crate) const COLLECTION_PAGE: u8 = 2;
pub(crate) const INDEX_PAGE: u8 = 3;
pub(crate) const DATA_PAGE: u8 = 4;
pub(crate) const EXTEND_PAGE: u8 = 5;

/// A node or block address: page and index inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Address {
    page: u32,
    index: u16,
}

impl Address {
    fn read(c: &mut Cursor<'_>) -> Result<Self, BsonError> {
        Ok(Self {
            page: c.u32()?,
            index: c.u16()?,
        })
    }

    fn is_empty(self) -> bool {
        self.page == NO_PAGE
    }
}

/// What a walk needs of an index node.
#[derive(Debug, Clone, Copy)]
struct Node {
    next: Address,
    data: Address,
}

/// An open LiteDB 4 file, held in memory.
pub struct LiteDb {
    data: Vec<u8>,
    collections: Vec<(String, u32)>,
}

impl LiteDb {
    /// Read the file at `path`. It is only read; a file another program
    /// holds open for itself (Playnite while it runs) can't be opened on
    /// Windows, which the error says.
    pub fn open(path: &Path) -> Result<Self> {
        let data = read_shared(path)?;
        Self::from_bytes(data).with_context(|| format!("reading {}", path.display()))
    }

    /// Read a LiteDB 4 file from its bytes.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        if data.len() < PAGE_SIZE {
            bail!("the file is too short to be a LiteDB database");
        }
        let page = &data[..PAGE_SIZE];
        let at = PAGE_HEADER;
        if page[4] != HEADER_PAGE || page.get(at..at + HEADER_INFO.len()) != Some(HEADER_INFO) {
            bail!("the file is not a LiteDB 4 database");
        }
        let mut c = Cursor::new(page);
        c.pos = at + HEADER_INFO.len();
        let version = c.u8()?;
        if version != FILE_VERSION {
            bail!("the file is a LiteDB database of an unsupported version ({version})");
        }
        let _change_id = c.u16()?;
        let _free_empty_page = c.u32()?;
        let _last_page = c.u32()?;
        let _user_version = c.u16()?;
        let password = c.take(20)?;
        if password.iter().any(|b| *b != 0) {
            bail!("the database is protected by a password");
        }
        let _salt = c.take(16)?;
        let count = c.u8()?;
        let mut collections = Vec::with_capacity(count.into());
        for _ in 0..count {
            let name = read_string(&mut c)?;
            let page = c.u32()?;
            collections.push((name, page));
        }
        // LiteDB keeps "a write was interrupted" in the page's last byte and
        // repairs the file from its journal the next time it opens it.
        if page[PAGE_SIZE - 1] != 0 {
            bail!("the database wasn't closed properly; open and close Playnite once first");
        }
        Ok(Self { data, collections })
    }

    /// The collections' names.
    pub fn collection_names(&self) -> impl Iterator<Item = &str> {
        self.collections.iter().map(|(n, _)| n.as_str())
    }

    /// Every document of `collection` (matched without letter case, as
    /// LiteDB matches names), in `_id` order.
    pub fn documents(&self, collection: &str) -> Result<Vec<Document>> {
        let page_id = self
            .collections
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(collection))
            .map(|(_, p)| *p)
            .with_context(|| format!("the database has no collection “{collection}”"))?;
        let page = self.page(page_id, COLLECTION_PAGE)?;
        let mut c = Cursor::new(page);
        c.pos = PAGE_HEADER;
        let _name = read_string(&mut c)?;
        let count = c.i64()?;
        let _free_data_page = c.u32()?;
        // The first index is always `_id`.
        let _field = read_string(&mut c)?;
        let _unique = c.u8()?;
        let head = Address::read(&mut c)?;
        let tail = Address::read(&mut c)?;

        let mut pages: HashMap<u32, HashMap<u16, Node>> = HashMap::new();
        let mut visited = HashSet::new();
        let mut out = Vec::with_capacity(usize::try_from(count).unwrap_or(0).min(100_000));
        let mut current = head;
        loop {
            let node = self.node(&mut pages, current)?;
            let next = node.next;
            if next.is_empty() || next == tail || next == head {
                break;
            }
            if !visited.insert(next) {
                bail!("the database's index loops");
            }
            let node = self.node(&mut pages, next)?;
            let bytes = self.block(node.data)?;
            out.push(bson::document(&bytes)?);
            current = next;
        }
        Ok(out)
    }

    /// Page `id`, checked to be of `kind`.
    fn page(&self, id: u32, kind: u8) -> Result<&[u8]> {
        let start = usize::try_from(id)
            .ok()
            .and_then(|i| i.checked_mul(PAGE_SIZE))
            .filter(|s| {
                s.checked_add(PAGE_SIZE)
                    .is_some_and(|e| e <= self.data.len())
            })
            .with_context(|| format!("the database points at page {id}, past its end"))?;
        let page = &self.data[start..start + PAGE_SIZE];
        let stored = u32::from_le_bytes([page[0], page[1], page[2], page[3]]);
        if stored != id || page[4] != kind {
            bail!("page {id} of the database isn't what it should be");
        }
        Ok(page)
    }

    /// The index node at `at`, parsing its page the first time.
    fn node(&self, pages: &mut HashMap<u32, HashMap<u16, Node>>, at: Address) -> Result<Node> {
        let nodes = match pages.entry(at.page) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => e.insert(self.index_page(at.page)?),
        };
        nodes
            .get(&at.index)
            .copied()
            .with_context(|| format!("index node {}:{} is missing", at.page, at.index))
    }

    fn index_page(&self, id: u32) -> Result<HashMap<u16, Node>> {
        let page = self.page(id, INDEX_PAGE)?;
        let mut c = Cursor::new(page);
        c.pos = 13;
        let items = c.u16()?;
        c.pos = PAGE_HEADER;
        let mut nodes = HashMap::with_capacity(items.into());
        for _ in 0..items {
            let index = c.u16()?;
            let levels = c.u8()?;
            let _slot = c.u8()?;
            let _prev_node = Address::read(&mut c)?;
            let _next_node = Address::read(&mut c)?;
            let key_len = c.u16()?;
            skip_key(&mut c, key_len)?;
            let data = Address::read(&mut c)?;
            if levels == 0 {
                bail!("index node {id}:{index} has no levels");
            }
            let mut next = None;
            for _ in 0..levels {
                let _prev = Address::read(&mut c)?;
                let n = Address::read(&mut c)?;
                next.get_or_insert(n);
            }
            nodes.insert(
                index,
                Node {
                    next: next.expect("levels > 0"),
                    data,
                },
            );
        }
        Ok(nodes)
    }

    /// The bytes of the data block at `at`, with its extend pages.
    fn block(&self, at: Address) -> Result<Vec<u8>> {
        let page = self.page(at.page, DATA_PAGE)?;
        let mut c = Cursor::new(page);
        c.pos = 13;
        let items = c.u16()?;
        c.pos = PAGE_HEADER;
        for _ in 0..items {
            let index = c.u16()?;
            let extend = c.u32()?;
            let size = c.u16()?;
            let bytes = c.take(size.into())?;
            if index != at.index {
                continue;
            }
            if extend == NO_PAGE {
                return Ok(bytes.to_vec());
            }
            return self.extend_data(extend);
        }
        bail!("data block {}:{} is missing", at.page, at.index)
    }

    fn extend_data(&self, first: u32) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut id = first;
        while id != NO_PAGE {
            if !seen.insert(id) {
                bail!("the database's extend pages loop");
            }
            let page = self.page(id, EXTEND_PAGE)?;
            let next = u32::from_le_bytes([page[9], page[10], page[11], page[12]]);
            let len = usize::from(u16::from_le_bytes([page[13], page[14]]));
            let data = page
                .get(PAGE_HEADER..PAGE_HEADER + len)
                .context("an extend page holds more than fits")?;
            out.extend_from_slice(data);
            id = next;
        }
        Ok(out)
    }
}

/// A LiteDB string: a 32-bit length and UTF-8 bytes.
fn read_string(c: &mut Cursor<'_>) -> Result<String, BsonError> {
    let len = usize::try_from(c.i32()?).map_err(|_| BsonError("negative length"))?;
    Ok(String::from_utf8_lossy(c.take(len)?).into_owned())
}

/// Skip an index key: a LiteDB type byte, then its value (strings and
/// binaries are `key_len` long; documents carry their own length).
fn skip_key(c: &mut Cursor<'_>, key_len: u16) -> Result<(), BsonError> {
    let len = match c.u8()? {
        0 | 1 | 14 => 0,         // MinValue, Null, MaxValue
        2 => 4,                  // Int32
        3 | 4 | 13 => 8,         // Int64, Double, DateTime
        5 | 11 => 16,            // Decimal, Guid
        6 | 9 => key_len.into(), // String, Binary
        7 | 8 => {
            // Document, Array: a BSON document, which starts with its length.
            let start = c.pos;
            let len = usize::try_from(c.i32()?).map_err(|_| BsonError("negative length"))?;
            c.pos = start;
            len
        }
        10 => 12, // ObjectId
        12 => 1,  // Boolean
        _ => return Err(BsonError("unknown index key type")),
    };
    c.take(len).map(|_| ())
}

/// Read a whole file without asking for more than read access. On Windows
/// the file is opened sharing reading and writing, so a program that has it
/// open shared is undisturbed; one that holds it exclusively (Playnite while
/// it runs) makes the open fail, and the message says to close it.
fn read_shared(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read as _;
    let file = open_shared(path).map_err(|e| {
        // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
        if cfg!(windows) && matches!(e.raw_os_error(), Some(32 | 33)) {
            anyhow::anyhow!("Playnite has its library open; close Playnite and try again")
        } else {
            anyhow::Error::new(e).context(format!("opening {}", path.display()))
        }
    })?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len > MAX_FILE {
        bail!("{} is too large to be a game library", path.display());
    }
    let mut data = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    file.take(MAX_FILE)
        .read_to_end(&mut data)
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(data)
}

#[cfg(windows)]
fn open_shared(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    // FILE_SHARE_READ | FILE_SHARE_WRITE
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2)
        .open(path)
}

#[cfg(not(windows))]
fn open_shared(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}
