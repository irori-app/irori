//! Box art from the LaunchBox Games Database, for the ROM library.
//!
//! LaunchBox publishes its whole database as one file, `Metadata.zip`
//! (games, their alternate names and the file names of their images), and
//! points programs that want its data at that file rather than at its web
//! pages; the images themselves are served by file name from
//! `images.launchbox-app.com`. That file and those images are all Irori
//! reads from LaunchBox: no page is fetched or scraped.
//!
//! The file is large (over 100 MB, half a gigabyte unpacked), so it is
//! fetched at most once a week ([`REFRESH_AFTER`], with `If-None-Match`),
//! only when the caller asks ([`Index::refresh`]: the ROM library while it
//! has games, or the user), read as a stream ([`parse`]), and deleted once
//! read. What stays is one small file per system under `<dir>/systems/`,
//! holding the games of the LaunchBox platforms the caller maps to it and
//! the front boxes of each, with their regions ([`SystemIndex`]). A failed
//! download is tried again after [`RETRY_AFTER`].

mod matching;
mod parse;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use md5::{Digest as _, Md5};

use anyhow::{Context as _, Result, bail};
use irori_core::json_store;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt as _;

pub use matching::{Lookup, best, rank, wanted_regions};

/// LaunchBox's whole database, the file it offers other programs.
pub const METADATA_URL: &str = "https://gamesdb.launchbox-app.com/Metadata.zip";
/// Where LaunchBox serves an image by its file name.
pub const IMAGE_BASE: &str = "https://images.launchbox-app.com/";
/// The name shown where its pictures are offered.
pub const SOURCE_NAME: &str = "LaunchBox";
/// How long an index is used before the database is asked again.
pub const REFRESH_AFTER: Duration = Duration::from_secs(7 * 24 * 3600);
/// How long after a failed download it is tried again on its own.
pub const RETRY_AFTER: Duration = Duration::from_secs(12 * 3600);
/// The largest download accepted; LaunchBox's is about 110 MB.
const MAX_DOWNLOAD: u64 = 1 << 30;
/// The most bytes read out of the archive's XML; LaunchBox's is about
/// 520 MB.
const MAX_XML: u64 = 4 << 30;
/// The XML file in the archive.
const XML_NAME: &str = "Metadata.xml";

/// One front box of a game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxArt {
    /// The image's file name on LaunchBox's image server.
    pub file: String,
    /// LaunchBox's region ("North America", "Japan"), when it gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// A cleaned-up rebuild of the box rather than a scan.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reconstructed: bool,
}

impl BoxArt {
    /// The full-size image's address.
    pub fn url(&self) -> String {
        format!("{IMAGE_BASE}{}", self.file)
    }
}

/// A game in LaunchBox's database with its front boxes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchBoxGame {
    /// LaunchBox's `DatabaseID`.
    pub id: u32,
    pub name: String,
    /// Other names (regional titles: "Rockman" for "Mega Man").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternate: Vec<String>,
    pub boxes: Vec<BoxArt>,
}

/// One system's games that have a front box, sorted by name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemIndex {
    pub games: Vec<LaunchBoxGame>,
}

/// What [`Index::refresh`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refreshed {
    /// The database was read again; `games` have boxes.
    Updated { games: usize },
    /// LaunchBox said the database hasn't changed.
    Unchanged,
    /// The index is recent enough, or a download failed a short while ago.
    NotDue,
}

/// When the index was built, and from what.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IndexState {
    /// When the index was last brought up to date (Unix seconds).
    pub updated_at: u64,
    /// When a download was last tried, whatever came of it.
    pub attempted_at: u64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// Systems with an index file.
    pub systems: Vec<String>,
    /// The platform table the files were built with ([`table_key`]): a new
    /// one (a system Irori learned) builds them again.
    pub table: String,
}

impl IndexState {
    /// Whether there is an index to read.
    pub fn has_index(&self) -> bool {
        self.updated_at > 0
    }

    /// Whether [`Index::refresh`] would download, at `now` (Unix seconds),
    /// for the platform table `table`.
    pub fn is_due(&self, table: &str, now: u64) -> bool {
        let fresh =
            self.table == table && now.saturating_sub(self.updated_at) < REFRESH_AFTER.as_secs();
        let tried =
            self.attempted_at > 0 && now.saturating_sub(self.attempted_at) < RETRY_AFTER.as_secs();
        !fresh && !tried
    }
}

/// A fingerprint of a platform → system table.
pub fn table_key(platforms: &HashMap<String, String>) -> String {
    let mut pairs: Vec<String> = platforms.iter().map(|(p, s)| format!("{p}={s}")).collect();
    pairs.sort();
    format!("{:x}", Md5::digest(pairs.join("\n")))
}

