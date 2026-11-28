//! Grouping of the Wine/Proton builds found on this system by where they
//! come from.

use std::path::{Path, PathBuf};

use gpui_kit::SharedString;
use irori_core::{WineInstallation, WineKind};
use irori_runtime::manager::Release;
use irori_ui::i18n::t;

/// Where a Wine or Proton build was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum WineOrigin {
    /// Downloaded through the Wine manager.
    Irori,
    /// Added by the user under "Custom paths".
    Custom,
    Steam,
    Lutris,
    Bottles,
    /// Installed by the distribution (`/usr`, `/opt`).
    System,
    Other,
}

impl WineOrigin {
    pub const ORDER: [WineOrigin; 7] = [
        WineOrigin::Irori,
        WineOrigin::Custom,
        WineOrigin::Steam,
        WineOrigin::Lutris,
        WineOrigin::Bottles,
        WineOrigin::System,
        WineOrigin::Other,
    ];

    pub fn label(self) -> SharedString {
        match self {
            WineOrigin::Irori => t("manage.wine.origin.irori"),
            WineOrigin::Custom => t("manage.wine.origin.custom"),
            WineOrigin::Steam => "Steam".into(),
            WineOrigin::Lutris => "Lutris".into(),
            WineOrigin::Bottles => "Bottles".into(),
            WineOrigin::System => t("manage.wine.origin.system"),
            WineOrigin::Other => t("manage.wine.origin.other"),
        }
    }
}

/// Quick origin filter chip for the Installed pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OriginFilter {
    All,
    Irori,
    Steam,
    Other,
}

impl OriginFilter {
    /// Whether a discovered build with `origin` passes this filter.
    pub(crate) fn matches_detected(self, origin: WineOrigin) -> bool {
        match self {
            Self::All => true,
            Self::Irori => origin == WineOrigin::Irori,
            Self::Steam => origin == WineOrigin::Steam,
            Self::Other => !matches!(origin, WineOrigin::Irori | WineOrigin::Steam),
        }
    }

    /// Whether Irori-managed releases pass this filter.
    pub(crate) fn includes_managed(self) -> bool {
        matches!(self, Self::All | Self::Irori)
    }
}

/// Classify a build from its binary path. `custom` are the user's custom
/// paths (settings), `own_dirs` Irori's download directories.
pub(crate) fn classify(bin: &str, custom: &[String], own_dirs: &[PathBuf]) -> WineOrigin {
    let path = Path::new(bin);
    if custom
        .iter()
        .filter(|c| !c.is_empty())
        .any(|c| path.starts_with(c) || bin == c.as_str())
    {
        return WineOrigin::Custom;
    }
    if own_dirs.iter().any(|d| path.starts_with(d)) {
        return WineOrigin::Irori;
    }
    let lower = bin.to_ascii_lowercase();
    if lower.contains("/steam/")
        || lower.contains("/.steam/")
        || lower.contains("steamapps")
        || lower.contains("compatibilitytools.d")
    {
        WineOrigin::Steam
    } else if lower.contains("/lutris/") {
        WineOrigin::Lutris
    } else if lower.contains("bottles") {
        WineOrigin::Bottles
    } else if bin.starts_with("/usr/") || bin.starts_with("/opt/") || bin.starts_with("/bin/") {
        WineOrigin::System
    } else {
        WineOrigin::Other
    }
}

/// "Wine", "Proton"…
pub(crate) fn kind_label(kind: WineKind) -> &'static str {
    match kind {
        WineKind::Wine => "Wine",
        WineKind::Proton => "Proton",
        WineKind::Crossover => "CrossOver",
        WineKind::Toolkit => "Game Porting Toolkit",
    }
}

/// Remove installations already represented by `managed` Irori releases so a
/// Proton build installed into Steam's `compatibilitytools.d` (or Irori's own
/// tools folder) is not listed twice.
pub(crate) fn exclude_managed(
    installs: &[WineInstallation],
    managed: &[Release],
) -> Vec<WineInstallation> {
    let dirs: Vec<&Path> = managed
        .iter()
        .filter_map(|r| r.install_dir.as_deref().filter(|d| !d.is_empty()))
        .map(Path::new)
        .collect();
    let bins: Vec<String> = managed
        .iter()
        .filter_map(|r| r.installation().map(|w| w.bin))
        .collect();
    installs
        .iter()
        .filter(|w| {
            let path = Path::new(&w.bin);
            !bins.iter().any(|b| b == &w.bin) && !dirs.iter().any(|d| path.starts_with(d))
        })
        .cloned()
        .collect()
}

/// Installations grouped by origin in display order; empty groups omitted.
pub(crate) fn group(
    installs: &[WineInstallation],
    custom: &[String],
    own_dirs: &[PathBuf],
) -> Vec<(WineOrigin, Vec<WineInstallation>)> {
    let mut groups: Vec<(WineOrigin, Vec<WineInstallation>)> =
        WineOrigin::ORDER.iter().map(|o| (*o, Vec::new())).collect();
    for w in installs {
        let origin = classify(&w.bin, custom, own_dirs);
        if let Some((_, list)) = groups.iter_mut().find(|(o, _)| *o == origin) {
            list.push(w.clone());
        }
    }
    for (_, list) in &mut groups {
        list.sort_by(|a, b| natural_desc(&a.name, &b.name));
    }
    groups.retain(|(_, list)| !list.is_empty());
    groups
}

