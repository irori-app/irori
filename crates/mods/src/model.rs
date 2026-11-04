//! What `mods.json` holds: a game's mods in load order and how they apply.

use serde::{Deserialize, Serialize};

/// A game's mods. Order is load order: a mod later in the list wins a file
/// that several mods provide.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModList {
    #[serde(default)]
    pub method: DeployMethod,
    #[serde(default)]
    pub mods: Vec<ModEntry>,
    /// The Thunderstore community the user chose for this game, when the
    /// detected one is wrong or missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thunderstore_community: Option<String>,
    /// The Nexus Mods game (`skyrimspecialedition`) the user chose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nexus_domain: Option<String>,
}

impl ModList {
    pub fn get(&self, id: &str) -> Option<&ModEntry> {
        self.mods.iter().find(|m| m.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut ModEntry> {
        self.mods.iter_mut().find(|m| m.id == id)
    }

    /// The installed mod that came from the same place as `source`.
    pub fn find_source(&self, source: &ModSource) -> Option<&ModEntry> {
        let key = source.key()?;
        self.mods
            .iter()
            .find(|m| m.source.key().as_ref() == Some(&key))
    }

    pub fn any_enabled(&self) -> bool {
        self.mods.iter().any(|m| m.enabled)
    }

    /// Put the mods in the order of `ids`. Ids it doesn't name keep their
    /// relative order after the named ones; unknown ids are ignored.
    pub fn reorder(&mut self, ids: &[String]) {
        let mut ordered = Vec::with_capacity(self.mods.len());
        for id in ids {
            if let Some(ix) = self.mods.iter().position(|m| &m.id == id) {
                ordered.push(self.mods.remove(ix));
            }
        }
        ordered.append(&mut self.mods);
        self.mods = ordered;
    }

    /// Move the mod `id` to position `to` (clamped). Returns whether it moved.
    pub fn move_to(&mut self, id: &str, to: usize) -> bool {
        let Some(from) = self.mods.iter().position(|m| m.id == id) else {
            return false;
        };
        let to = to.min(self.mods.len() - 1);
        if from == to {
            return false;
        }
        let entry = self.mods.remove(from);
        self.mods.insert(to, entry);
        true
    }
}

/// How mods are applied while the game runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeployMethod {
    /// An overlay where the system offers one (Linux with
    /// `fuse-overlayfs`), else links.
    #[default]
    Auto,
    /// Always links recorded in a manifest.
    Links,
}

/// One installed mod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModEntry {
    /// Folder name under the game's mod folder (see [`is_valid_id`]).
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub source: ModSource,
    pub enabled: bool,
    /// RFC 3339.
    #[serde(default)]
    pub installed_at: String,
    /// Bytes on disk.
    #[serde(default)]
    pub size: u64,
    /// Number of files.
    #[serde(default)]
    pub files: usize,
    /// Thunderstore packages this one needs, as `Namespace-Name`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
}

/// Where a mod came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ModSource {
    /// A file the user picked.
    #[serde(rename_all = "camelCase")]
    Archive { file_name: String },
    #[serde(rename_all = "camelCase")]
    Thunderstore {
        community: String,
        namespace: String,
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    Nexus {
        domain: String,
        mod_id: u64,
        file_id: u64,
    },
}

impl ModSource {
    /// The mod's page on its site.
    pub fn page_url(&self) -> Option<String> {
        match self {
            ModSource::Archive { .. } => None,
            ModSource::Thunderstore {
                community,
                namespace,
                name,
            } => Some(format!(
                "https://thunderstore.io/c/{community}/p/{namespace}/{name}/"
            )),
            ModSource::Nexus { domain, mod_id, .. } => {
                Some(format!("https://www.nexusmods.com/{domain}/mods/{mod_id}"))
            }
        }
    }

    /// What makes two installs the same mod (a newer version replaces the
    /// older one); `None` for archives, which may be anything.
    pub fn key(&self) -> Option<String> {
        match self {
            ModSource::Archive { .. } => None,
            ModSource::Thunderstore {
                namespace, name, ..
            } => Some(format!("ts:{}-{}", namespace, name).to_ascii_lowercase()),
            ModSource::Nexus { domain, mod_id, .. } => Some(format!("nexus:{domain}:{mod_id}")),
        }
    }
}

/// Whether `id` can name a mod folder: lower-case letters, digits and `-`,
/// so it never collides with `mods.json` or the dot folders beside it.
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A fresh id for a mod called `name`: its slug plus a random suffix.
pub(crate) fn new_id(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 40 {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "mod" } else { slug };
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    format!("{slug}-{}", &suffix[..6])
}
