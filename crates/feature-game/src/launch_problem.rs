//! Help after a game failed to start: likely causes read from the end of its
//! log, the community's known fix for it, and ways to try again, including
//! with another Wine or Proton build.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::link::Link;
use gpui_kit::component::select::{Select, SelectEvent};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, rems,
};
use irori_core::{GameId, HostPlatform, InstallPlatform, Runner, WineInstallation};
use irori_extras::known_fixes::{self, KnownFix};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::choice::{Choice, ChoiceSelect, choice_select, set_choices};
use crate::load::Load;
use crate::logs::{LineLevel, LogTail, open_log_sheet, read_tail};
use crate::ops;

/// Log lines shown in the sheet.
const SHOWN_LINES: usize = 30;

/// A likely cause of a failed start, recognised from the game's log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LaunchHint {
    MissingExecutable,
    WrongWineBuild,
    AntiCheat,
    MissingLibrary,
    MissingNativeLibrary,
    DotNet,
    Vulkan,
    BrokenPrefix,
    PermissionDenied,
    /// A program started alongside the game didn't start.
    ProgramFailed,
    /// The game crashed and said so (an unhandled exception, a segfault).
    Crash,
}

impl LaunchHint {
    fn text(self) -> SharedString {
        t(match self {
            LaunchHint::MissingExecutable => "game.launch_problem.hint.missing_executable",
            LaunchHint::WrongWineBuild => "game.launch_problem.hint.wrong_wine",
            LaunchHint::AntiCheat => "game.launch_problem.hint.anticheat",
            LaunchHint::MissingLibrary => "game.launch_problem.hint.missing_library",
            LaunchHint::MissingNativeLibrary => "game.launch_problem.hint.missing_native_library",
            LaunchHint::DotNet => "game.launch_problem.hint.dotnet",
            LaunchHint::Vulkan => "game.launch_problem.hint.vulkan",
            LaunchHint::BrokenPrefix => "game.launch_problem.hint.broken_prefix",
            LaunchHint::PermissionDenied => "game.launch_problem.hint.permission",
            LaunchHint::ProgramFailed => "game.launch_problem.hint.program_failed",
            LaunchHint::Crash => "game.launch_problem.hint.crash",
        })
    }

    fn matches(self, lower: &str) -> bool {
        let any = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));
        match self {
            LaunchHint::MissingExecutable => {
                any(&[
                    "wine: cannot find",
                    "could not find the program",
                    "shellexecuteex failed: file not found",
                    "application could not be started",
                    ": command not found",
                ]) || (lower.contains("no such file or directory")
                    && (lower.starts_with("sh:")
                        || lower.starts_with("bash:")
                        || lower.contains("exec")))
            }
            LaunchHint::WrongWineBuild => any(&[
                "cannot support 64-bit applications",
                "version mismatch",
                "wrong elf class",
            ]),
            LaunchHint::AntiCheat => any(&["easyanticheat", "battleye", "eac_launcher"]),
            LaunchHint::MissingLibrary => any(&["err:module:import_dll", "err:module:load_dll"]),
            LaunchHint::MissingNativeLibrary => any(&["error while loading shared libraries"]),
            LaunchHint::DotNet => any(&["err:mscoree", "mscoree.dll", ".net framework"]),
            LaunchHint::Vulkan => any(&[
                "vk_error_incompatible_driver",
                "vk_error_initialization_failed",
                "failed to load vulkan",
                "no vulkan",
                "dxvk: no adapters",
            ]),
            LaunchHint::BrokenPrefix => any(&["could not load kernel32.dll", "wine client error"]),
            LaunchHint::PermissionDenied => any(&["permission denied"]),
            // Irori's own line when a program alongside fails to start.
            LaunchHint::ProgramFailed => lower.starts_with("could not start "),
            LaunchHint::Crash => any(&[
                "unhandled exception",
                "unhandled page fault",
                "segmentation fault",
                "core dumped",
                "fatal error",
            ]),
        }
    }

    const ALL: [LaunchHint; 11] = [
        LaunchHint::MissingExecutable,
        LaunchHint::WrongWineBuild,
        LaunchHint::AntiCheat,
        LaunchHint::MissingLibrary,
        LaunchHint::MissingNativeLibrary,
        LaunchHint::DotNet,
        LaunchHint::Vulkan,
        LaunchHint::BrokenPrefix,
        LaunchHint::PermissionDenied,
        LaunchHint::ProgramFailed,
        LaunchHint::Crash,
    ];
}

