//! Thunderstore (thunderstore.io): per-game communities of mods for Unity
//! and other games (Lethal Company, Valheim, Risk of Rain 2…), installed
//! with their dependencies.
//!
//! Which games have a community, how to recognise them (Steam and Epic ids,
//! program names) and where each kind of file goes come from Thunderstore's
//! published ecosystem schema, the data r2modman uses. It is fetched at
//! most once a day and kept on disk in a compact form. Searching uses the
//! listing API the site itself uses; a package's versions and dependencies
//! come from the public package API.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result, bail};
use irori_core::json_store;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The site; tests point it elsewhere.
pub const BASE: &str = "https://thunderstore.io";

/// Mod loaders whose install rules Irori follows. Others need loader
/// specific steps (MelonLoader's recursive variant, Northstar…).
const SUPPORTED_LOADERS: &[&str] = &["bepinex", "melonloader"];

/// How long the ecosystem schema is trusted.
const SCHEMA_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Package files that describe the package rather than belong to the game.
const PACKAGE_META: &[&str] = &[
    "manifest.json",
    "icon.png",
    "readme.md",
    "changelog.md",
    "license",
    "license.md",
    "license.txt",
];

/// The communities Thunderstore hosts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ecosystem {
    pub communities: Vec<Community>,
    pub loader_packages: Vec<LoaderPackage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Community {
    /// Thunderstore's id (`lethal-company`).
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub steam_ids: Vec<String>,
    #[serde(default)]
    pub epic_ids: Vec<String>,
    /// Program names of the game (`Lethal Company.exe`).
    #[serde(default)]
    pub exe_names: Vec<String>,
    #[serde(default)]
    pub search_names: Vec<String>,
    /// `bepinex`, `melonloader`…
    #[serde(default)]
    pub loader: String,
    #[serde(default)]
    pub rules: Vec<InstallRule>,
}

impl Community {
    /// Whether Irori knows how to install this community's packages.
    pub fn supported(&self) -> bool {
        SUPPORTED_LOADERS.contains(&self.loader.as_str()) && !self.rules.is_empty()
    }
}

/// Where one kind of file goes, relative to the game folder.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallRule {
    /// `BepInEx/plugins`.
    pub route: String,
    /// Loose files with these endings go here (`.dll`).
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Each package gets a folder of its own under the route.
    #[serde(default)]
    pub per_package: bool,
    /// Files no other rule takes go here.
    #[serde(default)]
    pub default: bool,
}

/// A package that is a mod loader itself (BepInExPack): its `root_folder`
/// is unpacked at the game's root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderPackage {
    /// `Namespace-Name`.
    pub package: String,
    pub root_folder: String,
    pub loader: String,
}

