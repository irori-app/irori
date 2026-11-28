//! Metadata exported from the settings registry and layout so the shell's
//! global search can index every host-supported setting and sub-tab without
//! duplicating field tables.

use gpui_kit::assets::IconName;
use gpui_kit::{App, SharedString};
use irori_ui::i18n::t;
use irori_ui::models;
use serde_json::Value;

use crate::i18n::english;
use crate::layout::{APPEARANCE_TABS, BACKUP_TABS, GENERAL_TABS, SubTab, subtab_of};
use crate::registry::{
    ActionKind, CustomKind, FIELDS, FieldKind, FieldSpec, PanelKind, Scope, TextKind, find,
};
use crate::value::{bool_at, get_path, value_to_text};
use crate::view::SettingsSection;

/// One editable App or Game-defaults setting exposed to global search.
#[derive(Debug, Clone)]
pub struct SettingSearchEntry {
    pub path: &'static str,
    pub label: SharedString,
    pub english_label: &'static str,
    pub section: SettingsSection,
    pub breadcrumb: SharedString,
    pub group: SharedString,
    pub english_group: &'static str,
    pub help: SharedString,
    pub english_help: &'static str,
    pub keywords: &'static [&'static str],
    pub icon: IconName,
}

/// One settings sub-tab exposed to global search (e.g. `Backup & sync › Export`).
#[derive(Debug, Clone)]
pub struct SubtabSearchEntry {
    pub route_id: &'static str,
    pub label: SharedString,
    pub english_label: &'static str,
    pub section: SettingsSection,
    pub breadcrumb: SharedString,
    pub keywords: &'static str,
    pub icon: IconName,
}

fn subtab_label_key(section: SettingsSection, tab_id: &str) -> Option<&'static str> {
    let tabs: &[SubTab] = match section {
        SettingsSection::General => GENERAL_TABS,
        SettingsSection::Appearance => APPEARANCE_TABS,
        SettingsSection::Backup => BACKUP_TABS,
        SettingsSection::GameDefaults => crate::layout::DEFAULTS_TABS,
        _ => return None,
    };
    tabs.iter().find(|t| t.id == tab_id).map(|t| t.label_key)
}

fn breadcrumb_for(section: SettingsSection, tab_id: Option<&str>) -> SharedString {
    let base = section.label();
    match tab_id.and_then(|id| subtab_label_key(section, id)) {
        Some(key) => format!("{base} › {}", t(key)).into(),
        None => base,
    }
}

/// Whether `spec` is currently relevant in application settings on this machine.
fn available_in_app(spec: &FieldSpec, epic_available: bool) -> bool {
    if !spec.supported_on_host() || spec.scope == Scope::GameOnly {
        return false;
    }
    if matches!(spec.kind, FieldKind::Action(ActionKind::ImportHeroic))
        && !irori_ui::hooks::heroic_detected()
    {
        return false;
    }
    if matches!(spec.kind, FieldKind::Action(ActionKind::ImportLutris))
        && !irori_ui::hooks::lutris_detected()
    {
        return false;
    }
    if matches!(spec.kind, FieldKind::Action(ActionKind::ImportBottles))
        && !irori_ui::hooks::bottles_detected()
    {
        return false;
    }
    if matches!(spec.kind, FieldKind::Action(ActionKind::ImportFaugus))
        && !irori_ui::hooks::faugus_detected()
    {
        return false;
    }
    match spec.path {
        "iroriLauncherImports.lutris" if !irori_ui::hooks::lutris_detected() => return false,
        "iroriLauncherImports.bottles" if !irori_ui::hooks::bottles_detected() => return false,
        "iroriLauncherImports.faugus" if !irori_ui::hooks::faugus_detected() => return false,
        _ => {}
    }
    if matches!(spec.kind, FieldKind::Action(ActionKind::ImportPlaynite))
        && !irori_extras::playnite::is_installed()
    {
        return false;
    }
    if matches!(
        spec.kind,
        FieldKind::Panel(PanelKind::EosOverlay | PanelKind::EglSync)
    ) && !epic_available
    {
        return false;
    }
    true
}

/// Every App and Shared setting available on the current host and environment.
pub fn searchable_settings(cx: &App) -> Vec<SettingSearchEntry> {
    let epic_available = crate::stores::epic(cx).is_some();
    FIELDS
        .iter()
        .filter(|spec| available_in_app(spec, epic_available))
        .map(|spec| {
            let section = SettingsSection::from_field_section(spec.section);
            let tab = subtab_of(spec);
            SettingSearchEntry {
                path: spec.path,
                label: spec.label_text(),
                english_label: english(spec.label),
                section,
                breadcrumb: breadcrumb_for(section, tab),
                group: spec.group_text(),
                english_group: english(spec.group),
                help: spec.help_text(),
                english_help: if spec.help.is_empty() {
                    ""
                } else {
                    english(spec.help)
                },
                keywords: spec.keywords,
                icon: section.icon(),
            }
        })
        .collect()
}

