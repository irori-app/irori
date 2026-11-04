//! Ludusavi's command line in `--api` mode: finding the program, matching
//! titles, and backing up, listing and restoring one game's saves.
//!
//! Irori never bundles Ludusavi; it runs the copy the user installed. Every
//! command reads Ludusavi's own configuration, so backups made here also
//! show in Ludusavi's window. Output follows Ludusavi's `general-output`
//! schema; a command that fails may still print JSON (an unknown title, a
//! file that couldn't be copied), so stdout is parsed whatever the exit code.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use irori_core::process::{self, CancellationToken, CommandSpec, RunOptions};
use serde::Deserialize;
use serde_json::Value;

/// Flathub ID of Ludusavi.
pub const FLATPAK_ID: &str = "com.github.mtkennerly.ludusavi";

/// Where Ludusavi's installation instructions are.
pub const INSTALL_URL: &str = "https://github.com/mtkennerly/ludusavi#installation";

/// Where a Ludusavi program was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The program chosen in settings.
    Chosen,
    /// On `PATH` or in a usual install folder.
    Installed,
    /// The Flathub app.
    Flatpak,
}

/// A Ludusavi program to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ludusavi {
    /// Program and the arguments before Ludusavi's own (`flatpak run <id>`).
    argv: Vec<String>,
    pub source: Source,
}

impl Ludusavi {
    /// Find Ludusavi: the program chosen in settings (`chosen`, when not
    /// empty), else on `PATH`, else in the usual install folders, else the
    /// Flathub app. `None` when none of them is there.
    pub fn locate(chosen: &str) -> Option<Self> {
        let home = dirs::home_dir().unwrap_or_default();
        locate_from(
            chosen,
            &|name| which::which(name).ok(),
            &known_locations(&home),
            &[
                home.join(".local/share/flatpak/app"),
                PathBuf::from("/var/lib/flatpak/app"),
            ],
        )
    }

    /// The command as shown to the user: the program's path, or the
    /// Flatpak command.
    pub fn display(&self) -> String {
        self.argv.join(" ")
    }

    fn spec(&self, args: impl IntoIterator<Item = OsString>) -> CommandSpec {
        let mut argv = self.argv.iter();
        let program = argv.next().cloned().unwrap_or_default();
        // Offline, a stale manifest still knows the game; an update check
        // that fails must not fail the backup.
        CommandSpec::new(program)
            .args(argv.map(OsString::from))
            .arg("--try-manifest-update")
            .args(args)
    }

    /// Run with `args` and return the JSON it printed. Ludusavi exits with
    /// an error for unknown titles and failed files but still reports them,
    /// so only a run without JSON counts as a failure.
    async fn call(&self, args: Vec<OsString>, cancel: Option<CancellationToken>) -> Result<String> {
        let spec = self.spec(args);
        let mut opts = RunOptions::default();
        if let Some(cancel) = cancel {
            opts = opts.cancel(cancel);
        }
        let out = process::run(&spec, opts)
            .await
            .with_context(|| format!("Couldn’t start Ludusavi ({})", self.display()))?;
        if out.aborted {
            bail!("Cancelled");
        }
        if !out.stderr.trim().is_empty() {
            tracing::debug!("ludusavi: {}", out.stderr.trim());
        }
        let stdout = out.stdout.trim();
        if stdout.starts_with('{') {
            return Ok(stdout.to_string());
        }
        let reason = last_line(&out.stderr)
            .or_else(|| last_line(&out.stdout))
            .unwrap_or_else(|| match out.code {
                Some(code) => format!("exit code {code}"),
                None => "stopped by a signal".into(),
            });
        bail!("Ludusavi failed: {reason}")
    }

    /// Ludusavi's version ("0.31.0").
    pub async fn version(&self) -> Result<String> {
        let mut argv = self.argv.clone();
        let spec = CommandSpec::new(argv.remove(0)).args(argv).arg("--version");
        let out = process::run(&spec, RunOptions::default())
            .await
            .with_context(|| format!("Couldn’t start Ludusavi ({})", self.display()))?;
        if !out.success() {
            bail!(
                "Ludusavi failed: {}",
                last_line(&out.stderr).unwrap_or_else(|| "no version".into())
            );
        }
        Ok(parse_version(&out.stdout))
    }