/// Replace the user's home directory prefix with `~` for compact display.
pub(crate) fn shorten_home(path: &str) -> String {
    let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) else {
        return path.to_string();
    };
    let home = PathBuf::from(home);
    match Path::new(path).strip_prefix(&home) {
        Ok(rel) if rel.as_os_str().is_empty() => "~".to_string(),
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => path.to_string(),
    }
}

/// Newest-looking names first: compare digit runs numerically.
pub(crate) fn natural_desc(a: &str, b: &str) -> std::cmp::Ordering {
    natural_key(b).cmp(&natural_key(a))
}

fn natural_key(s: &str) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    let mut num = String::new();
    let mut text = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            if !text.is_empty() {
                out.push((0, std::mem::take(&mut text)));
            }
            num.push(c);
        } else {
            if !num.is_empty() {
                out.push((num.parse().unwrap_or(0), String::new()));
                num.clear();
            }
            text.push(c.to_ascii_lowercase());
        }
    }
    if !num.is_empty() {
        out.push((num.parse().unwrap_or(0), String::new()));
    }
    if !text.is_empty() {
        out.push((0, text));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(name: &str, bin: &str) -> WineInstallation {
        WineInstallation {
            name: name.into(),
            bin: bin.into(),
            kind: WineKind::Proton,
            ..Default::default()
        }
    }

    #[test]
    fn classifies_origins() {
        let own = vec![PathBuf::from("/home/u/.local/share/irori/tools")];
        let custom = vec!["/mnt/wine/my-build".to_string()];
        let c = |bin: &str| classify(bin, &custom, &own);
        assert_eq!(
            c("/home/u/.local/share/irori/tools/proton/GE-Proton9-20/proton"),
            WineOrigin::Irori
        );
        assert_eq!(c("/mnt/wine/my-build/bin/wine"), WineOrigin::Custom);
        assert_eq!(
            c("/home/u/.steam/steam/steamapps/common/Proton 9.0/proton"),
            WineOrigin::Steam
        );
        assert_eq!(
            c("/home/u/.steam/root/compatibilitytools.d/GE-Proton9/proton"),
            WineOrigin::Steam
        );
        assert_eq!(
            c("/home/u/.local/share/lutris/runners/wine/lutris-7/bin/wine"),
            WineOrigin::Lutris
        );
        assert_eq!(
            c("/home/u/.var/app/com.usebottles.bottles/data/bottles/runners/soda/bin/wine"),
            WineOrigin::Bottles
        );
        assert_eq!(c("/usr/bin/wine"), WineOrigin::System);
        assert_eq!(c("/srv/thing/wine"), WineOrigin::Other);
    }

    #[test]
    fn groups_in_order_newest_first() {
        let own = vec![PathBuf::from("/e")];
        let list = vec![
            w("Wine", "/usr/bin/wine"),
            w("GE-Proton9-9", "/e/proton/GE-Proton9-9/proton"),
            w("GE-Proton9-20", "/e/proton/GE-Proton9-20/proton"),
        ];
        let g = group(&list, &[], &own);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].0, WineOrigin::Irori);
        assert_eq!(g[0].1[0].name, "GE-Proton9-20");
        assert_eq!(g[0].1[1].name, "GE-Proton9-9");
        assert_eq!(g[1].0, WineOrigin::System);
    }

    #[test]
    fn excludes_managed_install_dirs_including_steam_compat_tools() {
        let managed = vec![Release {
            version: "GE-Proton-latest".into(),
            kind: "GE-Proton".into(),
            is_installed: Some(true),
            install_dir: Some("/home/u/.steam/root/compatibilitytools.d/GE-Proton-latest".into()),
            ..Default::default()
        }];
        let installs = vec![
            w(
                "GE-Proton-latest",
                "/home/u/.steam/root/compatibilitytools.d/GE-Proton-latest/proton",
            ),
            w(
                "Proton 9.0",
                "/home/u/.steam/steam/steamapps/common/Proton 9.0/proton",
            ),
        ];
        let remaining = exclude_managed(&installs, &managed);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "Proton 9.0");
    }

    #[test]
    fn origin_filter_splits_managed_and_detected_groups() {
        assert!(OriginFilter::All.includes_managed());
        assert!(OriginFilter::Irori.includes_managed());
        assert!(!OriginFilter::Steam.includes_managed());
        assert!(!OriginFilter::Other.includes_managed());
        assert!(OriginFilter::Steam.matches_detected(WineOrigin::Steam));
        assert!(!OriginFilter::Steam.matches_detected(WineOrigin::Lutris));
        assert!(OriginFilter::Other.matches_detected(WineOrigin::Lutris));
        assert!(OriginFilter::Other.matches_detected(WineOrigin::Custom));
        assert!(!OriginFilter::Other.matches_detected(WineOrigin::Steam));
    }
}
