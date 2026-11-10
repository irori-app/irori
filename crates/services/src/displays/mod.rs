//! The computer's displays: their names, the mode each runs in, the modes
//! they offer, and whether HDR is on or possible. Settings › System info
//! lists them, and the gamescope and HDR settings use them for their hints.
//!
//! Each system is asked the way it can answer without privileges:
//! - Linux: the kernel's connectors (`/sys/class/drm/card*-*`: status, modes,
//!   EDID for the name and HDR support) plus the compositor's own tool for
//!   the mode in use and HDR: `kscreen-doctor` on KDE Plasma, `hyprctl` on
//!   Hyprland, `wlr-randr` on other wlroots compositors, `xrandr` on X11
//!   (where HDR is always off). GNOME and other compositors leave the mode in
//!   use and HDR's state unknown.
//! - Windows: the display configuration (`QueryDisplayConfig`, advanced
//!   colour for HDR) and `EnumDisplaySettingsW` for the modes.
//! - macOS: `system_profiler SPDisplaysDataType -json`, which gives the
//!   resolution in use but no other modes and no HDR state.
//!
//! Probing runs tools with a timeout and reads files on blocking threads,
//! never on the UI thread; the answer is kept until a refresh asks again.

pub mod parse;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

use std::sync::Arc;

use serde::Serialize;
use tokio::sync::Mutex;

use crate::Backend;

/// One display mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    /// Refresh rate in millihertz; `None` when the source doesn't say.
    pub refresh_mhz: Option<u32>,
}

impl DisplayMode {
    /// The refresh rate for people: whole hertz when it is within a few
    /// hundredths of one (164.958 → "165"), else two decimals ("59.94").
    pub fn refresh_text(&self) -> Option<String> {
        let mhz = self.refresh_mhz?;
        let whole = (mhz + 500) / 1000;
        if mhz.abs_diff(whole * 1000) < 60 {
            Some(whole.to_string())
        } else {
            Some(format!("{:.2}", f64::from(mhz) / 1000.0))
        }
    }

    /// `2560x1440 @ 165 Hz`, for logs and the copied report.
    pub fn plain(&self) -> String {
        match self.refresh_text() {
            Some(hz) => format!("{}x{} @ {hz} Hz", self.width, self.height),
            None => format!("{}x{}", self.width, self.height),
        }
    }
}

/// One display.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Display {
    /// The connector or device (`DP-1`, `\\.\DISPLAY1`); empty when the
    /// system names none.
    pub connector: String,
    /// The monitor's own name ("DELL U2720Q"), when known.
    pub name: Option<String>,
    /// The main display.
    pub primary: bool,
    /// Showing a picture; a connected display can be turned off.
    pub enabled: bool,
    /// The mode in use, when known.
    pub current: Option<DisplayMode>,
    /// The modes it offers, largest first; empty when the system doesn't list
    /// them.
    pub modes: Vec<DisplayMode>,
    /// HDR is on; `None` when the system doesn't say.
    pub hdr_on: Option<bool>,
    /// The display can show HDR; `None` when unknown.
    pub hdr_capable: Option<bool>,
}

impl Display {
    /// What to call it: its name, else its connector.
    pub fn label(&self) -> String {
        match (&self.name, self.connector.is_empty()) {
            (Some(n), _) => n.clone(),
            (None, false) => self.connector.clone(),
            (None, true) => String::new(),
        }
    }

    /// The modes grouped by size, largest first, each with its refresh
    /// rates highest first.
    pub fn sizes(&self) -> Vec<((u32, u32), Vec<DisplayMode>)> {
        let mut out: Vec<((u32, u32), Vec<DisplayMode>)> = Vec::new();
        for m in &self.modes {
            match out.iter_mut().find(|(s, _)| *s == (m.width, m.height)) {
                Some((_, rates)) => rates.push(*m),
                None => out.push(((m.width, m.height), vec![*m])),
            }
        }
        out
    }

    fn tidy(&mut self) {
        let mut modes: Vec<DisplayMode> = Vec::new();
        for m in self.modes.drain(..) {
            if m.width > 0 && m.height > 0 && !modes.contains(&m) {
                modes.push(m);
            }
        }
        modes.sort_by(|a, b| {
            (
                u64::from(b.width) * u64::from(b.height),
                b.width,
                b.refresh_mhz,
            )
                .cmp(&(
                    u64::from(a.width) * u64::from(a.height),
                    a.width,
                    a.refresh_mhz,
                ))
        });
        self.modes = modes;
    }
}

