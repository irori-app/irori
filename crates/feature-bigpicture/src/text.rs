//! Localized text that is computed rather than looked up: status labels,
//! store names, play time, percentages, dates, the clock, and store
//! descriptions as plain text.

use chrono::NaiveDate;
use gpui_kit::SharedString;
use irori_core::{Runner, StatusKind};
use irori_ui::i18n::{t, t_args};

/// Present-progressive label of a game status ("Installing").
pub fn status_label(kind: StatusKind) -> SharedString {
    t(match kind {
        StatusKind::Installing => "bigpicture.status.installing",
        StatusKind::Importing => "bigpicture.status.importing",
        StatusKind::Updating => "bigpicture.status.updating",
        StatusKind::Launching => "bigpicture.status.launching",
        StatusKind::Playing => "bigpicture.status.playing",
        StatusKind::Uninstalling => "bigpicture.status.uninstalling",
        StatusKind::Repairing => "bigpicture.status.verifying",
        StatusKind::Moving => "bigpicture.status.moving",
        StatusKind::Queued => "bigpicture.status.queued",
        StatusKind::SyncingSaves => "bigpicture.status.syncing_saves",
        StatusKind::Redist => "bigpicture.status.installing_dependencies",
        StatusKind::Extracting => "bigpicture.status.extracting",
        StatusKind::Winetricks => "bigpicture.status.winetricks",
        StatusKind::Done => "bigpicture.status.done",
        StatusKind::Canceled => "bigpicture.status.canceled",
        StatusKind::Error => "bigpicture.status.error",
    })
}

/// A status label with its progress ("Installing · 42%").
pub fn status_with_percent(kind: StatusKind, percent: Option<f32>) -> SharedString {
    let label = status_label(kind);
    match percent {
        Some(p) => t_args(
            "bigpicture.status.with_percent",
            &[
                ("status", label.as_str()),
                ("percent", format!("{p:.0}").as_str()),
            ],
        ),
        None => label,
    }
}

/// Display name of a store. Store names are brands and stay as they are;
/// manually added apps get a translated name.
pub fn store_name(runner: Runner) -> SharedString {
    match runner {
        Runner::Sideload => t("bigpicture.store.sideload"),
        other => SharedString::new_static(other.store_name()),
    }
}

/// Play time in hours and minutes ("3 h 12 min"), "< 1 min" for less.
pub fn playtime(minutes: u64) -> SharedString {
    let (h, m) = ((minutes / 60).to_string(), (minutes % 60).to_string());
    match (minutes / 60, minutes % 60) {
        (0, 0) => t("bigpicture.playtime.under_minute"),
        (0, _) => t_args("bigpicture.playtime.minutes", &[("minutes", m.as_str())]),
        (_, 0) => t_args("bigpicture.playtime.hours", &[("hours", h.as_str())]),
        _ => t_args(
            "bigpicture.playtime.hours_minutes",
            &[("hours", h.as_str()), ("minutes", m.as_str())],
        ),
    }
}

/// When something happened, by calendar day: "Today", "Yesterday", "3 days
/// ago" within the week, else the date (ISO 8601, which reads the same in
/// every language).
pub fn relative_day(then: NaiveDate, today: NaiveDate) -> SharedString {
    match (today - then).num_days() {
        ..=0 => t("bigpicture.when.today"),
        1 => t("bigpicture.when.yesterday"),
        days @ 2..=6 => t_args(
            "bigpicture.when.days_ago",
            &[("count", days.to_string().as_str())],
        ),
        _ => then.format("%Y-%m-%d").to_string().into(),
    }
}

/// [`relative_day`] for an RFC 3339 timestamp, in local time.
pub fn last_played(rfc3339: &str) -> Option<SharedString> {
    let then = chrono::DateTime::parse_from_rfc3339(rfc3339).ok()?;
    let local = then.with_timezone(&chrono::Local).date_naive();
    Some(relative_day(local, chrono::Local::now().date_naive()))
}

/// The game's short description as plain text, else its full one.
pub fn description(game: &irori_core::GameInfo) -> Option<String> {
    let extra = game.extra.as_ref();
    [
        extra.and_then(|e| e.short_description.as_deref()),
        game.description.as_deref(),
        extra.and_then(|e| e.description.as_deref()),
    ]
    .into_iter()
    .flatten()
    .map(plain_text)
    .find(|t| !t.is_empty())
}

