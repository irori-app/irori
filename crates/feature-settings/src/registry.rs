//! The field registry: one static description per editable setting.
//!
//! A field is addressed by its JSON path in the serialized settings
//! (`AppSettings` for application fields, `GameSettings` for launch fields;
//! the two share keys because `AppSettings` flattens its game defaults). A
//! dotted path reaches into an object (`gamescope.fpsLimiter`,
//! `iroriUi.fontSize`). The first segment of a game field's path is the key
//! stored in a game's sparse override map.
//!
//! Labels, help, group names, tags, placeholders and option names are
//! translation keys (`settings.field.<id>.label`, …) resolved when rendered,
//! so a language change applies on the next frame. Keywords stay English and
//! the search also matches the English text, so a query works whichever
//! language the interface uses.
//!
//! The registry drives rendering order, grouping and the settings search.

use gpui_kit::SharedString;
use irori_runtime::DxTool;
use irori_ui::i18n::{LANGUAGES as UI_LANGUAGES, t};
use serde_json::Value;

/// Text shown for a registry item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    /// A translation key.
    Key(&'static str),
    /// Shown as written in every language (brand names, numbers, flags,
    /// language names).
    Verbatim(&'static str),
}

impl Label {
    /// The text in the interface language.
    pub fn text(self) -> SharedString {
        match self {
            Label::Key(key) => t(key),
            Label::Verbatim(s) => SharedString::new_static(s),
        }
    }

    /// The English text.
    pub fn english(self) -> &'static str {
        match self {
            Label::Key(key) => crate::i18n::english(key),
            Label::Verbatim(s) => s,
        }
    }
}

use Label::{Key, Verbatim};

/// Where a field is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldSection {
    /// Application paths, updates, tray, integrations, downloads.
    General,
    /// Theme, interface size and motion.
    Appearance,
    /// Helper binaries, controller timing, diagnostics and maintenance actions.
    Advanced,
    /// Wine/Proton, prefix, sync primitives, translation layers.
    Wine,
    /// GameMode, overlays, frame limits, gamescope.
    Performance,
    /// Graphics card, frame generation, upscaler upgrades, shader cache.
    Gpu,
    /// Arguments, wrappers, environment, scripts.
    Launch,
    /// Cloud save synchronisation and Ludusavi backups of one game.
    CloudSaves,
    /// One game's cover, background and logo.
    Artwork,
    /// Ludusavi, which backs up the saves of every game.
    Saves,
}

impl FieldSection {
    /// Sections of the shared launch-settings form, in display order.
    pub const GAME: [FieldSection; 6] = [
        FieldSection::Wine,
        FieldSection::Performance,
        FieldSection::Gpu,
        FieldSection::Launch,
        FieldSection::CloudSaves,
        FieldSection::Artwork,
    ];

    /// Sections holding application-wide fields.
    pub const APP: [FieldSection; 4] = [
        FieldSection::General,
        FieldSection::Appearance,
        FieldSection::Advanced,
        FieldSection::Saves,
    ];

    /// Translation key of the section heading.
    pub fn label_key(self) -> &'static str {
        match self {
            FieldSection::General => "settings.section.general",
            FieldSection::Appearance => "settings.section.appearance",
            FieldSection::Advanced => "settings.section.advanced",
            FieldSection::Wine => "settings.section.wine",
            FieldSection::Performance => "settings.section.performance",
            FieldSection::Gpu => "settings.section.gpu",
            FieldSection::Launch => "settings.section.launch",
            FieldSection::CloudSaves => "settings.section.game_saves",
            FieldSection::Saves => "settings.section.saves",
            FieldSection::Artwork => "settings.section.artwork",
        }
    }

    /// Section heading in the interface language.
    pub fn label(self) -> SharedString {
        t(self.label_key())
    }

    /// Whether the section belongs to the launch-settings form.
    pub fn is_game(self) -> bool {
        Self::GAME.contains(&self)
    }
}

/// Which settings context shows a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Application-wide settings only.
    App,
    /// Game defaults and per-game settings.
    Shared,
    /// Per-game settings only.
    GameOnly,
}

/// Input conversion of a text field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextKind {
    /// Free text stored as a string.
    Plain,
    /// A string of at most `max_len` ASCII digits (empty allowed).
    Digits { max_len: usize },
    /// An unsigned number stored as a JSON number.
    Number { min: u32, max: u32 },
    /// A directory path, with a folder picker.
    Directory,
    /// A file path, with a file picker.
    File,
    /// A secret: masked and never shown back in full.
    Secret,
}

/// A value offered by a choice field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceValue {
    Str(&'static str),
    Num(u64),
    /// JSON `false` (Heroic's spelling of "none" for `steamRuntime`).
    False,
}

impl ChoiceValue {
    pub fn to_json(self) -> Value {
        match self {
            ChoiceValue::Str(s) => Value::String(s.to_string()),
            ChoiceValue::Num(n) => Value::from(n),
            ChoiceValue::False => Value::Bool(false),
        }
    }

    /// Whether a stored value selects this option. `null` and `false` both
    /// mean "none".
    pub fn matches(self, value: &Value) -> bool {
        match (self, value) {
            (ChoiceValue::False, Value::Bool(false) | Value::Null) => true,
            (ChoiceValue::Str(s), Value::String(v)) => s == v,
            (ChoiceValue::Num(n), Value::Number(v)) => v.as_u64() == Some(n),
            _ => false,
        }
    }

    /// Stable key used as the select item value.
    pub fn key(self) -> String {
        match self {
            ChoiceValue::Str(s) => format!("s:{s}"),
            ChoiceValue::Num(n) => format!("n:{n}"),
            ChoiceValue::False => "none".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChoiceOption {
    pub value: ChoiceValue,
    pub label: Label,
}

const fn opt(value: ChoiceValue, label: Label) -> ChoiceOption {
    ChoiceOption { value, label }
}

/// Fields rendered by dedicated editors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomKind {
    /// Wine/Proton build picker fed by `irori_runtime::wine_installations`.
    WineVersion,
    /// `KEY=VALUE` environment variable rows.
    EnvVars,
    /// Wrapper command rows.
    Wrappers,
    /// GOG cloud-save locations.
    GogSaves,
    /// Theme picker from the theme registry.
    ThemeName,
    /// Graphics card picker fed by `irori_runtime::graphics`.
    GpuCard,
    /// Release picker of a tool installed into Wine prefixes.
    ToolVersion(DxTool),
    /// Base font size slider (interface zoom).
    FontSize,
    /// Library cover size slider.
    TileSize,
    /// Accent colour picker, with the theme's or system's accent as default.
    AccentColor,
    /// Interface typeface from the installed font families.
    FontFamily,
    /// A 0 to 100 percent slider.
    Percent,
}

/// Rows rendered by a self-contained panel (status plus actions) instead of
/// a single bound value. Their paths start with `panel.`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanelKind {
    /// Connected controllers with a refresh button.
    Controllers,
    /// Epic Online Services overlay: status, install, update, remove.
    EosOverlay,
    /// Link legendary with an Epic Games Launcher installation.
    EglSync,
    /// Manual cloud-save synchronisation of one game.
    SaveSync,
    /// One game's cover, background and logo, and fetching its details again.
    Artwork,
    /// Programs started alongside the game.
    LaunchPrograms,
    /// Launch presets: apply, save, apply to several games.
    LaunchPresets,
    /// Ludusavi: where it was found, or how to get it.
    Ludusavi,
    /// Ludusavi backups of one game.
    SaveBackups,
    /// The replay buffer's recorder, ways to save a clip and memory use.
    Replay,
    /// Sunshine: found or not, its login, syncing now, the last sync.
    Sunshine,
}

/// One-shot maintenance commands listed alongside fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionKind {
    ClearCaches,
    ResetSettings,
    OpenLogsFolder,
    ImportHeroic,
    ImportLutris,
    ImportBottles,
    ImportFaugus,
    /// Import Playnite's games (Windows).
    ImportPlaynite,
    /// Make Irori the program that opens Windows files (Linux).
    OpenWindowsFiles,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Toggle,
    Text(TextKind),
    Choice(&'static [ChoiceOption]),
    Custom(CustomKind),
    Action(ActionKind),
    Panel(PanelKind),
}

/// A field is only editable while the boolean at `path` equals `equals`;
/// otherwise it is shown disabled with the reason at `reason` (a
/// translation key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Condition {
    pub path: &'static str,
    pub equals: bool,
    pub reason: &'static str,
}

const fn when(path: &'static str, equals: bool, reason: &'static str) -> Condition {
    Condition {
        path,
        equals,
        reason,
    }
}

/// Something about the computer or the game a field needs. Unlike a
/// [`Condition`] it isn't a setting the user can flip here, so the reason
/// says what is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Linux: the GPU options set Mesa's, NVIDIA's and lsfg-vk's variables.
    Linux,
    /// More than one graphics card.
    SeveralGpus,
    /// No graphics card chosen in `iroriGpu` (it replaces this field).
    NoChosenGpu,
    /// A card on NVIDIA's own driver.
    NvidiaDriver,
    /// What NVIDIA Smooth Motion needs.
    SmoothMotion,
    /// lsfg-vk 2 and Lossless Scaling's DLL.
    Lsfg,
    /// A game run through Wine or Proton (or the defaults every game shares).
    WindowsGame,
}

/// What [`Need::blocker`] looks at.
#[derive(Debug, Clone, Copy)]
pub struct NeedContext<'a> {
    pub host: irori_core::HostPlatform,
    /// `None` while the graphics cards are looked for.
    pub graphics: Option<&'a irori_runtime::graphics::HostGraphics>,
    /// A graphics card is chosen for the game.
    pub chosen_gpu: bool,
    /// One game's settings, installed as a native Linux build.
    pub native_game: bool,
}

impl Need {
    /// Translation key of why the field can't apply; `None` when it can.
    pub fn blocker(self, cx: &NeedContext<'_>) -> Option<&'static str> {
        use irori_core::HostPlatform;
        use irori_runtime::graphics::{LsfgBlocker, SmoothMotionBlocker};
        let probing = "settings.needs.probing";
        match self {
            Need::Linux => match cx.host {
                HostPlatform::Linux => None,
                HostPlatform::Mac => Some("settings.needs.linux_mac"),
                HostPlatform::Windows => Some("settings.needs.linux_windows"),
            },
            Need::SeveralGpus => match cx.graphics {
                None => Some(probing),
                // A card chosen earlier stays changeable, so it can be undone.
                Some(g) if g.adapters.len() < 2 && !cx.chosen_gpu => {
                    Some("settings.needs.several_gpus")
                }
                Some(_) => None,
            },
            Need::NoChosenGpu => cx.chosen_gpu.then_some("settings.needs.no_chosen_gpu"),
            Need::NvidiaDriver => match cx.graphics {
                None => Some(probing),
                Some(g) if !g.has_nvidia_driver() => Some("settings.needs.nvidia"),
                Some(_) => None,
            },
            Need::SmoothMotion => match cx.graphics.map(|g| g.smooth_motion_blocker()) {
                None => Some(probing),
                Some(None) => None,
                Some(Some(SmoothMotionBlocker::NoNvidia)) => Some("settings.needs.nvidia"),
                Some(Some(SmoothMotionBlocker::OldCard)) => Some("settings.needs.rtx_40"),
                Some(Some(SmoothMotionBlocker::OldDriver)) => Some("settings.needs.nvidia_575"),
            },
            Need::Lsfg => match cx.graphics.map(|g| g.lsfg_blocker()) {
                None => Some(probing),
                Some(None) => None,
                Some(Some(LsfgBlocker::NotInstalled)) => Some("settings.needs.lsfg"),
                Some(Some(LsfgBlocker::Outdated)) => Some("settings.needs.lsfg_update"),
                Some(Some(LsfgBlocker::NoLosslessScaling)) => {
                    Some("settings.needs.lossless_scaling")
                }
            },
            Need::WindowsGame => cx.native_game.then_some("settings.needs.windows_game"),
        }
    }
}

/// Static description of one setting. Text members are translation keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct FieldSpec {
    /// JSON path of the value; unique identifier of the field.
    pub path: &'static str,
    /// Translation key of the label.
    pub label: &'static str,
    /// Translation key of one or two sentences under the label; empty for
    /// none.
    pub help: &'static str,
    /// Extra search terms, in English (they match in every language).
    pub keywords: &'static [&'static str],
    pub section: FieldSection,
    /// Translation key of the sub-heading within the section.
    pub group: &'static str,
    pub kind: FieldKind,
    pub scope: Scope,
    /// Short qualifier shown next to the label ("Epic", "Restart required").
    pub tag: Option<Label>,
    pub placeholder: Option<Label>,
    pub requires: &'static [Condition],
    /// What the computer or game must have; checked before `requires`.
    pub needs: &'static [Need],
    /// Only shown in the settings of a game installed as a native Linux build.
    pub native_only: bool,
}