/// A likely cause and the first log line that shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub hint: LaunchHint,
    /// The line, trimmed; `None` when the cause wasn't read from the log.
    pub line: Option<String>,
}

/// The likely causes in what the game wrote, most telling first, each with
/// the first line showing it. Exit code 127 is the shell's "command not
/// found" and 126 its "cannot execute".
pub fn diagnose<'a>(
    lines: impl IntoIterator<Item = &'a str>,
    exit_code: Option<i32>,
) -> Vec<Finding> {
    let mut found: Vec<Finding> = Vec::new();
    for line in lines {
        let lower = line.to_ascii_lowercase();
        for hint in LaunchHint::ALL {
            if !found.iter().any(|f| f.hint == hint) && hint.matches(&lower) {
                found.push(Finding {
                    hint,
                    line: Some(line.trim().to_string()),
                });
            }
        }
    }
    let by_code = match exit_code {
        Some(127) => Some(LaunchHint::MissingExecutable),
        Some(126) => Some(LaunchHint::PermissionDenied),
        _ => None,
    };
    if let Some(hint) = by_code
        && !found.iter().any(|f| f.hint == hint)
    {
        found.push(Finding { hint, line: None });
    }
    found.sort_by_key(|f| f.hint);
    found
}

/// The likely causes found in `lines`, most telling first.
pub fn launch_hints<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<LaunchHint> {
    diagnose(lines, None).into_iter().map(|f| f.hint).collect()
}

/// Windows libraries games often lack, with the winetricks verb that
/// installs them.
const RUNTIME_VERBS: &[(&str, &str)] = &[
    ("vcruntime140", "vcrun2022"),
    ("msvcp140", "vcrun2022"),
    ("concrt140", "vcrun2022"),
    ("msvcp120", "vcrun2013"),
    ("msvcr120", "vcrun2013"),
    ("msvcp110", "vcrun2012"),
    ("msvcr110", "vcrun2012"),
    ("msvcp100", "vcrun2010"),
    ("msvcr100", "vcrun2010"),
    ("d3dx9_", "d3dx9"),
    ("d3dx10_", "d3dx10"),
    ("d3dx11_", "d3dx11_43"),
    ("d3dcompiler_43", "d3dcompiler_43"),
    ("d3dcompiler_47", "d3dcompiler_47"),
    ("xinput1_3", "xinput"),
    ("xaudio2_7", "xact"),
    ("x3daudio1_7", "xact"),
    ("physxloader", "physx"),
    ("mscoree", "dotnet48"),
];

/// Winetricks verbs that install the Windows libraries the log says are
/// missing, in the order first named.
pub fn runtime_verbs<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<&'static str> {
    let mut verbs = Vec::new();
    for line in lines {
        let lower = line.to_ascii_lowercase();
        let missing = lower.contains("err:module:import_dll")
            || lower.contains("err:module:load_dll")
            || lower.contains("err:mscoree");
        if !missing {
            continue;
        }
        for (dll, verb) in RUNTIME_VERBS {
            if lower.contains(dll) && !verbs.contains(verb) {
                verbs.push(*verb);
            }
        }
    }
    verbs
}

/// A one-click fix the sheet offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    /// Install these winetricks verbs in the game's prefix, then try again.
    Winetricks(Vec<&'static str>),
    /// Turn on the Easy Anti-Cheat and BattlEye runtimes, then try again.
    AntiCheatRuntimes,
    /// Verify the game's files and download broken ones.
    VerifyFiles,
    /// Start the game's own executable instead of the alternative one set.
    ClearTargetExe,
    /// Run a native game in the Steam Linux Runtime, then try again.
    SteamRuntime,
    /// Open the settings with the programs started alongside.
    ProgramSettings,
}

