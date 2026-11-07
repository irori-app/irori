//! What Irori's applications in Sunshine should be, and what to change to
//! get there. Pure: the sync reads Sunshine's list, plans, then applies each
//! step against a fresh list, since Sunshine sorts its applications by name
//! after every change and indexes move.

use std::collections::{BTreeMap, HashMap};

use irori_core::{GameId, GameInfo, Runner, SunshineGames};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The key Irori marks its applications with. Sunshine keeps keys it doesn't
/// know in `apps.json`, so the mark survives restarts and other edits.
pub(crate) const MARK: &str = "irori-id";

/// The mark of the application that opens Big Picture.
pub(crate) const BIG_PICTURE: &str = "big-picture";

/// How long Sunshine waits for Irori to stop a game when the stream ends,
/// before ending it itself.
const EXIT_TIMEOUT: u64 = 10;

/// One application Irori wants in Sunshine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Wanted {
    /// The mark: a game's `runner:app`, or [`BIG_PICTURE`].
    pub id: String,
    pub name: String,
    pub cmd: String,
    /// The art to show in Moonlight, as an address or a `file://` path.
    pub cover: Option<String>,
}

/// A change to Sunshine's applications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    Add(Wanted),
    /// Rewrite Irori's own fields of the application marked `id`.
    Update(Wanted),
    /// Remove the application marked `id`.
    Remove(String),
}

/// The games Sunshine should list: installed, shown in the library and, if
/// asked, favourites. Each name is unique among them.
pub(crate) fn wanted(
    games: &[std::sync::Arc<GameInfo>],
    hidden: &std::collections::BTreeSet<String>,
    favourites: &std::collections::BTreeSet<String>,
    which: SunshineGames,
    big_picture: Option<String>,
    command: impl Fn(&GameId) -> Option<String>,
) -> Vec<Wanted> {
    let picked: Vec<&GameInfo> = games
        .iter()
        .map(|g| g.as_ref())
        .filter(|g| g.is_installed && !g.is_dlc)
        .filter(|g| !hidden.contains(&g.id().key()))
        .filter(|g| which == SunshineGames::Installed || favourites.contains(&g.id().key()))
        .collect();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for g in &picked {
        *seen.entry(g.title.trim().to_lowercase()).or_default() += 1;
    }
    let mut out: Vec<Wanted> = picked
        .into_iter()
        .filter_map(|g| {
            let id = g.id();
            let cmd = command(&id)?;
            let title = g.title.trim();
            // Two games of one title, from two stores, are told apart.
            let name = if seen[&title.to_lowercase()] > 1 {
                format!("{title} ({})", store_label(id.runner))
            } else {
                title.to_string()
            };
            Some(Wanted {
                id: id.key(),
                name,
                cmd,
                cover: g.cover_art().map(str::to_string),
            })
        })
        .collect();
    if let Some(cmd) = big_picture {
        out.push(Wanted {
            id: BIG_PICTURE.into(),
            name: "Irori Big Picture".into(),
            cmd,
            cover: None,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn store_label(runner: Runner) -> &'static str {
    match runner {
        Runner::Sideload => "Irori",
        Runner::Emulation => "ROM",
        other => other.store_name(),
    }
}

/// The mark of an application Irori added: the mark itself or, when an edit
/// in Sunshine's web UI dropped it, the game its command starts.
pub(crate) fn mark_of(app: &Value, program: &str) -> Option<String> {
    if let Some(mark) = app.get(MARK).and_then(Value::as_str) {
        return Some(mark.to_string());
    }
    let cmd = app.get("cmd").and_then(Value::as_str)?;
    let mut words = split_command(cmd).into_iter();
    if words.next()? != program {
        return None;
    }
    let rest: Vec<String> = words.collect();
    if rest.iter().any(|w| w == "--big-picture") {
        return Some(BIG_PICTURE.into());
    }
    let ix = rest.iter().position(|w| w == "--launch")?;
    let (runner, app) = rest.get(ix + 1)?.split_once('/')?;
    Some(GameId::new(Runner::from_id(runner)?, app).key())
}

/// What to change so Sunshine lists `wanted` among Irori's applications.
/// Other applications are never touched.
pub(crate) fn steps(current: &[Value], wanted: &[Wanted], program: &str) -> Vec<Step> {
    let mut ours: BTreeMap<String, &Value> = BTreeMap::new();
    let mut extra = Vec::new();
    for app in current {
        if let Some(mark) = mark_of(app, program) {
            // A second application of one game is a leftover.
            if ours.insert(mark.clone(), app).is_some() {
                extra.push(mark);
            }
        }
    }
    let mut out: Vec<Step> = extra.into_iter().map(Step::Remove).collect();
    for w in wanted {
        match ours.remove(&w.id) {
            None => out.push(Step::Add(w.clone())),
            Some(app) if needs_update(app, w) => out.push(Step::Update(w.clone())),
            Some(_) => {}
        }
    }
    out.extend(ours.into_keys().map(Step::Remove));
    out
}

fn needs_update(app: &Value, w: &Wanted) -> bool {
    let text = |key: &str| app.get(key).and_then(Value::as_str).unwrap_or_default();
    text("name") != w.name
        || text("cmd") != w.cmd
        || app.get(MARK).is_none()
        || app.get("wait-all").and_then(Value::as_bool) != Some(true)
        || app.get("auto-detach").and_then(Value::as_bool) != Some(false)
}

/// `app` with Irori's fields set from `w`, keeping everything else the user
/// set in Sunshine (preparation commands, output, working folder…).
/// `image` is the cover's path in Sunshine, when it has one.
pub(crate) fn merged(app: Option<&Value>, w: &Wanted, image: Option<&str>) -> Value {
    let mut map = app.and_then(Value::as_object).cloned().unwrap_or_default();
    map.insert("name".into(), w.name.clone().into());
    map.insert("cmd".into(), w.cmd.clone().into());
    map.insert(MARK.into(), w.id.clone().into());
    // Irori's command lasts as long as the game, which Sunshine follows with
    // the whole process group; it never hands over early.
    map.insert("wait-all".into(), true.into());
    map.insert("auto-detach".into(), false.into());
    map.entry("exit-timeout").or_insert(EXIT_TIMEOUT.into());
    for (key, value) in [
        ("output", Value::String(String::new())),
        ("exclude-global-prep-cmd", false.into()),
        ("elevated", false.into()),
        ("prep-cmd", Value::Array(Vec::new())),
        ("detached", Value::Array(Vec::new())),
    ] {
        map.entry(key).or_insert(value);
    }
    if let Some(image) = image {
        map.insert("image-path".into(), image.into());
    } else {
        map.entry("image-path")
            .or_insert(Value::String(String::new()));
    }
    Value::Object(map)
}

/// Where `mark`'s application sits in Sunshine's list now.
pub(crate) fn index_of(current: &[Value], mark: &str, program: &str) -> Option<usize> {
    current
        .iter()
        .position(|app| mark_of(app, program).as_deref() == Some(mark))
}

/// The application `mark` names in Sunshine's list.
pub(crate) fn find<'a>(current: &'a [Value], mark: &str, program: &str) -> Option<&'a Value> {
    index_of(current, mark, program).map(|ix| &current[ix])
}

/// A Sunshine command line running `program` with `args`. Sunshine splits
/// it like a shell on Linux and macOS and hands it to `CreateProcess` on
/// Windows; both take double quotes around a word with spaces.
pub(crate) fn command_line(program: &str, args: &[&str]) -> Option<String> {
    let quote = |word: &str| -> Option<String> {
        if word.contains('"') {
            return None;
        }
        Some(if word.is_empty() || word.contains(char::is_whitespace) {
            format!("\"{word}\"")
        } else {
            word.to_string()
        })
    };
    let mut out = vec![format!("\"{}\"", quote(program)?.trim_matches('"'))];
    for arg in args {
        out.push(quote(arg)?);
    }
    Some(out.join(" "))
}

/// The words of a command line built by [`command_line`].
fn split_command(cmd: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in cmd.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                any = false;
            }
            c => word.push(c),
        }
    }
    if any || !word.is_empty() {
        words.push(word);
    }
    words
}

