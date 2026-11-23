//! What the Wine manager's release checks may spend. GitHub allows 60 API
//! calls an hour to a computer that isn't signed in, shared with every other
//! program on it. A check reads the source's release feed first, which isn't
//! counted, and asks the API only when the feed changed. The API's
//! `x-ratelimit-*` answers are kept in `state/wine-release-checks.json`, so
//! checks Irori starts on its own leave a reserve for the user's, and a
//! used-up budget is reported with the time it resets.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use irori_core::Paths;
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};

/// API calls automatic checks leave for the ones the user asks for.
pub const RESERVE: u32 = 10;
/// Feed entries the fingerprint covers: enough to see a new or edited
/// release, few enough that a long history doesn't matter.
const FINGERPRINT_ENTRIES: usize = 10;

/// Who asked for a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckKind {
    /// The user (Refresh, an install): may use the whole budget.
    Manual,
    /// Irori on its own (a page opening, the daily update check): keeps
    /// [`RESERVE`] calls.
    Automatic,
}

/// GitHub's API budget as its last answer gave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    pub limit: u32,
    pub remaining: u32,
    /// Unix time the budget refills.
    pub reset: u64,
}

/// What the checks remember between runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckState {
    pub github: Option<Budget>,
    /// The release feed's fingerprint per source type when its list was last
    /// fetched.
    pub feeds: BTreeMap<String, String>,
}

/// GitHub refused a check, or would: its API budget is used up until
/// `reset` (unix time).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimited {
    pub reset: u64,
}

impl RateLimited {
    /// Whole minutes until the budget refills, at least one.
    pub fn minutes_left(&self, now: u64) -> u64 {
        self.reset.saturating_sub(now).div_ceil(60).max(1)
    }
}

impl fmt::Display for RateLimited {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let minutes = self.minutes_left(irori_core::util::unix_now());
        write!(
            f,
            "GitHub's limit for release checks is used up; it resets in {minutes} min"
        )
    }
}

impl std::error::Error for RateLimited {}

/// The rate limit in `err`'s chain, if that is why it failed.
pub fn rate_limited(err: &anyhow::Error) -> Option<RateLimited> {
    err.chain()
        .find_map(|e| e.downcast_ref::<RateLimited>().copied())
}

fn state_file() -> PathBuf {
    Paths::get().state_dir().join("wine-release-checks.json")
}

pub(crate) fn read_state() -> CheckState {
    irori_core::json_store::read_or_default(&state_file())
}

pub(crate) fn write_state(state: &CheckState) {
    if let Err(e) = irori_core::json_store::write(&state_file(), state) {
        tracing::warn!("saving the release check state: {e:#}");
    }
}

/// When GitHub's budget is used up, the time it refills. No network.
pub fn exhausted_until(now: u64) -> Option<u64> {
    read_state()
        .github
        .filter(|b| b.remaining == 0 && b.reset > now)
        .map(|b| b.reset)
}

/// Whether `api_url` is GitHub's API, whose calls are counted.
pub fn is_github(api_url: &str) -> bool {
    api_url.starts_with("https://api.github.com/")
}

/// The Atom feed of a releases API address: GitHub's
/// `https://github.com/<owner>/<repo>/releases.atom`, or a Forgejo's
/// `https://<host>/<owner>/<repo>/releases.atom`.
pub fn feed_url(api_url: &str) -> Option<String> {
    static API: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^https://([^/]+)/(?:api/v1/)?repos/([^/?#]+)/([^/?#]+)/releases/?(?:\?.*)?$")
            .expect("valid regex")
    });
    let c = API.captures(api_url)?;
    let host = match &c[1] {
        "api.github.com" => "github.com",
        other => other,
    };
    Some(format!("https://{host}/{}/{}/releases.atom", &c[2], &c[3]))
}