/// What the fixes depend on besides the findings.
#[derive(Debug, Clone, Copy, Default)]
pub struct FixContext {
    pub needs_wine: bool,
    /// Irori installed the game and can verify its files.
    pub verifiable: bool,
    /// Both anti-cheat runtimes are already on.
    pub anticheat_runtimes_on: bool,
    /// An alternative executable is set and doesn't exist.
    pub target_exe_missing: bool,
    /// A native Linux build running without the Steam runtime.
    pub native_without_runtime: bool,
}

/// The fixes worth offering for `findings`, most likely first.
pub fn fixes(findings: &[Finding], verbs: &[&'static str], ctx: FixContext) -> Vec<Fix> {
    let has = |hint| findings.iter().any(|f| f.hint == hint);
    let mut out = Vec::new();
    if ctx.target_exe_missing {
        out.push(Fix::ClearTargetExe);
    }
    if has(LaunchHint::MissingExecutable) && ctx.verifiable {
        out.push(Fix::VerifyFiles);
    }
    if ctx.needs_wine && !verbs.is_empty() {
        out.push(Fix::Winetricks(verbs.to_vec()));
    }
    if ctx.needs_wine && has(LaunchHint::AntiCheat) && !ctx.anticheat_runtimes_on {
        out.push(Fix::AntiCheatRuntimes);
    }
    if has(LaunchHint::MissingNativeLibrary) && ctx.native_without_runtime {
        out.push(Fix::SteamRuntime);
    }
    if has(LaunchHint::ProgramFailed) {
        out.push(Fix::ProgramSettings);
    }
    out
}

/// Index of the first line the game itself wrote: Irori's log header (game,
/// paths, the settings as JSON) ends with the launch command. The header
/// would otherwise feed the hints, e.g. `"battlEyeRuntime":true`.
pub fn game_output_start<'a>(lines: impl IntoIterator<Item = &'a str>) -> usize {
    lines
        .into_iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with("Launch command:"))
        .last()
        .map_or(0, |(i, _)| i + 1)
}

/// Open the help for a game that just failed to start.
pub fn open_launch_problem_sheet(
    id: GameId,
    exit_code: Option<i32>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(game) = backend(cx).game(&id) else {
        return;
    };
    let title = t_args("game.launch_problem.title", &[("title", &game.title)]);
    let view = cx.new(|cx| LaunchProblem::new(id, exit_code, window, cx));
    window.open_sheet(cx, move |sheet, _, _| {
        sheet
            .title(title.clone())
            .size(rems(40.))
            .child(view.clone())
    });
}

/// Contents of the launch-problem sheet.
pub struct LaunchProblem {
    id: GameId,
    exit_code: Option<i32>,
    needs_wine: bool,
    tail: Load<Option<Arc<LogTail>>>,
    /// Where the game's own output starts in `tail`.
    output_start: usize,
    findings: Vec<Finding>,
    /// Winetricks verbs for the Windows libraries the log says are missing.
    verbs: Vec<&'static str>,
    fix_context: FixContext,
    /// The fix being applied.
    fixing: Option<Fix>,
    fix: Option<KnownFix>,
    wines: Load<Vec<WineInstallation>>,
    wine: Option<WineInstallation>,
    wine_select: Entity<ChoiceSelect>,
    _subscriptions: Vec<Subscription>,
}

