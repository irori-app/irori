//! Rich store text rendering (descriptions and changelogs).
//!
//! Store descriptions and changelogs arrive as HTML (Steam, GOG, ZOOM) or
//! Markdown (Epic, itch.io). In `gpui-base 0.8.0`, `TextView::html` leaves
//! block source spans unset (`span: None`), so sibling `InlineFlow` paragraphs
//! collide on `ElementId("p")` and can panic in `inline_flow.rs` during
//! prepaint. Converting HTML to Markdown gives every block and table cell a
//! unique source span in `TextView::markdown`, while scoping each `TextView`'s
//! `ElementId` by `GameId` prevents stale element state across game switches.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::VecDeque;

use gpui_kit::component::text::TextView;
use gpui_kit::{AnyElement, ElementId, IntoElement, SharedString};
use irori_core::GameId;
use once_cell::sync::Lazy;
use regex::Regex;

use crate::load::{looks_like_html, skip_code_span_or_fence};

/// Small UI-thread cache for prepared Markdown strings so re-renders of the
/// same description or changelog avoid repeating regex and HTML conversion.
const PREPARED_CACHE_CAPACITY: usize = 8;

thread_local! {
    static PREPARED_CACHE: RefCell<VecDeque<(String, SharedString)>> =
        RefCell::new(VecDeque::with_capacity(PREPARED_CACHE_CAPACITY));
}

/// Render store description or changelog text scoped to `game_id`.
pub(super) fn rich_text(kind: &str, game_id: &GameId, text: &str) -> AnyElement {
    let id = ElementId::Name(format!("{kind}:{}", game_id.key()).into());
    let markdown = cached_markdown(text);
    TextView::markdown(id, markdown).into_any_element()
}

fn cached_markdown(text: &str) -> SharedString {
    PREPARED_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(pos) = cache
            .iter()
            .position(|(src, _)| src.len() == text.len() && src == text)
            && let Some(entry) = cache.remove(pos)
        {
            let md = entry.1.clone();
            cache.push_front(entry);
            return md;
        }
        let md: SharedString = prepare_markdown(text).into();
        if cache.len() >= PREPARED_CACHE_CAPACITY {
            cache.pop_back();
        }
        cache.push_front((text.to_owned(), md.clone()));
        md
    })
}

/// Sanitize links and convert HTML store text into Markdown so every rendered
/// block carries a unique source span in `TextView::markdown`.
pub(super) fn prepare_markdown(text: &str) -> String {
    let safe = web_links_only(text);
    if looks_like_html(&safe) {
        let converted = html_to_markdown(&safe);
        web_links_only(&converted)
    } else {
        sanitize_markdown_raw_html(&safe).into_owned()
    }
}

/// Neutralize raw HTML tags in Markdown documents (outside code spans, fences,
/// and `<http://...>` / `<https://...>` autolinks) so `gpui-base`'s Markdown
/// parser never falls back to `html::parse` with `span: None`.
fn sanitize_markdown_raw_html(text: &str) -> Cow<'_, str> {
    if !text.contains('<') {
        return Cow::Borrowed(text);
    }
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len);
    let mut i = 0;

    while i < len {
        if bytes[i] == b'`' {
            if let Some(after) = skip_code_span_or_fence(bytes, i) {
                out.push_str(&text[i..after]);
                i = after;
                continue;
            }
            let start = i;
            while i < len && bytes[i] == b'`' {
                i += 1;
            }
            out.push_str(&text[start..i]);
            continue;
        }

        if bytes[i] == b'<' {
            let rest = &text[i..];
            // Strip HTML comments.
            if rest.starts_with("<!--") {
                i = match rest.find("-->") {
                    Some(end) => i + end + 3,
                    None => len,
                };
                continue;
            }
            // Keep <http://...> / <https://...> autolinks untouched.
            if let Some(end) = rest.find('>') {
                let inner = &rest[1..end];
                if !inner.contains([' ', '\t', '\n', '\r', '<'])
                    && irori_ui::web_url(inner).is_some()
                {
                    out.push_str(&rest[..=end]);
                    i += end + 1;
                    continue;
                }
                // Convert inline <br> / <br/> and <hr> / <hr/> in Markdown.
                let trimmed = inner.trim();
                let tag_only = trimmed.strip_suffix('/').unwrap_or(trimmed).trim_end();
                if tag_only.eq_ignore_ascii_case("br") {
                    let trimmed_len = out.trim_end_matches([' ', '\t']).len();
                    out.truncate(trimmed_len);
                    if out.ends_with("  \n") {
                        out.truncate(out.len() - 3);
                        out.push_str("\n\n");
                    } else if !out.ends_with('\n') {
                        out.push_str("  \n");
                    } else if !out.ends_with("\n\n") {
                        out.push('\n');
                    }
                    i += end + 1;
                    continue;
                }
                if tag_only.eq_ignore_ascii_case("hr") {
                    if !out.is_empty() && !out.ends_with('\n') {
                        out.push('\n');
                    }
                    if !out.is_empty() && !out.ends_with("\n\n") {
                        out.push('\n');
                    }
                    out.push_str("---\n\n");
                    i += end + 1;
                    continue;
                }
            }
            // Escape any remaining tag-like '<' that could invoke html::parse.
            if bytes
                .get(i + 1)
                .is_some_and(|b| b.is_ascii_alphabetic() || matches!(b, b'/' | b'!' | b'?'))
                && !has_odd_trailing_backslashes(&out)
            {
                out.push('\\');
            }
            out.push('<');
            i += 1;
            continue;
        }

        let ch = text[i..].chars().next().expect("non-empty slice");
        out.push(ch);
        i += ch.len_utf8();
    }

    Cow::Owned(out)
}

