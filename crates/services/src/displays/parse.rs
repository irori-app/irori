//! Readers for what each system says about its displays: the Linux
//! compositors' tools (`kscreen-doctor -j`, `hyprctl -j monitors all`,
//! `wlr-randr --json`, `xrandr --query`), the kernel's connector files and
//! EDID, macOS's `system_profiler SPDisplaysDataType -json`, and the bits of
//! Windows' display configuration. All pure and tested on every system; the
//! probes that run the tools live beside this module.

use serde_json::Value;

use super::{Display, DisplayMode};

/// A refresh rate in Hz as millihertz.
fn millihertz(hz: f64) -> Option<u32> {
    (hz.is_finite() && hz > 0.0 && hz < 10_000.0).then(|| (hz * 1000.0).round() as u32)
}

/// `2560x1440`, `2560x1440@165`, `2560x1440@164.96Hz`, `1920x1080i`.
pub fn mode_text(text: &str) -> Option<DisplayMode> {
    let text = text.trim();
    let (size, rate) = match text.split_once('@') {
        Some((s, r)) => (s, Some(r.trim().trim_end_matches("Hz").trim())),
        None => (text, None),
    };
    let (w, h) = size.trim().split_once(['x', 'X'])?;
    let h = h.trim_end_matches(|c: char| !c.is_ascii_digit());
    Some(DisplayMode {
        width: w.trim().parse().ok()?,
        height: h.parse().ok()?,
        refresh_mhz: rate
            .and_then(|r| r.parse::<f64>().ok())
            .and_then(millihertz),
    })
}

fn u32_at(value: &Value, key: &str) -> Option<u32> {
    value.get(key)?.as_u64().and_then(|n| u32::try_from(n).ok())
}

fn str_at<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str().filter(|s| !s.trim().is_empty())
}

