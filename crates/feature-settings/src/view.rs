//! The application settings page (`Route::Settings`).

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AnyView, AppContext as _, ClipboardItem, Context, ElementId, Entity,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, div, rems,
};
use irori_core::Paths;
use irori_runtime::sysinfo::{self, SystemInfo};
use irori_services::displays::DisplayReport;
use irori_ui::components::{EmptyState, PageHeader};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models, nav};

use crate::app_update::AppUpdatePanel;
use crate::appearance::AppearancePreview;
use crate::backup::BackupPanel;
use crate::feedback::{self, Feedback};
use crate::form::{FormTarget, SettingsForm};
use crate::hud::{HudPanel, HudPanelScope};
use crate::i18n::{LanguageStamp, english};
use crate::logs::LogViewer;
use crate::registry::FieldSection;
use crate::storage::StoragePanel;
use crate::widgets::{
    group_header, group_heading, muted, nav_button_with_dot, panel, sidebar_section_label,
};

/// Sections of the settings page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsSection {
    Accounts,
    WineManager,
    Emulators,
    General,
    Controller,
    GameDefaults,
    PerformanceHud,
    Appearance,
    Storage,
    Advanced,
    Backup,
    Logs,
    SystemInfo,
    About,
}

impl SettingsSection {
    pub const ALL: [SettingsSection; 14] = [
        SettingsSection::Accounts,
        SettingsSection::WineManager,
        SettingsSection::Emulators,
        SettingsSection::General,
        SettingsSection::Controller,
        SettingsSection::GameDefaults,
        SettingsSection::PerformanceHud,
        SettingsSection::Appearance,
        SettingsSection::Storage,
        SettingsSection::Advanced,
        SettingsSection::Backup,
        SettingsSection::Logs,
        SettingsSection::SystemInfo,
        SettingsSection::About,
    ];

    /// Whether this settings section is available on the current host.
    pub fn supported_on_host(self) -> bool {
        match self {
            SettingsSection::PerformanceHud => irori_core::HudBackend::current().is_supported(),
            SettingsSection::WineManager => !cfg!(windows),
            _ => true,
        }
    }

    /// Whether this section hosts an embedded management view that owns its
    /// own page header, layout width and scrolling.
    fn is_embedded(self) -> bool {
        matches!(
            self,
            SettingsSection::Accounts | SettingsSection::WineManager | SettingsSection::Emulators
        )
    }