/// Seconds since the Unix epoch.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The index files in a folder, and the download that builds them.
pub struct Index {
    dir: PathBuf,
    url: String,
    /// One download at a time.
    lock: tokio::sync::Mutex<()>,
}

impl Index {
    /// An index kept in `dir`, built from LaunchBox's database.
    pub fn new(dir: PathBuf) -> Self {
        Self::with_url(dir, METADATA_URL.into())
    }

    /// An index built from the archive at `url` (tests).
    pub fn with_url(dir: PathBuf, url: String) -> Self {
        Self {
            dir,
            url,
            lock: tokio::sync::Mutex::new(()),
        }
    }

    fn state_file(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    fn system_file(dir: &Path, system: &str) -> Option<PathBuf> {
        // System ids come from Irori's own catalogue; refuse anything that
        // isn't a plain name all the same, since it becomes a file name.
        let plain = !system.is_empty()
            && system
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
        plain.then(|| dir.join("systems").join(format!("{system}.json")))
    }

    /// When the index was built. Blocking.
    pub fn state(&self) -> IndexState {
        json_store::read_or_default(&self.state_file())
    }

    /// `system`'s games, if the index has them. Blocking.
    pub fn load(&self, system: &str) -> Option<SystemIndex> {
        let path = Self::system_file(&self.dir, system)?;
        match json_store::read(&path) {
            Ok(found) => found,
            Err(e) => {
                tracing::warn!("reading LaunchBox's index of {system}: {e:#}");
                None
            }
        }
    }

    /// Download LaunchBox's database and build the index from it when it is
    /// due ([`IndexState::is_due`]), or always with `force`. `platforms`
    /// maps LaunchBox's platform names to the system ids files are kept by.
    pub async fn refresh(
        &self,
        platforms: &HashMap<String, String>,
        force: bool,
    ) -> Result<Refreshed> {
        let asked = now();
        let _one = self.lock.lock().await;
        let state = {
            let path = self.state_file();
            tokio::task::spawn_blocking(move || json_store::read_or_default::<IndexState>(&path))
                .await
                .context("reading LaunchBox's index stopped")?
        };
        let table = table_key(platforms);
        let started = now();
        if !force && !state.is_due(&table, started) {
            return Ok(Refreshed::NotDue);
        }
        // Another caller brought it up to date while this one waited.
        if force && state.has_index() && state.table == table && state.updated_at > asked {
            return Ok(Refreshed::Unchanged);
        }

        let mut next = IndexState {
            attempted_at: started,
            ..state.clone()
        };
        self.save_state(&next).await?;

        let mut request = irori_core::http::download_client().get(&self.url);
        if state.has_index() && state.table == table {
            if let Some(etag) = &state.etag {
                request = request.header(reqwest::header::IF_NONE_MATCH, etag);
            }
            if let Some(modified) = &state.last_modified {
                request = request.header(reqwest::header::IF_MODIFIED_SINCE, modified);
            }
        }
        let response = request
            .send()
            .await
            .map_err(|e| e.without_url())
            .context("Could not reach LaunchBox's games database")?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            next.updated_at = now();
            self.save_state(&next).await?;
            return Ok(Refreshed::Unchanged);
        }
        let mut response = response
            .error_for_status()
            .map_err(|e| e.without_url())
            .context("LaunchBox's games database could not be downloaded")?;
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let (etag, last_modified) = (
            header(reqwest::header::ETAG),
            header(reqwest::header::LAST_MODIFIED),
        );
        if response.content_length().is_some_and(|n| n > MAX_DOWNLOAD) {
            bail!("LaunchBox's games database is larger than Irori expects");
        }

        tokio::fs::create_dir_all(&self.dir)
            .await
            .with_context(|| format!("Could not create {}", self.dir.display()))?;
        let zip = self.dir.join("Metadata.zip");
        let part = self.dir.join("Metadata.zip.part");
        let downloaded = async {
            let mut file = tokio::fs::File::create(&part)
                .await
                .with_context(|| format!("Could not write {}", part.display()))?;
            let mut written = 0u64;
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| e.without_url())
                .context("The download of LaunchBox's games database stopped")?
            {
                written += chunk.len() as u64;
                if written > MAX_DOWNLOAD {
                    bail!("LaunchBox's games database is larger than Irori expects");
                }
                file.write_all(&chunk)
                    .await
                    .with_context(|| format!("Could not write {}", part.display()))?;
            }
            file.flush().await?;
            drop(file);
            tokio::fs::rename(&part, &zip)
                .await
                .with_context(|| format!("Could not write {}", zip.display()))
        }
        .await;
        if let Err(e) = downloaded {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(e);
        }

