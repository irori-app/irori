//! "Surprise me" shuffle showcase modal: cycles covers from the current library
//! selection, decelerating smoothly onto the game picked by
//! `random::pick_for_shuffle`, then showcases its art, thematic kicker, rating,
//! tags, synopsis and cached Linux/Deck badge with Play/Install, Shuffle again
//! (`Space`), and Open game page (`Enter` runs the primary action). Under
//! reduced motion it lands immediately.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, BoxShadow, Context, Entity, FocusHandle,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, SharedString,
    Styled as _, Task, Window, div, point, px, rems,
};
use irori_core::{ExtraInfo, GameInfo, InstallPlatform, Runner};
use irori_ui::art::LruImageCache;
use irori_ui::components::{ArtBackdrop, GameCover};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{TEXT_CONTRAST, legible, reduce_motion};
use irori_ui::{Route, backend, models, navigate};

use super::dialog_width;
use crate::compat_badges::{CompatBadge, render_cover_pill};

const SHUFFLE_STEPS: usize = 14;
const INITIAL_DELAY_MS: f64 = 55.0;
const DELAY_GROWTH: f64 = 1.16;
const MAX_DELAY_MS: u64 = 320;
const MAX_TAG_PILLS: usize = 5;
const MAX_SYNOPSIS_CHARS: usize = 260;

/// Ten tag/genre-aware thematic kicker i18n keys (checked in specificity order
/// so niche genres like Souls-like, Horror, Co-op or Retro are not drowned out
/// by broad tags like Action or Indie).
pub(crate) const THEMATIC_KICKERS: [(&str, &[&str]); 10] = [
    (
        "library.shuffle.kicker.souls",
        &[
            "souls",
            "soulslike",
            "souls-like",
            "difficult",
            "roguelike",
            "roguelite",
            "metroidvania",
            "precision platformer",
            "hardcore",
            "bullet hell",
        ],
    ),
    (
        "library.shuffle.kicker.horror",
        &[
            "horror",
            "terror",
            "survival horror",
            "psychological horror",
            "zombies",
            "lovecraftian",
        ],
    ),
    (
        "library.shuffle.kicker.coop",
        &[
            "co-op",
            "coop",
            "cooperative",
            "cooperativo",
            "multiplayer",
            "multijogador",
            "local co-op",
            "online co-op",
            "split screen",
            "party",
        ],
    ),
    (
        "library.shuffle.kicker.retro",
        &[
            "retro",
            "pixel graphics",
            "classic",
            "clássico",
            "arcade",
            "8-bit",
            "16-bit",
            "old school",
            "boomer shooter",
        ],
    ),
    (
        "library.shuffle.kicker.scifi",
        &[
            "sci-fi",
            "scifi",
            "science fiction",
            "ficção científica",
            "cyberpunk",
            "space",
            "espacial",
            "futuristic",
            "mechs",
        ],
    ),
    (
        "library.shuffle.kicker.rpg",
        &[
            "rpg",
            "jrpg",
            "crpg",
            "arpg",
            "action rpg",
            "role-playing",
            "role playing",
            "fantasy",
            "fantasia",
        ],
    ),
    (
        "library.shuffle.kicker.strategy",
        &[
            "strategy",
            "estratégia",
            "puzzle",
            "quebra-cabeça",
            "quebra-cabeca",
            "tactics",
            "tactical",
            "turn-based",
            "4x",
            "city builder",
            "card game",
            "deckbuilder",
            "management",
        ],
    ),
    (
        "library.shuffle.kicker.indie_cozy",
        &[
            "cozy",
            "aconchegante",
            "relaxing",
            "relaxante",
            "wholesome",
            "casual",
            "farming",
            "life sim",
            "walking simulator",
            "indie",
        ],
    ),
    (
        "library.shuffle.kicker.action",
        &[
            "action",
            "ação",
            "acao",
            "shooter",
            "fps",
            "fighting",
            "hack and slash",
            "beat 'em up",
            "racing",
            "corrida",
            "tiro",
            "luta",
        ],
    ),
    (
        "library.shuffle.kicker.adventure",
        &[
            "adventure",
            "aventura",
            "story rich",
            "open world",
            "mundo aberto",
            "exploration",
            "exploração",
            "platformer",
            "visual novel",
            "narrative",
        ],
    ),
];

/// Ten warm hearth / bonfire / backlog lines used as fallback (and mixed in
/// occasionally so even richly tagged libraries still surface Irori's voice).
pub(crate) const HEARTH_KICKERS: [&str; 10] = [
    "library.shuffle.kicker.hearth_1",
    "library.shuffle.kicker.hearth_2",
    "library.shuffle.kicker.hearth_3",
    "library.shuffle.kicker.hearth_4",
    "library.shuffle.kicker.hearth_5",
    "library.shuffle.kicker.hearth_6",
    "library.shuffle.kicker.hearth_7",
    "library.shuffle.kicker.hearth_8",
    "library.shuffle.kicker.hearth_9",
    "library.shuffle.kicker.hearth_10",
];

/// Primary action offered once the shuffle reel lands on a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShufflePrimary {
    Play,
    Install,
}