/// What identifies a feed's newest releases: the id and update time of its
/// first entries. A new release, or an edited one, changes it.
pub fn feed_fingerprint(xml: &str) -> Option<String> {
    static ENTRY: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?s)<entry\b.*?</entry>").expect("valid regex"));
    static ID: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?s)<id>\s*(.*?)\s*</id>").expect("valid regex"));
    static UPDATED: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?s)<updated>\s*(.*?)\s*</updated>").expect("valid regex"));
    if !xml.contains("<feed") {
        return None;
    }
    let parts: Vec<String> = ENTRY
        .find_iter(xml)
        .take(FINGERPRINT_ENTRIES)
        .map(|entry| {
            let entry = entry.as_str();
            let field = |re: &Regex| {
                re.captures(entry)
                    .map(|c| c[1].to_string())
                    .unwrap_or_default()
            };
            format!("{}@{}", field(&ID), field(&UPDATED))
        })
        .collect();
    // A feed without releases says nothing a list couldn't; ask the API.
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// GitHub's budget from an API answer's headers.
pub fn budget_from_headers(headers: &HeaderMap) -> Option<Budget> {
    let number =
        |name: &str| -> Option<u64> { headers.get(name)?.to_str().ok()?.trim().parse().ok() };
    Some(Budget {
        limit: u32::try_from(number("x-ratelimit-limit")?).ok()?,
        remaining: u32::try_from(number("x-ratelimit-remaining")?).ok()?,
        reset: number("x-ratelimit-reset")?,
    })
}

/// Whether a check may call GitHub's API with `budget` left at `now`. A
/// budget whose reset time has passed is full again.
pub fn may_call_api(budget: Option<Budget>, now: u64, check: CheckKind) -> Result<(), RateLimited> {
    let Some(b) = budget.filter(|b| b.reset > now) else {
        return Ok(());
    };
    let floor = match check {
        CheckKind::Manual => 1,
        CheckKind::Automatic => RESERVE,
    };
    if b.remaining < floor {
        return Err(RateLimited { reset: b.reset });
    }
    Ok(())
}

