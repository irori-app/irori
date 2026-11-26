//! Async load state and small text helpers shared by the views.

use gpui_kit::SharedString;
use irori_ui::i18n::{t, t_args};

/// State of data fetched from the backend.
#[derive(Debug, Clone, Default)]
pub(crate) enum Load<T> {
    /// Not requested (e.g. not applicable offline).
    #[default]
    Idle,
    Loading,
    Ready(T),
    Failed(SharedString),
}

impl<T> Load<T> {
    pub(crate) fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(v) => Some(v),
            _ => None,
        }
    }

    pub(crate) fn is_loading(&self) -> bool {
        matches!(self, Load::Loading)
    }

    pub(crate) fn from_result(r: anyhow::Result<T>) -> Self {
        match r {
            Ok(v) => Load::Ready(v),
            Err(e) => Load::Failed(format!("{e:#}").into()),
        }
    }
}

/// Display name of a store language code in the interface language
/// ("de-DE" → "German (de-DE)").
pub(crate) fn language_name(code: &str) -> String {
    let base = code
        .split(['-', '_'])
        .next()
        .unwrap_or(code)
        .to_ascii_lowercase();
    // English names and aliases some stores use instead of ISO codes.
    let english = match base.as_str() {
        "english" => "en",
        "german" => "de",
        "french" => "fr",
        "spanish" => "es",
        "italian" => "it",
        "portuguese" => "pt",
        "polish" => "pl",
        "russian" => "ru",
        "jp" => "ja",
        "nb" => "no",
        other => other,
    };
    let key = match english {
        "en" => "game.language_name.en",
        "de" => "game.language_name.de",
        "fr" => "game.language_name.fr",
        "es" => "game.language_name.es",
        "it" => "game.language_name.it",
        "pt" => "game.language_name.pt",
        "pl" => "game.language_name.pl",
        "ru" => "game.language_name.ru",
        "uk" => "game.language_name.uk",
        "cs" => "game.language_name.cs",
        "hu" => "game.language_name.hu",
        "nl" => "game.language_name.nl",
        "sv" => "game.language_name.sv",
        "da" => "game.language_name.da",
        "fi" => "game.language_name.fi",
        "no" => "game.language_name.no",
        "tr" => "game.language_name.tr",
        "ja" => "game.language_name.ja",
        "ko" => "game.language_name.ko",
        "zh" => "game.language_name.zh",
        "ar" => "game.language_name.ar",
        "he" => "game.language_name.he",
        "th" => "game.language_name.th",
        "el" => "game.language_name.el",
        "ro" => "game.language_name.ro",
        _ => return code.to_string(),
    };
    let name = t(key);
    if base.len() > 3 {
        // A full English name ("english") stands for itself.
        name.to_string()
    } else {
        t_args(
            "game.language_name.with_code",
            &[("name", &name), ("code", code)],
        )
        .to_string()
    }
}

/// The language to preselect: `current` when available, else one matching the
/// UI language, else English, else the first.
pub(crate) fn preferred_language(
    languages: &[String],
    current: Option<&str>,
    ui_language: &str,
) -> Option<String> {
    if let Some(c) = current
        && let Some(l) = languages.iter().find(|l| l.eq_ignore_ascii_case(c))
    {
        return Some(l.clone());
    }
    let starts = |prefix: &str| {
        let prefix = prefix.to_ascii_lowercase();
        languages
            .iter()
            .find(|l| l.to_ascii_lowercase().starts_with(&prefix))
            .cloned()
    };
    (!ui_language.is_empty())
        .then(|| starts(ui_language))
        .flatten()
        .or_else(|| starts("en"))
        .or_else(|| languages.first().cloned())
}

/// Date part of an RFC 3339 timestamp, relative when recent ("Today"), else
/// in the interface language ("Jul 14", "Jan 2, 2020" in another year).
pub(crate) fn format_day(rfc3339: &str, now_unix: i64) -> String {
    let Some(ts) = irori_core::util::parse_rfc3339(rfc3339) else {
        return rfc3339.chars().take(10).collect();
    };
    let days = (now_unix - ts).div_euclid(86_400);
    match days {
        d if d < 0 => rfc3339.chars().take(10).collect(),
        0 => t("game.date.today").to_string(),
        1 => t("game.date.yesterday").to_string(),
        d if d < 7 => t_args("game.date.days_ago", &[("count", &d.to_string())]).to_string(),
        _ => match (
            parse_date(rfc3339),
            chrono::DateTime::from_timestamp(now_unix, 0),
        ) {
            (Some(date), Some(now)) => {
                use chrono::Datelike as _;
                calendar_date(date, date.year() != now.year())
            }
            _ => rfc3339.chars().take(10).collect(),
        },
    }
}