/// `text` with every link that is not a web address emptied. Store text comes
/// from publishers, and the text view opens links with the system, which runs
/// whatever handles `file:`, a network share or a custom scheme.
pub(super) fn web_links_only(text: &str) -> String {
    static LINK: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r#"(?ix)
            (?P<before>
                \]\(\s*<?              # [text](address)
              | href\s*=\s*["']?       # <a href="address">
              | (?m:^\s*\[[^\]]+\]:\s*) # [reference]: address
              | <                     # <scheme:address>
            )
            (?P<url>[a-z\\/][^\s"'<>)]*)"#,
        )
        .expect("valid regex")
    });
    LINK.replace_all(text, |c: &regex::Captures| {
        let url = &c["url"];
        let before = &c["before"];
        let bare_tag = before == "<" && !url.contains(':');
        if bare_tag || irori_ui::web_url(url).is_some() {
            c[0].to_string()
        } else {
            format!("{before}#")
        }
    })
    .into_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListKind {
    Unordered,
    Ordered(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ListLevel {
    kind: ListKind,
    item_indent: usize,
}

enum InlineKind {
    Strong,
    Emphasis,
    Strike,
    Link(Option<String>),
    Heading(u8),
}

struct InlineFrame {
    kind: InlineKind,
    buf: String,
}

#[derive(Default)]
struct TableBuilder {
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    in_cell: bool,
    cell_buf: String,
}

struct Converter {
    out: String,
    inline_stack: Vec<InlineFrame>,
    list_stack: Vec<ListLevel>,
    in_list_item: bool,
    at_list_item_start: bool,
    blockquote_depth: usize,
    pre_depth: usize,
    pre_buf: String,
    code_depth: usize,
    code_buf: String,
    table: Option<TableBuilder>,
    nested_table_depth: usize,
}

impl Converter {
    fn new(capacity: usize) -> Self {
        Self {
            out: String::with_capacity(capacity),
            inline_stack: Vec::new(),
            list_stack: Vec::new(),
            in_list_item: false,
            at_list_item_start: false,
            blockquote_depth: 0,
            pre_depth: 0,
            pre_buf: String::new(),
            code_depth: 0,
            code_buf: String::new(),
            table: None,
            nested_table_depth: 0,
        }
    }

    /// The buffer currently receiving inline output.
    fn target_mut(&mut self) -> &mut String {
        if let Some(frame) = self.inline_stack.last_mut() {
            &mut frame.buf
        } else if let Some(table) = self.table.as_mut()
            && table.in_cell
        {
            &mut table.cell_buf
        } else {
            &mut self.out
        }
    }

    fn in_table_cell(&self) -> bool {
        self.table.as_ref().is_some_and(|t| t.in_cell)
    }

    fn push_blockquote_prefix(&mut self) {
        if self.blockquote_depth > 0 && self.inline_stack.is_empty() && !self.in_table_cell() {
            for _ in 0..self.blockquote_depth {
                self.out.push_str("> ");
            }
        }
    }

    fn push_line_prefix(&mut self) {
        if !self.inline_stack.is_empty() || self.in_table_cell() {
            return;
        }
        self.push_blockquote_prefix();
        if self.in_list_item && !self.at_list_item_start {
            let indent: usize = self.list_stack.iter().map(|l| l.item_indent).sum();
            for _ in 0..indent {
                self.out.push(' ');
            }
        }
    }

    fn ensure_blank_line(&mut self) {
        if self.in_table_cell() {
            return;
        }
        let trimmed_len = self.out.trim_end_matches([' ', '\t']).len();
        self.out.truncate(trimmed_len);
        if self.out.is_empty() {
            return;
        }
        if self.out.ends_with("\n\n") {
            return;
        }
        if self.out.ends_with('\n') {
            self.out.push('\n');
        } else {
            self.out.push_str("\n\n");
        }
        self.at_list_item_start = false;
    }

    fn ensure_newline(&mut self) {
        if self.in_table_cell() {
            return;
        }
        let trimmed_len = self.out.trim_end_matches([' ', '\t']).len();
        self.out.truncate(trimmed_len);
        if self.out.is_empty() || self.out.ends_with('\n') {
            return;
        }
        self.out.push('\n');
    }

    fn push_collapsed_text(&mut self, text: &str) {
        let decoded = decode_entities(text);
        if self.pre_depth > 0 {
            self.pre_buf.push_str(&decoded);
            return;
        }
        if self.code_depth > 0 {
            append_collapsed(&mut self.code_buf, &decoded, false);
            return;
        }
        let at_line_start = self.inline_stack.is_empty()
            && !self.in_table_cell()
            && (self.out.is_empty() || self.out.ends_with('\n'));
        if at_line_start && decoded.trim().is_empty() {
            return;
        }
        if at_line_start {
            self.push_line_prefix();
        }
        let target = self.target_mut();
        append_collapsed(target, &decoded, true);
        if !decoded.trim().is_empty() {
            self.at_list_item_start = false;
        }
    }

    fn flush_code(&mut self) {
        let content = self.code_buf.trim().to_string();
        self.code_buf.clear();
        if content.is_empty() {
            return;
        }
        let formatted = format_inline_code(&content);
        if self.inline_stack.is_empty()
            && !self.in_table_cell()
            && (self.out.is_empty() || self.out.ends_with('\n'))
        {
            self.push_line_prefix();
        }
        self.target_mut().push_str(&formatted);
        self.at_list_item_start = false;
    }

    fn flush_pre(&mut self) {
        let raw = std::mem::take(&mut self.pre_buf);
        let body = raw
            .strip_prefix("\r\n")
            .or_else(|| raw.strip_prefix('\n'))
            .unwrap_or(&raw)
            .replace("\r\n", "\n");
        let body = body.trim_end_matches('\n');
        if body.is_empty() {
            return;
        }
        if self.in_table_cell() {
            let single = body.replace('\n', " ");
            let formatted = format_inline_code(single.trim());
            self.target_mut().push_str(&formatted);
            return;
        }
        self.flush_all_inline();
        self.ensure_blank_line();
        let fence_len = max_backtick_run(body).max(2) + 1;
        let fence: String = "`".repeat(fence_len);
        self.push_blockquote_prefix();
        self.out.push_str(&fence);
        self.out.push('\n');
        for line in body.split('\n') {
            self.push_blockquote_prefix();
            self.out.push_str(line);
            self.out.push('\n');
        }
        self.push_blockquote_prefix();
        self.out.push_str(&fence);
        self.out.push_str("\n\n");
        self.at_list_item_start = false;
    }

    fn close_inline_frame(&mut self, frame: InlineFrame) {
        match frame.kind {
            InlineKind::Heading(level) => {
                let text = collapse_single_line(&frame.buf);
                if text.is_empty() {
                    return;
                }
                if self.in_table_cell() {
                    let target = self.target_mut();
                    if !target.is_empty() && !target.ends_with(' ') {
                        target.push(' ');
                    }
                    target.push_str("**");
                    target.push_str(&text);
                    target.push_str("**");
                    return;
                }
                self.ensure_blank_line();
                self.push_blockquote_prefix();
                for _ in 0..level.clamp(1, 6) {
                    self.out.push('#');
                }
                self.out.push(' ');
                self.out.push_str(&text);
                self.out.push_str("\n\n");
                self.at_list_item_start = false;
            }
            InlineKind::Strong => self.emit_wrapped(&frame.buf, "**"),
            InlineKind::Emphasis => self.emit_wrapped(&frame.buf, "*"),
            InlineKind::Strike => self.emit_wrapped(&frame.buf, "~~"),
            InlineKind::Link(href) => {
                let (lead, core, trail) = split_edge_whitespace(&frame.buf);
                if core.is_empty() {
                    if !lead.is_empty() || !trail.is_empty() {
                        self.append_space_to_target();
                    }
                    return;
                }
                let formatted = match href.and_then(|u| irori_ui::web_url(&u)) {
                    Some(url) => format!("[{core}]({})", format_markdown_url(&url)),
                    None => core.to_string(),
                };
                if self.inline_stack.is_empty()
                    && !self.in_table_cell()
                    && (self.out.is_empty() || self.out.ends_with('\n'))
                {
                    self.push_line_prefix();
                }
                let target = self.target_mut();
                if !lead.is_empty() && !target.is_empty() && !target.ends_with([' ', '\n']) {
                    target.push(' ');
                }
                target.push_str(&formatted);
                if !trail.is_empty() {
                    target.push(' ');
                }
                self.at_list_item_start = false;
            }
        }
    }

    fn emit_wrapped(&mut self, buf: &str, marker: &str) {
        let (lead, core, trail) = split_edge_whitespace(buf);
        if core.is_empty() {
            if !lead.is_empty() || !trail.is_empty() {
                self.append_space_to_target();
            }
            return;
        }
        if self.inline_stack.is_empty()
            && !self.in_table_cell()
            && (self.out.is_empty() || self.out.ends_with('\n'))
        {
            self.push_line_prefix();
        }
        let target = self.target_mut();
        if !lead.is_empty() && !target.is_empty() && !target.ends_with([' ', '\n']) {
            target.push(' ');
        }
        target.push_str(marker);
        target.push_str(core);
        target.push_str(marker);
        if !trail.is_empty() {
            target.push(' ');
        }
        self.at_list_item_start = false;
    }

    fn append_space_to_target(&mut self) {
        let target = self.target_mut();
        if !target.is_empty() && !target.ends_with([' ', '\n']) {
            target.push(' ');
        }
    }

    fn pop_matching_inline(&mut self, predicate: impl Fn(&InlineKind) -> bool) {
        if let Some(pos) = self.inline_stack.iter().rposition(|f| predicate(&f.kind)) {
            while self.inline_stack.len() > pos {
                if let Some(frame) = self.inline_stack.pop() {
                    self.close_inline_frame(frame);
                }
            }
        }
    }

    fn flush_all_inline(&mut self) {
        if self.code_depth > 0 {
            self.code_depth = 0;
            self.flush_code();
        }
        while let Some(frame) = self.inline_stack.pop() {
            self.close_inline_frame(frame);
        }
    }

    fn finish_table_cell(&mut self) {
        self.flush_all_inline();
        if let Some(table) = self.table.as_mut()
            && table.in_cell
        {
            let cell = collapse_single_line(&table.cell_buf).replace('|', "\\|");
            table.cell_buf.clear();
            table.in_cell = false;
            table.current_row.push(cell);
        }
    }

    fn finish_table_row(&mut self) {
        self.finish_table_cell();
        if let Some(table) = self.table.as_mut()
            && !table.current_row.is_empty()
        {
            let row = std::mem::take(&mut table.current_row);
            table.rows.push(row);
        }
    }

    fn finish_table(&mut self) {
        self.finish_table_row();
        let Some(table) = self.table.take() else {
            return;
        };
        let cols = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        self.ensure_blank_line();
        for (row_ix, row) in table.rows.iter().enumerate() {
            self.push_blockquote_prefix();
            self.out.push('|');
            for col in 0..cols {
                self.out.push(' ');
                if let Some(cell) = row.get(col) {
                    self.out.push_str(cell);
                }
                self.out.push_str(" |");
            }
            self.out.push('\n');
            if row_ix == 0 {
                self.push_blockquote_prefix();
                self.out.push('|');
                for _ in 0..cols {
                    self.out.push_str(" --- |");
                }
                self.out.push('\n');
            }
        }
        self.out.push('\n');
        self.at_list_item_start = false;
    }

    fn handle_open(&mut self, name: &str, raw_attrs: &str, self_closing: bool) {
        if self.pre_depth > 0 {
            if name == "pre" && !self_closing {
                self.pre_depth += 1;
            } else if name == "br" {
                self.pre_buf.push('\n');
            }
            return;
        }
        if self.code_depth > 0 {
            if name == "code" && !self_closing {
                self.code_depth += 1;
                return;
            }
            if name == "br" {
                self.code_buf.push(' ');
                return;
            }
            if is_block_tag(name)
                || matches!(
                    name,
                    "h1" | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "ul"
                        | "ol"
                        | "li"
                        | "blockquote"
                        | "table"
                        | "tr"
                        | "td"
                        | "th"
                        | "pre"
                        | "hr"
                )
            {
                self.code_depth = 0;
                self.flush_code();
            } else {
                return;
            }
        }

        match name {
            "pre" if !self_closing => {
                self.flush_all_inline();
                self.pre_depth = 1;
                self.pre_buf.clear();
            }
            "code" if !self_closing => {
                self.code_depth = 1;
                self.code_buf.clear();
            }
            "br" => {
                let in_heading = self
                    .inline_stack
                    .iter()
                    .any(|f| matches!(f.kind, InlineKind::Heading(_)));
                if self.in_table_cell() || in_heading {
                    self.append_space_to_target();
                } else if !self.inline_stack.is_empty() {
                    // CommonMark emphasis and links cannot span a blank line;
                    // emit at most a single hard line break inside an inline frame.
                    let target = self.target_mut();
                    let trimmed = target.trim_end_matches([' ', '\t']).len();
                    target.truncate(trimmed);
                    if !target.is_empty() && !target.ends_with('\n') {
                        target.push_str("  \n");
                    }
                } else {
                    let target = self.target_mut();
                    let trimmed = target.trim_end_matches([' ', '\t']).len();
                    target.truncate(trimmed);
                    if !target.is_empty() && !target.ends_with("\n\n") {
                        if target.ends_with("  \n") {
                            target.truncate(target.len() - 3);
                            target.push_str("\n\n");
                        } else if target.ends_with('\n') {
                            target.push('\n');
                        } else {
                            target.push_str("  \n");
                        }
                    }
                }
            }
            "hr" => {
                if !self.in_table_cell() {
                    self.flush_all_inline();
                    self.ensure_blank_line();
                    self.push_blockquote_prefix();
                    self.out.push_str("---\n\n");
                    self.at_list_item_start = false;
                }
            }
            "img" => {
                let attrs = parse_attrs(raw_attrs);
                let src = attrs
                    .iter()
                    .find(|(k, _)| k == "src")
                    .map(|(_, v)| decode_entities(v).into_owned())
                    .and_then(|u| irori_ui::web_url(&u));
                if let Some(url) = src {
                    let alt = attrs
                        .iter()
                        .find(|(k, _)| k == "alt")
                        .map(|(_, v)| escape_alt(&decode_entities(v)))
                        .unwrap_or_default();
                    if self.inline_stack.is_empty()
                        && !self.in_table_cell()
                        && (self.out.is_empty() || self.out.ends_with('\n'))
                    {
                        self.push_line_prefix();
                    }
                    let target = self.target_mut();
                    target.push_str("![");
                    target.push_str(&alt);
                    target.push_str("](");
                    target.push_str(&format_markdown_url(&url));
                    target.push(')');
                    self.at_list_item_start = false;
                }
            }
            "strong" | "b" if !self_closing => {
                self.inline_stack.push(InlineFrame {
                    kind: InlineKind::Strong,
                    buf: String::new(),
                });
            }
            "em" | "i" if !self_closing => {
                self.inline_stack.push(InlineFrame {
                    kind: InlineKind::Emphasis,
                    buf: String::new(),
                });
            }
            "del" | "s" | "strike" if !self_closing => {
                self.inline_stack.push(InlineFrame {
                    kind: InlineKind::Strike,
                    buf: String::new(),
                });
            }
            "a" if !self_closing => {
                let attrs = parse_attrs(raw_attrs);
                let href = attrs
                    .into_iter()
                    .find(|(k, _)| k == "href")
                    .map(|(_, v)| decode_entities(&v).into_owned());
                self.inline_stack.push(InlineFrame {
                    kind: InlineKind::Link(href),
                    buf: String::new(),
                });
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if !self_closing => {
                self.flush_all_inline();
                let level = name.as_bytes()[1] - b'0';
                self.inline_stack.push(InlineFrame {
                    kind: InlineKind::Heading(level),
                    buf: String::new(),
                });
            }
            "ul" if !self_closing => {
                if !self.in_table_cell() {
                    self.flush_all_inline();
                    if self.list_stack.is_empty() {
                        self.ensure_blank_line();
                    } else {
                        self.ensure_newline();
                    }
                    self.list_stack.push(ListLevel {
                        kind: ListKind::Unordered,
                        item_indent: 2,
                    });
                }
            }
            "ol" if !self_closing => {
                if !self.in_table_cell() {
                    self.flush_all_inline();
                    if self.list_stack.is_empty() {
                        self.ensure_blank_line();
                    } else {
                        self.ensure_newline();
                    }
                    let start = parse_attrs(raw_attrs)
                        .into_iter()
                        .find(|(k, _)| k == "start")
                        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                        .unwrap_or(1);
                    self.list_stack.push(ListLevel {
                        kind: ListKind::Ordered(start),
                        item_indent: 3,
                    });
                }
            }
            "li" => {
                if self.in_table_cell() {
                    self.append_space_to_target();
                } else {
                    self.flush_all_inline();
                    self.ensure_newline();
                    self.push_blockquote_prefix();
                    let parent_indent: usize = self
                        .list_stack
                        .iter()
                        .take(self.list_stack.len().saturating_sub(1))
                        .map(|l| l.item_indent)
                        .sum();
                    for _ in 0..parent_indent {
                        self.out.push(' ');
                    }
                    match self.list_stack.last_mut() {
                        Some(level) => match &mut level.kind {
                            ListKind::Ordered(n) => {
                                let marker = format!("{n}. ");
                                level.item_indent = marker.len();
                                self.out.push_str(&marker);
                                *n += 1;
                            }
                            ListKind::Unordered => {
                                level.item_indent = 2;
                                self.out.push_str("- ");
                            }
                        },
                        None => self.out.push_str("- "),
                    }
                    self.in_list_item = true;
                    self.at_list_item_start = true;
                }
            }
            "blockquote" if !self_closing => {
                if !self.in_table_cell() {
                    self.flush_all_inline();
                    self.ensure_blank_line();
                    self.blockquote_depth += 1;
                }
            }
            "table" if !self_closing => {
                if self.table.is_none() {
                    self.flush_all_inline();
                    self.table = Some(TableBuilder::default());
                } else {
                    self.nested_table_depth += 1;
                }
            }
            "tr" if !self_closing => {
                if self.nested_table_depth > 0 {
                    self.append_space_to_target();
                } else if self.table.is_some() {
                    self.finish_table_row();
                }
            }
            "td" | "th" if !self_closing => {
                if self.nested_table_depth > 0 {
                    self.append_space_to_target();
                } else if self.table.is_some() {
                    self.finish_table_cell();
                    if let Some(table) = self.table.as_mut() {
                        table.in_cell = true;
                    }
                }
            }
            tag if is_block_tag(tag) => {
                if self.in_table_cell() {
                    self.append_space_to_target();
                } else {
                    self.flush_all_inline();
                    if !self.at_list_item_start {
                        self.ensure_blank_line();
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_close(&mut self, name: &str) {
        if self.pre_depth > 0 {
            if name == "pre" {
                self.pre_depth -= 1;
                if self.pre_depth == 0 {
                    self.flush_pre();
                }
            }
            return;
        }
        if self.code_depth > 0 {
            if name == "code" {
                self.code_depth -= 1;
                if self.code_depth == 0 {
                    self.flush_code();
                }
                return;
            }
            if is_block_tag(name)
                || matches!(
                    name,
                    "h1" | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "ul"
                        | "ol"
                        | "li"
                        | "blockquote"
                        | "table"
                        | "tr"
                        | "td"
                        | "th"
                        | "pre"
                )
            {
                self.code_depth = 0;
                self.flush_code();
            } else {
                return;
            }
        }

        match name {
            "strong" | "b" => self.pop_matching_inline(|k| matches!(k, InlineKind::Strong)),
            "em" | "i" => self.pop_matching_inline(|k| matches!(k, InlineKind::Emphasis)),
            "del" | "s" | "strike" => self.pop_matching_inline(|k| matches!(k, InlineKind::Strike)),
            "a" => self.pop_matching_inline(|k| matches!(k, InlineKind::Link(_))),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = name.as_bytes()[1] - b'0';
                self.pop_matching_inline(|k| matches!(k, InlineKind::Heading(l) if *l == level));
            }
            "ul" | "ol" => {
                if !self.in_table_cell() {
                    self.flush_all_inline();
                    self.list_stack.pop();
                    if self.list_stack.is_empty() {
                        self.in_list_item = false;
                        self.ensure_blank_line();
                    } else {
                        self.ensure_newline();
                    }
                }
            }
            "li" => {
                if !self.in_table_cell() {
                    self.flush_all_inline();
                    self.ensure_newline();
                    self.in_list_item = false;
                    self.at_list_item_start = false;
                }
            }
            "blockquote" => {
                if !self.in_table_cell() {
                    self.flush_all_inline();
                    self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
                    self.ensure_blank_line();
                }
            }
            "td" | "th" => {
                if self.nested_table_depth > 0 {
                    self.append_space_to_target();
                } else {
                    self.finish_table_cell();
                }
            }
            "tr" => {
                if self.nested_table_depth > 0 {
                    self.append_space_to_target();
                } else {
                    self.finish_table_row();
                }
            }
            "table" => {
                if self.nested_table_depth > 0 {
                    self.nested_table_depth -= 1;
                    self.append_space_to_target();
                } else {
                    self.finish_table();
                }
            }
            tag if is_block_tag(tag) => {
                if self.in_table_cell() {
                    self.append_space_to_target();
                } else {
                    self.flush_all_inline();
                    if self.list_stack.is_empty() {
                        self.ensure_blank_line();
                    } else {
                        self.ensure_newline();
                    }
                }
            }
            _ => {}
        }
    }

    fn finish(mut self) -> String {
        if self.pre_depth > 0 {
            self.pre_depth = 0;
            self.flush_pre();
        }
        if self.table.is_some() {
            self.finish_table();
        }
        self.flush_all_inline();
        self.out.trim().to_string()
    }
}

/// Convert HTML store markup into Markdown without raw HTML blocks.
pub(super) fn html_to_markdown(html: &str) -> String {
    let mut conv = Converter::new(html.len());
    let bytes = html.as_bytes();
    let len = bytes.len();
    let mut pos = 0;

    while pos < len {
        let Some(rel_lt) = html[pos..].find('<') else {
            conv.push_collapsed_text(&html[pos..]);
            break;
        };
        if rel_lt > 0 {
            conv.push_collapsed_text(&html[pos..pos + rel_lt]);
        }
        let lt = pos + rel_lt;
        let rest = &html[lt..];

        // HTML comment: <!-- ... -->
        if rest.starts_with("<!--") {
            pos = match rest.find("-->") {
                Some(end) => lt + end + 3,
                None => len,
            };
            continue;
        }

        // Doctype or processing instruction: <!...> / <?...>
        if rest.starts_with("<!") || rest.starts_with("<?") {
            pos = match find_tag_end(rest) {
                Some(end) => lt + end + 1,
                None => len,
            };
            continue;
        }

        // Closing tag: </name ...>
        if let Some(after_slash) = rest.strip_prefix("</")
            && let Some(first) = after_slash.as_bytes().first()
            && first.is_ascii_alphabetic()
        {
            let end = match find_tag_end(rest) {
                Some(end) => end,
                None => {
                    pos = len;
                    continue;
                }
            };
            let inner = &rest[2..end];
            let name_end = inner
                .bytes()
                .position(|b| !(b.is_ascii_alphanumeric() || b == b'-' || b == b':'))
                .unwrap_or(inner.len());
            let name = inner[..name_end].to_ascii_lowercase();
            conv.handle_close(&name);
            pos = lt + end + 1;
            continue;
        }

        // Opening tag: <name ...>
        if let Some(first) = rest.as_bytes().get(1)
            && first.is_ascii_alphabetic()
        {
            let Some(end) = find_tag_end(rest) else {
                // Unclosed tag at EOF: treat '<' as literal text.
                conv.push_collapsed_text("<");
                pos = lt + 1;
                continue;
            };
            let inner = &rest[1..end];
            let name_end = inner
                .bytes()
                .position(|b| !(b.is_ascii_alphanumeric() || b == b'-' || b == b':'))
                .unwrap_or(inner.len());
            let name = inner[..name_end].to_ascii_lowercase();
            let after_name = &inner[name_end..];
            // A valid HTML tag has whitespace, '/', or end-of-tag right after its name.
            let valid_delim = after_name.is_empty()
                || after_name
                    .as_bytes()
                    .first()
                    .is_some_and(|b| b.is_ascii_whitespace() || *b == b'/');
            if !valid_delim {
                conv.push_collapsed_text("<");
                pos = lt + 1;
                continue;
            }
            let trimmed_after = after_name.trim_end();
            let self_closing = trimmed_after.ends_with('/') || is_void_tag(&name);
            let raw_attrs = trimmed_after.strip_suffix('/').unwrap_or(trimmed_after);

            if is_skipped_container(&name) && !self_closing {
                pos = skip_until_close(html, lt + end + 1, &name);
                continue;
            }

            conv.handle_open(&name, raw_attrs, self_closing);
            pos = lt + end + 1;
            continue;
        }

        // Bare '<' not starting a tag (e.g. "a < b" or "<3").
        conv.push_collapsed_text("<");
        pos = lt + 1;
    }

    conv.finish()
}

/// Find the index of the closing `>` of a tag starting at `s[0] == '<'`,
/// ignoring `>` inside quoted attribute values (quotes following `=`).
/// If a quoted attribute is malformed and left unclosed until EOF, fall back
/// to the first `>` so subsequent body text is not swallowed.
fn find_tag_end(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut quote: Option<u8> = None;
    let mut after_eq = false;
    let mut first_gt: Option<usize> = None;

    for (i, &b) in bytes.iter().enumerate().skip(1) {
        if b == b'>' && first_gt.is_none() {
            first_gt = Some(i);
        }
        match quote {
            Some(q) if b == q => {
                quote = None;
                after_eq = false;
            }
            Some(_) => {}
            None if after_eq && (b == b'"' || b == b'\'') => {
                quote = Some(b);
                after_eq = false;
            }
            None if b == b'>' => return Some(i),
            None if b == b'=' => after_eq = true,
            None if after_eq && b.is_ascii_whitespace() => {}
            None => after_eq = false,
        }
    }

    if quote.is_some() { first_gt } else { None }
}

/// Advance past the matching closing `</tag>` for raw skipped containers like
/// `<script>` or `<style>`.
fn skip_until_close(html: &str, start: usize, tag: &str) -> usize {
    let mut pos = start;
    let bytes = html.as_bytes();
    while pos < bytes.len() {
        let Some(rel) = html[pos..].find("</") else {
            return bytes.len();
        };
        let close_start = pos + rel;
        let after = &html.as_bytes()[close_start + 2..];
        if after
            .get(..tag.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(tag.as_bytes()))
            && after
                .get(tag.len())
                .is_some_and(|b| *b == b'>' || b.is_ascii_whitespace())
        {
            return match find_tag_end(&html[close_start..]) {
                Some(end) => close_start + end + 1,
                None => bytes.len(),
            };
        }
        pos = close_start + 2;
    }
    bytes.len()
}

fn is_skipped_container(tag: &str) -> bool {
    matches!(
        tag,
        "script" | "style" | "noscript" | "head" | "svg" | "iframe"
    )
}

fn is_void_tag(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn is_block_tag(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "div"
            | "center"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "main"
            | "aside"
            | "figure"
            | "figcaption"
            | "address"
            | "details"
            | "summary"
            | "dl"
            | "dt"
            | "dd"
            | "form"
            | "fieldset"
            | "nav"
            | "dialog"
    )
}

/// Parse HTML attributes from the text between the tag name and `>`.
fn parse_attrs(raw: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    let bytes = raw.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        while i < len && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        if i >= len {
            break;
        }
        let name_start = i;
        while i < len && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' && bytes[i] != b'/' {
            i += 1;
        }
        let name = raw[name_start..i].to_ascii_lowercase();
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < len && bytes[i] == b'=' {
            i += 1;
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let value = if i < len && (bytes[i] == b'"' || bytes[i] == b'\'') {
                let q = bytes[i];
                i += 1;
                let val_start = i;
                while i < len && bytes[i] != q {
                    i += 1;
                }
                let val = &raw[val_start..i];
                if i < len {
                    i += 1;
                }
                val
            } else {
                let val_start = i;
                while i < len && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                &raw[val_start..i]
            };
            if !name.is_empty() {
                attrs.push((name, value.to_string()));
            }
        } else if !name.is_empty() {
            attrs.push((name, String::new()));
        }
    }
    attrs
}

/// Decode common named and numeric HTML character references.
fn decode_entities(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut pos = 0;
    while let Some(rel) = s[pos..].find('&') {
        out.push_str(&s[pos..pos + rel]);
        let amp = pos + rel;
        let rest = &s[amp + 1..];
        let semi = rest.bytes().take(12).position(|b| b == b';');
        if let Some(semi) = semi
            && let Some(decoded) = decode_entity(&rest[..semi])
        {
            out.push_str(decoded.as_str());
            pos = amp + 1 + semi + 1;
            continue;
        }
        out.push('&');
        pos = amp + 1;
    }
    out.push_str(&s[pos..]);
    Cow::Owned(out)
}

fn decode_entity(name: &str) -> Option<String> {
    if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
        let code = u32::from_str_radix(hex, 16).ok()?;
        return (code != 0)
            .then(|| char::from_u32(code))
            .flatten()
            .map(|c| c.to_string());
    }
    if let Some(dec) = name.strip_prefix('#') {
        let code = dec.parse::<u32>().ok()?;
        return (code != 0)
            .then(|| char::from_u32(code))
            .flatten()
            .map(|c| c.to_string());
    }
    let ch = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "ndash" => '\u{2013}',
        "mdash" => '\u{2014}',
        "hellip" => '\u{2026}',
        "trade" => '\u{2122}',
        "reg" => '\u{00ae}',
        "copy" => '\u{00a9}',
        "bull" => '\u{2022}',
        "lsquo" => '\u{2018}',
        "rsquo" => '\u{2019}',
        "ldquo" => '\u{201c}',
        "rdquo" => '\u{201d}',
        "times" => '\u{00d7}',
        "euro" => '\u{20ac}',
        "pound" => '\u{00a3}',
        "yen" => '\u{00a5}',
        "laquo" => '\u{00ab}',
        "raquo" => '\u{00bb}',
        _ => return None,
    };
    Some(ch.to_string())
}

fn has_odd_trailing_backslashes(s: &str) -> bool {
    s.bytes().rev().take_while(|&b| b == b'\\').count() % 2 == 1
}

/// Append `text` to `out`, collapsing ASCII whitespace runs into single spaces
/// and optionally escaping `<` when it could start a raw HTML tag in Markdown
/// (including across adjacent text chunks).
fn append_collapsed(out: &mut String, text: &str, escape_html_open: bool) {
    let mut prev_space = out.is_empty() || out.ends_with([' ', '\n']);
    let mut iter = text.chars().peekable();
    while let Some(ch) = iter.next() {
        if ch.is_ascii_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            if escape_html_open {
                if out.ends_with('<')
                    && (ch.is_ascii_alphabetic() || matches!(ch, '/' | '!' | '?'))
                    && !has_odd_trailing_backslashes(&out[..out.len() - 1])
                {
                    out.pop();
                    out.push_str("\\<");
                }
                if ch == '<'
                    && iter.peek().is_some_and(|next| {
                        next.is_ascii_alphabetic() || matches!(next, '/' | '!' | '?')
                    })
                    && !has_odd_trailing_backslashes(out)
                {
                    out.push('\\');
                }
            }
            out.push(ch);
            prev_space = false;
        }
    }
}

fn split_edge_whitespace(s: &str) -> (&str, &str, &str) {
    let start = s
        .bytes()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(s.len());
    let rest = &s[start..];
    let end_in_rest = rest
        .bytes()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(0, |i| i + 1);
    (&s[..start], &rest[..end_in_rest], &rest[end_in_rest..])
}

fn collapse_single_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    append_collapsed(&mut out, s, false);
    out.trim().to_string()
}

fn escape_alt(alt: &str) -> String {
    let mut collapsed = String::with_capacity(alt.len());
    append_collapsed(&mut collapsed, alt, true);
    collapsed.trim().replace('[', "\\[").replace(']', "\\]")
}

fn format_markdown_url(url: &str) -> String {
    if url.contains([' ', '\t', '\n', '\r', '(', ')', '<', '>']) {
        let mut out = String::with_capacity(url.len() + 8);
        for ch in url.chars() {
            match ch {
                ' ' => out.push_str("%20"),
                '\t' => out.push_str("%09"),
                '\n' => out.push_str("%0A"),
                '\r' => out.push_str("%0D"),
                '(' => out.push_str("%28"),
                ')' => out.push_str("%29"),
                '<' => out.push_str("%3C"),
                '>' => out.push_str("%3E"),
                _ => out.push(ch),
            }
        }
        out
    } else {
        url.to_string()
    }
}

fn max_backtick_run(s: &str) -> usize {
    let mut best = 0;
    let mut current = 0;
    for b in s.bytes() {
        if b == b'`' {
            current += 1;
            best = best.max(current);
        } else {
            current = 0;
        }
    }
    best
}

fn format_inline_code(content: &str) -> String {
    let ticks = max_backtick_run(content) + 1;
    let fence: String = "`".repeat(ticks);
    let pad = content.starts_with('`') || content.ends_with('`');
    if pad {
        format!("{fence} {content} {fence}")
    } else {
        format!("{fence}{content}{fence}")
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        AppContext as _, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
        Window, WindowOptions, div, px,
    };
    use irori_core::{GameId, Runner};

    use super::{html_to_markdown, prepare_markdown, rich_text, web_links_only};

    #[test]
    fn keeps_web_links_and_tags_and_empties_the_rest() {
        for kept in [
            "[site](https://example.org/a)",
            "<a href=\"http://example.org\">x</a>",
            "<p>text</p><br/>",
            "[ref]: https://example.org",
        ] {
            assert_eq!(web_links_only(kept), kept);
        }
        for (bad, secret) in [
            ("[x](file:///etc/passwd)", "passwd"),
            ("<a href=\"\\\\host\\share\\x.exe\">x</a>", "share"),
            ("<a href='smb://host/x'>x</a>", "smb"),
            ("[ref]: steam://run/1", "steam"),
            ("<ms-msdt:/id>", "msdt"),
        ] {
            assert!(!web_links_only(bad).contains(secret), "{bad}");
        }
    }

    #[test]
    fn converts_steam_style_html_with_images_and_code() {
        let html = r#"
            <h2 class="bb_tag">About This Game</h2>
            <p class="bb_paragraph"><span class="bb_img_ctn"><img class="bb_img" src="https://cdn.example.com/banner1.png" alt="Banner 1" /></span><br>Explore the world with <code>WASD</code>.</p>
            <ul class="bb_ul">
                <li><p class="bb_paragraph"><strong> Fast combat </strong> and <em>deep lore</em></p></li>
                <li><img src="https://cdn.example.com/icon.png" alt="Icon" /> Co-op mode</li>
            </ul>
        "#;
        let md = prepare_markdown(html);
        assert!(md.contains("## About This Game"), "{md}");
        assert!(
            md.contains("![Banner 1](https://cdn.example.com/banner1.png)"),
            "{md}"
        );
        assert!(md.contains("`WASD`"), "{md}");
        assert!(md.contains("- **Fast combat** and *deep lore*"), "{md}");
        assert!(
            md.contains("- ![Icon](https://cdn.example.com/icon.png) Co-op mode"),
            "{md}"
        );
        assert!(!md.contains("<p"), "{md}");
        assert!(!md.contains("<span"), "{md}");
        assert!(!md.contains("<img"), "{md}");
    }

    #[test]
    fn converts_links_linked_images_tables_and_code_blocks() {
        let html = r#"
            <p><a href="https://example.com/home"><img src="https://example.com/logo.png" alt="Logo" /></a></p>
            <p>See <a href='https://example.com/docs?a=1&amp;b=2'>the docs</a> and <a href="&#102;ile:///etc/passwd">blocked</a>.</p>
            <pre><code>fn main() {
    println!("hi");
}</code></pre>
            <table>
                <thead><tr><th>Mode</th><th>Notes</th></tr></thead>
                <tbody><tr><td>Solo</td><td>Uses <code>--solo</code> | fast</td></tr></tbody>
            </table>
        "#;
        let md = prepare_markdown(html);
        assert!(
            md.contains("[![Logo](https://example.com/logo.png)](https://example.com/home)"),
            "{md}"
        );
        assert!(
            md.contains("[the docs](https://example.com/docs?a=1&b=2)"),
            "{md}"
        );
        assert!(!md.contains("passwd"), "{md}");
        assert!(md.contains("blocked"), "{md}");
        assert!(
            md.contains("```\nfn main() {\n    println!(\"hi\");\n}\n```"),
            "{md}"
        );
        assert!(md.contains("| Mode | Notes |"), "{md}");
        assert!(md.contains("| --- | --- |"), "{md}");
        assert!(md.contains("| Solo | Uses `--solo` \\| fast |"), "{md}");
    }

    #[test]
    fn handles_malformed_html_entities_quotes_and_scripts() {
        let html = r#"<!-- comment --><script>alert("x")</script><style>.a{}</style>
            <P>Escaped &lt;div&gt; and 1 &lt; 2 &amp; &#39;ok&#39; &#x21;</P>
            <p><img alt="a > b [1]" src="https://example.com/a (1).png"><b>unclosed bold"#;
        let md = html_to_markdown(html);
        assert!(!md.contains("alert"), "{md}");
        assert!(!md.contains("comment"), "{md}");
        assert!(md.contains(r"\<div>"), "{md}");
        assert!(md.contains("1 < 2 & 'ok' !"), "{md}");
        assert!(
            md.contains("![a > b \\[1\\]](https://example.com/a%20%281%29.png)"),
            "{md}"
        );
        assert!(md.contains("**unclosed bold**"), "{md}");
    }

    #[test]
    fn leaves_plain_markdown_and_empty_text_intact() {
        assert_eq!(prepare_markdown(""), "");
        let md = "# Title\n\nUse `<code>` for spans and [link](https://example.org).";
        assert_eq!(prepare_markdown(md), md);
    }

    #[test]
    fn inline_code_with_backticks_uses_longer_fence() {
        let md = html_to_markdown("<p>Run <code>`cmd`</code> now</p>");
        assert_eq!(md, "Run `` `cmd` `` now");
    }

    struct RichTextHost {
        game_id: GameId,
        text: String,
    }

    impl Render for RichTextHost {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui_kit::Context<Self>,
        ) -> impl IntoElement {
            div()
                .w(px(640.))
                .child(rich_text("game-description", &self.game_id, &self.text))
        }
    }

    #[test]
    fn handles_utf8_in_skipped_containers_and_unclosed_code() {
        let html = "<script>const x = '</日本語>';</script><p>First <code>unclosed</p><p>Second 日本語</p>";
        let md = html_to_markdown(html);
        assert_eq!(md, "First `unclosed`\n\nSecond 日本語");
    }

    #[gpui_kit::test]
    fn multi_paragraph_html_inline_flows_and_rapid_switches_do_not_panic(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);

        // Two sibling HTML paragraphs with inline images and text where the
        // second paragraph has more inline flow items than the first. Before
        // converting HTML to Markdown, gpui-base 0.8.0's TextView::html gave
        // both paragraphs ElementId("p") and panicked in inline_flow.rs.
        let steam_html = r#"
            <p><a href="https://example.com"><img src="https://example.com/1.png" /></a> first <a href="https://example.com"><img src="https://example.com/2.png" /></a></p>
            <p><a href="https://example.com"><img src="https://example.com/3.png" /></a> one <a href="https://example.com"><img src="https://example.com/4.png" /></a> two <a href="https://example.com"><img src="https://example.com/5.png" /></a> three</p>
            <ul>
                <li><img src="https://example.com/i1.png" /> item one with <code>alpha</code></li>
                <li><img src="https://example.com/i2.png" /> item two <img src="https://example.com/i3.png" /> with <code>beta</code> and <code>gamma</code></li>
            </ul>
            <table>
                <tr><th>A</th><th>B</th></tr>
                <tr><td><img src="https://example.com/t1.png" /> cell 1</td><td><code>x</code> <img src="https://example.com/t2.png" /> cell 2</td></tr>
            </table>
        "#;
        let gog_html = r#"
            <h2><img src="https://example.com/h.png" /> Patch Notes</h2>
            <p>Fixed <code>crash</code> when switching games.</p>
            <p>Added <span class="bb_img_ctn"><img src="https://example.com/a.png" /></span> and <span class="bb_img_ctn"><img src="https://example.com/b.png" /></span> support.</p>
        "#;

        let (handle, view) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|_| RichTextHost {
                    game_id: GameId::new(Runner::Steam, "1091500"),
                    text: steam_html.to_string(),
                })
            })
            .expect("open window")
        });
        cx.run_until_parked();
        cx.refresh().expect("draw steam html");

        // Rapidly switch games and redraw multiple times.
        for (runner, app, html) in [
            (Runner::Gog, "1207658924", gog_html),
            (Runner::Steam, "1091500", steam_html),
            (Runner::Gog, "1441974651", gog_html),
        ] {
            cx.update_window(handle, |_, _, cx| {
                view.update(cx, |host, cx| {
                    host.game_id = GameId::new(runner, app);
                    host.text = html.to_string();
                    cx.notify();
                });
            })
            .expect("switch game");
            cx.run_until_parked();
            cx.refresh().expect("draw after switch");
        }
    }

    #[gpui_kit::test]
    fn unclosed_backtick_and_split_entity_html_do_not_panic(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let unclosed_backtick = "Don`t miss this!\n<p><a href=\"https://example.com\"><img src=\"https://example.com/1.png\" /></a> first <a href=\"https://example.com\"><img src=\"https://example.com/2.png\" /></a></p>\n<p><a href=\"https://example.com\"><img src=\"https://example.com/3.png\" /></a> one <a href=\"https://example.com\"><img src=\"https://example.com/4.png\" /></a> two <a href=\"https://example.com\"><img src=\"https://example.com/5.png\" /></a> three</p>";
        let split_entity = "<p>&lt;<span>p></span>&lt;<span>a href=\"https://example.com\"></span>&lt;<span>img src=\"https://example.com/1.png\" /></span>&lt;<span>/a></span> first &lt;<span>a href=\"https://example.com\"></span>&lt;<span>img src=\"https://example.com/2.png\" /></span>&lt;<span>/a></span>&lt;<span>/p></span></p>\n<p>&lt;<span>p></span>&lt;<span>a href=\"https://example.com\"></span>&lt;<span>img src=\"https://example.com/3.png\" /></span>&lt;<span>/a></span> one &lt;<span>a href=\"https://example.com\"></span>&lt;<span>img src=\"https://example.com/4.png\" /></span>&lt;<span>/a></span> two &lt;<span>a href=\"https://example.com\"></span>&lt;<span>img src=\"https://example.com/5.png\" /></span>&lt;<span>/a></span> three&lt;<span>/p></span></p>";

        let escaped = prepare_markdown(split_entity);
        assert!(
            escaped.starts_with(r"\<p>"),
            "expected escaped tag: {escaped}"
        );
        assert!(
            !escaped.contains("\n<p>"),
            "unexpected unescaped block tag: {escaped}"
        );

        let (handle, view) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |_, cx| {
                cx.new(|_| RichTextHost {
                    game_id: GameId::new(Runner::Steam, "999001"),
                    text: unclosed_backtick.to_string(),
                })
            })
            .expect("open window")
        });
        cx.run_until_parked();
        cx.refresh().expect("draw unclosed backtick html");

        cx.update_window(handle, |_, _, cx| {
            view.update(cx, |host, cx| {
                host.game_id = GameId::new(Runner::Steam, "999002");
                host.text = split_entity.to_string();
                cx.notify();
            });
        })
        .expect("switch to split entity html");
        cx.run_until_parked();
        cx.refresh().expect("draw split entity html");
    }

    #[test]
    fn mixed_markdown_with_br_preserves_structure_and_neutralizes_raw_html() {
        let md_with_br = "## v1.2.0\n\n- Fixed crash<br>on startup\n- Added `<code>` support\n- Visit <https://example.org>\n\n```html\n<p>kept in fence</p>\n```\n\n<custom-widget>hello</custom-widget>\n\n## v1.1.0\n\nInitial release.";
        let out = prepare_markdown(md_with_br);
        assert!(out.contains("## v1.2.0\n\n"), "lost heading break: {out:?}");
        assert!(
            out.contains("- Fixed crash  \non startup"),
            "expected hard line break: {out:?}"
        );
        assert!(
            out.contains("- Added `<code>` support"),
            "lost code span: {out:?}"
        );
        assert!(
            out.contains("<https://example.org>"),
            "lost web autolink: {out:?}"
        );
        assert!(
            out.contains("```html\n<p>kept in fence</p>\n```"),
            "modified fenced code block: {out:?}"
        );
        assert!(
            out.contains(r"\<custom-widget>hello\</custom-widget>"),
            "raw HTML tag not neutralized: {out:?}"
        );
        assert!(
            out.contains("\n\n## v1.1.0\n\n"),
            "lost second heading: {out:?}"
        );
    }

    #[test]
    fn malformed_quotes_nested_tables_list_paragraphs_and_inline_breaks() {
        // Unquoted apostrophe and unclosed double quote in attributes do not swallow body text.
        let unquoted = "<p title=it's>Hello world</p><p>Don't lose this!</p>";
        assert_eq!(
            html_to_markdown(unquoted),
            "Hello world\n\nDon't lose this!"
        );

        let unclosed_quote = "<p title=\"unclosed>Keep me</p>";
        assert_eq!(html_to_markdown(unclosed_quote), "Keep me");

        // Nested table inside a table cell does not destroy the outer table.
        let nested_table = "<table><tr><th>A</th><th>B</th></tr><tr><td>outer <table><tr><td>inner</td></tr></table> after</td><td>col2</td></tr></table>";
        let md_table = html_to_markdown(nested_table);
        assert!(md_table.contains("| A | B |"), "{md_table}");
        assert!(
            md_table.contains("| outer inner after | col2 |"),
            "{md_table}"
        );

        // Continuation paragraphs stay indented inside their list item.
        let list_paras = "<ul><li><p>First para</p><p>Second para</p></li><li>Next item</li></ul>";
        assert_eq!(
            html_to_markdown(list_paras),
            "- First para\n\n  Second para\n- Next item"
        );

        // Double <br> inside <strong> emits a valid single hard line break instead of breaking bold.
        let bold_br = "<p><strong>Line 1<br><br>Line 2</strong></p>";
        let md_bold = html_to_markdown(bold_br);
        assert_eq!(md_bold, "**Line 1  \nLine 2**");

        // Double <br> inside <blockquote> keeps blockquote prefixes clean.
        let quote_br = "<blockquote>Line 1<br><br>Line 2</blockquote>";
        assert_eq!(html_to_markdown(quote_br), "> Line 1\n\n> Line 2");

        // Alt text escapes tag-like `<p>` and URLs encode whitespace/angle brackets.
        let img = "<p><img alt=\"<p>banner</p>\" src=\"https://example.com/a\tb.png\" /></p>";
        assert_eq!(
            html_to_markdown(img),
            "![\\<p>banner\\</p>](https://example.com/a%09b.png)"
        );
    }
}
