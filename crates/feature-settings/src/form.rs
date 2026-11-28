//! The settings form: renders registry fields bound to one settings target
//! and persists edits through the settings service.

mod actions;
mod controls;
mod loaders;
mod mutations;
mod render;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::component::alert::Alert;
use gpui_kit::component::input::{InputState, TextareaState};
use gpui_kit::component::select::{SelectItem, SelectState};
use gpui_kit::component::slider::SliderState;
use gpui_kit::{
    AnyView, Context, ElementId, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Task, Window, div,
};
use irori_core::GameId;
use irori_core::settings::{GameSettings, WineInstallation};
use irori_runtime::discovery::WineSource;
use irori_runtime::graphics::HostGraphics;
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};
use serde_json::Value;

use crate::i18n::LanguageStamp;
use crate::optiscaler::OptiscalerPanel;
use crate::registry::{self, ActionKind, CustomKind, FieldSection, FieldSpec, Scope};
use crate::widgets::{more_options_disclosure, muted};

/// Whether `game` is installed as this system's own build: native Linux
/// on Linux, a Mac build on macOS.
fn is_native(game: Option<&irori_core::GameInfo>) -> bool {
    game.and_then(|g| g.installed_platform()) == Some(irori_core::HostPlatform::current().native())
}

/// What a form edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormTarget {
    /// Application-wide fields (General, Appearance, Advanced).
    App,
    /// Launch settings every game inherits (`AppSettings::game`).
    Defaults,
    /// One game's sparse overrides on top of the defaults.
    Game(GameId),
}

/// Which fields a form shows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FormFilter {
    Sections(Vec<FieldSection>),
    Query(String),
    /// Only the fields the game changes from the defaults, every section.
    Changed,
}

/// Select item: a stable key and a label.
#[derive(Debug, Clone)]
pub(crate) struct ChoiceItem {
    key: SharedString,
    label: SharedString,
}

impl ChoiceItem {
    pub(crate) fn new(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
        }
    }
}

impl SelectItem for ChoiceItem {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &SharedString {
        &self.key
    }
}

type ChoiceState = Entity<SelectState<Vec<ChoiceItem>>>;

/// Two-input row editor for env vars, wrappers and GOG save locations.
struct PairEditor {
    kind: CustomKind,
    first: Entity<InputState>,
    second: Entity<InputState>,
    editing: Option<usize>,
    error: Option<SharedString>,
    /// The variables edited as text (environment variables only), while
    /// that view is open.
    text: Option<Entity<TextareaState>>,
}

impl PairEditor {
    /// JSON field names of a row.
    fn keys(kind: CustomKind) -> (&'static str, &'static str) {
        match kind {
            CustomKind::EnvVars => ("key", "value"),
            CustomKind::DllOverrides => ("dll", "mode"),
            CustomKind::Wrappers => ("exe", "args"),
            _ => ("name", "location"),
        }
    }

    fn placeholders(kind: CustomKind) -> (SharedString, SharedString) {
        match kind {
            CustomKind::EnvVars => (t("settings.pairs.env_name"), t("settings.pairs.env_value")),
            CustomKind::DllOverrides => {
                (t("settings.pairs.dll_name"), t("settings.pairs.dll_mode"))
            }
            CustomKind::Wrappers => (
                t("settings.pairs.wrapper_command"),
                t("settings.pairs.wrapper_arguments"),
            ),
            _ => (
                t("settings.pairs.save_name"),
                t("settings.pairs.save_folder"),
            ),
        }
    }
}

/// State of the Wine/Proton picker.
struct WinePicker {
    select: ChoiceState,
    builds: Option<Vec<WineInstallation>>,
    /// Where each build sharing its name with another was found, by `bin`.
    sources: HashMap<String, WineSource>,
    loading: bool,
}

