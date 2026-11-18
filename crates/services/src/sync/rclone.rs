//! rclone as a sync target: any remote the user set up in rclone (Google
//! Drive, Dropbox, OneDrive, SFTP, SMB, S3…) through its command line.
//!
//! Irori never bundles or downloads rclone and never handles the cloud
//! accounts: the user installs rclone and adds a remote with `rclone config`,
//! and Irori names the remote (`gdrive:` or `gdrive:Games`). rclone is found
//! at the program chosen in settings, on `PATH` or in the usual install
//! folders; inside Irori's Flatpak it is found and run on the host. Files
//! pass through temporary files in Irori's cache, which both sides see, so
//! nothing large goes through a pipe.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::host;
use irori_core::process::{self, CommandSpec, RunOptions};
use serde::Deserialize;

/// Where rclone's installation instructions are.
pub const INSTALL_URL: &str = "https://rclone.org/install/";

/// An rclone program to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rclone {
    program: PathBuf,
    /// Run on the host through `flatpak-spawn`, Irori being a Flatpak.
    on_host: bool,
}

/// One entry of a remote folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JsonEntry {
    name: String,
    #[serde(default)]
    size: i64,
    #[serde(default)]
    is_dir: bool,
}

/// Usual install folders that aren't always on `PATH`.
fn known_locations(home: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if cfg!(target_os = "macos") {
        paths.extend([
            PathBuf::from("/opt/homebrew/bin/rclone"),
            PathBuf::from("/usr/local/bin/rclone"),
        ]);
    } else if cfg!(windows) {
        if let Some(local) = dirs::data_local_dir() {
            paths.push(local.join("Microsoft/WinGet/Links/rclone.exe"));
        }
        paths.push(home.join("scoop/shims/rclone.exe"));
    } else {
        paths.extend([home.join(".local/bin/rclone"), home.join("bin/rclone")]);
    }
    paths
}

fn locate_from(
    chosen: &str,
    lookup: &dyn Fn(&str) -> Option<PathBuf>,
    known: &[PathBuf],
) -> Option<Rclone> {
    let program = |path: &Path| Rclone {
        program: path.to_path_buf(),
        on_host: false,
    };
    let chosen = chosen.trim();
    if !chosen.is_empty() {
        let path = irori_core::paths::expand_home(chosen);
        return path.is_file().then(|| program(&path));
    }
    lookup("rclone")
        .or_else(|| known.iter().find(|p| p.is_file()).cloned())
        .map(|p| program(&p))
}

/// The remote path of `rel` (`Irori sync/a.json`) inside `remote`
/// (`gdrive:` or `gdrive:Games/`).
pub fn remote_path(remote: &str, rel: &str) -> String {
    let remote = remote.trim();
    let rel = rel.trim_start_matches('/');
    if remote.ends_with(':') || remote.ends_with('/') {
        format!("{remote}{rel}")
    } else {
        format!("{remote}/{rel}")
    }
}

/// Whether `remote` names an rclone remote (`name:` with an optional path).
pub fn is_remote(remote: &str) -> bool {
    let remote = remote.trim();
    match remote.split_once(':') {
        Some((name, _)) => {
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ' ' | '@'))
        }
        None => false,
    }
}

/// The entries of `lsjson`'s answer.
pub fn parse_list(json: &str) -> Result<Vec<Entry>> {
    let entries: Vec<JsonEntry> =
        serde_json::from_str(json).context("rclone printed something Irori couldn’t read")?;
    Ok(entries
        .into_iter()
        .map(|e| Entry {
            name: e.name,
            size: e.size.max(0) as u64,
            is_dir: e.is_dir,
        })
        .collect())
}

/// `listremotes`' answer: one `name:` per line.
pub fn parse_remotes(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| l.ends_with(':') && l.len() > 1)
        .map(str::to_string)
        .collect()
}

/// The reason of a failed run, in words for the user.
fn failure(stderr: &str, stdout: &str) -> String {
    let line = stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or("rclone failed");
    // "2026/11/18 10:00:00 Failed to …: didn't find section in config file"
    let line = line
        .split_once(" ERROR : ")
        .or_else(|| line.split_once(" CRITICAL: "))
        .or_else(|| line.split_once(" NOTICE: "))
        .map_or(line, |(_, rest)| rest);
    if line.contains("didn't find section in config file") {
        return "rclone has no remote of that name. Add it with “rclone config”.".into();
    }
    line.to_string()
}