    /// Route id of the section.
    pub fn id(self) -> &'static str {
        match self {
            SettingsSection::Accounts => "accounts",
            SettingsSection::WineManager => "wine-manager",
            SettingsSection::Emulators => "emulators",
            SettingsSection::General => "general",
            SettingsSection::Controller => "controller",
            SettingsSection::GameDefaults => "game-defaults",
            SettingsSection::PerformanceHud => "hud",
            SettingsSection::Appearance => "appearance",
            SettingsSection::Storage => "storage",
            SettingsSection::Advanced => "advanced",
            SettingsSection::Backup => "backup",
            SettingsSection::Logs => "logs",
            SettingsSection::SystemInfo => "system",
            SettingsSection::About => "about",
        }
    }

    /// Parse a route id; accepts Heroic's route names. Unknown or unsupported
    /// ids open General.
    pub fn from_id(id: &str) -> Self {
        let parsed = match id {
            "accounts" | "account" | "login" => SettingsSection::Accounts,
            "wine-manager" | "wine_manager" | "proton" => SettingsSection::WineManager,
            "emulators" | "emulation" | "roms" => SettingsSection::Emulators,
            "game-defaults" | "games" | "games_settings" | "wine" | "defaults" => {
                SettingsSection::GameDefaults
            }
            "controller" | "controllers" | "gamepad" | "buttons" => SettingsSection::Controller,
            "hud" | "mangohud" | "metal-hud" | "performance-hud" => SettingsSection::PerformanceHud,
            "appearance" | "accessibility" | "theme" => SettingsSection::Appearance,
            "storage" | "disks" | "cleanup" => SettingsSection::Storage,
            "advanced" => SettingsSection::Advanced,
            "backup" | "restore" | "export" | "sync" | "saves" | "ludusavi" | "save-backups" => {
                SettingsSection::Backup
            }
            "logs" | "log" => SettingsSection::Logs,
            "system" | "systeminfo" | "system-info" => SettingsSection::SystemInfo,
            "about" => SettingsSection::About,
            _ => SettingsSection::General,
        };
        if parsed.supported_on_host() {
            parsed
        } else {
            SettingsSection::General
        }
    }

    fn label_key(self) -> &'static str {
        match self {
            SettingsSection::Accounts => "settings.page.accounts",
            SettingsSection::WineManager => {
                if cfg!(target_os = "macos") {
                    "settings.page.wine_manager_macos"
                } else {
                    "settings.page.wine_manager"
                }
            }
            SettingsSection::Emulators => "settings.page.emulators",
            SettingsSection::General => "settings.page.general",
            SettingsSection::Controller => "settings.page.controller",
            SettingsSection::GameDefaults => "settings.page.game_defaults",
            SettingsSection::PerformanceHud => "settings.page.performance_hud",
            SettingsSection::Appearance => "settings.page.appearance",
            SettingsSection::Storage => "settings.page.storage",
            SettingsSection::Advanced => "settings.page.advanced",
            SettingsSection::Backup => "settings.page.backup",
            SettingsSection::Logs => "settings.page.logs",
            SettingsSection::SystemInfo => "settings.page.system_info",
            SettingsSection::About => "settings.page.about",
        }
    }

    /// Name of the section in the interface language.
    pub fn label(self) -> SharedString {
        t(self.label_key())
    }

    fn description(self) -> SharedString {
        let key = match self {
            SettingsSection::Accounts
            | SettingsSection::WineManager
            | SettingsSection::Emulators
            | SettingsSection::About => return SharedString::default(),
            SettingsSection::General => "settings.page.general_description",
            SettingsSection::Controller => "settings.page.controller_description",
            SettingsSection::GameDefaults => "settings.page.game_defaults_description",
            SettingsSection::PerformanceHud => match irori_core::HudBackend::current() {
                irori_core::HudBackend::MetalHud => {
                    "settings.page.performance_hud_description_metal"
                }
                _ => "settings.page.performance_hud_description_mangohud",
            },
            SettingsSection::Appearance => "settings.page.appearance_description",
            SettingsSection::Storage => "settings.page.storage_description",
            SettingsSection::Advanced => "settings.page.advanced_description",
            SettingsSection::Backup => "settings.page.backup_description",
            SettingsSection::Logs => "settings.page.logs_description",
            SettingsSection::SystemInfo => "settings.page.system_info_description",
        };
        t(key)
    }

    fn icon(self) -> IconName {
        match self {
            SettingsSection::Accounts => IconName::CircleUser,
            SettingsSection::WineManager => IconName::Wine,
            SettingsSection::Emulators => IconName::Joystick,
            SettingsSection::General => IconName::Settings,
            SettingsSection::Controller => IconName::Gamepad2,
            SettingsSection::GameDefaults => IconName::Gamepad2,
            SettingsSection::PerformanceHud => IconName::Gauge,
            SettingsSection::Appearance => IconName::Palette,
            SettingsSection::Storage => IconName::HardDrive,
            SettingsSection::Advanced => IconName::Wrench,
            SettingsSection::Backup => IconName::DatabaseBackup,
            SettingsSection::Logs => IconName::ScrollText,
            SettingsSection::SystemInfo => IconName::Cpu,
            SettingsSection::About => IconName::Info,
        }
    }

    /// English search terms for pages without fields.
    fn keywords(self) -> &'static str {
        match self {
            SettingsSection::Accounts => {
                "accounts login sign in logout store epic gog amazon zoom itch xbox steam friends"
            }
            SettingsSection::WineManager => {
                "wine proton ge-proton umu runner prefix prefixes bottle download version rollback"
            }
            SettingsSection::Emulators => {
                "emulators emulation rom roms bios retroarch cores retroachievements console systems"
            }
            SettingsSection::PerformanceHud => {
                "performance hud mangohud metal overlay fps frametime gpu cpu vram temperature"
            }
            SettingsSection::Logs => "logs log launcher debug report problem crash panic backtrace",
            SettingsSection::SystemInfo => {
                "system information hardware cpu gpu memory driver os report"
            }
            SettingsSection::About => "about version license credits",
            SettingsSection::Storage => {
                "storage disk drive space size free clean cleanup delete prefix cache"
            }
            SettingsSection::Backup => {
                "game saves save backups ludusavi snapshots cloud backup restore sync export csv json schedule automatic computers devices webdav rclone settings library reset defaults factory redefinir danger"
            }
            _ => "",
        }
    }

    fn field_section(self) -> Option<FieldSection> {
        match self {
            SettingsSection::General => Some(FieldSection::General),
            SettingsSection::Controller => Some(FieldSection::Controller),
            SettingsSection::Appearance => Some(FieldSection::Appearance),
            SettingsSection::Advanced => Some(FieldSection::Advanced),
            _ => None,
        }
    }
}

#[derive(Default)]
struct SysInfoState {
    info: Option<Arc<SystemInfo>>,
    /// Read with the rest; the displays' modes and HDR.
    displays: Option<Arc<DisplayReport>>,
    loading: bool,
    copied: bool,
}

