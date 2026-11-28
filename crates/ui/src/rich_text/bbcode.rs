//! Steam BBCode -> Markdown conversion for game announcements and patch notes.
//!
//! Because GPUI runs with `NullHttpClient`, Markdown image syntax never loads
//! remote URLs. Instead, the first valid `*.steamstatic.com` image URL is
//! extracted into [`PreparedNews::banner_url`] so callers render it natively
//! through `art_img` + `LruImageCache`, while subsequent images become links.

use once_cell::sync::Lazy;
use regex::Regex;

use super::html::prepare_markdown;

const CLAN_IMAGE_BASE: &str = "https://clan.fastly.steamstatic.com/images";

/// Result of preparing a Steam announcement body for native GPUI rendering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreparedNews {
    /// First validated `*.steamstatic.com` banner URL for `art_img`, if any.
    pub banner_url: Option<String>,
    /// Sanitized Markdown ready for `TextView::markdown`.
    pub markdown: String,
    /// Single-line plain excerpt for feed cards.
    pub excerpt: String,
}

static RE_IMG_TAG: Lazy<Option<Regex>> =
    Lazy::new(|| Regex::new(r"(?is)\[img\](.*?)\[/img\]|\[img\s+src=([^\]]+)\](?:\[/img\])?").ok());
static RE_HTML_IMG: Lazy<Option<Regex>> =
    Lazy::new(|| Regex::new(r#"(?is)<img\b[^>]*\bsrc\s*=\s*["']([^"']+)["'][^>]*>"#).ok());
static RE_YOUTUBE: Lazy<Option<Regex>> = Lazy::new(|| {
    Regex::new(r"(?is)\[previewyoutube=([A-Za-z0-9_-]{6,20})(?:;[^\]]*)?\]\[/previewyoutube\]").ok()
});
static RE_URL_NAMED: Lazy<Option<Regex>> =
    Lazy::new(|| Regex::new(r"(?is)\[url=([^\]]+)\](.*?)\[/url\]").ok());
static RE_URL_BARE: Lazy<Option<Regex>> =
    Lazy::new(|| Regex::new(r"(?is)\[url\](.*?)\[/url\]").ok());
static RE_HEADING: Lazy<Option<Regex>> =
    Lazy::new(|| Regex::new(r"(?is)\[h([1-3])\](.*?)\[/h[1-3]\]").ok());
static RE_BOLD: Lazy<Option<Regex>> = Lazy::new(|| Regex::new(r"(?is)\[b\](.*?)\[/b\]").ok());
static RE_ITALIC: Lazy<Option<Regex>> = Lazy::new(|| Regex::new(r"(?is)\[i\](.*?)\[/i\]").ok());
static RE_STRIKE: Lazy<Option<Regex>> =
    Lazy::new(|| Regex::new(r"(?is)\[strike\](.*?)\[/strike\]").ok());
static RE_CODE: Lazy<Option<Regex>> = Lazy::new(|| Regex::new(r"(?is)\[code\](.*?)\[/code\]").ok());
static RE_QUOTE: Lazy<Option<Regex>> =
    Lazy::new(|| Regex::new(r"(?is)\[quote(?:=[^\]]*)?\](.*?)\[/quote\]").ok());
static RE_HR: Lazy<Option<Regex>> = Lazy::new(|| Regex::new(r"(?is)\[hr\]\s*(?:\[/hr\])?").ok());
static RE_LIST_ITEM_OPEN: Lazy<Option<Regex>> = Lazy::new(|| Regex::new(r"(?i)\[\*\][ \t]*").ok());
static RE_LIST_ITEM_CLOSE: Lazy<Option<Regex>> = Lazy::new(|| Regex::new(r"(?i)\[/\*\]").ok());
static RE_LIST_WRAP: Lazy<Option<Regex>> = Lazy::new(|| {
    Regex::new(r"(?is)\[/?(?:o?list|u|spoiler|noparse|table|tr|th|td|p|dynamiclink[^\]]*)\]").ok()
});
static RE_CAROUSEL: Lazy<Option<Regex>> =
    Lazy::new(|| Regex::new(r"(?is)\[/?carousel[^\]]*\]").ok());
static RE_BLANK_RUN: Lazy<Option<Regex>> = Lazy::new(|| Regex::new(r"\n{3,}").ok());
static RE_MD_LINK: Lazy<Option<Regex>> = Lazy::new(|| Regex::new(r"\[([^\]]+)\]\([^)]+\)").ok());

fn expand_clan_tokens(raw: &str) -> String {
    raw.replace("{STEAM_CLAN_IMAGE}", CLAN_IMAGE_BASE)
        .replace("{STEAM_CLAN_LOC_IMAGE}", CLAN_IMAGE_BASE)
}

fn is_steamstatic_banner(url: &str) -> bool {
    let Some(clean) = crate::web_url(url) else {
        return false;
    };
    let lower = clean.to_ascii_lowercase();
    let Some(after_scheme) = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
    else {
        return false;
    };
    let host = after_scheme.split('/').next().unwrap_or("");
    let is_cdn = host == "steamstatic.com" || host.ends_with(".steamstatic.com");
    if !is_cdn {
        return false;
    }
    let path = after_scheme.split('?').next().unwrap_or("");
    path.ends_with(".png")
        || path.ends_with(".jpg")
        || path.ends_with(".jpeg")
        || path.ends_with(".webp")
}

/// Convert raw Steam announcement contents (BBCode, HTML, or plain text) into
/// sanitized GPUI Markdown plus an optional Steam CDN hero banner and excerpt.
pub fn prepare_steam_news(raw: &str) -> PreparedNews {
    let expanded = expand_clan_tokens(raw)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut banner_url: Option<String> = None;

    // Handle [img] tags: first valid steamstatic URL becomes the hero banner;
    // subsequent images become clean web links.
    let mut text = if let Some(re) = RE_IMG_TAG.as_ref() {
        re.replace_all(&expanded, |caps: &regex::Captures<'_>| {
            let src = caps
                .get(1)
                .or_else(|| caps.get(2))
                .map(|m| m.as_str().trim().trim_matches('"').trim_matches('\''))
                .unwrap_or("");
            if banner_url.is_none() && is_steamstatic_banner(src) {
                banner_url = crate::web_url(src);
                String::new()
            } else if let Some(valid) = crate::web_url(src) {
                format!("\n[Image]({valid})\n")
            } else {
                String::new()
            }
        })
        .into_owned()
    } else {
        expanded
    };

    // If no BBCode banner was found, inspect any inline HTML <img> tag.
    if banner_url.is_none()
        && let Some(re) = RE_HTML_IMG.as_ref()
    {
        for caps in re.captures_iter(&text) {
            if let Some(m) = caps.get(1)
                && is_steamstatic_banner(m.as_str())
            {
                banner_url = crate::web_url(m.as_str());
                break;
            }
        }
    }

    if let Some(re) = RE_YOUTUBE.as_ref() {
        text = re
            .replace_all(&text, |caps: &regex::Captures<'_>| {
                let id = caps.get(1).map_or("", |m| m.as_str());
                format!("\n[YouTube video](https://www.youtube.com/watch?v={id})\n")
            })
            .into_owned();
    }

    if let Some(re) = RE_URL_NAMED.as_ref() {
        text = re
            .replace_all(&text, |caps: &regex::Captures<'_>| {
                let raw_url = caps.get(1).map_or("", |m| {
                    m.as_str().trim().trim_matches('"').trim_matches('\'')
                });
                let label = caps.get(2).map_or("", |m| m.as_str().trim());
                match crate::web_url(raw_url) {
                    Some(valid) if !label.is_empty() => format!("[{label}]({valid})"),
                    Some(valid) => format!("<{valid}>"),
                    None => label.to_string(),
                }
            })
            .into_owned();
    }

    if let Some(re) = RE_URL_BARE.as_ref() {
        text = re
            .replace_all(&text, |caps: &regex::Captures<'_>| {
                let raw_url = caps.get(1).map_or("", |m| m.as_str().trim());
                crate::web_url(raw_url).map_or_else(String::new, |valid| format!("<{valid}>"))
            })
            .into_owned();
    }

    if let Some(re) = RE_HEADING.as_ref() {
        text = re
            .replace_all(&text, |caps: &regex::Captures<'_>| {
                let level = caps.get(1).map_or("2", |m| m.as_str());
                let body = caps.get(2).map_or("", |m| m.as_str().trim());
                let hashes = match level {
                    "1" => "##",
                    "2" => "###",
                    _ => "####",
                };
                format!("\n\n{hashes} {body}\n\n")
            })
            .into_owned();
    }

    if let Some(re) = RE_CODE.as_ref() {
        text = re
            .replace_all(&text, |caps: &regex::Captures<'_>| {
                let body = caps.get(1).map_or("", |m| m.as_str());
                if body.contains('\n') {
                    format!("\n```\n{}\n```\n", body.trim())
                } else {
                    format!("`{}`", body.trim())
                }
            })
            .into_owned();
    }

    if let Some(re) = RE_QUOTE.as_ref() {
        text = re
            .replace_all(&text, |caps: &regex::Captures<'_>| {
                let quoted = caps
                    .get(1)
                    .map_or("", |m| m.as_str().trim())
                    .lines()
                    .map(|l| format!("> {}", l.trim()))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("\n\n{quoted}\n\n")
            })
            .into_owned();
    }

    if let Some(re) = RE_BOLD.as_ref() {
        text = re.replace_all(&text, "**$1**").into_owned();
    }
    if let Some(re) = RE_ITALIC.as_ref() {
        text = re.replace_all(&text, "*$1*").into_owned();
    }
    if let Some(re) = RE_STRIKE.as_ref() {
        text = re.replace_all(&text, "~~$1~~").into_owned();
    }
    if let Some(re) = RE_HR.as_ref() {
        text = re.replace_all(&text, "\n\n---\n\n").into_owned();
    }
    if let Some(re) = RE_LIST_ITEM_OPEN.as_ref() {
        text = re.replace_all(&text, "\n- ").into_owned();
    }
    if let Some(re) = RE_LIST_ITEM_CLOSE.as_ref() {
        text = re.replace_all(&text, "").into_owned();
    }
    if let Some(re) = RE_LIST_WRAP.as_ref() {
        text = re.replace_all(&text, "\n").into_owned();
    }
    if let Some(re) = RE_CAROUSEL.as_ref() {
        text = re.replace_all(&text, "\n").into_owned();
    }
    if let Some(re) = RE_BLANK_RUN.as_ref() {
        text = re.replace_all(&text, "\n\n").into_owned();
    }

    let markdown = prepare_markdown(text.trim());
    let excerpt = plain_excerpt(&markdown, 180);
    PreparedNews {
        banner_url,
        markdown,
        excerpt,
    }
}

