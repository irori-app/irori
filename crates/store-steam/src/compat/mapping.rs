//! Reading and changing one game's entry of `CompatToolMapping` in Steam's
//! `config/config.vdf`, the setting behind "Force the use of a specific
//! Steam Play compatibility tool".
//!
//! The file holds every setting of the Steam client, so a change splices
//! only the bytes of that one entry into the text as Steam wrote it: every
//! other key, its order, spacing, comments and escaping stay byte for byte.
//! A file this parser can't read completely is refused rather than guessed
//! at, and every edit is read back before it is accepted.

use anyhow::{Context as _, Result, bail};
use irori_runtime::vdf::{self, Vdf};

/// Where the mapping lives, from the file's root.
const MAPPING_PATH: [&str; 5] = [
    "InstallConfigStore",
    "Software",
    "Valve",
    "Steam",
    "CompatToolMapping",
];

/// The priority Steam gives a tool the user picked for one game (tools
/// Valve assigns use lower ones).
const USER_PRIORITY: &str = "250";

/// The tool `config.vdf` maps `app_id` to, if any.
pub(crate) fn compat_tool(text: &str, app_id: &str) -> Option<String> {
    let doc = vdf::parse(text);
    let mapping = MAPPING_PATH
        .iter()
        .try_fold(&doc, |node, key| node.get(key))?;
    let name = mapping.get(app_id)?.get("name")?.as_str()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// `text` with `app_id` mapped to `tool`, or with its mapping removed (Steam's
/// default) for `None`. Nothing but that entry changes.
pub(crate) fn set_compat_tool(text: &str, app_id: &str, tool: Option<&str>) -> Result<String> {
    if app_id.is_empty() || !app_id.bytes().all(|b| b.is_ascii_digit()) {
        bail!("“{app_id}” is not a Steam app ID");
    }
    if let Some(tool) = tool
        && (tool.trim().is_empty() || tool.chars().any(char::is_control))
    {
        bail!("“{tool}” is not a compatibility tool name");
    }
    let root = parse_strict(text).context("Steam's settings file could not be read")?;
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let edit = plan(text, &root, app_id, tool, newline)?;
    let edited = match edit {
        None => return Ok(text.to_string()),
        Some(Splice { start, end, with }) => {
            let mut out = String::with_capacity(text.len() + with.len());
            out.push_str(&text[..start]);
            out.push_str(&with);
            out.push_str(&text[end..]);
            out
        }
    };
    verify(text, &edited, app_id, tool)?;
    Ok(edited)
}

/// Replace `start..end` of the text with `with`.
struct Splice {
    start: usize,
    end: usize,
    with: String,
}

fn plan(
    text: &str,
    root: &[Node],
    app_id: &str,
    tool: Option<&str>,
    nl: &str,
) -> Result<Option<Splice>> {
    let Some(config) = find(root, MAPPING_PATH[0]) else {
        bail!("This is not Steam's settings file");
    };
    // Walk down as far as the path exists; the rest is created.
    let mut parent = config;
    let mut depth = 1;
    for key in &MAPPING_PATH[1..] {
        let Value::Obj { children, .. } = &parent.value else {
            bail!("Steam's settings file has an unexpected layout");
        };
        match find(children, key) {
            Some(child) if matches!(child.value, Value::Obj { .. }) => {
                parent = child;
                depth += 1;
            }
            Some(_) => bail!("Steam's settings file has an unexpected layout"),
            None => break,
        }
    }
    let Value::Obj {
        children, close, ..
    } = &parent.value
    else {
        bail!("Steam's settings file has an unexpected layout");
    };
    let missing = &MAPPING_PATH[depth..];

    if !missing.is_empty() {
        // Nothing to remove where there is no mapping at all.
        let Some(tool) = tool else {
            return Ok(None);
        };
        let mut block = entry_block(app_id, tool, depth + missing.len(), nl);
        for (i, key) in missing.iter().enumerate().rev() {
            block = object_block(key, &block, depth + i, nl);
        }
        return Ok(Some(insert_before_close(text, *close, block, depth, nl)));
    }

    // `parent` is CompatToolMapping itself, `depth` deep.
    match children.iter().find(|n| n.key == app_id) {
        None => Ok(tool.map(|tool| {
            insert_before_close(
                text,
                *close,
                entry_block(app_id, tool, depth, nl),
                depth,
                nl,
            )
        })),
        Some(entry) => match (tool, &entry.value) {
            (None, _) => Ok(Some(remove_entry(text, entry))),
            (Some(tool), Value::Str { .. }) => Ok(Some(Splice {
                start: entry.key_start,
                end: entry.end(),
                with: entry_block(app_id, tool, depth, nl)
                    .trim_start_matches('\t')
                    .trim_end_matches(nl)
                    .to_string(),
            })),
            (
                Some(tool),
                Value::Obj {
                    children: fields,
                    close,
                    ..
                },
            ) => Ok(set_name(text, fields, *close, tool, depth + 1, nl)),
        },
    }
}

/// Point an existing entry at `tool`: its `name` value is replaced in place
/// (its `config` and `priority` stay), or added when it has none.
fn set_name(
    text: &str,
    fields: &[Node],
    close: usize,
    tool: &str,
    depth: usize,
    nl: &str,
) -> Option<Splice> {
    match find(fields, "name") {
        Some(Node {
            value:
                Value::Str {
                    start,
                    end,
                    text: current,
                },
            ..
        }) => (current != tool).then(|| Splice {
            start: *start,
            end: *end,
            with: quote(tool),
        }),
        _ => {
            let mut lines = leaf("name", tool, depth, nl);
            if find(fields, "config").is_none() {
                lines.push_str(&leaf("config", "", depth, nl));
            }
            if find(fields, "priority").is_none() {
                lines.push_str(&leaf("priority", USER_PRIORITY, depth, nl));
            }
            Some(insert_before_close(text, close, lines, depth, nl))
        }
    }
}

/// Take an entry out along with the line it stands on, when nothing else
/// shares that line.
fn remove_entry(text: &str, entry: &Node) -> Splice {
    let mut start = entry.key_start;
    let mut end = entry.end();
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let rest = &text[end..];
    let line_end = rest.find('\n').map(|i| end + i + 1);
    if text[line_start..start].trim().is_empty()
        && let Some(line_end) = line_end
        && text[end..line_end].trim().is_empty()
    {
        start = line_start;
        end = line_end;
    }
    Splice {
        start,
        end,
        with: String::new(),
    }
}

/// Insert `block` (whole lines) as the last child of the object closing at
/// `close`, which is `depth` deep.
fn insert_before_close(text: &str, close: usize, block: String, depth: usize, nl: &str) -> Splice {
    let line_start = text[..close].rfind('\n').map_or(0, |i| i + 1);
    if text[line_start..close].trim().is_empty() {
        Splice {
            start: line_start,
            end: line_start,
            with: block,
        }
    } else {
        // `{ … }` on one line: the block gets lines of its own.
        Splice {
            start: close,
            end: close,
            with: format!("{nl}{block}{}", tabs(depth - 1)),
        }
    }
}

fn tabs(depth: usize) -> String {
    "\t".repeat(depth)
}

/// Quoted as Steam writes strings: `\` and `"` escaped.
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn leaf(key: &str, value: &str, depth: usize, nl: &str) -> String {
    format!("{}{}\t\t{}{nl}", tabs(depth), quote(key), quote(value))
}

fn object_block(key: &str, inner: &str, depth: usize, nl: &str) -> String {
    let indent = tabs(depth);
    format!(
        "{indent}{}{nl}{indent}{{{nl}{inner}{indent}}}{nl}",
        quote(key)
    )
}

fn entry_block(app_id: &str, tool: &str, depth: usize, nl: &str) -> String {
    let fields = [
        leaf("name", tool, depth + 1, nl),
        leaf("config", "", depth + 1, nl),
        leaf("priority", USER_PRIORITY, depth + 1, nl),
    ]
    .concat();
    object_block(app_id, &fields, depth, nl)
}

/// Check the edit did what was asked and nothing else: the edited file
/// reads back with the new mapping, and apart from the game's entry both
/// files hold the same keys and values in the same order.
fn verify(before: &str, after: &str, app_id: &str, tool: Option<&str>) -> Result<()> {
    parse_strict(after).context("The changed settings file could not be read back")?;
    if compat_tool(after, app_id).as_deref() != tool {
        bail!("The changed settings file doesn't hold the new compatibility tool");
    }
    let strip = |text: &str| {
        let mut doc = vdf::parse(text);
        if let Some(mapping) = mapping_mut(&mut doc) {
            mapping.retain(|(k, _)| k != app_id);
        }
        prune_empty_path(&mut doc);
        doc
    };
    if strip(before) != strip(after) {
        bail!("Changing the compatibility tool would have changed other Steam settings");
    }
    Ok(())
}

fn mapping_mut(doc: &mut Vdf) -> Option<&mut Vec<(String, Vdf)>> {
    let mut node = doc;
    for key in MAPPING_PATH {
        let Vdf::Obj(items) = node else {
            return None;
        };
        node = items
            .iter_mut()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)?;
    }
    match node {
        Vdf::Obj(items) => Some(items),
        Vdf::Str(_) => None,
    }
}

