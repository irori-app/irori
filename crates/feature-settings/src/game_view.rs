//! Settings of one game (`Route::GameSettings`). Its first section,
//! "Changed", lists only what the game changes from the game defaults, each
//! with its own reset.

use std::collections::HashSet;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, ElementId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Subscription, Window,
    div, rems,
};
use irori_core::{GameId, InstallPlatform, OpContext, Runner};
use irori_runtime::{DxTool, Target};
use irori_ui::components::{EmptyState, PageHeader};
use irori_ui::hooks::CloseFn;
use irori_ui::i18n::{t, t_args};
use irori_ui::{Route, backend, models};

use crate::form::{FormTarget, SettingsForm, info_alert};
use crate::graphics_mods::GraphicsModsChanged;
use crate::i18n::LanguageStamp;
use crate::logs::LogViewer;
use crate::registry::FieldSection;
use crate::renodx::RenodxPanel;
use crate::reshade::ReshadePanel;
use crate::steam_compat::SteamCompatPanel;
use crate::widgets::{RowState, field_row, muted, nav_button};
use crate::winetricks::WinetricksPanel;

/// Sections of a game's settings page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameSection {
    /// What the game changes from the defaults, from every section.
    Changed,
    Wine,
    Performance,
    Gpu,
    Launch,
    CloudSaves,
    Artwork,
    Tools,
    Log,
}

impl GameSection {
    pub const ALL: [GameSection; 9] = [
        GameSection::Changed,
        GameSection::Wine,
        GameSection::Performance,
        GameSection::Gpu,
        GameSection::Launch,
        GameSection::CloudSaves,
        GameSection::Artwork,
        GameSection::Tools,
        GameSection::Log,
    ];

    /// Route id of the section.
    pub fn id(self) -> &'static str {
        match self {
            GameSection::Changed => "changed",
            GameSection::Wine => "wine",
            GameSection::Performance => "performance",
            GameSection::Gpu => "gpu",
            GameSection::Launch => "launch",
            GameSection::CloudSaves => "saves",
            GameSection::Artwork => "art",
            GameSection::Tools => "tools",
            GameSection::Log => "log",
        }
    }

    /// Parse a route id. Unknown ids open the first section that applies.
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "changed" | "overrides" => GameSection::Changed,
            "wine" | "proton" => GameSection::Wine,
            "performance" | "other" | "optiscaler" | "replay" => GameSection::Performance,
            "gpu" | "graphics" | "frame-generation" => GameSection::Gpu,
            "art" | "artwork" | "metadata" => GameSection::Artwork,
            "launch" | "advanced" | "actions" => GameSection::Launch,
            "saves" | "sync" | "cloud" => GameSection::CloudSaves,
            "tools" | "reshade" | "renodx" | "hdr" => GameSection::Tools,
            "log" | "logs" => GameSection::Log,
            _ => return None,
        })
    }

    /// Name of the section in the interface language.
    pub fn label(self) -> SharedString {
        t(match self {
            GameSection::Changed => "settings.game.section.changed",
            GameSection::Wine => "settings.game.section.wine",
            GameSection::Performance => "settings.game.section.performance",
            GameSection::Gpu => "settings.game.section.gpu",
            GameSection::Artwork => "settings.game.section.artwork",
            GameSection::Launch => "settings.game.section.launch",
            GameSection::CloudSaves => "settings.game.section.saves",
            GameSection::Tools => "settings.game.section.tools",
            GameSection::Log => "settings.game.section.log",
        })
    }

    fn icon(self) -> IconName {
        match self {
            GameSection::Changed => IconName::RotateCcw,
            GameSection::Wine => IconName::Wine,
            GameSection::Performance => IconName::Gauge,
            GameSection::Gpu => IconName::Gpu,
            GameSection::Artwork => IconName::Images,
            GameSection::Launch => IconName::Rocket,
            GameSection::CloudSaves => IconName::Cloud,
            GameSection::Tools => IconName::Hammer,
            GameSection::Log => IconName::ScrollText,
        }
    }

    fn field_section(self) -> Option<FieldSection> {
        match self {
            GameSection::Wine => Some(FieldSection::Wine),
            GameSection::Performance => Some(FieldSection::Performance),
            GameSection::Gpu => Some(FieldSection::Gpu),
            GameSection::Artwork => Some(FieldSection::Artwork),
            GameSection::Launch => Some(FieldSection::Launch),
            GameSection::CloudSaves => Some(FieldSection::CloudSaves),
            _ => None,
        }
    }
}