    /// Titles in Ludusavi's data set that match `query`, best first.
    pub async fn find(&self, query: &TitleQuery) -> Result<Vec<TitleMatch>> {
        let json = self.call(query.args(), None).await?;
        parse_find(&json)
    }

    /// Back up `title`'s saves, looking in `wine_prefix` too, into `folder`
    /// (Ludusavi's own backup folder when `None`).
    pub async fn backup(
        &self,
        title: &str,
        wine_prefix: Option<&Path>,
        folder: Option<&Path>,
        cancel: CancellationToken,
    ) -> Result<Operation> {
        let mut args = os(&["backup", "--api", "--force"]);
        if let Some(prefix) = wine_prefix {
            args.push("--wine-prefix".into());
            args.push(prefix.into());
        }
        if let Some(folder) = folder {
            args.push("--path".into());
            args.push(folder.into());
        }
        args.push("--".into());
        args.push(title.into());
        let json = self.call(args, Some(cancel)).await?;
        parse_operation(&json, title)
    }

    /// The backups Ludusavi keeps of `title`, oldest first. `None` when
    /// there are none.
    pub async fn backups(&self, title: &str, folder: Option<&Path>) -> Result<Option<Backups>> {
        let mut args = os(&["backups", "--api"]);
        if let Some(folder) = folder {
            args.push("--path".into());
            args.push(folder.into());
        }
        args.push("--".into());
        args.push(title.into());
        let json = self.call(args, None).await?;
        parse_backups(&json, title)
    }

    /// Restore `backup` (an id from [`Backups`]; the newest when `None`) of
    /// `title` to where its files came from. `preview` only reports what
    /// would be written.
    pub async fn restore(
        &self,
        title: &str,
        backup: Option<&str>,
        folder: Option<&Path>,
        preview: bool,
    ) -> Result<Operation> {
        let mut args = os(&["restore", "--api", "--force"]);
        if preview {
            args.push("--preview".into());
        }
        if let Some(backup) = backup {
            args.push("--backup".into());
            args.push(backup.into());
        }
        if let Some(folder) = folder {
            args.push("--path".into());
            args.push(folder.into());
        }
        args.push("--".into());
        args.push(title.into());
        let json = self.call(args, None).await?;
        parse_operation(&json, title)
    }
}

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn last_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .map(str::to_string)
}

/// Usual install locations that aren't always on `PATH` (a desktop session
/// on macOS starts apps with a short `PATH`).
fn known_locations(home: &Path) -> Vec<PathBuf> {
    let mut paths = vec![home.join(".cargo/bin").join(exe("ludusavi"))];
    if cfg!(target_os = "macos") {
        paths.extend([
            PathBuf::from("/opt/homebrew/bin/ludusavi"),
            PathBuf::from("/usr/local/bin/ludusavi"),
            PathBuf::from("/Applications/Ludusavi.app/Contents/MacOS/ludusavi"),
            home.join("Applications/Ludusavi.app/Contents/MacOS/ludusavi"),
        ]);
    } else if cfg!(windows) {
        if let Some(local) = dirs::data_local_dir() {
            paths.extend([
                local.join("Microsoft/WinGet/Links/ludusavi.exe"),
                local.join("Programs/Ludusavi/ludusavi.exe"),
            ]);
        }
        paths.push(home.join("scoop/shims/ludusavi.exe"));
        if let Some(programs) = std::env::var_os("ProgramFiles") {
            paths.push(PathBuf::from(programs).join("Ludusavi/ludusavi.exe"));
        }
    } else {
        paths.push(home.join(".local/bin/ludusavi"));
    }
    paths
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn locate_from(
    chosen: &str,
    lookup: &dyn Fn(&str) -> Option<PathBuf>,
    known: &[PathBuf],
    flatpak_app_dirs: &[PathBuf],
) -> Option<Ludusavi> {
    let program = |path: &Path, source| Ludusavi {
        argv: vec![path.to_string_lossy().into_owned()],
        source,
    };
    let chosen = chosen.trim();
    if !chosen.is_empty() {
        let path = irori_core::paths::expand_home(chosen);
        return path.is_file().then(|| program(&path, Source::Chosen));
    }
    if let Some(path) = lookup("ludusavi") {
        return Some(program(&path, Source::Installed));
    }
    if let Some(path) = known.iter().find(|p| p.is_file()) {
        return Some(program(path, Source::Installed));
    }
    let flatpak = lookup("flatpak")?;
    flatpak_app_dirs
        .iter()
        .any(|d| d.join(FLATPAK_ID).is_dir())
        .then(|| Ludusavi {
            argv: vec![
                flatpak.to_string_lossy().into_owned(),
                "run".into(),
                FLATPAK_ID.into(),
            ],
            source: Source::Flatpak,
        })
}

/// "ludusavi 0.31.0" → "0.31.0".
fn parse_version(stdout: &str) -> String {
    let line = stdout.trim();
    line.rsplit(' ').next().unwrap_or(line).to_string()
}

/// What to look a game up by. Ludusavi tries the Steam id, then the GOG id,
/// then the exact title, then the normalized title, and stops at the first
/// match unless `multiple` is set.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TitleQuery {
    pub steam_id: Option<u32>,
    pub gog_id: Option<u64>,
    pub names: Vec<String>,
    /// Ignore case, "edition" and year suffixes and some symbols.
    pub normalized: bool,
    /// Fuzzy matching, for a search the user types.
    pub fuzzy: bool,
    /// Every match, ranked, instead of the first.
    pub multiple: bool,
}

