//! Grouping, plain-language copy, badges, and version-list compaction for the
//! Wine manager's downloadable Proton and Wine sources.

use gpui_kit::component::Sizable as _;
use gpui_kit::component::tag::Tag;
use gpui_kit::{ParentElement as _, SharedString};
use irori_runtime::manager::{Release, ReleaseKind};
use irori_ui::i18n::t;

/// Number of recent numbered versions shown before collapsing older ones.
pub(super) const INITIAL_VISIBLE_VERSIONS: usize = 8;

/// Category used to organize downloadable sources in the selector menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceGroup {
    Recommended,
    Specialized,
    Legacy,
}

impl SourceGroup {
    pub(super) const ALL: [Self; 3] = [Self::Recommended, Self::Specialized, Self::Legacy];

    pub(super) fn label(self) -> SharedString {
        match self {
            Self::Recommended => t("manage.wine.group.recommended"),
            Self::Specialized => t("manage.wine.group.specialized"),
            Self::Legacy => t("manage.wine.group.legacy"),
        }
    }
}

/// Short usage tag displayed next to a source title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SourceBadge {
    Recommended,
    VrChat,
    Xbox,
    Experimental,
    OlderGpus,
    Discontinued,
}

impl SourceBadge {
    pub(super) fn label(self) -> SharedString {
        match self {
            Self::Recommended => t("manage.wine.badge.recommended"),
            Self::VrChat => t("manage.wine.badge.vrchat"),
            Self::Xbox => t("manage.wine.badge.xbox"),
            Self::Experimental => t("manage.wine.badge.experimental"),
            Self::OlderGpus => t("manage.wine.badge.older_gpus"),
            Self::Discontinued => t("manage.wine.badge.discontinued"),
        }
    }

    pub(super) fn tag(self) -> Tag {
        let base = match self {
            Self::Recommended => Tag::primary(),
            Self::VrChat | Self::Xbox => Tag::info(),
            Self::Experimental => Tag::warning(),
            Self::OlderGpus | Self::Discontinued => Tag::secondary(),
        };
        base.xsmall().child(self.label())
    }
}

pub(super) fn source_group(kind: ReleaseKind) -> SourceGroup {
    match kind {
        ReleaseKind::GeProton
        | ReleaseKind::ProtonCachyOs
        | ReleaseKind::UmuProton
        | ReleaseKind::GamePortingToolkit
        | ReleaseKind::WineCrossover
        | ReleaseKind::WineStagingMac => SourceGroup::Recommended,
        ReleaseKind::DwProton
        | ReleaseKind::ProtonEm
        | ReleaseKind::ProtonWineland
        | ReleaseKind::ProtonGeRtsp
        | ReleaseKind::NexusRuntime => SourceGroup::Specialized,
        ReleaseKind::WineKron4ek | ReleaseKind::ProtonSarek | ReleaseKind::WineGe => {
            SourceGroup::Legacy
        }
    }
}

pub(super) fn source_badge(kind: ReleaseKind) -> Option<SourceBadge> {
    match kind {
        ReleaseKind::GeProton | ReleaseKind::ProtonCachyOs | ReleaseKind::GamePortingToolkit => {
            Some(SourceBadge::Recommended)
        }
        ReleaseKind::ProtonGeRtsp => Some(SourceBadge::VrChat),
        ReleaseKind::NexusRuntime => Some(SourceBadge::Xbox),
        ReleaseKind::ProtonEm | ReleaseKind::ProtonWineland => Some(SourceBadge::Experimental),
        ReleaseKind::ProtonSarek => Some(SourceBadge::OlderGpus),
        ReleaseKind::WineGe => Some(SourceBadge::Discontinued),
        ReleaseKind::DwProton
        | ReleaseKind::UmuProton
        | ReleaseKind::WineKron4ek
        | ReleaseKind::WineCrossover
        | ReleaseKind::WineStagingMac => None,
    }
}