impl FieldSpec {
    const fn new(
        path: &'static str,
        label: &'static str,
        section: FieldSection,
        group: &'static str,
        kind: FieldKind,
        scope: Scope,
    ) -> Self {
        Self {
            path,
            label,
            help: "",
            keywords: &[],
            section,
            group,
            kind,
            scope,
            tag: None,
            placeholder: None,
            requires: &[],
            needs: &[],
            native_only: false,
        }
    }

    const fn help(mut self, help: &'static str) -> Self {
        self.help = help;
        self
    }

    const fn keywords(mut self, keywords: &'static [&'static str]) -> Self {
        self.keywords = keywords;
        self
    }

    const fn tag(mut self, tag: Label) -> Self {
        self.tag = Some(tag);
        self
    }

    const fn placeholder(mut self, placeholder: Label) -> Self {
        self.placeholder = Some(placeholder);
        self
    }

    const fn requires(mut self, requires: &'static [Condition]) -> Self {
        self.requires = requires;
        self
    }

    const fn needs(mut self, needs: &'static [Need]) -> Self {
        self.needs = needs;
        self
    }

    const fn native_only(mut self) -> Self {
        self.native_only = true;
        self
    }

    /// The label in the interface language.
    pub fn label_text(&self) -> SharedString {
        t(self.label)
    }

    /// The help text in the interface language; empty when there is none.
    pub fn help_text(&self) -> SharedString {
        if self.help.is_empty() {
            SharedString::default()
        } else {
            t(self.help)
        }
    }

    /// The group heading in the interface language.
    pub fn group_text(&self) -> SharedString {
        t(self.group)
    }

    /// The placeholder in the interface language; empty when there is none.
    pub fn placeholder_text(&self) -> SharedString {
        self.placeholder.map(Label::text).unwrap_or_default()
    }

    /// Every translation key the field refers to.
    pub fn keys(&self) -> impl Iterator<Item = &'static str> + '_ {
        let labels = [self.tag, self.placeholder]
            .into_iter()
            .flatten()
            .chain(match self.kind {
                FieldKind::Choice(options) => options.iter().map(|o| o.label).collect(),
                _ => Vec::new(),
            })
            .filter_map(|l| match l {
                Label::Key(k) => Some(k),
                Label::Verbatim(_) => None,
            });
        [self.label, self.help, self.group, self.section.label_key()]
            .into_iter()
            .filter(|k| !k.is_empty())
            .chain(self.requires.iter().map(|c| c.reason))
            .chain(labels)
    }

    /// Key stored in a game's override map: the first path segment.
    pub fn override_key(&self) -> &'static str {
        crate::value::override_key(self.path)
    }

    /// Whether the field is shown for application settings, game defaults
    /// (`game == false`) or one game (`game == true`).
    pub fn supported_on_host(&self) -> bool {
        use irori_core::HostPlatform;
        let host = HostPlatform::current();
        let mac_only = matches!(
            self.path,
            "enableMsync"
                | "wineCrossoverBottle"
                | "advertiseAvxForRosetta"
                | "iroriRetinaMode"
                | "iroriCrossoverBottlesPath"
        );
        if mac_only && host != HostPlatform::Mac {
            return false;
        }
        // A portable Irori never adds itself to the Start menu.
        if self.path == "addStartMenuShortcuts" && irori_core::paths::is_portable() {
            return false;
        }
        // gpu-screen-recorder, which the replay buffer runs, is Linux only.
        // Only Linux desktops let Irori make itself the program for Windows
        // files (`irori_services::can_open_windows_files_by_default`).
        if (self.path == "panel.replay"
            || self.path.starts_with("iroriReplay.")
            || self.path == "action.open-windows-files")
            && host != HostPlatform::Linux
        {
            return false;
        }
        if host == HostPlatform::Linux {
            return true;
        }
        if self.path.starts_with("gamescope.")
            || matches!(
                self.path,
                "useGameMode"
                    | "showMangohud"
                    | "nvidiaPrime"
                    | "enableFsync"
                    | "enableFSR"
                    | "enableWineWayland"
                    | "enableHDR"
                    | "enableWoW64"
                    | "eacRuntime"
                    | "battlEyeRuntime"
                    | "disableUMU"
                    | "steamRuntime"
                    | "preferSystemLibs"
                    | "autoInstallDxvk"
                    | "autoInstallVkd3d"
                    | "autoInstallDxvkNvapi"
                    | "iroriDxvkVersion"
                    | "iroriVkd3dVersion"
                    | "iroriDescriptorHeap"
                    | "iroriDlssUpgrade"
                    | "iroriDlssIndicator"
                    | "iroriFsr4Upgrade"
                    | "iroriXessUpgrade"
                    | "downloadProtonToSteam"
                    | "showValveProton"
                    | "iroriShaderCacheSize"
                    | "iroriSandbox.noNetwork"
                    | "iroriSandbox.noHome"
            )
        {
            return false;
        }
        host != HostPlatform::Windows
            || (self.section != FieldSection::Wine
                && self.path != "iroriCompatToolsPath"
                && !matches!(self.path, "showFps" | "enableDXVKFpsLimit" | "DXVKFpsCap"))
    }

    pub fn applies_to_game(&self, per_game: bool) -> bool {
        match self.scope {
            Scope::App => false,
            Scope::Shared => true,
            Scope::GameOnly => per_game,
        }
    }

    /// Whether every whitespace-separated token of `query` occurs in the
    /// label, help, group, section or tag (in the interface language or in
    /// English), the keywords or the path (case-insensitive).
    pub fn matches(&self, query: &str) -> bool {
        let mut tokens = query.split_whitespace().peekable();
        if tokens.peek().is_none() {
            return false;
        }
        let haystack = self.haystack();
        tokens.all(|t| haystack.contains(&t.to_lowercase()))
    }

    fn haystack(&self) -> String {
        let mut s = String::with_capacity(256);
        let mut push = |part: &str| {
            s.push_str(part);
            s.push('\n');
        };
        let tag = self.tag.map(Label::text).unwrap_or_default();
        for part in [
            self.label_text(),
            self.help_text(),
            self.group_text(),
            self.section.label(),
            tag,
        ] {
            push(&part);
        }
        for key in [self.label, self.help, self.group, self.section.label_key()] {
            if !key.is_empty() {
                push(crate::i18n::english(key));
            }
        }
        if let Some(tag) = self.tag {
            push(tag.english());
        }
        push(self.path);
        for k in self.keywords {
            push(k);
        }
        s.to_lowercase()
    }
}

use FieldKind::{Action, Choice, Custom, Panel, Text, Toggle};
use FieldSection::{
    Advanced, Appearance, Artwork, CloudSaves, General, Gpu, Launch, Performance, Saves, Wine,
};
use Scope::{App, GameOnly, Shared};

/// Interface languages by native name (never translated).
static LANGUAGES: [ChoiceOption; UI_LANGUAGES.len()] = {
    let mut out = [opt(ChoiceValue::Str("en"), Verbatim("English")); UI_LANGUAGES.len()];
    let mut i = 0;
    while i < UI_LANGUAGES.len() {
        let lang = UI_LANGUAGES[i];
        out[i] = opt(ChoiceValue::Str(lang.code), Verbatim(lang.native_name));
        i += 1;
    }
    out
};

const TOP_SECTION: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("recently_played"),
        Key("settings.option.top_section.recently_played"),
    ),
    opt(
        ChoiceValue::Str("recently_played_installed"),
        Key("settings.option.top_section.recently_played_installed"),
    ),
    opt(
        ChoiceValue::Str("favourites"),
        Key("settings.option.top_section.favourites"),
    ),
    opt(
        ChoiceValue::Str("disabled"),
        Key("settings.option.top_section.nothing"),
    ),
];

const THEME_MODE: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("system"),
        Key("settings.option.theme_mode.system"),
    ),
    opt(
        ChoiceValue::Str("dark"),
        Key("settings.option.theme_mode.dark"),
    ),
    opt(
        ChoiceValue::Str("light"),
        Key("settings.option.theme_mode.light"),
    ),
];

const TRANSLUCENCY: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("auto"),
        Key("settings.option.translucency.auto"),
    ),
    opt(
        ChoiceValue::Str("off"),
        Key("settings.option.translucency.off"),
    ),
    opt(
        ChoiceValue::Str("subtle"),
        Key("settings.option.translucency.subtle"),
    ),
    opt(
        ChoiceValue::Str("strong"),
        Key("settings.option.translucency.strong"),
    ),
];

const BIG_PICTURE_SCALE: &[ChoiceOption] = &[
    opt(ChoiceValue::Num(100), Verbatim("100%")),
    opt(ChoiceValue::Num(125), Verbatim("125%")),
    opt(ChoiceValue::Num(150), Verbatim("150%")),
    opt(ChoiceValue::Num(175), Verbatim("175%")),
    opt(ChoiceValue::Num(200), Verbatim("200%")),
];

const LIBRARY_LAYOUT: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("grid"),
        Key("settings.option.library_layout.grid"),
    ),
    opt(
        ChoiceValue::Str("list"),
        Key("settings.option.library_layout.list"),
    ),
    opt(
        ChoiceValue::Str("focus"),
        Key("settings.option.library_layout.focus"),
    ),
];

const DENSITY: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("compact"),
        Key("settings.option.density.compact"),
    ),
    opt(
        ChoiceValue::Str("comfortable"),
        Key("settings.option.density.comfortable"),
    ),
    opt(
        ChoiceValue::Str("spacious"),
        Key("settings.option.density.spacious"),
    ),
];

const LIBRARY_BACKGROUND: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("none"),
        Key("settings.option.library_background.none"),
    ),
    opt(
        ChoiceValue::Str("game_art"),
        Key("settings.option.library_background.game_art"),
    ),
    opt(
        ChoiceValue::Str("picture"),
        Key("settings.option.library_background.picture"),
    ),
];

const STEAM_RUNTIME: &[ChoiceOption] = &[
    opt(
        ChoiceValue::False,
        Key("settings.option.steam_runtime.none"),
    ),
    opt(
        ChoiceValue::Str("umu-scout"),
        Verbatim("Steam Linux Runtime 1.0 (scout)"),
    ),
    opt(
        ChoiceValue::Str("umu-soldier"),
        Verbatim("Steam Linux Runtime 2.0 (soldier)"),
    ),
    opt(
        ChoiceValue::Str("umu-sniper"),
        Verbatim("Steam Linux Runtime 3.0 (sniper)"),
    ),
    opt(
        ChoiceValue::Str("umu-steamrt4"),
        Verbatim("Steam Linux Runtime 4.0"),
    ),
];

const SHARPNESS: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Num(1),
        Key("settings.option.sharpness.softest"),
    ),
    opt(ChoiceValue::Num(2), Verbatim("2")),
    opt(ChoiceValue::Num(3), Verbatim("3")),
    opt(ChoiceValue::Num(4), Verbatim("4")),
    opt(
        ChoiceValue::Num(5),
        Key("settings.option.sharpness.sharpest"),
    ),
];

const UPDATE_HOURS: &[ChoiceOption] = &[
    opt(ChoiceValue::Str(""), Key("settings.option.update_hour.any")),
    opt(ChoiceValue::Str("0"), Verbatim("00:00")),
    opt(ChoiceValue::Str("1"), Verbatim("01:00")),
    opt(ChoiceValue::Str("2"), Verbatim("02:00")),
    opt(ChoiceValue::Str("3"), Verbatim("03:00")),
    opt(ChoiceValue::Str("4"), Verbatim("04:00")),
    opt(ChoiceValue::Str("5"), Verbatim("05:00")),
    opt(ChoiceValue::Str("6"), Verbatim("06:00")),
    opt(ChoiceValue::Str("7"), Verbatim("07:00")),
    opt(ChoiceValue::Str("8"), Verbatim("08:00")),
    opt(ChoiceValue::Str("9"), Verbatim("09:00")),
    opt(ChoiceValue::Str("10"), Verbatim("10:00")),
    opt(ChoiceValue::Str("11"), Verbatim("11:00")),
    opt(ChoiceValue::Str("12"), Verbatim("12:00")),
    opt(ChoiceValue::Str("13"), Verbatim("13:00")),
    opt(ChoiceValue::Str("14"), Verbatim("14:00")),
    opt(ChoiceValue::Str("15"), Verbatim("15:00")),
    opt(ChoiceValue::Str("16"), Verbatim("16:00")),
    opt(ChoiceValue::Str("17"), Verbatim("17:00")),
    opt(ChoiceValue::Str("18"), Verbatim("18:00")),
    opt(ChoiceValue::Str("19"), Verbatim("19:00")),
    opt(ChoiceValue::Str("20"), Verbatim("20:00")),
    opt(ChoiceValue::Str("21"), Verbatim("21:00")),
    opt(ChoiceValue::Str("22"), Verbatim("22:00")),
    opt(ChoiceValue::Str("23"), Verbatim("23:00")),
];