/// Prefix tools offered for Windows games.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Tool {
    Winecfg,
    Winetricks,
    RunExe,
    KillPrefix,
    /// Install (`true`) or remove a DLL tool in the prefix.
    Dx(DxTool, bool),
}

/// Settings of one game: the shared form bound to the game's overrides,
/// prefix tools and the game's log.
pub struct GameSettingsView {
    id: GameId,
    section: GameSection,
    performance_tab: &'static str,
    search: Entity<InputState>,
    query: String,
    form: Entity<SettingsForm>,
    log: Entity<LogViewer>,
    winetricks: Entity<WinetricksPanel>,
    reshade: Entity<ReshadePanel>,
    renodx: Entity<RenodxPanel>,
    /// The compatibility tool choice of a Steam game, on Linux: Steam runs
    /// it, so Irori's Wine settings don't apply.
    steam_compat: Option<Entity<SteamCompatPanel>>,
    running: HashSet<Tool>,
    language: LanguageStamp,
    /// Set when another screen shows the view in place: Back calls it, and
    /// nothing leads off that screen.
    on_close: Option<CloseFn>,
    _subscriptions: Vec<Subscription>,
}

impl GameSettingsView {
    /// Settings of `id`, opened on `section` (a [`GameSection`] route id).
    pub fn new(
        id: GameId,
        section: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t("settings.common.search_settings"))
        });
        let sub = cx.subscribe_in(
            &search,
            window,
            |this, state, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.query = state.read(cx).value().to_string();
                    this.apply_filter(window, cx);
                }
            },
        );
        let form = cx.new(|cx| SettingsForm::new(FormTarget::Game(id.clone()), window, cx));
        form.update(cx, |f, cx| f.load_wine_versions(window, cx));
        let optiscaler = form.read(cx).optiscaler_panel();
        let b = backend(cx);
        let log_path = b.game_log(&id);
        // The upload name identifies the log at the paste service and in the
        // upload history, so it stays the same in every language.
        let log_name = format!(
            "Log of {}",
            b.game(&id)
                .map(|g| g.title.clone())
                .unwrap_or_else(|| id.app_name.to_string())
        );
        let log = cx.new(|cx| LogViewer::new(log_path, log_name, window, cx));
        let winetricks = {
            let id = id.clone();
            cx.new(|cx| WinetricksPanel::new(id, window, cx))
        };
        let reshade = {
            let id = id.clone();
            cx.new(|cx| ReshadePanel::new(id, cx))
        };
        let renodx = {
            let id = id.clone();
            cx.new(|cx| RenodxPanel::new(id, window, cx))
        };
        // RenoDX installs ReShade's build with add-on support, removing ReShade
        // removes RenoDX, and graphics mods may share or release proxy DLL slots:
        // each panel reads the others' changes.
        let opti_for_reshade = optiscaler.clone();
        let opti_for_renodx = optiscaler.clone();
        let mut mods_changed = vec![
            cx.subscribe_in(
                &reshade,
                window,
                move |this, _, _: &GraphicsModsChanged, window, cx| {
                    this.renodx.update(cx, |r, cx| r.refresh(window, cx));
                    if let Some(opti) = &opti_for_reshade {
                        opti.update(cx, |o, cx| o.refresh(window, cx));
                    }
                },
            ),
            cx.subscribe_in(
                &renodx,
                window,
                move |this, _, _: &GraphicsModsChanged, window, cx| {
                    this.reshade.update(cx, |r, cx| r.refresh(window, cx));
                    if let Some(opti) = &opti_for_renodx {
                        opti.update(cx, |o, cx| o.refresh(window, cx));
                    }
                },
            ),
        ];
        if let Some(opti) = optiscaler {
            mods_changed.push(cx.subscribe_in(
                &opti,
                window,
                |this, _, _: &GraphicsModsChanged, window, cx| {
                    this.reshade.update(cx, |r, cx| r.refresh(window, cx));
                    this.renodx.update(cx, |r, cx| r.refresh(window, cx));
                },
            ));
        }
        // Steam Play exists on Linux only.
        let steam_compat = (cfg!(target_os = "linux") && id.runner == Runner::Steam)
            .then(|| crate::stores::steam(cx))
            .flatten()
            .map(|steam| {
                let id = id.clone();
                cx.new(|cx| SteamCompatPanel::new(id, steam, window, cx))
            });
        let library = models(cx).library.clone();
        let observe = cx.observe(&library, |_, _, cx| cx.notify());
        let initial_perf_tab = if section.as_ref() == "replay"
            && crate::layout::supported_game_performance_tabs()
                .iter()
                .any(|t| t.id == "replay")
        {
            "replay"
        } else {
            "performance"
        };
        let mut this = Self {
            id,
            section: GameSection::Wine,
            performance_tab: initial_perf_tab,
            search,
            query: String::new(),
            form,
            log,
            winetricks,
            reshade,
            renodx,
            steam_compat,
            running: HashSet::new(),
            language: LanguageStamp::new(),
            on_close: None,
            _subscriptions: [sub, observe].into_iter().chain(mods_changed).collect(),
        };
        let initial = GameSection::from_id(&section).unwrap_or_else(|| this.default_section(cx));
        this.open(initial, window, cx);
        this
    }

    /// Settings of `id` shown in place by another screen (Big Picture), on
    /// the game's first section: Back calls `on_close`, and the link to the
    /// game defaults, which would leave that screen, is left out.
    pub fn embedded(
        id: GameId,
        on_close: CloseFn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::new(id, SharedString::default(), window, cx);
        this.on_close = Some(on_close);
        this
    }

    /// The game whose settings are shown.
    pub fn game_id(&self) -> &GameId {
        &self.id
    }

    /// The shared form bound to this game.
    pub fn form(&self) -> &Entity<SettingsForm> {
        &self.form
    }

    /// Show a section (route id) without recreating the view.
    pub fn set_section(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if id == "replay"
            && crate::layout::supported_game_performance_tabs()
                .iter()
                .any(|t| t.id == "replay")
        {
            self.performance_tab = "replay";
        } else if id == "optiscaler" || id == "performance" {
            self.performance_tab = "performance";
        }
        let section = GameSection::from_id(id).unwrap_or_else(|| self.default_section(cx));
        self.open(section, window, cx);
    }

    fn set_performance_tab(
        &mut self,
        tab: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.performance_tab == tab {
            return;
        }
        self.performance_tab = tab;
        if self.section == GameSection::Performance {
            if tab == "performance" {
                self.activate_optiscaler(window, cx);
            }
            self.apply_filter(window, cx);
        }
    }

    fn activate_optiscaler(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(opti) = self.form.read(cx).optiscaler_panel() {
            opti.update(cx, |o, cx| o.activate(window, cx));
        }
    }

    fn render_performance_tabs(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tabs = crate::layout::supported_game_performance_tabs();
        if tabs.len() <= 1 {
            return None;
        }
        let selected = tabs
            .iter()
            .position(|tab| tab.id == self.performance_tab)
            .unwrap_or(0);
        let tab_ids: Vec<&'static str> = tabs.iter().map(|tab| tab.id).collect();
        Some(
            div()
                .pb_2()
                .child(
                    TabBar::new("game-performance-tabs")
                        .underline()
                        .large()
                        .selected_index(selected)
                        .children(tabs.iter().map(|tab| Tab::new().label(t(tab.label_key))))
                        .on_click(cx.listener(move |this, ix: &usize, window, cx| {
                            if let Some(&tab_id) = tab_ids.get(*ix) {
                                this.set_performance_tab(tab_id, window, cx);
                            }
                        })),
                )
                .into_any_element(),
        )
    }

    fn platform(&self, cx: &Context<Self>) -> Option<InstallPlatform> {
        backend(cx)
            .game(&self.id)
            .and_then(|g| g.installed_platform())
    }

    fn uses_wine(&self, cx: &Context<Self>) -> bool {
        self.platform(cx).is_none_or(InstallPlatform::needs_wine)
    }

    /// The sections that apply to the game. Steam starts its games itself,
    /// so a Steam game has no Wine, graphics or prefix settings, and no game
    /// log; Wine & Proton becomes its Steam Play choice (Linux), Performance
    /// keeps the replay buffer (Linux), in-game stats (Linux and macOS), and
    /// OptiScaler (Windows builds), and Tools is shown for a Windows build
    /// (ReShade and RenoDX).
    fn sections(&self, cx: &Context<Self>) -> Vec<GameSection> {
        if self.id.runner != Runner::Steam {
            return GameSection::ALL.to_vec();
        }
        let graphics_mods = backend(cx)
            .game(&self.id)
            .is_some_and(|g| irori_services::supports_reshade(&g));
        let has_performance = cfg!(any(target_os = "linux", target_os = "macos")) || graphics_mods;
        GameSection::ALL
            .into_iter()
            .filter(|s| match s {
                GameSection::Changed => true,
                GameSection::Wine => self.steam_compat.is_some(),
                GameSection::Performance => has_performance,
                GameSection::Launch | GameSection::CloudSaves | GameSection::Artwork => true,
                GameSection::Tools => graphics_mods,
                GameSection::Gpu | GameSection::Log => false,
            })
            .collect()
    }

    fn label_of(&self, section: GameSection) -> SharedString {
        if section == GameSection::Wine && self.steam_compat.is_some() {
            t("settings.steam_compat.heading")
        } else {
            section.label()
        }
    }

    fn default_section(&self, cx: &Context<Self>) -> GameSection {
        if self.id.runner == Runner::Steam {
            // The first section after "Changed".
            return self.sections(cx)[1];
        }
        if self.uses_wine(cx) {
            GameSection::Wine
        } else {
            GameSection::Performance
        }
    }

    /// The section shown.
    pub fn section(&self) -> GameSection {
        self.section
    }

    fn open(&mut self, section: GameSection, window: &mut Window, cx: &mut Context<Self>) {
        let section = if self.sections(cx).contains(&section) {
            section
        } else {
            self.default_section(cx)
        };
        if self.section == GameSection::Log && section != GameSection::Log {
            self.log.update(cx, |l, _| l.stop());
        }
        self.section = section;
        if !self.query.is_empty() {
            self.query.clear();
            self.search.update(cx, |s, cx| s.set_value("", window, cx));
        }
        if section == GameSection::Log {
            self.log.update(cx, |l, cx| l.start(window, cx));
        }
        if section == GameSection::Tools && self.id.runner != Runner::Steam && self.uses_wine(cx) {
            self.winetricks.update(cx, |w, cx| w.activate(window, cx));
        }
        if section == GameSection::Tools {
            self.reshade.update(cx, |r, cx| r.activate(window, cx));
            self.renodx.update(cx, |r, cx| r.activate(window, cx));
        }
        if section == GameSection::Performance && self.performance_tab == "performance" {
            self.activate_optiscaler(window, cx);
        }
        self.apply_filter(window, cx);
    }

    fn apply_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let q = self.query.trim().to_string();
        let section = self.section.field_section();
        let changed = self.section == GameSection::Changed;
        let perf_subtab =
            (self.section == GameSection::Performance).then_some(self.performance_tab);
        self.form.update(cx, |f, cx| {
            if !q.is_empty() {
                f.set_query(&q, cx);
                f.set_subtab(None, cx);
            } else if changed {
                f.set_changed_only(cx);
                f.set_subtab(None, cx);
            } else if let Some(s) = section {
                f.set_sections(vec![s], cx);
                f.set_subtab(perf_subtab, cx);
            }
        });
        if !q.is_empty() {
            self.activate_optiscaler(window, cx);
        }
        cx.notify();
    }

    fn confirm_reset_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title(cx);
        let form = self.form.downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let form = form.clone();
            alert
                .title(t_args(
                    "settings.game.reset_all.title",
                    &[("title", &title)],
                ))
                .description(t("settings.game.reset_all.description"))
                .show_cancel(true)
                .cancel_text(t("settings.common.cancel"))
                .ok_text(t("settings.game.reset_all.confirm"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    form.update(cx, |f, cx| f.reset_all(window, cx)).ok();
                    true
                })
        });
    }

    fn title(&self, cx: &Context<Self>) -> SharedString {
        backend(cx)
            .game(&self.id)
            .map(|g| SharedString::from(g.title.clone()))
            .unwrap_or_else(|| SharedString::from(self.id.app_name.to_string()))
    }

    fn run_tool(&mut self, tool: Tool, window: &mut Window, cx: &mut Context<Self>) {
        if tool == Tool::RunExe {
            let rx = cx.prompt_for_paths(PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: Some(t("settings.tools.run_exe.prompt")),
            });
            cx.spawn_in(window, async move |this, cx| {
                if let Ok(Ok(Some(paths))) = rx.await
                    && let Some(exe) = paths.into_iter().next()
                {
                    this.update_in(cx, |this, window, cx| {
                        this.start_tool(
                            Tool::RunExe,
                            vec![exe.to_string_lossy().into_owned()],
                            window,
                            cx,
                        )
                    })
                    .ok();
                }
            })
            .detach();
            return;
        }
        let argv = match tool {
            Tool::Winecfg => vec!["winecfg".to_string()],
            _ => Vec::new(),
        };
        self.start_tool(tool, argv, window, cx);
    }

    fn start_tool(
        &mut self,
        tool: Tool,
        argv: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.running.insert(tool) {
            return;
        }
        let done: Option<SharedString> = match tool {
            Tool::Dx(dx, true) => Some(t_args(
                "settings.tools.dx.installed",
                &[("tool", dx.label())],
            )),
            Tool::Dx(dx, false) => {
                Some(t_args("settings.tools.dx.removed", &[("tool", dx.label())]))
            }
            _ => None,
        };
        let b = backend(cx);
        let id = self.id.clone();
        let missing = t("settings.error.game_missing");
        let task = irori_ui::run(cx, async move {
            match tool {
                Tool::Winetricks => b.winetricks(&id, Vec::new()).await.map(|_| ()),
                Tool::KillPrefix => irori_runtime::kill_prefix(&b.settings.game(&id)).await,
                Tool::Dx(dx, enable) => {
                    let game = b.game(&id).ok_or_else(|| anyhow::anyhow!("{missing}"))?;
                    let target = Target {
                        id: &id,
                        game: &game,
                        platform: game
                            .installed_platform()
                            .unwrap_or(InstallPlatform::Windows),
                    };
                    Box::pin(irori_runtime::set_dx_tool(
                        &b.settings.app(),
                        &b.settings.game(&id),
                        target,
                        dx,
                        enable,
                        OpContext::detached(),
                    ))
                    .await
                }
                _ => b.run_in_prefix(&id, argv).await.map(|_| ()),
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.running.remove(&tool);
                match result {
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}")).title(t("settings.tools.failed")),
                        cx,
                    ),
                    Ok(()) => {
                        if let Some(done) = done {
                            window.push_notification(Notification::success(done), cx);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn render_tools(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.id.runner == Runner::Steam {
            // Steam runs the game, so only what goes into its folder applies.
            return v_flex()
                .w_full()
                .child(crate::widgets::group_heading(
                    t("settings.tools.group.reshade"),
                    cx,
                ))
                .child(self.reshade.clone())
                .child(crate::widgets::group_heading(
                    t("settings.tools.group.renodx"),
                    cx,
                ))
                .child(self.renodx.clone())
                .into_any_element();
        }
        let enabled = self.uses_wine(cx);
        let reason = (!enabled).then(|| t("settings.tools.windows_only"));
        let tools = [
            (
                Tool::Winecfg,
                "settings.tools.winecfg.label",
                "settings.tools.winecfg.help",
                "settings.tools.winecfg.action",
            ),
            (
                Tool::Winetricks,
                "settings.tools.winetricks.label",
                "settings.tools.winetricks.help",
                "settings.tools.winetricks.action",
            ),
            (
                Tool::RunExe,
                "settings.tools.run_exe.label",
                "settings.tools.run_exe.help",
                "settings.tools.run_exe.action",
            ),
            (
                Tool::KillPrefix,
                "settings.tools.kill_prefix.label",
                "settings.tools.kill_prefix.help",
                "settings.tools.kill_prefix.action",
            ),
        ];
        let rows = tools.into_iter().map(|(tool, label, help, action)| {
            let row = RowState {
                label: t(label),
                help: t(help),
                disabled_reason: reason.clone(),
                ..Default::default()
            };
            let button = Button::new(ElementId::Name(format!("tool-{tool:?}").into()))
                .label(t(action))
                .loading(self.running.contains(&tool))
                .disabled(!enabled || self.running.contains(&tool))
                .on_click(cx.listener(move |this, _, window, cx| this.run_tool(tool, window, cx)))
                .into_any_element();
            field_row(row, button, cx)
        });
        let dx_rows = DxTool::ALL.into_iter().map(|dx| {
            let row = RowState {
                label: dx.label().into(),
                help: t("settings.tools.dx.help"),
                disabled_reason: reason.clone(),
                ..Default::default()
            };
            let busy = self.running.contains(&Tool::Dx(dx, true))
                || self.running.contains(&Tool::Dx(dx, false));
            let buttons = h_flex()
                .gap_2()
                .child(
                    Button::new(ElementId::Name(format!("dx-install-{}", dx.id()).into()))
                        .label(t("settings.common.install"))
                        .loading(self.running.contains(&Tool::Dx(dx, true)))
                        .disabled(!enabled || busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.run_tool(Tool::Dx(dx, true), window, cx)
                        })),
                )
                .child(
                    Button::new(ElementId::Name(format!("dx-remove-{}", dx.id()).into()))
                        .ghost()
                        .label(t("settings.common.remove"))
                        .loading(self.running.contains(&Tool::Dx(dx, false)))
                        .disabled(!enabled || busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.run_tool(Tool::Dx(dx, false), window, cx)
                        })),
                )
                .into_any_element();
            field_row(row, buttons, cx)
        });
        v_flex()
            .w_full()
            .child(crate::widgets::group_heading(
                t("settings.tools.group.prefix"),
                cx,
            ))
            .children(rows)
            .child(crate::widgets::group_heading(
                t("settings.tools.group.translation_layers"),
                cx,
            ))
            .children(dx_rows)
            .child(crate::widgets::group_heading(
                t("settings.tools.group.reshade"),
                cx,
            ))
            .child(self.reshade.clone())
            .child(crate::widgets::group_heading(
                t("settings.tools.group.renodx"),
                cx,
            ))
            .child(self.renodx.clone())
            .child(crate::widgets::group_heading(
                t("settings.tools.group.winetricks"),
                cx,
            ))
            .child(if enabled {
                div().pt_2().child(self.winetricks.clone())
            } else {
                div()
                    .py_3()
                    .child(muted(t("settings.tools.windows_only"), cx))
            })
            .into_any_element()
    }
}

