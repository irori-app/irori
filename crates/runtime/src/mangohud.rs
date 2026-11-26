//! MangoHud's configuration for one session, which Irori can change while
//! the game runs: show or hide MangoHud and change its frame limit.
//!
//! MangoHud reads `MANGOHUD_CONFIGFILE` and watches that file, applying an
//! edit at once. So a game with MangoHud gets a file of its own for the
//! session, `<state>/mangohud/<runner>-<app>.conf`: the user's own
//! configuration (their `MANGOHUD_CONFIGFILE`, else
//! `$XDG_CONFIG_HOME/MangoHud/MangoHud.conf`) followed by Irori's lines
//! after a `# irori` marker: `fps_limit` and `no_display`. The user's own
//! lines for those two keys move into Irori's part, so it holds the whole
//! state. The file goes when the session ends ([`remove_session`]).
//!
//! A user whose `MANGOHUD_CONFIG` variable doesn't say `read_cfg` keeps
//! MangoHud from reading any file: then the limit and the hidden start go
//! into that variable ([`env_config`]) and can't change during play.
//!
//! Per-program files (`MangoHud/<program>.conf`, `wine-<program>.conf`)
//! aren't read for the session's file: the global one is.

use std::path::{Path, PathBuf};

use irori_core::GameId;

use crate::env::{EnvMap, HostEnv, remove_quote_if_necessary};

/// The marker after which Irori's own lines start.
const MARKER: &str = "# irori";
/// The session file's first line.
const HEADER: &str = "# Written by Irori for this session; it goes when the game exits.";
/// The keys Irori manages in the session's file.
const KEYS: [&str; 2] = ["fps_limit", "no_display"];

/// MangoHud's state in a session, as its file says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MangoState {
    /// MangoHud draws its overlay (`no_display` absent).
    pub visible: bool,
    /// Its frame limit; `None` for none.
    pub fps_limit: Option<u32>,
    /// MangoHud limits the session's frame rate, or nothing does: its limit
    /// may change. False when gamescope or DXVK limits it
    /// (`crate::frame_limit`), so one limit never runs through two limiters.
    pub limits: bool,
}

/// The line that says another limiter applies.
const OTHER_LIMITER: &str = "# irori: another limiter applies";
/// Comment recording which Irori-managed HUD config file (`global` or `<runner>_<app>`)
/// seeded this session so live Settings edits can rewrite matching running sessions.
const SOURCE_PREFIX: &str = "# irori-source: ";

/// The session's file for `id`.
pub fn session_path(id: &GameId) -> PathBuf {
    let name = irori_core::paths::sanitize(&format!("{}-{}", id.runner.id(), id.app_name));
    irori_core::Paths::get()
        .state_dir()
        .join("mangohud")
        .join(format!("{name}.conf"))
}

/// The key of a configuration line (`fps_limit=60` → `fps_limit`,
/// `no_display` → `no_display`); `None` for comments and blank lines.
fn key_of(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    Some(line.split('=').next().unwrap_or(line).trim())
}

/// The state the user's own configuration starts a session with: hidden
/// when `hidden` or it says `no_display`, and the limit `fps_limit` names
/// unless `limit` replaces it. `limits` is [`MangoState::limits`].
pub fn initial_state(user: &str, hidden: bool, limit: Option<u32>, limits: bool) -> MangoState {
    let mut state = MangoState {
        visible: !hidden,
        fps_limit: limit,
        limits,
    };
    for line in user.lines() {
        match key_of(line) {
            Some("no_display") => state.visible = false,
            Some("fps_limit") if limit.is_none() => {
                // MangoHud takes a list of limits and starts with the first.
                state.fps_limit = line
                    .split_once('=')
                    .and_then(|(_, v)| v.split(',').next())
                    .and_then(|v| v.trim().parse().ok())
                    .filter(|n| *n > 0);
            }
            _ => {}
        }
    }
    state
}

/// The session's file: `user`'s configuration up to Irori's marker,
/// without the keys Irori manages, then Irori's lines for `state`.
pub fn session_config(user: &str, state: MangoState) -> String {
    session_config_with_source(user, state, None)
}