/// Decide the primary action for a shuffle winner.
///
/// Offers [`ShufflePrimary::Play`] only when the game is truly installed and
/// available (or a web app); offers [`ShufflePrimary::Install`] when
/// uninstalled or its files went missing and the store can install it; returns
/// `None` when neither applies (e.g. third-party non-installable or a sideload
/// whose program went missing).
pub(crate) fn shuffle_primary_action(game: &GameInfo, missing: bool) -> Option<ShufflePrimary> {
    let sideload = game.runner() == Runner::Sideload;
    let browser =
        game.browser_url.is_some() || game.installed_platform() == Some(InstallPlatform::Browser);
    if game.is_installed && (!missing || browser) {
        Some(ShufflePrimary::Play)
    } else if game.installable && !sideload {
        Some(ShufflePrimary::Install)
    } else {
        None
    }
}

/// Decelerating step intervals (in milliseconds) for the shuffle reel.
pub(crate) fn shuffle_delays_ms(steps: usize) -> Vec<u64> {
    let mut delay = INITIAL_DELAY_MS;
    let mut out = Vec::with_capacity(steps);
    for _ in 0..steps {
        out.push((delay.round() as u64).min(MAX_DELAY_MS));
        delay *= DELAY_GROWTH;
    }
    out
}

/// Deterministic reel candidate index for `step`, advancing through `len`
/// covers without repeating the previous index when `len > 1`.
pub(crate) fn reel_index(step: usize, prev: usize, len: usize) -> usize {
    if len <= 1 {
        return 0;
    }
    // Coprime-friendly jump keeps covers visually varied across small pools.
    let stride = if len > 3 { 3 } else { 1 };
    let next = (prev + stride + (step % len)) % len;
    if next == prev { (next + 1) % len } else { next }
}

/// Return all thematic kicker keys matching `game`'s tags, genres, or emulated
/// system, in priority order.
pub(crate) fn matching_theme_keys(game: &GameInfo, extra: Option<&ExtraInfo>) -> Vec<&'static str> {
    let terms: Vec<String> = game
        .store_tags
        .iter()
        .chain(&game.genres)
        .chain(extra.into_iter().flat_map(|e| &e.genres))
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    let mut matched = Vec::new();
    for (key, needles) in THEMATIC_KICKERS {
        let hit = if key == "library.shuffle.kicker.retro" && game.system.is_some() {
            true
        } else {
            terms
                .iter()
                .any(|term| needles.iter().any(|needle| term_matches(term, needle)))
        };
        if hit {
            matched.push(key);
        }
    }
    matched
}

fn term_matches(term: &str, needle: &str) -> bool {
    if term == needle {
        return true;
    }
    // Avoid short substring false positives like "rpg" inside unrelated words
    // unless bounded by non-alphanumeric separators.
    if needle.len() <= 4 {
        term.split(|c: char| !c.is_alphanumeric())
            .any(|part| part == needle)
    } else {
        term.contains(needle)
    }
}

/// Pick one of the 20 localized kicker keys for `game` given a spin `seed`.
///
/// Prefers matching thematic tag/genre lines (mixing in a warm hearth line on
/// every 5th seed so richly tagged libraries still see the full set), and falls
/// back across all 10 hearth/bonfire/backlog lines when no theme matches.
pub(crate) fn pick_kicker_key(
    game: &GameInfo,
    extra: Option<&ExtraInfo>,
    seed: usize,
) -> &'static str {
    let themes = matching_theme_keys(game, extra);
    if themes.is_empty() {
        return HEARTH_KICKERS[seed % HEARTH_KICKERS.len()];
    }
    if seed % 5 == 4 {
        return HEARTH_KICKERS[(seed / 5) % HEARTH_KICKERS.len()];
    }
    themes[seed % themes.len()]
}

/// Extract a four-digit release year (`1970..=2100`) from store date strings
/// such as `"2022-02-25"`, `"Feb 25, 2022"`, `"25 Feb, 2022"`, or `"2022"`.
pub(crate) fn extract_year(date: &str) -> Option<String> {
    let bytes = date.as_bytes();
    for i in 0..bytes.len().saturating_sub(3) {
        let slice = &bytes[i..i + 4];
        if !slice.iter().all(u8::is_ascii_digit) {
            continue;
        }
        let prev_ok = i == 0 || !bytes[i - 1].is_ascii_digit();
        let next_ok = i + 4 == bytes.len() || !bytes[i + 4].is_ascii_digit();
        if !prev_ok || !next_ok {
            continue;
        }
        if let Ok(year_str) = std::str::from_utf8(slice)
            && let Ok(year) = year_str.parse::<u16>()
            && (1970..=2100).contains(&year)
        {
            return Some(year_str.to_owned());
        }
    }
    None
}

/// Up to [`MAX_TAG_PILLS`] deduplicated tag/genre display pills for a game
/// (preserving order: user store tags first, then genres).
pub(crate) fn collect_tag_pills(game: &GameInfo, extra: Option<&ExtraInfo>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(MAX_TAG_PILLS);
    for raw in game
        .store_tags
        .iter()
        .chain(&game.genres)
        .chain(extra.into_iter().flat_map(|e| &e.genres))
    {
        let label = raw.trim();
        if label.is_empty() {
            continue;
        }
        if seen.insert(label.to_lowercase()) {
            out.push(label.to_owned());
            if out.len() >= MAX_TAG_PILLS {
                break;
            }
        }
    }
    out
}

