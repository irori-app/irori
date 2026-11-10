//! Linux's displays: the kernel's connectors, then the compositor's tool.

use std::path::Path;
use std::time::Duration;

use irori_core::host;
use irori_core::process::CommandSpec;
use irori_runtime::discovery::run_with_timeout;

use super::{Display, DisplayReport, merge, parse};

/// How long a tool may take; a hung compositor mustn't hold the page.
const TOOL_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) async fn probe() -> DisplayReport {
    let kernel = tokio::task::spawn_blocking(|| connectors(Path::new("/sys/class/drm")))
        .await
        .unwrap_or_default();
    let (source, tool, kernel_names) = match compositor().await {
        Some((source, displays, kernel_names)) => (source, displays, kernel_names),
        None => ("sysfs", Vec::new(), true),
    };
    DisplayReport::new(source, merge(tool, kernel, kernel_names), true)
}

/// The compositor's own answer: which tool gave it, the displays, and
/// whether it names connectors the kernel's way.
async fn compositor() -> Option<(&'static str, Vec<Display>, bool)> {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let desktop = var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_uppercase();
    let wayland =
        var("WAYLAND_DISPLAY").is_some() || var("XDG_SESSION_TYPE").is_some_and(|t| t == "wayland");
    if desktop.split(':').any(|d| d == "KDE")
        && let Some(text) = run("kscreen-doctor", &["-j"]).await
        && let Some(displays) = parse::kscreen(&text)
    {
        return Some(("kscreen-doctor", displays, true));
    }
    if var("HYPRLAND_INSTANCE_SIGNATURE").is_some()
        && let Some(text) = run("hyprctl", &["-j", "monitors", "all"]).await
        && let Some(displays) = parse::hyprctl(&text)
    {
        return Some(("hyprctl", displays, true));
    }
    if wayland {
        // Only wlroots compositors answer; GNOME's and KDE's refuse.
        if let Some(text) = run("wlr-randr", &["--json"]).await
            && let Some(displays) = parse::wlr_randr(&text)
        {
            return Some(("wlr-randr", displays, true));
        }
        return None;
    }
    // On X11 only: under Wayland, xrandr sees Xwayland's outputs.
    if var("DISPLAY").is_some()
        && let Some(text) = run("xrandr", &["--query"]).await
    {
        let displays = parse::xrandr(&text);
        if !displays.is_empty() {
            return Some(("xrandr", displays, false));
        }
    }
    None
}

/// A tool's output, run on the host inside Irori's Flatpak.
async fn run(program: &str, args: &[&str]) -> Option<String> {
    let path = host::find_program(program).await?;
    let spec = host::command(CommandSpec::new(path).args(args.iter().copied()));
    let out = run_with_timeout(&spec, TOOL_TIMEOUT).await?;
    if !out.success() {
        tracing::debug!("{program}: {}", out.error_summary());
        return None;
    }
    Some(out.stdout)
}

/// The connected connectors under a `/sys/class/drm`-like folder
/// (`card1-DP-1`): their modes, and the EDID's name and HDR support.
pub(super) fn connectors(drm: &Path) -> Vec<Display> {
    let Ok(entries) = std::fs::read_dir(drm) else {
        return Vec::new();
    };
    let mut found: Vec<(String, Display)> = entries
        .flatten()
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            let (card, connector) = file.split_once('-')?;
            card.strip_prefix("card")?.parse::<u32>().ok()?;
            let dir = e.path();
            let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
            if read("status")?.trim() != "connected" {
                return None;
            }
            let edid = std::fs::read(dir.join("edid"))
                .ok()
                .and_then(|b| parse::edid(&b));
            let name = edid
                .as_ref()
                .and_then(|e| e.name.clone().or_else(|| e.manufacturer.clone()));
            Some((
                file.clone(),
                Display {
                    connector: connector.to_string(),
                    name,
                    primary: false,
                    enabled: read("enabled").is_none_or(|v| v.trim() != "disabled"),
                    current: None,
                    modes: read("modes")
                        .map(|m| parse::sysfs_modes(&m))
                        .unwrap_or_default(),
                    hdr_on: None,
                    hdr_capable: edid.map(|e| e.hdr),
                },
            ))
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found.into_iter().map(|(_, d)| d).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_connectors() {
        let dir = tempfile::tempdir().unwrap();
        let drm = dir.path();
        let add = |name: &str, status: &str, enabled: &str, modes: &str| {
            let d = drm.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("status"), format!("{status}\n")).unwrap();
            std::fs::write(d.join("enabled"), format!("{enabled}\n")).unwrap();
            std::fs::write(d.join("modes"), modes).unwrap();
        };
        add(
            "card1-DP-1",
            "connected",
            "enabled",
            "2560x1440\n1920x1080\n",
        );
        add("card1-HDMI-A-1", "disconnected", "disabled", "");
        add("card0-eDP-1", "connected", "disabled", "1920x1200\n");
        std::fs::create_dir_all(drm.join("card1")).unwrap();
        std::fs::create_dir_all(drm.join("renderD128")).unwrap();
        let mut edid = vec![0u8; 128];
        edid[..8].copy_from_slice(&[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
        std::fs::write(drm.join("card1-DP-1/edid"), &edid).unwrap();

        let found = connectors(drm);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].connector, "eDP-1");
        assert!(!found[0].enabled);
        assert_eq!(found[0].hdr_capable, None, "no EDID");
        assert_eq!(found[1].connector, "DP-1");
        assert!(found[1].enabled);
        assert_eq!(found[1].modes.len(), 2);
        assert_eq!(found[1].hdr_capable, Some(false));
        assert!(connectors(&drm.join("missing")).is_empty());
    }
}