impl TitleQuery {
    fn args(&self) -> Vec<OsString> {
        let mut args = os(&["find", "--api"]);
        if let Some(id) = self.steam_id {
            args.push("--steam-id".into());
            args.push(id.to_string().into());
        }
        if let Some(id) = self.gog_id {
            args.push("--gog-id".into());
            args.push(id.to_string().into());
        }
        for (on, flag) in [
            (self.normalized, "--normalized"),
            (self.fuzzy, "--fuzzy"),
            (self.multiple, "--multiple"),
        ] {
            if on {
                args.push(flag.into());
            }
        }
        if !self.names.is_empty() {
            args.push("--".into());
            args.extend(self.names.iter().map(OsString::from));
        }
        args
    }
}

/// A title of Ludusavi's data set.
#[derive(Debug, Clone, PartialEq)]
pub struct TitleMatch {
    pub title: String,
    /// How well it matches, 0 to 1; `None` for an exact match by id or name.
    pub score: Option<f64>,
}

/// How Ludusavi handled a game in a backup or restore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
pub enum Decision {
    Processed,
    Cancelled,
    Ignored,
    #[default]
    #[serde(other)]
    Unknown,
}

/// How a game or file compares with its previous backup (backing up), or
/// a backup with the files on disk (restoring).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
pub enum Change {
    New,
    Different,
    Removed,
    Same,
    #[default]
    #[serde(other)]
    Unknown,
}

/// One file of a backup or restore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReport {
    pub path: String,
    pub bytes: u64,
    pub change: Change,
    pub failed: bool,
    pub ignored: bool,
    /// Why it failed, when it did.
    pub error: Option<String>,
}

/// What a backup or restore did with one game.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Operation {
    /// The game's title in Ludusavi; `None` when it reported nothing (no
    /// saves were found, or the title is unknown).
    pub title: Option<String>,
    pub decision: Decision,
    pub change: Change,
    pub files: Vec<FileReport>,
    /// Registry keys it handled (Windows saves kept in the registry).
    pub registry_keys: usize,
    /// Registry keys that failed.
    pub registry_failed: usize,
    /// Ludusavi doesn't know the title.
    pub unknown: bool,
}

impl Operation {
    /// Bytes of the files it handled.
    pub fn bytes(&self) -> u64 {
        self.files
            .iter()
            .filter(|f| !f.ignored && !f.failed)
            .map(|f| f.bytes)
            .sum()
    }

    /// Files it couldn't handle.
    pub fn failed(&self) -> impl Iterator<Item = &FileReport> {
        self.files.iter().filter(|f| f.failed)
    }

    /// Whether it found any saves at all.
    pub fn found_saves(&self) -> bool {
        self.files.iter().any(|f| !f.ignored) || self.registry_keys > 0
    }
}

/// The backups of one game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backups {
    /// The game's folder in the backup folder.
    pub folder: PathBuf,
    /// Oldest first, as Ludusavi lists them.
    pub backups: Vec<Backup>,
}

