//! Where devices exchange their sync files: a folder another program keeps
//! in sync, a WebDAV folder, or an rclone remote ([`super::rclone`]). Each device writes only its own file, so
//! the sync programs never have two versions of one file to choose from.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use super::rclone::{self, Rclone, Scratch};

/// The folder Irori uses inside the chosen one.
pub const DIR: &str = "Irori sync";

/// A WebDAV login, kept with `irori_secrets`.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WebDavLogin {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for WebDavLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebDavLogin")
            .field("username", &self.username)
            .field("password", &"…")
            .finish()
    }
}

/// Where the sync files are.
#[derive(Debug, Clone)]
pub enum Target {
    Folder(PathBuf),
    WebDav {
        url: String,
        login: WebDavLogin,
    },
    /// `remote` is an rclone remote with an optional path (`gdrive:Games`).
    Rclone {
        rclone: Rclone,
        remote: String,
    },
}

/// Whether `name` is a device file (`<id>.json`, or a copy a sync program
/// made of one when two versions met, such as Syncthing's
/// `<id>.sync-conflict-….json`).
pub fn is_device_file(name: &str) -> bool {
    name.ends_with(".json")
        && !name.starts_with('.')
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
}

impl Target {
    /// The names of every device file there.
    pub async fn list(&self) -> Result<Vec<String>> {
        match self {
            Target::Folder(root) => {
                let dir = folder_dir(root)?;
                let mut names = Vec::new();
                let mut entries = match tokio::fs::read_dir(&dir).await {
                    Ok(entries) => entries,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(names),
                    Err(e) => {
                        return Err(e).with_context(|| format!("Cannot read {}", dir.display()));
                    }
                };
                while let Some(entry) = entries.next_entry().await? {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if is_device_file(&name) {
                        names.push(name);
                    }
                }
                names.sort();
                Ok(names)
            }
            Target::WebDav { url, login } => {
                let response = dav(
                    reqwest::Method::from_bytes(b"PROPFIND").expect("a valid method"),
                    &dir_url(url),
                    login,
                )
                .header("Depth", "1")
                .header("Content-Type", "application/xml")
                .body(PROPFIND)
                .send()
                .await
                .map_err(|e| without_url(e, "The WebDAV server can’t be reached"))?;
                if response.status() == reqwest::StatusCode::NOT_FOUND {
                    return Ok(Vec::new());
                }
                let text = checked(response).await?.text().await?;
                let mut names: Vec<String> = hrefs(&text)
                    .into_iter()
                    .filter_map(|href| last_segment(&href))
                    .filter(|name| is_device_file(name))
                    .collect();
                names.sort();
                names.dedup();
                Ok(names)
            }
            Target::Rclone { rclone, remote } => {
                let mut names: Vec<String> = rclone
                    .list(&rclone::remote_path(remote, DIR))
                    .await?
                    .into_iter()
                    .filter(|e| !e.is_dir && is_device_file(&e.name))
                    .map(|e| e.name)
                    .collect();
                names.sort();
                Ok(names)
            }
        }
    }

    /// The contents of one device file.
    pub async fn read(&self, name: &str) -> Result<Vec<u8>> {
        match self {
            Target::Folder(root) => {
                let path = folder_dir(root)?.join(name);
                tokio::fs::read(&path)
                    .await
                    .with_context(|| format!("Cannot read {}", path.display()))
            }
            Target::WebDav { url, login } => {
                let response = dav(reqwest::Method::GET, &file_url(url, name), login)
                    .send()
                    .await
                    .map_err(|e| without_url(e, "The WebDAV server can’t be reached"))?;
                Ok(checked(response).await?.bytes().await?.to_vec())
            }
            Target::Rclone { rclone, remote } => {
                let scratch = Scratch::new()?;
                rclone
                    .download(
                        &rclone::remote_path(remote, &format!("{DIR}/{name}")),
                        scratch.path(),
                    )
                    .await?;
                tokio::fs::read(scratch.path())
                    .await
                    .context("Cannot read what rclone fetched")
            }
        }
    }

