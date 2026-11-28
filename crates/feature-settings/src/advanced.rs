//! Supporting banners for Settings -> Advanced: the live artwork source
//! summary strip and the cross-section navigation bar linking to Storage,
//! Logs, and Backup & sync.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex};
use gpui_kit::{
    AnyElement, App, ClickEvent, Div, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Stateful, Styled as _, Window, div,
};
use irori_core::settings::AppSettings;
use irori_ui::i18n::{t, t_args};

use crate::view::SettingsSection;

/// Total number of optional artwork and metadata sources configurable under
/// Settings -> Advanced -> Artwork & metadata.
pub(crate) const TOTAL_ARTWORK_SOURCES: usize = 5;

/// Callback used by cross-section footer links to jump to another settings page.
pub(crate) type OpenSectionFn = Rc<dyn Fn(SettingsSection, &mut Window, &mut App)>;

/// How many optional artwork and metadata sources are currently enabled or
/// configured in `app`.
pub(crate) fn count_active_artwork_sources(app: &AppSettings) -> usize {
    [
        !app.steam_grid_db_api_key.trim().is_empty(),
        app.igdb,
        app.steam_title_match,
        app.vndb,
        app.launchbox,
    ]
    .into_iter()
    .filter(|&on| on)
    .count()
}

/// Human-readable label for `AppSettings::art_cache_limit` (in megabytes).
pub(crate) fn art_cache_limit_label(limit_mb: u32) -> SharedString {
    match limit_mb {
        0 => t("settings.option.art_cache_limit.0"),
        512 => t("settings.option.art_cache_limit.512"),
        1024 => t("settings.option.art_cache_limit.1024"),
        2048 => t("settings.option.art_cache_limit.2048"),
        5120 => t("settings.option.art_cache_limit.5120"),
        10240 => t("settings.option.art_cache_limit.10240"),
        other if other >= 1024 && other.is_multiple_of(1024) => {
            format!("{} GB", other / 1024).into()
        }
        other => format!("{other} MB").into(),
    }
}

pub(crate) fn dot_stat(
    dot_color: gpui_kit::Hsla,
    label: SharedString,
    muted: bool,
    cx: &App,
) -> AnyElement {
    let text = if muted {
        div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(label)
    } else {
        div().text_xs().font_medium().child(label)
    };
    h_flex()
        .items_center()
        .gap_2()
        .child(div().flex_none().size_2().rounded_full().bg(dot_color))
        .child(text)
        .into_any_element()
}

pub(crate) fn summary_strip(id: &'static str, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    h_flex()
        .id(id)
        .w_full()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap_3()
        .px_4()
        .py_2p5()
        .rounded(theme.radius_lg)
        .bg(theme.secondary)
        .border_1()
        .border_color(theme.border)
}

/// Compact summary bar shown above the Artwork & metadata sub-tab: active
/// source count, current cache limit, and a 1-click shortcut to Storage.
pub(crate) fn render_artwork_summary(
    app: &AppSettings,
    on_open_storage: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let active = count_active_artwork_sources(app);
    let active_str = active.to_string();
    let total_str = TOTAL_ARTWORK_SOURCES.to_string();
    let limit = art_cache_limit_label(app.art_cache_limit);
    let theme = cx.theme();
    let dot_color = if active > 0 {
        theme.success
    } else {
        theme.muted_foreground
    };

    summary_strip("advanced-artwork-summary", cx)
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .flex_wrap()
                .items_center()
                .gap_x_4()
                .gap_y_1()
                .child(dot_stat(
                    dot_color,
                    t_args(
                        "settings.advanced.summary.sources",
                        &[("active", &active_str), ("total", &total_str)],
                    ),
                    false,
                    cx,
                ))
                .child(dot_stat(
                    theme.primary,
                    t_args(
                        "settings.advanced.summary.cache_limit",
                        &[("limit", &limit)],
                    ),
                    true,
                    cx,
                )),
        )
        .child(
            Button::new("advanced-open-storage")
                .outline()
                .small()
                .icon(IconName::HardDrive)
                .label(t("settings.advanced.manage_in_storage"))
                .on_click(on_open_storage),
        )
        .into_any_element()
}

/// Subtle footer bar at the bottom of Advanced linking directly to the
/// dedicated pages for cache cleanup, diagnostic logs, and factory reset.
pub(crate) fn render_moved_strip(on_open_section: OpenSectionFn, cx: &App) -> AnyElement {
    let muted_fg = cx.theme().muted_foreground;
    let targets = [
        (
            "advanced-goto-storage",
            SettingsSection::Storage,
            IconName::HardDrive,
            "settings.advanced.moved.storage",
        ),
        (
            "advanced-goto-logs",
            SettingsSection::Logs,
            IconName::ScrollText,
            "settings.advanced.moved.logs",
        ),
        (
            "advanced-goto-backup",
            SettingsSection::Backup,
            IconName::ArchiveRestore,
            "settings.advanced.moved.backup",
        ),
    ];

    summary_strip("advanced-moved-strip", cx)
        .mt_3()
        .gap_2()
        .child(
            div()
                .text_xs()
                .text_color(muted_fg)
                .child(t("settings.advanced.moved.label")),
        )
        .child(
            h_flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .children(targets.into_iter().map(|(id, section, icon, label_key)| {
                    let open = Rc::clone(&on_open_section);
                    Button::new(id)
                        .outline()
                        .xsmall()
                        .icon(icon)
                        .label(t(label_key))
                        .on_click(move |_, window, cx| open(section, window, cx))
                })),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_active_artwork_sources_accurately() {
        let mut app = AppSettings::default();
        assert_eq!(count_active_artwork_sources(&app), 0);

        app.steam_grid_db_api_key = "   ".into();
        assert_eq!(count_active_artwork_sources(&app), 0);

        app.steam_grid_db_api_key = "sgdb_key".into();
        app.igdb = true;
        app.steam_title_match = true;
        assert_eq!(count_active_artwork_sources(&app), 3);

        app.vndb = true;
        app.launchbox = true;
        assert_eq!(count_active_artwork_sources(&app), TOTAL_ARTWORK_SOURCES);
    }

    #[test]
    fn formats_art_cache_limit_labels() {
        crate::i18n::register();
        assert_eq!(art_cache_limit_label(0), "No limit");
        assert_eq!(art_cache_limit_label(512), "512 MB");
        assert_eq!(art_cache_limit_label(2048), "2 GB");
        assert_eq!(art_cache_limit_label(4096), "4 GB");
        assert_eq!(art_cache_limit_label(768), "768 MB");
    }
}
