//! The displays in Settings › System info, and the hints the HDR and
//! gamescope settings take from them (`irori_services::displays`).

use gpui_kit::SharedString;
use irori_services::displays::{Display, DisplayMode, DisplayReport, HdrSummary, refresh_texts};
use irori_ui::i18n::{t, t_args};

/// Sizes listed per display before "and N more".
const SIZES_SHOWN: usize = 6;

/// `2560 × 1440 at 165 Hz`, or `2560 × 1440` without a rate.
pub(crate) fn mode_text(mode: &DisplayMode) -> SharedString {
    let (w, h) = (mode.width.to_string(), mode.height.to_string());
    match mode.refresh_text() {
        Some(hz) => t_args(
            "settings.display.mode_refresh",
            &[("width", &w), ("height", &h), ("refresh", &hz)],
        ),
        None => t_args(
            "settings.display.mode_size",
            &[("width", &w), ("height", &h)],
        ),
    }
}

/// The System info rows of every display: what it is and runs at, then the
/// modes it offers.
pub(crate) fn rows(report: &DisplayReport) -> Vec<(SharedString, SharedString)> {
    if report.displays.is_empty() {
        return vec![(t("settings.display.label"), t("settings.display.none"))];
    }
    let several = report.displays.len() > 1;
    let mut rows = Vec::new();
    for (i, d) in report.displays.iter().enumerate() {
        let number = (i + 1).to_string();
        let (label, modes_label) = if several {
            (
                t_args("settings.display.numbered", &[("number", &number)]),
                t_args("settings.display.modes_numbered", &[("number", &number)]),
            )
        } else {
            (t("settings.display.label"), t("settings.display.modes"))
        };
        rows.push((label, summary(d)));
        let modes = if !report.lists_modes {
            Some(t("settings.display.modes_unlisted"))
        } else if d.modes.is_empty() {
            None
        } else {
            Some(sizes(d))
        };
        if let Some(modes) = modes {
            rows.push((modes_label, modes));
        }
    }
    rows
}

/// `M32QC (HDMI-A-2) · main · 2560 × 1440 at 144 Hz · HDR on`.
fn summary(d: &Display) -> SharedString {
    let mut parts: Vec<SharedString> = Vec::new();
    let mut title = d.label();
    if d.name.is_some() && !d.connector.is_empty() {
        title = format!("{title} ({})", d.connector);
    }
    if !title.is_empty() {
        parts.push(title.into());
    }
    if d.primary {
        parts.push(t("settings.display.main"));
    }
    if !d.enabled {
        parts.push(t("settings.display.off"));
    } else {
        parts.push(match &d.current {
            Some(m) => mode_text(m),
            None => t("settings.display.mode_unknown"),
        });
    }
    let hdr = match (d.hdr_on, d.hdr_capable) {
        (Some(true), _) => Some("settings.display.hdr_on"),
        (_, Some(false)) => Some("settings.display.hdr_unsupported"),
        (Some(false), _) => Some("settings.display.hdr_off"),
        (None, Some(true)) => Some("settings.display.hdr_supported"),
        (None, None) => None,
    };
    if let Some(key) = hdr.filter(|_| d.enabled) {
        parts.push(t(key));
    }
    parts.join(" · ").into()
}

/// The sizes a display offers with their refresh rates, the largest first.
fn sizes(d: &Display) -> SharedString {
    let all = d.sizes();
    let mut shown: Vec<String> = all
        .iter()
        .take(SIZES_SHOWN)
        .map(|((w, h), rates)| {
            let (w, h) = (w.to_string(), h.to_string());
            let hz = refresh_texts(rates);
            if hz.is_empty() {
                t_args(
                    "settings.display.mode_size",
                    &[("width", &w), ("height", &h)],
                )
            } else {
                t_args(
                    "settings.display.mode_rates",
                    &[("width", &w), ("height", &h), ("rates", &hz.join(", "))],
                )
            }
            .to_string()
        })
        .collect();
    let more = all.len().saturating_sub(SIZES_SHOWN);
    if more > 0 {
        let key = if more == 1 {
            "settings.display.more_one"
        } else {
            "settings.display.more_other"
        };
        shown.push(t_args(key, &[("count", &more.to_string())]).to_string());
    }
    shown.join(", ").into()
}