/// Same as [`session_config`], additionally recording the Irori-managed source
/// stem (`global` or `<runner>_<app>`) so live HUD settings changes can sync
/// running sessions in place.
pub fn session_config_with_source(user: &str, state: MangoState, source: Option<&str>) -> String {
    let mut out = format!("{HEADER}\n");
    for line in user.lines() {
        if line.trim() == MARKER {
            break;
        }
        if line == HEADER {
            continue;
        }
        if key_of(line).is_some_and(|k| KEYS.contains(&k)) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(MARKER);
    out.push('\n');
    if let Some(fps) = state.fps_limit {
        out.push_str(&format!("fps_limit={fps}\n"));
    }
    if !state.visible {
        out.push_str("no_display\n");
    }
    if !state.limits {
        out.push_str(OTHER_LIMITER);
        out.push('\n');
    }
    if let Some(src) = source.map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str(SOURCE_PREFIX);
        out.push_str(src);
        out.push('\n');
    }
    out
}

/// The Irori-managed source stem recorded in a session file, if any.
pub fn parse_source(text: &str) -> Option<&str> {
    let (_, ours) = text.split_once(&format!("\n{MARKER}\n"))?;
    ours.lines()
        .find_map(|line| line.trim().strip_prefix(SOURCE_PREFIX))
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The state in a session's file; `None` when it isn't one of Irori's.
pub fn parse_state(text: &str) -> Option<MangoState> {
    let (_, ours) = text.split_once(&format!("\n{MARKER}\n"))?;
    let mut state = MangoState {
        visible: true,
        fps_limit: None,
        limits: !ours.lines().any(|l| l.trim() == OTHER_LIMITER),
    };
    for line in ours.lines() {
        match key_of(line) {
            Some("no_display") => state.visible = false,
            Some("fps_limit") => {
                state.fps_limit = line
                    .split_once('=')
                    .and_then(|(_, v)| v.trim().parse().ok())
            }
            _ => {}
        }
    }
    Some(state)
}

/// Whether the user's `MANGOHUD_CONFIG` keeps MangoHud from reading a file
/// (it is set and doesn't say `read_cfg`).
pub fn env_only(existing: Option<&str>) -> bool {
    existing
        .map(remove_quote_if_necessary)
        .is_some_and(|v| !v.trim().is_empty() && !v.split(',').any(|o| o.trim() == "read_cfg"))
}

/// `MANGOHUD_CONFIG` for a user who configures MangoHud through that
/// variable alone: their options, with Irori's limit and hidden start in
/// place of their own.
pub fn env_config(existing: &str, state: MangoState) -> String {
    let existing = remove_quote_if_necessary(existing);
    let mut options: Vec<String> = existing
        .split(',')
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .filter(|o| key_of(o).is_none_or(|k| !KEYS.contains(&k)))
        .map(str::to_owned)
        .collect();
    if let Some(fps) = state.fps_limit {
        options.push(format!("fps_limit={fps}"));
    }
    if !state.visible {
        options.push("no_display".into());
    }
    options.join(",")
}

/// The user's own MangoHud configuration file: their
/// `MANGOHUD_CONFIGFILE`, else the global one under the configuration
/// folder.
fn user_config_path(vars: &EnvMap, host: &HostEnv) -> Option<PathBuf> {
    if let Some(file) = vars
        .get("MANGOHUD_CONFIGFILE")
        .map(String::as_str)
        .or_else(|| host.get("MANGOHUD_CONFIGFILE"))
        .filter(|f| !f.trim().is_empty())
    {
        return Some(PathBuf::from(file));
    }
    let config = host
        .get("XDG_CONFIG_HOME")
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| host.get("HOME").map(|h| Path::new(h).join(".config")))?;
    Some(config.join("MangoHud").join("MangoHud.conf"))
}

/// What setting MangoHud up for a session does, worked out without doing
/// it: the variable it sets and the session file it writes, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MangoPlan {
    pub state: MangoState,
    /// The variable and its value.
    pub var: (String, String),
    /// The session's own file and what it holds, when MangoHud reads one.
    pub file: Option<(PathBuf, String)>,
}