        let (dir, archive, map) = (self.dir.clone(), zip.clone(), platforms.clone());
        let built = tokio::task::spawn_blocking(move || build(&archive, &dir, &map))
            .await
            .context("reading LaunchBox's games database stopped");
        // The archive is only needed to build the index.
        let _ = tokio::fs::remove_file(&zip).await;
        let (systems, games) = built??;

        next.updated_at = now();
        next.etag = etag;
        next.last_modified = last_modified;
        next.systems = systems;
        next.table = table;
        self.save_state(&next).await?;
        Ok(Refreshed::Updated { games })
    }

    /// The folder holding the index.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The bytes the index takes on disk. Blocking.
    pub fn size(&self) -> u64 {
        fn walk(dir: &Path) -> u64 {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return 0;
            };
            entries
                .flatten()
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => walk(&e.path()),
                    Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
                    _ => 0,
                })
                .sum()
        }
        walk(&self.dir)
    }

    /// Delete the index (and a download left behind), waiting for a
    /// download that runs. The caller vets the folder with `fs_guard`.
    pub async fn remove(&self) -> Result<()> {
        let _one = self.lock.lock().await;
        let dir = self.dir.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let systems = dir.join("systems");
            if systems.exists() {
                std::fs::remove_dir_all(&systems)
                    .with_context(|| format!("Could not delete {}", systems.display()))?;
            }
            for name in ["state.json", "Metadata.zip", "Metadata.zip.part"] {
                let path = dir.join(name);
                match std::fs::remove_file(&path) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                        return Err(e)
                            .with_context(|| format!("Could not delete {}", path.display()));
                    }
                    _ => {}
                }
            }
            Ok(())
        })
        .await
        .context("deleting LaunchBox's index stopped")?
    }

    async fn save_state(&self, state: &IndexState) -> Result<()> {
        let (path, state) = (self.state_file(), state.clone());
        tokio::task::spawn_blocking(move || json_store::write(&path, &state))
            .await
            .context("saving LaunchBox's index stopped")?
            .context("Could not save LaunchBox's index")
    }
}