/// Drop the objects of the mapping's path that hold nothing, so a file the
/// edit gave a new `CompatToolMapping` compares equal to the original once
/// the entry is taken out.
fn prune_empty_path(doc: &mut Vdf) {
    fn prune(node: &mut Vdf, path: &[&str]) {
        let (Some(key), Vdf::Obj(items)) = (path.first(), node) else {
            return;
        };
        if let Some(i) = items.iter().position(|(k, _)| k.eq_ignore_ascii_case(key)) {
            prune(&mut items[i].1, &path[1..]);
            if path.len() < MAPPING_PATH.len() && items[i].1 == Vdf::Obj(Vec::new()) {
                items.remove(i);
            }
        }
    }
    // The root object and InstallConfigStore always stay.
    prune(doc, &MAPPING_PATH);
}

// ---- a parser that remembers where everything is ----

#[derive(Debug)]
struct Node {
    key: String,
    /// Byte offset of the key's first character (its opening quote).
    key_start: usize,
    value: Value,
}

#[derive(Debug)]
enum Value {
    /// A string value: its token's span, quotes included, and its text.
    Str {
        start: usize,
        end: usize,
        text: String,
    },
    /// An object: the offset of its closing brace and its children.
    Obj { close: usize, children: Vec<Node> },
}

impl Node {
    /// The offset just past the entry.
    fn end(&self) -> usize {
        match &self.value {
            Value::Str { end, .. } => *end,
            Value::Obj { close, .. } => close + 1,
        }
    }
}