/// Strip HTML, BBCode, and simple Markdown from a store description into a
/// clean single-paragraph plain-text synopsis capped at [`MAX_SYNOPSIS_CHARS`]
/// characters on a word boundary.
pub(crate) fn clean_synopsis(raw: &str) -> Option<String> {
    let without_blocks = strip_html_blocks(raw);
    let no_html = strip_html_tags(&without_blocks);
    let no_bb = strip_bbcode(&no_html);
    let decoded = decode_html_entities(&no_bb);
    let no_md = strip_simple_markdown(&decoded);

    let mut collapsed = String::with_capacity(no_md.len().min(MAX_SYNOPSIS_CHARS + 32));
    for word in no_md.split_whitespace() {
        if !collapsed.is_empty() {
            collapsed.push(' ');
        }
        collapsed.push_str(word);
    }
    let trimmed = collapsed.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.chars().count() <= MAX_SYNOPSIS_CHARS {
        return Some(trimmed.to_owned());
    }

    let mut cut_byte = trimmed.len();
    for (count, (byte_ix, _)) in trimmed.char_indices().enumerate() {
        if count == MAX_SYNOPSIS_CHARS {
            cut_byte = byte_ix;
            break;
        }
    }
    let prefix = &trimmed[..cut_byte];
    let boundary = prefix
        .rfind(char::is_whitespace)
        .filter(|&ix| ix > cut_byte / 2)
        .unwrap_or(cut_byte);
    let clipped = prefix[..boundary].trim_end_matches(|c: char| {
        c.is_whitespace() || matches!(c, ',' | ';' | ':' | '-' | '—' | '–' | '(')
    });
    Some(format!("{clipped}…"))
}

fn strip_html_blocks(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0;
    while cursor < input.len() {
        let rest_lower = &lower[cursor..];
        let next_block = ["<script", "<style"]
            .into_iter()
            .filter_map(|tag| rest_lower.find(tag).map(|pos| (pos, tag)))
            .min_by_key(|(pos, _)| *pos);
        let Some((rel_pos, tag)) = next_block else {
            out.push_str(&input[cursor..]);
            break;
        };
        out.push_str(&input[cursor..cursor + rel_pos]);
        let close_tag = if tag == "<script" {
            "</script>"
        } else {
            "</style>"
        };
        let search_from = cursor + rel_pos + tag.len();
        if let Some(close_rel) = lower[search_from..].find(close_tag) {
            cursor = search_from + close_rel + close_tag.len();
            out.push(' ');
        } else {
            break;
        }
    }
    out
}

fn strip_html_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    let mut tag_buf = String::new();
    for ch in input.chars() {
        if in_tag {
            if ch == '>' {
                let tag_name = tag_buf
                    .trim_start_matches('/')
                    .split(|c: char| c.is_whitespace() || c == '/')
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if matches!(
                    tag_name.as_str(),
                    "br" | "p" | "div" | "li" | "h1" | "h2" | "h3" | "h4" | "tr"
                ) {
                    out.push(' ');
                }
                tag_buf.clear();
                in_tag = false;
            } else if tag_buf.len() < 32 {
                tag_buf.push(ch);
            }
        } else if ch == '<' {
            in_tag = true;
            tag_buf.clear();
        } else {
            out.push(ch);
        }
    }
    if in_tag {
        // Unclosed `<` (e.g. "score < 50"): keep literal text.
        out.push('<');
        out.push_str(&tag_buf);
    }
    out
}

fn strip_bbcode(input: &str) -> String {
    const BBCODE_TAGS: &[&str] = &[
        "b", "i", "u", "s", "strike", "h1", "h2", "h3", "url", "img", "list", "olist", "*",
        "quote", "code", "spoiler", "noparse", "hr", "table", "tr", "th", "td",
    ];
    let mut out = String::with_capacity(input.len());
    let mut chars = input.char_indices().peekable();
    while let Some((ix, ch)) = chars.next() {
        if ch == '['
            && let Some(end_rel) = input[ix + 1..].find(']')
        {
            let inner = &input[ix + 1..ix + 1 + end_rel];
            let name = inner
                .trim_start_matches('/')
                .split('=')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if BBCODE_TAGS.contains(&name.as_str()) {
                if matches!(name.as_str(), "h1" | "h2" | "h3" | "*" | "hr" | "list") {
                    out.push(' ');
                }
                let target = ix + 1 + end_rel;
                while let Some(&(next_ix, _)) = chars.peek() {
                    chars.next();
                    if next_ix >= target {
                        break;
                    }
                }
                continue;
            }
        }
        out.push(ch);
    }
    out
}

fn decode_html_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0;
    while let Some(rel) = input[cursor..].find('&') {
        let amp = cursor + rel;
        out.push_str(&input[cursor..amp]);
        if let Some(semi_rel) = input[amp + 1..].find(';')
            && semi_rel <= 12
        {
            let entity = &input[amp + 1..amp + 1 + semi_rel];
            if let Some(decoded) = decode_entity(entity) {
                out.push_str(&decoded);
                cursor = amp + 1 + semi_rel + 1;
                continue;
            }
        }
        out.push('&');
        cursor = amp + 1;
    }
    out.push_str(&input[cursor..]);
    out
}