/// Parse Thunderstore's ecosystem schema.
pub fn parse_ecosystem(schema: &Value) -> Ecosystem {
    let mut out = Ecosystem::default();
    if let Some(games) = schema.get("games").and_then(Value::as_object) {
        for (label, game) in games {
            // The first entry is the game; later ones are its servers.
            let Some(r2) = game
                .get("r2modman")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
            else {
                continue;
            };
            let strings = |v: Option<&Value>| -> Vec<String> {
                v.and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let ids = |platform: &str| -> Vec<String> {
                r2.get("distributions")
                    .and_then(Value::as_array)
                    .map(|d| {
                        d.iter()
                            .filter(|x| x.get("platform").and_then(Value::as_str) == Some(platform))
                            .filter_map(|x| x.get("identifier").and_then(Value::as_str))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let rules = r2
                .get("installRules")
                .and_then(Value::as_array)
                .map(|rules| {
                    rules
                        .iter()
                        .filter_map(|r| {
                            Some(InstallRule {
                                route: r.get("route")?.as_str()?.trim_matches('/').to_string(),
                                extensions: strings(r.get("defaultFileExtensions"))
                                    .into_iter()
                                    .map(|e| e.to_ascii_lowercase())
                                    .collect(),
                                per_package: r.get("trackingMethod").and_then(Value::as_str)
                                    == Some("subdir"),
                                default: r.get("isDefaultLocation").and_then(Value::as_bool)
                                    == Some(true),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            out.communities.push(Community {
                id: label.clone(),
                name: r2
                    .pointer("/meta/displayName")
                    .and_then(Value::as_str)
                    .unwrap_or(label)
                    .to_string(),
                steam_ids: ids("steam"),
                epic_ids: ids("epic-games-store"),
                exe_names: strings(r2.get("exeNames")),
                search_names: strings(r2.get("additionalSearchStrings")),
                loader: r2
                    .get("packageLoader")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                rules,
            });
        }
    }
    out.communities
        .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    if let Some(loaders) = schema.get("modloaderPackages").and_then(Value::as_array) {
        for l in loaders {
            let (Some(package), Some(root), Some(loader)) = (
                l.get("packageId").and_then(Value::as_str),
                l.get("rootFolder").and_then(Value::as_str),
                l.get("loader").and_then(Value::as_str),
            ) else {
                continue;
            };
            out.loader_packages.push(LoaderPackage {
                package: package.to_string(),
                root_folder: root.trim_matches('/').to_string(),
                loader: loader.to_string(),
            });
        }
    }
    out
}

/// The ecosystem, from `cache` when it is less than a day old, else from
/// Thunderstore (falling back to an older `cache` when that fails).
pub async fn ecosystem(cache: &Path) -> Result<Ecosystem> {
    let fresh = std::fs::metadata(cache)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < SCHEMA_MAX_AGE);
    let cached = || json_store::read::<Ecosystem>(cache).ok().flatten();
    if fresh && let Some(eco) = cached() {
        return Ok(eco);
    }
    match fetch_ecosystem(BASE).await {
        Ok(eco) => {
            if let Err(e) = json_store::write(cache, &eco) {
                tracing::warn!("caching the Thunderstore schema: {e:#}");
            }
            Ok(eco)
        }
        Err(e) => cached().ok_or(e),
    }
}

pub async fn fetch_ecosystem(base: &str) -> Result<Ecosystem> {
    let url = format!("{base}/api/experimental/schema/dev/latest/");
    let schema: Value = get_json(&url)
        .await
        .context("Thunderstore's list of games could not be loaded")?;
    let eco = parse_ecosystem(&schema);
    if eco.communities.is_empty() {
        bail!("Thunderstore's list of games is empty");
    }
    Ok(eco)
}

async fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T> {
    let response = irori_core::http::client()
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Thunderstore could not be reached: {}", e.without_url()))?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        bail!("Thunderstore doesn't know this package");
    }
    if !status.is_success() {
        bail!("Thunderstore answered {status}");
    }
    response.json().await.map_err(|e| {
        anyhow::anyhow!(
            "Thunderstore's answer was not understood: {}",
            e.without_url()
        )
    })
}

/// Letters and digits only, lower case: titles compare without punctuation.
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// What Irori knows about a game, to find its community.
#[derive(Debug, Clone, Default)]
pub struct GameKeys<'a> {
    pub steam_id: Option<&'a str>,
    /// Epic app name and namespace.
    pub epic_ids: Vec<&'a str>,
    pub title: &'a str,
    pub game_dir: Option<&'a Path>,
}

impl Ecosystem {
    pub fn community(&self, id: &str) -> Option<&Community> {
        self.communities.iter().find(|c| c.id == id)
    }

    /// The community of a game: by store id, else by the game's program in
    /// its folder, else by title.
    pub fn find(&self, game: &GameKeys) -> Option<&Community> {
        if let Some(steam) = game.steam_id
            && let Some(c) = self
                .communities
                .iter()
                .find(|c| c.steam_ids.iter().any(|s| s == steam))
        {
            return Some(c);
        }
        if let Some(c) = self.communities.iter().find(|c| {
            c.epic_ids
                .iter()
                .any(|e| game.epic_ids.iter().any(|g| g.eq_ignore_ascii_case(e)))
        }) {
            return Some(c);
        }
        if let Some(dir) = game.game_dir {
            let names: HashSet<String> = std::fs::read_dir(dir)
                .map(|r| {
                    r.flatten()
                        .map(|e| e.file_name().to_string_lossy().to_lowercase())
                        .collect()
                })
                .unwrap_or_default();
            if let Some(c) = self.communities.iter().find(|c| {
                c.exe_names
                    .iter()
                    .any(|e| names.contains(&e.to_lowercase()))
            }) {
                return Some(c);
            }
        }
        let title = normalize(game.title);
        if title.is_empty() {
            return None;
        }
        self.communities.iter().find(|c| {
            normalize(&c.name) == title || c.search_names.iter().any(|s| normalize(s) == title)
        })
    }

    /// The loader package `full_name` (`Namespace-Name`) is, for `community`.
    pub fn loader_package(&self, community: &Community, full_name: &str) -> Option<&LoaderPackage> {
        self.loader_packages
            .iter()
            .find(|l| l.loader == community.loader && l.package.eq_ignore_ascii_case(full_name))
    }
}

/// Where the file `rel` of package `full_name` goes, relative to the game
/// folder; `None` leaves it out. Follows the community's install rules:
/// folders named after a route go to that route, loose files by their
/// ending, the rest to the default route; a loader package's root folder
/// goes to the game's root.
pub fn route(
    community: &Community,
    loader: Option<&LoaderPackage>,
    full_name: &str,
    rel: &str,
) -> Option<String> {
    let lower = rel.to_ascii_lowercase();
    if let Some(loader) = loader {
        let prefix = format!("{}/", loader.root_folder.to_ascii_lowercase());
        return lower
            .starts_with(&prefix)
            .then(|| rel[prefix.len()..].to_string())
            .filter(|r| !r.is_empty());
    }
    if !rel.contains('/') && PACKAGE_META.contains(&lower.as_str()) {
        return None;
    }
    let place = |rule: &InstallRule, rest: &str| {
        if rule.per_package {
            format!("{}/{full_name}/{rest}", rule.route)
        } else {
            format!("{}/{rest}", rule.route)
        }
    };
    // `BepInEx/plugins/X.dll` or just `plugins/X.dll`.
    for rule in &community.rules {
        let route = rule.route.to_ascii_lowercase();
        let last = route.rsplit('/').next().unwrap_or(&route);
        for prefix in [format!("{route}/"), format!("{last}/")] {
            if lower.starts_with(&prefix) && lower.len() > prefix.len() {
                return Some(place(rule, &rel[prefix.len()..]));
            }
        }
    }
    if !rel.contains('/') {
        let mut by_ending: Vec<(&InstallRule, &String)> = community
            .rules
            .iter()
            .flat_map(|r| r.extensions.iter().map(move |e| (r, e)))
            .collect();
        // `.mm.dll` before `.dll`.
        by_ending.sort_by_key(|(_, e)| std::cmp::Reverse(e.len()));
        if let Some((rule, _)) = by_ending.iter().find(|(_, e)| lower.ends_with(e.as_str())) {
            return Some(place(rule, rel));
        }
    }
    let default = community
        .rules
        .iter()
        .find(|r| r.default)
        .or(community.rules.first())?;
    Some(place(default, rel))
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PackageSummary {
    pub namespace: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub download_count: u64,
    #[serde(default)]
    pub rating_count: u64,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub last_updated: Option<String>,
    #[serde(default)]
    pub is_deprecated: bool,
    #[serde(default)]
    pub is_nsfw: bool,
}

impl PackageSummary {
    pub fn full_name(&self) -> String {
        format!("{}-{}", self.namespace, self.name)
    }

    /// The name as people write it: `MoreCompany`, `Lethal_Lib` → `Lethal Lib`.
    pub fn display_name(&self) -> String {
        self.name.replace('_', " ")
    }
}

/// One page of search results.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchPage {
    pub total: u64,
    pub results: Vec<PackageSummary>,
    pub has_more: bool,
}

/// Search `community`, most downloaded first. Pages start at 1.
pub async fn search(base: &str, community: &str, query: &str, page: u32) -> Result<SearchPage> {
    let mut url = url::Url::parse(&format!("{base}/api/cyberstorm/listing/{community}/"))
        .context("bad Thunderstore address")?;
    url.query_pairs_mut()
        .append_pair("q", query.trim())
        .append_pair("ordering", "most-downloaded")
        .append_pair("page", &page.max(1).to_string());
    let body: Value = get_json(url.as_str()).await?;
    Ok(parse_search(&body))
}

pub fn parse_search(body: &Value) -> SearchPage {
    SearchPage {
        total: body.get("count").and_then(Value::as_u64).unwrap_or(0),
        results: body
            .get("results")
            .and_then(Value::as_array)
            .map(|r| {
                r.iter()
                    .filter_map(|p| serde_json::from_value::<PackageSummary>(p.clone()).ok())
                    .filter(|p| !p.is_nsfw)
                    .collect()
            })
            .unwrap_or_default(),
        has_more: body.get("next").is_some_and(|n| !n.is_null()),
    }
}

/// A package's latest version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub namespace: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon: Option<String>,
    /// `Namespace-Name-1.2.3`.
    pub dependencies: Vec<String>,
    pub download_url: String,
}

impl Package {
    pub fn full_name(&self) -> String {
        format!("{}-{}", self.namespace, self.name)
    }
}

pub async fn package(base: &str, namespace: &str, name: &str) -> Result<Package> {
    let url = format!("{base}/api/experimental/package/{namespace}/{name}/");
    let body: Value = get_json(&url).await?;
    parse_package(&body).context("Thunderstore's package details were not understood")
}

pub fn parse_package(body: &Value) -> Option<Package> {
    let latest = body.get("latest")?;
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let download_url = s(latest, "download_url")?;
    // Downloads must come from Thunderstore itself.
    let host = url::Url::parse(&download_url).ok()?.host_str()?.to_string();
    if host != "thunderstore.io" && !host.ends_with(".thunderstore.io") && host != "127.0.0.1" {
        return None;
    }
    Some(Package {
        namespace: s(body, "namespace")?,
        name: s(body, "name")?,
        version: s(latest, "version_number").unwrap_or_default(),
        description: s(latest, "description").unwrap_or_default(),
        icon: s(latest, "icon"),
        dependencies: latest
            .get("dependencies")
            .and_then(Value::as_array)
            .map(|d| {
                d.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        download_url,
    })
}

/// `Namespace-Name-1.2.3` → (`Namespace`, `Name`).
pub fn split_dependency(dep: &str) -> Option<(String, String)> {
    let (rest, version) = dep.rsplit_once('-')?;
    if !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let (namespace, name) = rest.split_once('-')?;
    (!namespace.is_empty() && !name.is_empty()).then(|| (namespace.into(), name.into()))
}

/// The packages to install for `namespace-name`: its dependencies (deepest
/// first) and itself, leaving out those whose `Namespace-Name` (any case)
/// is in `installed`.
pub async fn resolve(
    base: &str,
    namespace: &str,
    name: &str,
    installed: &HashSet<String>,
) -> Result<Vec<Package>> {
    const MAX_PACKAGES: usize = 200;
    let mut order: Vec<Package> = Vec::new();
    let mut seen: HashSet<String> = installed.iter().map(|s| s.to_lowercase()).collect();
    // Depth-first, so each package follows what it needs.
    let mut stack: Vec<(String, String, bool)> = vec![(namespace.into(), name.into(), false)];
    let mut fetched: std::collections::HashMap<String, Package> = Default::default();
    while let Some((ns, n, expanded)) = stack.pop() {
        let key = format!("{ns}-{n}").to_lowercase();
        if expanded {
            if let Some(p) = fetched.remove(&key) {
                order.push(p);
            }
            continue;
        }
        if !seen.insert(key.clone()) {
            continue;
        }
        if seen.len() > MAX_PACKAGES + installed.len() {
            bail!("the mod needs more than {MAX_PACKAGES} packages");
        }
        let package = package(base, &ns, &n)
            .await
            .with_context(|| format!("loading {ns}-{n}"))?;
        stack.push((ns, n, true));
        for dep in package.dependencies.iter().rev() {
            if let Some((dns, dn)) = split_dependency(dep) {
                stack.push((dns, dn, false));
            }
        }
        fetched.insert(key, package);
    }
    Ok(order)
}