/// A store description as plain text: tags dropped (block tags become
/// spaces), common entities decoded and whitespace collapsed.
pub fn plain_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How a language usually writes the time of day, following the CLDR short
/// time format of each interface language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HourCycle {
    /// 1–12 with a day period ("3:05 PM", "오후 3:05", "下午3:05").
    H12,
    /// 0–23; `pad` writes a leading zero ("09:05" rather than "9:05").
    H23 { pad: bool },
}

/// The usual clock of an interface language code.
///
/// English (Irori's English follows US/Canadian/Australian usage), Korean and
/// Traditional Chinese (Taiwan, Hong Kong) commonly use a 12-hour clock with a
/// day period. Every other shipped language uses 24 hours; Czech, Spanish and
/// Japanese write the hour without a leading zero.
pub fn hour_cycle(language: &str) -> HourCycle {
    match language {
        "en" | "ko" | "zh_Hant" => HourCycle::H12,
        "cs" | "es" | "ja" => HourCycle::H23 { pad: false },
        _ => HourCycle::H23 { pad: true },
    }
}

/// The digits of a clock reading and, on a 12-hour clock, whether it is
/// afternoon (`Some(true)` for PM).
pub fn clock_parts(hour: u32, minute: u32, cycle: HourCycle) -> (String, Option<bool>) {
    match cycle {
        HourCycle::H12 => {
            let h12 = match hour % 12 {
                0 => 12,
                h => h,
            };
            (format!("{h12}:{minute:02}"), Some(hour >= 12))
        }
        HourCycle::H23 { pad: true } => (format!("{hour:02}:{minute:02}"), None),
        HourCycle::H23 { pad: false } => (format!("{hour}:{minute:02}"), None),
    }
}

/// Clock text for `hour:minute` in the active interface language.
pub fn clock(hour: u32, minute: u32) -> SharedString {
    let (time, pm) = clock_parts(hour, minute, hour_cycle(&irori_ui::i18n::language()));
    match pm {
        None => time.into(),
        Some(pm) => {
            let period = t(if pm {
                "bigpicture.clock.pm"
            } else {
                "bigpicture.clock.am"
            });
            t_args(
                "bigpicture.clock.time_12h",
                &[("time", time.as_str()), ("period", period.as_str())],
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_read_relative_within_a_week() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        let day = |d: u32| NaiveDate::from_ymd_opt(2026, 10, d).unwrap();
        crate::register_catalogs();
        assert_eq!(relative_day(today, today), t("bigpicture.when.today"));
        assert_eq!(
            relative_day(NaiveDate::from_ymd_opt(2026, 11, 2).unwrap(), today),
            t("bigpicture.when.yesterday")
        );
        assert_eq!(
            relative_day(day(30), today),
            t_args("bigpicture.when.days_ago", &[("count", "4")])
        );
        assert_eq!(relative_day(day(1), today).as_str(), "2026-10-01");
        assert!(last_played("not a date").is_none());
    }

    #[test]
    fn descriptions_become_plain_text() {
        assert_eq!(
            plain_text("<p>Fight &amp; explore.</p><br/>Tom&#39;s  <b>quest</b>\n"),
            "Fight & explore. Tom's quest"
        );
        assert_eq!(plain_text("Plain"), "Plain");
    }

    #[test]
    fn clock_follows_the_language() {
        assert_eq!(hour_cycle("en"), HourCycle::H12);
        assert_eq!(hour_cycle("ko"), HourCycle::H12);
        assert_eq!(hour_cycle("de"), HourCycle::H23 { pad: true });
        assert_eq!(hour_cycle("ja"), HourCycle::H23 { pad: false });

        assert_eq!(
            clock_parts(0, 5, HourCycle::H12),
            ("12:05".into(), Some(false))
        );
        assert_eq!(
            clock_parts(12, 0, HourCycle::H12),
            ("12:00".into(), Some(true))
        );
        assert_eq!(
            clock_parts(15, 7, HourCycle::H12),
            ("3:07".into(), Some(true))
        );
        assert_eq!(
            clock_parts(9, 5, HourCycle::H23 { pad: true }),
            ("09:05".into(), None)
        );
        assert_eq!(
            clock_parts(9, 5, HourCycle::H23 { pad: false }),
            ("9:05".into(), None)
        );
        assert_eq!(
            clock_parts(23, 59, HourCycle::H23 { pad: true }),
            ("23:59".into(), None)
        );
    }
}