/// One backup of a game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backup {
    /// Ludusavi's id ("backup-20241104T183012Z", ".zip" when zipped,
    /// "-diff" for a differential backup, "." for the old single backup).
    pub name: String,
    /// When it was made, RFC 3339 in UTC.
    pub when: String,
    /// The system it was made on ("windows", "linux", "mac").
    pub os: Option<String>,
    pub comment: Option<String>,
    /// Locked backups are never removed by Ludusavi's retention.
    pub locked: bool,
}

impl Backup {
    /// A differential backup only holds what changed since its full backup.
    pub fn is_differential(&self) -> bool {
        self.name.trim_end_matches(".zip").ends_with("-diff")
    }

    /// Space it takes in `folder` (the game's backup folder): the zip, or
    /// the files of its folder. Reads the disk; call off the UI thread.
    pub fn size_on_disk(&self, folder: &Path) -> u64 {
        if self.name == "." {
            // The old single-backup layout keeps its files beside
            // mapping.yaml and the newer backups.
            let Ok(entries) = std::fs::read_dir(folder) else {
                return 0;
            };
            return entries
                .flatten()
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    name != "mapping.yaml" && !name.starts_with("backup-")
                })
                .map(|e| size_of(&e.path()))
                .sum();
        }
        size_of(&folder.join(&self.name))
    }
}