impl LaunchProblem {
    fn new(
        id: GameId,
        exit_code: Option<i32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let b = backend(cx);
        let game = b.game(&id);
        let platform = game.as_ref().and_then(|g| g.installed_platform());
        let needs_wine = platform.is_some_and(|p| HostPlatform::current().needs_wine(p));
        let settings = b.settings.game(&id);
        let fix_context = FixContext {
            needs_wine,
            verifiable: id.runner.manages_installs() && id.runner != Runner::Sideload,
            anticheat_runtimes_on: settings.eac_runtime && settings.battl_eye_runtime,
            target_exe_missing: false,
            native_without_runtime: cfg!(target_os = "linux")
                && platform == Some(InstallPlatform::Linux)
                && settings.steam_runtime.is_none(),
        };
        let wine_select = choice_select(Vec::new(), None, window, cx);
        let subscription = cx.subscribe_in(
            &wine_select,
            window,
            |this, _, e: &SelectEvent<Vec<Choice>>, _, cx| {
                let SelectEvent::Confirm(v) = e;
                if let (Some(bin), Some(list)) = (v, this.wines.ready()) {
                    this.wine = list.iter().find(|w| w.bin == bin.as_ref()).cloned();
                }
                cx.notify();
            },
        );
        let mut this = Self {
            id,
            exit_code,
            needs_wine,
            tail: Load::Loading,
            output_start: 0,
            findings: Vec::new(),
            verbs: Vec::new(),
            fix_context,
            fixing: None,
            fix: None,
            wines: Load::Idle,
            wine: None,
            wine_select,
            _subscriptions: vec![subscription],
        };
        this.load(window, cx);
        this
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let path = b.game_log(&self.id);
        let tail = irori_ui::run(cx, read_tail(path));
        let (runner, app_name) = (self.id.runner, self.id.app_name.clone());
        let fix = irori_ui::run(cx, async move {
            Ok(known_fixes::lookup(runner, &app_name).await)
        });
        // Whether the program the launch starts is there at all.
        let game = b.game(&self.id);
        let target_exe = b.settings.game(&self.id).target_exe;
        let files = irori_ui::run(cx, async move {
            let exists = |p: std::path::PathBuf| async move {
                tokio::fs::try_exists(&p).await.unwrap_or(true)
            };
            let target_missing = !target_exe.trim().is_empty()
                && !exists(irori_core::paths::expand_home(target_exe.trim())).await;
            let executable = game.as_ref().and_then(|g| {
                let install = g.install.as_ref()?;
                let exe = install.executable.trim();
                if exe.is_empty() || install.install_path.is_empty() {
                    return None;
                }
                Some(std::path::Path::new(&install.install_path).join(exe))
            });
            let exe_missing = match executable {
                Some(exe) => !exists(exe).await,
                None => false,
            };
            Ok((target_missing, exe_missing))
        });
        let exit_code = self.exit_code;
        cx.spawn_in(window, async move |this, cx| {
            let tail = tail.await;
            let fix = fix.await.ok().flatten();
            let (target_missing, exe_missing) = files.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.tail = Load::from_result(tail.map(|t| t.map(Arc::new)));
                let mut findings = Vec::new();
                if let Some(Some(tail)) = this.tail.ready() {
                    let lines = || tail.lines.iter().map(|(_, l)| l.as_ref());
                    let start = game_output_start(lines());
                    this.output_start = start;
                    findings = diagnose(lines().skip(start), exit_code);
                    this.verbs = runtime_verbs(lines().skip(start));
                } else if let Some(hint) = diagnose([], exit_code).first() {
                    findings.push(hint.clone());
                }
                if (target_missing || exe_missing)
                    && !findings
                        .iter()
                        .any(|f| f.hint == LaunchHint::MissingExecutable)
                {
                    findings.insert(
                        0,
                        Finding {
                            hint: LaunchHint::MissingExecutable,
                            line: None,
                        },
                    );
                }
                this.fix_context.target_exe_missing = target_missing;
                this.findings = findings;
                this.fix = fix;
                cx.notify();
            });
        })
        .detach();

        if !self.needs_wine {
            return;
        }
        self.wines = Load::Loading;
        let app = b.settings.app();
        let current = b.settings.game(&self.id).wine_version;
        let task = irori_ui::run(cx, async move {
            Ok(irori_runtime::wine_installations(&app).await)
        });
        cx.spawn_in(window, async move |this, cx| {
            let list = task.await.unwrap_or_default();
            let _ = this.update_in(cx, |this, window, cx| {
                // The current build just failed; offer the others.
                let list: Vec<WineInstallation> =
                    list.into_iter().filter(|w| w.bin != current.bin).collect();
                this.wine = list.first().cloned();
                let items = list
                    .iter()
                    .map(|w| Choice::new(w.bin.clone(), w.name.clone()))
                    .collect();
                let selected = this.wine.as_ref().map(|w| w.bin.clone());
                set_choices(&this.wine_select, items, selected.as_deref(), window, cx);
                this.wines = Load::Ready(list);
                cx.notify();
            });
        })
        .detach();
    }

    fn try_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.close_sheet(cx);
        ops::play(&self.id, None, window, cx).detach();
    }

    fn try_with_wine(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(wine) = self.wine.clone() else {
            return;
        };
        if let Ok(value) = serde_json::to_value(&wine) {
            backend(cx)
                .settings
                .set_game_value(&self.id, "wineVersion", Some(value));
        }
        self.try_again(window, cx);
    }

    fn section(label: SharedString, cx: &App) -> gpui_kit::Div {
        v_flex().gap_1().child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
    }

    /// Apply a one-click fix, then start the game again where that's the
    /// point of the fix.
    fn apply_fix(&mut self, fix: Fix, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let id = self.id.clone();
        match &fix {
            Fix::ClearTargetExe => {
                b.settings.set_game_value(&id, "targetExe", None);
                self.try_again(window, cx);
            }
            Fix::AntiCheatRuntimes => {
                for key in ["eacRuntime", "battlEyeRuntime"] {
                    b.settings
                        .set_game_value(&id, key, Some(serde_json::Value::Bool(true)));
                }
                self.try_again(window, cx);
            }
            Fix::SteamRuntime => {
                b.settings.set_game_value(
                    &id,
                    "steamRuntime",
                    Some(serde_json::Value::String("umu-scout".into())),
                );
                self.try_again(window, cx);
            }
            Fix::ProgramSettings => {
                window.close_sheet(cx);
                ops::open_settings(&id, "launch", cx);
            }
            Fix::VerifyFiles => {
                window.close_sheet(cx);
                ops::verify(&id, window, cx).detach();
            }
            Fix::Winetricks(verbs) => {
                let verbs: Vec<String> = verbs.iter().map(|v| v.to_string()).collect();
                self.fixing = Some(fix.clone());
                cx.notify();
                let task = irori_ui::run(
                    cx,
                    async move { b.winetricks(&id, verbs).await.map(|_| ()) },
                );
                cx.spawn_in(window, async move |this, cx| {
                    let result = task.await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.fixing = None;
                        match result {
                            Ok(()) => this.try_again(window, cx),
                            Err(e) => {
                                ops::notify_error(
                                    window,
                                    cx,
                                    t("game.launch_problem.fix.failed"),
                                    &e,
                                );
                                cx.notify();
                            }
                        }
                    });
                })
                .detach();
            }
        }
    }

    fn fix_label(fix: &Fix) -> SharedString {
        match fix {
            Fix::Winetricks(verbs) => t_args(
                "game.launch_problem.fix.winetricks",
                &[("verbs", &verbs.join(", "))],
            ),
            Fix::AntiCheatRuntimes => t("game.launch_problem.fix.anticheat"),
            Fix::VerifyFiles => t("game.launch_problem.fix.verify"),
            Fix::ClearTargetExe => t("game.launch_problem.fix.target_exe"),
            Fix::SteamRuntime => t("game.launch_problem.fix.steam_runtime"),
            Fix::ProgramSettings => t("game.launch_problem.fix.programs"),
        }
    }

    fn render_fixes(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.tail.is_loading() {
            return None;
        }
        let fixes = fixes(&self.findings, &self.verbs, self.fix_context);
        if fixes.is_empty() {
            return None;
        }
        let busy = self.fixing.is_some();
        let buttons = fixes
            .into_iter()
            .enumerate()
            .map(|(ix, fix)| {
                let running = self.fixing.as_ref() == Some(&fix);
                let button = Button::new(("launch-problem-fix", ix))
                    .label(Self::fix_label(&fix))
                    .loading(running)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply_fix(fix.clone(), window, cx)
                    }));
                if ix == 0 {
                    button.primary()
                } else {
                    button.outline()
                }
            })
            .collect::<Vec<_>>();
        Some(
            Self::section(t("game.launch_problem.fixes"), cx)
                .child(h_flex().gap_2().flex_wrap().children(buttons))
                .into_any_element(),
        )
    }

    fn render_hints(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let body: Vec<AnyElement> = if self.tail.is_loading() {
            vec![div().child(t("game.logs.reading")).into_any_element()]
        } else if self.findings.is_empty() {
            vec![
                div()
                    .child(t("game.launch_problem.no_hints"))
                    .into_any_element(),
            ]
        } else {
            self.findings
                .iter()
                .map(|f| {
                    h_flex()
                        .gap_2()
                        .items_start()
                        .child(div().flex_shrink_0().child("•"))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(f.hint.text())
                                .when_some(f.line.clone(), |col, line| {
                                    col.child(
                                        div()
                                            .w_full()
                                            .min_w_0()
                                            .truncate()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .font_family(theme.mono_font_family.clone())
                                            .child(line),
                                    )
                                }),
                        )
                        .into_any_element()
                })
                .collect()
        };
        Self::section(t("game.launch_problem.causes"), cx)
            .text_sm()
            .children(body)
            .into_any_element()
    }

    fn render_fix(&self, cx: &App) -> Option<AnyElement> {
        let fix = self.fix.as_ref()?;
        let notes = fix.notes.values().map(|n| {
            div()
                .text_sm()
                .min_w_0()
                .child(SharedString::from(n.clone()))
        });
        Some(
            Self::section(t("game.launch_problem.known_fix"), cx)
                .child(
                    div()
                        .text_sm()
                        .child(t("game.launch_problem.known_fix_applied")),
                )
                .children(notes)
                .when_some(
                    fix.wiki_link.as_deref().and_then(irori_ui::web_url),
                    |col, link| {
                        col.child(
                            Link::new("launch-problem-wiki")
                                .href(link)
                                .child(t("game.launch_problem.open_wiki")),
                        )
                    },
                )
                .into_any_element(),
        )
    }

    fn render_log(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let body = match &self.tail {
            Load::Ready(Some(tail))
                if tail.lines[self.output_start..]
                    .iter()
                    .any(|(_, l)| !l.trim().is_empty()) =>
            {
                let start = tail
                    .lines
                    .len()
                    .saturating_sub(SHOWN_LINES)
                    .max(self.output_start);
                v_flex()
                    .children(tail.lines[start..].iter().map(|(level, text)| {
                        let color = match level {
                            LineLevel::Error => theme.danger,
                            LineLevel::Warning => theme.warning,
                            LineLevel::Info => theme.foreground,
                        };
                        div()
                            .w_full()
                            .min_w_0()
                            .truncate()
                            .text_color(color)
                            .child(text.clone())
                    }))
                    .into_any_element()
            }
            Load::Failed(e) => div()
                .text_color(theme.danger)
                .child(t_args("game.logs.read_failed", &[("error", e.as_ref())]))
                .into_any_element(),
            Load::Loading | Load::Idle => div().child(t("game.logs.reading")).into_any_element(),
            Load::Ready(_) => div()
                .text_color(theme.muted_foreground)
                .child(t("game.launch_problem.log_empty"))
                .into_any_element(),
        };
        Self::section(t("game.launch_problem.last_lines"), cx)
            .child(
                div()
                    .id("launch-problem-log")
                    .w_full()
                    .max_h(rems(16.))
                    .overflow_y_scroll()
                    .p_2()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.muted)
                    .text_xs()
                    .font_family(theme.mono_font_family.clone())
                    .child(body),
            )
            .into_any_element()
    }

    fn render_wine(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.needs_wine {
            return None;
        }
        let empty = self.wines.ready().is_some_and(|l| l.is_empty());
        let row = if empty {
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t("game.launch_problem.no_other_wine"))
                .into_any_element()
        } else {
            h_flex()
                .gap_2()
                .flex_wrap()
                .child(div().flex_1().min_w(rems(12.)).child(
                    Select::new(&self.wine_select).placeholder(t("game.install.wine_version")),
                ))
                .child(
                    Button::new("launch-problem-try-wine")
                        .outline()
                        .label(t("game.launch_problem.try_with_wine"))
                        .disabled(self.wine.is_none())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.try_with_wine(window, cx)),
                        ),
                )
                .into_any_element()
        };
        Some(
            Self::section(t("game.launch_problem.other_wine"), cx)
                .child(row)
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(t("game.launch_problem.other_wine_help")),
                )
                .into_any_element(),
        )
    }
}