/// First node with `key`, ignoring case as Steam does.
fn find<'a>(nodes: &'a [Node], key: &str) -> Option<&'a Node> {
    nodes.iter().find(|n| n.key.eq_ignore_ascii_case(key))
}

#[derive(Debug, PartialEq)]
enum Tok {
    Str(String),
    Open,
    Close,
}

/// Tokens with their byte spans. Unlike [`vdf::parse`], which tolerates a
/// damaged file for reading, anything unexpected is an error here.
fn tokenize(text: &str) -> Result<Vec<(Tok, usize, usize)>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            c if c.is_ascii_whitespace() => i += 1,
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'{' => {
                out.push((Tok::Open, i, i + 1));
                i += 1;
            }
            b'}' => {
                out.push((Tok::Close, i, i + 1));
                i += 1;
            }
            b'"' => {
                let start = i;
                i += 1;
                let mut s = String::new();
                loop {
                    let Some(ch) = text[i..].chars().next() else {
                        bail!("a quoted string never ends");
                    };
                    i += ch.len_utf8();
                    match ch {
                        '"' => break,
                        '\\' => {
                            let Some(next) = text[i..].chars().next() else {
                                bail!("a quoted string never ends");
                            };
                            i += next.len_utf8();
                            match next {
                                'n' => s.push('\n'),
                                't' => s.push('\t'),
                                other => s.push(other),
                            }
                        }
                        ch => s.push(ch),
                    }
                }
                out.push((Tok::Str(s), start, i));
            }
            b'[' => {
                // A conditional such as `[$WIN32]` after a value.
                let Some(len) = text[i..].find(']') else {
                    bail!("a condition never ends");
                };
                i += len + 1;
            }
            _ => {
                let start = i;
                while i < bytes.len()
                    && !bytes[i].is_ascii_whitespace()
                    && !matches!(bytes[i], b'{' | b'}' | b'"')
                {
                    i += 1;
                }
                out.push((Tok::Str(text[start..i].to_string()), start, i));
            }
        }
    }
    Ok(out)
}

fn parse_strict(text: &str) -> Result<Vec<Node>> {
    let tokens = tokenize(text)?;
    let mut pos = 0;
    let nodes = parse_children(&tokens, &mut pos, false)?;
    if pos != tokens.len() {
        bail!("unexpected text after the last setting");
    }
    Ok(nodes)
}

