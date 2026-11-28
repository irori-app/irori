//! Unified accent-insensitive, bilingual scoring and dynamic group ordering
//! for the global search palette (`Ctrl+K`) and Quick Launcher (`Ctrl+Alt+K`).

use std::cmp::Reverse;

use gpui_kit::SharedString;
use irori_services::search::{FoldedField, MatchTier, Query};

use super::view::{Host, Row};

/// Semantic category of a palette entry, driving grouping and host-aware
/// tie-breaking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum EntryKind {
    /// Running or launching games, and "Save clip" while recording.
    Live,
    /// Commands scoped to the currently open game page.
    CurrentGame,
    /// Top-level pages, all Settings sections, and Settings sub-tabs.
    Page,
    /// Individual editable settings from `irori_settings::registry`.
    Setting,
    /// Library games and recent games.
    Game,
    /// Application, library, category, emulator, and launcher commands.
    Command,
}

/// Visual section shown in the palette list (`CommandGroup`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum GroupKind {
    Live,
    CurrentGame,
    Recent,
    Pages,
    Settings,
    Games,
    Commands,
    QuickActions,
}

impl GroupKind {
    /// Translation key for this group's heading.
    pub(super) fn heading_key(self) -> &'static str {
        match self {
            GroupKind::Live => "shell.palette.group.live",
            GroupKind::CurrentGame => "shell.palette.group.current_game",
            GroupKind::Recent => "shell.palette.group.recent",
            GroupKind::Pages => "shell.palette.group.pages",
            GroupKind::Settings => "shell.palette.group.settings",
            GroupKind::Games => "shell.palette.group.games",
            GroupKind::Commands => "shell.palette.group.commands",
            GroupKind::QuickActions => "shell.palette.group.quick_actions",
        }
    }

    pub(super) fn from_entry_kind(kind: EntryKind) -> Self {
        match kind {
            EntryKind::Live => GroupKind::Live,
            EntryKind::CurrentGame => GroupKind::CurrentGame,
            EntryKind::Page => GroupKind::Pages,
            EntryKind::Setting => GroupKind::Settings,
            EntryKind::Game => GroupKind::Games,
            EntryKind::Command => GroupKind::Commands,
        }
    }
}

/// Optional scope prefix parsed from the start of a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScopeFilter {
    /// Search everything (default).
    All,
    /// `,` — only Pages and Settings.
    Settings,
    /// `>` — only Commands (and current-game commands).
    Commands,
    /// `#` — only Categories and saved library views.
    Categories,
    /// `play <game>` / `jogar <game>` — prioritize launching matching installed games.
    PlayIntent,
}

/// Result of parsing raw input text for optional scope prefixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ParsedInput<'a> {
    pub(super) scope: ScopeFilter,
    /// Text to match within the chosen scope (trimmed, prefix stripped).
    pub(super) text: &'a str,
    /// Full trimmed text (used so `play status` still matches commands too).
    pub(super) full: &'a str,
}

/// Parse optional prefixes (`,`, `>`, `#`, or `play ` / `jogar `) from `raw`.
pub(super) fn parse_input(raw: &str) -> ParsedInput<'_> {
    let full = raw.trim();
    if let Some(rest) = full.strip_prefix(',') {
        return ParsedInput {
            scope: ScopeFilter::Settings,
            text: rest.trim(),
            full,
        };
    }
    if let Some(rest) = full.strip_prefix('>') {
        return ParsedInput {
            scope: ScopeFilter::Commands,
            text: rest.trim(),
            full,
        };
    }
    if let Some(rest) = full.strip_prefix('#') {
        return ParsedInput {
            scope: ScopeFilter::Categories,
            text: rest.trim(),
            full,
        };
    }
    for verb in ["play ", "jogar "] {
        if full.len() > verb.len()
            && full
                .get(..verb.len())
                .is_some_and(|p| p.eq_ignore_ascii_case(verb))
        {
            let rest = full[verb.len()..].trim();
            if !rest.is_empty() {
                return ParsedInput {
                    scope: ScopeFilter::PlayIntent,
                    text: rest,
                    full,
                };
            }
        }
    }
    ParsedInput {
        scope: ScopeFilter::All,
        text: full,
        full,
    }
}