impl Render for GameSettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.language.changed() {
            cx.defer_in(window, |this, window, cx| {
                let placeholder = t("settings.common.search_settings");
                this.search
                    .update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
            });
        }
        let searching = !self.query.trim().is_empty();
        let title = self.title(cx);
        let overridden = self.form.read(cx).overridden_count();
        let known = backend(cx).game(&self.id).is_some();
        let id = self.id.clone();
        let nav = v_flex()
            .w(rems(15.))
            .flex_shrink_0()
            .gap_1()
            .p_3()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                Input::new(&self.search)
                    .id("game-settings-search")
                    .aria_label(t("settings.common.search_settings"))
                    .prefix(Icon::new(IconName::Search).small())
                    .cleanable(true)
                    .mb_2(),
            )
            .children(self.sections(cx).into_iter().map(|s| {
                let label = if s == GameSection::Changed {
                    t_args(
                        "settings.game.section.changed_count",
                        &[("count", &overridden.to_string())],
                    )
                } else {
                    self.label_of(s)
                };
                nav_button(
                    ElementId::Name(format!("game-settings-nav-{}", s.id()).into()),
                    s.icon(),
                    label,
                    !searching && s == self.section,
                    cx.listener(move |this, _, window, cx| this.open(s, window, cx)),
                )
            }));

        let description = match overridden {
            0 => t("settings.game.overridden_none"),
            1 => t("settings.game.overridden_one"),
            n => t_args(
                "settings.game.overridden_other",
                &[("count", &n.to_string())],
            ),
        };
        let on_close = self.on_close.clone();
        let header = PageHeader::new(title)
            .description(description)
            .when(on_close.is_none(), |header| {
                header.action(
                    Button::new("game-settings-defaults")
                        .ghost()
                        .label(t("settings.game.open_defaults"))
                        .on_click(|_, _, cx| {
                            irori_ui::navigate(Route::Settings("game-defaults".into()), cx)
                        }),
                )
            })
            .action(
                Button::new("game-settings-reset-all")
                    .outline()
                    .icon(IconName::RotateCcw)
                    .label(t("settings.game.reset_all.action"))
                    .disabled(overridden == 0)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.confirm_reset_all(window, cx)),
                    ),
            )
            .action(
                Button::new("game-settings-back")
                    .outline()
                    .icon(IconName::ArrowLeft)
                    .label(t("settings.game.back"))
                    .on_click(move |_, window, cx| match &on_close {
                        Some(close) => close(window, cx),
                        None => irori_ui::navigate(Route::Game(id.clone()), cx),
                    }),
            );

        if !known {
            return h_flex()
                .size_full()
                .child(
                    EmptyState::new(IconName::Gamepad2, t("settings.game.unknown.title"))
                        .description(t("settings.game.unknown.description")),
                )
                .into_any_element();
        }

        let content: AnyElement = if !searching && self.section == GameSection::Log {
            v_flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .gap_4()
                .p_6()
                .child(header)
                .child(div().flex_1().min_h_0().child(self.log.clone()))
                .into_any_element()
        } else {
            let steam_compat = self
                .steam_compat
                .clone()
                .filter(|_| !searching && self.section == GameSection::Wine);
            let wine_note = (!searching
                && self.section == GameSection::Wine
                && !self.uses_wine(cx)
                && steam_compat.is_none())
            .then(|| info_alert("native-note", t("settings.game.native_note")));
            // Steam starts its games itself: explain on Launch why most launch
            // controls are omitted. Performance handles Steam's launch snippet
            // directly beside the Live preview instead of stacking a second banner.
            let steam_note = (!searching
                && self.id.runner == Runner::Steam
                && self.section == GameSection::Launch)
                .then(|| {
                    let text = if cfg!(target_os = "linux") {
                        t("settings.game.steam_note_linux")
                    } else {
                        t("settings.game.steam_note")
                    };
                    info_alert("steam-note", text)
                });
            let performance_tabs = (!searching && self.section == GameSection::Performance)
                .then(|| self.render_performance_tabs(cx))
                .flatten();
            let body: AnyElement = if let Some(panel) = steam_compat {
                panel.into_any_element()
            } else if !searching && self.section == GameSection::Tools {
                self.render_tools(cx)
            } else if !searching && self.section == GameSection::Changed && overridden == 0 {
                EmptyState::new(IconName::CircleCheck, t("settings.game.changed.none_title"))
                    .description(t("settings.game.changed.none"))
                    .into_any_element()
            } else if searching && self.form.read(cx).visible_count() == 0 {
                EmptyState::new(
                    IconName::Search,
                    t_args("settings.common.no_match", &[("query", self.query.trim())]),
                )
                .into_any_element()
            } else {
                self.form.clone().into_any_element()
            };
            v_flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(
                    div()
                        .flex_shrink_0()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .w_full()
                                .max_w(rems(48.))
                                .px_6()
                                .pt_6()
                                .pb_4()
                                .child(header),
                        ),
                )
                .child(
                    div()
                        .id("game-settings-content")
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .overflow_y_scrollbar()
                        .child(
                            v_flex()
                                .w_full()
                                .max_w(rems(48.))
                                .px_6()
                                .pt_4()
                                .pb_12()
                                .gap_2()
                                .when(searching, |v| {
                                    v.child(muted(t("settings.common.search_results"), cx))
                                })
                                .children(performance_tabs)
                                .children(wine_note)
                                .children(steam_note)
                                .child(body),
                        ),
                )
                .into_any_element()
        };
        h_flex()
            .id("game-settings")
            .size_full()
            .items_stretch()
            .when(irori_ui::glass::solid_pages(cx), |this| {
                this.bg(cx.theme().background)
            })
            .child(nav)
            .child(content)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_ids_round_trip() {
        for s in GameSection::ALL {
            assert_eq!(GameSection::from_id(s.id()), Some(s));
        }
        assert_eq!(GameSection::from_id("sync"), Some(GameSection::CloudSaves));
        assert_eq!(
            GameSection::from_id("other"),
            Some(GameSection::Performance)
        );
        assert_eq!(GameSection::from_id("graphics"), Some(GameSection::Gpu));
        assert_eq!(GameSection::from_id("artwork"), Some(GameSection::Artwork));
        assert_eq!(GameSection::from_id("?"), None);
    }
}