    /// Replace this device's file.
    pub async fn write(&self, name: &str, bytes: Vec<u8>) -> Result<()> {
        match self {
            Target::Folder(root) => {
                let dir = folder_dir(root)?;
                let path = dir.join(name);
                tokio::task::spawn_blocking(move || {
                    std::fs::create_dir_all(&dir)?;
                    irori_core::json_store::write_bytes(&path, &bytes)
                        .with_context(|| format!("Cannot write {}", path.display()))
                })
                .await?
            }
            Target::WebDav { url, login } => {
                // Make the folder; servers answer 405 when it exists.
                let mkcol = reqwest::Method::from_bytes(b"MKCOL").expect("a valid method");
                let made = dav(mkcol, &dir_url(url), login)
                    .send()
                    .await
                    .map_err(|e| without_url(e, "The WebDAV server can’t be reached"))?;
                let status = made.status();
                if !status.is_success() && status != reqwest::StatusCode::METHOD_NOT_ALLOWED {
                    checked(made).await?;
                }
                let response = dav(reqwest::Method::PUT, &file_url(url, name), login)
                    .header("Content-Type", "application/json")
                    .body(bytes)
                    .send()
                    .await
                    .map_err(|e| without_url(e, "The WebDAV server can’t be reached"))?;
                checked(response).await?;
                Ok(())
            }
            Target::Rclone { rclone, remote } => {
                let scratch = Scratch::new()?;
                tokio::fs::write(scratch.path(), &bytes)
                    .await
                    .context("Cannot prepare the file for rclone")?;
                rclone
                    .upload(
                        scratch.path(),
                        &rclone::remote_path(remote, &format!("{DIR}/{name}")),
                    )
                    .await
            }
        }
    }
}