/// Sunshine's cover key for `mark`: letters, digits, `-` and `_` only, since
/// it names a file of Sunshine's.
pub(crate) fn cover_key(mark: &str) -> String {
    let clean: String = mark
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("irori-{clean}")
}

/// Keys of `obj` that hold nothing, so a merge leaves them out.
#[cfg(test)]
fn keys(obj: &Value) -> Vec<&str> {
    obj.as_object()
        .map(|m| m.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use serde_json::json;

    use super::*;

    const PROGRAM: &str = "/opt/Irori/irori";

    fn game(runner: Runner, app: &str, title: &str, installed: bool) -> Arc<GameInfo> {
        Arc::new(GameInfo {
            runner: Some(runner),
            app_name: app.into(),
            title: title.into(),
            is_installed: installed,
            art_cover: Some(format!("https://art/{app}.jpg")),
            ..Default::default()
        })
    }

    fn cmd(id: &GameId) -> Option<String> {
        let target = format!("{}/{}", id.runner.id(), id.app_name);
        command_line(PROGRAM, &["--launch", &target, "--wait"])
    }

    #[test]
    fn picks_installed_shown_games_and_tells_same_titles_apart() {
        let games = vec![
            game(Runner::Gog, "1", "Hades", true),
            game(Runner::Legendary, "Min", "Hades", true),
            game(Runner::Gog, "2", "Celeste", false),
            game(Runner::Gog, "3", "Hidden one", true),
        ];
        let hidden = BTreeSet::from(["gog:3".to_string()]);
        let wanted = wanted(
            &games,
            &hidden,
            &BTreeSet::new(),
            SunshineGames::Installed,
            None,
            cmd,
        );
        let names: Vec<&str> = wanted.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["Hades (GOG)", "Hades (Epic Games)"]);
        assert_eq!(wanted[0].cmd, r#""/opt/Irori/irori" --launch gog/1 --wait"#);
        assert_eq!(wanted[0].cover.as_deref(), Some("https://art/1.jpg"));
    }

    #[test]
    fn favourites_only_and_the_big_picture_app() {
        let games = vec![
            game(Runner::Gog, "1", "Hades", true),
            game(Runner::Gog, "2", "Celeste", true),
        ];
        let favourites = BTreeSet::from(["gog:2".to_string()]);
        let big = command_line(PROGRAM, &["--big-picture", "--wait"]);
        let wanted = wanted(
            &games,
            &BTreeSet::new(),
            &favourites,
            SunshineGames::Favourites,
            big,
            cmd,
        );
        let ids: Vec<&str> = wanted.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, [BIG_PICTURE, "gog:2"]);
    }

    #[test]
    fn only_irori_applications_change() {
        let wanted = vec![
            Wanted {
                id: "gog:1".into(),
                name: "Hades".into(),
                cmd: cmd(&GameId::new(Runner::Gog, "1")).unwrap(),
                cover: None,
            },
            Wanted {
                id: "gog:2".into(),
                name: "Celeste".into(),
                cmd: cmd(&GameId::new(Runner::Gog, "2")).unwrap(),
                cover: None,
            },
        ];
        let current = vec![
            json!({"name": "Desktop", "image-path": "desktop.png"}),
            json!({"name": "Steam Big Picture", "cmd": "steam steam://open/bigpicture"}),
            merged(None, &wanted[0], None),
            json!({"name": "Old", "cmd": "x", MARK: "gog:9"}),
        ];
        assert_eq!(
            steps(&current, &wanted, PROGRAM),
            [Step::Add(wanted[1].clone()), Step::Remove("gog:9".into())]
        );
    }

    #[test]
    fn finds_applications_whose_mark_an_edit_dropped() {
        let w = Wanted {
            id: "legendary:Fortnite".into(),
            name: "Fortnite".into(),
            cmd: cmd(&GameId::new(Runner::Legendary, "Fortnite")).unwrap(),
            cover: None,
        };
        let mut edited = merged(None, &w, None);
        edited.as_object_mut().unwrap().remove(MARK);
        assert_eq!(
            mark_of(&edited, PROGRAM).as_deref(),
            Some("legendary:Fortnite")
        );
        // It is still Irori's, so it gets its mark back rather than a twin.
        assert_eq!(
            steps(&[edited], std::slice::from_ref(&w), PROGRAM),
            [Step::Update(w)]
        );
        let other = json!({"name": "x", "cmd": "\"/usr/bin/lutris\" --launch gog/1"});
        assert_eq!(mark_of(&other, PROGRAM), None);
    }

    #[test]
    fn a_rename_updates_and_keeps_the_users_own_fields() {
        let mut w = Wanted {
            id: "gog:1".into(),
            name: "Hades".into(),
            cmd: cmd(&GameId::new(Runner::Gog, "1")).unwrap(),
            cover: None,
        };
        let mut app = merged(None, &w, Some("/covers/irori-gog-1.png"));
        app["prep-cmd"] = json!([{"do": "xrandr --output HDMI-1 --mode 1280x800", "undo": ""}]);
        app["exit-timeout"] = json!(30);
        w.name = "Hades II".into();
        assert_eq!(
            steps(std::slice::from_ref(&app), &[w.clone()], PROGRAM),
            [Step::Update(w.clone())]
        );
        let new = merged(Some(&app), &w, None);
        assert_eq!(new["name"], "Hades II");
        assert_eq!(new["prep-cmd"], app["prep-cmd"]);
        assert_eq!(new["exit-timeout"], 30);
        assert_eq!(new["image-path"], "/covers/irori-gog-1.png");
        assert_eq!(new["auto-detach"], false);
        assert_eq!(new["wait-all"], true);
        assert!(keys(&new).contains(&MARK));
    }

    #[test]
    fn duplicates_are_removed() {
        let w = Wanted {
            id: "gog:1".into(),
            name: "Hades".into(),
            cmd: cmd(&GameId::new(Runner::Gog, "1")).unwrap(),
            cover: None,
        };
        let app = merged(None, &w, None);
        assert_eq!(
            steps(&[app.clone(), app], &[w], PROGRAM),
            [Step::Remove("gog:1".into())]
        );
    }

    #[test]
    fn command_lines_quote_paths_and_refuse_quotes() {
        assert_eq!(
            command_line(
                r"C:\Users\Ana Pop\Irori\irori.exe",
                &["--launch", "sideload/a b"]
            )
            .unwrap(),
            r#""C:\Users\Ana Pop\Irori\irori.exe" --launch "sideload/a b""#
        );
        assert_eq!(command_line(PROGRAM, &["--launch", "x\"y"]), None);
        let line = command_line("/a b/irori", &["--launch", "gog/1"]).unwrap();
        assert_eq!(split_command(&line), ["/a b/irori", "--launch", "gog/1"]);
    }

    #[test]
    fn cover_keys_are_plain_names() {
        assert_eq!(cover_key("legendary:Fortnite"), "irori-legendary-Fortnite");
        assert_eq!(cover_key("sideload:../x"), "irori-sideload----x");
    }
}
