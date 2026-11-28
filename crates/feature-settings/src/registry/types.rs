use gpui_kit::SharedString;
use irori_runtime::DxTool;
use irori_ui::i18n::t;
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
    /// What the controller's buttons do, the pointer, repeat timing, Big
    /// Picture's button and the in-game overlay.
    Controller,
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
    pub const APP: [FieldSection; 5] = [
        FieldSection::General,
        FieldSection::Controller,
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
            FieldSection::Controller => "settings.section.controller",
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

pub(super) const fn opt(value: ChoiceValue, label: Label) -> ChoiceOption {
    ChoiceOption { value, label }
}

/// Fields rendered by dedicated editors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomKind {
    /// Wine/Proton build picker fed by `irori_runtime::wine_installations`.
    WineVersion,
    /// `KEY=VALUE` environment variable rows.
    EnvVars,
    /// Wine DLL override rows: a DLL and its load mode.
    DllOverrides,
    /// Wrapper command rows.
    Wrappers,
    /// GOG cloud-save locations.
    GogSaves,
    /// Theme picker from the theme registry.
    ThemeName,
    /// Graphics card picker fed by `irori_runtime::graphics`.
    GpuCard,
    /// The graphics card Windows hides from a game, fed by
    /// `Backend::hideable_gpus`.
    HiddenGpu,
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
    /// Download speed cap in KB/s with quick speed preset chips.
    DownloadSpeed,
    /// Network connectivity mode as a 3-way segmented control with live mode hint.
    NetworkMode,
}

/// Rows rendered by a self-contained panel (status plus actions) instead of
/// a single bound value. Their paths start with `panel.`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanelKind {
    /// Connected controllers with a refresh button.
    Controllers,
    /// What the in-game overlay can do on this computer.
    Overlay,
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
    /// The user's own actions beside the game's Play button.
    UserActions,
    /// Launch presets: apply, save, apply to several games.
    LaunchPresets,
    /// Ludusavi: where it was found, or how to get it.
    Ludusavi,
    /// Ludusavi backups of one game.
    SaveBackups,
    /// The replay buffer's recorder, ways to save a clip and memory use.
    Replay,
    /// Per-game Performance HUD overrides (MangoHud on Linux, Metal HUD on macOS).
    PerformanceHud,
    /// OptiScaler upscaling bridge (global defaults in Game defaults, per-game panel in Performance).
    Optiscaler,
    /// Sunshine: found or not, its login, syncing now, the last sync.
    Sunshine,
    /// IGDB: the user's Twitch application, saving and forgetting it.
    Igdb,
    /// LaunchBox's stored game list: its size, and removing it while off.
    LaunchBox,
    /// A Steam game's launch options in Steam itself.
    SteamLaunchOptions,
    /// How a game's next launch starts: command, wrappers, variables.
    HowItStarts,
    /// One game's umu id: the one used, its source, a database search.
    Umu,
    /// What each controller button does, drawn on the controller, with the
    /// buttons' jobs changeable.
    ButtonMap,
    /// The quick launcher's shortcut: its keys, or the lines to bind.
    QuickLaunch,
    /// Named folders games install into, and moving their games.
    LibraryFolders,
    /// Toggle chips controlling which extra details appear on achievement unlock cards.
    AchievementDetails,
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