/// One entry of a folder at the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Files by path relative to Irori's folder there (`devices/<id>/backups/
/// a.json`), for the cloud copies ([`super::copies`]). Paths use `/` and
/// are made by Irori from plain names.
impl Target {
    /// What a folder holds; empty when it doesn't exist.
    pub async fn list_dir(&self, rel: &str) -> Result<Vec<RemoteEntry>> {
        match self {
            Target::Folder(root) => {
                let dir = folder_dir(root)?.join(rel);
                let mut out = Vec::new();
                let mut entries = match tokio::fs::read_dir(&dir).await {
                    Ok(entries) => entries,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
                    Err(e) => {
                        return Err(e).with_context(|| format!("Cannot read {}", dir.display()));
                    }
                };
                while let Some(entry) = entries.next_entry().await? {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') || name.ends_with(".irori-part") {
                        continue;
                    }
                    let is_dir = entry.file_type().await?.is_dir();
                    out.push(RemoteEntry { name, is_dir });
                }
                out.sort_by(|a, b| a.name.cmp(&b.name));
                Ok(out)
            }
            Target::WebDav { url, login } => {
                let dir = format!("{}{}/", dir_url(url), encode_path(rel));
                let response = dav(
                    reqwest::Method::from_bytes(b"PROPFIND").expect("a valid method"),
                    &dir,
                    login,
                )
                .header("Depth", "1")
                .header("Content-Type", "application/xml")
                .body(PROPFIND)
                .send()
                .await
                .map_err(|e| without_url(e, "The WebDAV server can’t be reached"))?;
                if response.status() == reqwest::StatusCode::NOT_FOUND {
                    return Ok(Vec::new());
                }
                let text = checked(response).await?.text().await?;
                Ok(entries_of(&text, &dir))
            }
            Target::Rclone { rclone, remote } => Ok(rclone
                .list(&rclone::remote_path(remote, &format!("{DIR}/{rel}")))
                .await?
                .into_iter()
                .map(|e| RemoteEntry {
                    name: e.name,
                    is_dir: e.is_dir,
                })
                .collect()),
        }
    }

    /// Copy the local file `from` to `rel`, making its folders.
    pub async fn put_file(&self, rel: &str, from: &Path) -> Result<()> {
        match self {
            Target::Folder(root) => {
                let to = folder_dir(root)?.join(rel);
                let from = from.to_path_buf();
                tokio::task::spawn_blocking(move || -> Result<()> {
                    if let Some(parent) = to.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    // Through a sibling, so a sync program never sees half
                    // a file.
                    let mut part = to.clone().into_os_string();
                    part.push(".irori-part");
                    std::fs::copy(&from, &part)
                        .with_context(|| format!("Cannot write {}", to.display()))?;
                    std::fs::rename(&part, &to)?;
                    Ok(())
                })
                .await?
            }
            Target::WebDav { url, login } => {
                let mut made = dir_url(url);
                mkcol(&made, login).await?;
                let parts: Vec<&str> = rel.split('/').collect();
                for part in &parts[..parts.len().saturating_sub(1)] {
                    made = format!("{made}{}/", encode(part));
                    mkcol(&made, login).await?;
                }
                let file = tokio::fs::File::open(from)
                    .await
                    .with_context(|| format!("Cannot read {}", from.display()))?;
                let response = dav(
                    reqwest::Method::PUT,
                    &format!("{}{}", dir_url(url), encode_path(rel)),
                    login,
                )
                .body(reqwest::Body::from(file))
                .send()
                .await
                .map_err(|e| without_url(e, "The WebDAV server can’t be reached"))?;
                checked(response).await?;
                Ok(())
            }
            Target::Rclone { rclone, remote } => {
                rclone
                    .upload(from, &rclone::remote_path(remote, &format!("{DIR}/{rel}")))
                    .await
            }
        }
    }

    /// Copy `rel` to the local file `to`, making its folders.
    pub async fn get_file(&self, rel: &str, to: &Path) -> Result<()> {
        if let Some(parent) = to.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("Cannot create {}", parent.display()))?;
        }
        match self {
            Target::Folder(root) => {
                let from = folder_dir(root)?.join(rel);
                tokio::fs::copy(&from, to)
                    .await
                    .with_context(|| format!("Cannot copy {}", from.display()))?;
                Ok(())
            }
            Target::WebDav { url, login } => {
                use tokio::io::AsyncWriteExt as _;
                let response = dav(
                    reqwest::Method::GET,
                    &format!("{}{}", dir_url(url), encode_path(rel)),
                    login,
                )
                .send()
                .await
                .map_err(|e| without_url(e, "The WebDAV server can’t be reached"))?;
                let mut response = checked(response).await?;
                let mut file = tokio::fs::File::create(to)
                    .await
                    .with_context(|| format!("Cannot write {}", to.display()))?;
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .map_err(|e| without_url(e, "The download stopped"))?
                {
                    file.write_all(&chunk).await?;
                }
                file.flush().await?;
                Ok(())
            }
            Target::Rclone { rclone, remote } => {
                rclone
                    .download(&rclone::remote_path(remote, &format!("{DIR}/{rel}")), to)
                    .await
            }
        }
    }

    /// A key naming this target, so what was sent to one isn't taken as
    /// sent to another.
    pub fn key(&self) -> String {
        match self {
            Target::Folder(root) => format!("folder:{}", root.display()),
            Target::WebDav { url, login } => format!("webdav:{}@{url}", login.username),
            Target::Rclone { remote, .. } => format!("rclone:{remote}"),
        }
    }
}

/// Make a WebDAV folder; servers answer 405 when it exists.
async fn mkcol(url: &str, login: &WebDavLogin) -> Result<()> {
    let mkcol = reqwest::Method::from_bytes(b"MKCOL").expect("a valid method");
    let made = dav(mkcol, url, login)
        .send()
        .await
        .map_err(|e| without_url(e, "The WebDAV server can’t be reached"))?;
    let status = made.status();
    if !status.is_success() && status != reqwest::StatusCode::METHOD_NOT_ALLOWED {
        checked(made).await?;
    }
    Ok(())
}