/// Pre-folded text fields of a palette row, built once when the palette opens
/// so keystroke scoring does zero accent folding on candidate entries.
#[derive(Clone, Default)]
pub(super) struct FoldedEntry {
    labels: Vec<FoldedField>,
    keywords: Vec<FoldedField>,
    combined: Option<FoldedField>,
    help: Option<FoldedField>,
}

impl FoldedEntry {
    pub(super) fn build(
        label: &str,
        english_label: Option<&str>,
        keywords: &[SharedString],
        help: Option<&str>,
        english_help: Option<&str>,
    ) -> Self {
        let mut labels = Vec::with_capacity(2);
        if let Some(f) = FoldedField::new(label) {
            labels.push(f);
        }
        if let Some(en) = english_label
            && let Some(f) = FoldedField::new(en)
            && labels.iter().all(|existing| existing.text() != f.text())
        {
            labels.push(f);
        }

        let mut kw_fields = Vec::new();
        let mut combined_buf = String::with_capacity(128);
        let push_buf = |part: &str, buf: &mut String| {
            let part = part.trim();
            if !part.is_empty() {
                if !buf.is_empty() {
                    buf.push(' ');
                }
                buf.push_str(part);
            }
        };
        push_buf(label, &mut combined_buf);
        if let Some(en) = english_label {
            push_buf(en, &mut combined_buf);
        }
        for kw in keywords {
            push_buf(kw.as_ref(), &mut combined_buf);
            if let Some(f) = FoldedField::new(kw.as_ref()) {
                kw_fields.push(f);
            }
        }

        let mut help_buf = String::new();
        if let Some(h) = help {
            push_buf(h, &mut help_buf);
        }
        if let Some(en_h) = english_help
            && help != Some(en_h)
        {
            push_buf(en_h, &mut help_buf);
        }

        Self {
            labels,
            keywords: kw_fields,
            combined: FoldedField::new(&combined_buf),
            help: FoldedField::new(&help_buf),
        }
    }
}

/// Minimum folded query character length before matching inside a setting's
/// help text (prevents 1-2 char queries from matching hundreds of descriptions).
const MIN_HELP_QUERY_CHARS: usize = 3;

/// Score a non-game palette entry against a folded `query`; higher is better.
pub(super) fn score_entry(kind: EntryKind, folded: &FoldedEntry, query: &Query) -> Option<i64> {
    let (exact, prefix, word, kw_match, fuzzy) = match kind {
        EntryKind::Live => (7000, 6000, 5000, 4000, 2200),
        EntryKind::Page => (6200, 5200, 4300, 3200, 2100),
        EntryKind::Setting => (6000, 5000, 4200, 3100, 2000),
        EntryKind::CurrentGame => (5900, 4900, 4100, 3000, 1950),
        EntryKind::Command => (5800, 4800, 4000, 2800, 1900),
        EntryKind::Game => (6000, 5000, 4000, 3000, 2000),
    };

    let mut best: Option<i64> = None;
    let take = |best: &mut Option<i64>, score: i64| {
        *best = Some((*best).map_or(score, |b| b.max(score)));
    };

    for field in &folded.labels {
        if let Some((tier, typos)) = field.matches(query) {
            let s = match tier {
                MatchTier::Exact => exact,
                MatchTier::Prefix => prefix,
                MatchTier::Word => word,
                MatchTier::Fuzzy => fuzzy - 100 * i64::from(typos),
            };
            take(&mut best, s);
        } else if field.contains_all_words(query) {
            // Mid-word match in the label itself (e.g. "hud" in "MangoHud",
            // "sync" in "Fsync").
            take(&mut best, kw_match + 150);
        }
    }

    // Check individual keyword phrases and whole-token keyword hits.
    for kw in &folded.keywords {
        if let Some((tier, _)) = kw.matches(query) {
            let s = match tier {
                MatchTier::Exact => word,
                MatchTier::Prefix | MatchTier::Word => kw_match,
                MatchTier::Fuzzy => continue,
            };
            take(&mut best, s);
        } else if query.words().len() == 1 && kw.text().split(' ').any(|tok| tok == query.text()) {
            take(&mut best, word);
        } else if kw.contains_all_words(query) {
            take(&mut best, kw_match - 100);
        }
    }

    // Multi-word query spanning label + keywords (e.g. "backup export", "wine proton").
    if best.is_none_or(|b| b < kw_match)
        && let Some(combined) = &folded.combined
    {
        if let Some((tier, _)) = combined.matches(query) {
            match tier {
                MatchTier::Exact | MatchTier::Prefix | MatchTier::Word => take(&mut best, kw_match),
                MatchTier::Fuzzy => {}
            }
        } else if combined.contains_all_words(query) {
            take(&mut best, kw_match - 150);
        }
    }

    // Setting help text, only for 3+ char queries and strictly below title/keyword hits.
    if kind == EntryKind::Setting
        && best.is_none()
        && query.text().chars().count() >= MIN_HELP_QUERY_CHARS
        && let Some(help) = &folded.help
        && help.contains_all_words(query)
    {
        take(&mut best, 2400);
    }

    best
}

