//! Minimal parser for Valve's text KeyValues format (`libraryfolders.vdf`,
//! `config.vdf`) and Steam library discovery on top of it, plus
//! `compatibilitytool.vdf` rewriting for `-latest` Proton builds.

use std::path::{Path, PathBuf};

/// A parsed KeyValues node. Objects keep their key order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Vdf {
    Str(String),
    Obj(Vec<(String, Vdf)>),
}

impl Vdf {
    /// First child with `key` (case-insensitive, as Steam treats keys).
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Vdf::Obj(items) => items
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v),
            Vdf::Str(_) => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Vdf::Str(s) => Some(s),
            Vdf::Obj(_) => None,
        }
    }

    /// Children of an object, in file order.
    pub fn entries(&self) -> &[(String, Vdf)] {
        match self {
            Vdf::Obj(items) => items,
            Vdf::Str(_) => &[],
        }
    }
}

#[derive(Debug, PartialEq)]
enum Token {
    Str(String),
    Open,
    Close,
}

fn tokenize(input: &str) -> Vec<Token> {
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let mut out = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '/' => {
                chars.next();
                if chars.peek() == Some(&'/') {
                    for c in chars.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                }
            }
            '{' => {
                chars.next();
                out.push(Token::Open);
            }
            '}' => {
                chars.next();
                out.push(Token::Close);
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(other) => s.push(other),
                            None => break,
                        },
                        c => s.push(c),
                    }
                }
                out.push(Token::Str(s));
            }
            '[' => {
                // Conditional suffixes like `[$WIN32]` are ignored.
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                }
            }
            _ => {
                let mut s = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || c == '{' || c == '}' || c == '"' {
                        break;
                    }
                    s.push(c);
                    chars.next();
                }
                out.push(Token::Str(s));
            }
        }
    }
    out
}

/// Parse a text VDF document into its root object. Malformed trailing input
/// is ignored rather than rejected, as Steam itself tolerates it.
pub fn parse(input: &str) -> Vdf {
    let tokens = tokenize(input);
    let mut pos = 0;
    Vdf::Obj(parse_object(&tokens, &mut pos))
}

fn parse_object(tokens: &[Token], pos: &mut usize) -> Vec<(String, Vdf)> {
    let mut items = Vec::new();
    while *pos < tokens.len() {
        match &tokens[*pos] {
            Token::Close => {
                *pos += 1;
                break;
            }
            Token::Open => {
                *pos += 1;
            }
            Token::Str(key) => {
                *pos += 1;
                match tokens.get(*pos) {
                    Some(Token::Str(value)) => {
                        *pos += 1;
                        items.push((key.clone(), Vdf::Str(value.clone())));
                    }
                    Some(Token::Open) => {
                        *pos += 1;
                        let child = parse_object(tokens, pos);
                        items.push((key.clone(), Vdf::Obj(child)));
                    }
                    _ => break,
                }
            }
        }
    }
    items
}

/// Library folder paths listed in a `libraryfolders.vdf` document, in file
/// order. Supports both the current format (`"0" { "path" "…" }`) and the old
/// one (`"1" "/path"`).
pub fn library_folders(doc: &Vdf) -> Vec<PathBuf> {
    let Some(root) = doc
        .get("libraryfolders")
        .or_else(|| doc.get("LibraryFolders"))
    else {
        return Vec::new();
    };
    root.entries()
        .iter()
        .filter(|(k, _)| k.chars().all(|c| c.is_ascii_digit()))
        .filter_map(|(_, v)| match v {
            Vdf::Str(path) => Some(PathBuf::from(path)),
            Vdf::Obj(_) => v.get("path").and_then(Vdf::as_str).map(PathBuf::from),
        })
        .collect()
}

/// Steam library roots: `/usr/share/steam` (system Steam packages), the Steam
/// installation itself, and every folder from its `libraryfolders.vdf`.
/// Only existing directories are returned, de-duplicated by canonical path.
pub fn steam_libraries(steam_path: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![PathBuf::from("/usr/share/steam"), steam_path.to_path_buf()];
    let vdf = steam_path.join("steamapps").join("libraryfolders.vdf");
    match std::fs::read_to_string(&vdf) {
        Ok(text) => candidates.extend(library_folders(&parse(&text))),
        Err(_) => tracing::debug!("no Steam library list at {}", vdf.display()),
    }
    let mut seen = std::collections::HashSet::new();
    candidates
        .into_iter()
        .filter(|p| p.is_dir())
        .filter(|p| seen.insert(crate::locations::canonical(p)))
        .collect()
}

fn escape_vdf(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out
}

