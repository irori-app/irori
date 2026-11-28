use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::component::IndexPath;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::slider::{SliderEvent, SliderState};
use gpui_kit::{AnyView, App, AppContext as _, Context, Focusable as _, SharedString, Window};
use irori_core::Runner;
use irori_core::settings::AppSettings;
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};
use irori_ui::theme::Desktop;
use serde_json::Value;

use crate::appearance::{AccentPicker, FontPicker};
use crate::artwork::ArtworkPanel;
use crate::controller_map::ButtonMapPanel;
use crate::controllers::ControllersPanel;
use crate::epic::{EglSyncPanel, EosOverlayPanel};
use crate::hud::{HudPanel, HudPanelScope};
use crate::igdb::IgdbPanel;
use crate::launch_presets::LaunchPresetsPanel;
use crate::launch_programs::LaunchProgramsPanel;
use crate::optiscaler::OptiscalerPanel;
use crate::optiscaler_defaults::OptiscalerDefaultsPanel;
use crate::overlay::OverlayPanel;
use crate::registry::{
    self, ActionKind, ChoiceOption, CustomKind, FieldKind, FieldSection, FieldSpec, NeedContext,
    PanelKind, TextKind,
};
use crate::replay::ReplayPanel;
use crate::saves::SaveSyncPanel;
use crate::sunshine::SunshinePanel;
use crate::umu::UmuPanel;
use crate::user_actions::UserActionsPanel;
use crate::value::{bool_at, effective_overrides, get_path, value_to_text};

use super::{ChoiceItem, FormFilter, FormTarget, PairEditor, SettingsForm, WinePicker, is_native};

impl SettingsForm {
    pub(super) fn visible_fields(&self) -> impl Iterator<Item = &'static FieldSpec> + '_ {
        self.fields.iter().copied().filter(move |f| {
            if matches!(f.kind, FieldKind::Action(ActionKind::ImportHeroic))
                && !self.heroic_detected
            {
                return false;
            }
            if matches!(f.kind, FieldKind::Action(ActionKind::ImportLutris))
                && !self.lutris_detected
            {
                return false;
            }
            if matches!(f.kind, FieldKind::Action(ActionKind::ImportBottles))
                && !self.bottles_detected
            {
                return false;
            }
            if matches!(f.kind, FieldKind::Action(ActionKind::ImportFaugus))
                && !self.faugus_detected
            {
                return false;
            }
            // Keeping a launcher in sync is offered where it is found.
            match f.path {
                "iroriLauncherImports.lutris" if !self.lutris_detected => return false,
                "iroriLauncherImports.bottles" if !self.bottles_detected => return false,
                "iroriLauncherImports.faugus" if !self.faugus_detected => return false,
                _ => {}
            }
            if matches!(f.kind, FieldKind::Action(ActionKind::ImportPlaynite))
                && !self.playnite_detected
            {
                return false;
            }
            if matches!(
                f.kind,
                FieldKind::Panel(PanelKind::EosOverlay | PanelKind::EglSync)
            ) && !self.epic_available
            {
                return false;
            }
            // Steam's own launch options belong to Steam games only.
            if matches!(f.kind, FieldKind::Panel(PanelKind::SteamLaunchOptions))
                && !matches!(&self.target, FormTarget::Game(id) if id.runner == Runner::Steam)
            {
                return false;
            }
            // Per-game OptiScaler appears only for installed Windows builds
            // (and Steam Proton builds) that Irori can install graphics mods into.
            if matches!(f.kind, FieldKind::Panel(PanelKind::Optiscaler))
                && matches!(&self.target, FormTarget::Game(_))
                && !self.supports_optiscaler
            {
                return false;
            }
            // The defaults reach native builds too, so only a game that isn't
            // one leaves native-only fields out.
            if f.native_only && matches!(self.target, FormTarget::Game(_)) && !self.native {
                return false;
            }
            // A game started without Wine has no use for the Wine fields.
            if matches!(self.target, FormTarget::Game(_))
                && f.section == FieldSection::Wine
                && f.path != "iroriNoWine"
                && self.bool_value("iroriNoWine")
            {
                return false;
            }
            if matches!(&self.target, FormTarget::Game(id) if id.runner == Runner::Steam)
                && !f.applies_to_steam()
            {
                return false;
            }
            match &self.filter {
                FormFilter::Sections(s) => {
                    if !s.contains(&f.section) {
                        return false;
                    }
                    if let Some(tab) = self.subtab
                        && crate::layout::subtab_of(f) != Some(tab)
                    {
                        return false;
                    }
                    !(self.compact && crate::layout::is_hidden_by_parent(f, &self.snapshot))
                }
                FormFilter::Query(q) => f.matches(q),
                FormFilter::Changed => self.is_overridden(f),
            }
        })
    }

