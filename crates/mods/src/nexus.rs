//! Nexus Mods: the public API v1 with the user's personal API key.
//!
//! Premium accounts may ask for download links directly. Free accounts get
//! one from the site's "Mod manager download" button: it opens an `nxm://`
//! link carrying a key and expiry for that one file ([`NxmLink`]), which
//! Irori receives through its link handler.
//!
//! The key is a secret: it travels only in the `apikey` header, never in
//! addresses, logs or errors.

use std::fmt;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const API: &str = "https://api.nexusmods.com";

/// The user's API key. `Debug` never prints it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKey(pub String);

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(…)")
    }
}

/// The signed-in account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NexusUser {
    pub name: String,
    pub is_premium: bool,
}

/// A `nxm://<game>/mods/<mod>/files/<file>?key=…&expires=…` link.
#[derive(Clone, PartialEq, Eq)]
pub struct NxmLink {
    pub domain: String,
    pub mod_id: u64,
    pub file_id: u64,
    /// Present for free accounts; one download of this file.
    pub key: Option<String>,
    pub expires: Option<u64>,
}

impl fmt::Debug for NxmLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NxmLink")
            .field("domain", &self.domain)
            .field("mod_id", &self.mod_id)
            .field("file_id", &self.file_id)
            .field("key", &self.key.as_ref().map(|_| "…"))
            .field("expires", &self.expires)
            .finish()
    }
}

impl NxmLink {
    pub fn parse(link: &str) -> Option<Self> {
        let url = url::Url::parse(link.trim()).ok()?;
        if url.scheme() != "nxm" {
            return None;
        }
        let domain = url.host_str()?.to_ascii_lowercase();
        if !is_domain(&domain) {
            return None;
        }
        let parts: Vec<&str> = url.path().trim_matches('/').split('/').collect();
        let [mods, mod_id, files, file_id] = parts.as_slice() else {
            return None;
        };
        if *mods != "mods" || *files != "files" {
            return None;
        }
        let mut key = None;
        let mut expires = None;
        for (k, v) in url.query_pairs() {
            match k.as_ref() {
                "key" => key = Some(v.into_owned()),
                "expires" => expires = v.parse().ok(),
                _ => {}
            }
        }
        Some(Self {
            domain,
            mod_id: mod_id.parse().ok()?,
            file_id: file_id.parse().ok()?,
            key,
            expires,
        })
    }
}

/// A game's id on Nexus (`skyrimspecialedition`): letters, digits, `-`.
pub fn is_domain(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `https://www.nexusmods.com/<game>/mods/<id>` (any query or tab) →
/// (game, mod id).
pub fn parse_mod_url(text: &str) -> Option<(String, u64)> {
    let url = url::Url::parse(text.trim()).ok()?;
    let host = url.host_str()?;
    if host != "nexusmods.com" && !host.ends_with(".nexusmods.com") {
        return None;
    }
    let mut parts = url.path().trim_matches('/').split('/');
    let domain = parts.next()?.to_ascii_lowercase();
    (parts.next()? == "mods").then_some(())?;
    let id = parts.next()?.parse().ok()?;
    is_domain(&domain).then_some((domain, id))
}

/// A mod's details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModInfo {
    pub name: String,
    pub summary: String,
    pub version: String,
    pub author: String,
    pub picture: Option<String>,
}

/// One downloadable file of a mod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModFile {
    pub file_id: u64,
    pub name: String,
    pub version: String,
    /// `MAIN`, `OPTIONAL`, `UPDATE`, `MISCELLANEOUS`, `OLD_VERSION`…
    pub category: String,
    pub size_bytes: u64,
    pub file_name: String,
    pub uploaded: i64,
}

impl ModFile {
    /// Old and archived files are listed last and not offered by default.
    pub fn is_current(&self) -> bool {
        !matches!(
            self.category.as_str(),
            "OLD_VERSION" | "ARCHIVED" | "REMOVED"
        )
    }
}

/// Calls the API with one key.
#[derive(Debug, Clone)]
pub struct Client {
    base: String,
    key: ApiKey,
}

impl Client {
    pub fn new(key: ApiKey) -> Self {
        Self::with_base(API, key)
    }