fn write_vdf_node(out: &mut String, key: &str, value: &Vdf, indent: usize) {
    use std::fmt::Write as _;
    let pad = "\t".repeat(indent);
    match value {
        Vdf::Str(s) => {
            let _ = writeln!(out, "{pad}\"{}\"\t\t\"{}\"", escape_vdf(key), escape_vdf(s));
        }
        Vdf::Obj(children) => {
            let _ = writeln!(out, "{pad}\"{}\"", escape_vdf(key));
            let _ = writeln!(out, "{pad}{{");
            for (child_key, child_val) in children {
                write_vdf_node(out, child_key, child_val, indent + 1);
            }
            let _ = writeln!(out, "{pad}}}");
        }
    }
}

/// Rewrite a single-tool `compatibilitytool.vdf` document so both its internal
/// `compat_tools` key and `display_name` match `stable_name`. Returns `None`
/// when `stable_name` is invalid or the manifest shape is unexpected, leaving
/// multi-tool or malformed manifests untouched.
pub fn rewrite_compat_tool_vdf_text(text: &str, stable_name: &str) -> Option<String> {
    if stable_name.is_empty()
        || stable_name.trim() != stable_name
        || stable_name.chars().any(char::is_control)
    {
        return None;
    }

    let doc = parse(text);
    let mut compat_roots = doc
        .entries()
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("compatibilitytools"));
    let (_, root_val) = compat_roots.next()?;
    if compat_roots.next().is_some() {
        return None;
    }
    let Vdf::Obj(inner) = root_val else {
        return None;
    };

    let mut tools_blocks = inner
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("compat_tools"));
    let (_, tools_val) = tools_blocks.next()?;
    if tools_blocks.next().is_some() {
        return None;
    }
    let Vdf::Obj(tool_entries) = tools_val else {
        return None;
    };

    let [(name, value)] = tool_entries.as_slice() else {
        return None;
    };
    if name.trim().is_empty() {
        return None;
    }
    let Vdf::Obj(fields) = value else {
        return None;
    };

    let mut updated_fields = Vec::with_capacity(fields.len() + 1);
    let mut saw_display_name = false;
    for (k, v) in fields {
        if k.eq_ignore_ascii_case("display_name") {
            if !saw_display_name {
                updated_fields.push((k.clone(), Vdf::Str(stable_name.to_string())));
                saw_display_name = true;
            }
        } else {
            updated_fields.push((k.clone(), v.clone()));
        }
    }
    if !saw_display_name {
        updated_fields.push((
            "display_name".to_string(),
            Vdf::Str(stable_name.to_string()),
        ));
    }

    let mut out = String::new();
    for (k, v) in doc.entries() {
        if k.eq_ignore_ascii_case("compatibilitytools")
            && let Vdf::Obj(inner_entries) = v
        {
            let new_inner: Vec<(String, Vdf)> = inner_entries
                .iter()
                .map(|(ik, iv)| {
                    if ik.eq_ignore_ascii_case("compat_tools") {
                        (
                            ik.clone(),
                            Vdf::Obj(vec![(
                                stable_name.to_string(),
                                Vdf::Obj(updated_fields.clone()),
                            )]),
                        )
                    } else {
                        (ik.clone(), iv.clone())
                    }
                })
                .collect();
            write_vdf_node(&mut out, k, &Vdf::Obj(new_inner), 0);
        } else {
            write_vdf_node(&mut out, k, v, 0);
        }
    }
    Some(out)
}