/// Percent-encode every segment of a relative path.
fn encode_path(rel: &str) -> String {
    rel.split('/').map(encode).collect::<Vec<_>>().join("/")
}

/// The entries of a PROPFIND answer for the folder at `dir`, without the
/// folder itself.
fn entries_of(xml: &str, dir: &str) -> Vec<RemoteEntry> {
    let own = dir.trim_end_matches('/').rsplit('/').next().map(decode);
    let mut out: Vec<RemoteEntry> = hrefs(xml)
        .into_iter()
        .filter_map(|href| {
            let path = href.split(['?', '#']).next()?.to_string();
            let is_dir = path.ends_with('/');
            let name = decode(path.trim_end_matches('/').rsplit('/').next()?);
            Some(RemoteEntry { name, is_dir })
        })
        .filter(|e| !(e.is_dir && Some(&e.name) == own.as_ref()) && !e.name.is_empty())
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup();
    out
}

/// Irori's folder inside the chosen one. The chosen folder must exist: a
/// share that isn't mounted or a drive of another system that isn't there
/// is reported, not created anew.
fn folder_dir(root: &Path) -> Result<PathBuf> {
    if root.as_os_str().is_empty() {
        bail!("Choose a sync folder first");
    }
    if !root.is_dir() {
        bail!("The sync folder {} isn’t available", root.display());
    }
    Ok(root.join(DIR))
}

const PROPFIND: &str = r#"<?xml version="1.0" encoding="utf-8"?><propfind xmlns="DAV:"><prop><resourcetype/></prop></propfind>"#;

fn dav(method: reqwest::Method, url: &str, login: &WebDavLogin) -> reqwest::RequestBuilder {
    let mut request = irori_core::http::client().request(method, url);
    if !login.username.is_empty() {
        request = request.basic_auth(&login.username, Some(&login.password));
    }
    request
}

/// The error of a response that failed, in words for the user.
async fn checked(response: reqwest::Response) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    match status.as_u16() {
        401 | 403 => bail!("The WebDAV server refused the login"),
        404 | 409 => bail!("The WebDAV folder doesn’t exist"),
        507 => bail!("The WebDAV server is full"),
        _ => bail!("The WebDAV server answered {status}"),
    }
}

/// A request error without its address, which may name the user's server
/// and folder layout in shared logs.
fn without_url(e: reqwest::Error, what: &str) -> anyhow::Error {
    anyhow::anyhow!("{what}: {}", e.without_url())
}

/// The address of Irori's folder inside the chosen WebDAV folder.
pub fn dir_url(base: &str) -> String {
    format!("{}/{}/", base.trim_end_matches('/'), encode(DIR))
}

fn file_url(base: &str, name: &str) -> String {
    format!("{}{}", dir_url(base), encode(name))
}