/// The shared settings form.
///
/// Owns one retained input or select per field of its target, mirrors the
/// current settings snapshot (re-read whenever the settings model changes)
/// and writes edits through `SettingsService`: toggles and selects on
/// change, text fields on blur or Enter, sliders after a short pause.
/// For a game target, each field that differs from the global default shows
/// an "Overridden" marker with a reset button.
pub struct SettingsForm {
    target: FormTarget,
    fields: Vec<&'static FieldSpec>,
    /// Current values as JSON (`AppSettings` for App/Defaults, effective
    /// `GameSettings` for a game).
    snapshot: Value,
    /// Game target: global defaults as JSON and as settings.
    defaults_json: Value,
    defaults: GameSettings,
    effective: GameSettings,
    overridden: HashSet<String>,
    texts: HashMap<&'static str, Entity<InputState>>,
    choices: HashMap<&'static str, ChoiceState>,
    pairs: HashMap<&'static str, PairEditor>,
    sliders: HashMap<&'static str, Entity<SliderState>>,
    slider_writes: HashMap<&'static str, Task<()>>,
    /// The last value each slider reported. A drag reports every pointer
    /// move, mostly with an unchanged value.
    slider_seen: HashMap<&'static str, f32>,
    wine: Option<WinePicker>,
    /// The graphics cards and layers the GPU options need, once looked for.
    graphics: Option<Arc<HostGraphics>>,
    /// Windows' display adapters, for `iroriHideGpu`, once looked for.
    hideable_gpus: Option<Arc<Vec<irori_services::gpu_hide::Adapter>>>,
    /// The displays, for the HDR and gamescope hints (Linux, where those
    /// settings exist).
    displays: Option<Arc<irori_services::displays::DisplayReport>>,
    /// Game target: the prefixes it could use instead of its own, once
    /// looked for.
    prefix_choices: Option<Arc<Vec<irori_services::PrefixChoice>>>,
    /// Release tags offered by each version picker, once loaded.
    tool_versions: HashMap<&'static str, Vec<String>>,
    tool_versions_requested: bool,
    errors: HashMap<&'static str, SharedString>,
    filter: FormFilter,
    /// Optional sub-tab filter active in SettingsView compact mode.
    subtab: Option<&'static str>,
    /// Enables progressive disclosure, compact pairing, and "More options".
    compact: bool,
    /// Disclosure keys (`<target>-<subtab>`) currently expanded by the user.
    expanded_advanced: HashSet<String>,
    busy: HashSet<ActionKind>,
    import_summary: Option<SharedString>,
    heroic_detected: bool,
    lutris_detected: bool,
    bottles_detected: bool,
    faugus_detected: bool,
    playnite_detected: bool,
    /// Self-contained panels (controllers, EOS overlay, EGL sync, save sync).
    panels: HashMap<&'static str, AnyView>,
    /// Per-game OptiScaler panel when built for `FormTarget::Game`.
    optiscaler_panel: Option<Entity<OptiscalerPanel>>,
    /// Game target: the game is installed as this system's own build.
    native: bool,
    /// Game target: Irori can install Windows graphics mods (OptiScaler) for this game.
    supports_optiscaler: bool,
    /// The shell installed the Epic store handle.
    epic_available: bool,
    /// Language of the retained placeholders and select items.
    language: LanguageStamp,
    _subscriptions: Vec<Subscription>,
}

/// The shared launch-settings form used by the game defaults page and by
/// per-game settings. Build it with [`SettingsForm::new`] and a
/// [`FormTarget::Defaults`] or [`FormTarget::Game`] target.
pub type GameSettingsForm = SettingsForm;