/// Bytes of a file, or of every file under a folder.
fn size_of(path: &Path) -> u64 {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => std::fs::read_dir(path)
            .map(|entries| entries.flatten().map(|e| size_of(&e.path())).sum())
            .unwrap_or(0),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Output {
    errors: Option<Errors>,
    games: BTreeMap<String, Value>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Errors {
    unknown_games: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct GameEntry {
    decision: Decision,
    change: Change,
    files: BTreeMap<String, FileEntry>,
    registry: BTreeMap<String, RegistryEntry>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct FileEntry {
    bytes: u64,
    change: Change,
    failed: bool,
    ignored: bool,
    error: Option<ErrorEntry>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct RegistryEntry {
    failed: bool,
    ignored: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ErrorEntry {
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredEntry {
    backup_path: String,
    #[serde(default)]
    backups: Vec<BackupEntry>,
}

#[derive(Deserialize)]
struct BackupEntry {
    name: String,
    when: String,
    #[serde(default)]
    os: Option<String>,
    #[serde(default)]
    comment: Option<String>,
    #[serde(default)]
    locked: bool,
}

fn parse_output(json: &str) -> Result<Output> {
    serde_json::from_str(json).context("Ludusavi printed something Irori couldn’t read")
}

/// Ranked titles from `find --api`. An unknown title is no match, not an
/// error.
pub fn parse_find(json: &str) -> Result<Vec<TitleMatch>> {
    let output = parse_output(json)?;
    let mut matches: Vec<TitleMatch> = output
        .games
        .into_iter()
        .map(|(title, game)| TitleMatch {
            title,
            score: game.get("score").and_then(Value::as_f64),
        })
        .collect();
    // Exact matches (no score) first, then by score.
    matches.sort_by(|a, b| {
        let key = |m: &TitleMatch| m.score.unwrap_or(f64::INFINITY);
        key(b)
            .total_cmp(&key(a))
            .then_with(|| a.title.cmp(&b.title))
    });
    Ok(matches)
}

/// One game's part of `backup --api` or `restore --api`. `title` names the
/// game asked for; Ludusavi reports it under its own title, which is the
/// same once matched.
pub fn parse_operation(json: &str, title: &str) -> Result<Operation> {
    let mut output = parse_output(json)?;
    let unknown = output
        .errors
        .as_ref()
        .and_then(|e| e.unknown_games.as_ref())
        .is_some_and(|games| games.iter().any(|g| g == title));
    let entry = match output.games.remove(title) {
        Some(game) => Some((title.to_string(), game)),
        None => output.games.into_iter().next(),
    };
    let Some((name, game)) = entry else {
        return Ok(Operation {
            unknown,
            ..Default::default()
        });
    };
    let game: GameEntry =
        serde_json::from_value(game).context("Ludusavi printed something Irori couldn’t read")?;
    Ok(Operation {
        title: Some(name),
        decision: game.decision,
        change: game.change,
        files: game
            .files
            .into_iter()
            .map(|(path, f)| FileReport {
                path,
                bytes: f.bytes,
                change: f.change,
                failed: f.failed,
                ignored: f.ignored,
                error: f.error.map(|e| e.message).filter(|m| !m.is_empty()),
            })
            .collect(),
        registry_keys: game.registry.values().filter(|r| !r.ignored).count(),
        registry_failed: game.registry.values().filter(|r| r.failed).count(),
        unknown,
    })
}

/// The backups of `title` from `backups --api`; `None` when it has none.
pub fn parse_backups(json: &str, title: &str) -> Result<Option<Backups>> {
    let mut output = parse_output(json)?;
    let Some(game) = output
        .games
        .remove(title)
        .or_else(|| output.games.into_values().next())
    else {
        return Ok(None);
    };
    let stored: StoredEntry =
        serde_json::from_value(game).context("Ludusavi printed something Irori couldn’t read")?;
    Ok(Some(Backups {
        folder: PathBuf::from(stored.backup_path),
        backups: stored
            .backups
            .into_iter()
            .map(|b| Backup {
                name: b.name,
                when: b.when,
                os: b.os,
                comment: b.comment.filter(|c| !c.trim().is_empty()),
                locked: b.locked,
            })
            .collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_program_chosen_on_path_known_or_flathub() {
        let dir = tempfile::tempdir().unwrap();
        let apps = vec![dir.path().join("apps")];
        let none = |_: &str| None;
        assert_eq!(locate_from("", &none, &[], &apps), None);

        let chosen = dir.path().join("my-ludusavi");
        std::fs::write(&chosen, b"").unwrap();
        let found = locate_from(&chosen.to_string_lossy(), &none, &[], &apps).unwrap();
        assert_eq!(found.source, Source::Chosen);
        assert_eq!(found.display(), chosen.to_string_lossy());
        // A chosen program that is gone is not replaced by another one.
        let on_path = |n: &str| (n == "ludusavi").then(|| PathBuf::from("/usr/bin/ludusavi"));
        assert_eq!(locate_from("/nowhere/ludusavi", &on_path, &[], &apps), None);

        let found = locate_from("", &on_path, &[], &apps).unwrap();
        assert_eq!(
            (found.display().as_str(), found.source),
            ("/usr/bin/ludusavi", Source::Installed)
        );

        let known = dir.path().join("cargo-ludusavi");
        std::fs::write(&known, b"").unwrap();
        let found =
            locate_from("", &none, &[PathBuf::from("/gone"), known.clone()], &apps).unwrap();
        assert_eq!(found.display(), known.to_string_lossy());

        let flatpak = |n: &str| (n == "flatpak").then(|| PathBuf::from("/usr/bin/flatpak"));
        assert_eq!(locate_from("", &flatpak, &[], &apps), None);
        std::fs::create_dir_all(apps[0].join(FLATPAK_ID)).unwrap();
        let found = locate_from("", &flatpak, &[], &apps).unwrap();
        assert_eq!(found.source, Source::Flatpak);
        assert_eq!(
            found.display(),
            format!("/usr/bin/flatpak run {FLATPAK_ID}")
        );
    }

    #[test]
    fn builds_commands_with_ludusavi_arguments_last() {
        let flatpak = Ludusavi {
            argv: vec!["/usr/bin/flatpak".into(), "run".into(), FLATPAK_ID.into()],
            source: Source::Flatpak,
        };
        let spec = flatpak.spec(os(&["backups", "--api"]));
        assert_eq!(spec.program, PathBuf::from("/usr/bin/flatpak"));
        assert_eq!(
            spec.args,
            os(&[
                "run",
                FLATPAK_ID,
                "--try-manifest-update",
                "backups",
                "--api"
            ])
        );

        let query = TitleQuery {
            steam_id: Some(1643320),
            gog_id: Some(1_207_658_691),
            names: vec!["-S.T.A.L.K.E.R. 2".into()],
            normalized: true,
            ..Default::default()
        };
        assert_eq!(
            query.args(),
            os(&[
                "find",
                "--api",
                "--steam-id",
                "1643320",
                "--gog-id",
                "1207658691",
                "--normalized",
                "--",
                "-S.T.A.L.K.E.R. 2",
            ])
        );
    }

    /// Runs the client against a script standing in for Ludusavi: it records
    /// its arguments and prints what Ludusavi would.
    #[cfg(unix)]
    #[tokio::test]
    async fn runs_ludusavi_and_reads_its_answers() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let args = dir.path().join("args");
        let script = dir.path().join("ludusavi");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
[ "$1" = --version ] && {{ echo "ludusavi 0.31.0"; exit 0; }}
printf '%s\n' "$@" > '{args}'
case "$2" in
  find) echo '{{"errors": {{"unknownGames": ["Nope"]}}, "games": {{}}}}'; exit 1 ;;
  backup) echo '{{"games": {{"Hades": {{"decision": "Processed", "change": "New",
    "files": {{"/p/pfx/drive_c/users/steamuser/Saved/a.sav": {{"bytes": 12,
    "change": "New", "failed": false, "ignored": false, "duplicatedBy": []}}}},
    "registry": {{}}}}}}}}' ;;
  *) echo "error: unrecognized subcommand" >&2; exit 2 ;;
esac
"#,
                args = args.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ludusavi = locate_from(&script.to_string_lossy(), &|_| None, &[], &[]).unwrap();

        assert_eq!(ludusavi.version().await.unwrap(), "0.31.0");

        // Ludusavi exits with an error for an unknown title but still says so.
        let query = TitleQuery {
            names: vec!["Nope".into()],
            ..Default::default()
        };
        assert!(ludusavi.find(&query).await.unwrap().is_empty());

        let prefix = Path::new("/p/pfx");
        let op = ludusavi
            .backup("Hades", Some(prefix), None, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!((op.title.as_deref(), op.bytes()), (Some("Hades"), 12));
        let passed = std::fs::read_to_string(&args).unwrap();
        assert_eq!(
            passed.lines().collect::<Vec<_>>(),
            [
                "--try-manifest-update",
                "backup",
                "--api",
                "--force",
                "--wine-prefix",
                "/p/pfx",
                "--",
                "Hades"
            ]
        );

        let err = ludusavi
            .restore("Hades", None, None, true)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("unrecognized subcommand"),
            "{err:#}"
        );
    }

    #[test]
    fn reads_the_version() {
        assert_eq!(parse_version("ludusavi 0.31.0\n"), "0.31.0");
        assert_eq!(parse_version("0.29.1"), "0.29.1");
    }

    #[test]
    fn ranks_found_titles() {
        let json = r#"{
            "games": {
                "Hades II": {"score": 0.71},
                "Hades": {"score": 0.98},
                "Hades: Battle Out of Hell": {}
            }
        }"#;
        let found = parse_find(json).unwrap();
        let titles: Vec<_> = found.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, ["Hades: Battle Out of Hell", "Hades", "Hades II"]);
        assert_eq!(found[1].score, Some(0.98));
    }

    #[test]
    fn an_unknown_title_is_no_match() {
        let json = r#"{"errors": {"unknownGames": ["Not A Game"]}, "games": {}}"#;
        assert!(parse_find(json).unwrap().is_empty());
        let op = parse_operation(json, "Not A Game").unwrap();
        assert!(op.unknown);
        assert_eq!(op.title, None);
        assert!(!op.found_saves());
    }

    #[test]
    fn reads_a_backup() {
        let json = r#"{
            "errors": {"someGamesFailed": true},
            "overall": {"totalGames": 1, "totalBytes": 150, "processedGames": 1,
                        "processedBytes": 100,
                        "changedGames": {"new": 0, "different": 1, "same": 0}},
            "games": {
                "S.T.A.L.K.E.R. 2: Heart of Chornobyl": {
                    "decision": "Processed",
                    "change": "Different",
                    "files": {
                        "/games/pfx/drive_c/users/steamuser/AppData/Local/Stalker2/Saved/SaveGames/a.sav": {
                            "bytes": 100, "change": "Different", "failed": false,
                            "ignored": false, "duplicatedBy": []
                        },
                        "/games/pfx/drive_c/users/steamuser/AppData/Local/Stalker2/Saved/SaveGames/b.sav": {
                            "bytes": 50, "change": "New", "failed": true, "ignored": false,
                            "duplicatedBy": [], "error": {"message": "Permission denied"}
                        }
                    },
                    "registry": {
                        "HKEY_CURRENT_USER/Software/GSC": {"failed": false, "ignored": false,
                            "change": "Same", "duplicatedBy": [], "values": {}}
                    }
                }
            }
        }"#;
        let op = parse_operation(json, "S.T.A.L.K.E.R. 2: Heart of Chornobyl").unwrap();
        assert_eq!(op.decision, Decision::Processed);
        assert_eq!(op.change, Change::Different);
        assert_eq!(op.files.len(), 2);
        assert_eq!(op.bytes(), 100);
        let failed: Vec<_> = op.failed().collect();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].error.as_deref(), Some("Permission denied"));
        assert_eq!((op.registry_keys, op.registry_failed), (1, 0));
        assert!(op.found_saves() && !op.unknown);
    }

    #[test]
    fn a_game_without_saves_reports_nothing() {
        let json = r#"{"overall": {"totalGames": 0, "totalBytes": 0, "processedGames": 0,
            "processedBytes": 0, "changedGames": {"new": 0, "different": 0, "same": 0}},
            "games": {}}"#;
        let op = parse_operation(json, "Celeste").unwrap();
        assert_eq!(op, Operation::default());
        assert!(!op.found_saves());
    }

    #[test]
    fn tolerates_values_added_later() {
        let json = r#"{"games": {"Celeste": {"decision": "Deferred", "change": "Moved",
            "files": {"/a": {"bytes": 3, "change": "Renamed", "failed": false,
            "ignored": false, "duplicatedBy": [], "tags": ["save"]}}, "registry": {},
            "somethingNew": 1}}}"#;
        let op = parse_operation(json, "Celeste").unwrap();
        assert_eq!(op.decision, Decision::Unknown);
        assert_eq!(op.files[0].change, Change::Unknown);
        assert_eq!(op.bytes(), 3);
    }

    #[test]
    fn reads_the_backups_of_a_game() {
        let json = r#"{
            "games": {
                "Celeste": {
                    "backupPath": "/home/u/ludusavi-backup/Celeste",
                    "backups": [
                        {"name": "backup-20241102T101500Z", "when": "2024-11-02T10:15:00Z",
                         "os": "linux", "locked": false},
                        {"name": "backup-20241104T183012Z-diff.zip",
                         "when": "2024-11-04T18:30:12Z", "os": "windows",
                         "comment": "Before the last chapter", "locked": true},
                        {"name": ".", "when": "2023-01-01T00:00:00Z", "comment": " ", "locked": false}
                    ]
                }
            }
        }"#;
        let stored = parse_backups(json, "Celeste").unwrap().unwrap();
        assert_eq!(
            stored.folder,
            PathBuf::from("/home/u/ludusavi-backup/Celeste")
        );
        assert_eq!(stored.backups.len(), 3);
        let diff = &stored.backups[1];
        assert!(diff.is_differential() && diff.locked);
        assert_eq!(diff.os.as_deref(), Some("windows"));
        assert_eq!(diff.comment.as_deref(), Some("Before the last chapter"));
        assert!(!stored.backups[0].is_differential());
        assert_eq!(stored.backups[2].comment, None);

        assert_eq!(parse_backups(r#"{"games": {}}"#, "Celeste").unwrap(), None);
        assert!(parse_backups("{not json", "Celeste").is_err());
    }

    #[test]
    fn measures_backups_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path();
        std::fs::write(game.join("mapping.yaml"), b"0123456789").unwrap();
        std::fs::write(game.join("backup-1.zip"), vec![0; 300]).unwrap();
        let folder = game.join("backup-2/drive-0/saves");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("a.sav"), vec![0; 40]).unwrap();
        std::fs::write(folder.join("b.sav"), vec![0; 2]).unwrap();
        let legacy = game.join("drive-0");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("old.sav"), vec![0; 7]).unwrap();

        let backup = |name: &str| Backup {
            name: name.into(),
            when: String::new(),
            os: None,
            comment: None,
            locked: false,
        };
        assert_eq!(backup("backup-1.zip").size_on_disk(game), 300);
        assert_eq!(backup("backup-2").size_on_disk(game), 42);
        assert_eq!(backup("backup-3").size_on_disk(game), 0);
        // The old layout: everything but mapping.yaml and the newer backups.
        assert_eq!(backup(".").size_on_disk(game), 7);
    }
}