/// Extract the sanitized config stem (`global` or `<runner>_<app>`) when
/// `path` is an Irori-managed MangoHud config inside `Paths::mangohud_dir()`.
fn managed_source_name(path: &Path) -> Option<String> {
    let dir = irori_core::Paths::get().mangohud_dir();
    if path.parent() != Some(dir.as_path()) {
        return None;
    }
    if path.extension().and_then(|e| e.to_str()) != Some("conf") {
        return None;
    }
    path.file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// [`prepare`]'s plan for `id`: its own file when MangoHud reads one (it
/// may change during play), else the user's variable. Reads the user's
/// MangoHud file, writes nothing.
pub fn plan(
    id: &GameId,
    vars: &EnvMap,
    host: &HostEnv,
    hidden: bool,
    limit: Option<u32>,
    limits: bool,
) -> MangoPlan {
    let existing = vars
        .get("MANGOHUD_CONFIG")
        .cloned()
        .or_else(|| host.get("MANGOHUD_CONFIG").map(str::to_owned));
    if env_only(existing.as_deref()) {
        let existing = existing.unwrap_or_default();
        let state = initial_state(&existing.replace(',', "\n"), hidden, limit, limits);
        return MangoPlan {
            state,
            var: ("MANGOHUD_CONFIG".into(), env_config(&existing, state)),
            file: None,
        };
    }
    let user_path = user_config_path(vars, host);
    let source = user_path.as_deref().and_then(managed_source_name);
    let user = user_path
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    let state = initial_state(&user, hidden, limit, limits);
    let path = session_path(id);
    MangoPlan {
        state,
        var: (
            "MANGOHUD_CONFIGFILE".into(),
            path.to_string_lossy().into_owned(),
        ),
        file: Some((
            path,
            session_config_with_source(&user, state, source.as_deref()),
        )),
    }
}

/// Set MangoHud up for the session of `id` in `vars` as [`plan`] says,
/// writing the session's file. Returns the state it starts with, and
/// whether it can change.
pub fn prepare(
    id: &GameId,
    vars: &mut EnvMap,
    host: &HostEnv,
    hidden: bool,
    limit: Option<u32>,
    limits: bool,
) -> (MangoState, bool) {
    let MangoPlan { state, var, file } = plan(id, vars, host, hidden, limit, limits);
    let Some((path, text)) = file else {
        vars.insert(var.0, var.1);
        return (state, false);
    };
    let written = path
        .parent()
        .is_some_and(|dir| std::fs::create_dir_all(dir).is_ok())
        && std::fs::write(&path, text).is_ok();
    if !written {
        tracing::warn!("writing MangoHud's file for {id}: {}", path.display());
        if limit.is_some() || hidden {
            let options = env_config("read_cfg", state);
            vars.insert("MANGOHUD_CONFIG".into(), options);
        }
        return (state, false);
    }
    vars.insert(var.0, var.1);
    (state, true)
}

/// MangoHud's state in the running session of `id`, when Irori can change
/// it. Reads a small file: call it off the UI thread.
pub fn read_session(id: &GameId) -> Option<MangoState> {
    parse_state(&std::fs::read_to_string(session_path(id)).ok()?)
}

/// Change MangoHud's state in the running session of `id`. The file is
/// rewritten in place, not replaced: MangoHud watches the file itself for
/// changes, and a file renamed over it would end the watch.
pub fn write_session(id: &GameId, state: MangoState) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let path = session_path(id);
    let text = std::fs::read_to_string(&path).context("MangoHud isn't running with this game")?;
    anyhow::ensure!(
        parse_state(&text).is_some(),
        "MangoHud isn't running with this game"
    );
    let source = parse_source(&text).map(str::to_owned);
    std::fs::write(
        &path,
        session_config_with_source(&text, state, source.as_deref()),
    )
    .context("MangoHud's settings couldn't be changed")
}