/// Produce a clean single-line plain-text summary from Markdown for list cards.
pub fn plain_excerpt(markdown: &str, max_chars: usize) -> String {
    let mut cleaned = if let Some(re) = RE_MD_LINK.as_ref() {
        re.replace_all(markdown, "$1").into_owned()
    } else {
        markdown.to_string()
    };
    cleaned = cleaned.replace(['#', '*', '`', '>'], " ");
    let mut out = String::new();
    for word in cleaned.split_whitespace() {
        if word == "---"
            || word == "-"
            || word.starts_with("http://")
            || word.starts_with("https://")
        {
            continue;
        }
        if !out.is_empty() {
            if out.chars().count() + 1 + word.chars().count() > max_chars {
                out.push('…');
                break;
            }
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_steam_bbcode_and_extracts_banner() {
        let raw = "[img]{STEAM_CLAN_IMAGE}/1234/banner.jpg[/img]\n\
            [h1]Patch 8[/h1]\n\
            [b]Fixes[/b] in [i]Act 3[/i]:\n\
            [list][*]Fixed [code]Champion Challenge[/code][/*][*]Improved [url=https://larian.com]Cross-Play[/url][/*][/list]\n\
            [previewyoutube=dQw4w9WgXcQ;full][/previewyoutube]";
        let prep = prepare_steam_news(raw);
        assert_eq!(
            prep.banner_url.as_deref(),
            Some("https://clan.fastly.steamstatic.com/images/1234/banner.jpg")
        );
        assert!(prep.markdown.contains("## Patch 8"));
        assert!(prep.markdown.contains("**Fixes**"));
        assert!(prep.markdown.contains("*Act 3*"));
        assert!(prep.markdown.contains("- Fixed `Champion Challenge`"));
        assert!(prep.markdown.contains("[Cross-Play](https://larian.com)"));
        assert!(
            prep.markdown
                .contains("[YouTube video](https://www.youtube.com/watch?v=dQw4w9WgXcQ)")
        );
        assert!(!prep.excerpt.is_empty());
    }

    #[test]
    fn rejects_non_steamstatic_banner_hosts() {
        let raw = "[img]https://evil.example.com/tracker.png[/img]\nHello world";
        let prep = prepare_steam_news(raw);
        assert_eq!(prep.banner_url, None);
        assert!(
            prep.markdown
                .contains("[Image](https://evil.example.com/tracker.png)")
        );
    }
}