fn parse_children(
    tokens: &[(Tok, usize, usize)],
    pos: &mut usize,
    nested: bool,
) -> Result<Vec<Node>> {
    let mut nodes = Vec::new();
    loop {
        let Some((tok, start, _)) = tokens.get(*pos) else {
            if nested {
                bail!("a section never ends");
            }
            return Ok(nodes);
        };
        match tok {
            Tok::Close if nested => return Ok(nodes),
            Tok::Close => bail!("a section ends that never began"),
            Tok::Open => bail!("a section has no name"),
            Tok::Str(key) => {
                *pos += 1;
                let value = match tokens.get(*pos) {
                    Some((Tok::Str(text), s, e)) => {
                        *pos += 1;
                        Value::Str {
                            start: *s,
                            end: *e,
                            text: text.clone(),
                        }
                    }
                    Some((Tok::Open, _, _)) => {
                        *pos += 1;
                        let children = parse_children(tokens, pos, true)?;
                        let Some((Tok::Close, close, _)) = tokens.get(*pos) else {
                            bail!("a section never ends");
                        };
                        *pos += 1;
                        Value::Obj {
                            close: *close,
                            children,
                        }
                    }
                    _ => bail!("“{key}” has no value"),
                };
                nodes.push(Node {
                    key: key.clone(),
                    key_start: *start,
                    value,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A trimmed `config.vdf` as Steam writes it, with neighbours of the
    /// mapping that must survive: other games' tools, Steam's global
    /// default (`"0"`), escaped paths, a comment and a conditional.
    const CONFIG: &str = "\"InstallConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"AutoUpdateWindowEnabled\"\t\t\"0\"\n\t\t\t\t\"BaseInstallFolder_1\"\t\t\"D:\\\\Games\\\\Steam \\\"Library\\\"\"\n\t\t\t\t// kept as written\n\t\t\t\t\"CompatToolMapping\"\n\t\t\t\t{\n\t\t\t\t\t\"0\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"proton_experimental\"\n\t\t\t\t\t\t\"config\"\t\t\"\"\n\t\t\t\t\t\t\"priority\"\t\t\"75\"\n\t\t\t\t\t}\n\t\t\t\t\t\"620\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"proton_9\"\n\t\t\t\t\t\t\"config\"\t\t\"\"\n\t\t\t\t\t\t\"priority\"\t\t\"250\"\n\t\t\t\t\t}\n\t\t\t\t}\n\t\t\t\t\"ShaderCacheManager\"\n\t\t\t\t{\n\t\t\t\t\t\"HasCurrentBucket\"\t\t\"1\" [$LINUX]\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n\t\"Music\"\n\t{\n\t\t\"CrawlSteamInstallFolders\"\t\t\"1\"\n\t}\n}\n";

    #[test]
    fn reads_a_games_tool() {
        assert_eq!(compat_tool(CONFIG, "620").as_deref(), Some("proton_9"));
        assert_eq!(
            compat_tool(CONFIG, "0").as_deref(),
            Some("proton_experimental")
        );
        assert_eq!(compat_tool(CONFIG, "70"), None);
        assert_eq!(compat_tool("garbage {", "620"), None);
    }

    #[test]
    fn changes_only_the_name_of_an_existing_entry() {
        let out = set_compat_tool(CONFIG, "620", Some("GE-Proton10-4")).unwrap();
        assert_eq!(
            out,
            CONFIG.replace("\"proton_9\"", "\"GE-Proton10-4\""),
            "nothing but the value changes"
        );
        assert_eq!(compat_tool(&out, "620").as_deref(), Some("GE-Proton10-4"));
        // The same choice again changes nothing.
        assert_eq!(
            set_compat_tool(&out, "620", Some("GE-Proton10-4")).unwrap(),
            out
        );
    }

    #[test]
    fn adds_an_entry_after_the_others() {
        let out = set_compat_tool(CONFIG, "70", Some("proton_hotfix")).unwrap();
        let block = "\t\t\t\t\t\"70\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"proton_hotfix\"\n\t\t\t\t\t\t\"config\"\t\t\"\"\n\t\t\t\t\t\t\"priority\"\t\t\"250\"\n\t\t\t\t\t}\n";
        let anchor = "\t\t\t\t}\n\t\t\t\t\"ShaderCacheManager\"";
        assert_eq!(out, CONFIG.replacen(anchor, &format!("{block}{anchor}"), 1));
        assert_eq!(compat_tool(&out, "70").as_deref(), Some("proton_hotfix"));
        assert_eq!(compat_tool(&out, "620").as_deref(), Some("proton_9"));
    }

    #[test]
    fn removing_restores_the_file_exactly() {
        let added = set_compat_tool(CONFIG, "70", Some("proton_hotfix")).unwrap();
        assert_eq!(set_compat_tool(&added, "70", None).unwrap(), CONFIG);
        let removed = set_compat_tool(CONFIG, "620", None).unwrap();
        assert_eq!(compat_tool(&removed, "620"), None);
        assert_eq!(
            compat_tool(&removed, "0").as_deref(),
            Some("proton_experimental")
        );
        assert!(!removed.contains("proton_9"));
        assert!(removed.contains("\"ShaderCacheManager\""));
        // Nothing to remove: the text comes back as it was.
        assert_eq!(set_compat_tool(CONFIG, "70", None).unwrap(), CONFIG);
    }

    #[test]
    fn creates_the_mapping_when_steam_has_none() {
        let config = "\"InstallConfigStore\"\n{\n\t\"Software\"\n\t{\n\t\t\"valve\"\n\t\t{\n\t\t\t\"Steam\"\n\t\t\t{\n\t\t\t\t\"SurveyDate\"\t\t\"2024-01-01\"\n\t\t\t}\n\t\t}\n\t}\n}\n";
        let out = set_compat_tool(config, "620", Some("proton_9")).unwrap();
        assert!(out.starts_with(&config[..config.find("\t\t\t}").unwrap()]));
        assert!(out.contains(
            "\t\t\t\t\"CompatToolMapping\"\n\t\t\t\t{\n\t\t\t\t\t\"620\"\n\t\t\t\t\t{\n\t\t\t\t\t\t\"name\"\t\t\"proton_9\"\n"
        ));
        assert_eq!(compat_tool(&out, "620").as_deref(), Some("proton_9"));

        // Without even a Steam section.
        let bare = "\"InstallConfigStore\"\n{\n\t\"Music\"\t\t\"1\"\n}\n";
        let out = set_compat_tool(bare, "620", Some("proton_9")).unwrap();
        assert_eq!(compat_tool(&out, "620").as_deref(), Some("proton_9"));
        assert!(
            out.starts_with("\"InstallConfigStore\"\n{\n\t\"Music\"\t\t\"1\"\n\t\"Software\"\n")
        );
        assert_eq!(set_compat_tool(bare, "620", None).unwrap(), bare);
    }

    #[test]
    fn keeps_windows_line_endings_and_one_line_sections() {
        let crlf = CONFIG.replace('\n', "\r\n");
        let out = set_compat_tool(&crlf, "70", Some("proton_9")).unwrap();
        assert!(!out.replace("\r\n", "").contains('\n'));
        assert_eq!(set_compat_tool(&out, "70", None).unwrap(), crlf);

        let inline = "\"InstallConfigStore\" { \"Software\" { \"Valve\" { \"Steam\" { \"CompatToolMapping\" { } } } } }";
        let out = set_compat_tool(inline, "620", Some("proton_9")).unwrap();
        assert_eq!(compat_tool(&out, "620").as_deref(), Some("proton_9"));
    }

    #[test]
    fn completes_an_entry_without_a_name() {
        let config = CONFIG.replace("\t\t\t\t\t\t\"name\"\t\t\"proton_9\"\n", "");
        let out = set_compat_tool(&config, "620", Some("proton_8")).unwrap();
        assert_eq!(compat_tool(&out, "620").as_deref(), Some("proton_8"));
        // Its own config and priority stay; only the name is added.
        assert_eq!(out.matches("\"priority\"").count(), 2);
    }

    #[test]
    fn escapes_names_as_steam_does() {
        let out = set_compat_tool(CONFIG, "70", Some("My \"Proton\" \\ build")).unwrap();
        assert!(out.contains("\"My \\\"Proton\\\" \\\\ build\""));
        assert_eq!(
            compat_tool(&out, "70").as_deref(),
            Some("My \"Proton\" \\ build")
        );
    }

    #[test]
    fn refuses_damaged_files_and_bad_input() {
        let truncated = &CONFIG[..CONFIG.len() / 2];
        assert!(set_compat_tool(truncated, "620", Some("proton_9")).is_err());
        assert!(set_compat_tool("\"Other\" { }", "620", Some("proton_9")).is_err());
        assert!(set_compat_tool("", "620", Some("proton_9")).is_err());
        assert!(set_compat_tool(CONFIG, "6a0", Some("proton_9")).is_err());
        assert!(set_compat_tool(CONFIG, "620", Some(" ")).is_err());
        assert!(set_compat_tool(CONFIG, "620", Some("a\nb")).is_err());
        let stray = CONFIG.replace("\"Music\"\n\t{", "\"Music\"\n\t{ \"lonely\"");
        assert!(set_compat_tool(&stray, "620", Some("proton_9")).is_err());
    }
}
