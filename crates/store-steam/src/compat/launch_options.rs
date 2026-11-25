//! A game's launch options in Steam ("Properties → General → Launch
//! options"): the line Steam keeps per account in
//! `userdata/<account>/config/localconfig.vdf`, under
//! `UserLocalConfigStore/Software/Valve/Steam/apps/<appid>/LaunchOptions`.
//!
//! Steam reads the file when it starts and rewrites it when it exits, so it
//! is changed under the rules of `config.vdf` ([`super`]): only while Steam is
//! closed, splicing only the bytes of that one value, refusing a file the
//! strict parser can't read, reading every edit back, and keeping the
//! previous file as `localconfig.vdf.irori-backup`.
//!
//! The line itself is Steam's: `VAR=value wrapper %command% arguments`, or
//! just arguments without `%command%`. [`with_variable`] changes one
//! variable in it and leaves the rest as the user wrote it.

use anyhow::{Context as _, Result, bail};
use irori_runtime::vdf::{self, Vdf};

use super::mapping::{
    Node, Splice, Value, find, insert_before_close, leaf, object_block, parse_strict, quote,
    remove_entry,
};

/// The apps' section, from the file's root.
const APPS_PATH: [&str; 5] = ["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"];

/// The value's key inside a game's section.
const KEY: &str = "LaunchOptions";

/// Steam's placeholder for the game's own command.
pub const COMMAND: &str = "%command%";

/// The launch options `localconfig.vdf` holds for `app_id`; `None` when
/// there are none.
pub(crate) fn launch_options(text: &str, app_id: &str) -> Option<String> {
    let doc = vdf::parse(text);
    let app = APPS_PATH
        .iter()
        .try_fold(&doc, |node, key| node.get(key))?
        .get(app_id)?;
    let line = app.get(KEY)?.as_str()?.trim();
    (!line.is_empty()).then(|| line.to_string())
}

/// `text` with the launch options of `app_id` set to `line`, or removed
/// for `None` or an empty line. Nothing but that value changes.
pub(crate) fn set_launch_options(text: &str, app_id: &str, line: Option<&str>) -> Result<String> {
    if app_id.is_empty() || !app_id.bytes().all(|b| b.is_ascii_digit()) {
        bail!("“{app_id}” is not a Steam app ID");
    }
    let line = line.map(str::trim).filter(|l| !l.is_empty());
    if let Some(line) = line
        && line.chars().any(char::is_control)
    {
        bail!("Launch options are one line, without tabs or line breaks");
    }
    let root = parse_strict(text).context("Steam's account settings file could not be read")?;
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let edited = match plan(text, &root, app_id, line, newline)? {
        None => return Ok(text.to_string()),
        Some(splice) => splice.apply(text),
    };
    verify(text, &edited, app_id, line)?;
    Ok(edited)
}

fn plan(
    text: &str,
    root: &[Node],
    app_id: &str,
    line: Option<&str>,
    nl: &str,
) -> Result<Option<Splice>> {
    let Some(store) = find(root, APPS_PATH[0]) else {
        bail!("This is not Steam's account settings file");
    };
    let path: Vec<&str> = APPS_PATH[1..].iter().copied().chain([app_id]).collect();
    // Walk down as far as the path exists; the rest is created.
    let mut parent = store;
    let mut depth = 1;
    for key in &path {
        let Value::Obj { children, .. } = &parent.value else {
            bail!("Steam's account settings file has an unexpected layout");
        };
        match find(children, key) {
            Some(child) if matches!(child.value, Value::Obj { .. }) => {
                parent = child;
                depth += 1;
            }
            Some(_) => bail!("Steam's account settings file has an unexpected layout"),
            None => break,
        }
    }
    let Value::Obj {
        children, close, ..
    } = &parent.value
    else {
        bail!("Steam's account settings file has an unexpected layout");
    };
    let missing = &path[depth - 1..];
    if !missing.is_empty() {
        let Some(line) = line else {
            return Ok(None);
        };
        let mut block = leaf(KEY, line, depth + missing.len(), nl);
        for (i, key) in missing.iter().enumerate().rev() {
            block = object_block(key, &block, depth + i, nl);
        }
        return Ok(Some(insert_before_close(text, *close, block, depth, nl)));
    }
    // `parent` is the game's own section, `depth` deep.
    match (find(children, KEY), line) {
        (None, None) => Ok(None),
        (None, Some(line)) => Ok(Some(insert_before_close(
            text,
            *close,
            leaf(KEY, line, depth, nl),
            depth,
            nl,
        ))),
        (Some(entry), None) => Ok(Some(remove_entry(text, entry))),
        (
            Some(Node {
                value:
                    Value::Str {
                        start,
                        end,
                        text: current,
                    },
                ..
            }),
            Some(line),
        ) => Ok((current != line).then(|| Splice {
            start: *start,
            end: *end,
            with: quote(line),
        })),
        (Some(_), Some(_)) => bail!("Steam's account settings file has an unexpected layout"),
    }
}