/// One scored group waiting to be ordered and capped into [`ShownGroup`].
pub(super) struct ScoredGroup {
    pub(super) kind: GroupKind,
    pub(super) heading: SharedString,
    /// `(score, row)` pairs.
    pub(super) rows: Vec<(i64, Row)>,
}

/// One rendered group in the palette list.
#[derive(Clone)]
pub(super) struct ShownGroup {
    #[cfg(test)]
    pub(super) kind: GroupKind,
    pub(super) heading: SharedString,
    pub(super) rows: Vec<Row>,
}

/// Host-aware tie-breaker added to a group's top score when ordering groups.
fn group_boost(host: Host, kind: GroupKind) -> i64 {
    match (host, kind) {
        (_, GroupKind::Live) => i64::MAX / 2,
        (Host::Dialog, GroupKind::Pages) => 250,
        (Host::Dialog, GroupKind::Settings) => 150,
        (Host::Dialog, GroupKind::CurrentGame) => 100,
        (Host::Dialog, GroupKind::Games | GroupKind::Commands) => 0,
        (Host::Window, GroupKind::Games) => 400,
        (Host::Window, GroupKind::Pages | GroupKind::Settings | GroupKind::CurrentGame) => 0,
        (Host::Window, GroupKind::Commands) => -100,
        _ => 0,
    }
}

/// Deterministic tie-breaker when two groups reach the exact same boosted score.
fn group_tiebreak(host: Host, kind: GroupKind) -> u8 {
    match host {
        Host::Dialog => match kind {
            GroupKind::Live => 0,
            GroupKind::Pages => 1,
            GroupKind::Settings => 2,
            GroupKind::CurrentGame => 3,
            GroupKind::Games => 4,
            GroupKind::Commands => 5,
            GroupKind::Recent => 6,
            GroupKind::QuickActions => 7,
        },
        Host::Window => match kind {
            GroupKind::Live => 0,
            GroupKind::Games => 1,
            GroupKind::Recent => 2,
            GroupKind::Pages => 3,
            GroupKind::Settings => 4,
            GroupKind::CurrentGame => 5,
            GroupKind::Commands => 6,
            GroupKind::QuickActions => 7,
        },
    }
}

/// Maximum rows kept per group: compact when multiple groups compete, generous
/// when only one group matched (or when scoped by prefix).
fn group_cap(host: Host, kind: GroupKind, sole_group: bool) -> usize {
    if sole_group {
        return match kind {
            GroupKind::Live => usize::MAX,
            GroupKind::Pages => 15,
            GroupKind::Settings => 20,
            GroupKind::Games => 25,
            GroupKind::CurrentGame => 20,
            GroupKind::Commands => 20,
            GroupKind::Recent | GroupKind::QuickActions => 10,
        };
    }
    match kind {
        GroupKind::Live => usize::MAX,
        GroupKind::Pages => 6,
        GroupKind::Settings => 6,
        GroupKind::Games => match host {
            Host::Dialog => 8,
            Host::Window => 12,
        },
        GroupKind::CurrentGame => 5,
        GroupKind::Commands => 6,
        GroupKind::Recent | GroupKind::QuickActions => 6,
    }
}

