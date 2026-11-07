//! The Playnite import against LiteDB files built here: a small writer
//! produces LiteDB 4's on-disk format (header page, collection page,
//! skip-list index pages, data pages and extend pages, BSON documents), as
//! LiteDB 4.1 writes it.

use super::bson::{Bson, Document, Guid};
use super::litedb::*;
use super::*;

/// BSON, as LiteDB's `BsonWriter` writes it.
fn encode(doc: &Document) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in &doc.0 {
        let (kind, bytes) = encode_value(value);
        body.push(kind);
        body.extend(name.as_bytes());
        body.push(0);
        body.extend(bytes);
    }
    body.push(0);
    let mut out = ((body.len() + 4) as i32).to_le_bytes().to_vec();
    out.extend(body);
    out
}

fn encode_value(value: &Bson) -> (u8, Vec<u8>) {
    match value {
        Bson::Double(v) => (0x01, v.to_le_bytes().to_vec()),
        Bson::String(s) => {
            let mut out = ((s.len() + 1) as i32).to_le_bytes().to_vec();
            out.extend(s.as_bytes());
            out.push(0);
            (0x02, out)
        }
        Bson::Document(d) => (0x03, encode(d)),
        Bson::Array(items) => {
            let doc = Document(
                items
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (i.to_string(), v.clone()))
                    .collect(),
            );
            (0x04, encode(&doc))
        }
        Bson::Binary(b) => {
            let mut out = (b.len() as i32).to_le_bytes().to_vec();
            out.push(0);
            out.extend(b);
            (0x05, out)
        }
        Bson::Guid(g) => {
            let mut out = 16i32.to_le_bytes().to_vec();
            out.push(0x04);
            out.extend(g.0);
            (0x05, out)
        }
        Bson::ObjectId(b) => (0x07, b.to_vec()),
        Bson::Bool(b) => (0x08, vec![u8::from(*b)]),
        Bson::DateTime(ms) => (0x09, ms.to_le_bytes().to_vec()),
        Bson::Null => (0x0A, Vec::new()),
        Bson::Int32(v) => (0x10, v.to_le_bytes().to_vec()),
        Bson::Int64(v) => (0x12, v.to_le_bytes().to_vec()),
        Bson::Decimal(b) => (0x13, b.to_vec()),
        Bson::MinKey => (0xFF, Vec::new()),
        Bson::MaxKey => (0x7F, Vec::new()),
    }
}

const AVAILABLE: usize = PAGE_SIZE - PAGE_HEADER;
const BLOCK_FIXED: usize = 8;
const HEAD_LEVELS: u8 = 32;

/// One page under construction.
struct Page {
    kind: u8,
    prev: u32,
    next: u32,
    items: u16,
    content: Vec<u8>,
}

impl Page {
    fn new(kind: u8) -> Self {
        Self {
            kind,
            prev: NO_PAGE,
            next: NO_PAGE,
            items: 0,
            content: Vec::new(),
        }
    }

    fn bytes(&self, id: u32) -> Vec<u8> {
        let mut out = vec![0u8; PAGE_SIZE];
        out[0..4].copy_from_slice(&id.to_le_bytes());
        out[4] = self.kind;
        out[5..9].copy_from_slice(&self.prev.to_le_bytes());
        out[9..13].copy_from_slice(&self.next.to_le_bytes());
        out[13..15].copy_from_slice(&self.items.to_le_bytes());
        let free = (AVAILABLE - self.content.len().min(AVAILABLE)) as u16;
        out[15..17].copy_from_slice(&free.to_le_bytes());
        out[PAGE_HEADER..PAGE_HEADER + self.content.len()].copy_from_slice(&self.content);
        out
    }
}

fn string(out: &mut Vec<u8>, s: &str) {
    out.extend((s.len() as i32).to_le_bytes());
    out.extend(s.as_bytes());
}

fn address(out: &mut Vec<u8>, page: u32, index: u16) {
    out.extend(page.to_le_bytes());
    out.extend(index.to_le_bytes());
}