fn decode_entity(entity: &str) -> Option<String> {
    match entity {
        "amp" => Some("&".into()),
        "lt" => Some("<".into()),
        "gt" => Some(">".into()),
        "quot" => Some("\"".into()),
        "apos" => Some("'".into()),
        "nbsp" => Some(" ".into()),
        "mdash" => Some("—".into()),
        "ndash" => Some("–".into()),
        "hellip" => Some("…".into()),
        "lsquo" | "rsquo" => Some("'".into()),
        "ldquo" | "rdquo" => Some("\"".into()),
        "trade" => Some("™".into()),
        "reg" => Some("®".into()),
        "copy" => Some("©".into()),
        _ if entity.starts_with("#x") || entity.starts_with("#X") => {
            u32::from_str_radix(&entity[2..], 16)
                .ok()
                .and_then(char::from_u32)
                .map(|c| c.to_string())
        }
        _ if entity.starts_with('#') => entity[1..]
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .map(|c| c.to_string()),
        _ => None,
    }
}

fn strip_simple_markdown(input: &str) -> String {
    // Turn Markdown links `[label](url)` into `label`.
    let mut linked = String::with_capacity(input.len());
    let mut cursor = 0;
    while cursor < input.len() {
        let rest = &input[cursor..];
        let Some(open_rel) = rest.find('[') else {
            linked.push_str(rest);
            break;
        };
        let open = cursor + open_rel;
        linked.push_str(&input[cursor..open]);
        if let Some(mid_rel) = input[open + 1..].find("](") {
            let mid = open + 1 + mid_rel;
            if let Some(close_rel) = input[mid + 2..].find(')') {
                let label = &input[open + 1..mid];
                let url = &input[mid + 2..mid + 2 + close_rel];
                if !url.contains(char::is_whitespace) {
                    linked.push_str(label);
                    cursor = mid + 2 + close_rel + 1;
                    continue;
                }
            }
        }
        linked.push('[');
        cursor = open + 1;
    }

    let mut out = String::with_capacity(linked.len());
    for line in linked.lines() {
        if !out.is_empty() {
            out.push(' ');
        }
        let cleaned_line = line
            .trim_start()
            .trim_start_matches('#')
            .trim_start_matches(['-', '*', '>'])
            .trim_start();
        out.push_str(cleaned_line);
    }
    out.replace("**", "").replace("__", "").replace('`', "")
}

