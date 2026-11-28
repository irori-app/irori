//! Shared rich-text rendering: HTML and Steam BBCode converted into sanitized
//! Markdown for GPUI's `TextView::markdown`.

mod bbcode;
mod detect;
mod html;

pub use bbcode::{PreparedNews, plain_excerpt, prepare_steam_news};
pub use detect::{looks_like_html, skip_code_span_or_fence};
pub use html::{prepare_markdown, rich_text, rich_text_keyed};
