//! What the Xbox store keeps on disk: the installed games
//! (`installed.json` in its config folder), the library last fetched
//! (`library.json` in its cache) and, beside the credentials, the offline
//! licences (`licenses/`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use irori_core::{InstallPlatform, InstalledInfo, json_store};
use serde::{Deserialize, Serialize};

use crate::library::Entry;

/// Where the store's files live.
#[derive(Debug, Clone)]
pub struct Layout {
    pub config: PathBuf,
    pub cache: PathBuf,
}

impl Layout {
    pub fn credentials_file(&self) -> PathBuf {
        self.config.join("credentials.json")
    }

    pub fn installed_file(&self) -> PathBuf {
        self.config.join("installed.json")
    }

    pub fn library_file(&self) -> PathBuf {
        self.cache.join("library.json")
    }

    /// Device-bound offline licences.
    pub fn licenses_dir(&self) -> PathBuf {
        self.config.join("licenses")
    }

    /// Private folders of the account services of running games.
    pub fn service_dir(&self) -> PathBuf {
        self.cache.join("service")
    }
}

/// An installed game.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InstalledRecord {
    pub title: String,
    pub install_path: String,
    /// The package's content ID.
    pub content_id: String,
    /// The package version the files are from.
    pub version: String,
    /// The program that starts the game, as the package names it.
    pub executable: String,
    /// The Xbox title ID (decimal), when the manifest names one.
    pub title_id: Option<u32>,
    pub size: u64,
    /// The user keeps this version: no update checks.
    pub pinned_version: bool,
    /// Add-ons installed into their own folders, by product ID.
    pub content: BTreeMap<String, String>,
}

impl InstalledRecord {
    pub fn to_installed_info(&self) -> InstalledInfo {
        InstalledInfo {
            install_path: self.install_path.clone(),
            executable: self.executable.clone(),
            install_size: irori_core::util::format_bytes(self.size),
            install_size_bytes: self.size,
            version: self.version.clone(),
            platform: InstallPlatform::Windows,
            pinned_version: self.pinned_version,
            installed_dlcs: self.content.keys().cloned().collect(),
            ..Default::default()
        }
    }

    pub fn root(&self) -> &Path {
        Path::new(&self.install_path)
    }
}

/// The library as last fetched: the market it was read for, and its entries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryCache {
    pub market: String,
    pub entries: BTreeMap<String, Entry>,
}

pub fn load_installed(layout: &Layout) -> BTreeMap<String, InstalledRecord> {
    json_store::read_or_default(&layout.installed_file())
}

pub fn save_installed(
    layout: &Layout,
    installed: &BTreeMap<String, InstalledRecord>,
) -> Result<()> {
    json_store::write(&layout.installed_file(), installed)
}

pub fn load_library(layout: &Layout) -> LibraryCache {
    json_store::read_or_default(&layout.library_file())
}

pub fn save_library(layout: &Layout, library: &LibraryCache) -> Result<()> {
    json_store::write(&layout.library_file(), library)
}