pub(crate) use irori_ui::dates::{calendar_date, parse_date};

/// Whether store text is HTML rather than Markdown.
///
/// Recognizes common HTML tags (including uppercase tags and tags with
/// attributes such as Steam's `<p class="bb_paragraph">` or
/// `<span class="bb_img_ctn">`) while ignoring valid Markdown backtick code
/// spans/fences and `<http://...>` / `<https://...>` autolinks. Markdown
/// documents that only mix in `<br>` or `<hr>` alongside Markdown block syntax
/// stay classified as Markdown so headings, lists, and code spans are kept.
pub(crate) fn looks_like_html(text: &str) -> bool {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut saw_break_only = false;

    while i < len {
        if bytes[i] == b'`' {
            if let Some(after) = skip_code_span_or_fence(bytes, i) {
                i = after;
                continue;
            }
            while i < len && bytes[i] == b'`' {
                i += 1;
            }
            continue;
        }

        if bytes[i] == b'<' {
            if text[i..].starts_with("<!--") {
                return true;
            }
            let mut j = i + 1;
            if j < len && bytes[j] == b'/' {
                j += 1;
            }
            let name_start = j;
            while j < len && bytes[j].is_ascii_alphanumeric() {
                j += 1;
            }
            if j > name_start
                && bytes
                    .get(j)
                    .is_some_and(|b| matches!(b, b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\r'))
            {
                let name = &text[name_start..j];
                if name.eq_ignore_ascii_case("br") || name.eq_ignore_ascii_case("hr") {
                    saw_break_only = true;
                } else if is_html_tag(name) {
                    return true;
                }
            }
        }
        i += 1;
    }

    saw_break_only && !has_markdown_block_syntax(text)
}

/// If `bytes[start]` starts a valid Markdown fenced code block or closed inline
/// code span, return the byte index immediately after its closing delimiter.
/// Returns `None` for unclosed inline backticks so stray backticks in prose
/// never hide subsequent HTML tags.
pub(crate) fn skip_code_span_or_fence(bytes: &[u8], start: usize) -> Option<usize> {
    let len = bytes.len();
    if bytes.get(start) != Some(&b'`') {
        return None;
    }
    let mut ticks = 0;
    while start + ticks < len && bytes[start + ticks] == b'`' {
        ticks += 1;
    }

    // Fenced code block: >= 3 backticks at line start (up to 3 leading spaces)
    // with no backtick in the info string.
    if ticks >= 3 && at_indented_line_start(bytes, start) {
        let mut line_end = start + ticks;
        let mut info_has_tick = false;
        while line_end < len && bytes[line_end] != b'\n' {
            if bytes[line_end] == b'`' {
                info_has_tick = true;
            }
            line_end += 1;
        }
        if !info_has_tick {
            let mut pos = if line_end < len { line_end + 1 } else { len };
            while pos < len {
                let mut p = pos;
                let mut spaces = 0;
                while p < len && bytes[p] == b' ' && spaces < 3 {
                    p += 1;
                    spaces += 1;
                }
                let mut close = 0;
                while p + close < len && bytes[p + close] == b'`' {
                    close += 1;
                }
                let mut rest = p + close;
                while rest < len && matches!(bytes[rest], b' ' | b'\t' | b'\r') {
                    rest += 1;
                }
                if close >= ticks && (rest == len || bytes[rest] == b'\n') {
                    return Some(if rest < len { rest + 1 } else { len });
                }
                while pos < len && bytes[pos] != b'\n' {
                    pos += 1;
                }
                if pos < len {
                    pos += 1;
                }
            }
            return Some(len);
        }
    }

    // Inline code span: must close with an exact run of `ticks` backticks
    // before a blank line (paragraph break) or EOF.
    let mut j = start + ticks;
    while j < len {
        if bytes[j] == b'\n' {
            let mut k = j + 1;
            while k < len && matches!(bytes[k], b' ' | b'\t' | b'\r') {
                k += 1;
            }
            if k < len && bytes[k] == b'\n' {
                return None;
            }
            j += 1;
            continue;
        }
        if bytes[j] == b'`' {
            let mut close = 0;
            while j + close < len && bytes[j + close] == b'`' {
                close += 1;
            }
            if close == ticks {
                return Some(j + close);
            }
            j += close;
            continue;
        }
        j += 1;
    }
    None
}

fn at_indented_line_start(bytes: &[u8], pos: usize) -> bool {
    let mut line_start = pos;
    while line_start > 0 && bytes[line_start - 1] != b'\n' {
        line_start -= 1;
    }
    let prefix = &bytes[line_start..pos];
    prefix.len() <= 3 && prefix.iter().all(|&b| b == b' ')
}

fn has_markdown_block_syntax(text: &str) -> bool {
    for line in text.lines() {
        let trimmed = line.trim_start_matches(' ');
        if line.len() - trimmed.len() > 3 {
            continue;
        }
        let b = trimmed.as_bytes();
        // ATX heading: # .. ###### followed by space or tab.
        let hashes = b.iter().take_while(|&&c| c == b'#').count();
        if (1..=6).contains(&hashes) && b.get(hashes).is_some_and(|&c| c == b' ' || c == b'\t') {
            return true;
        }
        // Fenced code block.
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            return true;
        }
        // Unordered list item.
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
            return true;
        }
        // Ordered list item: digits followed by ". ".
        let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 && trimmed[digits..].starts_with(". ") {
            return true;
        }
    }
    false
}

