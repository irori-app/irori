//! Text formatting shared by the management screens.

use chrono::{NaiveDateTime, Timelike as _};
use gpui_kit::SharedString;
use irori_core::util::{format_bytes, format_duration};
use irori_ui::i18n::{t, t_args};

/// `then` relative to `now` (both unix seconds), phrased to also fit inside
/// a sentence: "just now", "5 min ago", "3 h ago", "yesterday", "4 days ago".
/// Future times read as "just now".
pub(crate) fn relative_time(then: u64, now: u64) -> SharedString {
    let secs = now.saturating_sub(then);
    match secs {
        0..60 => t("manage.time.just_now"),
        60..3_600 => t_args(
            "manage.time.minutes_ago",
            &[("count", &(secs / 60).to_string())],
        ),
        3_600..86_400 => t_args(
            "manage.time.hours_ago",
            &[("count", &(secs / 3_600).to_string())],
        ),
        86_400..172_800 => t("manage.time.yesterday"),
        _ => t_args(
            "manage.time.days_ago",
            &[("count", &(secs / 86_400).to_string())],
        ),
    }
}

/// What limits a download, or an em dash when nothing can be said.
pub(crate) fn bottleneck(b: Option<irori_core::Bottleneck>) -> SharedString {
    use irori_core::Bottleneck;
    match b {
        Some(Bottleneck::Network) => t("manage.downloads.limit.network"),
        Some(Bottleneck::Server) => t("manage.downloads.limit.server"),
        Some(Bottleneck::Disk) => t("manage.downloads.limit.disk"),
        Some(Bottleneck::SpeedLimit) => t("manage.downloads.limit.speed_limit"),
        Some(Bottleneck::Reconnecting) => t("manage.downloads.limit.reconnecting"),
        None => "—".into(),
    }
}

/// A transfer rate, or an em dash when unknown or zero.
pub(crate) fn rate(bytes_per_sec: Option<u64>) -> String {
    match bytes_per_sec.filter(|b| *b > 0) {
        Some(b) => format!("{}/s", format_bytes(b)),
        None => "—".to_string(),
    }
}

/// Remaining time, or an em dash when unknown.
pub(crate) fn eta(secs: Option<u64>) -> String {
    match secs.filter(|s| *s > 0) {
        Some(s) => format_duration(s),
        None => "—".to_string(),
    }
}

/// A size in bytes, or `None` when unknown (zero).
pub(crate) fn size(bytes: u64) -> Option<String> {
    (bytes > 0).then(|| format_bytes(bytes))
}

/// A time of day the way `language` usually writes it (the CLDR short time
/// format): a 12-hour clock with its day period in English, Korean and
/// Traditional Chinese, 24 hours elsewhere, without a leading zero in Czech,
/// Spanish and Japanese.
pub(crate) fn clock(hour: u32, minute: u32, language: &str) -> String {
    let h12 = match hour % 12 {
        0 => 12,
        h => h,
    };
    let pm = hour >= 12;
    match language {
        "en" => format!("{h12}:{minute:02} {}", if pm { "PM" } else { "AM" }),
        "ko" => format!("{} {h12}:{minute:02}", if pm { "오후" } else { "오전" }),
        "zh_Hant" => format!("{}{h12}:{minute:02}", if pm { "下午" } else { "上午" }),
        "cs" | "es" | "ja" => format!("{hour}:{minute:02}"),
        _ => format!("{hour:02}:{minute:02}"),
    }
}

/// When something `secs` from `now` (local time) is done, phrased to fit a
/// sentence: "around 9:39 PM", "tomorrow around 8:10 AM", "in about 3 days".
pub(crate) fn finish_when(now: NaiveDateTime, secs: u64, language: &str) -> SharedString {
    let at = now + chrono::Duration::seconds(i64::try_from(secs).unwrap_or(i64::MAX / 2));
    let time = clock(at.hour(), at.minute(), language);
    match (at.date() - now.date()).num_days() {
        0 => t_args("manage.downloads.eta.today", &[("time", &time)]),
        1 => t_args("manage.downloads.eta.tomorrow", &[("time", &time)]),
        days => t_args("manage.downloads.eta.days", &[("count", &days.to_string())]),
    }
}

/// [`finish_when`] from this moment, in the interface language.
pub(crate) fn finish_from_now(secs: u64) -> SharedString {
    finish_when(
        chrono::Local::now().naive_local(),
        secs,
        &irori_ui::i18n::language(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks_per_language() {
        assert_eq!(clock(21, 9, "en"), "9:09 PM");
        assert_eq!(clock(0, 5, "en"), "12:05 AM");
        assert_eq!(clock(12, 0, "ko"), "오후 12:00");
        assert_eq!(clock(9, 30, "zh_Hant"), "上午9:30");
        assert_eq!(clock(9, 30, "es"), "9:30");
        assert_eq!(clock(9, 30, "de"), "09:30");
        assert_eq!(clock(21, 30, "fr"), "21:30");
    }

    #[test]
    fn finish_today_tomorrow_or_later() {
        crate::i18n::register();
        let now = chrono::NaiveDate::from_ymd_opt(2026, 11, 3)
            .unwrap()
            .and_hms_opt(20, 0, 0)
            .unwrap();
        assert_eq!(finish_when(now, 95 * 60, "en"), "around 9:35 PM");
        assert_eq!(finish_when(now, 5 * 3600, "en"), "tomorrow around 1:00 AM");
        assert_eq!(finish_when(now, 3 * 86_400, "en"), "in about 3 days");
    }

    #[test]
    fn relative_times() {
        crate::i18n::register();
        let now = 1_000_000;
        assert_eq!(relative_time(now, now), "just now");
        assert_eq!(relative_time(now + 30, now), "just now");
        assert_eq!(relative_time(now - 125, now), "2 min ago");
        assert_eq!(relative_time(now - 7_200, now), "2 h ago");
        assert_eq!(relative_time(now - 90_000, now), "yesterday");
        assert_eq!(relative_time(now - 5 * 86_400, now), "5 days ago");
    }

    #[test]
    fn rates_and_sizes() {
        assert_eq!(rate(None), "—");
        assert_eq!(rate(Some(0)), "—");
        assert_eq!(rate(Some(1536)), "1.5 KiB/s");
        assert_eq!(eta(Some(95)), "1m 35s");
        assert_eq!(size(0), None);
        assert_eq!(size(2048).as_deref(), Some("2.0 KiB"));
    }
}