    // ---- snapshot ----

    pub(super) fn read_snapshot(&mut self, cx: &mut Context<Self>) {
        let b = backend(cx);
        let app: Arc<AppSettings> = b.settings.app();
        match &self.target {
            FormTarget::App | FormTarget::Defaults => {
                self.snapshot = serde_json::to_value(&*app).unwrap_or_default();
            }
            FormTarget::Game(id) => {
                let game = b.game(id);
                self.native = is_native(game.as_deref());
                self.supports_optiscaler = game
                    .as_deref()
                    .is_some_and(irori_services::supports_reshade);
                let overrides = b.settings.overrides(id);
                self.defaults = app.game.clone();
                self.defaults_json = serde_json::to_value(&self.defaults).unwrap_or_default();
                self.effective = overrides.apply(&self.defaults);
                self.snapshot = serde_json::to_value(&self.effective).unwrap_or_default();
                self.overridden = effective_overrides(&self.defaults, &overrides)
                    .into_iter()
                    .collect();
            }
        }
    }

    pub(super) fn value(&self, path: &str) -> Option<&Value> {
        get_path(&self.snapshot, path)
    }

    pub(super) fn bool_value(&self, path: &str) -> bool {
        bool_at(&self.snapshot, path)
    }

    /// Whether this field differs from the global default (game target).
    pub(super) fn is_overridden(&self, spec: &FieldSpec) -> bool {
        matches!(self.target, FormTarget::Game(_))
            && self.overridden.contains(spec.override_key())
            && get_path(&self.defaults_json, spec.path) != self.value(spec.path)
    }

    pub(super) fn disabled_reason(&self, spec: &FieldSpec) -> Option<SharedString> {
        let need = NeedContext {
            host: irori_core::HostPlatform::current(),
            graphics: self.graphics.as_deref(),
            chosen_gpu: !value_to_text(self.value("iroriGpu")).trim().is_empty(),
            native_game: matches!(self.target, FormTarget::Game(_)) && self.native,
        };
        if let Some(reason) = spec.needs.iter().find_map(|n| n.blocker(&need)) {
            return Some(t(reason));
        }
        spec.requires
            .iter()
            .find(|c| self.bool_value(c.path) != c.equals)
            .map(|c| t(c.reason))
    }

    pub(super) fn hardware_blocked(&self, spec: &FieldSpec) -> bool {
        let need = NeedContext {
            host: irori_core::HostPlatform::current(),
            graphics: self.graphics.as_deref(),
            chosen_gpu: !value_to_text(self.value("iroriGpu")).trim().is_empty(),
            native_game: matches!(self.target, FormTarget::Game(_)) && self.native,
        };
        spec.needs.iter().any(|n| n.blocker(&need).is_some())
    }

    pub(super) fn advanced_bucket_key(&self) -> String {
        let scope = match &self.target {
            FormTarget::App => "app",
            FormTarget::Defaults => "defaults",
            FormTarget::Game(_) => "game",
        };
        let fallback = match &self.filter {
            FormFilter::Sections(s) if s.len() == 1 => s[0].label_key(),
            _ => "all",
        };
        format!("{scope}-{}", self.subtab.unwrap_or(fallback))
    }