/// The refresh rates of one size as shown, without repeats (29.97 and 30
/// both read "30").
pub fn refresh_texts(modes: &[DisplayMode]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for text in modes.iter().filter_map(DisplayMode::refresh_text) {
        if !out.contains(&text) {
            out.push(text);
        }
    }
    out
}

/// HDR across the displays, for the HDR settings' hints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HdrSummary {
    /// A display runs with HDR on.
    On(String),
    /// A display could; `off` when the system says HDR is off, `false` when
    /// it doesn't say.
    Capable { display: String, off: bool },
    /// Every display says it can't.
    Unsupported,
    /// Nothing is known.
    Unknown,
}

/// What [`Backend::displays`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DisplayReport {
    pub displays: Vec<Display>,
    /// Where the answer came from (`kscreen-doctor`, `xrandr`, `sysfs`,
    /// `DisplayConfig`, `system_profiler`), for the copied report.
    pub source: String,
    /// Whether the source lists the modes a display offers.
    pub lists_modes: bool,
}

impl DisplayReport {
    fn new(source: &str, mut displays: Vec<Display>, lists_modes: bool) -> Self {
        for d in &mut displays {
            d.tidy();
        }
        // The main display first, then the others in the system's order.
        displays.sort_by_key(|d| (!d.primary, !d.enabled));
        Self {
            displays,
            source: source.to_string(),
            lists_modes,
        }
    }

    /// The display games open on: the main one, else the first showing a
    /// picture.
    pub fn main(&self) -> Option<&Display> {
        self.displays
            .iter()
            .find(|d| d.primary && d.enabled)
            .or_else(|| self.displays.iter().find(|d| d.enabled))
    }

    /// HDR across the displays showing a picture.
    pub fn hdr(&self) -> HdrSummary {
        let on: Vec<&Display> = self.displays.iter().filter(|d| d.enabled).collect();
        if let Some(d) = on.iter().find(|d| d.hdr_on == Some(true)) {
            return HdrSummary::On(d.label());
        }
        if let Some(d) = on.iter().find(|d| d.hdr_capable == Some(true)) {
            return HdrSummary::Capable {
                display: d.label(),
                off: d.hdr_on == Some(false),
            };
        }
        if !on.is_empty() && on.iter().all(|d| d.hdr_capable == Some(false)) {
            return HdrSummary::Unsupported;
        }
        HdrSummary::Unknown
    }
}

/// Fill what a compositor's tool reported with the kernel's connectors:
/// the EDID's name and HDR support, and the modes when the tool lists none.
/// Connectors only the kernel knows are added when the tool names them the
/// kernel's way (Wayland compositors do; X11 drivers may not), and a single
/// display on both sides is the same one whatever its name.
pub fn merge(tool: Vec<Display>, kernel: Vec<Display>, kernel_names: bool) -> Vec<Display> {
    if tool.is_empty() {
        return kernel;
    }
    let single = tool.len() == 1 && kernel.len() == 1;
    let mut used = vec![false; kernel.len()];
    let mut out: Vec<Display> = tool
        .into_iter()
        .map(|mut d| {
            let found = kernel
                .iter()
                .position(|k| k.connector == d.connector)
                .or(single.then_some(0));
            if let Some(i) = found {
                used[i] = true;
                let k = &kernel[i];
                if d.name.is_none() {
                    d.name = k.name.clone();
                }
                if d.hdr_capable.is_none() {
                    d.hdr_capable = k.hdr_capable;
                }
                if d.modes.is_empty() {
                    d.modes = k.modes.clone();
                }
            }
            d
        })
        .collect();
    if kernel_names {
        out.extend(
            kernel
                .into_iter()
                .zip(used)
                .filter(|(_, used)| !used)
                .map(|(d, _)| d),
        );
    }
    out
}