/// Whether a refused API call (`status`) was GitHub's rate limit: a 403 or
/// 429 with no calls left, or with a `retry-after` (its secondary limit).
pub fn refusal(status: u16, headers: &HeaderMap, now: u64) -> Option<RateLimited> {
    if status != 403 && status != 429 {
        return None;
    }
    let retry_after = headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    if let Some(secs) = retry_after {
        return Some(RateLimited { reset: now + secs });
    }
    budget_from_headers(headers)
        .filter(|b| b.remaining == 0)
        .map(|b| RateLimited {
            reset: b.reset.max(now),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn feeds_of_release_lists() {
        assert_eq!(
            feed_url("https://api.github.com/repos/GloriousEggroll/proton-ge-custom/releases")
                .as_deref(),
            Some("https://github.com/GloriousEggroll/proton-ge-custom/releases.atom")
        );
        assert_eq!(
            feed_url("https://dawn.wine/api/v1/repos/dawn-winery/dwproton/releases?limit=50")
                .as_deref(),
            Some("https://dawn.wine/dawn-winery/dwproton/releases.atom")
        );
        assert_eq!(feed_url("https://example.com/releases.json"), None);
        assert!(is_github("https://api.github.com/repos/a/b/releases"));
        assert!(!is_github("https://dawn.wine/api/v1/repos/a/b/releases"));
    }

    const FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <id>tag:github.com,2008:https://github.com/o/r/releases</id>
  <updated>2026-11-13T15:14:59Z</updated>
  <entry>
    <id>tag:github.com,2008:Repository/1/GE-Proton10-25</id>
    <updated>2026-11-13T15:14:59Z</updated>
    <title>GE-Proton10-25</title>
  </entry>
  <entry>
    <id>tag:github.com,2008:Repository/1/GE-Proton10-24</id>
    <updated>2026-10-31T15:14:40Z</updated>
    <title>GE-Proton10-24</title>
  </entry>
</feed>"#;

    #[test]
    fn a_new_or_edited_release_changes_the_fingerprint() {
        let base = feed_fingerprint(FEED).unwrap();
        assert!(base.starts_with("tag:github.com,2008:Repository/1/GE-Proton10-25@2026-11-13"));
        // The feed's own update time isn't a release.
        let touched = FEED.replacen(
            "<updated>2026-11-13T15:14:59Z</updated>\n  <entry>",
            "<updated>2026-11-18T00:00:00Z</updated>\n  <entry>",
            1,
        );
        assert_eq!(feed_fingerprint(&touched).as_deref(), Some(base.as_str()));
        let edited = FEED.replace("2026-10-31T15:14:40Z", "2026-11-01T00:00:00Z");
        assert_ne!(feed_fingerprint(&edited).unwrap(), base);
        let newer = FEED.replacen(
            "<entry>",
            "<entry><id>x/GE-Proton10-26</id><updated>2026-11-18T00:00:00Z</updated></entry>\n  <entry>",
            1,
        );
        assert_ne!(feed_fingerprint(&newer).unwrap(), base);
        // An error page or an empty feed says nothing.
        assert_eq!(feed_fingerprint("<html>rate limited</html>"), None);
        assert_eq!(feed_fingerprint("<feed></feed>"), None);
    }

    #[test]
    fn the_budget_comes_from_the_headers() {
        let h = headers(&[
            ("x-ratelimit-limit", "60"),
            ("x-ratelimit-remaining", "7"),
            ("x-ratelimit-reset", "1800000000"),
        ]);
        assert_eq!(
            budget_from_headers(&h),
            Some(Budget {
                limit: 60,
                remaining: 7,
                reset: 1_800_000_000
            })
        );
        assert_eq!(
            budget_from_headers(&headers(&[("x-ratelimit-limit", "60")])),
            None
        );
    }

    #[test]
    fn automatic_checks_keep_a_reserve() {
        let now = 1_000;
        let budget = |remaining| {
            Some(Budget {
                limit: 60,
                remaining,
                reset: now + 600,
            })
        };
        assert!(may_call_api(None, now, CheckKind::Automatic).is_ok());
        assert!(may_call_api(budget(RESERVE), now, CheckKind::Automatic).is_ok());
        assert_eq!(
            may_call_api(budget(RESERVE - 1), now, CheckKind::Automatic),
            Err(RateLimited { reset: now + 600 })
        );
        assert!(may_call_api(budget(1), now, CheckKind::Manual).is_ok());
        assert!(may_call_api(budget(0), now, CheckKind::Manual).is_err());
        // Past the reset time the budget is full again.
        let spent = Some(Budget {
            limit: 60,
            remaining: 0,
            reset: now - 1,
        });
        assert!(may_call_api(spent, now, CheckKind::Automatic).is_ok());
    }

    #[test]
    fn refusals_say_when_to_try_again() {
        let now = 1_000;
        let spent = headers(&[
            ("x-ratelimit-limit", "60"),
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset", "4600"),
        ]);
        assert_eq!(
            refusal(403, &spent, now),
            Some(RateLimited { reset: 4_600 })
        );
        assert_eq!(
            refusal(429, &headers(&[("retry-after", "120")]), now),
            Some(RateLimited { reset: 1_120 })
        );
        // A 403 with calls left is something else (a blocked repository).
        let left = headers(&[
            ("x-ratelimit-limit", "60"),
            ("x-ratelimit-remaining", "12"),
            ("x-ratelimit-reset", "4600"),
        ]);
        assert_eq!(refusal(403, &left, now), None);
        assert_eq!(refusal(404, &spent, now), None);
        let limited = RateLimited { reset: 4_600 };
        assert_eq!(limited.minutes_left(now), 60);
        assert_eq!(limited.minutes_left(4_590), 1);
        let err = anyhow::Error::new(limited).context("could not fetch GE-Proton releases");
        assert_eq!(rate_limited(&err), Some(limited));
    }

    #[test]
    fn the_state_reads_what_it_wrote() {
        let state = CheckState {
            github: Some(Budget {
                limit: 60,
                remaining: 3,
                reset: 9,
            }),
            feeds: [("GE-Proton".to_string(), "fp".to_string())].into(),
        };
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(serde_json::from_str::<CheckState>(&json).unwrap(), state);
        assert_eq!(
            serde_json::from_str::<CheckState>("{}").unwrap(),
            CheckState::default()
        );
    }
}