/// Notable settings sub-tabs indexed as direct navigation destinations.
pub fn searchable_subtabs() -> Vec<SubtabSearchEntry> {
    let mut out = Vec::new();
    let mut push = |route_id: &'static str,
                    section: SettingsSection,
                    label_key: &'static str,
                    keywords: &'static str| {
        if !section.supported_on_host() {
            return;
        }
        let label = t(label_key);
        let breadcrumb = format!("{} › {}", section.label(), label).into();
        out.push(SubtabSearchEntry {
            route_id,
            label,
            english_label: english(label_key),
            section,
            breadcrumb,
            keywords,
            icon: section.icon(),
        });
    };

    push(
        "general/window",
        SettingsSection::General,
        "settings.subtab.general.window",
        "window tray minimize autostart startup quick launch big picture",
    );
    push(
        "general/library",
        SettingsSection::General,
        "settings.subtab.general.library",
        "library stores achievements shortcuts deals xbox itch zoom",
    );
    push(
        "general/integrations",
        SettingsSection::General,
        "settings.subtab.general.integrations",
        "integrations downloads speed network offline discord sunshine gog comet",
    );
    push(
        "appearance/theme",
        SettingsSection::Appearance,
        "settings.subtab.appearance.theme",
        "theme dark light colors accent translucency font wallpaper",
    );
    push(
        "appearance/library",
        SettingsSection::Appearance,
        "settings.subtab.appearance.library",
        "library layout covers badges grid list focus background",
    );
    push(
        "appearance/accessibility",
        SettingsSection::Appearance,
        "settings.subtab.appearance.accessibility",
        "accessibility reduce motion animations middle click scroll",
    );
    push(
        "backup/sync",
        SettingsSection::Backup,
        "settings.subtab.backup.sync",
        "sync synchronize computers devices deck cloud folder",
    );
    push(
        "backup/export",
        SettingsSection::Backup,
        "settings.subtab.backup.export",
        "export csv json game list spreadsheet",
    );

    let supported_defaults = crate::layout::supported_defaults_tabs();
    let has_default_tab = |id: &str| supported_defaults.iter().any(|t| t.id == id);
    if has_default_tab("replay") {
        push(
            "game-defaults/replay",
            SettingsSection::GameDefaults,
            "settings.subtab.defaults.replay",
            "replay buffer clip record video capture screenshot gpu-screen-recorder",
        );
    }
    if has_default_tab("gpu") {
        push(
            "game-defaults/gpu",
            SettingsSection::GameDefaults,
            "settings.subtab.defaults.gpu",
            "graphics gpu card dlss fsr xess reflex frame generation smooth motion shader cache",
        );
    }
    out
}

/// Short read-only preview of a setting's current value for the palette preview
/// pane. Secrets are never revealed.
pub fn current_value_preview(path: &str, cx: &App) -> Option<SharedString> {
    let spec = find(path)?;
    let app = models(cx).settings.read(cx).app.clone();
    let snapshot = serde_json::to_value(&app).ok()?;
    match spec.kind {
        FieldKind::Toggle => Some(if bool_at(&snapshot, spec.path) {
            t("settings.preview.on")
        } else {
            t("settings.preview.off")
        }),
        FieldKind::Choice(options) => {
            let val = get_path(&snapshot, spec.path).unwrap_or(&Value::Null);
            options
                .iter()
                .find(|o| o.value.matches(val))
                .map(|o| o.label.text())
        }
        FieldKind::Text(TextKind::Secret) => None,
        FieldKind::Text(_) => {
            let text = value_to_text(get_path(&snapshot, spec.path));
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string().into())
        }
        FieldKind::Custom(kind) => {
            let val = get_path(&snapshot, spec.path);
            match kind {
                CustomKind::FontSize => {
                    let v = val.and_then(Value::as_f64)?;
                    Some(format!("{:.0}%", v / 16.0 * 100.0).into())
                }
                CustomKind::TileSize => {
                    let v = val.and_then(Value::as_f64)?;
                    Some(format!("{v:.1} rem").into())
                }
                CustomKind::Percent => {
                    let v = val.and_then(Value::as_f64)?;
                    Some(format!("{v:.0}%").into())
                }
                CustomKind::ThemeName | CustomKind::FontFamily => {
                    let s = val.and_then(Value::as_str).unwrap_or_default().trim();
                    if s.is_empty() {
                        Some(t("settings.preview.default"))
                    } else {
                        Some(s.to_string().into())
                    }
                }
                CustomKind::AccentColor | CustomKind::GpuCard | CustomKind::HiddenGpu => {
                    let s = val.and_then(Value::as_str).unwrap_or_default().trim();
                    (!s.is_empty()).then(|| s.to_string().into())
                }
                CustomKind::WineVersion => {
                    let name = val
                        .and_then(|v| v.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .trim();
                    (!name.is_empty()).then(|| name.to_string().into())
                }
                _ => None,
            }
        }
        FieldKind::Action(_) | FieldKind::Panel(_) => None,
    }
}