/// Atomically rewrite `compatibilitytool.vdf` inside `dir` so a `-latest`
/// Proton build keeps `stable_name` across updates and rollbacks.
///
/// Missing files are ignored; unexpected manifests or I/O errors log a warning
/// without failing the install. Intended to run on a blocking thread.
pub fn rewrite_compat_tool_file(dir: &Path, stable_name: &str) {
    let path = dir.join("compatibilitytool.vdf");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::warn!("could not read {}: {e}", path.display());
            return;
        }
    };
    let Some(rewritten) = rewrite_compat_tool_vdf_text(&text, stable_name) else {
        tracing::warn!(
            "leaving {} unchanged: unexpected compatibilitytool.vdf structure",
            path.display()
        );
        return;
    };
    if let Err(e) = irori_core::json_store::write_bytes(&path, rewritten.as_bytes()) {
        tracing::warn!("could not write {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIBRARY: &str = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"/home/u/.local/share/Steam"
		"label"		""
		"apps"
		{
			"228980"		"128606066"
		}
	}
	// second drive
	"1"
	{
		"path"		"/mnt/games/Steam\\Library"
	}
}
"#;

    #[test]
    fn parses_library_folders() {
        let doc = parse(LIBRARY);
        assert_eq!(
            library_folders(&doc),
            vec![
                PathBuf::from("/home/u/.local/share/Steam"),
                PathBuf::from("/mnt/games/Steam\\Library")
            ]
        );
        let apps = doc
            .get("libraryfolders")
            .and_then(|l| l.get("0"))
            .and_then(|l| l.get("apps"))
            .unwrap();
        assert_eq!(apps.get("228980").and_then(Vdf::as_str), Some("128606066"));
    }

    #[test]
    fn parses_old_format_and_unquoted_tokens() {
        let doc = parse(
            "LibraryFolders { \"TimeNextStatsReport\" \"1\" \"1\" \"/data/steam\" key value }",
        );
        assert_eq!(library_folders(&doc), vec![PathBuf::from("/data/steam")]);
        assert_eq!(
            doc.get("libraryfolders")
                .and_then(|l| l.get("key"))
                .and_then(Vdf::as_str),
            Some("value")
        );
    }

    #[test]
    fn tolerates_garbage() {
        assert_eq!(library_folders(&parse("")), Vec::<PathBuf>::new());
        assert_eq!(
            library_folders(&parse("\"libraryfolders\" { \"0\" {")),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn libraries_are_existing_and_unique() {
        let dir = tempfile::tempdir().unwrap();
        let steam = dir.path().join("steam");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        let other = dir.path().join("lib2");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            steam.join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} \"1\" {{ \"path\" \"{}\" }} \"2\" {{ \"path\" \"/nonexistent/x\" }} }}",
                steam.to_string_lossy().replace('\\', "\\\\"),
                other.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .unwrap();
        let libs = steam_libraries(&steam);
        let ours: Vec<_> = libs.iter().filter(|p| p.starts_with(dir.path())).collect();
        assert_eq!(ours, vec![&steam, &other]);
    }

    #[test]
    fn latest_proton_vdf_gets_stable_name_and_display_name() {
        let manifest = r#"
"compatibilitytools"
{
  "compat_tools"
  {
    "GE-Proton10-20" // Internal name of this tool
    {
      "install_path" "."
      "display_name" "GE-Proton10-20"
      "from_oslist"  "windows"
      "to_oslist"    "linux"
    }
  }
  "extra" "kept"
}
"#;
        let dir = tempfile::tempdir().unwrap();
        let vdf_path = dir.path().join("compatibilitytool.vdf");
        std::fs::write(&vdf_path, manifest).unwrap();

        rewrite_compat_tool_file(dir.path(), "GE-Proton-latest");

        let written = std::fs::read_to_string(&vdf_path).unwrap();
        let doc = parse(&written);
        let tools = doc
            .get("compatibilitytools")
            .and_then(|c| c.get("compat_tools"))
            .unwrap();
        assert_eq!(tools.entries().len(), 1);
        assert_eq!(tools.entries()[0].0, "GE-Proton-latest");

        let entry = tools.get("GE-Proton-latest").unwrap();
        assert_eq!(
            entry.get("display_name").and_then(Vdf::as_str),
            Some("GE-Proton-latest")
        );
        assert_eq!(entry.get("install_path").and_then(Vdf::as_str), Some("."));
        assert_eq!(
            entry.get("from_oslist").and_then(Vdf::as_str),
            Some("windows")
        );
        assert_eq!(entry.get("to_oslist").and_then(Vdf::as_str), Some("linux"));
        assert_eq!(
            doc.get("compatibilitytools")
                .and_then(|c| c.get("extra"))
                .and_then(Vdf::as_str),
            Some("kept")
        );
    }

    #[test]
    fn latest_proton_vdf_adds_display_name_when_missing() {
        let manifest = r#"
"compatibilitytools"
{
  "compat_tools"
  {
    "cachyos-10.0-20261101-slr"
    {
      "install_path" "."
      "from_oslist" "windows"
      "to_oslist" "linux"
    }
  }
}
"#;
        let rewritten = rewrite_compat_tool_vdf_text(manifest, "Proton-CachyOS-latest").unwrap();
        let doc = parse(&rewritten);
        let entry = doc
            .get("compatibilitytools")
            .and_then(|c| c.get("compat_tools"))
            .and_then(|t| t.get("Proton-CachyOS-latest"))
            .unwrap();
        assert_eq!(
            entry.get("display_name").and_then(Vdf::as_str),
            Some("Proton-CachyOS-latest")
        );
        assert_eq!(entry.get("install_path").and_then(Vdf::as_str), Some("."));
    }

    #[test]
    fn latest_proton_vdf_handles_utf8_bom_and_escaping_round_trip() {
        let manifest = concat!(
            "\u{feff}",
            "\"compatibilitytools\"\n",
            "{\n",
            "  \"compat_tools\"\n",
            "  {\n",
            "    \"Old \\\"Tool\\\" \\\\ 1\"\n",
            "    {\n",
            "      \"install_path\" \".\"\n",
            "      \"display_name\" \"Old\"\n",
            "      \"note\" \"line1\\nline2\\t\\\\end\\\"!\"\n",
            "    }\n",
            "  }\n",
            "}\n",
        );
        let stable = "My \"Proton\" \\ build";
        let rewritten = rewrite_compat_tool_vdf_text(manifest, stable).unwrap();
        let doc = parse(&rewritten);
        let entry = doc
            .get("compatibilitytools")
            .and_then(|c| c.get("compat_tools"))
            .and_then(|t| t.get(stable))
            .unwrap();
        assert_eq!(
            entry.get("display_name").and_then(Vdf::as_str),
            Some(stable)
        );
        assert_eq!(
            entry.get("note").and_then(Vdf::as_str),
            Some("line1\nline2\t\\end\"!")
        );
    }

    #[test]
    fn latest_proton_vdf_rejects_bad_stable_names() {
        let manifest = r#"
"compatibilitytools"
{
  "compat_tools"
  {
    "GE-Proton10-20"
    {
      "install_path" "."
      "display_name" "GE-Proton10-20"
    }
  }
}
"#;
        for bad in [
            "",
            " ",
            " GE-Proton-latest",
            "GE-Proton-latest ",
            "bad\nname",
            "bad\tname",
        ] {
            assert!(
                rewrite_compat_tool_vdf_text(manifest, bad).is_none(),
                "expected None for {bad:?}"
            );
        }
    }

    #[test]
    fn latest_proton_vdf_leaves_missing_duplicate_or_unexpected_manifests_untouched() {
        let dir = tempfile::tempdir().unwrap();
        rewrite_compat_tool_file(dir.path(), "GE-Proton-latest");
        assert!(!dir.path().join("compatibilitytool.vdf").exists());

        let vdf_path = dir.path().join("compatibilitytool.vdf");
        let multi = r#"
"compatibilitytools"
{
  "compat_tools"
  {
    "ToolA" { "install_path" "." "display_name" "ToolA" }
    "ToolB" { "install_path" "." "display_name" "ToolB" }
  }
}
"#;
        let dup_compat_tools = r#"
"compatibilitytools"
{
  "compat_tools" { "ToolA" { "install_path" "." "display_name" "ToolA" } }
  "compat_tools" "stray"
}
"#;
        let dup_root = r#"
"compatibilitytools" { "compat_tools" { "ToolA" { "install_path" "." } } }
"compatibilitytools" { "compat_tools" { "ToolB" { "install_path" "." } } }
"#;
        let unrelated = "\"other\" { \"foo\" \"bar\" }\n";

        for case in [multi, dup_compat_tools, dup_root, unrelated] {
            std::fs::write(&vdf_path, case).unwrap();
            rewrite_compat_tool_file(dir.path(), "GE-Proton-latest");
            assert_eq!(std::fs::read_to_string(&vdf_path).unwrap(), case);
        }
    }

    #[test]
    fn latest_proton_vdf_stays_identical_across_updates() {
        let make_manifest = |version: &str| {
            format!(
                "\"compatibilitytools\"\n{{\n  \"compat_tools\"\n  {{\n    \"{version}\"\n    {{\n      \"install_path\" \".\"\n      \"display_name\" \"{version}\"\n      \"from_oslist\" \"windows\"\n      \"to_oslist\" \"linux\"\n    }}\n  }}\n}}\n"
            )
        };
        let first =
            rewrite_compat_tool_vdf_text(&make_manifest("GE-Proton10-20"), "GE-Proton-latest")
                .unwrap();
        let second =
            rewrite_compat_tool_vdf_text(&make_manifest("GE-Proton10-21"), "GE-Proton-latest")
                .unwrap();
        assert_eq!(first, second);
    }

    #[cfg(unix)]
    #[test]
    fn latest_proton_vdf_replaces_read_only_file() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let vdf_path = dir.path().join("compatibilitytool.vdf");
        std::fs::write(
            &vdf_path,
            "\"compatibilitytools\" { \"compat_tools\" { \"GE-Proton10-20\" { \"install_path\" \".\" \"display_name\" \"GE-Proton10-20\" } } }\n",
        )
        .unwrap();
        std::fs::set_permissions(&vdf_path, std::fs::Permissions::from_mode(0o444)).unwrap();

        rewrite_compat_tool_file(dir.path(), "GE-Proton-latest");

        let written = std::fs::read_to_string(&vdf_path).unwrap();
        let doc = parse(&written);
        assert_eq!(
            doc.get("compatibilitytools")
                .and_then(|c| c.get("compat_tools"))
                .and_then(|t| t.get("GE-Proton-latest"))
                .and_then(|e| e.get("display_name"))
                .and_then(Vdf::as_str),
            Some("GE-Proton-latest")
        );
    }
}
