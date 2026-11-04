//! Words and numbers of the stats page in the interface language: play
//! time, counts with their plural, period names and dates.

use chrono::{Datelike as _, NaiveDate};
use gpui_kit::SharedString;
use irori_ui::i18n::{t, t_args};

use super::period::{Period, Span, Unit};

const MONTHS: [&str; 12] = [
    "library.stats.month.january",
    "library.stats.month.february",
    "library.stats.month.march",
    "library.stats.month.april",
    "library.stats.month.may",
    "library.stats.month.june",
    "library.stats.month.july",
    "library.stats.month.august",
    "library.stats.month.september",
    "library.stats.month.october",
    "library.stats.month.november",
    "library.stats.month.december",
];

const MONTHS_SHORT: [&str; 12] = [
    "library.stats.month_short.january",
    "library.stats.month_short.february",
    "library.stats.month_short.march",
    "library.stats.month_short.april",
    "library.stats.month_short.may",
    "library.stats.month_short.june",
    "library.stats.month_short.july",
    "library.stats.month_short.august",
    "library.stats.month_short.september",
    "library.stats.month_short.october",
    "library.stats.month_short.november",
    "library.stats.month_short.december",
];

const WEEKDAYS_SHORT: [&str; 7] = [
    "library.stats.weekday_short.monday",
    "library.stats.weekday_short.tuesday",
    "library.stats.weekday_short.wednesday",
    "library.stats.weekday_short.thursday",
    "library.stats.weekday_short.friday",
    "library.stats.weekday_short.saturday",
    "library.stats.weekday_short.sunday",
];

/// "45 min", "2 h", "2 h 5 min"; "0 min" for nothing.
pub fn duration(seconds: u64) -> SharedString {
    crate::i18n::playtime(seconds / 60).into()
}

/// Whole hours, for big figures: "490 h"; under an hour, minutes.
pub fn hours(minutes: u64) -> SharedString {
    if minutes < 60 {
        return crate::i18n::playtime(minutes).into();
    }
    t_args(
        "library.playtime.hours",
        &[("hours", &(minutes / 60).to_string())],
    )
}

/// A count with its noun, from the `one` or `other` form.
pub fn count(one: &str, other: &str, n: impl Into<u64>) -> SharedString {
    let n = n.into();
    t_args(
        if n == 1 { one } else { other },
        &[("count", &n.to_string())],
    )
}

/// The noun alone for a count shown apart from it ("games beaten" next
/// to a big "9"), from the `one` or `other` form.
pub fn count_word(one: &str, other: &str, n: u64) -> SharedString {
    t(if n == 1 { one } else { other })
}

/// "October" (standalone, as in a heading).
pub fn month(date: NaiveDate) -> SharedString {
    t(MONTHS[date.month0() as usize])
}

/// "Oct".
pub fn month_short(date: NaiveDate) -> SharedString {
    t(MONTHS_SHORT[date.month0() as usize])
}

/// "Mon".
pub fn weekday_short(date: NaiveDate) -> SharedString {
    t(WEEKDAYS_SHORT[date.weekday().num_days_from_monday() as usize])
}

/// A day and month in the interface language's order ("10/19", "19.10.").
pub fn short_date(date: NaiveDate) -> SharedString {
    t_args(
        "library.stats.short_date",
        &[
            ("day", &date.day().to_string()),
            ("month", &date.month().to_string()),
        ],
    )
}

/// "Oct 19, 2026" in the interface language's order.
pub fn long_date(day: NaiveDate) -> SharedString {
    t_args(
        "library.stats.long_date",
        &[
            ("day", &day.day().to_string()),
            ("month", &month_short(day)),
            ("year", &day.year().to_string()),
        ],
    )
}

/// "October 2026".
pub fn month_year(date: NaiveDate) -> SharedString {
    t_args(
        "library.stats.month_year",
        &[("month", &month(date)), ("year", &date.year().to_string())],
    )
}

/// Name of a period: "Week of 10/26", "October 2026", "2026", "All time".
pub fn period(period: &Period) -> SharedString {
    match period.span {
        Span::Week => t_args(
            "library.stats.week_of",
            &[("date", &short_date(period.start))],
        ),
        Span::Month => month_year(period.start),
        Span::Year => period.start.year().to_string().into(),
        Span::All => t("library.stats.span.all"),
    }
}

/// Name of a span's pill.
pub fn span(span: Span) -> SharedString {
    t(match span {
        Span::Week => "library.stats.span.week",
        Span::Month => "library.stats.span.month",
        Span::Year => "library.stats.span.year",
        Span::All => "library.stats.span.all",
    })
}

/// The label under a chart bar.
pub fn bar_label(unit: Unit, start: NaiveDate, span: Span) -> SharedString {
    match (unit, span) {
        (Unit::Day, Span::Week) => weekday_short(start),
        (Unit::Day, _) => start.day().to_string().into(),
        (Unit::Month, _) => month_short(start),
        (Unit::Year, _) => start.year().to_string().into(),
    }
}

/// A bar's full name, for its tooltip and the busiest one: "10/18",
/// "October 2026", "2026".
pub fn bar_name(unit: Unit, start: NaiveDate) -> SharedString {
    match unit {
        Unit::Day => short_date(start),
        Unit::Month => month_year(start),
        Unit::Year => start.year().to_string().into(),
    }
}

/// Value axis ticks in hours: "2 h", "1.5 h".
pub fn hours_tick(hours: f64) -> SharedString {
    let value = if hours.fract() == 0.0 {
        format!("{hours:.0}")
    } else {
        format!("{hours:.1}")
    };
    t_args("library.playtime.hours", &[("hours", &value)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_periods_and_counts() {
        crate::i18n::register();
        let day = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        assert_eq!(
            period(&Period::containing(Span::Month, day, None)),
            "October 2026"
        );
        assert_eq!(
            period(&Period::containing(Span::Week, day, None)),
            "Week of 10/26"
        );
        assert_eq!(period(&Period::containing(Span::Year, day, None)), "2026");
        assert_eq!(bar_label(Unit::Day, day, Span::Week), "Mon");
        assert_eq!(bar_label(Unit::Month, day, Span::Year), "Oct");
        assert_eq!(
            count(
                "library.stats.sessions_one",
                "library.stats.sessions_other",
                1u32
            ),
            "1 session"
        );
        assert_eq!(
            count(
                "library.stats.sessions_one",
                "library.stats.sessions_other",
                61u32
            ),
            "61 sessions"
        );
        assert_eq!(hours(29_400), "490 h");
        assert_eq!(duration(5 * 3600 + 120), "5 h 2 min");
    }
}