pub(super) fn source_description(kind: ReleaseKind) -> SharedString {
    match kind {
        ReleaseKind::GeProton => t("manage.wine.source.ge_proton"),
        ReleaseKind::ProtonCachyOs => t("manage.wine.source.proton_cachyos"),
        ReleaseKind::ProtonWineland => t("manage.wine.source.proton_wineland"),
        ReleaseKind::WineGe => t("manage.wine.source.wine_ge"),
        ReleaseKind::WineKron4ek => t("manage.wine.source.wine_kron4ek"),
        ReleaseKind::NexusRuntime => t("manage.wine.source.nexus_runtime"),
        ReleaseKind::DwProton => t("manage.wine.source.dw_proton"),
        ReleaseKind::UmuProton => t("manage.wine.source.umu_proton"),
        ReleaseKind::ProtonEm => t("manage.wine.source.proton_em"),
        ReleaseKind::ProtonGeRtsp => t("manage.wine.source.proton_ge_rtsp"),
        ReleaseKind::ProtonSarek => t("manage.wine.source.proton_sarek"),
        ReleaseKind::WineCrossover => t("manage.wine.source.wine_crossover"),
        ReleaseKind::WineStagingMac => t("manage.wine.source.wine_staging_mac"),
        ReleaseKind::GamePortingToolkit => t("manage.wine.source.game_porting_toolkit"),
    }
}

/// Sources shown directly as quick-pick buttons. When three or fewer exist (macOS),
/// all of them fit without a secondary dropdown menu.
pub(super) fn quick_sources(available: &[ReleaseKind]) -> Vec<ReleaseKind> {
    if available.len() <= 3 {
        return available.to_vec();
    }
    [ReleaseKind::GeProton, ReleaseKind::ProtonCachyOs]
        .into_iter()
        .filter(|kind| available.contains(kind))
        .collect()
}

/// Available sources partitioned into non-empty groups for the "More builds" menu.
pub(super) fn grouped_sources(available: &[ReleaseKind]) -> Vec<(SourceGroup, Vec<ReleaseKind>)> {
    SourceGroup::ALL
        .into_iter()
        .filter_map(|group| {
            let items: Vec<ReleaseKind> = available
                .iter()
                .copied()
                .filter(|kind| source_group(*kind) == group)
                .collect();
            (!items.is_empty()).then_some((group, items))
        })
        .collect()
}

pub(super) fn is_latest_release(release: &Release) -> bool {
    release.version.ends_with("-latest")
}

/// Other downloadable sources whose name, badge, or description matches `query`.
pub(super) fn matching_sources(
    available: &[ReleaseKind],
    current: ReleaseKind,
    query: &str,
) -> Vec<ReleaseKind> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    available
        .iter()
        .copied()
        .filter(|&kind| kind != current)
        .filter(|&kind| {
            kind.type_str().to_lowercase().contains(&needle)
                || source_description(kind).to_lowercase().contains(&needle)
                || source_badge(kind).is_some_and(|b| b.label().to_lowercase().contains(&needle))
        })
        .collect()
}

/// Subset of releases to show for a source along with how many older builds are collapsed.
pub(super) struct VisibleReleases<'a> {
    pub(super) items: Vec<&'a Release>,
    pub(super) hidden_count: usize,
}