/// Percent-encode one path segment.
fn encode(segment: &str) -> String {
    let mut out = String::new();
    for b in segment.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn decode(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = segment.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `href` values of a PROPFIND answer, whatever prefix the server gives
/// the DAV namespace.
pub fn hrefs(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(open) = rest.find('<') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('>') else { break };
        let tag = &rest[..close];
        rest = &rest[close + 1..];
        let name = tag.split_whitespace().next().unwrap_or("");
        let local = name.rsplit(':').next().unwrap_or(name);
        if !local.eq_ignore_ascii_case("href") || tag.ends_with('/') {
            continue;
        }
        let end = rest.find('<').unwrap_or(rest.len());
        let text = rest[..end].trim();
        out.push(
            text.replace("&amp;", "&")
                .replace("&lt;", "<")
                .replace("&gt;", ">"),
        );
        rest = &rest[end..];
    }
    out
}

/// The file name an `href` ends with; `None` for a folder.
fn last_segment(href: &str) -> Option<String> {
    let path = href.split(['?', '#']).next()?;
    if path.ends_with('/') {
        return None;
    }
    path.rsplit('/').next().map(decode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_file_names_from_propfind_answers() {
        // Nextcloud names the namespace `d:`, others `D:` or nothing.
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:">
 <d:response><d:href>/remote.php/dav/files/me/Irori/Irori%20sync/</d:href></d:response>
 <d:response><d:href>/remote.php/dav/files/me/Irori/Irori%20sync/0a1b2c.json</d:href></d:response>
 <d:response><d:href>/remote.php/dav/files/me/Irori/Irori%20sync/0a1b2c.json.tmp</d:href></d:response>
</d:multistatus>
<D:response><D:href>https://dav.example/Irori%20sync/ff00.sync-conflict-20261104.json</D:href></D:response>
<response><href >/x/.hidden.json</href></response>"#;
        let names: Vec<String> = hrefs(xml)
            .iter()
            .filter_map(|h| last_segment(h))
            .filter(|n| is_device_file(n))
            .collect();
        assert_eq!(names, ["0a1b2c.json", "ff00.sync-conflict-20261104.json"]);
    }

    #[test]
    fn builds_addresses_inside_the_chosen_folder() {
        assert_eq!(
            dir_url("https://dav.example/files/me/"),
            "https://dav.example/files/me/Irori%20sync/"
        );
        assert_eq!(
            file_url("https://dav.example/files/me", "ab12.json"),
            "https://dav.example/files/me/Irori%20sync/ab12.json"
        );
        assert_eq!(decode("Irori%20sync"), "Irori sync");
    }

    #[test]
    fn the_password_stays_out_of_logs() {
        let login = WebDavLogin {
            username: "me".into(),
            password: "hunter2".into(),
        };
        assert!(!format!("{login:?}").contains("hunter2"));
    }

    #[test]
    fn lists_a_webdav_folder_without_itself() {
        let xml = r#"<d:multistatus xmlns:d="DAV:">
 <d:response><d:href>/dav/Irori%20sync/devices/ab/backups/</d:href></d:response>
 <d:response><d:href>/dav/Irori%20sync/devices/ab/backups/irori-2026.json</d:href></d:response>
 <d:response><d:href>/dav/Irori%20sync/devices/ab/backups/old/</d:href></d:response>
</d:multistatus>"#;
        let entries = entries_of(
            xml,
            "https://dav.example/dav/Irori%20sync/devices/ab/backups/",
        );
        assert_eq!(
            entries,
            [
                RemoteEntry {
                    name: "irori-2026.json".into(),
                    is_dir: false
                },
                RemoteEntry {
                    name: "old".into(),
                    is_dir: true
                }
            ]
        );
        assert_eq!(encode_path("devices/a b/x.json"), "devices/a%20b/x.json");
    }

    #[tokio::test]
    async fn moves_files_in_and_out_of_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let target = Target::Folder(dir.path().to_path_buf());
        assert!(target.list_dir("devices").await.unwrap().is_empty());
        let local = dir.path().join("shot.png");
        std::fs::write(&local, b"png").unwrap();
        target
            .put_file("devices/ab/screenshots/x/shot.png", &local)
            .await
            .unwrap();
        let listed = target.list_dir("devices/ab/screenshots").await.unwrap();
        assert_eq!(
            listed,
            [RemoteEntry {
                name: "x".into(),
                is_dir: true
            }]
        );
        let back = dir.path().join("back/shot.png");
        target
            .get_file("devices/ab/screenshots/x/shot.png", &back)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&back).unwrap(), b"png");
    }

    #[tokio::test]
    async fn a_missing_folder_is_reported_not_created() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("unmounted");
        let target = Target::Folder(gone.clone());
        assert!(target.list().await.is_err());
        assert!(target.write("a.json", b"{}".to_vec()).await.is_err());
        assert!(!gone.exists());

        let target = Target::Folder(dir.path().to_path_buf());
        assert!(target.list().await.unwrap().is_empty());
        target.write("a.json", b"{}".to_vec()).await.unwrap();
        assert_eq!(target.list().await.unwrap(), ["a.json"]);
        assert_eq!(target.read("a.json").await.unwrap(), b"{}");
    }
}