fn is_not_found(stderr: &str) -> bool {
    stderr.contains("directory not found") || stderr.contains("object not found")
}

impl Rclone {
    /// Find rclone: the program chosen in settings (`chosen`, when not
    /// empty), else on `PATH`, else in the usual install folders. Inside a
    /// Flatpak the host is asked.
    pub async fn locate(chosen: &str) -> Option<Self> {
        let home = dirs::home_dir().unwrap_or_default();
        if cfg!(target_os = "linux") && host::is_flatpak() {
            let name = if chosen.trim().is_empty() {
                "rclone".to_string()
            } else {
                irori_core::paths::expand_home(chosen.trim())
                    .to_string_lossy()
                    .into_owned()
            };
            return host::find_program(&name).await.map(|program| Rclone {
                program,
                on_host: true,
            });
        }
        locate_from(
            chosen,
            &|name| which::which(name).ok(),
            &known_locations(&home),
        )
    }

    /// The program as shown to the user.
    pub fn display(&self) -> String {
        self.program.to_string_lossy().into_owned()
    }

    fn spec(&self, args: &[&str]) -> CommandSpec {
        let spec = CommandSpec::new(&self.program).args(args.iter().copied());
        if self.on_host {
            host::flatpak_spawn(spec, true)
        } else {
            spec
        }
    }

    async fn run(&self, args: &[&str]) -> Result<process::ExecResult> {
        let opts = RunOptions {
            capture_limit: Some(16 * 1024 * 1024),
            ..Default::default()
        };
        process::run(&self.spec(args), opts)
            .await
            .with_context(|| format!("Couldn’t start rclone ({})", self.display()))
    }

    /// rclone's version ("v1.68.2").
    pub async fn version(&self) -> Result<String> {
        let out = self.run(&["version"]).await?;
        if !out.success() {
            bail!("{}", failure(&out.stderr, &out.stdout));
        }
        let first = out.stdout.lines().next().unwrap_or_default();
        Ok(first.rsplit(' ').next().unwrap_or(first).trim().to_string())
    }

    /// The remotes the user set up.
    pub async fn remotes(&self) -> Result<Vec<String>> {
        let out = self.run(&["listremotes"]).await?;
        if !out.success() {
            bail!("{}", failure(&out.stderr, &out.stdout));
        }
        Ok(parse_remotes(&out.stdout))
    }

    /// The files and folders in `dir` (a remote path); empty when it
    /// doesn't exist yet.
    pub async fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        let out = self.run(&["lsjson", "--no-mimetype", dir]).await?;
        if !out.success() {
            if is_not_found(&out.stderr) {
                return Ok(Vec::new());
            }
            bail!("{}", failure(&out.stderr, &out.stdout));
        }
        parse_list(&out.stdout)
    }

    /// Copy the remote file `from` to the local file `to`.
    pub async fn download(&self, from: &str, to: &Path) -> Result<()> {
        let to = to.to_string_lossy();
        let out = self.run(&["copyto", from, &to]).await?;
        if !out.success() {
            bail!("{}", failure(&out.stderr, &out.stdout));
        }
        Ok(())
    }

    /// Copy the local file `from` to the remote file `to`, making its
    /// folders.
    pub async fn upload(&self, from: &Path, to: &str) -> Result<()> {
        let from = from.to_string_lossy();
        let out = self.run(&["copyto", &from, to]).await?;
        if !out.success() {
            bail!("{}", failure(&out.stderr, &out.stdout));
        }
        Ok(())
    }
}