    pub(super) fn build_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for spec in self.fields.clone() {
            match spec.kind {
                FieldKind::Text(kind) => {
                    let text = if kind == TextKind::Secret {
                        String::new()
                    } else {
                        value_to_text(self.value(spec.path))
                    };
                    let placeholder = self.placeholder(spec, cx);
                    let state = cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder(placeholder)
                            .masked(kind == TextKind::Secret)
                            .default_value(text)
                    });
                    let path = spec.path;
                    self._subscriptions.push(cx.subscribe_in(
                        &state,
                        window,
                        move |this, _, event: &InputEvent, window, cx| match event {
                            InputEvent::Blur | InputEvent::PressEnter { .. } => {
                                this.commit_text(path, window, cx)
                            }
                            InputEvent::Change => {
                                if this.errors.remove(path).is_some() {
                                    cx.notify();
                                }
                            }
                            InputEvent::Focus => {}
                        },
                    ));
                    self.texts.insert(spec.path, state);
                }
                FieldKind::Choice(options) => {
                    let items = choice_items(options);
                    let selected = self.choice_index(options, spec.path);
                    let state = cx.new(|cx| SelectState::new(items, selected, window, cx));
                    let path = spec.path;
                    self._subscriptions.push(cx.subscribe_in(
                        &state,
                        window,
                        move |this, _, event: &SelectEvent<Vec<ChoiceItem>>, window, cx| {
                            let SelectEvent::Confirm(Some(key)) = event else {
                                return;
                            };
                            let Some(FieldKind::Choice(options)) =
                                registry::find(path).map(|f| f.kind)
                            else {
                                return;
                            };
                            if let Some(o) = options.iter().find(|o| o.value.key() == key.as_ref())
                            {
                                this.write(path, o.value.to_json(), window, cx);
                            }
                        },
                    ));
                    self.choices.insert(spec.path, state);
                }
                FieldKind::Custom(kind) => self.build_custom(spec, kind, window, cx),
                FieldKind::Panel(kind) => self.build_panel(spec, kind, window, cx),
                FieldKind::Toggle | FieldKind::Action(_) => {}
            }
        }
    }

    fn build_panel(
        &mut self,
        spec: &'static FieldSpec,
        kind: PanelKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view: Option<AnyView> = match kind {
            PanelKind::Controllers => Some(cx.new(ControllersPanel::new).into()),
            PanelKind::ButtonMap => Some(cx.new(ButtonMapPanel::new).into()),
            PanelKind::Overlay => Some(cx.new(OverlayPanel::new).into()),
            PanelKind::EosOverlay => crate::stores::epic(cx)
                .map(|epic| cx.new(|cx| EosOverlayPanel::new(epic, window, cx)).into()),
            PanelKind::EglSync => crate::stores::epic(cx)
                .map(|epic| cx.new(|cx| EglSyncPanel::new(epic, window, cx)).into()),
            PanelKind::UserActions => match &self.target {
                FormTarget::Game(id) => {
                    let id = id.clone();
                    Some(cx.new(|cx| UserActionsPanel::new(id, window, cx)).into())
                }
                _ => None,
            },
            PanelKind::LaunchPrograms => {
                let target = self.target.clone();
                Some(
                    cx.new(|cx| LaunchProgramsPanel::new(target, window, cx))
                        .into(),
                )
            }
            PanelKind::LaunchPresets => {
                let target = self.target.clone();
                Some(
                    cx.new(|cx| LaunchPresetsPanel::new(target, window, cx))
                        .into(),
                )
            }
            PanelKind::SaveSync => match &self.target {
                FormTarget::Game(id) => {
                    let id = id.clone();
                    Some(cx.new(|cx| SaveSyncPanel::new(id, window, cx)).into())
                }
                _ => None,
            },
            PanelKind::Artwork => match &self.target {
                FormTarget::Game(id) => {
                    let id = id.clone();
                    Some(cx.new(|cx| ArtworkPanel::new(id, window, cx)).into())
                }
                _ => None,
            },
            PanelKind::Ludusavi => Some(
                cx.new(|cx| irori_ui::save_backups::LudusaviPanel::new(window, cx))
                    .into(),
            ),
            PanelKind::SaveBackups => match &self.target {
                FormTarget::Game(id) => {
                    let id = id.clone();
                    Some(
                        cx.new(|cx| irori_ui::save_backups::SaveBackupsPanel::new(id, window, cx))
                            .into(),
                    )
                }
                _ => None,
            },
            PanelKind::Replay => {
                let game = match &self.target {
                    FormTarget::Game(id) => Some(id.clone()),
                    _ => None,
                };
                Some(cx.new(|cx| ReplayPanel::new(game, cx)).into())
            }
            PanelKind::PerformanceHud => match &self.target {
                FormTarget::Game(id) => {
                    let id = id.clone();
                    let is_native_linux = self.native;
                    let is_steam = id.runner == Runner::Steam;
                    Some(
                        cx.new(|cx| {
                            HudPanel::new(
                                HudPanelScope::PerGame {
                                    id,
                                    is_native_linux,
                                    is_steam,
                                },
                                None,
                                window,
                                cx,
                            )
                        })
                        .into(),
                    )
                }
                _ => None,
            },
            PanelKind::Optiscaler => match &self.target {
                FormTarget::Defaults => {
                    Some(cx.new(|cx| OptiscalerDefaultsPanel::new(window, cx)).into())
                }
                FormTarget::Game(id) => {
                    let id = id.clone();
                    let entity = cx.new(|cx| OptiscalerPanel::new(id, window, cx));
                    self.optiscaler_panel = Some(entity.clone());
                    Some(entity.into())
                }
                FormTarget::App => None,
            },
            PanelKind::Sunshine => Some(cx.new(|cx| SunshinePanel::new(window, cx)).into()),
            PanelKind::QuickLaunch => {
                Some(cx.new(crate::quick_launch::QuickLaunchPanel::new).into())
            }
            PanelKind::Igdb => Some(cx.new(|cx| IgdbPanel::new(window, cx)).into()),
            PanelKind::LibraryFolders => Some(
                cx.new(|cx| crate::library_folders::LibraryFoldersPanel::new(window, cx))
                    .into(),
            ),
            PanelKind::LaunchBox => crate::stores::emulation(cx).map(|store| {
                cx.new(|cx| crate::launchbox::LaunchBoxPanel::new(store, cx))
                    .into()
            }),
            PanelKind::SteamLaunchOptions => match &self.target {
                FormTarget::Game(id) if id.runner == Runner::Steam => {
                    crate::stores::steam(cx).map(|steam| {
                        let id = id.clone();
                        cx.new(|cx| {
                            crate::steam_launch::SteamLaunchPanel::new(id, steam, window, cx)
                        })
                        .into()
                    })
                }
                _ => None,
            },
            PanelKind::HowItStarts => match &self.target {
                FormTarget::Game(id) if id.runner.manages_installs() => {
                    let id = id.clone();
                    Some(
                        cx.new(|cx| crate::how_it_starts::HowItStartsPanel::new(id, window, cx))
                            .into(),
                    )
                }
                _ => None,
            },
            PanelKind::Umu => match &self.target {
                FormTarget::Game(id) => {
                    let id = id.clone();
                    Some(cx.new(|cx| UmuPanel::new(id, window, cx)).into())
                }
                _ => None,
            },
        };
        if let Some(view) = view {
            self.panels.insert(spec.path, view);
        }
    }

    fn placeholder(&self, spec: &FieldSpec, cx: &App) -> SharedString {
        match spec.kind {
            FieldKind::Text(TextKind::Secret)
                if spec.path == "steamGridDbApiKey" && backend(cx).steamgrid_key_saved_cached() =>
            {
                t("settings.field.secret_saved_placeholder")
            }
            FieldKind::Text(TextKind::Secret)
                if !value_to_text(self.value(spec.path)).is_empty() =>
            {
                t("settings.field.secret_saved_placeholder")
            }
            FieldKind::Text(_) if spec.path == "winePrefix" => match &self.target {
                FormTarget::Game(id) => {
                    let b = backend(cx);
                    match b.game(id) {
                        Some(game) => irori_runtime::default_prefix(&b.settings.app(), &game)
                            .to_string_lossy()
                            .to_string()
                            .into(),
                        None => spec.placeholder_text(),
                    }
                }
                _ => spec.placeholder_text(),
            },
            _ => spec.placeholder_text(),
        }
    }

    fn choice_index(&self, options: &[ChoiceOption], path: &str) -> Option<IndexPath> {
        let v = self.value(path).cloned().unwrap_or(Value::Null);
        options
            .iter()
            .position(|o| o.value.matches(&v))
            .map(|ix| IndexPath::default().row(ix))
    }

    fn build_custom(
        &mut self,
        spec: &'static FieldSpec,
        kind: CustomKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match kind {
            CustomKind::WineVersion => {
                let select = cx.new(|cx| {
                    SelectState::new(Vec::<ChoiceItem>::new(), None, window, cx).searchable(true)
                });
                self._subscriptions.push(cx.subscribe_in(
                    &select,
                    window,
                    |this, _, event: &SelectEvent<Vec<ChoiceItem>>, window, cx| {
                        let SelectEvent::Confirm(Some(bin)) = event else {
                            return;
                        };
                        let build = this
                            .wine
                            .as_ref()
                            .and_then(|w| w.builds.as_ref())
                            .and_then(|b| b.iter().find(|w| w.bin == bin.as_ref()))
                            .cloned();
                        if let Some(build) = build
                            && let Ok(v) = serde_json::to_value(build)
                        {
                            this.write("wineVersion", v, window, cx);
                        }
                    },
                ));
                self.wine = Some(WinePicker {
                    select,
                    builds: None,
                    sources: HashMap::new(),
                    loading: false,
                });
                self.sync_wine_select(window, cx);
            }
            CustomKind::EnvVars
            | CustomKind::DllOverrides
            | CustomKind::Wrappers
            | CustomKind::GogSaves => {
                let (p1, p2) = PairEditor::placeholders(kind);
                let first = cx.new(|cx| InputState::new(window, cx).placeholder(p1));
                let second = cx.new(|cx| InputState::new(window, cx).placeholder(p2));
                let path = spec.path;
                for input in [&first, &second] {
                    self._subscriptions.push(cx.subscribe_in(
                        input,
                        window,
                        move |this, _, event: &InputEvent, window, cx| {
                            if let InputEvent::PressEnter { .. } = event {
                                this.commit_pair(path, window, cx);
                            }
                        },
                    ));
                }
                self.pairs.insert(
                    spec.path,
                    PairEditor {
                        kind,
                        first,
                        second,
                        editing: None,
                        error: None,
                        text: None,
                    },
                );
            }
            CustomKind::ToolVersion(_) => {
                let items = self.tool_version_items(spec.path);
                let state = cx.new(|cx| {
                    SelectState::new(items, Some(IndexPath::default()), window, cx).searchable(true)
                });
                let path = spec.path;
                self._subscriptions.push(cx.subscribe_in(
                    &state,
                    window,
                    move |this, _, event: &SelectEvent<Vec<ChoiceItem>>, window, cx| {
                        if let SelectEvent::Confirm(Some(tag)) = event {
                            this.write(path, Value::String(tag.to_string()), window, cx);
                        }
                    },
                ));
                self.choices.insert(spec.path, state);
            }
            CustomKind::GpuCard => {
                let state =
                    cx.new(|cx| SelectState::new(Vec::<ChoiceItem>::new(), None, window, cx));
                let path = spec.path;
                self._subscriptions.push(cx.subscribe_in(
                    &state,
                    window,
                    move |this, _, event: &SelectEvent<Vec<ChoiceItem>>, window, cx| {
                        if let SelectEvent::Confirm(Some(slot)) = event {
                            this.write(path, Value::String(slot.to_string()), window, cx);
                        }
                    },
                ));
                self.choices.insert(spec.path, state);
                self.sync_gpu_select(window, cx);
            }
            CustomKind::HiddenGpu => {
                let state =
                    cx.new(|cx| SelectState::new(Vec::<ChoiceItem>::new(), None, window, cx));
                let path = spec.path;
                self._subscriptions.push(cx.subscribe_in(
                    &state,
                    window,
                    move |this, _, event: &SelectEvent<Vec<ChoiceItem>>, window, cx| {
                        if let SelectEvent::Confirm(Some(id)) = event {
                            this.write(path, Value::String(id.to_string()), window, cx);
                        }
                    },
                ));
                self.choices.insert(spec.path, state);
                self.sync_hide_gpu_select(window, cx);
            }
            CustomKind::ThemeName => {
                let items = theme_items(cx);
                let current = value_to_text(self.value(spec.path));
                let selected = items
                    .iter()
                    .position(|i| i.key.as_ref() == current)
                    .map(|ix| IndexPath::default().row(ix));
                let state =
                    cx.new(|cx| SelectState::new(items, selected, window, cx).searchable(true));
                self._subscriptions.push(cx.subscribe_in(
                    &state,
                    window,
                    |this, _, event: &SelectEvent<Vec<ChoiceItem>>, window, cx| {
                        if let SelectEvent::Confirm(Some(name)) = event {
                            this.pick_theme(name, window, cx);
                        }
                    },
                ));
                self.choices.insert(spec.path, state);
            }
            CustomKind::AccentColor => {
                let view = cx.new(|cx| AccentPicker::new(window, cx));
                self.panels.insert(spec.path, view.into());
            }
            CustomKind::FontFamily => {
                let view = cx.new(|cx| FontPicker::new(window, cx));
                self.panels.insert(spec.path, view.into());
            }
            CustomKind::FontSize | CustomKind::TileSize | CustomKind::Percent => {
                let (min, max, step) = match kind {
                    CustomKind::FontSize => (12., 24., 1.),
                    CustomKind::TileSize => (8., 16., 0.5),
                    _ => (0., 100., 5.),
                };
                let current = self
                    .value(spec.path)
                    .and_then(Value::as_f64)
                    .unwrap_or(min as f64) as f32;
                let state = cx.new(|_| {
                    SliderState::new()
                        .min(min)
                        .max(max)
                        .step(step)
                        .default_value(current)
                });
                let path = spec.path;
                self._subscriptions.push(cx.subscribe_in(
                    &state,
                    window,
                    move |this, _, event: &SliderEvent, window, cx| {
                        let (v, released) = match event {
                            SliderEvent::Change(v) => (v.start(), false),
                            SliderEvent::Release(v) => (v.start(), true),
                        };
                        this.slider_changed(path, v, released, window, cx);
                    },
                ));
                self.sliders.insert(spec.path, state);
            }
        }
    }

    /// Re-read settings and bring controls in line, leaving focused inputs alone.
    pub(super) fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.read_snapshot(cx);
        for (path, input) in &self.texts {
            let Some(spec) = registry::find(path) else {
                continue;
            };
            if spec.kind == FieldKind::Text(TextKind::Secret) {
                let placeholder = self.placeholder(spec, cx);
                input.update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
                continue;
            }
            if input.read(cx).focus_handle(cx).is_focused(window) {
                continue;
            }
            let text = value_to_text(get_path(&self.snapshot, path));
            if input.read(cx).value().as_ref() != text {
                input.update(cx, |s, cx| s.set_value(text, window, cx));
            }
        }
        for (path, state) in &self.choices {
            let Some(spec) = registry::find(path) else {
                continue;
            };
            let selected = match spec.kind {
                FieldKind::Choice(options) => self.choice_index(options, path),
                _ => {
                    let current = value_to_text(get_path(&self.snapshot, path));
                    state.update(cx, |s, cx| {
                        s.set_selected_value(&current.into(), window, cx)
                    });
                    continue;
                }
            };
            if state.read(cx).selected_index(cx) != selected {
                state.update(cx, |s, cx| s.set_selected_index(selected, window, cx));
            }
        }
        for (path, slider) in &self.sliders {
            if self.slider_writes.contains_key(path) {
                continue;
            }
            if let Some(v) = get_path(&self.snapshot, path).and_then(Value::as_f64) {
                let v = v as f32;
                if (slider.read(cx).value().start() - v).abs() > f32::EPSILON {
                    slider.update(cx, |s, cx| s.set_value(v, window, cx));
                }
            }
        }
        self.sync_wine_select(window, cx);
        self.sync_gpu_select(window, cx);
        self.sync_hide_gpu_select(window, cx);
        cx.notify();
    }

    /// Rebuild the retained text (placeholders, select items) in the
    /// current interface language.
    pub(super) fn retranslate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for (path, input) in &self.texts {
            if let Some(spec) = registry::find(path) {
                let placeholder = self.placeholder(spec, cx);
                input.update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
            }
        }
        for editor in self.pairs.values() {
            let (first, second) = PairEditor::placeholders(editor.kind);
            editor
                .first
                .update(cx, |s, cx| s.set_placeholder(first, window, cx));
            editor
                .second
                .update(cx, |s, cx| s.set_placeholder(second, window, cx));
        }
        for (path, state) in &self.choices {
            let items = match registry::find(path).map(|f| f.kind) {
                Some(FieldKind::Choice(options)) => choice_items(options),
                Some(FieldKind::Custom(CustomKind::ThemeName)) => theme_items(cx),
                Some(FieldKind::Custom(CustomKind::ToolVersion(_))) => {
                    self.tool_version_items(path)
                }
                _ => continue,
            };
            state.update(cx, |s, cx| s.set_items(items, window, cx));
        }
        self.sync_wine_select(window, cx);
        self.sync_gpu_select(window, cx);
        self.sync_hide_gpu_select(window, cx);
        cx.notify();
    }
}

