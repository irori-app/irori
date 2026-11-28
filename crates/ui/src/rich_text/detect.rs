//! HTML vs Markdown detection helpers shared across store descriptions,
//! changelogs, and news rendering.

/// Whether store text is HTML rather than Markdown.
///
/// Recognizes common HTML tags (including uppercase tags and tags with
/// attributes such as Steam's `<p class="bb_paragraph">` or
/// `<span class="bb_img_ctn">`) while ignoring valid Markdown backtick code
/// spans/fences and `<http://...>` / `<https://...>` autolinks. Markdown
/// documents that only mix in `<br>` or `<hr>` alongside Markdown block syntax
/// stay classified as Markdown so headings, lists, and code spans are kept.
pub fn looks_like_html(text: &str) -> bool {
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
pub fn skip_code_span_or_fence(bytes: &[u8], start: usize) -> Option<usize> {
    let len = bytes.len();
    if bytes.get(start) != Some(&b'`') {
        return None;
    }
    let mut ticks = 0;
    while start + ticks < len && bytes[start + ticks] == b'`' {
        ticks += 1;
    }

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
        let hashes = b.iter().take_while(|&&c| c == b'#').count();
        if (1..=6).contains(&hashes) && b.get(hashes).is_some_and(|&c| c == b' ' || c == b'\t') {
            return true;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            return true;
        }
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("+ ") {
            return true;
        }
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