fn is_html_tag(name: &str) -> bool {
    const TAGS: &[&str] = &[
        "a",
        "abbr",
        "address",
        "article",
        "aside",
        "audio",
        "b",
        "big",
        "blockquote",
        "body",
        "br",
        "button",
        "caption",
        "center",
        "cite",
        "code",
        "col",
        "colgroup",
        "dd",
        "del",
        "details",
        "dialog",
        "div",
        "dl",
        "dt",
        "em",
        "fieldset",
        "figcaption",
        "figure",
        "font",
        "footer",
        "form",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "head",
        "header",
        "hr",
        "html",
        "i",
        "iframe",
        "img",
        "input",
        "ins",
        "kbd",
        "label",
        "legend",
        "li",
        "main",
        "mark",
        "nav",
        "noscript",
        "ol",
        "p",
        "picture",
        "pre",
        "q",
        "s",
        "samp",
        "script",
        "section",
        "select",
        "small",
        "source",
        "span",
        "strike",
        "strong",
        "style",
        "sub",
        "summary",
        "sup",
        "svg",
        "table",
        "tbody",
        "td",
        "textarea",
        "tfoot",
        "th",
        "thead",
        "tr",
        "u",
        "ul",
        "var",
        "video",
        "wbr",
    ];
    TAGS.iter().any(|t| name.eq_ignore_ascii_case(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_names() {
        crate::i18n::register();
        assert_eq!(language_name("de-DE"), "German (de-DE)");
        assert_eq!(language_name("english"), "English");
        assert_eq!(language_name("xx"), "xx");
    }

    #[test]
    fn preferred() {
        let langs: Vec<String> = ["de-DE", "en-US", "fr-FR"].map(String::from).to_vec();
        assert_eq!(
            preferred_language(&langs, Some("FR-fr"), "de").as_deref(),
            Some("fr-FR")
        );
        assert_eq!(
            preferred_language(&langs, None, "de").as_deref(),
            Some("de-DE")
        );
        assert_eq!(
            preferred_language(&langs, None, "ja").as_deref(),
            Some("en-US")
        );
        assert_eq!(preferred_language(&[], None, "ja"), None);
    }

    #[test]
    fn days() {
        crate::i18n::register();
        // Dates come from `irori_ui::dates`, in its catalogs.
        irori_ui::i18n::register(irori_ui::i18n::CATALOGS);
        let now = 1_700_000_000;
        let iso = |t: i64| {
            time::OffsetDateTime::from_unix_timestamp(t)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap()
        };
        assert_eq!(format_day(&iso(now - 60), now), "Today");
        assert_eq!(format_day(&iso(now - 86_400 - 5), now), "Yesterday");
        assert_eq!(format_day(&iso(now - 3 * 86_400), now), "3 days ago");
        assert_eq!(format_day("2020-01-02T00:00:00Z", now), "Jan 2, 2020");
    }

    #[test]
    fn html_detection() {
        assert!(looks_like_html("<p>Hi</p>"));
        assert!(looks_like_html("Text<br/>more"));
        assert!(looks_like_html("Intro <p class=\"bb_paragraph\">body</p>"));
        assert!(looks_like_html(
            "Banner <span class=\"bb_img_ctn\"><img src=\"https://example.com/a.png\"/></span>"
        ));
        assert!(looks_like_html("Uppercase <H2>Title</H2>"));
        assert!(!looks_like_html("# Title\n\n- item"));
        assert!(!looks_like_html("Visit <https://example.org> for details."));
        assert!(!looks_like_html(
            "Use `<code>` or `<img src=\"x\">` in docs."
        ));
        assert!(!looks_like_html("```html\n<p>example</p>\n```"));
        assert!(!looks_like_html("## v1.2\n\n- Fixed crash<br>on startup"));
        assert!(looks_like_html("Don`t miss <p>this update</p>"));
        assert!(looks_like_html(
            "Don`t miss\n\n<p>this update</p>\n\nAlso `code`"
        ));
        assert!(!looks_like_html("1 < 2 and 3 > 1"));
    }
}