/// The settings page: section navigation, a search field filtering every
/// field, and the selected section.
pub struct SettingsView {
    section: SettingsSection,
    general_tab: &'static str,
    defaults_tab: &'static str,
    appearance_tab: &'static str,
    search: Entity<InputState>,
    query: String,
    app_form: Entity<SettingsForm>,
    defaults_form: Entity<SettingsForm>,
    hud: Entity<HudPanel>,
    log: Entity<LogViewer>,
    crash_reports: Vec<irori_core::crash::CrashReportMeta>,
    backup: Entity<BackupPanel>,
    app_update: Entity<AppUpdatePanel>,
    preview: Entity<AppearancePreview>,
    storage: Entity<StoragePanel>,
    accounts_view: Option<AnyView>,
    wine_view: Option<AnyView>,
    emulators_view: Option<AnyView>,
    sysinfo: SysInfoState,
    /// A problem report is reading this computer's facts.
    reporting: bool,
    language: LanguageStamp,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    /// The page opened on `section` (a [`SettingsSection`] route id).
    pub fn new(section: SharedString, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("settings.common.search_settings"))
        });
        let sub = cx.subscribe_in(
            &search,
            window,
            |this, state, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    let q = state.read(cx).value().to_string();
                    this.set_query(q, window, cx);
                }
            },
        );
        let app_form = cx.new(|cx| {
            let mut form = SettingsForm::new(FormTarget::App, window, cx);
            form.set_compact(true, cx);
            form
        });
        let defaults_form = cx.new(|cx| {
            let mut form = SettingsForm::new(FormTarget::Defaults, window, cx);
            form.set_compact(true, cx);
            form
        });
        let hud = cx.new(|cx| {
            HudPanel::new(HudPanelScope::Global, None, window, cx).with_pinned_preview(true)
        });
        // The upload name identifies the log at the paste service and in the
        // upload history, so it stays the same in every language.
        let log =
            cx.new(|cx| LogViewer::new(Paths::get().launcher_log_file(), "Irori log", window, cx));
        let storage = cx.new(StoragePanel::new);
        let accounts = models(cx).accounts.clone();
        let default_defaults_tab = crate::layout::supported_defaults_tabs()
            .first()
            .map(|tab| tab.id)
            .unwrap_or("launch");
        let mut this = Self {
            section: SettingsSection::General,
            general_tab: "basics",
            defaults_tab: default_defaults_tab,
            appearance_tab: "theme",
            search,
            query: String::new(),
            app_form,
            defaults_form,
            hud,
            log,
            crash_reports: Vec::new(),
            backup: cx.new(|cx| BackupPanel::new(window, cx)),
            app_update: cx.new(AppUpdatePanel::new),
            preview: cx.new(AppearancePreview::new),
            storage: storage.clone(),
            accounts_view: None,
            wine_view: None,
            emulators_view: None,
            sysinfo: SysInfoState::default(),
            reporting: false,
            language: LanguageStamp::new(),
            // The header's Rescan button shows the measuring; accounts updates
            // the expired-login indicator on the Accounts section row.
            _subscriptions: vec![
                sub,
                cx.observe(&storage, |_, _, cx| cx.notify()),
                cx.observe(&accounts, |_, _, cx| cx.notify()),
            ],
        };
        this.open_route_id(&section, true, window, cx);
        this
    }

    /// The current section.
    pub fn section(&self) -> SettingsSection {
        self.section
    }

    /// The current search query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Show a section (route id), sub-tab (`backup/<tab>`), or direct setting
    /// (`field:<path>`); the shell calls this when the route changes while the
    /// page is open.
    pub fn set_section(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_route_id(id, false, window, cx);
    }

    fn open_route_id(
        &mut self,
        id: &str,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = id.strip_prefix("field:")
            && let Some(spec) = crate::registry::find(path)
            && spec.section == FieldSection::Saves
            && spec.supported_on_host()
        {
            self.open(SettingsSection::Backup, false, window, cx);
            self.backup
                .update(cx, |b, cx| b.reveal_field(spec.path, window, cx));
            return;
        }
        if id == "downloads" {
            self.general_tab = "integrations";
            self.open(SettingsSection::General, false, window, cx);
            return;
        }
        if let Some((sec, sub)) = id.split_once('/') {
            let target = SettingsSection::from_id(sec);
            if target == SettingsSection::Backup {
                let backup_tab = match sub {
                    "saves" | "ludusavi" | "save-backups" => Some(crate::backup::BackupTab::Saves),
                    "sync" => Some(crate::backup::BackupTab::Sync),
                    "library" | "backup_restore" | "backup" | "restore" | "export" => {
                        Some(crate::backup::BackupTab::Library)
                    }
                    _ => None,
                };
                if let Some(tab) = backup_tab {
                    self.backup.update(cx, |b, cx| b.set_tab(tab, cx));
                }
            }
            self.open(target, false, window, cx);
            return;
        }
        let alias_tab = match id {
            "saves" | "ludusavi" | "save-backups" => Some(crate::backup::BackupTab::Saves),
            "restore" | "export" => Some(crate::backup::BackupTab::Library),
            "sync" => Some(crate::backup::BackupTab::Sync),
            _ => None,
        };
        if let Some(tab) = alias_tab {
            self.backup.update(cx, |b, cx| b.set_tab(tab, cx));
            self.open(SettingsSection::Backup, false, window, cx);
            return;
        }
        let target = SettingsSection::from_id(id);
        if !force
            && target == self.section
            && self.query.is_empty()
            && (!target.is_embedded() || self.embedded_view(target).is_some())
        {
            return;
        }
        self.open(target, false, window, cx);
    }

    fn embedded_view(&self, section: SettingsSection) -> Option<AnyView> {
        match section {
            SettingsSection::Accounts => self.accounts_view.clone(),
            SettingsSection::WineManager => self.wine_view.clone(),
            SettingsSection::Emulators => self.emulators_view.clone(),
            _ => None,
        }
    }

    /// The shared application-fields form (General, Appearance, Advanced).
    pub fn app_form(&self) -> &Entity<SettingsForm> {
        &self.app_form
    }

    #[cfg(test)]
    pub(crate) fn search_input(&self) -> Entity<InputState> {
        self.search.clone()
    }

    /// The game defaults form.
    pub fn defaults_form(&self) -> &Entity<SettingsForm> {
        &self.defaults_form
    }

    #[cfg(test)]
    pub(crate) fn set_subtab_for_test(
        &mut self,
        tab: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_subtab(tab, window, cx);
    }

    #[cfg(test)]
    pub(crate) fn backup_panel(&self) -> &Entity<BackupPanel> {
        &self.backup
    }

    fn set_subtab(&mut self, tab: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        match self.section {
            SettingsSection::General => self.general_tab = tab,
            SettingsSection::GameDefaults => self.defaults_tab = tab,
            SettingsSection::Appearance => self.appearance_tab = tab,
            _ => return,
        }
        self.open(self.section, false, window, cx);
    }

    fn open(
        &mut self,
        section: SettingsSection,
        sync_nav: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let section = if section.supported_on_host() {
            section
        } else {
            SettingsSection::General
        };
        if self.section == SettingsSection::Logs && section != SettingsSection::Logs {
            self.log.update(cx, |l, _| l.stop());
        }
        self.section = section;
        if !self.query.is_empty() {
            self.query.clear();
            self.search.update(cx, |s, cx| s.set_value("", window, cx));
        }
        if let Some(fs) = section.field_section() {
            let subtab = match section {
                SettingsSection::General => Some(self.general_tab),
                SettingsSection::Appearance => Some(self.appearance_tab),
                _ => None,
            };
            self.app_form.update(cx, |f, cx| {
                f.set_sections(vec![fs], cx);
                f.set_subtab(subtab, cx);
            });
        }
        match section {
            SettingsSection::Accounts if self.accounts_view.is_none() => {
                self.accounts_view = irori_ui::hooks::accounts_view(window, cx);
            }
            SettingsSection::WineManager if self.wine_view.is_none() => {
                self.wine_view = irori_ui::hooks::wine_manager_view(window, cx);
            }
            SettingsSection::Emulators if self.emulators_view.is_none() => {
                self.emulators_view = irori_ui::hooks::emulators_view(window, cx);
            }
            SettingsSection::GameDefaults => {
                let fs = crate::layout::defaults_section_for_tab(self.defaults_tab);
                let tab = self.defaults_tab;
                self.defaults_form.update(cx, |f, cx| {
                    f.set_sections(vec![fs], cx);
                    f.set_subtab(Some(tab), cx);
                    f.load_wine_versions(window, cx);
                });
            }
            SettingsSection::Logs => {
                self.log.update(cx, |l, cx| l.start(window, cx));
                self.load_crash_reports(window, cx);
            }
            SettingsSection::SystemInfo if self.sysinfo.info.is_none() => {
                self.load_sysinfo(window, cx)
            }
            SettingsSection::Storage => {
                self.storage.update(cx, |s, cx| s.ensure_loaded(window, cx))
            }
            _ => {}
        }
        if sync_nav {
            let navigator = nav::navigator(cx);
            let target = nav::Route::Settings(section.id().into());
            let should_sync = {
                let current = navigator.read(cx).current();
                current.section() == "settings" && current != &target
            };
            if should_sync {
                navigator.update(cx, |n, cx| n.replace(target, cx));
            }
        }
        cx.notify();
    }

    fn set_query(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        if query.trim() == self.query.trim() {
            self.query = query;
            return;
        }
        self.query = query;
        if self.query.trim().is_empty() {
            let section = self.section;
            self.query.clear();
            self.open(section, false, window, cx);
            return;
        }
        let q = self.query.clone();
        self.app_form.update(cx, |f, cx| f.set_query(&q, cx));
        self.defaults_form.update(cx, |f, cx| {
            f.set_query(&q, cx);
            f.load_wine_versions(window, cx);
        });
        cx.notify();
    }

    fn load_sysinfo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sysinfo.loading {
            return;
        }
        self.sysinfo.loading = true;
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            Ok(tokio::join!(sysinfo::get(true), b.displays(true)))
        });
        cx.spawn_in(window, async move |this, cx| {
            let (info, displays) = task.await.ok().unzip();
            this.update(cx, |this, cx| {
                this.sysinfo.loading = false;
                this.sysinfo.info = info;
                this.sysinfo.displays = displays;
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Show this information instead of reading the computer's.
    #[cfg(test)]
    pub(crate) fn show_sysinfo_for_test(
        &mut self,
        info: Arc<SystemInfo>,
        displays: Arc<DisplayReport>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sysinfo.info = Some(info);
        self.sysinfo.displays = Some(displays);
        self.open(SettingsSection::SystemInfo, false, window, cx);
    }

    fn copy_sysinfo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(info) = &self.sysinfo.info else {
            return;
        };
        let mut text = sysinfo::format(info);
        if let Some(displays) = &self.sysinfo.displays {
            text.push('\n');
            text.push_str(&irori_services::displays::format(displays));
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.sysinfo.copied = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                this.sysinfo.copied = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let searching = !self.query.trim().is_empty();
        let accounts_expired = models(cx).accounts.read(cx).any_expired();
        let warning_dot = cx.theme().warning;
        let mut list = v_flex()
            .id("settings-nav-list")
            .flex_1()
            .min_h_0()
            .gap_0p5()
            .overflow_y_scrollbar();
        for (gx, group) in crate::layout::SIDEBAR_GROUPS.iter().enumerate() {
            let items: Vec<SettingsSection> = group
                .sections
                .iter()
                .copied()
                .filter(|s| s.supported_on_host())
                .collect();
            if items.is_empty() {
                continue;
            }
            list = list.child(sidebar_section_label(t(group.label_key), gx == 0, cx));
            for s in items {
                let dot =
                    (s == SettingsSection::Accounts && accounts_expired).then_some(warning_dot);
                list = list.child(nav_button_with_dot(
                    ElementId::Name(format!("settings-nav-{}", s.id()).into()),
                    s.icon(),
                    s.label(),
                    !searching && s == self.section,
                    dot,
                    cx.listener(move |this, _, window, cx| this.open(s, true, window, cx)),
                ));
            }
        }
        v_flex()
            .w(rems(15.))
            .h_full()
            .min_h_0()
            .flex_shrink_0()
            .p_3()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                Input::new(&self.search)
                    .id("settings-search")
                    .aria_label(t("settings.common.search_settings"))
                    .prefix(Icon::new(IconName::Search).small())
                    .cleanable(true)
                    .mb_2(),
            )
            .child(list)
    }

    fn render_subtabs(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, tabs, current): (&'static str, Vec<crate::layout::SubTab>, &'static str) =
            match self.section {
                SettingsSection::General => (
                    "settings-general-tabs",
                    crate::layout::GENERAL_TABS.to_vec(),
                    self.general_tab,
                ),
                SettingsSection::GameDefaults => (
                    "settings-defaults-tabs",
                    crate::layout::supported_defaults_tabs(),
                    self.defaults_tab,
                ),
                SettingsSection::Appearance => (
                    "settings-appearance-tabs",
                    crate::layout::APPEARANCE_TABS.to_vec(),
                    self.appearance_tab,
                ),
                _ => return None,
            };
        if tabs.len() <= 1 {
            return None;
        }
        let selected = tabs.iter().position(|tab| tab.id == current).unwrap_or(0);
        let tab_ids: Vec<&'static str> = tabs.iter().map(|tab| tab.id).collect();
        Some(
            div()
                .pb_2()
                .child(
                    TabBar::new(id)
                        .underline()
                        .large()
                        .selected_index(selected)
                        .children(tabs.iter().map(|tab| Tab::new().label(t(tab.label_key))))
                        .on_click(cx.listener(move |this, ix: &usize, window, cx| {
                            if let Some(&tab_id) = tab_ids.get(*ix) {
                                this.set_subtab(tab_id, window, cx);
                            }
                        })),
                )
                .into_any_element(),
        )
    }

    fn render_search(&self, cx: &mut Context<Self>) -> AnyElement {
        let q = self.query.trim().to_lowercase();
        let pages: Vec<SettingsSection> = SettingsSection::ALL
            .into_iter()
            .filter(|s| s.supported_on_host())
            .filter(|s| {
                let hay = format!("{} {} {}", s.label(), english(s.label_key()), s.keywords())
                    .to_lowercase();
                !s.keywords().is_empty() && q.split_whitespace().all(|t| hay.contains(t))
            })
            .collect();
        let count =
            self.app_form.read(cx).visible_count() + self.defaults_form.read(cx).visible_count();
        if count == 0 && pages.is_empty() {
            return EmptyState::new(
                IconName::Search,
                t_args("settings.common.no_match", &[("query", self.query.trim())]),
            )
            .description(t("settings.search.empty_hint"))
            .into_any_element();
        }
        v_flex()
            .w_full()
            .when(!pages.is_empty(), |v| {
                v.child(group_heading(t("settings.search.pages"), cx))
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .children(pages.into_iter().map(|s| {
                                Button::new(ElementId::Name(
                                    format!("search-page-{}", s.id()).into(),
                                ))
                                .debug_selector(move || format!("search-page-{}", s.id()))
                                .outline()
                                .icon(s.icon())
                                .label(s.label())
                                .on_click(cx.listener(
                                    move |this, _, window, cx| this.open(s, true, window, cx),
                                ))
                            })),
                    )
            })
            .child(self.app_form.clone())
            .child(self.defaults_form.clone())
            .into_any_element()
    }

    fn render_sysinfo(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(info) = &self.sysinfo.info else {
            return muted(
                if self.sysinfo.loading {
                    t("settings.sysinfo.loading")
                } else {
                    t("settings.sysinfo.unavailable")
                },
                cx,
            )
            .into_any_element();
        };
        let border = cx.theme().border;
        let muted_fg = cx.theme().muted_foreground;
        let opt = |v: &Option<String>| -> SharedString {
            v.clone()
                .map(SharedString::from)
                .unwrap_or_else(|| t("settings.sysinfo.not_installed"))
        };
        let mut hardware_rows: Vec<(SharedString, SharedString)> = vec![
            (
                t("settings.sysinfo.cpu"),
                format!("{} × {}", info.cpu.cores, info.cpu.model).into(),
            ),
            (
                t("settings.sysinfo.memory"),
                t_args(
                    "settings.sysinfo.memory_value",
                    &[
                        ("used", &irori_core::util::format_bytes(info.memory.used)),
                        ("total", &irori_core::util::format_bytes(info.memory.total)),
                    ],
                ),
            ),
        ];
        for (i, gpu) in info.gpus.iter().enumerate() {
            let label = if info.gpus.len() > 1 {
                t_args(
                    "settings.sysinfo.gpu_numbered",
                    &[("number", &(i + 1).to_string())],
                )
            } else {
                t("settings.sysinfo.gpu")
            };
            let name = match (&gpu.vendor_name, &gpu.device_name) {
                (Some(v), Some(d)) => format!("{v} {d}"),
                (None, Some(d)) => d.clone(),
                _ => format!("PCI {}:{}", gpu.vendor_id, gpu.device_id),
            };
            let driver: SharedString = match (&gpu.driver, &gpu.driver_version) {
                (Some(d), Some(v)) => format!("{d} {v}").into(),
                (Some(d), None) => d.clone().into(),
                _ => t("settings.sysinfo.no_driver"),
            };
            hardware_rows.push((label, format!("{name} · {driver}").into()));
        }
        if let Some(displays) = &self.sysinfo.displays {
            hardware_rows.extend(crate::displays::rows(displays));
        }
        let os = format!(
            "{} {}",
            info.os.name,
            info.os.version.as_deref().unwrap_or("")
        );
        let mut software_rows: Vec<(SharedString, SharedString)> = vec![(
            t("settings.sysinfo.os"),
            t_args(
                "settings.sysinfo.os_value",
                &[("os", os.trim()), ("kernel", &info.os.kernel)],
            ),
        )];
        if cfg!(target_os = "linux") {
            let desktop = info
                .desktop
                .clone()
                .map(SharedString::from)
                .unwrap_or_else(|| t("settings.sysinfo.unknown_desktop"));
            let session = info
                .session_type
                .clone()
                .map(SharedString::from)
                .unwrap_or_else(|| t("settings.sysinfo.unknown_session"));
            software_rows.push((
                t("settings.sysinfo.session"),
                format!(
                    "{desktop} · {session}{}",
                    if info.is_flatpak { " · Flatpak" } else { "" }
                )
                .into(),
            ));
            software_rows.push((
                "Vulkan".into(),
                if info.vulkan.available() {
                    info.vulkan
                        .instance_version
                        .clone()
                        .map(SharedString::from)
                        .unwrap_or_else(|| t("settings.sysinfo.available"))
                } else {
                    t("settings.sysinfo.not_available")
                },
            ));
        }
        let software = &info.software;
        software_rows.push(("Irori".into(), software.irori.clone().into()));
        for (name, v) in [
            ("Wine", &software.wine),
            ("gamescope", &software.gamescope),
            ("GameMode", &software.gamemode),
            ("MangoHud", &software.mangohud),
            ("umu-launcher", &software.umu),
        ] {
            if cfg!(target_os = "linux") || (name == "Wine" && !cfg!(windows)) {
                software_rows.push((name.into(), opt(v)));
            }
        }
        let render_table = |rows: Vec<(SharedString, SharedString)>| {
            rows.into_iter()
                .enumerate()
                .fold(panel(cx), |p, (i, (k, v))| {
                    p.child(
                        h_flex()
                            .gap_4()
                            .px_4()
                            .py_1p5()
                            .when(i > 0, |r| r.border_t_1().border_color(border))
                            .child(
                                div()
                                    .w_40()
                                    .flex_shrink_0()
                                    .min_w_0()
                                    .text_sm()
                                    .text_color(muted_fg)
                                    .child(k),
                            )
                            .child(div().flex_1().min_w_0().text_sm().child(v)),
                    )
                })
        };
        v_flex()
            .gap_2()
            .child(group_header(t("settings.sysinfo.hardware"), None, true, cx))
            .child(render_table(hardware_rows))
            .child(group_header(
                t("settings.sysinfo.software"),
                None,
                false,
                cx,
            ))
            .child(render_table(software_rows))
            .into_any_element()
    }

    fn open_feedback(&mut self, way: Feedback, window: &mut Window, cx: &mut Context<Self>) {
        if way == Feedback::Report {
            if self.reporting {
                return;
            }
            self.reporting = true;
            cx.notify();
        }
        let task = feedback::open(way, window, cx);
        cx.spawn(async move |this, cx| {
            task.await;
            this.update(cx, |this, cx| {
                this.reporting = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Help and feedback: one row per way to reach the project.
    fn render_feedback(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let border = cx.theme().border;
        let tour = h_flex()
            .gap_3()
            .px_4()
            .py_3()
            .child(Icon::new(IconName::Compass).text_color(cx.theme().muted_foreground))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .child(t("settings.about.tour")),
                    )
                    .child(muted(t("settings.about.tour_help"), cx)),
            )
            .child(
                Button::new("about-tour")
                    .outline()
                    .small()
                    .label(t("settings.about.tour"))
                    .on_click(|_, window, cx| irori_ui::tour::start(window, cx)),
            );
        Feedback::ALL
            .into_iter()
            .enumerate()
            .fold(panel(cx).child(tour), |list, (_, way)| {
                let button = if way == Feedback::Report {
                    "settings.about.feedback.report_open"
                } else {
                    "settings.about.feedback.open"
                };
                list.child(
                    h_flex()
                        .gap_3()
                        .px_4()
                        .py_3()
                        .border_t_1()
                        .border_color(border)
                        .child(Icon::new(way.icon()).text_color(cx.theme().muted_foreground))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_sm().font_medium().child(t(way.label_key())))
                                .child(muted(t(way.help_key()), cx)),
                        )
                        .child(
                            Button::new(ElementId::Name(
                                format!("feedback-{}", way.label_key()).into(),
                            ))
                            .outline()
                            .small()
                            .icon(IconName::ExternalLink)
                            .label(t(button))
                            .loading(way == Feedback::Report && self.reporting)
                            .on_click(cx.listener(
                                move |this, _, window, cx| this.open_feedback(way, window, cx),
                            )),
                        ),
                )
            })
    }

    fn render_about(&self, cx: &mut Context<Self>) -> AnyElement {
        let credits: [(SharedString, SharedString); 6] = [
            (t("settings.about.credit_gpui"), "Apache-2.0".into()),
            (t("settings.about.credit_lucide"), "ISC".into()),
            (t("settings.about.credit_themes"), "Apache-2.0".into()),
            ("legendary, gogdl, nile".into(), "GPL-3.0".into()),
            ("comet".into(), "Apache-2.0".into()),
            ("umu-launcher".into(), "GPL-3.0".into()),
        ];
        let border = cx.theme().border;
        v_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xl()
                            .font_semibold()
                            .child(format!("Irori {}", env!("CARGO_PKG_VERSION"))),
                    )
                    .child(muted(t("settings.about.tagline"), cx)),
            )
            .child(self.app_update.clone())
            .child(group_header(
                t("settings.about.feedback.title"),
                None,
                false,
                cx,
            ))
            .child(self.render_feedback(cx))
            .child(group_header(
                t("settings.about.third_party"),
                None,
                false,
                cx,
            ))
            .child(
                credits
                    .into_iter()
                    .enumerate()
                    .fold(panel(cx), |p, (ix, (name, license))| {
                        p.child(
                            h_flex()
                                .justify_between()
                                .gap_4()
                                .px_4()
                                .py_1p5()
                                .text_sm()
                                .when(ix > 0, |r| r.border_t_1().border_color(border))
                                .child(div().flex_1().min_w_0().child(name))
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(license),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }

    fn load_crash_reports(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let list = cx
                .background_executor()
                .spawn(async move { irori_core::crash::list_reports(&Paths::get().logs_dir()) })
                .await;
            this.update(cx, |this, cx| {
                this.crash_reports = list;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn delete_crash_report(
        &mut self,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            let result =
                cx.background_executor()
                    .spawn(async move {
                        irori_core::crash::delete_report(&Paths::get().logs_dir(), &path)
                    })
                    .await;
            this.update_in(cx, |this, window, cx| {
                if let Err(e) = result {
                    window.push_notification(
                        Notification::error(format!("{e}"))
                            .title(t("settings.crash.delete_failed")),
                        cx,
                    );
                } else {
                    this.load_crash_reports(window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn render_crash_reports(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.crash_reports.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let (border, muted_fg, warning) = (theme.border, theme.muted_foreground, theme.warning);
        let rows = self.crash_reports.iter().enumerate().map(|(ix, report)| {
            let when = crate::crash_report::format_crash_timestamp(report.timestamp)
                .unwrap_or_else(|| {
                    report
                        .path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("-")
                        .to_string()
                });
            let msg = report
                .summary
                .message
                .as_deref()
                .filter(|m| !m.trim().is_empty() && *m != "(no message)")
                .or(report.summary.location.as_deref())
                .unwrap_or("-");
            let open_path = report.path.clone();
            let reveal_path = report.path.clone();
            let delete_path = report.path.clone();
            h_flex()
                .gap_3()
                .px_3()
                .py_2()
                .items_center()
                .when(ix > 0, |r| r.border_t_1().border_color(border))
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .small()
                        .text_color(warning),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .truncate()
                                .child(msg.to_string()),
                        )
                        .child(div().text_xs().text_color(muted_fg).truncate().child(when)),
                )
                .child(
                    Button::new(ElementId::Name(format!("crash-review-{ix}").into()))
                        .outline()
                        .xsmall()
                        .label(t("settings.crash.review_action"))
                        .on_click(move |_, window, cx| {
                            crate::open_crash_report_sheet(open_path.clone(), window, cx);
                        }),
                )
                .child(
                    Button::new(ElementId::Name(format!("crash-show-{ix}").into()))
                        .ghost()
                        .xsmall()
                        .icon(IconName::FolderOpen)
                        .tooltip(t("settings.logs.show_file"))
                        .on_click(move |_, _, cx| {
                            if reveal_path.exists() {
                                cx.reveal_path(&reveal_path);
                            } else if let Some(dir) = reveal_path.parent() {
                                cx.open_with_system(dir);
                            }
                        }),
                )
                .child(
                    Button::new(ElementId::Name(format!("crash-delete-{ix}").into()))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Trash)
                        .tooltip(t("settings.crash.delete"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.delete_crash_report(delete_path.clone(), window, cx);
                        })),
                )
        });
        Some(
            v_flex()
                .gap_1()
                .child(group_header(
                    t("settings.crash.recent_title"),
                    Some(t("settings.crash.recent_help")),
                    true,
                    cx,
                ))
                .child(panel(cx).children(rows))
                .into_any_element(),
        )
    }

    fn header_actions(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match self.section {
            SettingsSection::Storage => Some(
                Button::new("storage-rescan")
                    .outline()
                    .icon(IconName::RefreshCw)
                    .label(t("settings.storage.rescan"))
                    .loading(self.storage.read(cx).loading())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.storage.update(cx, |s, cx| s.load(true, window, cx))
                    }))
                    .into_any_element(),
            ),
            SettingsSection::SystemInfo => Some(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("sysinfo-refresh")
                            .outline()
                            .icon(IconName::RefreshCw)
                            .label(t("settings.common.refresh"))
                            .loading(self.sysinfo.loading)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.load_sysinfo(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("sysinfo-copy")
                            .icon(if self.sysinfo.copied {
                                IconName::Check
                            } else {
                                IconName::Copy
                            })
                            .label(if self.sysinfo.copied {
                                t("settings.common.copied")
                            } else {
                                t("settings.common.copy")
                            })
                            .disabled(self.sysinfo.info.is_none())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.copy_sysinfo(window, cx)),
                            ),
                    )
                    .into_any_element(),
            ),
            _ => None,
        }
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.language.changed() {
            cx.defer_in(window, |this, window, cx| {
                let placeholder = t("settings.common.search_settings");
                this.search
                    .update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
            });
        }
        let searching = !self.query.trim().is_empty();
        let section = self.section;
        let content: AnyElement = if !searching && section.is_embedded() {
            match self.embedded_view(section) {
                Some(view) => div()
                    .id("settings-embedded-content")
                    .debug_selector(|| "settings-embedded-content".into())
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .h_full()
                    .child(view)
                    .into_any_element(),
                None => v_flex()
                    .id("settings-embedded-fallback")
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .p_6()
                    .child(PageHeader::new(section.label()))
                    .into_any_element(),
            }
        } else if !searching && section == SettingsSection::Logs {
            v_flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .gap_4()
                .p_6()
                .child(PageHeader::new(section.label()).description(section.description()))
                .children(self.render_crash_reports(cx))
                .child(div().flex_1().min_h_0().child(self.log.clone()))
                .into_any_element()
        } else if !searching && section == SettingsSection::PerformanceHud {
            v_flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .max_w(rems(52.))
                .p_5()
                .pb_4()
                .gap_2()
                .child(PageHeader::new(section.label()).description(section.description()))
                .child(div().flex_1().min_h_0().child(self.hud.clone()))
                .into_any_element()
        } else {
            let (title, description): (SharedString, SharedString) = if searching {
                (t("settings.common.search_results"), SharedString::default())
            } else {
                (section.label(), section.description())
            };
            let mut header = PageHeader::new(title);
            if !description.is_empty() {
                header = header.description(description);
            }
            if !searching && let Some(actions) = self.header_actions(cx) {
                header = header.action(actions);
            }
            let subtabs = (!searching).then(|| self.render_subtabs(cx)).flatten();
            let body: AnyElement = if searching {
                self.render_search(cx)
            } else {
                match section {
                    SettingsSection::Appearance => v_flex()
                        .when(self.appearance_tab != "accessibility", |v| {
                            v.child(self.preview.clone())
                        })
                        .child(self.app_form.clone())
                        .into_any_element(),
                    SettingsSection::General
                    | SettingsSection::Controller
                    | SettingsSection::Advanced => self.app_form.clone().into_any_element(),
                    SettingsSection::Storage => self.storage.clone().into_any_element(),
                    SettingsSection::GameDefaults => self.defaults_form.clone().into_any_element(),
                    SettingsSection::PerformanceHud => self.hud.clone().into_any_element(),
                    SettingsSection::Backup => self.backup.clone().into_any_element(),
                    SettingsSection::SystemInfo => self.render_sysinfo(cx),
                    SettingsSection::About => self.render_about(cx),
                    SettingsSection::Accounts
                    | SettingsSection::WineManager
                    | SettingsSection::Emulators
                    | SettingsSection::Logs => div().into_any_element(),
                }
            };
            div()
                .id("settings-content")
                .flex_1()
                .min_w_0()
                .min_h_0()
                .overflow_y_scrollbar()
                .child(
                    v_flex()
                        .w_full()
                        .max_w(rems(52.))
                        .p_5()
                        .pb_10()
                        .gap_2()
                        .child(header)
                        .children(subtabs)
                        .child(body),
                )
                .into_any_element()
        };
        h_flex()
            .id("settings")
            .size_full()
            .items_stretch()
            .child(self.render_nav(cx))
            .child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_ids_round_trip_and_accept_heroic_names() {
        for s in SettingsSection::ALL {
            if s.supported_on_host() {
                assert_eq!(SettingsSection::from_id(s.id()), s);
            } else {
                assert_eq!(SettingsSection::from_id(s.id()), SettingsSection::General);
            }
        }
        assert_eq!(
            SettingsSection::from_id("games_settings"),
            SettingsSection::GameDefaults
        );
        assert_eq!(
            SettingsSection::from_id("wine"),
            SettingsSection::GameDefaults
        );
        for alias in ["accounts", "account", "login"] {
            assert_eq!(SettingsSection::from_id(alias), SettingsSection::Accounts);
        }
        let expected_wine = if SettingsSection::WineManager.supported_on_host() {
            SettingsSection::WineManager
        } else {
            SettingsSection::General
        };
        for alias in ["wine-manager", "wine_manager", "proton"] {
            assert_eq!(SettingsSection::from_id(alias), expected_wine);
        }
        for alias in ["emulators", "emulation", "roms"] {
            assert_eq!(SettingsSection::from_id(alias), SettingsSection::Emulators);
        }
        let expected_hud = if SettingsSection::PerformanceHud.supported_on_host() {
            SettingsSection::PerformanceHud
        } else {
            SettingsSection::General
        };
        for alias in ["hud", "mangohud", "metal-hud", "performance-hud"] {
            assert_eq!(SettingsSection::from_id(alias), expected_hud);
        }
        assert_eq!(
            SettingsSection::from_id("systeminfo"),
            SettingsSection::SystemInfo
        );
        assert_eq!(SettingsSection::from_id("log"), SettingsSection::Logs);
        assert_eq!(
            SettingsSection::from_id("nonsense"),
            SettingsSection::General
        );
    }
}