/// A LiteDB 4 file with one collection of `docs`, each keyed by its `_id`
/// Guid.
fn database(collection: &str, docs: &[Document]) -> Vec<u8> {
    let mut pages: Vec<Page> = vec![Page::new(HEADER_PAGE), Page::new(COLLECTION_PAGE)];

    // Data blocks first, so the nodes know where their documents are.
    let mut blocks = Vec::new();
    let mut data_page: Option<usize> = None;
    for doc in docs {
        let bytes = encode(doc);
        let extend = bytes.len() + BLOCK_FIXED > AVAILABLE;
        let need = BLOCK_FIXED + if extend { 0 } else { bytes.len() };
        let fits = data_page.is_some_and(|p| pages[p].content.len() + need <= AVAILABLE);
        if !fits {
            pages.push(Page::new(DATA_PAGE));
            data_page = Some(pages.len() - 1);
        }
        let p = data_page.unwrap();
        let mut extend_first = NO_PAGE;
        if extend {
            let mut previous: Option<usize> = None;
            for chunk in bytes.chunks(AVAILABLE) {
                let mut page = Page::new(EXTEND_PAGE);
                page.items = chunk.len() as u16;
                page.content = chunk.to_vec();
                pages.push(page);
                let id = pages.len() - 1;
                if let Some(prev) = previous {
                    pages[prev].next = id as u32;
                    pages[id].prev = prev as u32;
                } else {
                    extend_first = id as u32;
                }
                previous = Some(id);
            }
        }
        let index = pages[p].items;
        let content = &mut pages[p].content;
        content.extend(index.to_le_bytes());
        content.extend(extend_first.to_le_bytes());
        let stored: &[u8] = if extend { &[] } else { &bytes };
        content.extend((stored.len() as u16).to_le_bytes());
        content.extend(stored);
        pages[p].items += 1;
        blocks.push((p as u32, index));
    }

    // Index nodes: head, the documents', tail; placed in pages as they fit.
    struct NodeSpec {
        levels: u8,
        key: Vec<u8>,
        key_len: u16,
        data: (u32, u16),
    }
    let mut specs = vec![NodeSpec {
        levels: HEAD_LEVELS,
        key: vec![0],
        key_len: 0,
        data: (NO_PAGE, u16::MAX),
    }];
    for (doc, block) in docs.iter().zip(&blocks) {
        let guid = doc.get("_id").and_then(Bson::as_guid).unwrap();
        let mut key = vec![11u8];
        key.extend(guid.0);
        specs.push(NodeSpec {
            levels: 1,
            key,
            key_len: 16,
            data: *block,
        });
    }
    specs.push(NodeSpec {
        levels: HEAD_LEVELS,
        key: vec![14],
        key_len: 0,
        data: (NO_PAGE, u16::MAX),
    });
    let size = |s: &NodeSpec| 18 + s.key.len() + 6 + usize::from(s.levels) * 12;
    let mut at = Vec::new();
    let mut index_page: Option<usize> = None;
    let mut used = 0;
    for spec in &specs {
        if index_page.is_none() || used + size(spec) > AVAILABLE {
            pages.push(Page::new(INDEX_PAGE));
            index_page = Some(pages.len() - 1);
            used = 0;
        }
        let p = index_page.unwrap();
        at.push((p as u32, pages[p].items));
        pages[p].items += 1;
        used += size(spec);
    }
    let empty = (NO_PAGE, u16::MAX);
    let last = specs.len() - 1;
    for (i, spec) in specs.iter().enumerate() {
        let (p, index) = at[i];
        let prev = if i == 0 { empty } else { at[i - 1] };
        let next = if i == last { empty } else { at[i + 1] };
        let content = &mut pages[p as usize].content;
        content.extend(index.to_le_bytes());
        content.push(spec.levels);
        content.push(0);
        address(content, empty.0, empty.1);
        address(content, empty.0, empty.1);
        content.extend(spec.key_len.to_le_bytes());
        content.extend(&spec.key);
        address(content, spec.data.0, spec.data.1);
        for level in 0..spec.levels {
            let (prev, next) = match (level, i) {
                (0, _) => (prev, next),
                // Higher levels of the head and tail point at each other.
                (_, 0) => (empty, at[last]),
                _ => (at[0], empty),
            };
            address(content, prev.0, prev.1);
            address(content, next.0, next.1);
        }
    }

    // The collection page.
    let mut c = Vec::new();
    string(&mut c, collection);
    c.extend((docs.len() as i64).to_le_bytes());
    c.extend(NO_PAGE.to_le_bytes());
    for slot in 0..INDEXES {
        if slot == 0 {
            string(&mut c, "_id=$._id");
            c.push(1);
            address(&mut c, at[0].0, at[0].1);
            address(&mut c, at[last].0, at[last].1);
        } else {
            string(&mut c, "");
            c.push(0);
            address(&mut c, NO_PAGE, u16::MAX);
            address(&mut c, NO_PAGE, u16::MAX);
        }
        c.extend(NO_PAGE.to_le_bytes());
    }
    pages[1].items = 1;
    pages[1].content = c;

    // The header page.
    let mut h = Vec::new();
    h.extend(HEADER_INFO);
    h.push(FILE_VERSION);
    h.extend(3u16.to_le_bytes());
    h.extend(NO_PAGE.to_le_bytes());
    h.extend(((pages.len() - 1) as u32).to_le_bytes());
    h.extend(0u16.to_le_bytes());
    h.extend([0u8; 20]);
    h.extend([0u8; 16]);
    h.push(1);
    string(&mut h, collection);
    h.extend(1u32.to_le_bytes());
    pages[0].items = 1;
    pages[0].content = h;

    let mut file = Vec::new();
    for (id, page) in pages.iter().enumerate() {
        let mut bytes = page.bytes(id as u32);
        if id == 1 {
            // Each index's max level sits in the page's footer, before the
            // sequence.
            let footer = PAGE_SIZE - 8 - INDEXES;
            bytes[footer] = HEAD_LEVELS;
        }
        file.extend(bytes);
    }
    file
}