pub(super) const fn when(path: &'static str, equals: bool, reason: &'static str) -> Condition {
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
    /// A frame generation layer: lsfg-vk 2 with Lossless Scaling's DLL, or
    /// MAKO.
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
    /// One game's settings, installed as this system's own build.
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
            Need::Lsfg => match cx.graphics.map(|g| g.frame_generation_blocker()) {
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
    /// Translation key of the plain-language hover tooltip; empty for none.
    pub tooltip: &'static str,
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
    /// In a game's settings, only shown when the game is installed as this
    /// system's own build (Linux on Linux, Mac on macOS); a field shared
    /// with the game defaults shows there too.
    pub native_only: bool,
    /// Flip the switch's visual state relative to the stored boolean
    /// (for example a stored `disable*` or `no*` flag shown with a positive label).
    pub inverted: bool,
}

impl FieldSpec {
    pub(super) const fn new(
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
            tooltip: "",
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
            inverted: false,
        }
    }

    pub(super) const fn help(mut self, help: &'static str) -> Self {
        self.help = help;
        self
    }

    pub(super) const fn tooltip(mut self, tooltip: &'static str) -> Self {
        self.tooltip = tooltip;
        self
    }

    pub(super) const fn keywords(mut self, keywords: &'static [&'static str]) -> Self {
        self.keywords = keywords;
        self
    }

    pub(super) const fn tag(mut self, tag: Label) -> Self {
        self.tag = Some(tag);
        self
    }

    pub(super) const fn placeholder(mut self, placeholder: Label) -> Self {
        self.placeholder = Some(placeholder);
        self
    }

    pub(super) const fn requires(mut self, requires: &'static [Condition]) -> Self {
        self.requires = requires;
        self
    }

    pub(super) const fn needs(mut self, needs: &'static [Need]) -> Self {
        self.needs = needs;
        self
    }

    pub(super) const fn native_only(mut self) -> Self {
        self.native_only = true;
        self
    }

    pub(super) const fn inverted(mut self) -> Self {
        self.inverted = true;
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

    /// The hover tooltip in the interface language; empty when there is none.
    pub fn tooltip_text(&self) -> SharedString {
        if self.tooltip.is_empty() {
            SharedString::default()
        } else {
            t(self.tooltip)
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
        [
            self.label,
            self.help,
            self.tooltip,
            self.group,
            self.section.label_key(),
        ]
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
                | "iroriRosetta"
                | "iroriMetalFxDlss"
        );
        if mac_only && host != HostPlatform::Mac {
            return false;
        }
        // Windows' own graphics preference stands in for the card chooser
        // there, which only Linux can apply.
        match self.path {
            "iroriWindowsGpu" | "iroriHideGpu" | "iroriOverlay.backStart" => {
                return host == HostPlatform::Windows;
            }
            "iroriOverlay.screenshotKey" => return host != HostPlatform::Linux,
            "iroriGpu" if host == HostPlatform::Windows => return false,
            _ => {}
        }
        // Legacy showMangohud toggle is superseded by the native Performance HUD
        // panel; its FieldSpec entry stays registered so preset labels resolve.
        if self.path == "showMangohud" {
            return false;
        }
        // Per-game Performance HUD is available on Linux (MangoHud) and macOS
        // (Metal HUD), hidden on Windows.
        if self.path == "panel.performance-hud" {
            return irori_core::HudBackend::current().is_supported();
        }
        // DXVK_HUD=fps is Linux-only.
        if self.path == "showFps" {
            return host == HostPlatform::Linux;
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
                    | "iroriMangohudHidden"
                    | "iroriFpsLimiter"
                    | "nvidiaPrime"
                    | "enableFsync"
                    | "iroriNtsync"
                    | "enableFSR"
                    | "enableWineWayland"
                    | "enableHDR"
                    | "enableWoW64"
                    | "iroriProtonPreferSdl"
                    | "eacRuntime"
                    | "battlEyeRuntime"
                    | "disableUMU"
                    | "iroriUmuId"
                    | "panel.umu"
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
                    | "iroriProtonCpuLevel"
                    | "showValveProton"
                    | "iroriShaderCacheSize"
                    | "iroriSandbox.noNetwork"
                    | "iroriSandbox.noHome"
                    | "iroriSteamOverlay"
            )
        {
            return false;
        }
        host != HostPlatform::Windows
            || (self.section != FieldSection::Wine
                && self.path != "iroriCompatToolsPath"
                && self.path != "defaultWinePrefixDir"
                && !matches!(self.path, "enableDXVKFpsLimit" | "DXVKFpsCap"))
    }

    /// Whether the field still does something for a Steam game. Steam starts
    /// its games with its own Proton, variables and wrappers, so only what
    /// Irori does around the launch applies: the arguments it passes, the
    /// scripts and programs it runs, the replay buffer, Sunshine, save
    /// backups and art.
    pub fn applies_to_steam(&self) -> bool {
        self.path.starts_with("iroriReplay.")
            || matches!(
                self.path,
                "panel.replay"
                    | "panel.performance-hud"
                    | "panel.optiscaler"
                    | "launcherArgs"
                    | "iroriInSunshine"
                    | "iroriDiscordPresence"
                    | "beforeLaunchScriptPath"
                    | "iroriBeforeLaunchScriptAlongside"
                    | "afterLaunchScriptPath"
                    | "panel.launchPrograms"
                    | "panel.actions"
                    | "iroriLudusaviAutoBackup"
                    | "iroriLudusaviInterval"
                    | "panel.save-backups"
                    | "panel.artwork"
                    | "panel.steamLaunchOptions"
            )
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
            self.tooltip_text(),
            self.group_text(),
            self.section.label(),
            tag,
        ] {
            push(&part);
        }
        for key in [
            self.label,
            self.help,
            self.tooltip,
            self.group,
            self.section.label_key(),
        ] {
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

pub(super) const fn app(
    path: &'static str,
    label: &'static str,
    section: FieldSection,
    group: &'static str,
    kind: FieldKind,
) -> FieldSpec {
    FieldSpec::new(path, label, section, group, kind, Scope::App)
}

pub(super) const fn game(
    path: &'static str,
    label: &'static str,
    section: FieldSection,
    group: &'static str,
    kind: FieldKind,
) -> FieldSpec {
    FieldSpec::new(path, label, section, group, kind, Scope::Shared)
}

pub(super) const fn game_only(
    path: &'static str,
    label: &'static str,
    section: FieldSection,
    group: &'static str,
    kind: FieldKind,
) -> FieldSpec {
    FieldSpec::new(path, label, section, group, kind, Scope::GameOnly)
}