const UPSCALE_METHOD: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("fsr"),
        Verbatim("AMD FidelityFX Super Resolution 1.0 (FSR)"),
    ),
    opt(
        ChoiceValue::Str("nis"),
        Verbatim("NVIDIA Image Scaling (NIS)"),
    ),
    opt(
        ChoiceValue::Str("integer"),
        Key("settings.option.upscale_method.integer"),
    ),
    opt(
        ChoiceValue::Str("stretch"),
        Key("settings.option.upscale_method.stretch"),
    ),
];

const WINDOW_TYPE: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("fullscreen"),
        Key("settings.option.window_type.fullscreen"),
    ),
    opt(
        ChoiceValue::Str("borderless"),
        Key("settings.option.window_type.borderless"),
    ),
    opt(
        ChoiceValue::Str("windowed"),
        Key("settings.option.window_type.windowed"),
    ),
];

const LSFG_MULTIPLIER: &[ChoiceOption] = &[
    opt(ChoiceValue::Num(2), Verbatim("2×")),
    opt(ChoiceValue::Num(3), Verbatim("3×")),
    opt(ChoiceValue::Num(4), Verbatim("4×")),
    opt(ChoiceValue::Num(6), Verbatim("6×")),
    opt(ChoiceValue::Num(8), Verbatim("8×")),
];

const LSFG_FLOW_SCALE: &[ChoiceOption] = &[
    opt(ChoiceValue::Num(25), Verbatim("25%")),
    opt(ChoiceValue::Num(50), Verbatim("50%")),
    opt(ChoiceValue::Num(75), Verbatim("75%")),
    opt(ChoiceValue::Num(85), Verbatim("85%")),
    opt(ChoiceValue::Num(100), Verbatim("100%")),
];

const REPLAY_SECONDS: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Num(15),
        Key("settings.option.replay_seconds.15"),
    ),
    opt(
        ChoiceValue::Num(30),
        Key("settings.option.replay_seconds.30"),
    ),
    opt(
        ChoiceValue::Num(60),
        Key("settings.option.replay_seconds.60"),
    ),
    opt(
        ChoiceValue::Num(120),
        Key("settings.option.replay_seconds.120"),
    ),
    opt(
        ChoiceValue::Num(300),
        Key("settings.option.replay_seconds.300"),
    ),
    opt(
        ChoiceValue::Num(600),
        Key("settings.option.replay_seconds.600"),
    ),
];

const REPLAY_QUALITY: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("medium"),
        Key("settings.option.replay_quality.medium"),
    ),
    opt(
        ChoiceValue::Str("high"),
        Key("settings.option.replay_quality.high"),
    ),
    opt(
        ChoiceValue::Str("very_high"),
        Key("settings.option.replay_quality.very_high"),
    ),
    opt(
        ChoiceValue::Str("ultra"),
        Key("settings.option.replay_quality.ultra"),
    ),
];

const REPLAY_AUDIO: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("desktop"),
        Key("settings.option.replay_audio.desktop"),
    ),
    opt(
        ChoiceValue::Str("desktop_and_mic"),
        Key("settings.option.replay_audio.desktop_and_mic"),
    ),
    opt(
        ChoiceValue::Str("none"),
        Key("settings.option.replay_audio.none"),
    ),
];

const REPLAY_CAPTURE: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("screen"),
        Key("settings.option.replay_capture.screen"),
    ),
    opt(
        ChoiceValue::Str("portal"),
        Key("settings.option.replay_capture.portal"),
    ),
];

const NEEDS_TRAY: &[Condition] = &[when("noTrayIcon", false, "settings.requires.tray_icon")];
const NEEDS_EXIT_TO_TRAY: &[Condition] = &[
    when("noTrayIcon", false, "settings.requires.tray_icon"),
    when("exitToTray", true, "settings.requires.close_to_tray"),
];
const NEEDS_START_WITH_SYSTEM: &[Condition] = &[
    when("noTrayIcon", false, "settings.requires.tray_icon"),
    when(
        "iroriStartWithSystem",
        true,
        "settings.requires.start_with_system",
    ),
];
const NEEDS_WAYLAND: &[Condition] = &[when(
    "enableWineWayland",
    true,
    "settings.requires.wine_wayland",
)];
const NEEDS_FSR: &[Condition] = &[when("enableFSR", true, "settings.requires.fsr")];
const NEEDS_DXVK: &[Condition] = &[when("autoInstallDxvk", true, "settings.requires.dxvk")];
const NEEDS_FPS_LIMIT: &[Condition] = &[when(
    "enableDXVKFpsLimit",
    true,
    "settings.requires.fps_limit",
)];
const NEEDS_NVAPI: &[Condition] = &[when(
    "autoInstallDxvkNvapi",
    true,
    "settings.requires.dxvk_nvapi",
)];
const NEEDS_LSFG: &[Condition] = &[when("iroriLsfg.enabled", true, "settings.requires.lsfg")];
const NEEDS_VKD3D: &[Condition] = &[when("autoInstallVkd3d", true, "settings.requires.vkd3d")];
const NEEDS_SUNSHINE: &[Condition] = &[when(
    "iroriSunshine.enabled",
    true,
    "settings.requires.sunshine",
)];
const SUNSHINE_GAMES: &[ChoiceOption] = &[
    opt(
        ChoiceValue::Str("installed"),
        Key("settings.option.sunshine_games.installed"),
    ),
    opt(
        ChoiceValue::Str("favourites"),
        Key("settings.option.sunshine_games.favourites"),
    ),
];
const NEEDS_REPLAY: &[Condition] = &[when(
    "iroriReplay.enabled",
    true,
    "settings.requires.replay",
)];
const NEEDS_GAMESCOPE: &[Condition] = &[when(
    "gamescope.iroriEnabled",
    true,
    "settings.requires.gamescope",
)];
const NEEDS_UPSCALING: &[Condition] = &[
    when(
        "gamescope.iroriEnabled",
        true,
        "settings.requires.gamescope",
    ),
    when(
        "gamescope.enableUpscaling",
        true,
        "settings.requires.gamescope_upscaling",
    ),
];
const NEEDS_LIMITER: &[Condition] = &[
    when(
        "gamescope.iroriEnabled",
        true,
        "settings.requires.gamescope",
    ),
    when(
        "gamescope.enableLimiter",
        true,
        "settings.requires.gamescope_limiter",
    ),
];

const NEEDS_CONTROLLER: &[Condition] = &[when(
    "disableController",
    false,
    "settings.requires.controller_navigation",
)];

const fn app(
    path: &'static str,
    label: &'static str,
    section: FieldSection,
    group: &'static str,
    kind: FieldKind,
) -> FieldSpec {
    FieldSpec::new(path, label, section, group, kind, App)
}

const fn game(
    path: &'static str,
    label: &'static str,
    section: FieldSection,
    group: &'static str,
    kind: FieldKind,
) -> FieldSpec {
    FieldSpec::new(path, label, section, group, kind, Shared)
}

const fn game_only(
    path: &'static str,
    label: &'static str,
    section: FieldSection,
    group: &'static str,
    kind: FieldKind,
) -> FieldSpec {
    FieldSpec::new(path, label, section, group, kind, GameOnly)
}