/// The edited file reads back with the new line, and apart from it both
/// files hold the same keys and values in the same order.
fn verify(before: &str, after: &str, app_id: &str, line: Option<&str>) -> Result<()> {
    parse_strict(after).context("The changed settings file could not be read back")?;
    if launch_options(after, app_id).as_deref() != line {
        bail!("The changed settings file doesn't hold the new launch options");
    }
    let strip = |text: &str| {
        let mut doc = vdf::parse(text);
        strip_value(&mut doc, app_id);
        doc
    };
    if strip(before) != strip(after) {
        bail!("Changing the launch options would have changed other Steam settings");
    }
    Ok(())
}

/// Take the game's launch options out of a parsed file, then the sections
/// on their path left empty, so a file the edit gave new sections compares
/// equal to the original.
fn strip_value(doc: &mut Vdf, app_id: &str) {
    fn walk(node: &mut Vdf, path: &[&str]) {
        let Vdf::Obj(items) = node else { return };
        let Some((key, rest)) = path.split_first() else {
            items.retain(|(k, _)| !k.eq_ignore_ascii_case(KEY));
            return;
        };
        if let Some(i) = items.iter().position(|(k, _)| k.eq_ignore_ascii_case(key)) {
            walk(&mut items[i].1, rest);
            // The root section always stays.
            if path.len() < APPS_PATH.len() + 1 && items[i].1 == Vdf::Obj(Vec::new()) {
                items.remove(i);
            }
        }
    }
    let path: Vec<&str> = APPS_PATH.iter().copied().chain([app_id]).collect();
    walk(doc, &path);
}

// ---- the line itself ----

/// One shell word of a launch line, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Word<'a> {
    raw: &'a str,
    start: usize,
    end: usize,
}

/// The line's words, split the way a POSIX shell does (Steam hands the line
/// to `sh` on Linux and macOS): blanks separate words, quotes and `\` keep
/// them together.
fn words(line: &str) -> Vec<Word<'_>> {
    let mut out = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some(&(start, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut quote: Option<char> = None;
        let mut end = start;
        while let Some(&(i, c)) = chars.peek() {
            match (quote, c) {
                (None, c) if c.is_whitespace() => break,
                (None, '"' | '\'') => quote = Some(c),
                (Some(q), c) if c == q => quote = None,
                (q, '\\') if q != Some('\'') => {
                    chars.next();
                    end = i + 1;
                    if let Some(&(j, escaped)) = chars.peek() {
                        end = j + escaped.len_utf8();
                    }
                }
                _ => {}
            }
            if let Some((i, c)) = chars.next() {
                end = i + c.len_utf8();
            }
        }
        out.push(Word {
            raw: &line[start..end],
            start,
            end,
        });
    }
    out
}

