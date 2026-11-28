//! Settings pages, sub-tabs, and individual editable settings indexed into the
//! global command palette from `irori_settings` as a single source of truth.

use gpui_kit::App;
use irori_settings::SettingsSection;
use irori_settings::search_catalog::{searchable_settings, searchable_subtabs};
use irori_ui::Route;
use irori_ui::i18n::t;

use super::PaletteTarget;
use super::entry::Entry;
use super::score::EntryKind;
use super::view::Row;

/// Build palette rows for every host-supported Settings section, notable
/// sub-tab, and individual App / Game-defaults setting.
pub(super) fn rows(cx: &App) -> Vec<Row> {
    let settings_root_label = t("shell.nav.settings");
    let mut out = Vec::new();

    // Every Settings section in `SettingsSection::ALL` (single source of truth).
    for section in SettingsSection::ALL {
        if !section.supported_on_host() {
            continue;
        }
        let entry = Entry::new()
            .label(section.label())
            .english_label(section.english_label())
            .keywords([section.search_keywords()])
            .icon(section.icon())
            .kind(EntryKind::Page)
            .finish();
        out.push(Row {
            entry,
            target: PaletteTarget::Route(section.route()),
            game: None,
            about: None,
            detail: Some(settings_root_label.clone()),
            setting_path: None,
            help: None,
        });
    }

    // Notable Settings sub-tabs (Backup › Sync, Backup › Export, Replay, etc.).
    for sub in searchable_subtabs() {
        let entry = Entry::new()
            .label(sub.label)
            .english_label(sub.english_label)
            .keywords([
                sub.keywords.into(),
                sub.section.label(),
                sub.section.english_label().into(),
            ])
            .icon(sub.icon)
            .kind(EntryKind::Page)
            .finish();
        out.push(Row {
            entry,
            target: PaletteTarget::Route(Route::Settings(sub.route_id.into())),
            game: None,
            about: None,
            detail: Some(sub.breadcrumb),
            setting_path: None,
            help: None,
        });
    }

    // Every editable App and Game-defaults setting from the settings registry.
    for s in searchable_settings(cx) {
        let mut keywords: Vec<gpui_kit::SharedString> = Vec::with_capacity(s.keywords.len() + 5);
        keywords.push(s.section.label());
        keywords.push(s.section.english_label().into());
        keywords.push(s.group);
        keywords.push(s.english_group.into());
        keywords.push(s.path.into());
        for &kw in s.keywords {
            keywords.push(kw.into());
        }
        let help_for_preview = (!s.help.is_empty()).then(|| s.help.clone());
        let entry = Entry::new()
            .label(s.label)
            .english_label(s.english_label)
            .keywords(keywords)
            .help(s.help)
            .english_help(s.english_help)
            .icon(s.icon)
            .kind(EntryKind::Setting)
            .finish();
        out.push(Row {
            entry,
            target: PaletteTarget::OpenSetting(s.path),
            game: None,
            about: None,
            detail: Some(s.breadcrumb),
            setting_path: Some(s.path),
            help: help_for_preview,
        });
    }

    out
}