/// First available raw description for `winner`, preferring concise store
/// summaries when loaded.
fn raw_synopsis<'a>(winner: &'a GameInfo, extra: Option<&'a ExtraInfo>) -> Option<&'a str> {
    extra
        .and_then(|e| e.short_description.as_deref())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            winner
                .description
                .as_deref()
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(|| {
            winner
                .extra
                .as_ref()
                .and_then(|e| e.short_description.as_deref().or(e.description.as_deref()))
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(|| {
            extra
                .and_then(|e| e.description.as_deref())
                .filter(|s| !s.trim().is_empty())
        })
}

struct ShuffleDialog {
    pool: Vec<Arc<GameInfo>>,
    winner: Arc<GameInfo>,
    shown: Arc<GameInfo>,
    landed: bool,
    landed_kicker: Option<&'static str>,
    spin_count: usize,
    extra: Option<ExtraInfo>,
    loading_extra: bool,
    compat: Option<CompatBadge>,
    images: Entity<LruImageCache>,
    focus: FocusHandle,
    _spin: Option<Task<()>>,
    _extra_task: Option<Task<()>>,
    _compat_task: Option<Task<()>>,
}

impl ShuffleDialog {
    fn new(pool: Vec<Arc<GameInfo>>, winner: Arc<GameInfo>, cx: &mut Context<Self>) -> Self {
        let images = LruImageCache::new(24, cx);
        let seed = (irori_core::util::unix_now() as usize)
            .wrapping_add(winner.title.len())
            .wrapping_add(pool.len());
        let mut this = Self {
            shown: pool.first().cloned().unwrap_or_else(|| winner.clone()),
            pool,
            winner,
            landed: false,
            landed_kicker: None,
            spin_count: seed,
            extra: None,
            loading_extra: false,
            compat: None,
            images,
            focus: cx.focus_handle(),
            _spin: None,
            _extra_task: None,
            _compat_task: None,
        };
        this.start_spin(cx);
        this
    }

    fn start_spin(&mut self, cx: &mut Context<Self>) {
        // Cancel any previous winner's background lookups immediately.
        self.extra = None;
        self.loading_extra = false;
        self.compat = None;
        self.landed_kicker = None;
        self._extra_task = None;
        self._compat_task = None;

        // Kick off winner metadata + cached compat lookups while the reel spins
        // so the showcase card is populated as soon as the cover lands.
        self.load_winner_enrichment(cx);

        if reduce_motion(cx) || self.pool.len() <= 1 {
            self.shown = self.winner.clone();
            self.landed = true;
            self.landed_kicker = Some(pick_kicker_key(
                &self.winner,
                self.extra.as_ref(),
                self.spin_count,
            ));
            self._spin = None;
            cx.notify();
            return;
        }
        self.landed = false;
        let delays = shuffle_delays_ms(SHUFFLE_STEPS);
        self._spin = Some(cx.spawn(async move |this, cx| {
            let mut prev = 0usize;
            for (step, ms) in delays.into_iter().enumerate() {
                cx.background_executor()
                    .timer(Duration::from_millis(ms))
                    .await;
                let keep_going = this
                    .update(cx, |this, cx| {
                        let len = this.pool.len();
                        let ix = reel_index(step, prev, len);
                        prev = ix;
                        if let Some(game) = this.pool.get(ix) {
                            this.shown = game.clone();
                            cx.notify();
                        }
                    })
                    .is_ok();
                if !keep_going {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.shown = this.winner.clone();
                this.landed = true;
                this.landed_kicker = Some(pick_kicker_key(
                    &this.winner,
                    this.extra.as_ref(),
                    this.spin_count,
                ));
                this._spin = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Load cached compatibility (disk-only, zero network) and fetch missing
    /// store synopsis/score via `Backend::extra_info` when online. Both tasks
    /// cancel automatically when dropped on close or reshuffle.
    fn load_winner_enrichment(&mut self, cx: &mut Context<Self>) {
        let winner = self.winner.clone();
        let target_id = winner.id();
        let b = backend(cx);

        // Disk-cached ProtonDB / Steam Deck badge (always safe, zero network).
        let compat_backend = b.clone();
        let compat_game = winner.clone();
        let compat_id = target_id.clone();
        let compat_job = irori_ui::run(cx, async move {
            Ok(compat_backend
                .cached_compat(&compat_game)
                .await
                .flatten()
                .and_then(|c| CompatBadge::from_compat(&c)))
        });
        self._compat_task = Some(cx.spawn(async move |this, cx| {
            let badge = compat_job.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                if this.winner.id() == compat_id {
                    this.compat = badge;
                    this._compat_task = None;
                    cx.notify();
                }
            });
        }));

        // Fetch `extra_info` only when synopsis (or rating/date) is missing and online.
        let has_synopsis = raw_synopsis(&winner, None)
            .and_then(clean_synopsis)
            .is_some();
        let needs_extra = !has_synopsis || winner.rating.is_none();
        if !needs_extra || b.working_offline() {
            return;
        }

        self.loading_extra = !has_synopsis;
        let extra_backend = b;
        let extra_id = target_id;
        let fetch_id = extra_id.clone();
        let extra_job = irori_ui::run(cx, async move { extra_backend.extra_info(&fetch_id).await });
        self._extra_task = Some(cx.spawn(async move |this, cx| {
            let result = extra_job.await;
            let _ = this.update(cx, |this, cx| {
                if this.winner.id() == extra_id {
                    this.loading_extra = false;
                    this._extra_task = None;
                    if let Ok(info) = result {
                        this.extra = Some(info);
                    }
                    cx.notify();
                }
            });
        }));
    }

    fn reshuffle(&mut self, cx: &mut Context<Self>) {
        let Some((pool, winner)) = crate::random::pick_for_shuffle(cx) else {
            return;
        };
        self.pool = pool;
        self.winner = winner;
        self.spin_count = self.spin_count.wrapping_add(1);
        self.start_spin(cx);
    }

    fn trigger_primary(&self, window: &mut Window, cx: &mut Context<Self>) {
        let winner_id = self.winner.id();
        let missing =
            self.winner.is_installed && crate::view::remove::is_game_unavailable(&winner_id, cx);
        let action = shuffle_primary_action(&self.winner, missing);
        window.close_dialog(cx);
        match action {
            Some(ShufflePrimary::Play) => {
                window.defer(cx, move |window, cx| {
                    irori_ui::hooks::play_game(winner_id, window, cx);
                });
            }
            Some(ShufflePrimary::Install) => {
                window.defer(cx, move |window, cx| {
                    irori_ui::hooks::install_game(winner_id, window, cx);
                });
            }
            None => navigate(Route::Game(winner_id), cx),
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = &event.keystroke.modifiers;
        if m.control || m.platform || m.alt {
            return;
        }
        // Only handle modal-level Space/Enter shortcuts while the modal itself
        // holds focus; if the user tabbed onto a button (Open game page, Close,
        // Shuffle again), let that button handle Enter/Space without hijacking
        // or double-triggering it.
        if !self.focus.is_focused(window) {
            return;
        }
        match event.keystroke.key.as_str() {
            "space" => {
                cx.stop_propagation();
                self.reshuffle(cx);
            }
            "enter" if self.landed => {
                cx.stop_propagation();
                self.trigger_primary(window, cx);
            }
            _ => {}
        }
    }
}

fn stat_pill(
    icon: IconName,
    icon_color: gpui_kit::Hsla,
    label: SharedString,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .gap_1()
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(theme.secondary.opacity(0.75))
        .border_1()
        .border_color(theme.border.opacity(0.5))
        .items_center()
        .text_xs()
        .font_semibold()
        .text_color(theme.foreground)
        .child(Icon::new(icon).xsmall().text_color(icon_color))
        .child(label)
        .into_any_element()
}

impl Render for ShuffleDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let primary = theme.primary;
        let border = theme.border;
        let secondary = theme.secondary;
        let radius = theme.radius_lg;
        let active_fg = legible(primary, &[theme.background], TEXT_CONTRAST, cx);
        let landed = self.landed;
        let winner = self.winner.clone();
        let winner_id = winner.id();
        let missing =
            winner.is_installed && crate::view::remove::is_game_unavailable(&winner_id, cx);
        let primary_action = shuffle_primary_action(&winner, missing);
        let minutes = models(cx).library.read(cx).playtime_minutes(&winner_id);

        let extra = self.extra.as_ref();
        let hero_art = if landed {
            winner.hero_art().map(str::to_owned)
        } else {
            self.shown.hero_art().map(str::to_owned)
        };

        let glow = landed.then(|| BoxShadow {
            color: primary.opacity(0.45),
            offset: point(px(0.), px(12.)),
            blur_radius: px(32.),
            spread_radius: px(-6.),
            inset: false,
        });

        let kicker_text = if landed {
            let key = self
                .landed_kicker
                .unwrap_or_else(|| pick_kicker_key(&winner, extra, self.spin_count));
            t(key)
        } else {
            t("library.shuffle.shuffling")
        };

        let year = winner
            .release_date
            .as_deref()
            .and_then(extract_year)
            .or_else(|| {
                extra
                    .and_then(|e| e.release_date.as_deref())
                    .and_then(extract_year)
            });
        let developer = winner
            .developer
            .as_deref()
            .filter(|d| !d.trim().is_empty())
            .or_else(|| {
                extra
                    .and_then(|e| e.developers.first())
                    .map(String::as_str)
                    .filter(|d| !d.trim().is_empty())
            });
        let mut meta_parts: Vec<String> = vec![winner.runner().store_name().to_owned()];
        if let Some(y) = year {
            meta_parts.push(y);
        }
        if let Some(dev) = developer {
            meta_parts.push(dev.trim().to_owned());
        }
        if missing {
            meta_parts.push(t("library.caption.files_missing").to_string());
        }
        let meta_line = meta_parts.join(" · ");

        let score = winner
            .rating
            .or_else(|| extra.and_then(|e| e.score.as_ref()).map(|s| s.score));
        let score_color = score.map(|s| {
            if s >= 75 {
                theme.success
            } else if s >= 50 {
                theme.warning
            } else {
                muted
            }
        });
        let playtime_text = crate::format::playtime(minutes);
        let compat_pill = self
            .compat
            .map(|badge| render_cover_pill(badge, secondary.opacity(0.85), cx));
        let has_stats = score.is_some() || playtime_text.is_some() || compat_pill.is_some();

        let tag_pills = collect_tag_pills(&winner, extra);
        let synopsis = raw_synopsis(&winner, extra).and_then(clean_synopsis);
        let has_synopsis = synopsis.is_some();

        let action_id = winner_id.clone();
        let open_id = winner_id;

        let right_column: AnyElement = if landed {
            v_flex()
                .flex_1()
                .min_w_0()
                .pr_6()
                .gap_2p5()
                .child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .text_xs()
                        .font_semibold()
                        .text_color(active_fg)
                        .child(Icon::new(IconName::Sparkles).xsmall().flex_shrink_0())
                        .child(div().min_w_0().truncate().child(kicker_text)),
                )
                .child(
                    div()
                        .debug_selector(|| "shuffle-winner-title".into())
                        .text_xl()
                        .font_extrabold()
                        .text_color(fg)
                        .line_clamp(2)
                        .child(winner.title.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .truncate()
                        .child(meta_line),
                )
                .when(has_stats, |el| {
                    el.child(
                        h_flex()
                            .flex_wrap()
                            .items_center()
                            .gap_1p5()
                            .when_some(score.zip(score_color), |row, (s, color)| {
                                row.child(stat_pill(
                                    IconName::Star,
                                    color,
                                    t_args("library.shuffle.score", &[("score", &s.to_string())]),
                                    cx,
                                ))
                            })
                            .when_some(playtime_text, |row, pt| {
                                row.child(stat_pill(IconName::Clock, muted, pt.into(), cx))
                            })
                            .children(compat_pill),
                    )
                })
                .when(!tag_pills.is_empty(), |el| {
                    el.child(
                        h_flex()
                            .flex_wrap()
                            .gap_1p5()
                            .children(tag_pills.into_iter().map(|tag| {
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .bg(secondary.opacity(0.6))
                                    .border_1()
                                    .border_color(border.opacity(0.55))
                                    .text_xs()
                                    .text_color(muted)
                                    .child(tag)
                            })),
                    )
                })
                .when_some(synopsis, |el, text| {
                    el.child(
                        div()
                            .debug_selector(|| "shuffle-synopsis".into())
                            .pt_0p5()
                            .text_sm()
                            .text_color(fg.opacity(0.88))
                            .line_clamp(4)
                            .child(text),
                    )
                })
                .when(!has_synopsis && self.loading_extra, |el| {
                    el.child(
                        v_flex()
                            .pt_1()
                            .gap_1p5()
                            .child(Skeleton::new().h_3().w_full())
                            .child(Skeleton::new().h_3().w(rems(14.))),
                    )
                })
                .into_any_element()
        } else {
            v_flex()
                .flex_1()
                .min_w_0()
                .pr_6()
                .gap_2p5()
                .pt_2()
                .child(
                    h_flex()
                        .gap_1p5()
                        .items_center()
                        .text_xs()
                        .font_semibold()
                        .text_color(active_fg)
                        .child(Icon::new(IconName::Dices).xsmall().flex_shrink_0())
                        .child(kicker_text),
                )
                .child(
                    div()
                        .text_xl()
                        .font_extrabold()
                        .text_color(muted)
                        .line_clamp(2)
                        .child(self.shown.title.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted.opacity(0.75))
                        .truncate()
                        .child(self.shown.runner().store_name()),
                )
                .into_any_element()
        };

        let footer = h_flex()
            .w_full()
            .min_h(rems(2.5))
            .pt_3()
            .border_t_1()
            .border_color(border.opacity(0.5))
            .items_center()
            .justify_between()
            .gap_2()
            .flex_wrap()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("shuffle-again")
                            .debug_selector(|| "shuffle-again".into())
                            .outline()
                            .icon(IconName::Dices)
                            .label(t("library.shuffle.again"))
                            .tooltip(t("library.shuffle.again_hint"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.focus.focus(window, cx);
                                this.reshuffle(cx);
                            })),
                    )
                    .child(
                        div()
                            .px_1p5()
                            .py_0p5()
                            .rounded_sm()
                            .bg(secondary.opacity(0.6))
                            .border_1()
                            .border_color(border.opacity(0.6))
                            .text_xs()
                            .text_color(muted)
                            .child(t("library.shuffle.key_space")),
                    ),
            )
            .child(h_flex().items_center().gap_2().when(landed, |el| {
                let open_btn = Button::new("shuffle-open")
                    .debug_selector(|| "shuffle-open".into())
                    .label(t("library.command.open_page"))
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        navigate(Route::Game(open_id.clone()), cx);
                    });
                let open_btn = if primary_action.is_some() {
                    open_btn.ghost()
                } else {
                    open_btn.primary()
                };
                el.child(open_btn).when_some(primary_action, |el, action| {
                    let (primary_icon, primary_label) = match action {
                        ShufflePrimary::Play => (IconName::Play, t("library.command.play")),
                        ShufflePrimary::Install => {
                            (IconName::Download, t("library.command.install"))
                        }
                    };
                    el.child(
                        Button::new("shuffle-primary")
                            .debug_selector(|| "shuffle-primary".into())
                            .primary()
                            .icon(primary_icon)
                            .label(primary_label)
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let id = action_id.clone();
                                window.defer(cx, move |window, cx| match action {
                                    ShufflePrimary::Play => {
                                        irori_ui::hooks::play_game(id, window, cx);
                                    }
                                    ShufflePrimary::Install => {
                                        irori_ui::hooks::install_game(id, window, cx);
                                    }
                                });
                            }),
                    )
                })
            }));

        div()
            .image_cache(self.images.clone())
            .id("library-shuffle-dialog")
            .debug_selector(|| "library-shuffle-dialog".into())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .relative()
            .w_full()
            .overflow_hidden()
            .rounded(radius)
            .child(
                ArtBackdrop::new(hero_art)
                    .strength(0.75)
                    .image_cache(&self.images),
            )
            .child(
                v_flex()
                    .relative()
                    .w_full()
                    .p_6()
                    .gap_5()
                    .child(
                        h_flex()
                            .w_full()
                            .items_start()
                            .gap_6()
                            .child(
                                div()
                                    .flex_none()
                                    .rounded(radius)
                                    .when_some(glow, |el, shadow| el.shadow(vec![shadow]))
                                    .child(
                                        GameCover::new(self.shown.clone())
                                            .width(rems(11.5))
                                            .radius(radius),
                                    ),
                            )
                            .child(right_column),
                    )
                    .child(footer),
            )
            .child(
                div().absolute().top_2p5().right_2p5().child(
                    Button::new("shuffle-close")
                        .debug_selector(|| "shuffle-close".into())
                        .ghost()
                        .xsmall()
                        .icon(IconName::X)
                        .tooltip(t("library.shuffle.close"))
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                ),
            )
    }
}