/// Every field, in display order.
pub static FIELDS: &[FieldSpec] = &[
    // ---- General ----
    app(
        "uiLanguage",
        "settings.field.ui_language.label",
        General,
        "settings.group.language",
        Choice(&LANGUAGES),
    )
    .help("settings.field.ui_language.help")
    .keywords(&["language", "locale", "translation", "interface"]),
    app(
        "defaultInstallPath",
        "settings.field.default_install_path.label",
        General,
        "settings.group.folders",
        Text(TextKind::Directory),
    )
    .help("settings.field.default_install_path.help")
    .keywords(&["path", "directory", "games"]),
    app(
        "defaultWinePrefixDir",
        "settings.field.default_wine_prefix_dir.label",
        General,
        "settings.group.folders",
        Text(TextKind::Directory),
    )
    .help("settings.field.default_wine_prefix_dir.help")
    .keywords(&["prefix", "path", "wineprefix"]),
    app(
        "defaultSteamPath",
        "settings.field.default_steam_path.label",
        General,
        "settings.group.folders",
        Text(TextKind::Directory),
    )
    .help("settings.field.default_steam_path.help")
    .keywords(&["steam", "proton", "path"]),
    app(
        "iroriCrossoverBottlesPath",
        "settings.field.crossover_bottles_path.label",
        General,
        "settings.group.folders",
        Text(TextKind::Directory),
    )
    .tag(Verbatim("CrossOver"))
    .help("settings.field.crossover_bottles_path.help")
    .placeholder(Verbatim("~/Library/Application Support/CrossOver/Bottles"))
    .keywords(&["crossover", "bottle", "bottles", "path", "mac"]),
    app(
        "iroriCheckForAppUpdates",
        "settings.field.check_for_app_updates.label",
        General,
        "settings.group.updates",
        Toggle,
    )
    .help("settings.field.check_for_app_updates.help")
    .keywords(&["irori", "version", "update", "release", "new"]),
    app(
        "checkForUpdatesOnStartup",
        "settings.field.check_for_updates_on_startup.label",
        General,
        "settings.group.updates",
        Toggle,
    )
    .keywords(&["update", "startup"]),
    app(
        "checkUpdatesInterval",
        "settings.field.check_updates_interval.label",
        General,
        "settings.group.updates",
        Text(TextKind::Number { min: 5, max: 1440 }),
    )
    .help("settings.field.check_updates_interval.help")
    .keywords(&["minutes", "update", "frequency"]),
    app(
        "autoUpdateGames",
        "settings.field.auto_update_games.label",
        General,
        "settings.group.updates",
        Toggle,
    )
    .help("settings.field.auto_update_games.help")
    .keywords(&["update", "automatic"]),
    app(
        "iroriAutoUpdateComponents",
        "settings.field.auto_update_components.label",
        General,
        "settings.group.updates",
        Toggle,
    )
    .help("settings.field.auto_update_components.help")
    .keywords(&["update", "automatic", "wine", "proton", "dxvk", "emulator"]),
    app(
        "iroriAutoUpdateFrom",
        "settings.field.auto_update_from.label",
        General,
        "settings.group.updates",
        Choice(UPDATE_HOURS),
    )
    .help("settings.field.auto_update_window.help")
    .keywords(&["update", "schedule", "night", "window", "time"]),
    app(
        "iroriAutoUpdateUntil",
        "settings.field.auto_update_until.label",
        General,
        "settings.group.updates",
        Choice(UPDATE_HOURS),
    )
    .keywords(&["update", "schedule", "night", "window", "time"]),
    app(
        "iroriAutoUpdateUnmetered",
        "settings.field.auto_update_unmetered.label",
        General,
        "settings.group.updates",
        Toggle,
    )
    .help("settings.field.auto_update_unmetered.help")
    .keywords(&["update", "metered", "data limit", "mobile", "hotspot"]),
    app(
        "noTrayIcon",
        "settings.field.no_tray_icon.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .tag(Key("settings.tag.restart_required"))
    .keywords(&["tray", "systray", "icon"]),
    app(
        "exitToTray",
        "settings.field.exit_to_tray.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.exit_to_tray.help")
    .keywords(&["tray", "exit", "close", "background"])
    .requires(NEEDS_TRAY),
    app(
        "startInTray",
        "settings.field.start_in_tray.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.start_in_tray.help")
    .keywords(&["tray", "minimized", "startup"])
    .requires(NEEDS_EXIT_TO_TRAY),
    app(
        "iroriStartWithSystem",
        "settings.field.start_with_system.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.start_with_system.help")
    .keywords(&["autostart", "login", "startup", "boot", "log in"]),
    app(
        "iroriStartWithSystemMinimized",
        "settings.field.start_with_system_minimized.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.start_with_system_minimized.help")
    .keywords(&["autostart", "tray", "minimized", "startup"])
    .requires(NEEDS_START_WITH_SYSTEM),
    app(
        "minimizeOnLaunch",
        "settings.field.minimize_on_launch.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.minimize_on_launch.help")
    .keywords(&["minimize", "hide", "launch"])
    .requires(NEEDS_TRAY),
    app(
        "iroriShowLogOnLaunch",
        "settings.field.show_log_on_launch.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.show_log_on_launch.help")
    .keywords(&["log", "console", "output", "launch"]),
    app(
        "darkTrayIcon",
        "settings.field.dark_tray_icon.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .tag(Key("settings.tag.restart_required"))
    .keywords(&["tray", "icon", "dark"])
    .requires(NEEDS_TRAY),
    app(
        "hideWindowOnProtocolLaunch",
        "settings.field.hide_window_on_protocol_launch.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.hide_window_on_protocol_launch.help")
    .keywords(&["protocol", "url", "heroic://", "link", "shortcut"]),
    app(
        "startInConsoleMode",
        "settings.field.start_in_console_mode.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .keywords(&["console", "gamepad", "fullscreen", "big picture"]),
    app(
        "iroriUi.bigPictureInGameMode",
        "settings.field.big_picture_in_game_mode.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.big_picture_in_game_mode.help")
    .keywords(&[
        "steam deck",
        "game mode",
        "gamescope",
        "console",
        "big picture",
    ]),
    app(
        "iroriUi.bigPictureOnController",
        "settings.field.big_picture_on_controller.label",
        General,
        "settings.group.tray_and_window",
        Toggle,
    )
    .help("settings.field.big_picture_on_controller.help")
    .keywords(&["controller", "gamepad", "console", "big picture", "connect"]),
    app(
        "addDesktopShortcuts",
        "settings.field.add_desktop_shortcuts.label",
        General,
        "settings.group.new_installs",
        Toggle,
    )
    .help("settings.field.add_desktop_shortcuts.help")
    .keywords(&["shortcut", "desktop"]),
    app(
        "addStartMenuShortcuts",
        "settings.field.add_start_menu_shortcuts.label",
        General,
        "settings.group.new_installs",
        Toggle,
    )
    .keywords(&["shortcut", "start menu", "applications"]),
    app(
        "addSteamShortcuts",
        "settings.field.add_steam_shortcuts.label",
        General,
        "settings.group.new_installs",
        Toggle,
    )
    .help("settings.field.add_steam_shortcuts.help")
    .keywords(&["steam", "shortcut", "deck"]),
    app(
        "libraryTopSection",
        "settings.field.library_top_section.label",
        General,
        "settings.group.library",
        Choice(TOP_SECTION),
    )
    .keywords(&["recent", "favourites", "top"]),
    app(
        "iroriUi.stores.epic",
        "settings.field.show_store_epic.label",
        General,
        "settings.group.stores",
        Toggle,
    )
    .help("settings.field.show_stores.help")
    .keywords(&["store", "hide", "epic", "legendary"]),
    app(
        "iroriUi.stores.gog",
        "settings.field.show_store_gog.label",
        General,
        "settings.group.stores",
        Toggle,
    )
    .help("settings.field.show_stores.help")
    .keywords(&["store", "hide", "gog"]),
    app(
        "iroriUi.stores.amazon",
        "settings.field.show_store_amazon.label",
        General,
        "settings.group.stores",
        Toggle,
    )
    .help("settings.field.show_stores.help")
    .keywords(&["store", "hide", "amazon", "prime", "nile"]),
    app(
        "maxRecentGames",
        "settings.field.max_recent_games.label",
        General,
        "settings.group.library",
        Text(TextKind::Number { min: 1, max: 20 }),
    )
    .keywords(&["recent", "tray"]),
    app(
        "iroriAchievementNotices",
        "settings.field.achievement_notices.label",
        General,
        "settings.group.library",
        Toggle,
    )
    .help("settings.field.achievement_notices.help")
    .keywords(&[
        "achievements",
        "trophies",
        "unlocked",
        "notification",
        "retroachievements",
    ]),
    app(
        "discordRPC",
        "settings.field.discord_rpc.label",
        General,
        "settings.group.integrations",
        Toggle,
    )
    .help("settings.field.discord_rpc.help")
    .keywords(&["discord", "presence", "rpc"]),
    app(
        "discordClientId",
        "settings.field.discord_client_id.label",
        General,
        "settings.group.integrations",
        Text(TextKind::Digits { max_len: 20 }),
    )
    .help("settings.field.discord_client_id.help")
    .placeholder(Key("settings.field.discord_client_id.placeholder"))
    .keywords(&[
        "discord",
        "presence",
        "rpc",
        "client id",
        "application",
        "developers",
    ]),
    app(
        "iroriSunshine.enabled",
        "settings.field.sunshine_enabled.label",
        General,
        "settings.group.integrations",
        Toggle,
    )
    .help("settings.field.sunshine_enabled.help")
    .keywords(&[
        "sunshine",
        "moonlight",
        "stream",
        "streaming",
        "remote play",
    ]),
    app(
        "iroriSunshine.games",
        "settings.field.sunshine_games.label",
        General,
        "settings.group.integrations",
        Choice(SUNSHINE_GAMES),
    )
    .keywords(&["sunshine", "moonlight", "stream", "favourites"])
    .requires(NEEDS_SUNSHINE),
    app(
        "iroriSunshine.bigPicture",
        "settings.field.sunshine_big_picture.label",
        General,
        "settings.group.integrations",
        Toggle,
    )
    .help("settings.field.sunshine_big_picture.help")
    .keywords(&["sunshine", "moonlight", "big picture", "couch"])
    .requires(NEEDS_SUNSHINE),
    app(
        "iroriSunshine.matchClient",
        "settings.field.sunshine_match_client.label",
        General,
        "settings.group.integrations",
        Toggle,
    )
    .help("settings.field.sunshine_match_client.help")
    .keywords(&[
        "sunshine",
        "moonlight",
        "resolution",
        "refresh rate",
        "fps",
        "hdr",
        "gamescope",
    ])
    .requires(NEEDS_SUNSHINE),
    app(
        "panel.sunshine",
        "settings.field.panel_sunshine.label",
        General,
        "settings.group.integrations",
        Panel(PanelKind::Sunshine),
    )
    .keywords(&["sunshine", "moonlight", "stream", "password", "login"]),
    app(
        "disablePlaytimeSync",
        "settings.field.disable_playtime_sync.label",
        General,
        "settings.group.integrations",
        Toggle,
    )
    .tag(Verbatim("GOG · Epic"))
    .keywords(&["playtime", "sync", "gog", "epic"]),
    app(
        "disableGOGPresence",
        "settings.field.disable_gog_presence.label",
        General,
        "settings.group.integrations",
        Toggle,
    )
    .tag(Verbatim("GOG"))
    .keywords(&["presence", "gog", "galaxy", "online"]),
    app(
        "cometSupport",
        "settings.field.comet_support.label",
        General,
        "settings.group.integrations",
        Toggle,
    )
    .tag(Verbatim("GOG"))
    .help("settings.field.comet_support.help")
    .keywords(&["comet", "galaxy", "achievements", "gog"]),
    app(
        "zoomPlatform",
        "settings.field.zoom_platform.label",
        General,
        "settings.group.integrations",
        Toggle,
    )
    .keywords(&["zoom", "store"]),
    app(
        "iroriItchIo",
        "settings.field.itch_io.label",
        General,
        "settings.group.integrations",
        Toggle,
    )
    .help("settings.field.itch_io.help")
    .keywords(&["itch", "itch.io", "store", "indie"]),
    app(
        "action.open-windows-files",
        "settings.field.action_open_windows_files.label",
        General,
        "settings.group.integrations",
        Action(ActionKind::OpenWindowsFiles),
    )
    .help("settings.field.action_open_windows_files.help")
    .keywords(&[
        "exe",
        "msi",
        "bat",
        "reg",
        "double click",
        "open with",
        "file",
        "wine",
        "default",
    ]),
    app(
        "maxWorkers",
        "settings.field.max_workers.label",
        General,
        "settings.group.downloads",
        Text(TextKind::Number { min: 0, max: 64 }),
    )
    .help("settings.field.max_workers.help")
    .keywords(&["workers", "threads", "parallel", "download"]),
    app(
        "maxDownloadSpeed",
        "settings.field.max_download_speed.label",
        General,
        "settings.group.downloads",
        Text(TextKind::Number {
            min: 0,
            max: 10_000_000,
        }),
    )
    .help("settings.field.max_download_speed.help")
    .keywords(&["bandwidth", "speed", "limit", "throttle", "download"]),
    app(
        "downloadNoHttps",
        "settings.field.download_no_https.label",
        General,
        "settings.group.downloads",
        Toggle,
    )
    .tag(Verbatim("Epic"))
    .help("settings.field.download_no_https.help")
    .keywords(&["https", "lancache", "cdn", "cache"]),
    app(
        "panel.controllers",
        "settings.field.panel_controllers.label",
        General,
        "settings.group.controller",
        Panel(PanelKind::Controllers),
    )
    .keywords(&[
        "gamepad",
        "controller",
        "joystick",
        "xbox",
        "playstation",
        "dualsense",
        "steam deck",
        "battery",
    ]),
    app(
        "disableController",
        "settings.field.disable_controller.label",
        General,
        "settings.group.controller",
        Toggle,
    )
    .help("settings.field.disable_controller.help")
    .keywords(&["gamepad", "controller", "joystick"]),
    app(
        "gamepadInitialRepeatDelay",
        "settings.field.gamepad_initial_repeat_delay.label",
        General,
        "settings.group.controller",
        Text(TextKind::Number { min: 50, max: 500 }),
    )
    .help("settings.field.gamepad_initial_repeat_delay.help")
    .keywords(&["gamepad", "controller", "repeat", "delay"])
    .requires(NEEDS_CONTROLLER),
    app(
        "gamepadRepeatDelay",
        "settings.field.gamepad_repeat_delay.label",
        General,
        "settings.group.controller",
        Text(TextKind::Number { min: 5, max: 150 }),
    )
    .help("settings.field.gamepad_repeat_delay.help")
    .keywords(&["gamepad", "controller", "repeat", "frequency"])
    .requires(NEEDS_CONTROLLER),
    // ---- Appearance ----
    app(
        "iroriUi.theme",
        "settings.field.ui_theme.label",
        Appearance,
        "settings.group.theme",
        Choice(THEME_MODE),
    )
    .keywords(&["dark", "light", "system", "mode"]),
    app(
        "iroriUi.themeName",
        "settings.field.ui_theme_name.label",
        Appearance,
        "settings.group.theme",
        Custom(CustomKind::ThemeName),
    )
    .keywords(&[
        "colors",
        "palette",
        "catppuccin",
        "gruvbox",
        "omarchy",
        "cosmic",
    ]),
    app(
        "iroriUi.systemAccent",
        "settings.field.ui_system_accent.label",
        Appearance,
        "settings.group.theme",
        Toggle,
    )
    .help("settings.field.ui_system_accent.help")
    .keywords(&["accent", "colour", "color", "highlight", "system", "tint"]),
    app(
        "iroriUi.accentColor",
        "settings.field.ui_accent_color.label",
        Appearance,
        "settings.group.theme",
        Custom(CustomKind::AccentColor),
    )
    .help("settings.field.ui_accent_color.help")
    .keywords(&["accent", "colour", "color", "highlight", "tint", "picker"]),
    app(
        "iroriUi.translucency",
        "settings.field.ui_translucency.label",
        Appearance,
        "settings.group.theme",
        Choice(TRANSLUCENCY),
    )
    .help("settings.field.ui_translucency.help")
    .keywords(&[
        "translucency",
        "transparency",
        "blur",
        "glass",
        "vibrancy",
        "mica",
        "acrylic",
    ]),
    app(
        "iroriUi.fontSize",
        "settings.field.ui_font_size.label",
        Appearance,
        "settings.group.size",
        Custom(CustomKind::FontSize),
    )
    .help("settings.field.ui_font_size.help")
    .keywords(&["zoom", "font", "scale", "text size"]),
    app(
        "iroriUi.tileSize",
        "settings.field.ui_tile_size.label",
        Appearance,
        "settings.group.size",
        Custom(CustomKind::TileSize),
    )
    .help("settings.field.ui_tile_size.help")
    .keywords(&["tile", "grid", "cover", "library"]),
    app(
        "iroriUi.density",
        "settings.field.ui_density.label",
        Appearance,
        "settings.group.size",
        Choice(DENSITY),
    )
    .help("settings.field.ui_density.help")
    .keywords(&[
        "density", "compact", "spacious", "spacing", "padding", "dense",
    ]),
    app(
        "iroriUi.bigPictureScale",
        "settings.field.ui_big_picture_scale.label",
        Appearance,
        "settings.group.size",
        Choice(BIG_PICTURE_SCALE),
    )
    .help("settings.field.ui_big_picture_scale.help")
    .keywords(&[
        "big picture",
        "tv",
        "console",
        "zoom",
        "scale",
        "steam deck",
    ]),
    app(
        "iroriUi.libraryLayout",
        "settings.field.ui_library_layout.label",
        Appearance,
        "settings.group.library",
        Choice(LIBRARY_LAYOUT),
    )
    .keywords(&["grid", "list", "focus", "view"]),
    app(
        "iroriUi.showStoreBadges",
        "settings.field.ui_show_store_badges.label",
        Appearance,
        "settings.group.library",
        Toggle,
    )
    .keywords(&["badge", "store", "label"]),
    app(
        "iroriUi.reduceMotion",
        "settings.field.ui_reduce_motion.label",
        Appearance,
        "settings.group.motion",
        Toggle,
    )
    .help("settings.field.ui_reduce_motion.help")
    .keywords(&["animation", "accessibility", "motion"]),
    app(
        "disableAnimations",
        "settings.field.disable_animations.label",
        Appearance,
        "settings.group.motion",
        Toggle,
    )
    .keywords(&["animation", "accessibility"]),
    app(
        "iroriUi.fontFamily",
        "settings.field.ui_font_family.label",
        Appearance,
        "settings.group.font",
        Custom(CustomKind::FontFamily),
    )
    .help("settings.field.ui_font_family.help")
    .keywords(&["font", "typeface", "manrope", "text", "family"]),
    app(
        "iroriUi.libraryBackground",
        "settings.field.ui_library_background.label",
        Appearance,
        "settings.group.background",
        Choice(LIBRARY_BACKGROUND),
    )
    .help("settings.field.ui_library_background.help")
    .keywords(&[
        "background",
        "wallpaper",
        "backdrop",
        "art",
        "picture",
        "image",
    ]),
    app(
        "iroriUi.libraryBackgroundPicture",
        "settings.field.ui_library_background_picture.label",
        Appearance,
        "settings.group.background",
        Text(TextKind::File),
    )
    .help("settings.field.ui_library_background_picture.help")
    .keywords(&["background", "wallpaper", "picture", "image"]),
    app(
        "iroriUi.libraryBackgroundStrength",
        "settings.field.ui_library_background_strength.label",
        Appearance,
        "settings.group.background",
        Custom(CustomKind::Percent),
    )
    .help("settings.field.ui_library_background_strength.help")
    .keywords(&["background", "strength", "opacity", "intensity"]),
    app(
        "iroriUi.libraryBackgroundDim",
        "settings.field.ui_library_background_dim.label",
        Appearance,
        "settings.group.background",
        Custom(CustomKind::Percent),
    )
    .help("settings.field.ui_library_background_dim.help")
    .keywords(&["background", "dim", "darken", "fade"]),
    // ---- Advanced ----
    app(
        "steamGridDbApiKey",
        "settings.field.steam_grid_db_api_key.label",
        Advanced,
        "settings.group.artwork",
        Text(TextKind::Secret),
    )
    .help("settings.field.steam_grid_db_api_key.help")
    .placeholder(Key("settings.field.steam_grid_db_api_key.placeholder"))
    .keywords(&["steamgriddb", "artwork", "cover", "key"]),
    app(
        "panel.eos-overlay",
        "settings.field.panel_eos_overlay.label",
        Advanced,
        "settings.group.epic_games",
        Panel(PanelKind::EosOverlay),
    )
    .tag(Verbatim("Epic"))
    .help("settings.field.panel_eos_overlay.help")
    .keywords(&[
        "eos",
        "overlay",
        "epic online services",
        "epic",
        "legendary",
    ]),
    app(
        "panel.egl-sync",
        "settings.field.panel_egl_sync.label",
        Advanced,
        "settings.group.epic_games",
        Panel(PanelKind::EglSync),
    )
    .tag(Verbatim("Epic"))
    .help("settings.field.panel_egl_sync.help")
    .keywords(&[
        "egl",
        "egs",
        "epic games launcher",
        "sync",
        "epic",
        "legendary",
        "link",
    ]),
    app(
        "showValveProton",
        "settings.field.show_valve_proton.label",
        Advanced,
        "settings.group.wine",
        Toggle,
    )
    .help("settings.field.show_valve_proton.help")
    .keywords(&["proton", "valve", "steam"]),
    app(
        "iroriCompatToolsPath",
        "settings.field.compat_tools_path.label",
        Advanced,
        "settings.group.wine",
        Text(TextKind::Directory),
    )
    .help("settings.field.compat_tools_path.help")
    .keywords(&["proton", "wine", "download", "folder", "path", "ge-proton"]),
    app(
        "downloadProtonToSteam",
        "settings.field.download_proton_to_steam.label",
        Advanced,
        "settings.group.wine",
        Toggle,
    )
    .keywords(&["proton", "steam", "compatibilitytools"]),
    app(
        "allowInstallationBrokenAnticheat",
        "settings.field.allow_installation_broken_anticheat.label",
        Advanced,
        "settings.group.installs",
        Toggle,
    )
    .help("settings.field.allow_installation_broken_anticheat.help")
    .keywords(&["anticheat", "eac", "battleye", "denied"]),
    app(
        "disableLogs",
        "settings.field.disable_logs.label",
        Advanced,
        "settings.group.diagnostics",
        Toggle,
    )
    .help("settings.field.disable_logs.help")
    .keywords(&["log", "debug"]),
    app(
        "action.open-logs",
        "settings.field.action_open_logs.label",
        Advanced,
        "settings.group.diagnostics",
        Action(ActionKind::OpenLogsFolder),
    )
    .keywords(&["log", "folder", "open"]),
    app(
        "action.import-heroic",
        "settings.field.action_import_heroic.label",
        Advanced,
        "settings.group.maintenance",
        Action(ActionKind::ImportHeroic),
    )
    .help("settings.field.action_import_heroic.help")
    .keywords(&["heroic", "import", "migrate"]),
    app(
        "action.import-lutris",
        "settings.field.action_import_lutris.label",
        Advanced,
        "settings.group.maintenance",
        Action(ActionKind::ImportLutris),
    )
    .help("settings.field.action_import_lutris.help")
    .keywords(&["lutris", "import", "migrate", "emulator"]),
    app(
        "action.import-bottles",
        "settings.field.action_import_bottles.label",
        Advanced,
        "settings.group.maintenance",
        Action(ActionKind::ImportBottles),
    )
    .help("settings.field.action_import_bottles.help")
    .keywords(&["bottles", "import", "migrate", "wine", "prefix"]),
    app(
        "action.import-faugus",
        "settings.field.action_import_faugus.label",
        Advanced,
        "settings.group.maintenance",
        Action(ActionKind::ImportFaugus),
    )
    .help("settings.field.action_import_faugus.help")
    .keywords(&["faugus", "import", "migrate", "proton", "umu"]),
    app(
        "action.import-playnite",
        "settings.field.action_import_playnite.label",
        Advanced,
        "settings.group.maintenance",
        Action(ActionKind::ImportPlaynite),
    )
    .help("settings.field.action_import_playnite.help")
    .keywords(&["playnite", "import", "migrate", "playtime", "windows"]),
    app(
        "action.clear-caches",
        "settings.field.action_clear_caches.label",
        Advanced,
        "settings.group.maintenance",
        Action(ActionKind::ClearCaches),
    )
    .help("settings.field.action_clear_caches.help")
    .keywords(&["cache", "clear", "artwork", "refresh"]),
    app(
        "action.reset-settings",
        "settings.field.action_reset_settings.label",
        Advanced,
        "settings.group.maintenance",
        Action(ActionKind::ResetSettings),
    )
    .help("settings.field.action_reset_settings.help")
    .keywords(&["reset", "defaults", "factory"]),
    // ---- Wine & Proton ----
    game(
        "wineVersion",
        "settings.field.wine_version.label",
        Wine,
        "settings.group.compatibility_layer",
        Custom(CustomKind::WineVersion),
    )
    .keywords(&["wine", "proton", "ge", "version", "runner"]),
    game(
        "winePrefix",
        "settings.field.wine_prefix.label",
        Wine,
        "settings.group.compatibility_layer",
        Text(TextKind::Directory),
    )
    .help("settings.field.wine_prefix.help")
    .placeholder(Key("settings.field.wine_prefix.placeholder"))
    .keywords(&["prefix", "wineprefix", "path", "bottle"]),
    game(
        "wineCrossoverBottle",
        "settings.field.wine_crossover_bottle.label",
        Wine,
        "settings.group.compatibility_layer",
        Text(TextKind::Plain),
    )
    .tag(Verbatim("CrossOver"))
    .help("settings.field.wine_crossover_bottle.help")
    .keywords(&["crossover", "bottle", "prefix", "mac"]),
    game(
        "disableUMU",
        "settings.field.disable_umu.label",
        Wine,
        "settings.group.compatibility_layer",
        Toggle,
    )
    .tag(Verbatim("Proton"))
    .help("settings.field.disable_umu.help")
    .keywords(&["umu", "proton"]),
    game(
        "enableEsync",
        "settings.field.enable_esync.label",
        Wine,
        "settings.group.wine_sync",
        Toggle,
    )
    .help("settings.field.enable_esync.help")
    .keywords(&["esync", "sync", "performance"]),
    game(
        "enableFsync",
        "settings.field.enable_fsync.label",
        Wine,
        "settings.group.wine_sync",
        Toggle,
    )
    .help("settings.field.enable_fsync.help")
    .keywords(&["fsync", "futex", "sync", "performance"]),
    game(
        "enableMsync",
        "settings.field.enable_msync.label",
        Wine,
        "settings.group.wine_sync",
        Toggle,
    )
    .tag(Verbatim("macOS"))
    .keywords(&["msync", "sync", "mac"]),
    game(
        "iroriRetinaMode",
        "settings.field.retina_mode.label",
        Wine,
        "settings.group.display",
        Toggle,
    )
    .tag(Verbatim("macOS"))
    .help("settings.field.retina_mode.help")
    .keywords(&["retina", "hidpi", "resolution", "scaling", "mac"]),
    game(
        "enableWineWayland",
        "settings.field.enable_wine_wayland.label",
        Wine,
        "settings.group.display",
        Toggle,
    )
    .tag(Key("settings.tag.experimental"))
    .help("settings.field.enable_wine_wayland.help")
    .keywords(&["wayland", "xwayland", "display"]),
    game(
        "enableHDR",
        "settings.field.enable_hdr.label",
        Wine,
        "settings.group.display",
        Toggle,
    )
    .keywords(&["hdr", "high dynamic range", "wayland"])
    .requires(NEEDS_WAYLAND),
    game(
        "enableFSR",
        "settings.field.enable_fsr.label",
        Wine,
        "settings.group.display",
        Toggle,
    )
    .keywords(&["fsr", "upscale", "amd", "fidelityfx"]),
    game(
        "maxSharpness",
        "settings.field.max_sharpness.label",
        Wine,
        "settings.group.display",
        Choice(SHARPNESS),
    )
    .keywords(&["fsr", "sharpness", "strength"])
    .requires(NEEDS_FSR),
    game(
        "enableWoW64",
        "settings.field.enable_wow64.label",
        Wine,
        "settings.group.display",
        Toggle,
    )
    .tag(Key("settings.tag.experimental"))
    .help("settings.field.enable_wow64.help")
    .keywords(&["wow64", "32-bit", "multilib"]),
    game(
        "autoInstallDxvk",
        "settings.field.auto_install_dxvk.label",
        Wine,
        "settings.group.graphics_translation",
        Toggle,
    )
    .help("settings.field.auto_install_dxvk.help")
    .keywords(&["dxvk", "directx", "d3d11", "vulkan"]),
    game(
        "autoInstallVkd3d",
        "settings.field.auto_install_vkd3d.label",
        Wine,
        "settings.group.graphics_translation",
        Toggle,
    )
    .help("settings.field.auto_install_vkd3d.help")
    .keywords(&["vkd3d", "directx 12", "d3d12", "vulkan"])
    .requires(NEEDS_DXVK),
    game(
        "autoInstallDxvkNvapi",
        "settings.field.auto_install_dxvk_nvapi.label",
        Wine,
        "settings.group.graphics_translation",
        Toggle,
    )
    .help("settings.field.auto_install_dxvk_nvapi.help")
    .keywords(&["nvapi", "dlss", "nvidia"])
    .requires(NEEDS_DXVK),
    game(
        "iroriDxvkVersion",
        "settings.field.dxvk_version.label",
        Wine,
        "settings.group.graphics_translation",
        Custom(CustomKind::ToolVersion(DxTool::Dxvk)),
    )
    .help("settings.field.tool_version.help")
    .keywords(&["dxvk", "version", "release"])
    .requires(NEEDS_DXVK),
    game(
        "iroriVkd3dVersion",
        "settings.field.vkd3d_version.label",
        Wine,
        "settings.group.graphics_translation",
        Custom(CustomKind::ToolVersion(DxTool::Vkd3d)),
    )
    .help("settings.field.tool_version.help")
    .keywords(&["vkd3d", "version", "release"])
    .requires(NEEDS_VKD3D),
    game(
        "iroriDescriptorHeap",
        "settings.field.descriptor_heap.label",
        Wine,
        "settings.group.graphics_translation",
        Toggle,
    )
    .tag(Key("settings.tag.experimental"))
    .help("settings.field.descriptor_heap.help")
    .keywords(&[
        "descriptor heap",
        "vkd3d_config",
        "proton_vkd3d_heap",
        "d3d12",
    ]),
    game(
        "preferSystemLibs",
        "settings.field.prefer_system_libs.label",
        Wine,
        "settings.group.graphics_translation",
        Toggle,
    )
    .help("settings.field.prefer_system_libs.help")
    .keywords(&["libraries", "ld_library_path", "system"]),
    game(
        "eacRuntime",
        "settings.field.eac_runtime.label",
        Wine,
        "settings.group.anti_cheat",
        Toggle,
    )
    .keywords(&["eac", "easyanticheat", "anticheat", "runtime"]),
    game(
        "battlEyeRuntime",
        "settings.field.battleye_runtime.label",
        Wine,
        "settings.group.anti_cheat",
        Toggle,
    )
    .keywords(&["battleye", "anticheat", "runtime"]),
    // ---- Performance ----
    game(
        "useGameMode",
        "settings.field.use_game_mode.label",
        Performance,
        "settings.group.system",
        Toggle,
    )
    .help("settings.field.use_game_mode.help")
    .keywords(&["gamemode", "feral", "performance", "cpu governor"]),
    game(
        "showMangohud",
        "settings.field.show_mangohud.label",
        Performance,
        "settings.group.system",
        Toggle,
    )
    .help("settings.field.show_mangohud.help")
    .keywords(&["mangohud", "overlay", "fps", "hud"]),
    game(
        "showFps",
        "settings.field.show_fps.label",
        Performance,
        "settings.group.system",
        Toggle,
    )
    .keywords(&["fps", "hud", "dxvk", "counter"]),
    game(
        "panel.replay",
        "settings.field.panel_replay.label",
        Performance,
        "settings.group.replay_buffer",
        Panel(PanelKind::Replay),
    )
    .keywords(&[
        "replay",
        "clip",
        "record",
        "video",
        "capture",
        "gpu-screen-recorder",
        "shadowplay",
    ]),
    game(
        "iroriReplay.enabled",
        "settings.field.replay_enabled.label",
        Performance,
        "settings.group.replay_buffer",
        Toggle,
    )
    .help("settings.field.replay_enabled.help")
    .keywords(&["replay", "clip", "record", "video", "instant replay"]),
    game(
        "iroriReplay.seconds",
        "settings.field.replay_seconds.label",
        Performance,
        "settings.group.replay_buffer",
        Choice(REPLAY_SECONDS),
    )
    .keywords(&["replay", "clip", "length", "duration", "seconds"])
    .requires(NEEDS_REPLAY),
    game(
        "iroriReplay.quality",
        "settings.field.replay_quality.label",
        Performance,
        "settings.group.replay_buffer",
        Choice(REPLAY_QUALITY),
    )
    .help("settings.field.replay_quality.help")
    .keywords(&["replay", "clip", "quality", "bitrate"])
    .requires(NEEDS_REPLAY),
    game(
        "iroriReplay.audio",
        "settings.field.replay_audio.label",
        Performance,
        "settings.group.replay_buffer",
        Choice(REPLAY_AUDIO),
    )
    .keywords(&["replay", "clip", "audio", "sound", "microphone"])
    .requires(NEEDS_REPLAY),
    game(
        "iroriReplay.capture",
        "settings.field.replay_capture.label",
        Performance,
        "settings.group.replay_buffer",
        Choice(REPLAY_CAPTURE),
    )
    .help("settings.field.replay_capture.help")
    .keywords(&["replay", "clip", "monitor", "screen", "portal", "wayland"])
    .requires(NEEDS_REPLAY),
    game(
        "iroriReplay.folder",
        "settings.field.replay_folder.label",
        Performance,
        "settings.group.replay_buffer",
        Text(TextKind::Directory),
    )
    .placeholder(Key("settings.field.replay_folder.placeholder"))
    .keywords(&["replay", "clip", "folder", "videos", "output"])
    .requires(NEEDS_REPLAY),
    game(
        "enableDXVKFpsLimit",
        "settings.field.enable_dxvk_fps_limit.label",
        Performance,
        "settings.group.frame_rate",
        Toggle,
    )
    .help("settings.field.enable_dxvk_fps_limit.help")
    .keywords(&["fps", "limit", "cap", "dxvk", "frame rate"]),
    game(
        "DXVKFpsCap",
        "settings.field.dxvk_fps_cap.label",
        Performance,
        "settings.group.frame_rate",
        Text(TextKind::Digits { max_len: 4 }),
    )
    .placeholder(Verbatim("60"))
    .keywords(&["fps", "limit", "cap"])
    .requires(NEEDS_FPS_LIMIT),
    game(
        "gamescope.iroriEnabled",
        "settings.field.gamescope_enabled.label",
        Performance,
        "settings.group.gamescope",
        Toggle,
    )
    .help("settings.field.gamescope_enabled.help")
    .keywords(&["gamescope", "compositor", "presenter", "window"]),
    game(
        "gamescope.enableUpscaling",
        "settings.field.gamescope_enable_upscaling.label",
        Performance,
        "settings.group.gamescope",
        Toggle,
    )
    .help("settings.field.gamescope_enable_upscaling.help")
    .keywords(&["gamescope", "upscale", "resolution"])
    .requires(NEEDS_GAMESCOPE),
    game(
        "gamescope.upscaleMethod",
        "settings.field.gamescope_upscale_method.label",
        Performance,
        "settings.group.gamescope",
        Choice(UPSCALE_METHOD),
    )
    .keywords(&["gamescope", "fsr", "nis", "integer"])
    .requires(NEEDS_UPSCALING),
    game(
        "gamescope.windowType",
        "settings.field.gamescope_window_type.label",
        Performance,
        "settings.group.gamescope",
        Choice(WINDOW_TYPE),
    )
    .keywords(&["gamescope", "fullscreen", "borderless", "windowed"])
    .requires(NEEDS_GAMESCOPE),
    game(
        "gamescope.gameWidth",
        "settings.field.gamescope_game_width.label",
        Performance,
        "settings.group.gamescope",
        Text(TextKind::Digits { max_len: 5 }),
    )
    .placeholder(Verbatim("1280"))
    .keywords(&["gamescope", "resolution", "width"])
    .requires(NEEDS_GAMESCOPE),
    game(
        "gamescope.gameHeight",
        "settings.field.gamescope_game_height.label",
        Performance,
        "settings.group.gamescope",
        Text(TextKind::Digits { max_len: 5 }),
    )
    .placeholder(Verbatim("720"))
    .keywords(&["gamescope", "resolution", "height"])
    .requires(NEEDS_GAMESCOPE),
    game(
        "gamescope.upscaleWidth",
        "settings.field.gamescope_upscale_width.label",
        Performance,
        "settings.group.gamescope",
        Text(TextKind::Digits { max_len: 5 }),
    )
    .placeholder(Verbatim("2560"))
    .keywords(&["gamescope", "resolution", "width"])
    .requires(NEEDS_GAMESCOPE),
    game(
        "gamescope.upscaleHeight",
        "settings.field.gamescope_upscale_height.label",
        Performance,
        "settings.group.gamescope",
        Text(TextKind::Digits { max_len: 5 }),
    )
    .placeholder(Verbatim("1440"))
    .keywords(&["gamescope", "resolution", "height"])
    .requires(NEEDS_GAMESCOPE),
    game(
        "gamescope.enableLimiter",
        "settings.field.gamescope_enable_limiter.label",
        Performance,
        "settings.group.gamescope",
        Toggle,
    )
    .keywords(&["gamescope", "fps", "limit"])
    .requires(NEEDS_GAMESCOPE),
    game(
        "gamescope.fpsLimiter",
        "settings.field.gamescope_fps_limiter.label",
        Performance,
        "settings.group.gamescope",
        Text(TextKind::Digits { max_len: 3 }),
    )
    .placeholder(Verbatim("60"))
    .keywords(&["gamescope", "fps", "limit"])
    .requires(NEEDS_LIMITER),
    game(
        "gamescope.fpsLimiterNoFocus",
        "settings.field.gamescope_fps_limiter_no_focus.label",
        Performance,
        "settings.group.gamescope",
        Text(TextKind::Digits { max_len: 3 }),
    )
    .placeholder(Verbatim("30"))
    .keywords(&["gamescope", "fps", "limit", "focus", "background"])
    .requires(NEEDS_LIMITER),
    game(
        "gamescope.enableForceGrabCursor",
        "settings.field.gamescope_enable_force_grab_cursor.label",
        Performance,
        "settings.group.gamescope",
        Toggle,
    )
    .keywords(&["gamescope", "cursor", "mouse", "grab"])
    .requires(NEEDS_GAMESCOPE),
    game(
        "gamescope.additionalOptions",
        "settings.field.gamescope_additional_options.label",
        Performance,
        "settings.group.gamescope",
        Text(TextKind::Plain),
    )
    .placeholder(Verbatim("--hdr-enabled"))
    .keywords(&["gamescope", "options", "arguments"])
    .requires(NEEDS_GAMESCOPE),
    // ---- GPU ----
    game(
        "iroriGpu",
        "settings.field.gpu.label",
        Gpu,
        "settings.group.graphics_card",
        Custom(CustomKind::GpuCard),
    )
    .help("settings.field.gpu.help")
    .keywords(&[
        "gpu",
        "graphics card",
        "dri_prime",
        "mesa_vk_device_select",
        "prime",
        "multi gpu",
        "integrated",
        "discrete",
    ])
    .needs(&[Need::Linux, Need::SeveralGpus]),
    game(
        "nvidiaPrime",
        "settings.field.nvidia_prime.label",
        Gpu,
        "settings.group.graphics_card",
        Toggle,
    )
    .help("settings.field.nvidia_prime.help")
    .keywords(&["prime", "optimus", "dgpu", "nvidia", "hybrid"])
    .needs(&[Need::NoChosenGpu]),
    game(
        "iroriSmoothMotion",
        "settings.field.smooth_motion.label",
        Gpu,
        "settings.group.frame_generation",
        Toggle,
    )
    .tag(Verbatim("NVIDIA"))
    .help("settings.field.smooth_motion.help")
    .keywords(&[
        "smooth motion",
        "frame generation",
        "nvpresent",
        "nvidia",
        "fps",
    ])
    .needs(&[Need::Linux, Need::SmoothMotion]),
    game(
        "iroriLsfg.enabled",
        "settings.field.lsfg_enabled.label",
        Gpu,
        "settings.group.frame_generation",
        Toggle,
    )
    .tag(Verbatim("lsfg-vk"))
    .help("settings.field.lsfg_enabled.help")
    .keywords(&[
        "lossless scaling",
        "lsfg",
        "lsfg-vk",
        "frame generation",
        "fps",
    ])
    .needs(&[Need::Linux, Need::Lsfg]),
    game(
        "iroriLsfg.multiplier",
        "settings.field.lsfg_multiplier.label",
        Gpu,
        "settings.group.frame_generation",
        Choice(LSFG_MULTIPLIER),
    )
    .help("settings.field.lsfg_multiplier.help")
    .keywords(&["lossless scaling", "lsfg", "multiplier", "frame generation"])
    .needs(&[Need::Linux, Need::Lsfg])
    .requires(NEEDS_LSFG),
    game(
        "iroriLsfg.flowScale",
        "settings.field.lsfg_flow_scale.label",
        Gpu,
        "settings.group.frame_generation",
        Choice(LSFG_FLOW_SCALE),
    )
    .help("settings.field.lsfg_flow_scale.help")
    .keywords(&["lossless scaling", "lsfg", "flow scale", "frame generation"])
    .needs(&[Need::Linux, Need::Lsfg])
    .requires(NEEDS_LSFG),
    game(
        "iroriLsfg.performanceMode",
        "settings.field.lsfg_performance_mode.label",
        Gpu,
        "settings.group.frame_generation",
        Toggle,
    )
    .help("settings.field.lsfg_performance_mode.help")
    .keywords(&[
        "lossless scaling",
        "lsfg",
        "performance",
        "frame generation",
    ])
    .needs(&[Need::Linux, Need::Lsfg])
    .requires(NEEDS_LSFG),
    game(
        "iroriNvidiaReflex",
        "settings.field.nvidia_reflex.label",
        Gpu,
        "settings.group.latency",
        Toggle,
    )
    .tag(Verbatim("NVIDIA"))
    .help("settings.field.nvidia_reflex.help")
    .keywords(&[
        "reflex",
        "latency",
        "dxvk-nvapi",
        "vkreflex",
        "nvidia",
        "input lag",
    ])
    .needs(&[Need::Linux, Need::NvidiaDriver, Need::WindowsGame])
    .requires(NEEDS_NVAPI),
    game(
        "iroriDlssUpgrade",
        "settings.field.dlss_upgrade.label",
        Gpu,
        "settings.group.upscalers",
        Toggle,
    )
    .tag(Verbatim("Proton"))
    .help("settings.field.proton_feature.help")
    .keywords(&["dlss", "proton_dlss_upgrade", "nvidia"]),
    game(
        "iroriDlssIndicator",
        "settings.field.dlss_indicator.label",
        Gpu,
        "settings.group.upscalers",
        Toggle,
    )
    .tag(Verbatim("Proton"))
    .help("settings.field.proton_feature.help")
    .keywords(&["dlss", "proton_dlss_indicator", "overlay"]),
    game(
        "iroriFsr4Upgrade",
        "settings.field.fsr4_upgrade.label",
        Gpu,
        "settings.group.upscalers",
        Toggle,
    )
    .tag(Verbatim("Proton"))
    .help("settings.field.proton_feature.help")
    .keywords(&["fsr", "fsr4", "proton_fsr4_upgrade", "amd"]),
    game(
        "iroriXessUpgrade",
        "settings.field.xess_upgrade.label",
        Gpu,
        "settings.group.upscalers",
        Toggle,
    )
    .tag(Verbatim("Proton"))
    .help("settings.field.proton_feature.help")
    .keywords(&["xess", "proton_xess_upgrade", "intel"]),
    game(
        "iroriShaderCacheSize",
        "settings.field.shader_cache_size.label",
        Gpu,
        "settings.group.shader_cache",
        Text(TextKind::Number { min: 0, max: 1024 }),
    )
    .help("settings.field.shader_cache_size.help")
    .keywords(&["shader", "cache", "stutter", "nvidia", "mesa", "gl"]),
    // ---- Launch ----
    game(
        "panel.launchPresets",
        "settings.field.panel_launch_presets.label",
        Launch,
        "settings.group.presets",
        Panel(PanelKind::LaunchPresets),
    )
    .help("settings.field.panel_launch_presets.help")
    .keywords(&[
        "preset",
        "profile",
        "template",
        "steam deck",
        "nvidia",
        "latency",
        "32-bit",
        "apply",
    ]),
    game(
        "launcherArgs",
        "settings.field.launcher_args.label",
        Launch,
        "settings.group.command",
        Text(TextKind::Plain),
    )
    .help("settings.field.launcher_args.help")
    .placeholder(Verbatim("-nolauncher"))
    .keywords(&["arguments", "args", "parameters", "command line"]),
    game_only(
        "targetExe",
        "settings.field.target_exe.label",
        Launch,
        "settings.group.command",
        Text(TextKind::File),
    )
    .help("settings.field.target_exe.help")
    .placeholder(Key("settings.field.target_exe.placeholder"))
    .keywords(&["exe", "executable", "override", "alternative"]),
    game(
        "language",
        "settings.field.language.label",
        Launch,
        "settings.group.command",
        Text(TextKind::Plain),
    )
    .help("settings.field.language.help")
    .placeholder(Key("settings.field.language.placeholder"))
    .keywords(&["language", "locale", "translation"]),
    game_only(
        "iroriInSunshine",
        "settings.field.in_sunshine.label",
        Launch,
        "settings.group.streaming",
        Toggle,
    )
    .help("settings.field.in_sunshine.help")
    .keywords(&[
        "sunshine",
        "apollo",
        "moonlight",
        "stream",
        "hide",
        "exclude",
    ]),
    game_only(
        "steamRuntime",
        "settings.field.steam_runtime.label",
        Launch,
        "settings.group.native_linux",
        Choice(STEAM_RUNTIME),
    )
    .help("settings.field.steam_runtime.help")
    .keywords(&[
        "runtime",
        "scout",
        "soldier",
        "sniper",
        "native",
        "container",
        "steam",
    ])
    .native_only(),
    game(
        "wrapperOptions",
        "settings.field.wrapper_options.label",
        Launch,
        "settings.group.wrappers_and_environment",
        Custom(CustomKind::Wrappers),
    )
    .help("settings.field.wrapper_options.help")
    .keywords(&["wrapper", "prefix command", "strace", "obs-gamecapture"]),
    game(
        "enviromentOptions",
        "settings.field.environment_options.label",
        Launch,
        "settings.group.wrappers_and_environment",
        Custom(CustomKind::EnvVars),
    )
    .keywords(&["environment", "env", "variables", "dll overrides"]),
    game(
        "beforeLaunchScriptPath",
        "settings.field.before_launch_script_path.label",
        Launch,
        "settings.group.scripts",
        Text(TextKind::File),
    )
    .keywords(&["script", "before", "pre-launch"]),
    game(
        "afterLaunchScriptPath",
        "settings.field.after_launch_script_path.label",
        Launch,
        "settings.group.scripts",
        Text(TextKind::File),
    )
    .keywords(&["script", "after", "post-exit"]),
    game(
        "panel.launchPrograms",
        "settings.field.panel_launch_programs.label",
        Launch,
        "settings.group.programs",
        Panel(PanelKind::LaunchPrograms),
    )
    .help("settings.field.panel_launch_programs.help")
    .keywords(&[
        "program",
        "alongside",
        "trainer",
        "overlay",
        "voice chat",
        "discord",
        "mod loader",
        "companion",
        "start with",
    ]),
    game_only(
        "offlineMode",
        "settings.field.offline_mode.label",
        Launch,
        "settings.group.online",
        Toggle,
    )
    .tag(Verbatim("Epic"))
    .help("settings.field.offline_mode.help")
    .keywords(&["offline", "network", "internet"]),
    game_only(
        "ignoreGameUpdates",
        "settings.field.ignore_game_updates.label",
        Launch,
        "settings.group.online",
        Toggle,
    )
    .help("settings.field.ignore_game_updates.help")
    .keywords(&["update", "ignore", "pin", "version"]),
    game_only(
        "iroriSandbox.noNetwork",
        "settings.field.sandbox_no_network.label",
        Launch,
        "settings.group.sandbox",
        Toggle,
    )
    .help("settings.field.sandbox_no_network.help")
    .keywords(&[
        "sandbox",
        "bubblewrap",
        "bwrap",
        "network",
        "internet",
        "untrusted",
        "isolate",
    ]),
    game_only(
        "iroriSandbox.noHome",
        "settings.field.sandbox_no_home.label",
        Launch,
        "settings.group.sandbox",
        Toggle,
    )
    .help("settings.field.sandbox_no_home.help")
    .keywords(&[
        "sandbox",
        "bubblewrap",
        "bwrap",
        "home",
        "files",
        "privacy",
        "untrusted",
        "isolate",
    ]),
    game(
        "verboseLogs",
        "settings.field.verbose_logs.label",
        Launch,
        "settings.group.logging",
        Toggle,
    )
    .help("settings.field.verbose_logs.help")
    .keywords(&["log", "verbose", "debug"]),
    // ---- Cloud saves ----
    game(
        "autoSyncSaves",
        "settings.field.auto_sync_saves.label",
        CloudSaves,
        "settings.group.save_sync",
        Toggle,
    )
    .help("settings.field.auto_sync_saves.help")
    .keywords(&["cloud", "save", "sync"]),
    game_only(
        "savesPath",
        "settings.field.saves_path.label",
        CloudSaves,
        "settings.group.locations",
        Text(TextKind::Directory),
    )
    .tag(Verbatim("Epic"))
    .placeholder(Key("settings.field.saves_path.placeholder"))
    .keywords(&["save", "cloud", "path"]),
    game_only(
        "gogSaves",
        "settings.field.gog_saves.label",
        CloudSaves,
        "settings.group.locations",
        Custom(CustomKind::GogSaves),
    )
    .tag(Verbatim("GOG"))
    .keywords(&["save", "cloud", "path", "gog"]),
    game_only(
        "panel.sync-saves",
        "settings.field.panel_sync_saves.label",
        CloudSaves,
        "settings.group.sync_now",
        Panel(PanelKind::SaveSync),
    )
    .help("settings.field.panel_sync_saves.help")
    .keywords(&["cloud", "save", "sync", "upload", "download", "force"]),
    game(
        "iroriLudusaviAutoBackup",
        "settings.field.ludusavi_auto_backup.label",
        CloudSaves,
        "settings.group.ludusavi",
        Toggle,
    )
    .tag(Verbatim("Ludusavi"))
    .help("settings.field.ludusavi_auto_backup.help")
    .keywords(&["ludusavi", "backup", "save", "session", "exit"]),
    game_only(
        "panel.save-backups",
        "settings.field.panel_save_backups.label",
        CloudSaves,
        "settings.group.ludusavi",
        Panel(PanelKind::SaveBackups),
    )
    .help("settings.field.panel_save_backups.help")
    .keywords(&["ludusavi", "backup", "restore", "save", "prefix"]),
    // ---- Artwork ----
    game_only(
        "panel.artwork",
        "settings.field.panel_artwork.label",
        Artwork,
        "settings.group.custom_art",
        Panel(PanelKind::Artwork),
    )
    .help("settings.field.panel_artwork.help")
    .keywords(&[
        "art",
        "artwork",
        "cover",
        "background",
        "hero",
        "logo",
        "image",
        "steamgriddb",
        "screenshot",
        "metadata",
        "refresh",
    ]),
    // ---- Saves ----
    app(
        "panel.ludusavi",
        "settings.field.panel_ludusavi.label",
        Saves,
        "settings.group.ludusavi",
        Panel(PanelKind::Ludusavi),
    )
    .help("settings.field.panel_ludusavi.help")
    .keywords(&["ludusavi", "backup", "save", "install"]),
    app(
        "iroriLudusaviPath",
        "settings.field.ludusavi_path.label",
        Saves,
        "settings.group.ludusavi",
        Text(TextKind::File),
    )
    .help("settings.field.ludusavi_path.help")
    .placeholder(Key("settings.field.ludusavi_path.placeholder"))
    .keywords(&["ludusavi", "program", "path", "flatpak"]),
    app(
        "iroriLudusaviBackupPath",
        "settings.field.ludusavi_backup_path.label",
        Saves,
        "settings.group.ludusavi",
        Text(TextKind::Directory),
    )
    .help("settings.field.ludusavi_backup_path.help")
    .placeholder(Key("settings.field.ludusavi_backup_path.placeholder"))
    .keywords(&["ludusavi", "backup", "folder", "path"]),
];

/// The field with this path.
pub fn find(path: &str) -> Option<&'static FieldSpec> {
    FIELDS.iter().find(|f| f.path == path)
}

/// Fields of one section, in display order.
pub fn section_fields(section: FieldSection) -> impl Iterator<Item = &'static FieldSpec> {
    FIELDS
        .iter()
        .filter(move |f| f.supported_on_host() && f.section == section)
}

/// Fields matching a search query, in display order. Empty for a blank query.
pub fn search(query: &str) -> impl Iterator<Item = &'static FieldSpec> + '_ {
    FIELDS
        .iter()
        .filter(move |f| f.supported_on_host() && f.matches(query))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::get_path;
    use irori_core::{AppSettings, GameSettings};
    use std::collections::HashSet;

    #[test]
    fn paths_are_unique() {
        let mut seen = HashSet::new();
        for f in FIELDS {
            assert!(seen.insert(f.path), "duplicate field {}", f.path);
        }
    }

    #[test]
    fn every_value_field_exists_in_the_schema() {
        let app = serde_json::to_value(AppSettings::default()).unwrap();
        let game = serde_json::to_value(GameSettings::default()).unwrap();
        for f in FIELDS {
            if matches!(f.kind, FieldKind::Action(_)) {
                assert!(f.path.starts_with("action."), "{}", f.path);
                continue;
            }
            if matches!(f.kind, FieldKind::Panel(_)) {
                assert!(f.path.starts_with("panel."), "{}", f.path);
                continue;
            }
            assert!(!f.path.starts_with("panel.") && !f.path.starts_with("action."));
            let root = if f.scope == Scope::App { &app } else { &game };
            assert!(
                get_path(root, f.path).is_some(),
                "{} is not a settings key",
                f.path
            );
            // Game fields must also resolve in the flattened app settings
            // (game defaults are edited there).
            if f.scope != Scope::App {
                assert!(get_path(&app, f.path).is_some(), "{}", f.path);
            }
        }
    }

    #[test]
    fn conditions_point_at_booleans() {
        let app = serde_json::to_value(AppSettings::default()).unwrap();
        for f in FIELDS {
            for c in f.requires {
                // Gamescope's switch stays unset until first used (`bool_at`).
                assert!(
                    c.path == "gamescope.iroriEnabled"
                        || matches!(get_path(&app, c.path), Some(Value::Bool(_))),
                    "{} requires non-boolean {}",
                    f.path,
                    c.path
                );
            }
        }
    }

    #[test]
    fn game_sections_hold_only_game_fields() {
        for f in FIELDS {
            assert_eq!(f.section.is_game(), f.scope != Scope::App, "{}", f.path);
        }
    }

    #[test]
    fn search_matches_labels_keywords_and_tags() {
        crate::i18n::register();
        let paths = |q: &str| search(q).map(|f| f.path).collect::<Vec<_>>();
        assert_eq!(
            paths("fsync"),
            if cfg!(target_os = "linux") {
                vec!["enableFsync"]
            } else {
                vec![]
            }
        );
        assert!(paths("TRAY").contains(&"exitToTray"));
        assert!(paths("epic").contains(&"downloadNoHttps"));
        assert!(paths("epic").contains(&"offlineMode"));
        // Every token must match.
        let fps = paths("gamescope fps");
        assert_eq!(
            fps.contains(&"gamescope.fpsLimiter"),
            cfg!(target_os = "linux")
        );
        assert!(!fps.contains(&"enableDXVKFpsLimit"));
        assert!(paths("   ").is_empty());
        assert!(paths("no such setting").is_empty());
    }

    #[test]
    fn search_finds_actions() {
        crate::i18n::register();
        assert!(search("heroic").any(|f| f.path == "action.import-heroic"));
        assert!(search("cache").any(|f| f.path == "action.clear-caches"));
    }

    #[test]
    fn scope_filters_per_game_items() {
        let target = find("targetExe").unwrap();
        assert!(target.applies_to_game(true));
        assert!(!target.applies_to_game(false));
        let esync = find("enableEsync").unwrap();
        assert!(esync.applies_to_game(false) && esync.applies_to_game(true));
        assert!(!find("exitToTray").unwrap().applies_to_game(true));
        assert_eq!(
            find("gamescope.fpsLimiter").unwrap().override_key(),
            "gamescope"
        );
    }

    #[test]
    fn mac_options_show_on_macos_only() {
        let mac = cfg!(target_os = "macos");
        for path in [
            "iroriRetinaMode",
            "wineCrossoverBottle",
            "iroriCrossoverBottlesPath",
        ] {
            assert_eq!(find(path).unwrap().supported_on_host(), mac, "{path}");
        }
        assert!(
            !find("iroriCrossoverBottlesPath")
                .unwrap()
                .applies_to_game(true)
        );
        assert!(find("iroriRetinaMode").unwrap().applies_to_game(true));
    }

    #[test]
    fn gpu_options_live_in_the_graphics_section() {
        for path in [
            "iroriGpu",
            "nvidiaPrime",
            "iroriSmoothMotion",
            "iroriLsfg.enabled",
            "iroriLsfg.multiplier",
            "iroriLsfg.flowScale",
            "iroriLsfg.performanceMode",
            "iroriNvidiaReflex",
            "iroriDlssUpgrade",
            "iroriFsr4Upgrade",
            "iroriXessUpgrade",
            "iroriShaderCacheSize",
        ] {
            let f = find(path).unwrap_or_else(|| panic!("{path}"));
            assert_eq!(f.section, FieldSection::Gpu, "{path}");
            assert_eq!(f.scope, Scope::Shared, "{path}");
        }
        assert_eq!(find("panel.artwork").unwrap().scope, Scope::GameOnly);
        assert_eq!(
            find("panel.artwork").unwrap().section,
            FieldSection::Artwork
        );
        let paths = |q: &str| search(q).map(|f| f.path).collect::<Vec<_>>();
        assert!(paths("frame generation").contains(&"iroriLsfg.enabled"));
        assert!(paths("frame generation").contains(&"iroriSmoothMotion"));
        assert!(paths("lossless").contains(&"iroriLsfg.multiplier"));
        assert!(paths("graphics card").contains(&"iroriGpu"));
        assert!(paths("steamgriddb").contains(&"panel.artwork"));
        assert!(paths("cover").contains(&"panel.artwork"));
    }

    #[test]
    fn needs_say_what_is_missing() {
        use irori_core::HostPlatform::{Linux, Mac, Windows};
        use irori_runtime::graphics::{GpuAdapter, HostGraphics, LsfgVk};
        let amd = GpuAdapter {
            slot: "0000:03:00.0".into(),
            vendor: 0x1002,
            device: 0x744c,
            driver: "amdgpu".into(),
            boot_vga: true,
            model: None,
        };
        let one = HostGraphics {
            adapters: vec![amd.clone()],
            ..Default::default()
        };
        let cx = |host, graphics: Option<&'static HostGraphics>| NeedContext {
            host,
            graphics,
            chosen_gpu: false,
            native_game: false,
        };
        let one: &'static HostGraphics = Box::leak(Box::new(one));
        assert_eq!(Need::Linux.blocker(&cx(Linux, Some(one))), None);
        assert_eq!(
            Need::Linux.blocker(&cx(Mac, None)),
            Some("settings.needs.linux_mac")
        );
        assert_eq!(
            Need::Linux.blocker(&cx(Windows, None)),
            Some("settings.needs.linux_windows")
        );
        assert_eq!(
            Need::SeveralGpus.blocker(&cx(Linux, None)),
            Some("settings.needs.probing")
        );
        assert_eq!(
            Need::SeveralGpus.blocker(&cx(Linux, Some(one))),
            Some("settings.needs.several_gpus")
        );
        assert_eq!(
            Need::SmoothMotion.blocker(&cx(Linux, Some(one))),
            Some("settings.needs.nvidia")
        );
        assert_eq!(
            Need::NvidiaDriver.blocker(&cx(Linux, Some(one))),
            Some("settings.needs.nvidia")
        );
        assert_eq!(
            Need::Lsfg.blocker(&cx(Linux, Some(one))),
            Some("settings.needs.lsfg")
        );

        let mut two = one.clone();
        two.adapters.push(GpuAdapter {
            slot: "0000:01:00.0".into(),
            vendor: 0x10de,
            device: 0x2b85,
            driver: "nvidia".into(),
            boot_vga: false,
            model: None,
        });
        two.lsfg = Some(LsfgVk {
            manifest: "/m.json".into(),
            current: false,
            dll: None,
        });
        let two: &'static HostGraphics = Box::leak(Box::new(two));
        assert_eq!(Need::SeveralGpus.blocker(&cx(Linux, Some(two))), None);
        assert_eq!(Need::NvidiaDriver.blocker(&cx(Linux, Some(two))), None);
        assert_eq!(
            Need::SmoothMotion.blocker(&cx(Linux, Some(two))),
            Some("settings.needs.nvidia_575")
        );
        assert_eq!(
            Need::Lsfg.blocker(&cx(Linux, Some(two))),
            Some("settings.needs.lsfg_update")
        );

        let chosen = NeedContext {
            chosen_gpu: true,
            native_game: true,
            ..cx(Linux, Some(two))
        };
        assert_eq!(
            Need::NoChosenGpu.blocker(&chosen),
            Some("settings.needs.no_chosen_gpu")
        );
        assert_eq!(
            Need::WindowsGame.blocker(&chosen),
            Some("settings.needs.windows_game")
        );
        assert_eq!(Need::NoChosenGpu.blocker(&cx(Linux, None)), None);
        let gone = NeedContext {
            chosen_gpu: true,
            ..cx(Linux, Some(one))
        };
        assert_eq!(Need::SeveralGpus.blocker(&gone), None);
    }

    #[test]
    fn steam_runtime_is_a_native_per_game_choice() {
        let natives: Vec<_> = FIELDS.iter().filter(|f| f.native_only).collect();
        assert_eq!(natives.len(), 1);
        let rt = natives[0];
        assert_eq!(rt.path, "steamRuntime");
        assert!(rt.applies_to_game(true));
        assert!(!rt.applies_to_game(false));
        assert_ne!(rt.section, FieldSection::Wine);
    }

    #[test]
    fn controller_and_epic_rows_are_findable() {
        crate::i18n::register();
        let paths = |q: &str| search(q).map(|f| f.path).collect::<Vec<_>>();
        let pads = paths("controller");
        for p in [
            "panel.controllers",
            "disableController",
            "gamepadInitialRepeatDelay",
            "gamepadRepeatDelay",
        ] {
            assert!(pads.contains(&p), "{p}");
            assert_eq!(find(p).unwrap().section, FieldSection::General);
        }
        assert!(paths("egl").contains(&"panel.egl-sync"));
        assert!(paths("eos overlay").contains(&"panel.eos-overlay"));
        assert!(paths("discord").contains(&"discordClientId"));
        assert!(paths("sync saves").contains(&"panel.sync-saves"));
        assert_eq!(find("panel.sync-saves").unwrap().scope, Scope::GameOnly);
    }

    #[test]
    fn language_choice_lists_every_interface_language_by_native_name() {
        let FieldKind::Choice(options) = find("uiLanguage").unwrap().kind else {
            panic!("uiLanguage is not a choice");
        };
        assert_eq!(options.len(), UI_LANGUAGES.len());
        for (o, lang) in options.iter().zip(UI_LANGUAGES) {
            assert_eq!(o.value, ChoiceValue::Str(lang.code));
            assert_eq!(o.label, Label::Verbatim(lang.native_name));
        }
        assert_eq!(find("uiLanguage").unwrap().section, FieldSection::General);
    }

    #[test]
    fn search_matches_english_text() {
        crate::i18n::register();
        let paths = |q: &str| search(q).map(|f| f.path).collect::<Vec<_>>();
        assert!(paths("default install folder").contains(&"defaultInstallPath"));
        assert!(paths("language").contains(&"uiLanguage"));
        assert!(paths("restart required").contains(&"noTrayIcon"));
    }

    #[test]
    fn choice_values_match_stored_json() {
        assert!(ChoiceValue::False.matches(&Value::Bool(false)));
        assert!(ChoiceValue::False.matches(&Value::Null));
        assert!(ChoiceValue::Str("fsr").matches(&Value::from("fsr")));
        assert!(ChoiceValue::Num(3).matches(&Value::from(3)));
        assert!(!ChoiceValue::Num(3).matches(&Value::from("3")));
    }
}