/// KDE Plasma's `kscreen-doctor -j`. `hdr` (Plasma 6) says whether HDR is
/// on; whether the display could comes from its EDID ([`super::merge`]).
pub fn kscreen(json: &str) -> Option<Vec<Display>> {
    let root: Value = serde_json::from_str(json).ok()?;
    let outputs = root.get("outputs")?.as_array()?;
    let displays = outputs
        .iter()
        .filter(|o| o.get("connected").and_then(Value::as_bool) != Some(false))
        .map(|o| {
            let modes: Vec<(String, DisplayMode)> = o
                .get("modes")
                .and_then(Value::as_array)
                .map(|modes| {
                    modes
                        .iter()
                        .filter_map(|m| {
                            let size = m.get("size")?;
                            let id = match m.get("id")? {
                                Value::String(s) => s.clone(),
                                other => other.to_string(),
                            };
                            Some((
                                id,
                                DisplayMode {
                                    width: u32_at(size, "width")?,
                                    height: u32_at(size, "height")?,
                                    refresh_mhz: m
                                        .get("refreshRate")
                                        .and_then(Value::as_f64)
                                        .and_then(millihertz),
                                },
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let current_id = match o.get("currentModeId") {
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Number(n)) => Some(n.to_string()),
                _ => None,
            };
            let enabled = o.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            let hdr_on = o
                .get("hdr")
                .or_else(|| o.get("hdrEnabled"))
                .and_then(Value::as_bool);
            Display {
                connector: str_at(o, "name").unwrap_or_default().to_string(),
                name: None,
                primary: o.get("priority").and_then(Value::as_u64) == Some(1)
                    || o.get("primary").and_then(Value::as_bool) == Some(true),
                enabled,
                current: current_id
                    .filter(|_| enabled)
                    .and_then(|id| modes.iter().find(|(m, _)| *m == id).map(|(_, m)| *m)),
                modes: modes.into_iter().map(|(_, m)| m).collect(),
                hdr_on: if enabled { hdr_on } else { None },
                // HDR on shows it can; off says nothing about the display.
                hdr_capable: hdr_on.filter(|on| *on),
            }
        })
        .collect();
    Some(displays)
}

/// Hyprland's `hyprctl -j monitors all`. Newer versions name the colour
/// management preset (`hdr`, `hdredid`) the monitor runs with.
pub fn hyprctl(json: &str) -> Option<Vec<Display>> {
    let root: Value = serde_json::from_str(json).ok()?;
    let displays = root
        .as_array()?
        .iter()
        .map(|m| {
            let enabled = m.get("disabled").and_then(Value::as_bool) != Some(true);
            let current = enabled
                .then(|| {
                    Some(DisplayMode {
                        width: u32_at(m, "width")?,
                        height: u32_at(m, "height")?,
                        refresh_mhz: m
                            .get("refreshRate")
                            .and_then(Value::as_f64)
                            .and_then(millihertz),
                    })
                })
                .flatten();
            let modes = m
                .get("availableModes")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| mode_text(v.as_str()?)).collect())
                .unwrap_or_default();
            let name = [str_at(m, "make"), str_at(m, "model")]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ");
            Display {
                connector: str_at(m, "name").unwrap_or_default().to_string(),
                name: (!name.is_empty()).then_some(name),
                primary: m.get("id").and_then(Value::as_u64) == Some(0),
                enabled,
                current,
                modes,
                hdr_on: str_at(m, "colorManagementPreset")
                    .filter(|_| enabled)
                    .map(|p| p.to_ascii_lowercase().contains("hdr")),
                hdr_capable: None,
            }
        })
        .collect();
    Some(displays)
}

/// `wlr-randr --json` (sway and other wlroots compositors). It says
/// nothing about HDR.
pub fn wlr_randr(json: &str) -> Option<Vec<Display>> {
    let root: Value = serde_json::from_str(json).ok()?;
    let displays = root
        .as_array()?
        .iter()
        .map(|o| {
            let enabled = o.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            let mut current = None;
            let modes = o
                .get("modes")
                .and_then(Value::as_array)
                .map(|modes| {
                    modes
                        .iter()
                        .filter_map(|m| {
                            let mode = DisplayMode {
                                width: u32_at(m, "width")?,
                                height: u32_at(m, "height")?,
                                refresh_mhz: m
                                    .get("refresh")
                                    .and_then(Value::as_f64)
                                    .and_then(millihertz),
                            };
                            if enabled && m.get("current").and_then(Value::as_bool) == Some(true) {
                                current = Some(mode);
                            }
                            Some(mode)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let name = [str_at(o, "make"), str_at(o, "model")]
                .into_iter()
                .flatten()
                .filter(|s| *s != "Unknown")
                .collect::<Vec<_>>()
                .join(" ");
            Display {
                connector: str_at(o, "name").unwrap_or_default().to_string(),
                name: (!name.is_empty()).then_some(name),
                primary: false,
                enabled,
                current,
                modes,
                hdr_on: None,
                hdr_capable: None,
            }
        })
        .collect();
    Some(displays)
}

/// `xrandr --query` on X11, which has no HDR: every display reports it off.
pub fn xrandr(text: &str) -> Vec<Display> {
    let mut displays: Vec<Display> = Vec::new();
    let mut in_connected = false;
    for line in text.lines() {
        if !line.starts_with(char::is_whitespace) {
            in_connected = false;
            let mut words = line.split_whitespace();
            let (Some(connector), Some(state)) = (words.next(), words.next()) else {
                continue;
            };
            if state != "connected" {
                continue;
            }
            in_connected = true;
            let rest: Vec<&str> = words.collect();
            let geometry = rest
                .iter()
                .find(|w| w.contains('x') && w.contains('+'))
                .copied();
            displays.push(Display {
                connector: connector.to_string(),
                name: None,
                primary: rest.first() == Some(&"primary"),
                // A connected output without a geometry is turned off.
                enabled: geometry.is_some(),
                current: None,
                modes: Vec::new(),
                hdr_on: Some(false),
                hdr_capable: None,
            });
            continue;
        }
        if !in_connected {
            continue;
        }
        let Some(display) = displays.last_mut() else {
            continue;
        };
        let mut words = line.split_whitespace();
        let Some(size) = words.next().and_then(mode_text) else {
            continue;
        };
        let mut any_rate = false;
        for rate in words {
            let current = rate.contains('*');
            let Some(mhz) = rate
                .trim_end_matches(['*', '+', ' '])
                .parse::<f64>()
                .ok()
                .and_then(millihertz)
            else {
                continue;
            };
            any_rate = true;
            let mode = DisplayMode {
                refresh_mhz: Some(mhz),
                ..size
            };
            display.modes.push(mode);
            if current && display.enabled {
                display.current = Some(mode);
            }
        }
        if !any_rate {
            display.modes.push(size);
        }
    }
    displays
}

/// A DRM connector's `modes` file: one `WxH` per line, preferred first, no
/// refresh rates.
pub fn sysfs_modes(text: &str) -> Vec<DisplayMode> {
    text.lines().filter_map(mode_text).collect()
}

/// What a display's EDID says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Edid {
    /// Three-letter manufacturer id (`DEL`, `SAM`).
    pub manufacturer: Option<String>,
    /// The monitor name descriptor ("DELL U2720Q").
    pub name: Option<String>,
    /// The CTA extension's HDR static metadata block lists the PQ (ST 2084)
    /// or HLG transfer function.
    pub hdr: bool,
}

/// Read an EDID: the 128-byte base block and its CTA-861 extensions. Every
/// offset is checked; a short or foreign file gives what could be read.
pub fn edid(bytes: &[u8]) -> Option<Edid> {
    const HEADER: [u8; 8] = [0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0];
    if bytes.len() < 128 || bytes[..8] != HEADER {
        return None;
    }
    let mut out = Edid::default();
    let id = u16::from_be_bytes([bytes[8], bytes[9]]);
    let letters: Option<String> = [10u16, 5, 0]
        .iter()
        .map(|shift| {
            let n = ((id >> shift) & 0x1f) as u8;
            (1..=26).contains(&n).then(|| (b'A' + n - 1) as char)
        })
        .collect();
    out.manufacturer = letters;
    for start in [54usize, 72, 90, 108] {
        let d = &bytes[start..start + 18];
        if d[0] == 0 && d[1] == 0 && d[3] == 0xfc {
            let text: String = d[5..]
                .iter()
                .take_while(|b| **b != 0x0a && **b != 0)
                .map(|b| *b as char)
                .collect();
            let text = text.trim().to_string();
            if !text.is_empty() {
                out.name = Some(text);
            }
        }
    }
    for ext in bytes[128..].as_chunks::<128>().0 {
        if ext[0] != 0x02 {
            continue;
        }
        // Data blocks run from byte 4 to the detailed timings' offset.
        let end = usize::from(ext[2]).clamp(4, 127);
        let mut i = 4;
        while i < end {
            let tag = ext[i] >> 5;
            let len = usize::from(ext[i] & 0x1f);
            if i + 1 + len > end {
                break;
            }
            // An extended block (tag 7) whose extended tag 6 is HDR static
            // metadata; its first byte lists the transfer functions.
            if tag == 7 && len >= 2 && ext[i + 1] == 6 {
                let eotf = ext[i + 2];
                if eotf & 0b1100 != 0 {
                    out.hdr = true;
                }
            }
            i += 1 + len;
        }
    }
    Some(out)
}

/// macOS's `system_profiler SPDisplaysDataType -json`: every graphics
/// processor's displays, with the resolution in use. It lists no other
/// modes, and says nothing about HDR beyond the XDR displays' names.
pub fn system_profiler(json: &str) -> Option<Vec<Display>> {
    let root: Value = serde_json::from_str(json).ok()?;
    let gpus = root.get("SPDisplaysDataType")?.as_array()?;
    let yes = |v: Option<&str>| v.is_some_and(|v| v.ends_with("_yes"));
    let displays = gpus
        .iter()
        .filter_map(|g| g.get("spdisplays_ndrvs")?.as_array())
        .flatten()
        .map(|d| {
            let name = str_at(d, "_name").map(str::to_string);
            let resolution = str_at(d, "_spdisplays_resolution")
                .or_else(|| str_at(d, "spdisplays_resolution"))
                .and_then(resolution_text);
            let kind = [str_at(d, "_name"), str_at(d, "spdisplays_display_type")]
                .into_iter()
                .flatten()
                .any(|s| s.to_ascii_lowercase().contains("xdr"));
            let online = str_at(d, "spdisplays_online");
            Display {
                connector: String::new(),
                name,
                primary: yes(str_at(d, "spdisplays_main")),
                enabled: online.is_none() || yes(online),
                current: resolution,
                modes: Vec::new(),
                hdr_on: None,
                hdr_capable: kind.then_some(true),
            }
        })
        .collect();
    Some(displays)
}

/// `1728 x 1117 @ 120.00Hz`, `2560 x 1440 (QHD/WQHD - Wide Quad High Definition)`.
fn resolution_text(text: &str) -> Option<DisplayMode> {
    let (size, rate) = match text.split_once('@') {
        Some((s, r)) => (s, Some(r)),
        None => (text, None),
    };
    let mut numbers = size
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty());
    let width = numbers.next()?.parse().ok()?;
    let height = numbers.next()?.parse().ok()?;
    let refresh_mhz = rate.and_then(|r| {
        let digits: String = r
            .trim()
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        digits.parse::<f64>().ok().and_then(millihertz)
    });
    Some(DisplayMode {
        width,
        height,
        refresh_mhz,
    })
}

/// Windows' `DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO` bits: whether the
/// display can show advanced colour (HDR) and whether it is on.
pub fn advanced_color(bits: u32) -> (bool, bool) {
    let supported = bits & 0x1 != 0;
    let force_disabled = bits & 0x8 != 0;
    (supported && !force_disabled, bits & 0x2 != 0)
}

/// A `DISPLAYCONFIG_RATIONAL` refresh rate as millihertz.
pub fn rational(numerator: u32, denominator: u32) -> Option<u32> {
    (denominator != 0 && numerator != 0)
        .then(|| (u64::from(numerator) * 1000 / u64::from(denominator)) as u32)
        .filter(|m| *m > 0)
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

    #[test]
    fn mode_names() {
        assert_eq!(mode_text("2560x1440"), Some(mode(2560, 1440, None)));
        assert_eq!(
            mode_text("2560x1440@164.96Hz"),
            Some(mode(2560, 1440, Some(164_960)))
        );
        assert_eq!(mode_text("1920x1080i"), Some(mode(1920, 1080, None)));
        assert_eq!(mode_text("auto"), None);
        assert_eq!(mode_text(""), None);
    }

    const KSCREEN: &str = r#"{"outputs": [
        {"name": "DP-2", "connected": true, "enabled": true, "priority": 1,
         "currentModeId": "72", "hdr": true, "wcg": true, "sdr-brightness": 200,
         "modes": [
            {"id": "72", "name": "2560x1440@165", "refreshRate": 164.958,
             "size": {"width": 2560, "height": 1440}},
            {"id": "73", "name": "1920x1080@60", "refreshRate": 60.0,
             "size": {"width": 1920, "height": 1080}}]},
        {"name": "HDMI-A-1", "connected": true, "enabled": false, "priority": 0,
         "currentModeId": "", "hdr": false, "modes": [],
         "future": {"x": 1}},
        {"name": "DP-3", "connected": false, "modes": []}
    ], "screen": {}}"#;

    #[test]
    fn kscreen_doctor() {
        let d = kscreen(KSCREEN).unwrap();
        assert_eq!(d.len(), 2, "disconnected outputs are left out");
        assert_eq!(d[0].connector, "DP-2");
        assert!(d[0].primary && d[0].enabled);
        assert_eq!(d[0].current, Some(mode(2560, 1440, Some(164_958))));
        assert_eq!(d[0].modes.len(), 2);
        assert_eq!((d[0].hdr_on, d[0].hdr_capable), (Some(true), Some(true)));
        assert!(!d[1].enabled && d[1].current.is_none());
        assert_eq!((d[1].hdr_on, d[1].hdr_capable), (None, None));
        // Plasma 5: no HDR keys, numeric mode ids, `primary`.
        let old = r#"{"outputs": [{"name": "eDP-1", "connected": true, "enabled": true,
            "primary": true, "currentModeId": 3,
            "modes": [{"id": 3, "refreshRate": 60.0, "size": {"width": 1920, "height": 1200}}]}]}"#;
        let d = kscreen(old).unwrap();
        assert!(d[0].primary);
        assert_eq!(d[0].current, Some(mode(1920, 1200, Some(60_000))));
        assert_eq!((d[0].hdr_on, d[0].hdr_capable), (None, None));
        assert!(kscreen("not json").is_none());
    }

    #[test]
    fn hyprland() {
        let json = r#"[{"id": 0, "name": "DP-1", "make": "Dell Inc.", "model": "DELL U2720Q",
            "width": 3840, "height": 2160, "refreshRate": 59.997, "disabled": false,
            "colorManagementPreset": "hdr",
            "availableModes": ["3840x2160@60.00Hz", "2560x1440@59.95Hz"]},
            {"id": 1, "name": "HDMI-A-1", "width": 1920, "height": 1080,
             "refreshRate": 60.0, "disabled": true, "availableModes": []}]"#;
        let d = hyprctl(json).unwrap();
        assert_eq!(d[0].name.as_deref(), Some("Dell Inc. DELL U2720Q"));
        assert!(d[0].primary);
        assert_eq!(d[0].current, Some(mode(3840, 2160, Some(59_997))));
        assert_eq!(d[0].modes[1], mode(2560, 1440, Some(59_950)));
        assert_eq!(d[0].hdr_on, Some(true));
        assert!(!d[1].enabled && d[1].current.is_none() && d[1].hdr_on.is_none());
    }

    #[test]
    fn wlroots() {
        let json = r#"[{"name": "DP-1", "make": "Unknown", "model": "XG27AQ",
            "enabled": true, "modes": [
              {"width": 2560, "height": 1440, "refresh": 170.0, "preferred": true, "current": true},
              {"width": 2560, "height": 1440, "refresh": 59.951, "preferred": false, "current": false}]}]"#;
        let d = wlr_randr(json).unwrap();
        assert_eq!(d[0].name.as_deref(), Some("XG27AQ"));
        assert_eq!(d[0].current, Some(mode(2560, 1440, Some(170_000))));
        assert_eq!(d[0].modes.len(), 2);
        assert_eq!(d[0].hdr_on, None);
    }

    #[test]
    fn x11() {
        let text = "\
Screen 0: minimum 320 x 200, current 4480 x 1440, maximum 16384 x 16384
DisplayPort-0 connected primary 2560x1440+0+0 (normal left inverted right x axis y axis) 597mm x 336mm
   2560x1440    164.96*+ 143.97   59.95
   1920x1080     60.00    50.00
   1920x1080i    60.00
HDMI-A-0 connected 1920x1080+2560+0 (normal left inverted right x axis y axis) 527mm x 296mm
   1920x1080     60.00*+
DisplayPort-1 disconnected (normal left inverted right x axis y axis)
DVI-D-0 connected (normal left inverted right x axis y axis)
   1280x1024     60.02 +
";
        let d = xrandr(text);
        assert_eq!(d.len(), 3);
        assert!(d[0].primary && d[0].enabled);
        assert_eq!(d[0].current, Some(mode(2560, 1440, Some(164_960))));
        assert_eq!(d[0].modes.len(), 6);
        assert_eq!(d[0].hdr_on, Some(false));
        assert_eq!(d[1].current, Some(mode(1920, 1080, Some(60_000))));
        assert!(!d[1].primary);
        assert!(!d[2].enabled && d[2].current.is_none());
        assert_eq!(d[2].modes, [mode(1280, 1024, Some(60_020))]);
    }

    #[test]
    fn kernel_modes() {
        assert_eq!(
            sysfs_modes("2560x1440\n1920x1080\n1920x1080i\n\n"),
            [
                mode(2560, 1440, None),
                mode(1920, 1080, None),
                mode(1920, 1080, None)
            ]
        );
    }

    /// A base block with a name descriptor, plus a CTA extension holding
    /// an HDR static metadata block when `hdr`.
    fn edid_bytes(name: &str, hdr: bool) -> Vec<u8> {
        let mut b = vec![0u8; 256];
        b[..8].copy_from_slice(&[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
        // "DEL": D=4, E=5, L=12.
        let id: u16 = (4 << 10) | (5 << 5) | 12;
        b[8..10].copy_from_slice(&id.to_be_bytes());
        let d = &mut b[90..108];
        d[3] = 0xfc;
        for (i, c) in name.bytes().chain([0x0a]).enumerate() {
            d[5 + i] = c;
        }
        let ext = &mut b[128..];
        ext[0] = 0x02;
        ext[1] = 3;
        // A video block (tag 2, 2 bytes), then HDR static metadata.
        ext[4] = (2 << 5) | 2;
        ext[5] = 16;
        ext[6] = 31;
        ext[7] = (7 << 5) | 3;
        ext[8] = 6;
        ext[9] = if hdr { 0b0101 } else { 0b0011 };
        ext[10] = 1;
        ext[2] = 11;
        b
    }

    #[test]
    fn edid_names_and_hdr() {
        let e = edid(&edid_bytes("DELL U2720Q", true)).unwrap();
        assert_eq!(e.manufacturer.as_deref(), Some("DEL"));
        assert_eq!(e.name.as_deref(), Some("DELL U2720Q"));
        assert!(e.hdr);
        // SDR and HDR (traditional gamma) but no PQ or HLG.
        assert!(!edid(&edid_bytes("SDR", false)).unwrap().hdr);
        // Only the base block.
        let base = &edid_bytes("Panel", true)[..128];
        let e = edid(base).unwrap();
        assert_eq!(e.name.as_deref(), Some("Panel"));
        assert!(!e.hdr);
        // A block claiming more than the extension holds is ignored.
        let mut broken = edid_bytes("X", true);
        broken[128 + 7] = (7 << 5) | 31;
        assert!(!edid(&broken).unwrap().hdr);
        assert_eq!(edid(&[0u8; 64]), None);
        assert_eq!(edid(&[1u8; 128]), None);
    }

    #[test]
    fn mac_displays() {
        let json = r#"{"SPDisplaysDataType": [{"_name": "Apple M1 Pro",
            "spdisplays_ndrvs": [
              {"_name": "Color LCD", "_spdisplays_pixels": "3456 x 2234",
               "_spdisplays_resolution": "1728 x 1117 @ 120.00Hz",
               "spdisplays_display_type": "spdisplays_built-in-liquid-retina-xdr",
               "spdisplays_main": "spdisplays_yes", "spdisplays_online": "spdisplays_yes"},
              {"_name": "LG HDR 4K", "_spdisplays_resolution": "2560 x 1440 (QHD/WQHD - Wide Quad High Definition)",
               "spdisplays_online": "spdisplays_yes"}]}]}"#;
        let d = system_profiler(json).unwrap();
        assert_eq!(d.len(), 2);
        assert!(d[0].primary && d[0].enabled);
        assert_eq!(d[0].current, Some(mode(1728, 1117, Some(120_000))));
        assert_eq!(d[0].hdr_capable, Some(true));
        assert_eq!(d[1].current, Some(mode(2560, 1440, None)));
        assert_eq!((d[1].hdr_on, d[1].hdr_capable), (None, None));
        assert!(d[1].modes.is_empty());
    }

    #[test]
    fn windows_bits() {
        assert_eq!(advanced_color(0b0011), (true, true));
        assert_eq!(advanced_color(0b0001), (true, false));
        assert_eq!(advanced_color(0b1001), (false, false));
        assert_eq!(advanced_color(0), (false, false));
        assert_eq!(rational(144_000, 1000), Some(144_000));
        assert_eq!(rational(60_000, 1001), Some(59_940));
        assert_eq!(rational(60, 0), None);
    }
}