/// The report as text, for "Copy system info" and logs.
pub fn format(report: &DisplayReport) -> String {
    let mut out = format!("Displays ({}):\n", report.source);
    if report.displays.is_empty() {
        out.push_str("  none found\n");
    }
    let yes_no = |v: Option<bool>, yes: &str, no: &str| match v {
        Some(true) => yes.to_string(),
        Some(false) => no.to_string(),
        None => "unknown".to_string(),
    };
    for (i, d) in report.displays.iter().enumerate() {
        let mut title = d.label();
        if d.name.is_some() && !d.connector.is_empty() {
            title.push_str(&format!(" ({})", d.connector));
        }
        let flags = [
            d.primary.then_some("main"),
            (!d.enabled).then_some("turned off"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
        out.push_str(&format!(
            "  Display {}: {title}{}\n",
            i + 1,
            if flags.is_empty() {
                String::new()
            } else {
                format!(", {flags}")
            }
        ));
        out.push_str(&format!(
            "    Mode: {}\n    HDR: {} (supported: {})\n",
            d.current.map_or_else(|| "unknown".into(), |m| m.plain()),
            yes_no(d.hdr_on, "on", "off"),
            yes_no(d.hdr_capable, "yes", "no"),
        ));
        if !d.modes.is_empty() {
            let sizes: Vec<String> = d
                .sizes()
                .iter()
                .map(|((w, h), rates)| {
                    let hz = refresh_texts(rates);
                    if hz.is_empty() {
                        format!("{w}x{h}")
                    } else {
                        format!("{w}x{h} ({} Hz)", hz.join(", "))
                    }
                })
                .collect();
            out.push_str(&format!("    Modes: {}\n", sizes.join(", ")));
        }
    }
    out
}

static CACHE: Mutex<Option<Arc<DisplayReport>>> = Mutex::const_new(None);

/// Ask the system; never fails, an unanswerable system gives no displays.
async fn probe() -> DisplayReport {
    #[cfg(target_os = "linux")]
    {
        linux::probe().await
    }
    #[cfg(windows)]
    {
        windows::probe().await
    }
    #[cfg(target_os = "macos")]
    {
        macos::probe().await
    }
    #[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
    {
        DisplayReport::default()
    }
}

impl Backend {
    /// The displays, read once and kept until `refresh` asks again (a
    /// display plugged in, a mode changed).
    pub async fn displays(&self, refresh: bool) -> Arc<DisplayReport> {
        let mut guard = CACHE.lock().await;
        if !refresh && let Some(report) = guard.as_ref() {
            return report.clone();
        }
        let report = Arc::new(probe().await);
        *guard = Some(report.clone());
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(width: u32, height: u32, hz: Option<u32>) -> DisplayMode {
        DisplayMode {
            width,
            height,
            refresh_mhz: hz,
        }
    }

    fn display(connector: &str) -> Display {
        Display {
            connector: connector.into(),
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn refresh_rates_for_people() {
        assert_eq!(mode(1, 1, Some(164_958)).refresh_text().unwrap(), "165");
        assert_eq!(mode(1, 1, Some(59_940)).refresh_text().unwrap(), "59.94");
        assert_eq!(mode(1, 1, Some(60_000)).refresh_text().unwrap(), "60");
        assert_eq!(mode(1, 1, Some(143_970)).refresh_text().unwrap(), "144");
        assert_eq!(mode(1, 1, None).refresh_text(), None);
        assert_eq!(
            mode(2560, 1440, Some(165_000)).plain(),
            "2560x1440 @ 165 Hz"
        );
        assert_eq!(mode(2560, 1440, None).plain(), "2560x1440");
    }

    #[test]
    fn modes_are_sorted_and_grouped() {
        let mut d = display("DP-1");
        d.modes = vec![
            mode(1920, 1080, Some(60_000)),
            mode(2560, 1440, Some(60_000)),
            mode(2560, 1440, Some(165_000)),
            mode(1920, 1080, Some(60_000)),
            mode(0, 0, None),
        ];
        let report = DisplayReport::new("test", vec![d], true);
        let d = &report.displays[0];
        assert_eq!(
            d.modes,
            [
                mode(2560, 1440, Some(165_000)),
                mode(2560, 1440, Some(60_000)),
                mode(1920, 1080, Some(60_000)),
            ]
        );
        let sizes = d.sizes();
        assert_eq!(sizes.len(), 2);
        assert_eq!(sizes[0].0, (2560, 1440));
        assert_eq!(sizes[0].1.len(), 2);
    }

    #[test]
    fn the_main_display_comes_first() {
        let mut off = display("HDMI-A-1");
        off.enabled = false;
        let mut main = display("DP-2");
        main.primary = true;
        let report = DisplayReport::new("test", vec![off, display("DP-1"), main], true);
        let order: Vec<&str> = report
            .displays
            .iter()
            .map(|d| d.connector.as_str())
            .collect();
        assert_eq!(order, ["DP-2", "DP-1", "HDMI-A-1"]);
        assert_eq!(report.main().unwrap().connector, "DP-2");
        let none = DisplayReport::new("test", vec![display("DP-1")], true);
        assert_eq!(none.main().unwrap().connector, "DP-1");
        assert!(DisplayReport::default().main().is_none());
    }

    #[test]
    fn hdr_summaries() {
        let report = |displays: Vec<Display>| DisplayReport::new("t", displays, true).hdr();
        let mut on = display("DP-1");
        on.hdr_on = Some(true);
        on.name = Some("LG".into());
        let mut capable_off = display("DP-2");
        capable_off.hdr_capable = Some(true);
        capable_off.hdr_on = Some(false);
        let mut capable = display("DP-3");
        capable.hdr_capable = Some(true);
        let mut sdr = display("DP-4");
        sdr.hdr_capable = Some(false);
        assert_eq!(
            report(vec![sdr.clone(), on.clone()]),
            HdrSummary::On("LG".into())
        );
        assert_eq!(
            report(vec![sdr.clone(), capable_off]),
            HdrSummary::Capable {
                display: "DP-2".into(),
                off: true
            }
        );
        assert_eq!(
            report(vec![capable]),
            HdrSummary::Capable {
                display: "DP-3".into(),
                off: false
            }
        );
        assert_eq!(report(vec![sdr.clone()]), HdrSummary::Unsupported);
        assert_eq!(report(vec![sdr, display("DP-5")]), HdrSummary::Unknown);
        assert_eq!(report(vec![]), HdrSummary::Unknown);
        // A display that is turned off doesn't count.
        let mut off = on;
        off.enabled = false;
        assert_eq!(report(vec![off]), HdrSummary::Unknown);
    }

    #[test]
    fn kernel_details_fill_the_tools_answer() {
        let mut kernel_dp = display("DP-1");
        kernel_dp.name = Some("DELL U2720Q".into());
        kernel_dp.hdr_capable = Some(true);
        kernel_dp.modes = vec![mode(3840, 2160, None)];
        let mut kernel_hdmi = display("HDMI-A-1");
        kernel_hdmi.enabled = false;
        let mut tool = display("DP-1");
        tool.current = Some(mode(3840, 2160, Some(60_000)));
        tool.hdr_on = Some(false);

        let merged = merge(
            vec![tool.clone()],
            vec![kernel_dp.clone(), kernel_hdmi.clone()],
            true,
        );
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].name.as_deref(), Some("DELL U2720Q"));
        assert_eq!(merged[0].hdr_capable, Some(true));
        assert_eq!(merged[0].hdr_on, Some(false));
        assert_eq!(merged[0].modes, [mode(3840, 2160, None)]);
        assert_eq!(merged[1].connector, "HDMI-A-1");

        // X11 names connectors its own way: only a single display pairs up,
        // and the kernel's names aren't added.
        let mut x11 = tool.clone();
        x11.connector = "DisplayPort-0".into();
        let merged = merge(vec![x11.clone()], vec![kernel_dp.clone()], false);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].name.as_deref(), Some("DELL U2720Q"));
        let merged = merge(vec![x11], vec![kernel_dp.clone(), kernel_hdmi], false);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].name, None);

        assert_eq!(merge(vec![], vec![kernel_dp.clone()], true), [kernel_dp]);
    }

    #[test]
    fn copied_report() {
        let mut d = display("DP-1");
        d.name = Some("DELL U2720Q".into());
        d.primary = true;
        d.current = Some(mode(3840, 2160, Some(60_000)));
        d.modes = vec![
            mode(3840, 2160, Some(60_000)),
            mode(3840, 2160, Some(30_000)),
            mode(3840, 2160, Some(29_970)),
            mode(1920, 1080, None),
        ];
        d.hdr_on = Some(false);
        d.hdr_capable = Some(true);
        let text = format(&DisplayReport::new("kscreen-doctor", vec![d], true));
        assert_eq!(
            text,
            "Displays (kscreen-doctor):\n  Display 1: DELL U2720Q (DP-1), main\n    \
             Mode: 3840x2160 @ 60 Hz\n    HDR: off (supported: yes)\n    \
             Modes: 3840x2160 (60, 30 Hz), 1920x1080\n"
        );
        let empty = format(&DisplayReport::new("sysfs", vec![], true));
        assert_eq!(empty, "Displays (sysfs):\n  none found\n");
    }
}