impl Render for LaunchProblem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = match self.exit_code {
            Some(code) => t_args(
                "game.launch_problem.summary_code",
                &[("code", &code.to_string())],
            ),
            None => t("game.launch_problem.summary"),
        };
        let (log_id, settings_id) = (self.id.clone(), self.id.clone());
        v_flex()
            .id("launch-problem")
            .w_full()
            .gap_4()
            .py_2()
            .child(div().text_sm().child(summary))
            .child(self.render_hints(cx))
            .children(self.render_fixes(cx))
            .children(self.render_fix(cx))
            .children(self.render_wine(cx))
            .child(self.render_log(cx))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("launch-problem-retry")
                            .primary()
                            .icon(IconName::Play)
                            .label(t("game.launch_problem.try_again"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.try_again(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("launch-problem-log-open")
                            .outline()
                            .label(t("game.launch_problem.show_log"))
                            .on_click(move |_, window, cx| {
                                open_log_sheet(log_id.clone(), window, cx)
                            }),
                    )
                    .child(
                        Button::new("launch-problem-settings")
                            .ghost()
                            .icon(IconName::Settings)
                            .label(t("game.launch_problem.settings"))
                            .on_click(move |_, window, cx| {
                                window.close_sheet(cx);
                                ops::open_settings(&settings_id, "general", cx);
                            }),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_causes_in_logs() {
        let log = [
            "wine: Read access denied for device L\"\\\\??\\\\Z:\\\\\"",
            "0024:err:module:import_dll Library VCRUNTIME140.dll (which is needed by L\"game.exe\") not found",
            "EasyAntiCheat: launcher error",
            "err:module:import_dll Library MSVCP140.dll not found",
        ];
        assert_eq!(
            launch_hints(log),
            vec![LaunchHint::AntiCheat, LaunchHint::MissingLibrary]
        );
        assert_eq!(
            launch_hints(["err:vulkan:wine_vk_init Failed to load Vulkan"]),
            vec![LaunchHint::Vulkan]
        );
        assert_eq!(
            launch_hints(["sh: ./game.x86_64: Permission denied"]),
            vec![LaunchHint::PermissionDenied]
        );
        assert!(launch_hints(["Started", "fixme:d3d:stub"]).is_empty());
    }

    #[test]
    fn finds_missing_programs_wrong_builds_and_crashes_with_their_line() {
        let log = [
            "wine: '/home/me/.wine32' is a 32-bit installation, it cannot support 64-bit applications.",
            "wine: cannot find L\"C:\\Games\\game.exe\"",
            "Could not start Trainer: failed to start /opt/trainer",
            "wine: Unhandled page fault on read access to 00000000",
        ];
        let found = diagnose(log, Some(53));
        let hints: Vec<LaunchHint> = found.iter().map(|f| f.hint).collect();
        assert_eq!(
            hints,
            [
                LaunchHint::MissingExecutable,
                LaunchHint::WrongWineBuild,
                LaunchHint::ProgramFailed,
                LaunchHint::Crash
            ]
        );
        assert_eq!(found[0].line.as_deref(), Some(log[1]));
        assert!(found[3].line.as_deref().unwrap().contains("page fault"));

        // The shell's own exit codes say what the log may not.
        assert_eq!(
            diagnose(["Starting"], Some(127)),
            [Finding {
                hint: LaunchHint::MissingExecutable,
                line: None
            }]
        );
        assert_eq!(
            diagnose([], Some(126))[0].hint,
            LaunchHint::PermissionDenied
        );
        // Games log missing optional files; only the shell's line counts.
        assert!(diagnose(["open(config.ini): No such file or directory"], Some(1)).is_empty());
        assert_eq!(
            launch_hints(["sh: ./game.x86_64: No such file or directory"]),
            [LaunchHint::MissingExecutable]
        );
        assert_eq!(
            launch_hints(["./game: error while loading shared libraries: libSDL2-2.0.so.0"]),
            [LaunchHint::MissingNativeLibrary]
        );
    }

    #[test]
    fn maps_missing_windows_libraries_to_winetricks_verbs() {
        let log = [
            "0024:err:module:import_dll Library VCRUNTIME140.dll (which is needed by L\"game.exe\") not found",
            "0024:err:module:import_dll Library MSVCP140.dll not found",
            "0024:err:module:import_dll Library d3dx9_43.dll not found",
            "0024:err:mscoree:LoadLibraryShim error reading registry key for mscoree.dll",
            "fixme:d3d:stub vcruntime140 mentioned without an error",
        ];
        assert_eq!(runtime_verbs(log), ["vcrun2022", "d3dx9", "dotnet48"]);
        assert!(runtime_verbs(["Started", "msvcp140 loaded"]).is_empty());
    }

    #[test]
    fn offers_the_fixes_that_apply() {
        let finding = |hint| Finding { hint, line: None };
        let wine = FixContext {
            needs_wine: true,
            verifiable: true,
            ..Default::default()
        };
        let found = [
            finding(LaunchHint::MissingExecutable),
            finding(LaunchHint::AntiCheat),
            finding(LaunchHint::MissingLibrary),
        ];
        assert_eq!(
            fixes(&found, &["vcrun2022"], wine),
            [
                Fix::VerifyFiles,
                Fix::Winetricks(vec!["vcrun2022"]),
                Fix::AntiCheatRuntimes
            ]
        );
        // Runtimes already on, added apps can't be verified, a missing
        // alternative executable comes first.
        let ctx = FixContext {
            verifiable: false,
            anticheat_runtimes_on: true,
            target_exe_missing: true,
            ..wine
        };
        assert_eq!(fixes(&found, &[], ctx), [Fix::ClearTargetExe]);
        // Native games: the Steam runtime for missing libraries; never
        // winetricks.
        let native = FixContext {
            native_without_runtime: true,
            ..Default::default()
        };
        assert_eq!(
            fixes(
                &[
                    finding(LaunchHint::MissingNativeLibrary),
                    finding(LaunchHint::ProgramFailed)
                ],
                &["vcrun2022"],
                native
            ),
            [Fix::SteamRuntime, Fix::ProgramSettings]
        );
        assert!(fixes(&[finding(LaunchHint::Crash)], &[], wine).is_empty());
    }

    #[test]
    fn reads_only_what_the_game_wrote() {
        let log = [
            "Irori 0.1.0 - Game (sideload:game)",
            r#"Settings: {"battlEyeRuntime":true,"eacRuntime":true}"#,
            "",
            "Launch command: /games/start.sh",
            "",
            "Starting",
        ];
        let start = game_output_start(log);
        assert_eq!(start, 4);
        assert!(launch_hints(log[start..].iter().copied()).is_empty());
        assert_eq!(game_output_start(["no header", "at all"]), 0);
    }
}