/// Update the base configuration of a running session whose `# irori-source:`
/// matches `source_name`, preserving the session's limiter ownership and inode.
///
/// Always rewrites in place (even when the base config bytes are unchanged) so
/// Linux `O_TRUNC` + `write` fires MangoHud's `IN_MODIFY` watch and reloads
/// updated `MANGOHUD_PRESETSFILE` contents immediately.
pub fn sync_session_base(
    id: &GameId,
    source_name: &str,
    base_config: &str,
    fps_limit_update: Option<Option<u32>>,
    no_display_update: Option<bool>,
) -> anyhow::Result<bool> {
    use anyhow::Context as _;
    let path = session_path(id);
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("reading MangoHud session file {}", path.display()));
        }
    };
    if parse_source(&existing) != Some(source_name) {
        return Ok(false);
    }
    let Some(mut state) = parse_state(&existing) else {
        return Ok(false);
    };
    if let Some(limit) = fps_limit_update
        && state.limits
    {
        state.fps_limit = limit.filter(|&v| v > 0);
    }
    if let Some(hidden) = no_display_update {
        state.visible = !hidden;
    }
    let updated = session_config_with_source(base_config, state, Some(source_name));
    std::fs::write(&path, updated)
        .with_context(|| format!("updating MangoHud session file {}", path.display()))?;
    Ok(true)
}

/// Forget the session's file once the game exited.
pub fn remove_session(id: &GameId) {
    let path = session_path(id);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("removing {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = "# my HUD\nposition=top-right\nfps_limit=144,60\ngpu_stats\n";

    #[test]
    fn the_session_file_keeps_the_users_lines_and_adds_irori_s() {
        let state = initial_state(USER, false, None, true);
        assert_eq!(
            state,
            MangoState {
                visible: true,
                fps_limit: Some(144),
                limits: true,
            },
            "the user's first limit"
        );
        let file = session_config(USER, state);
        assert_eq!(
            file,
            "# Written by Irori for this session; it goes when the game exits.\n\
             # my HUD\nposition=top-right\ngpu_stats\n# irori\nfps_limit=144\n"
        );
        assert_eq!(parse_state(&file), Some(state));

        // Hidden, with Irori's own limit, then changed during play.
        let start = initial_state(USER, true, Some(60), true);
        let file = session_config(USER, start);
        assert_eq!(parse_state(&file), Some(start));
        let shown = MangoState {
            visible: true,
            fps_limit: None,
            limits: true,
        };
        let changed = session_config(&file, shown);
        assert_eq!(parse_state(&changed), Some(shown));
        assert!(changed.contains("position=top-right"));
        assert_eq!(changed.matches("# irori").count(), 1, "one marker");
        assert_eq!(changed.matches("Written by Irori").count(), 1, "one header");
        // A file that isn't Irori's says nothing.
        assert_eq!(parse_state(USER), None);
        assert!(!initial_state("no_display\n", false, None, true).visible);
        // gamescope or DXVK limits the session: the file says so.
        let other = initial_state("", false, None, false);
        let file = session_config("", other);
        assert_eq!(parse_state(&file), Some(other));
        assert!(!parse_state(&file).unwrap().limits);
    }

    #[test]
    fn a_variable_without_read_cfg_carries_the_options() {
        assert!(env_only(Some("fps,frametime")));
        assert!(!env_only(Some("read_cfg,fps")));
        assert!(!env_only(Some("\"read_cfg\"")));
        assert!(!env_only(None));
        assert!(!env_only(Some("  ")));
        let state = MangoState {
            visible: false,
            fps_limit: Some(75),
            limits: true,
        };
        assert_eq!(
            env_config("\"fps,fps_limit=30,no_display\"", state),
            "fps,fps_limit=75,no_display"
        );
        assert_eq!(
            env_config(
                "full",
                MangoState {
                    visible: true,
                    ..Default::default()
                }
            ),
            "full"
        );
    }

    #[test]
    fn session_source_survives_overlay_writes_and_syncs_in_place() {
        let state = MangoState {
            visible: true,
            fps_limit: Some(60),
            limits: true,
        };
        let seeded = session_config_with_source(
            "legacy_layout=false\npreset=3,4,1,2\nposition=top-left\n",
            state,
            Some("global"),
        );
        assert_eq!(parse_source(&seeded), Some("global"));
        assert_eq!(parse_state(&seeded), Some(state));

        // Rewriting with the same source preserves it and updates base lines.
        let hidden = MangoState {
            visible: false,
            fps_limit: Some(90),
            limits: false,
        };
        let after_overlay = session_config_with_source(&seeded, hidden, parse_source(&seeded));
        assert_eq!(parse_source(&after_overlay), Some("global"));
        assert_eq!(parse_state(&after_overlay), Some(hidden));
        assert!(after_overlay.contains("preset=3,4,1,2"));
    }
}