    pub fn with_base(base: &str, key: ApiKey) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            key,
        }
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let response = irori_core::http::client()
            .get(format!("{}{path}", self.base))
            .header("apikey", &self.key.0)
            .header("Application-Name", "Irori")
            .header("Application-Version", env!("CARGO_PKG_VERSION"))
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Nexus Mods could not be reached: {}", e.without_url()))?;
        let status = response.status();
        match status.as_u16() {
            200..=299 => {}
            401 => bail!("Nexus Mods refused the API key"),
            403 => bail!(
                "Nexus Mods only gives this download to Premium members; use “Mod manager download” on the mod's page instead"
            ),
            404 => bail!("Nexus Mods doesn't have this mod or file"),
            410 => {
                bail!("the download link from Nexus Mods has expired; start it again on the site")
            }
            429 => bail!("Nexus Mods' hourly limit is reached; try again later"),
            _ => bail!("Nexus Mods answered {status}"),
        }
        response.json().await.map_err(|e| {
            anyhow::anyhow!("Nexus Mods' answer was not understood: {}", e.without_url())
        })
    }

    pub async fn validate(&self) -> Result<NexusUser> {
        let v = self.get("/v1/users/validate.json").await?;
        Ok(NexusUser {
            name: v
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            is_premium: v.get("is_premium").and_then(Value::as_bool) == Some(true),
        })
    }

    pub async fn mod_info(&self, domain: &str, mod_id: u64) -> Result<ModInfo> {
        check_domain(domain)?;
        let v = self
            .get(&format!("/v1/games/{domain}/mods/{mod_id}.json"))
            .await?;
        Ok(parse_mod_info(&v))
    }

    pub async fn files(&self, domain: &str, mod_id: u64) -> Result<Vec<ModFile>> {
        check_domain(domain)?;
        let v = self
            .get(&format!("/v1/games/{domain}/mods/{mod_id}/files.json"))
            .await?;
        Ok(parse_files(&v))
    }

    /// Where to download a file from. Free accounts need the key and expiry
    /// of an `nxm://` link.
    pub async fn download_url(&self, link: &NxmLink) -> Result<String> {
        check_domain(&link.domain)?;
        let mut path = format!(
            "/v1/games/{}/mods/{}/files/{}/download_link.json",
            link.domain, link.mod_id, link.file_id
        );
        if let (Some(key), Some(expires)) = (&link.key, link.expires) {
            let query: String = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("key", key)
                .append_pair("expires", &expires.to_string())
                .finish();
            path = format!("{path}?{query}");
        }
        let v = self.get(&path).await?;
        v.as_array()
            .and_then(|mirrors| mirrors.first())
            .and_then(|m| m.get("URI"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("Nexus Mods offered no download for this file")
    }
}

fn check_domain(domain: &str) -> Result<()> {
    if !is_domain(domain) {
        bail!("“{domain}” is not a Nexus Mods game");
    }
    Ok(())
}

pub fn parse_mod_info(v: &Value) -> ModInfo {
    let s = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    ModInfo {
        name: s("name"),
        summary: s("summary"),
        version: s("version"),
        author: s("author"),
        picture: v
            .get("picture_url")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

pub fn parse_files(v: &Value) -> Vec<ModFile> {
    let mut files: Vec<ModFile> = v
        .get("files")
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(|f| {
                    let s = |k: &str| {
                        f.get(k)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    Some(ModFile {
                        file_id: f.get("file_id").and_then(Value::as_u64)?,
                        name: s("name"),
                        version: s("version"),
                        category: s("category_name"),
                        size_bytes: f
                            .get("size_in_bytes")
                            .and_then(Value::as_u64)
                            .or_else(|| f.get("size_kb").and_then(Value::as_u64).map(|k| k * 1024))
                            .unwrap_or(0),
                        file_name: s("file_name"),
                        uploaded: f
                            .get("uploaded_timestamp")
                            .and_then(Value::as_i64)
                            .unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    // Current files first, newest first.
    files.sort_by_key(|f| (!f.is_current(), std::cmp::Reverse(f.uploaded)));
    files
}
