//! Calendar dates in the interface language, shared by every page that
//! shows one (release dates, the Wine manager's builds, backups), so dates
//! read alike everywhere instead of as raw ISO strings.

use chrono::Datelike as _;
use gpui_kit::SharedString;

use crate::i18n::{t, t_args};

const MONTHS: [&str; 12] = [
    "ui.month.1",
    "ui.month.2",
    "ui.month.3",
    "ui.month.4",
    "ui.month.5",
    "ui.month.6",
    "ui.month.7",
    "ui.month.8",
    "ui.month.9",
    "ui.month.10",
    "ui.month.11",
    "ui.month.12",
];

/// The calendar date of a date as stores and release lists write it:
/// "2022-02-25", RFC 3339 or "2022-02-25T00:00:00.000Z", or Steam's English
/// "16 Nov, 2009" and "Nov 16, 2009".
pub fn parse_date(text: &str) -> Option<chrono::NaiveDate> {
    let text = text.trim();
    if let Some(d) = text
        .get(..10)
        .and_then(|t| chrono::NaiveDate::parse_from_str(t, "%Y-%m-%d").ok())
    {
        return Some(d);
    }
    ["%d %b, %Y", "%b %d, %Y", "%d %B, %Y", "%B %d, %Y"]
        .iter()
        .find_map(|f| chrono::NaiveDate::parse_from_str(text, f).ok())
}

/// A date in the interface language: "Oct 11", or "Oct 11, 2024" with the year.
pub fn calendar_date(date: chrono::NaiveDate, with_year: bool) -> String {
    let month = t(MONTHS[date.month0() as usize]);
    let day = date.day().to_string();
    if with_year {
        t_args(
            "ui.date.day_year",
            &[
                ("month", &month),
                ("day", &day),
                ("year", &date.year().to_string()),
            ],
        )
    } else {
        t_args("ui.date.day", &[("month", &month), ("day", &day)])
    }
    .to_string()
}

/// A date written by a store or a release list, in the interface language
/// with its year; text that isn't a date stays as it was written.
pub fn localized_date(text: &str) -> String {
    match parse_date(text) {
        Some(date) => calendar_date(date, true),
        None => text.trim().to_string(),
    }
}

/// The local calendar day of a moment written in RFC 3339 (a backup made,
/// a change allowed again), in the interface language; text that isn't
/// one goes through [`localized_date`].
pub fn localized_day(rfc3339: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(rfc3339.trim()) {
        Ok(time) => calendar_date(time.with_timezone(&chrono::Local).date_naive(), true),
        Err(_) => localized_date(rfc3339),
    }
}

/// `then` relative to `now` (both Unix seconds), phrased to fit inside a
/// sentence: "just now", "5 min ago", "3 h ago", "yesterday", "4 days ago".
/// Future times read as "just now".
pub fn relative_time(then: u64, now: u64) -> SharedString {
    let secs = now.saturating_sub(then);
    match secs {
        0..60 => t("ui.time.just_now"),
        60..3_600 => t_args(
            "ui.time.minutes_ago",
            &[("count", &(secs / 60).to_string())],
        ),
        3_600..86_400 => t_args(
            "ui.time.hours_ago",
            &[("count", &(secs / 3_600).to_string())],
        ),
        86_400..172_800 => t("ui.time.yesterday"),
        _ => t_args(
            "ui.time.days_ago",
            &[("count", &(secs / 86_400).to_string())],
        ),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone as _;

    use super::*;

    #[test]
    fn dates_in_store_formats() {
        let date = chrono::NaiveDate::from_ymd_opt(2009, 11, 16);
        for text in [
            "2009-11-16",
            "2009-11-16T00:00:00.000Z",
            "2009-11-16T08:12:00Z",
            "16 Nov, 2009",
            "Nov 16, 2009",
            "16 November, 2009",
        ] {
            assert_eq!(parse_date(text), date, "{text}");
        }
        assert_eq!(parse_date("Coming soon"), None);
    }

    #[test]
    fn dates_in_the_interface_language() {
        crate::i18n::register(crate::i18n::CATALOGS);
        assert_eq!(localized_date("2024-10-11T09:00:00Z"), "Oct 11, 2024");
        assert_eq!(localized_date(" soon "), "soon");
        let date = chrono::NaiveDate::from_ymd_opt(2024, 10, 11).unwrap();
        assert_eq!(calendar_date(date, false), "Oct 11");
        // Noon UTC is the same day in every time zone but the farthest.
        let noon = chrono::Utc
            .with_ymd_and_hms(2024, 10, 11, 12, 0, 0)
            .unwrap()
            .to_rfc3339();
        let local = chrono::DateTime::parse_from_rfc3339(&noon)
            .unwrap()
            .with_timezone(&chrono::Local)
            .date_naive();
        assert_eq!(localized_day(&noon), calendar_date(local, true));
        assert_eq!(localized_day("2024-10-11"), "Oct 11, 2024");
    }

    #[test]
    fn relative_times_in_english() {
        crate::i18n::register(crate::i18n::CATALOGS);
        let now = 1_000_000;
        assert_eq!(relative_time(now, now), "just now");
        assert_eq!(relative_time(now + 30, now), "just now");
        assert_eq!(relative_time(now - 125, now), "2 min ago");
        assert_eq!(relative_time(now - 7_200, now), "2 h ago");
        assert_eq!(relative_time(now - 90_000, now), "yesterday");
        assert_eq!(relative_time(now - 5 * 86_400, now), "5 days ago");
    }
}
