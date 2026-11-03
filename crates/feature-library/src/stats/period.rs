//! The stretch of time the stats page looks at: a calendar week (Monday to
//! Sunday), month or year, or all time, and how to step between them.

use chrono::{Datelike as _, Days, Months, NaiveDate};

/// How long a period is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Span {
    Week,
    Month,
    Year,
    All,
}

impl Span {
    pub const ALL: [Span; 4] = [Span::Week, Span::Month, Span::Year, Span::All];
}

/// What one bar of the period's chart covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Day,
    Month,
    Year,
}

impl Unit {
    /// Start of the bar holding `day`.
    pub fn start_of(self, day: NaiveDate) -> NaiveDate {
        match self {
            Unit::Day => day,
            Unit::Month => day.with_day(1).unwrap_or(day),
            Unit::Year => NaiveDate::from_ymd_opt(day.year(), 1, 1).unwrap_or(day),
        }
    }

    /// Start of the bar after the one starting at `start`.
    pub fn next(self, start: NaiveDate) -> NaiveDate {
        match self {
            Unit::Day => start + Days::new(1),
            Unit::Month => start + Months::new(1),
            Unit::Year => start + Months::new(12),
        }
    }
}

/// One period. `start` is its first day; all time starts on the first day
/// with a recorded session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Period {
    pub span: Span,
    pub start: NaiveDate,
}

/// All time spans longer than this many months are charted by year.
const ALL_TIME_MONTHS: i32 = 24;

impl Period {
    /// The period of `span` holding `day`. `first` is the first day of the
    /// history, where all time starts (`day` without one).
    pub fn containing(span: Span, day: NaiveDate, first: Option<NaiveDate>) -> Self {
        let start = match span {
            Span::Week => day - Days::new(u64::from(day.weekday().num_days_from_monday())),
            Span::Month => Unit::Month.start_of(day),
            Span::Year => Unit::Year.start_of(day),
            Span::All => first.unwrap_or(day).min(day),
        };
        Self { span, start }
    }

    /// The day after the period's last day. All time runs through `today`.
    pub fn end(&self, today: NaiveDate) -> NaiveDate {
        match self.span {
            Span::Week => self.start + Days::new(7),
            Span::Month => self.start + Months::new(1),
            Span::Year => self.start + Months::new(12),
            Span::All => today.succ_opt().unwrap_or(today),
        }
    }

    pub fn contains(&self, day: NaiveDate, today: NaiveDate) -> bool {
        (self.start..self.end(today)).contains(&day)
    }

    /// The period before this one; all time has none.
    pub fn previous(&self) -> Option<Self> {
        let start = match self.span {
            Span::Week => self.start - Days::new(7),
            Span::Month => self.start - Months::new(1),
            Span::Year => self.start - Months::new(12),
            Span::All => return None,
        };
        Some(Self { start, ..*self })
    }

    /// The period after this one; all time has none.
    pub fn next(&self, today: NaiveDate) -> Option<Self> {
        (self.span != Span::All).then(|| Self {
            start: self.end(today),
            ..*self
        })
    }

    /// Whether `today` falls in it, so it is still going on.
    pub fn is_current(&self, today: NaiveDate) -> bool {
        self.contains(today, today)
    }

    /// Days of the period that have begun: all of a past period, up to and
    /// including today for the current one, none of a future one.
    pub fn days_elapsed(&self, today: NaiveDate) -> u32 {
        let end = self.end(today).min(today.succ_opt().unwrap_or(today));
        (end - self.start).num_days().max(0) as u32
    }

    /// What one bar of its chart covers.
    pub fn unit(&self, today: NaiveDate) -> Unit {
        match self.span {
            Span::Week | Span::Month => Unit::Day,
            Span::Year => Unit::Month,
            Span::All => {
                let months = (today.year() - self.start.year()) * 12 + today.month() as i32
                    - self.start.month() as i32;
                if months >= ALL_TIME_MONTHS {
                    Unit::Year
                } else {
                    Unit::Month
                }
            }
        }
    }

    /// Start of each bar of its chart, in order.
    pub fn bars(&self, today: NaiveDate) -> Vec<NaiveDate> {
        let unit = self.unit(today);
        let end = self.end(today);
        let mut bars = Vec::new();
        let mut at = unit.start_of(self.start);
        while at < end {
            bars.push(at);
            at = unit.next(at);
        }
        bars
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn calendar_periods_and_stepping() {
        // A Tuesday.
        let today = day("2026-11-03");
        let week = Period::containing(Span::Week, today, None);
        assert_eq!(week.start, day("2026-11-02"));
        assert_eq!(week.end(today), day("2026-11-09"));
        assert_eq!(week.days_elapsed(today), 2);
        assert!(week.is_current(today));
        let before = week.previous().unwrap();
        assert_eq!(before.start, day("2026-10-26"));
        assert_eq!(before.days_elapsed(today), 7);
        assert_eq!(before.next(today), Some(week));

        let month = Period::containing(Span::Month, day("2026-03-31"), None);
        assert_eq!(month.start, day("2026-03-01"));
        assert_eq!(month.previous().unwrap().start, day("2026-02-01"));
        assert_eq!(month.previous().unwrap().bars(today).len(), 28);

        let year = Period::containing(Span::Year, today, None);
        assert_eq!(year.bars(today).len(), 12);
        assert_eq!(year.unit(today), Unit::Month);
        let next = year.next(today).unwrap();
        assert_eq!(next.start, day("2027-01-01"));
        assert_eq!(next.days_elapsed(today), 0);
    }

    #[test]
    fn all_time_starts_with_the_history_and_charts_long_ones_by_year() {
        let today = day("2026-11-03");
        let short = Period::containing(Span::All, today, Some(day("2026-05-20")));
        assert_eq!(short.start, day("2026-05-20"));
        assert_eq!(short.unit(today), Unit::Month);
        assert_eq!(short.bars(today).first(), Some(&day("2026-05-01")));
        assert_eq!(short.bars(today).len(), 7);
        assert_eq!(short.previous(), None);
        assert_eq!(short.next(today), None);
        assert!(short.contains(today, today));

        let long = Period::containing(Span::All, today, Some(day("2022-02-10")));
        assert_eq!(long.unit(today), Unit::Year);
        assert_eq!(long.bars(today).len(), 5);
        // Without a history it is just today.
        assert_eq!(Period::containing(Span::All, today, None).start, today);
    }
}