fn guid(n: u8) -> Guid {
    let mut b = [0u8; 16];
    b[0] = n;
    b[15] = 0xAB;
    Guid(b)
}

fn doc(fields: Vec<(&str, Bson)>) -> Document {
    Document(
        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

fn s(v: &str) -> Bson {
    Bson::String(v.into())
}

#[test]
fn reads_every_document_across_pages() {
    let long = "x".repeat(10_000);
    let mut docs: Vec<Document> = (0..150u8)
        .map(|n| {
            doc(vec![
                ("_id", Bson::Guid(guid(n))),
                ("Name", s(&format!("Game {n}"))),
                ("Playtime", Bson::Int64(i64::from(n) * 60)),
            ])
        })
        .collect();
    // One document larger than a page, in extend pages.
    docs.push(doc(vec![
        ("_id", Bson::Guid(guid(200))),
        ("Name", s("Long")),
        ("Description", s(&long)),
        (
            "Nested",
            Bson::Array(vec![Bson::Document(doc(vec![("Path", s("a"))]))]),
        ),
    ]));
    let file = database("Game", &docs);
    // Several index and data pages, and extend pages.
    assert!(file.len() > 8 * PAGE_SIZE);
    let db = LiteDb::from_bytes(file).unwrap();
    assert_eq!(db.collection_names().collect::<Vec<_>>(), ["Game"]);
    let read = db.documents("game").unwrap();
    assert_eq!(read, docs);
    assert_eq!(read[150].str("Description").map(str::len), Some(10_000));
    assert!(db.documents("Platform").is_err());

    let empty = LiteDb::from_bytes(database("Game", &[])).unwrap();
    assert!(empty.documents("Game").unwrap().is_empty());
}

#[test]
fn damaged_files_are_errors_not_panics() {
    let docs: Vec<Document> = (0..40u8)
        .map(|n| {
            doc(vec![
                ("_id", Bson::Guid(guid(n))),
                ("Name", s(&"y".repeat(usize::from(n) * 20))),
            ])
        })
        .collect();
    let good = database("Game", &docs);

    let mut short = good.clone();
    short.truncate(good.len() - PAGE_SIZE);
    assert!(
        LiteDb::from_bytes(short)
            .and_then(|d| d.documents("Game"))
            .is_err()
    );
    assert!(LiteDb::from_bytes(good[..100].to_vec()).is_err());

    let mut version = good.clone();
    version[PAGE_HEADER + HEADER_INFO.len()] = 8;
    assert!(LiteDb::from_bytes(version).is_err());
    let mut password = good.clone();
    password[PAGE_HEADER + HEADER_INFO.len() + 13] = 1;
    assert!(LiteDb::from_bytes(password).is_err());
    let mut recovery = good.clone();
    recovery[PAGE_SIZE - 1] = 1;
    assert!(LiteDb::from_bytes(recovery).is_err());

    // Every byte of every page changed in turn (a cheap fuzz): reading
    // either works or fails, never panics or hangs.
    let mut state = 0x2545_f491_u32;
    for _ in 0..3000 {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let mut bad = good.clone();
        let at = state as usize % bad.len();
        bad[at] ^= (state >> 24) as u8 | 1;
        if let Ok(db) = LiteDb::from_bytes(bad) {
            let _ = db.documents("Game");
        }
    }
}

#[test]
fn an_index_loop_is_refused() {
    let docs: Vec<Document> = (0..3u8)
        .map(|n| doc(vec![("_id", Bson::Guid(guid(n)))]))
        .collect();
    let mut file = database("Game", &docs);
    // The index page is the last one: head, three document nodes, tail.
    // The second document node's next link is pointed back at the first.
    let page = file.len() / PAGE_SIZE - 1;
    let start = page * PAGE_SIZE + PAGE_HEADER;
    let head = 18 + 1 + 6 + 32 * 12;
    let node = 18 + 17 + 6 + 12;
    // Its level-0 next link follows the fixed part and the prev link.
    let third_next = start + head + node + (18 + 17 + 6) + 6;
    file[third_next..third_next + 4].copy_from_slice(&(page as u32).to_le_bytes());
    file[third_next + 4..third_next + 6].copy_from_slice(&1u16.to_le_bytes());
    let db = LiteDb::from_bytes(file).unwrap();
    assert!(db.documents("Game").is_err());
}

/// A Playnite library under `root` (the program in `root/Playnite`, roaming
/// data in `root/AppData`), installed with an uninstaller.
fn library(root: &Path, games: &[Document], categories: &[Document]) -> PlayniteDirs {
    let program = root.join("Playnite");
    let appdata = root.join("AppData");
    std::fs::create_dir_all(&program).unwrap();
    std::fs::write(program.join("unins000.exe"), "").unwrap();
    let dirs = PlayniteDirs::from_program_dir(&program, &appdata);
    assert_eq!(dirs.library_dir, appdata.join("Playnite/library"));
    std::fs::create_dir_all(dirs.library_dir.join("files")).unwrap();
    std::fs::write(dirs.library_dir.join("games.db"), database("Game", games)).unwrap();
    if !categories.is_empty() {
        std::fs::write(
            dirs.library_dir.join("categories.db"),
            database("Category", categories),
        )
        .unwrap();
    }
    dirs
}

const STEAM: &str = "cb91dfc9-b977-43bf-8e70-55f46e410fab";
const BATTLE_NET: &str = "e3c26a3d-d695-4cb7-a769-5ff7612c7edd";
const HUMBLE: &str = "96e8c4bc-ec5c-4c8b-87e7-18ee5a690626";

#[test]
fn plans_installed_games_and_store_matches() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let hades = root.join("Games/Hades");
    std::fs::create_dir_all(&hades).unwrap();
    let rpg = guid(90);
    let games = vec![
        doc(vec![
            ("_id", Bson::Guid(guid(1))),
            ("Name", s("Hades")),
            ("IsInstalled", Bson::Bool(true)),
            ("InstallDirectory", s(&hades.to_string_lossy())),
            ("PluginId", Bson::Guid(Guid::parse(HUMBLE).unwrap())),
            ("GameId", s("hades")),
            ("Playtime", Bson::Int64(5430)),
            ("LastActivity", Bson::DateTime(1_700_000_000_000)),
            ("Favorite", Bson::Bool(true)),
            ("CategoryIds", Bson::Array(vec![Bson::Guid(rpg)])),
            ("CoverImage", s(&format!("{}/cover.jpg", guid(1)))),
            ("BackgroundImage", s("https://example.com/bg.jpg")),
        ]),
        doc(vec![
            ("_id", Bson::Guid(guid(2))),
            ("Name", s("Portal")),
            ("IsInstalled", Bson::Bool(false)),
            ("PluginId", Bson::Guid(Guid::parse(STEAM).unwrap())),
            ("GameId", s("400")),
            ("Playtime", Bson::Int64(600)),
        ]),
        doc(vec![
            ("_id", Bson::Guid(guid(3))),
            ("Name", s("Overwatch")),
            ("IsInstalled", Bson::Bool(true)),
            ("PluginId", Bson::Guid(Guid::parse(BATTLE_NET).unwrap())),
            ("GameId", s("Pro")),
        ]),
        doc(vec![
            ("_id", Bson::Guid(guid(4))),
            ("Name", s("Not here")),
            ("IsInstalled", Bson::Bool(false)),
        ]),
        doc(vec![
            ("_id", Bson::Guid(guid(5))),
            ("Name", s("Secret")),
            ("IsInstalled", Bson::Bool(true)),
            ("Hidden", Bson::Bool(true)),
            ("InstallDirectory", s(&hades.to_string_lossy())),
        ]),
        doc(vec![
            ("_id", Bson::Guid(guid(6))),
            ("Name", s("Gone")),
            ("IsInstalled", Bson::Bool(true)),
            (
                "InstallDirectory",
                s(&root.join("Games/Gone").to_string_lossy()),
            ),
        ]),
        doc(vec![
            ("_id", Bson::Guid(guid(7))),
            ("Name", s("Somewhere")),
            ("IsInstalled", Bson::Bool(true)),
            (
                "GameActions",
                Bson::Array(vec![Bson::Document(doc(vec![
                    ("Type", s("URL")),
                    ("Path", s("https://example.com/play")),
                    ("IsPlayAction", Bson::Bool(true)),
                ]))]),
            ),
        ]),
        // An emulated game: followed by its ROM's folder.
        doc(vec![
            ("_id", Bson::Guid(guid(8))),
            ("Name", s("Chrono Trigger")),
            ("IsInstalled", Bson::Bool(true)),
            (
                "Roms",
                Bson::Array(vec![Bson::Document(doc(vec![
                    ("Name", s("Chrono Trigger")),
                    ("Path", s(&hades.join("ct.sfc").to_string_lossy())),
                ]))]),
            ),
        ]),
    ];
    let categories = vec![doc(vec![("_id", Bson::Guid(rpg)), ("Name", s("RPG"))])];
    let dirs = library(root, &games, &categories);
    let cover = dirs
        .library_dir
        .join("files")
        .join(guid(1).to_string())
        .join("cover.jpg");
    std::fs::create_dir_all(cover.parent().unwrap()).unwrap();
    std::fs::write(&cover, "jpg").unwrap();

    let plan = plan(&dirs).unwrap();
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
    assert_eq!(plan.games.len(), 8);
    let hades_game = &plan.games[0];
    assert_eq!(hades_game.id, guid(1).to_string());
    assert_eq!(
        hades_game.kind,
        PlayniteKind::Playnite {
            link: format!("playnite://playnite/start/{}", guid(1)),
            install_dir: hades.clone(),
        }
    );
    assert!(hades_game.favourite);
    assert_eq!(hades_game.categories, ["RPG"]);
    assert_eq!(hades_game.playtime_minutes, 91);
    assert_eq!(
        hades_game.last_played.as_deref(),
        Some("2023-11-14T22:13:20Z")
    );
    assert_eq!(
        hades_game.cover,
        Some(url::Url::from_file_path(&cover).unwrap().to_string())
    );
    assert_eq!(
        hades_game.background.as_deref(),
        Some("https://example.com/bg.jpg")
    );

    // Store games are matched installed or not.
    assert_eq!(
        plan.games[1].kind,
        PlayniteKind::Store {
            runner: Runner::Steam,
            app_name: "400".into(),
            store: "Steam".into(),
            account: false,
        }
    );
    assert!(!plan.games[1].installed);
    assert_eq!(plan.games[1].playtime_minutes, 10);
    assert!(matches!(
        &plan.games[2].kind,
        PlayniteKind::Store { app_name, .. } if app_name == "battlenet:pro"
    ));

    let skipped: Vec<_> = plan.games[3..7]
        .iter()
        .map(|g| match &g.kind {
            PlayniteKind::Skipped { reason } => reason.id(),
            other => panic!("{} became {other:?}", g.name),
        })
        .collect();
    assert_eq!(
        skipped,
        [
            "playniteNotInstalled",
            "playniteHidden",
            "folderMissing",
            "noInstallFolder"
        ]
    );
    assert!(matches!(
        &plan.games[7].kind,
        PlayniteKind::Playnite { install_dir, .. } if install_dir == &hades
    ));
}

#[test]
fn art_never_leaves_the_library() {
    let dir = tempfile::tempdir().unwrap();
    let files = dir.path().join("files");
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(dir.path().join("secret.png"), "").unwrap();
    assert_eq!(art(&files, "../secret.png"), None);
    assert_eq!(art(&files, "missing/cover.png"), None);
    assert_eq!(art(&files, "  "), None);
    assert!(art(&files, &dir.path().join("secret.png").to_string_lossy()).is_some());
}

#[test]
fn finds_the_library_of_installed_and_portable_copies() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let appdata = root.join("AppData");
    let portable = root.join("Portable Playnite");
    std::fs::create_dir_all(&portable).unwrap();
    assert_eq!(
        PlayniteDirs::from_program_dir(&portable, &appdata).library_dir,
        portable.join("library")
    );
    // A portable copy's config.json beside it, with a BOM, moving the
    // library with Playnite's variable.
    std::fs::write(
        portable.join("config.json"),
        "\u{feff}{\"DatabasePath\": \"{PlayniteDir}\\\\data\\\\library\"}",
    )
    .unwrap();
    let dirs = PlayniteDirs::from_program_dir(&portable, &appdata);
    assert_eq!(
        dirs.library_dir,
        native(&format!("{}\\data\\library", portable.to_string_lossy()))
    );

    let installed = root.join("Program Files/Playnite");
    std::fs::create_dir_all(&installed).unwrap();
    std::fs::write(installed.join("unins000.exe"), "").unwrap();
    std::fs::create_dir_all(appdata.join("Playnite")).unwrap();
    std::fs::write(
        appdata.join("Playnite/config.json"),
        "{\"DatabasePath\": \"%appdata%\\\\Playnite\\\\other\"}",
    )
    .unwrap();
    assert_eq!(
        PlayniteDirs::from_program_dir(&installed, &appdata).library_dir,
        native(&format!("{}\\Playnite\\other", appdata.to_string_lossy()))
    );
}

#[test]
fn reads_the_program_of_the_link() {
    assert_eq!(
        command_program(
            r#""C:\Users\Ann\AppData\Local\Playnite\Playnite.DesktopApp.exe" --uridata "%1""#
        ),
        Some(PathBuf::from(
            r"C:\Users\Ann\AppData\Local\Playnite\Playnite.DesktopApp.exe"
        ))
    );
    assert_eq!(
        command_program(r#"D:\Playnite\Playnite.DesktopApp.exe --uridata "%1""#),
        Some(PathBuf::from(r"D:\Playnite\Playnite.DesktopApp.exe"))
    );
    assert_eq!(command_program("  "), None);
}

#[test]
fn a_missing_library_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let dirs = PlayniteDirs {
        program_dir: dir.path().to_path_buf(),
        library_dir: dir.path().join("library"),
    };
    assert!(plan(&dirs).is_err());
}