impl SettingsForm {
    /// Create a form for `target`, showing every section that applies to it.
    pub fn new(target: FormTarget, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let fields: Vec<&'static FieldSpec> = registry::FIELDS
            .iter()
            .filter(|f| f.supported_on_host())
            .filter(|f| match &target {
                FormTarget::App => f.scope == Scope::App,
                FormTarget::Defaults => f.applies_to_game(false),
                FormTarget::Game(id) => {
                    f.applies_to_game(true)
                        && (!f.path.starts_with("iroriSandbox.")
                            || irori_runtime::sandbox::applies_to(id.runner))
                        && (f.path != irori_services::file_achievements::APP_ID_KEY
                            || irori_services::file_achievements::applies_to(id.runner))
                }
            })
            .collect();
        let sections = match target {
            FormTarget::App => FieldSection::APP.to_vec(),
            _ => FieldSection::GAME.to_vec(),
        };
        let mut this = Self {
            target,
            fields,
            snapshot: Value::Null,
            defaults_json: Value::Null,
            defaults: GameSettings::default(),
            effective: GameSettings::default(),
            overridden: HashSet::new(),
            texts: HashMap::new(),
            choices: HashMap::new(),
            pairs: HashMap::new(),
            sliders: HashMap::new(),
            slider_writes: HashMap::new(),
            slider_seen: HashMap::new(),
            wine: None,
            graphics: None,
            hideable_gpus: None,
            displays: None,
            prefix_choices: None,
            tool_versions: HashMap::new(),
            tool_versions_requested: false,
            errors: HashMap::new(),
            filter: FormFilter::Sections(sections),
            subtab: None,
            compact: false,
            expanded_advanced: HashSet::new(),
            busy: HashSet::new(),
            import_summary: None,
            heroic_detected: irori_ui::hooks::heroic_detected(),
            lutris_detected: irori_ui::hooks::lutris_detected(),
            bottles_detected: irori_ui::hooks::bottles_detected(),
            faugus_detected: irori_ui::hooks::faugus_detected(),
            playnite_detected: irori_extras::playnite::is_installed(),
            panels: HashMap::new(),
            optiscaler_panel: None,
            native: false,
            supports_optiscaler: false,
            epic_available: crate::stores::epic(cx).is_some(),
            language: LanguageStamp::new(),
            _subscriptions: Vec::new(),
        };
        this.read_snapshot(cx);
        this.build_controls(window, cx);
        if !matches!(this.target, FormTarget::App) {
            this.load_graphics(window, cx);
            if cfg!(target_os = "linux") {
                this.load_displays(window, cx);
            }
        }
        if this.fields.iter().any(|f| f.path == "winePrefix") {
            this.load_prefix_choices(window, cx);
        }
        if this.fields.iter().any(|f| f.path == "iroriHideGpu") {
            this.load_hideable_gpus(window, cx);
        }
        let settings_model = models(cx).settings.clone();
        this._subscriptions.push(
            cx.observe_in(&settings_model, window, |this, _, window, cx| {
                this.reload(window, cx)
            }),
        );
        if let FormTarget::Game(id) = &this.target {
            // Installing or moving the game can change whether it runs natively
            // or supports Windows graphics mods.
            let id = id.clone();
            let library = models(cx).library.clone();
            this._subscriptions
                .push(cx.observe(&library, move |this, _, cx| {
                    let game = backend(cx).game(&id);
                    let native = is_native(game.as_deref());
                    let supports_optiscaler = game
                        .as_deref()
                        .is_some_and(irori_services::supports_reshade);
                    if native != this.native || supports_optiscaler != this.supports_optiscaler {
                        this.native = native;
                        this.supports_optiscaler = supports_optiscaler;
                        cx.notify();
                    }
                }));
        }
        this
    }

    /// Per-game OptiScaler panel embedded in this form, when bound to a game.
    pub(crate) fn optiscaler_panel(&self) -> Option<Entity<OptiscalerPanel>> {
        self.optiscaler_panel.clone()
    }

    /// The edited target.
    pub fn target(&self) -> &FormTarget {
        &self.target
    }

    /// Show only these sections (in this order).
    pub fn set_sections(&mut self, sections: Vec<FieldSection>, cx: &mut Context<Self>) {
        self.filter = FormFilter::Sections(sections);
        cx.notify();
    }

    /// Show only the fields that differ from the game defaults (game
    /// target), grouped by section, each with its reset.
    pub fn set_changed_only(&mut self, cx: &mut Context<Self>) {
        self.filter = FormFilter::Changed;
        cx.notify();
    }

    /// Enable compact progressive-disclosure layout (used by App Settings).
    pub(crate) fn set_compact(&mut self, compact: bool, cx: &mut Context<Self>) {
        if self.compact != compact {
            self.compact = compact;
            cx.notify();
        }
    }