/// Open the "Surprise me" shuffle showcase modal for the current library
/// selection, or push a notice when no games are eligible.
pub fn open_shuffle_dialog(window: &mut Window, cx: &mut App) {
    let Some((pool, winner)) = crate::random::pick_for_shuffle(cx) else {
        window.push_notification(
            Notification::info(t("library.random.empty")).title(t("library.random.title")),
            cx,
        );
        return;
    };
    let view = cx.new(|cx| ShuffleDialog::new(pool, winner, cx));
    let width = dialog_width(window, 38.);
    let focus = std::cell::Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if focus.replace(false) {
            let handle = view.read(cx).focus.clone();
            handle.focus(window, cx);
        }
        let view = view.clone();
        dialog
            .close_button(false)
            .p_0()
            .w(width)
            .content(move |content, _, _| content.child(view.clone()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_decelerate_and_stay_bounded() {
        let delays = shuffle_delays_ms(SHUFFLE_STEPS);
        assert_eq!(delays.len(), SHUFFLE_STEPS);
        for pair in delays.windows(2) {
            assert!(pair[1] >= pair[0], "must decelerate: {delays:?}");
            assert!(pair[1] <= MAX_DELAY_MS);
        }
        assert!(*delays.last().unwrap() > delays[0]);
    }

    #[test]
    fn reel_index_stays_in_bounds_and_varies() {
        assert_eq!(reel_index(0, 0, 1), 0);
        let mut prev = 0;
        for step in 0..20 {
            let next = reel_index(step, prev, 7);
            assert!(next < 7);
            assert_ne!(next, prev);
            prev = next;
        }
    }

    #[test]
    fn primary_action_respects_installation_missing_files_and_installability() {
        let store_game = |installed: bool, installable: bool| GameInfo {
            runner: Some(Runner::Legendary),
            app_name: "112".into(),
            is_installed: installed,
            installable,
            ..Default::default()
        };
        assert_eq!(
            shuffle_primary_action(&store_game(true, true), false),
            Some(ShufflePrimary::Play)
        );
        // Installed record whose files went missing on disk -> Install, never Play.
        assert_eq!(
            shuffle_primary_action(&store_game(true, true), true),
            Some(ShufflePrimary::Install)
        );
        // Uninstalled store game -> Install.
        assert_eq!(
            shuffle_primary_action(&store_game(false, true), false),
            Some(ShufflePrimary::Install)
        );
        // Non-installable (e.g. third-party) and not ready -> no Play/Install button.
        assert_eq!(
            shuffle_primary_action(&store_game(false, false), false),
            None
        );
        assert_eq!(shuffle_primary_action(&store_game(true, false), true), None);

        // Sideloaded app whose program is missing cannot be installed via store.
        let sideload = GameInfo {
            runner: Some(Runner::Sideload),
            app_name: "custom".into(),
            is_installed: true,
            installable: false,
            ..Default::default()
        };
        assert_eq!(
            shuffle_primary_action(&sideload, false),
            Some(ShufflePrimary::Play)
        );
        assert_eq!(shuffle_primary_action(&sideload, true), None);

        // Browser web apps remain playable even if `missing` were flagged.
        let web = GameInfo {
            runner: Some(Runner::Sideload),
            app_name: "web".into(),
            is_installed: true,
            browser_url: Some("https://example.com".into()),
            ..Default::default()
        };
        assert_eq!(
            shuffle_primary_action(&web, true),
            Some(ShufflePrimary::Play)
        );
    }

    #[test]
    fn twenty_kicker_variations_cover_all_ten_themes_and_ten_hearth_lines() {
        let all_keys: HashSet<&str> = THEMATIC_KICKERS
            .iter()
            .map(|(k, _)| *k)
            .chain(HEARTH_KICKERS)
            .collect();
        assert_eq!(all_keys.len(), 20);

        let sample_tags = [
            ("Souls-like", "library.shuffle.kicker.souls"),
            ("Psychological Horror", "library.shuffle.kicker.horror"),
            ("Online Co-Op", "library.shuffle.kicker.coop"),
            ("Pixel Graphics", "library.shuffle.kicker.retro"),
            ("Cyberpunk", "library.shuffle.kicker.scifi"),
            ("JRPG", "library.shuffle.kicker.rpg"),
            ("Turn-Based Strategy", "library.shuffle.kicker.strategy"),
            ("Cozy", "library.shuffle.kicker.indie_cozy"),
            ("Action", "library.shuffle.kicker.action"),
            ("Story Rich", "library.shuffle.kicker.adventure"),
        ];
        for (tag, expected) in sample_tags {
            let game = GameInfo {
                store_tags: vec![tag.into()],
                ..Default::default()
            };
            assert_eq!(
                pick_kicker_key(&game, None, 0),
                expected,
                "tag {tag} should match {expected}"
            );
        }

        // Emulated ROMs match Retro even without tags.
        let rom = GameInfo {
            system: Some("snes".into()),
            ..Default::default()
        };
        assert_eq!(
            pick_kicker_key(&rom, None, 0),
            "library.shuffle.kicker.retro"
        );

        // Untagged games cycle through all 10 warm hearth lines.
        let plain = GameInfo::default();
        let fallback_seen: HashSet<&str> = (0..10)
            .map(|seed| pick_kicker_key(&plain, None, seed))
            .collect();
        assert_eq!(fallback_seen.len(), 10);
        for key in HEARTH_KICKERS {
            assert!(fallback_seen.contains(key));
        }
    }

    #[test]
    fn clean_synopsis_strips_html_bbcode_markdown_entities_and_truncates_cleanly() {
        let html = "<p><strong>Explore</strong> a <em>shattered</em> realm&mdash;fight &amp; survive!<br/>[b]Now with co-op[/b] and [url=https://example.com]secrets[/url].</p><script>alert(1)</script>";
        let cleaned = clean_synopsis(html).unwrap();
        assert_eq!(
            cleaned,
            "Explore a shattered realm—fight & survive! Now with co-op and secrets."
        );

        let md = "## Welcome\nPlay **boldly** with [friends](https://example.com) &amp; win&#39;s!";
        assert_eq!(
            clean_synopsis(md).unwrap(),
            "Welcome Play boldly with friends & win's!"
        );

        assert_eq!(clean_synopsis("   <br/>  "), None);

        // Long descriptions truncate at a word boundary with an ellipsis.
        let long = "adventure ".repeat(40);
        let clipped = clean_synopsis(&long).unwrap();
        assert!(clipped.ends_with('…'));
        assert!(clipped.chars().count() <= MAX_SYNOPSIS_CHARS + 1);
    }

    #[test]
    fn extract_year_and_collect_tags_deduplicate_and_cap_at_five() {
        assert_eq!(extract_year("2022-02-25").as_deref(), Some("2022"));
        assert_eq!(extract_year("Feb 25, 2022").as_deref(), Some("2022"));
        assert_eq!(extract_year("Coming soon"), None);
        assert_eq!(extract_year("12345"), None);

        let game = GameInfo {
            store_tags: vec![
                "RPG".into(),
                "Souls-like".into(),
                "Dark Fantasy".into(),
                "rpg".into(),
            ],
            genres: vec![
                "Action".into(),
                "RPG".into(),
                "Adventure".into(),
                "Indie".into(),
            ],
            ..Default::default()
        };
        assert_eq!(
            collect_tag_pills(&game, None),
            vec!["RPG", "Souls-like", "Dark Fantasy", "Action", "Adventure"]
        );
    }
}