/// A temporary file in Irori's cache for one transfer, removed when
/// dropped. The cache is visible to a host rclone too.
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    pub(crate) fn new() -> Result<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = irori_core::Paths::get().cache.join("rclone");
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Cannot create {}", dir.display()))?;
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        Ok(Self(dir.join(format!("{}-{n}.part", std::process::id()))))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_paths_inside_the_remote() {
        assert_eq!(
            remote_path("gdrive:", "Irori sync/a.json"),
            "gdrive:Irori sync/a.json"
        );
        assert_eq!(
            remote_path("gdrive:Games", "Irori sync/a.json"),
            "gdrive:Games/Irori sync/a.json"
        );
        assert_eq!(remote_path(" nas:/x/ ", "/b"), "nas:/x/b");
        assert!(is_remote("gdrive:") && is_remote("my-nas:backups/irori"));
        assert!(!is_remote("gdrive") && !is_remote(":local") && !is_remote("a/b:c"));
    }

    #[test]
    fn reads_rclones_answers() {
        let json = r#"[
            {"Path":"Irori sync","Name":"Irori sync","Size":-1,"ModTime":"2026-11-18T10:00:00Z","IsDir":true},
            {"Path":"ab12.json","Name":"ab12.json","Size":1234,"ModTime":"2026-11-18T10:00:00Z","IsDir":false}
        ]"#;
        assert_eq!(
            parse_list(json).unwrap(),
            [
                Entry {
                    name: "Irori sync".into(),
                    size: 0,
                    is_dir: true
                },
                Entry {
                    name: "ab12.json".into(),
                    size: 1234,
                    is_dir: false
                }
            ]
        );
        assert!(parse_list("not json").is_err());
        assert_eq!(parse_remotes("gdrive:\nnas:\n\n"), ["gdrive:", "nas:"]);
        assert_eq!(
            failure(
                "2026/11/18 10:00:00 CRITICAL: Failed to create file system for \"x:\": didn't find section in config file",
                ""
            ),
            "rclone has no remote of that name. Add it with “rclone config”."
        );
        assert_eq!(
            failure("2026/11/18 10:00:00 ERROR : a.json: quota exceeded", ""),
            "a.json: quota exceeded"
        );
    }

    #[test]
    fn finds_the_chosen_program_or_one_on_path() {
        let dir = tempfile::tempdir().unwrap();
        let none = |_: &str| None;
        assert_eq!(locate_from("", &none, &[]), None);
        let chosen = dir.path().join("rclone");
        std::fs::write(&chosen, b"").unwrap();
        assert_eq!(
            locate_from(&chosen.to_string_lossy(), &none, &[])
                .unwrap()
                .program,
            chosen
        );
        assert_eq!(
            locate_from("/gone/rclone", &|_| Some("/usr/bin/rclone".into()), &[]),
            None
        );
        let found = locate_from("", &|_| Some("/usr/bin/rclone".into()), &[]).unwrap();
        assert_eq!(found.program, PathBuf::from("/usr/bin/rclone"));
        assert_eq!(
            locate_from("", &none, &[chosen.clone()]).unwrap().program,
            chosen
        );
    }

    /// Runs a remote through a script standing in for rclone, which maps
    /// `test:` to a folder.
    #[cfg(unix)]
    #[tokio::test]
    async fn moves_files_through_a_remote() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("remote");
        std::fs::create_dir_all(&root).unwrap();
        let script = dir.path().join("rclone");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
map() {{ case "$1" in test:*) echo "{root}/${{1#test:}}" ;; *) echo "$1" ;; esac; }}
case "$1" in
  version) echo "rclone v1.68.2" ;;
  listremotes) echo "test:" ;;
  lsjson) d=$(map "$3"); [ -d "$d" ] || {{ echo "ERROR : directory not found" >&2; exit 3; }}
    printf '['; sep=''; for f in "$d"/*; do [ -e "$f" ] || continue
    printf '%s{{"Name":"%s","Size":%s,"IsDir":false}}' "$sep" "$(basename "$f")" "$(wc -c < "$f" | tr -d ' ')"; sep=','; done; echo ']' ;;
  copyto) s=$(map "$2"); t=$(map "$3"); mkdir -p "$(dirname "$t")"; cp "$s" "$t" ;;
esac
"#,
                root = root.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let rclone = locate_from(&script.to_string_lossy(), &|_| None, &[]).unwrap();
        assert_eq!(rclone.version().await.unwrap(), "v1.68.2");
        assert_eq!(rclone.remotes().await.unwrap(), ["test:"]);
        assert!(rclone.list("test:Irori sync").await.unwrap().is_empty());

        let local = dir.path().join("a.json");
        std::fs::write(&local, b"{}").unwrap();
        rclone
            .upload(&local, "test:Irori sync/a.json")
            .await
            .unwrap();
        let listed = rclone.list("test:Irori sync").await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].name.as_str(), listed[0].size), ("a.json", 2));
        let back = dir.path().join("b.json");
        rclone
            .download("test:Irori sync/a.json", &back)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&back).unwrap(), b"{}");
    }
}