    /// Filter visible fields to one horizontal sub-tab when not searching.
    pub(crate) fn set_subtab(&mut self, subtab: Option<&'static str>, cx: &mut Context<Self>) {
        if self.subtab != subtab {
            self.subtab = subtab;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub(crate) fn set_advanced_open_for_test(
        &mut self,
        key: &str,
        open: bool,
        cx: &mut Context<Self>,
    ) {
        if open {
            self.expanded_advanced.insert(key.to_string());
        } else {
            self.expanded_advanced.remove(key);
        }
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn visible_paths(&self) -> Vec<&'static str> {
        self.visible_fields().map(|f| f.path).collect()
    }

    #[cfg(test)]
    pub(crate) fn is_compact(&self) -> bool {
        self.compact
    }

    /// Show the fields matching a search query, grouped by section.
    pub fn set_query(&mut self, query: &str, cx: &mut Context<Self>) {
        self.filter = FormFilter::Query(query.to_string());
        cx.notify();
    }

    /// Number of fields the current filter shows.
    pub fn visible_count(&self) -> usize {
        self.visible_fields().count()
    }

    /// Number of fields that differ from the global defaults (game target).
    pub fn overridden_count(&self) -> usize {
        self.overridden.len()
    }
}

impl Render for SettingsForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.language.changed() {
            cx.defer_in(window, |this, window, cx| this.retranslate(window, cx));
        }
        let searching = matches!(&self.filter, FormFilter::Query(_));
        let multi_section = match &self.filter {
            FormFilter::Sections(s) => s.len() > 1,
            FormFilter::Query(_) | FormFilter::Changed => true,
        };
        let fields: Vec<&'static FieldSpec> = self.visible_fields().collect();
        if self.compact && !searching {
            let (fields, blocked_count) = if matches!(self.target, FormTarget::Defaults) {
                let mut available = Vec::with_capacity(fields.len());
                let mut blocked = 0usize;
                for spec in fields {
                    if self.hardware_blocked(spec) {
                        blocked += 1;
                    } else {
                        available.push(spec);
                    }
                }
                (available, blocked)
            } else {
                (fields, 0)
            };
            let (essential, advanced): (Vec<_>, Vec<_>) = fields
                .into_iter()
                .partition(|f| !crate::layout::is_advanced(f) && !self.hardware_blocked(f));
            let mut body =
                self.render_field_list(&essential, multi_section, true, true, window, cx);
            if !advanced.is_empty() {
                let bucket_key = self.advanced_bucket_key();
                let open = self.expanded_advanced.contains(&bucket_key);
                let count = advanced.len();
                let toggle_key = bucket_key.clone();
                body = body.child(more_options_disclosure(
                    ElementId::Name(format!("more-{bucket_key}").into()),
                    open,
                    count,
                    cx.listener(move |this, _, _, cx| {
                        if !this.expanded_advanced.remove(&toggle_key) {
                            this.expanded_advanced.insert(toggle_key.clone());
                        }
                        cx.notify();
                    }),
                    || {
                        self.render_field_list(&advanced, multi_section, true, true, window, cx)
                            .into_any_element()
                    },
                ));
            }
            if blocked_count > 0 {
                let note = if blocked_count == 1 {
                    t("settings.form.hardware_hidden_one")
                } else {
                    t_args(
                        "settings.form.hardware_hidden_other",
                        &[("count", &blocked_count.to_string())],
                    )
                };
                body = body.child(
                    div()
                        .id("defaults-hardware-hidden")
                        .debug_selector(|| "defaults-hardware-hidden".into())
                        .pt_3()
                        .child(muted(note, cx).text_xs()),
                );
            }
            return body;
        }
        self.render_field_list(&fields, multi_section, false, self.compact, window, cx)
    }
}

/// Inline notice explaining why a whole group does not apply.
pub(crate) fn info_alert(id: &'static str, text: impl Into<SharedString>) -> Alert {
    Alert::info(id, text.into())
}