/// The variable a word assigns (`NAME=…`), when it is an assignment.
fn assigned(word: &str) -> Option<&str> {
    let (name, _) = word.split_once('=')?;
    let mut chars = name.chars();
    let first = chars.next()?;
    ((first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_'))
    .then_some(name)
}

/// A value as one shell word: bare when it is plain, else single-quoted.
fn shell_quote(value: &str) -> String {
    let plain = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.,/:=+@%".contains(c));
    if plain {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

/// The value a launch line gives `name` before the game's command, without
/// its quotes; `None` when it sets none.
pub fn variable(line: &str, name: &str) -> Option<String> {
    let words = words(line);
    if !words.iter().any(|w| w.raw == COMMAND) {
        return None;
    }
    words
        .iter()
        .take_while(|w| assigned(w.raw).is_some())
        .find(|w| assigned(w.raw) == Some(name))
        .map(|w| unquote(&w.raw[name.len() + 1..]))
}

/// A shell word without its quoting.
fn unquote(word: &str) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    let mut chars = word.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (q, '\\') if q != Some('\'') => {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// `line` with the variable `name` set to `value` before the game's command,
/// or taken out for `None`. Everything else stays as written: other
/// variables, wrappers and the game's arguments. A line of bare arguments
/// gains `%command%` when a variable is added, and loses it when the last
/// variable goes and nothing but arguments is left.
pub fn with_variable(line: &str, name: &str, value: Option<&str>) -> String {
    let line = line.trim();
    let words = words(line);
    let has_command = words.iter().any(|w| w.raw == COMMAND);
    let assignment = value.map(|v| format!("{name}={}", shell_quote(v)));
    if !has_command {
        return match assignment {
            None => line.to_string(),
            Some(a) if line.is_empty() => format!("{a} {COMMAND}"),
            Some(a) => format!("{a} {COMMAND} {line}"),
        };
    }
    let existing = words
        .iter()
        .take_while(|w| assigned(w.raw).is_some())
        .find(|w| assigned(w.raw) == Some(name));
    let edited = match (existing, assignment) {
        (Some(w), Some(a)) => format!("{}{a}{}", &line[..w.start], &line[w.end..]),
        (Some(w), None) => {
            let rest = line[w.end..].trim_start();
            format!("{}{rest}", &line[..w.start])
        }
        (None, Some(a)) => format!("{a} {line}"),
        (None, None) => line.to_string(),
    };
    let edited = edited.trim().to_string();
    // `%command% -arg` alone means the same as `-arg`.
    match edited.strip_prefix(COMMAND) {
        Some(rest) if rest.is_empty() || rest.starts_with(char::is_whitespace) => {
            rest.trim().to_string()
        }
        _ => edited,
    }
}

/// Wine's variable for DLLs to load from the game folder.
const DLL_OVERRIDES: &str = "WINEDLLOVERRIDES";

/// `line` with `WINEDLLOVERRIDES` loading each of `native` from the game
/// folder (`<dll>=n,b`) and saying nothing more about `unset`, the user's
/// other overrides kept. The variable goes when nothing is left in it.
pub fn with_dll_overrides(line: &str, native: &[&str], unset: &[&str]) -> String {
    let current = variable(line, DLL_OVERRIDES).unwrap_or_default();
    let mut value = without_dll_overrides(&current, unset);
    if !native.is_empty() {
        let add: Vec<(&str, &str)> = native.iter().map(|d| (*d, "n,b")).collect();
        value = irori_runtime::env::add_dll_overrides(Some(&value), &add);
    }
    if value == current && variable(line, DLL_OVERRIDES).is_some() != value.is_empty() {
        return line.trim().to_string();
    }
    with_variable(
        line,
        DLL_OVERRIDES,
        (!value.is_empty()).then_some(value.as_str()),
    )
}

/// A `WINEDLLOVERRIDES` value without the entries naming `dlls`.
pub fn without_dll_overrides(value: &str, dlls: &[&str]) -> String {
    let norm = |name: &str| {
        let name = name.trim().to_ascii_lowercase();
        name.strip_suffix(".dll").map(str::to_owned).unwrap_or(name)
    };
    let gone: Vec<String> = dlls.iter().map(|d| norm(d)).collect();
    value
        .split(';')
        .filter_map(|entry| {
            let entry = entry.trim();
            let (names, mode) = match entry.split_once('=') {
                Some((n, m)) => (n, Some(m)),
                None => (entry, None),
            };
            let kept: Vec<&str> = names
                .split(',')
                .map(str::trim)
                .filter(|d| !d.is_empty() && !gone.contains(&norm(d)))
                .collect();
            (!kept.is_empty()).then(|| match mode {
                Some(mode) => format!("{}={mode}", kept.join(",")),
                None => kept.join(","),
            })
        })
        .collect::<Vec<_>>()
        .join(";")
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCALCONFIG: &str = "\"UserLocalConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"apps\"\n\t\t\t\t{\n\t\t\t\t\t\"620\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"LastPlayed\"\t\t\"1700000000\"\n\t\t\t\t\t\t\"LaunchOptions\"\t\t\"-novid\"\n\t\t\t\t\t}\n\t\t\t\t\t\"440\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"Playtime\"\t\t\"12\"\n\t\t\t\t\t}\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n\t\"friends\"\n\t{\n\t\t\"PersonaName\"\t\t\"ana\"\n\t}\n}\n";

    #[test]
    fn reads_a_games_launch_options() {
        assert_eq!(
            launch_options(LOCALCONFIG, "620").as_deref(),
            Some("-novid")
        );
        assert_eq!(launch_options(LOCALCONFIG, "440"), None);
        assert_eq!(launch_options(LOCALCONFIG, "1"), None);
        assert_eq!(launch_options("garbage", "620"), None);
    }

    #[test]
    fn replaces_only_the_value() {
        let line = r#"PROTON_LOG=1 %command% -name "Ana B""#;
        let edited = set_launch_options(LOCALCONFIG, "620", Some(line)).unwrap();
        assert_eq!(launch_options(&edited, "620").as_deref(), Some(line));
        // Byte for byte the same apart from the value.
        let expected =
            LOCALCONFIG.replace("\"-novid\"", r#""PROTON_LOG=1 %command% -name \"Ana B\"""#);
        assert_eq!(edited, expected);
        // The same line again changes nothing.
        assert_eq!(
            set_launch_options(&edited, "620", Some(line)).unwrap(),
            edited
        );
    }

    #[test]
    fn adds_a_value_and_its_sections() {
        let edited = set_launch_options(LOCALCONFIG, "440", Some("-console")).unwrap();
        assert_eq!(launch_options(&edited, "440").as_deref(), Some("-console"));
        assert!(edited.contains("\t\t\t\t\t\t\"LaunchOptions\"\t\t\"-console\"\n\t\t\t\t\t}"));
        // A game Steam has no section for yet.
        let edited = set_launch_options(LOCALCONFIG, "1091500", Some("-skipStartScreen")).unwrap();
        assert_eq!(
            launch_options(&edited, "1091500").as_deref(),
            Some("-skipStartScreen")
        );
        assert_eq!(launch_options(&edited, "620").as_deref(), Some("-novid"));
        // A file without an apps section at all.
        let bare = "\"UserLocalConfigStore\"\n{\n\t\"friends\"\n\t{\n\t}\n}\n";
        let edited = set_launch_options(bare, "620", Some("-novid")).unwrap();
        assert_eq!(launch_options(&edited, "620").as_deref(), Some("-novid"));
        assert!(edited.contains("\"friends\""));
    }

    #[test]
    fn removes_a_value_with_its_line() {
        let edited = set_launch_options(LOCALCONFIG, "620", None).unwrap();
        assert_eq!(launch_options(&edited, "620"), None);
        assert_eq!(
            edited,
            LOCALCONFIG.replace("\t\t\t\t\t\t\"LaunchOptions\"\t\t\"-novid\"\n", "")
        );
        // Blank counts as none; nothing to remove changes nothing.
        assert_eq!(
            set_launch_options(&edited, "620", Some("  ")).unwrap(),
            edited
        );
        assert_eq!(
            set_launch_options(LOCALCONFIG, "99", None).unwrap(),
            LOCALCONFIG
        );
    }

    #[test]
    fn refuses_what_it_cant_read_or_write() {
        assert!(set_launch_options("\"UserLocalConfigStore\"\n{\n", "620", Some("-x")).is_err());
        assert!(set_launch_options("\"Other\"\n{\n}\n", "620", Some("-x")).is_err());
        assert!(set_launch_options(LOCALCONFIG, "abc", Some("-x")).is_err());
        assert!(set_launch_options(LOCALCONFIG, "620", Some("-a\n-b")).is_err());
        // CRLF files stay CRLF.
        let crlf = LOCALCONFIG.replace('\n', "\r\n");
        let edited = set_launch_options(&crlf, "440", Some("-x")).unwrap();
        assert!(!edited.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn splits_lines_like_a_shell() {
        fn raw(l: &str) -> Vec<&str> {
            words(l).into_iter().map(|w| w.raw).collect()
        }
        assert_eq!(
            raw(r#"A="x y" B='p q' gamemoderun %command% -n "a b" c\ d"#),
            [
                r#"A="x y""#,
                "B='p q'",
                "gamemoderun",
                COMMAND,
                "-n",
                r#""a b""#,
                r"c\ d"
            ]
        );
        assert_eq!(unquote(r#""x y""#), "x y");
        assert_eq!(unquote(r"c\ d"), "c d");
        assert_eq!(
            assigned("WINEDLLOVERRIDES=dxgi=n,b"),
            Some("WINEDLLOVERRIDES")
        );
        assert_eq!(assigned("-x=1"), None);
        assert_eq!(assigned("%command%"), None);
    }

    #[test]
    fn changes_one_variable_and_keeps_the_rest() {
        let key = "WINEDLLOVERRIDES";
        assert_eq!(
            with_variable("", key, Some("dxgi=n,b")),
            "WINEDLLOVERRIDES=dxgi=n,b %command%"
        );
        assert_eq!(
            with_variable("-novid", key, Some("dxgi=n,b")),
            "WINEDLLOVERRIDES=dxgi=n,b %command% -novid"
        );
        let line = "PROTON_LOG=1 WINEDLLOVERRIDES=\"d3d9=n\" mangohud %command% -x";
        assert_eq!(variable(line, key).as_deref(), Some("d3d9=n"));
        assert_eq!(
            with_variable(line, key, Some("d3d9=n;dxgi=n,b")),
            "PROTON_LOG=1 WINEDLLOVERRIDES='d3d9=n;dxgi=n,b' mangohud %command% -x"
        );
        assert_eq!(
            with_variable(line, key, None),
            "PROTON_LOG=1 mangohud %command% -x"
        );
        // Taking the only variable out leaves the arguments as they were.
        assert_eq!(
            with_variable("WINEDLLOVERRIDES=dxgi=n,b %command% -novid", key, None),
            "-novid"
        );
        assert_eq!(
            with_variable("WINEDLLOVERRIDES=dxgi=n,b %command%", key, None),
            ""
        );
        // A wrapper's own NAME=value after the wrapper isn't the line's.
        let wrapped = "gamescope -- env A=1 %command%";
        assert_eq!(variable(wrapped, "A"), None);
        assert_eq!(
            with_variable(wrapped, "A", Some("2")),
            "A=2 gamescope -- env A=1 %command%"
        );
        assert_eq!(variable("-novid", key), None);
    }

    #[test]
    fn sets_and_unsets_dll_overrides_in_a_line() {
        let all = ["dxgi", "d3dcompiler_47"];
        assert_eq!(
            with_dll_overrides("-novid", &all, &all),
            "WINEDLLOVERRIDES='dxgi=n,b;d3dcompiler_47=n,b' %command% -novid"
        );
        let line = "WINEDLLOVERRIDES=winmm=n,b %command%";
        let on = with_dll_overrides(line, &["dxgi"], &all);
        assert_eq!(on, "WINEDLLOVERRIDES='winmm=n,b;dxgi=n,b' %command%");
        // Already so: the line stays as written.
        assert_eq!(with_dll_overrides(&on, &["dxgi"], &all), on);
        assert_eq!(
            with_dll_overrides(&on, &[], &all),
            "WINEDLLOVERRIDES=winmm=n,b %command%"
        );
        assert_eq!(
            with_dll_overrides("WINEDLLOVERRIDES=dxgi=n,b %command% -x", &[], &all),
            "-x"
        );
        assert_eq!(with_dll_overrides("", &[], &all), "");
    }

    #[test]
    fn removes_dll_overrides() {
        assert_eq!(
            without_dll_overrides("d3d9=n;dxgi,d3d11=n,b;winmm", &["dxgi.dll", "winmm"]),
            "d3d9=n;d3d11=n,b"
        );
        assert_eq!(without_dll_overrides("dxgi=n,b", &["DXGI"]), "");
    }
}