/// Read the archive at `zip` and write one index file per system into
/// `dir`, removing those of systems it no longer has. Returns the systems
/// written and how many games they hold. Blocking.
fn build(
    zip: &Path,
    dir: &Path,
    platforms: &HashMap<String, String>,
) -> Result<(Vec<String>, usize)> {
    use std::io::Read as _;
    let file =
        std::fs::File::open(zip).with_context(|| format!("Could not open {}", zip.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).context("LaunchBox's games database isn't a valid archive")?;
    let xml = archive
        .by_name(XML_NAME)
        .context("LaunchBox's games database holds no game list")?;
    let systems: BTreeMap<String, SystemIndex> = parse::read(xml.take(MAX_XML), platforms)?;

    let mut written = Vec::new();
    let mut games = 0;
    for (system, index) in &systems {
        let Some(path) = Index::system_file(dir, system) else {
            continue;
        };
        json_store::write(&path, index)
            .with_context(|| format!("Could not save LaunchBox's index of {system}"))?;
        written.push(system.clone());
        games += index.games.len();
    }
    if let Ok(entries) = std::fs::read_dir(dir.join("systems")) {
        for entry in entries.flatten() {
            let path = entry.path();
            let kept = path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| written.iter().any(|w| w == s));
            if !kept && path.extension().is_some_and(|e| e == "json") {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    Ok((written, games))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use irori_depot::test_server::{Resp, TestServer};

    use super::*;

    fn archive(xml: &str) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            w.start_file("Platforms.xml", opts).unwrap();
            w.write_all(b"<LaunchBox />").unwrap();
            w.start_file(XML_NAME, opts).unwrap();
            w.write_all(xml.as_bytes()).unwrap();
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    #[tokio::test]
    async fn downloads_builds_and_asks_again_with_the_etag() {
        let body = archive(parse::tests::FIXTURE);
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = hits.clone();
        let server = TestServer::start(move |req| {
            counted.fetch_add(1, Ordering::SeqCst);
            if req.header("If-None-Match") == Some("\"v1\"") {
                return Resp::status(304);
            }
            Resp {
                status: 200,
                headers: vec![
                    ("ETag".into(), "\"v1\"".into()),
                    ("Content-Type".into(), "application/x-zip-compressed".into()),
                ],
                body: body.clone(),
                ..Default::default()
            }
        });
        let dir = tempfile::tempdir().unwrap();
        // A system the database no longer has is removed.
        std::fs::create_dir_all(dir.path().join("systems")).unwrap();
        std::fs::write(dir.path().join("systems/psx.json"), "{\"games\":[]}").unwrap();
        let index = Index::with_url(dir.path().to_path_buf(), server.url("/Metadata.zip"));
        let platforms = parse::tests::platforms();

        let done = index.refresh(&platforms, false).await.unwrap();
        assert_eq!(done, Refreshed::Updated { games: 2 });
        assert!(!dir.path().join("Metadata.zip").exists());
        assert!(!dir.path().join("systems/psx.json").exists());
        let state = index.state();
        assert!(state.has_index());
        assert_eq!(state.etag.as_deref(), Some("\"v1\""));
        assert_eq!(state.systems, ["genesis", "nes"]);
        let nes = index.load("nes").unwrap();
        assert_eq!(nes.find("Super Mario Bros. 3").unwrap().boxes.len(), 3);
        assert!(index.load("psx").is_none());
        assert!(index.load("../state").is_none());

        // Fresh: nothing is asked.
        assert_eq!(
            index.refresh(&platforms, false).await.unwrap(),
            Refreshed::NotDue
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        // Asked anyway: LaunchBox says nothing changed.
        assert_eq!(
            index.refresh(&platforms, true).await.unwrap(),
            Refreshed::Unchanged
        );
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        assert_eq!(index.load("nes"), Some(nes));
        assert!(index.size() > 0);

        // A new platform table builds the files again, without the ETag.
        let mut more = platforms.clone();
        more.insert("Sony Playstation".into(), "psx".into());
        assert!(
            index
                .state()
                .is_due(&table_key(&more), now() + RETRY_AFTER.as_secs())
        );

        // Removed, nothing is left and the next refresh is due.
        index.remove().await.unwrap();
        assert_eq!(index.size(), 0);
        assert!(!index.state().has_index());
        assert!(index.load("nes").is_none());
        index.remove().await.unwrap();
    }

    #[tokio::test]
    async fn a_failed_download_waits_before_trying_again() {
        let server = TestServer::start(|_| Resp::status(500));
        let dir = tempfile::tempdir().unwrap();
        let index = Index::with_url(dir.path().to_path_buf(), server.url("/Metadata.zip"));
        let platforms = parse::tests::platforms();
        let err = index.refresh(&platforms, false).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("could not be downloaded"),
            "{err:#}"
        );
        let state = index.state();
        assert!(!state.has_index());
        assert!(state.attempted_at > 0);
        assert_eq!(
            index.refresh(&platforms, false).await.unwrap(),
            Refreshed::NotDue
        );
        assert_eq!(server.requests().len(), 1);
        assert!(index.refresh(&platforms, true).await.is_err());
        assert_eq!(server.requests().len(), 2);
    }

    #[tokio::test]
    async fn an_archive_without_the_game_list_is_refused() {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            w.start_file("Other.xml", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(b"<x/>").unwrap();
            w.finish().unwrap();
        }
        let body = buf.into_inner();
        let server = TestServer::start(move |_| Resp {
            status: 200,
            body: body.clone(),
            ..Default::default()
        });
        let dir = tempfile::tempdir().unwrap();
        let index = Index::with_url(dir.path().to_path_buf(), server.url("/Metadata.zip"));
        let err = index
            .refresh(&parse::tests::platforms(), true)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("no game list"), "{err:#}");
        assert!(!dir.path().join("Metadata.zip").exists());
        assert!(!index.state().has_index());
    }

    #[test]
    fn due_after_a_week_or_a_new_table() {
        let state = IndexState {
            updated_at: 1_000,
            attempted_at: 1_000,
            table: "t".into(),
            ..Default::default()
        };
        assert!(!state.is_due("t", 1_000 + 3600));
        assert!(state.is_due("t", 1_000 + REFRESH_AFTER.as_secs()));
        // A new table, but a download was tried an hour ago.
        assert!(!state.is_due("u", 1_000 + 3600));
        assert!(state.is_due("u", 1_000 + RETRY_AFTER.as_secs()));
        assert!(IndexState::default().is_due("t", 1));
    }

    #[test]
    fn box_urls_point_at_launchboxs_image_server() {
        let art = BoxArt {
            file: "a-b.png".into(),
            region: None,
            reconstructed: false,
        };
        assert_eq!(art.url(), "https://images.launchbox-app.com/a-b.png");
    }
}