/// Sort rows inside each group by descending score, order groups dynamically
/// by their best match + host boost, and apply per-group caps.
pub(super) fn rank_groups(host: Host, mut groups: Vec<ScoredGroup>) -> Vec<ShownGroup> {
    groups.retain(|g| !g.rows.is_empty());
    let sole_group = groups.len() <= 1;

    let mut ranked: Vec<(i64, u8, ShownGroup)> = groups
        .into_iter()
        .map(|mut group| {
            // Stable sort keeps registration/library order between equal scores.
            group.rows.sort_by_key(|(score, _)| Reverse(*score));
            let best = group.rows.first().map(|(s, _)| *s).unwrap_or(0);
            let group_score = best.saturating_add(group_boost(host, group.kind));
            let cap = group_cap(host, group.kind, sole_group);
            let rows: Vec<Row> = group.rows.into_iter().take(cap).map(|(_, r)| r).collect();
            (
                group_score,
                group_tiebreak(host, group.kind),
                ShownGroup {
                    #[cfg(test)]
                    kind: group.kind,
                    heading: group.heading,
                    rows,
                },
            )
        })
        .collect();

    ranked.sort_by_key(|(score, tb, _)| (Reverse(*score), *tb));
    ranked.into_iter().map(|(_, _, g)| g).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_optional_scope_prefixes() {
        assert_eq!(
            parse_input("  ,tema "),
            ParsedInput {
                scope: ScopeFilter::Settings,
                text: "tema",
                full: ",tema",
            }
        );
        assert_eq!(
            parse_input(">refresh"),
            ParsedInput {
                scope: ScopeFilter::Commands,
                text: "refresh",
                full: ">refresh",
            }
        );
        assert_eq!(
            parse_input("#rpg"),
            ParsedInput {
                scope: ScopeFilter::Categories,
                text: "rpg",
                full: "#rpg",
            }
        );
        assert_eq!(
            parse_input("Jogar Hades"),
            ParsedInput {
                scope: ScopeFilter::PlayIntent,
                text: "Hades",
                full: "Jogar Hades",
            }
        );
        assert_eq!(
            parse_input("play hades"),
            ParsedInput {
                scope: ScopeFilter::PlayIntent,
                text: "hades",
                full: "play hades",
            }
        );
        // Bare verb without a target stays a normal query.
        assert_eq!(parse_input("play").scope, ScopeFilter::All);
    }

    #[test]
    fn scores_accents_bilingual_keywords_and_help_threshold() {
        let appearance = FoldedEntry::build(
            "Aparência",
            Some("Appearance"),
            &[SharedString::new_static("theme colors")],
            None,
            None,
        );
        let q_no_accent = Query::new("aparencia").unwrap();
        let q_english = Query::new("appearance").unwrap();
        let q_typo = Query::new("aparencai").unwrap();
        assert_eq!(
            score_entry(EntryKind::Page, &appearance, &q_no_accent),
            Some(6200)
        );
        assert_eq!(
            score_entry(EntryKind::Page, &appearance, &q_english),
            Some(6200)
        );
        assert_eq!(
            score_entry(EntryKind::Page, &appearance, &q_typo),
            Some(2000)
        );

        let setting = FoldedEntry::build(
            "Atalho do iniciador rápido",
            Some("Quick launch shortcut"),
            &[SharedString::new_static("hotkey")],
            Some("Usa o portal do desktop para registrar o atalho."),
            Some("Uses the desktop portal to register the shortcut."),
        );
        // 2-char query never matches help text.
        assert_eq!(
            score_entry(EntryKind::Setting, &setting, &Query::new("po").unwrap()),
            None
        );
        // 3+ char query matches help text strictly below a game title match.
        let help_score =
            score_entry(EntryKind::Setting, &setting, &Query::new("portal").unwrap()).unwrap();
        assert_eq!(help_score, 2400);
        assert!(help_score < 4000);
    }
}