pub(super) fn visible_releases<'a>(
    list: &'a [Release],
    query: &str,
    show_older: bool,
) -> VisibleReleases<'a> {
    let eligible: Vec<&Release> = list
        .iter()
        .filter(|r| !r.download.is_empty() || r.installed())
        .collect();
    let needle = query.trim().to_lowercase();
    if !needle.is_empty() {
        let items = eligible
            .into_iter()
            .filter(|r| r.version.to_lowercase().contains(&needle))
            .collect();
        return VisibleReleases {
            items,
            hidden_count: 0,
        };
    }
    if show_older {
        return VisibleReleases {
            items: eligible,
            hidden_count: 0,
        };
    }

    let mut items = Vec::with_capacity(eligible.len().min(INITIAL_VISIBLE_VERSIONS + 1));
    let mut numbered_seen = 0usize;
    for release in &eligible {
        if is_latest_release(release) {
            items.push(*release);
        } else if numbered_seen < INITIAL_VISIBLE_VERSIONS {
            items.push(*release);
            numbered_seen += 1;
        } else if release.installed() {
            items.push(*release);
        }
    }
    let hidden_count = eligible.len().saturating_sub(items.len());
    VisibleReleases {
        items,
        hidden_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_release(version: &str, installed: bool) -> Release {
        let dir = installed.then(|| std::env::temp_dir().to_string_lossy().into_owned());
        Release {
            version: version.into(),
            kind: "GE-Proton".into(),
            download: format!("https://example.invalid/{version}.tar.gz"),
            is_installed: installed.then_some(true),
            install_dir: dir,
            ..Default::default()
        }
    }

    #[test]
    fn quick_and_grouped_sources_cover_linux_and_mac() {
        let linux = ReleaseKind::for_host(irori_core::HostPlatform::Linux);
        assert_eq!(
            quick_sources(linux),
            vec![ReleaseKind::GeProton, ReleaseKind::ProtonCachyOs]
        );
        let groups = grouped_sources(linux);
        assert_eq!(groups.len(), 3);
        let flattened: usize = groups.iter().map(|(_, items)| items.len()).sum();
        assert_eq!(flattened, linux.len());

        let mac = ReleaseKind::for_host(irori_core::HostPlatform::Mac);
        assert_eq!(quick_sources(mac), mac.to_vec());
    }

    #[test]
    fn visible_releases_keeps_latest_recent_and_installed_unless_expanded_or_searched() {
        let mut list = vec![fake_release("GE-Proton-latest", false)];
        for i in (1..=12).rev() {
            list.push(fake_release(&format!("GE-Proton10-{i}"), i == 2));
        }

        let compact = visible_releases(&list, "", false);
        // 1 latest + 8 recent + 1 older installed build (GE-Proton10-2)
        assert_eq!(compact.items.len(), 10);
        assert_eq!(compact.hidden_count, 3);
        assert!(
            compact
                .items
                .iter()
                .any(|r| r.version == "GE-Proton-latest")
        );
        assert!(compact.items.iter().any(|r| r.version == "GE-Proton10-2"));

        let expanded = visible_releases(&list, "", true);
        assert_eq!(expanded.items.len(), 13);
        assert_eq!(expanded.hidden_count, 0);

        let searched = visible_releases(&list, "10-1", false);
        assert_eq!(searched.hidden_count, 0);
        assert_eq!(
            searched
                .items
                .iter()
                .map(|r| r.version.as_str())
                .collect::<Vec<_>>(),
            vec![
                "GE-Proton10-12",
                "GE-Proton10-11",
                "GE-Proton10-10",
                "GE-Proton10-1"
            ]
        );
    }

    #[test]
    fn matching_sources_finds_other_sources_by_name_or_purpose() {
        crate::i18n::register();
        let linux = ReleaseKind::for_host(irori_core::HostPlatform::Linux);
        assert!(matching_sources(linux, ReleaseKind::GeProton, "   ").is_empty());

        let rtsp = matching_sources(linux, ReleaseKind::GeProton, "rtsp");
        assert!(rtsp.contains(&ReleaseKind::ProtonGeRtsp));
        assert!(!rtsp.contains(&ReleaseKind::GeProton));

        let vrchat = matching_sources(linux, ReleaseKind::GeProton, "vrchat");
        assert!(vrchat.contains(&ReleaseKind::ProtonGeRtsp));

        let xbox = matching_sources(linux, ReleaseKind::GeProton, "xbox");
        assert!(xbox.contains(&ReleaseKind::NexusRuntime));

        // Current source is never suggested again.
        assert!(
            !matching_sources(linux, ReleaseKind::ProtonGeRtsp, "rtsp")
                .contains(&ReleaseKind::ProtonGeRtsp)
        );
    }
}