/// What a settings field says about the displays, as a translation key and
/// its arguments; `None` when there's nothing useful to say.
pub(crate) fn hint(
    path: &str,
    report: &DisplayReport,
) -> Option<(&'static str, Vec<(&'static str, String)>)> {
    match path {
        "enableHDR" | "gamescope.additionalOptions" => match report.hdr() {
            HdrSummary::On(display) => {
                Some(("settings.display.hint_hdr_on", vec![("display", display)]))
            }
            HdrSummary::Capable { display, off: true } => {
                Some(("settings.display.hint_hdr_off", vec![("display", display)]))
            }
            HdrSummary::Capable {
                display,
                off: false,
            } => Some((
                "settings.display.hint_hdr_capable",
                vec![("display", display)],
            )),
            HdrSummary::Unsupported => Some(("settings.display.hint_hdr_unsupported", vec![])),
            HdrSummary::Unknown => None,
        },
        "gamescope.upscaleWidth" | "gamescope.upscaleHeight" | "gamescope.fpsLimiter" => {
            let main = report.main()?;
            let mode = main.current?;
            let display = main.label();
            (!display.is_empty()).then(|| {
                (
                    "settings.display.hint_mode",
                    vec![("display", display), ("mode", mode_text(&mode).to_string())],
                )
            })
        }
        _ => None,
    }
}

/// [`hint`] in the interface language.
pub(crate) fn hint_text(path: &str, report: &DisplayReport) -> Option<SharedString> {
    let (key, args) = hint(path, report)?;
    let args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (*k, v.as_str())).collect();
    Some(t_args(key, &args))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(name: &str) -> Display {
        Display {
            connector: "DP-1".into(),
            name: Some(name.into()),
            enabled: true,
            primary: true,
            current: Some(DisplayMode {
                width: 2560,
                height: 1440,
                refresh_mhz: Some(164_958),
            }),
            ..Default::default()
        }
    }

    fn report(displays: Vec<Display>) -> DisplayReport {
        DisplayReport {
            displays,
            source: "test".into(),
            lists_modes: true,
        }
    }

    #[test]
    fn hdr_hints_follow_the_displays() {
        let mut d = display("M32QC");
        d.hdr_on = Some(true);
        let (key, args) = hint("enableHDR", &report(vec![d.clone()])).unwrap();
        assert_eq!(key, "settings.display.hint_hdr_on");
        assert_eq!(args, [("display", "M32QC".to_string())]);

        d.hdr_on = Some(false);
        d.hdr_capable = Some(true);
        assert_eq!(
            hint("gamescope.additionalOptions", &report(vec![d.clone()]))
                .unwrap()
                .0,
            "settings.display.hint_hdr_off"
        );
        d.hdr_on = None;
        assert_eq!(
            hint("enableHDR", &report(vec![d.clone()])).unwrap().0,
            "settings.display.hint_hdr_capable"
        );
        d.hdr_capable = Some(false);
        assert_eq!(
            hint("enableHDR", &report(vec![d.clone()])).unwrap().0,
            "settings.display.hint_hdr_unsupported"
        );
        // Unknown says nothing rather than guess.
        d.hdr_capable = None;
        assert!(hint("enableHDR", &report(vec![d])).is_none());
        assert!(hint("enableHDR", &report(vec![])).is_none());
    }

    #[test]
    fn gamescope_hints_name_the_main_display() {
        let (key, args) = hint("gamescope.upscaleWidth", &report(vec![display("M32QC")])).unwrap();
        assert_eq!(key, "settings.display.hint_mode");
        assert_eq!(args[0], ("display", "M32QC".to_string()));
        assert!(hint("gamescope.fpsLimiter", &report(vec![display("M32QC")])).is_some());
        let mut unknown = display("M32QC");
        unknown.current = None;
        assert!(hint("gamescope.upscaleWidth", &report(vec![unknown])).is_none());
        assert!(hint("wineVersion", &report(vec![display("M32QC")])).is_none());
    }
}