/// Select items of a choice field, labelled in the interface language.
fn choice_items(options: &[ChoiceOption]) -> Vec<ChoiceItem> {
    options
        .iter()
        .map(|o| ChoiceItem::new(o.value.key(), o.label.text()))
        .collect()
}

/// "Hide nothing", every card a game may hide (never one showing a display:
/// the same rule the launch keeps, `gpu_hide::plan`), and the chosen card
/// even when it can't be hidden now, so the choice stays visible.
pub(super) fn hide_gpu_items(
    adapters: Option<&[irori_services::gpu_hide::Adapter]>,
    current: &str,
) -> Vec<ChoiceItem> {
    use irori_services::gpu_hide::{Plan, plan};
    let mut items = vec![ChoiceItem::new("", t("settings.hide_gpu.none"))];
    let all = adapters.unwrap_or_default();
    items.extend(
        all.iter()
            .filter(|a| matches!(plan(&a.instance_id, all), Plan::Hide(_)))
            .map(|a| ChoiceItem::new(a.instance_id.clone(), a.name.clone())),
    );
    if !current.is_empty() && !items.iter().any(|i| i.key.as_ref() == current) {
        let name = all
            .iter()
            .find(|a| a.instance_id.eq_ignore_ascii_case(current))
            .map_or(current, |a| a.name.as_str());
        let label = if adapters.is_some() {
            t_args("settings.hide_gpu.unavailable", &[("name", name)])
        } else {
            name.to_string().into()
        };
        items.push(ChoiceItem::new(current.to_string(), label));
    }
    items
}

/// The default theme followed by every theme of the theme registry.
fn theme_items(cx: &App) -> Vec<ChoiceItem> {
    let default = match irori_ui::theme::followed_desktop(cx) {
        Some(Desktop::Omarchy) => "settings.theme.omarchy",
        Some(Desktop::Cosmic) => "settings.theme.cosmic",
        Some(Desktop::Kde) => "settings.theme.kde",
        Some(Desktop::Wallpaper) => "settings.theme.wallpaper",
        None => "settings.theme.default",
    };
    let mut items = vec![ChoiceItem::new("", t(default))];
    for (name, dark) in irori_ui::theme::theme_names(cx) {
        let key = if dark {
            "settings.theme.dark_variant"
        } else {
            "settings.theme.light_variant"
        };
        let label = t_args(key, &[("name", &name)]);
        items.push(ChoiceItem::new(name, label));
    }
    items
}

/// A slider's value as stored: percentages are whole numbers, sizes are not.
pub(super) fn slider_json(path: &str, v: f32) -> Value {
    match registry::find(path).map(|f| f.kind) {
        Some(FieldKind::Custom(CustomKind::Percent)) => {
            Value::from(v.round().clamp(0., 100.) as u64)
        }
        _ => Value::from(v as f64),
    }
}
