//! Help after a game failed to start, or on request ("Troubleshoot
//! launch"): the likely causes `irori_services::troubleshoot` works out from
//! the game's last run, one-click fixes, the community's known fix for it,
//! and ways to try again, including with another Wine or Proton build.

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
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, rems,
};
use irori_core::{GameId, HostPlatform, WineInstallation};
use irori_extras::anticheat::AntiCheatStatus;
use irori_extras::known_fixes::{self, KnownFix};
use irori_services::troubleshoot::{
    Detail, Diagnosis, Finding, Fix, LaunchHint, MAP_COUNT_WANTED, map_count_commands,
};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::choice::{Choice, ChoiceSelect, choice_select, set_choices};
use crate::load::Load;
use crate::logs::{LineLevel, LogTail, open_log_sheet, read_tail};
use crate::ops;

/// Log lines shown in the sheet.
const SHOWN_LINES: usize = 30;

fn hint_text(hint: LaunchHint) -> SharedString {
    t(match hint {
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
        LaunchHint::MapCount => "game.launch_problem.hint.map_count_low",
        LaunchHint::Crash => "game.launch_problem.hint.crash",
    })
}

/// What a finding says, with what the checks found filled in.
fn finding_text(f: &Finding) -> SharedString {
    match (&f.hint, &f.detail) {
        (LaunchHint::MissingLibrary, Some(Detail::Libraries { dlls, .. })) if !dlls.is_empty() => {
            t_args(
                "game.launch_problem.hint.missing_dlls",
                &[("dlls", &dlls.join(", "))],
            )
        }
        (LaunchHint::MissingLibrary, Some(Detail::ProgramLibraries { dlls })) => t_args(
            "game.launch_problem.hint.program_libraries",
            &[("dlls", &dlls.join(", "))],
        ),
        (LaunchHint::DotNet, Some(Detail::DotNetProgram)) => {
            t("game.launch_problem.hint.dotnet_program")
        }
        (LaunchHint::AntiCheat, Some(Detail::AntiCheat { names, status })) if !names.is_empty() => {
            let key = match status {
                AntiCheatStatus::Denied => "game.launch_problem.hint.anticheat_denied",
                AntiCheatStatus::Broken => "game.launch_problem.hint.anticheat_broken",
                _ => "game.launch_problem.hint.anticheat_runtime",
            };
            t_args(key, &[("anticheat", &names.join(", "))])
        }
        (LaunchHint::MapCount, Some(Detail::MapCount { current })) => t_args(
            "game.launch_problem.hint.map_count",
            &[
                ("current", &current.to_string()),
                ("wanted", &MAP_COUNT_WANTED.to_string()),
            ],
        ),
        _ => hint_text(f.hint),
    }
}

/// Open the help for a game that just failed to start.
pub fn open_launch_problem_sheet(
    id: GameId,
    exit_code: Option<i32>,
    window: &mut Window,
    cx: &mut App,
) {
    open_sheet(id, exit_code, false, window, cx);
}

/// Check a game's last run on request ("Troubleshoot launch"): the same
/// help, with the exit code of its last session this run.
pub fn open_troubleshooter(id: GameId, window: &mut Window, cx: &mut App) {
    open_sheet(id, None, true, window, cx);
}

fn open_sheet(
    id: GameId,
    exit_code: Option<i32>,
    on_request: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(game) = backend(cx).game(&id) else {
        return;
    };
    let key = if on_request {
        "game.launch_problem.troubleshoot_title"
    } else {
        "game.launch_problem.title"
    };
    let title = t_args(key, &[("title", &game.title)]);
    let view = cx.new(|cx| LaunchProblem::new(id, exit_code, on_request, window, cx));
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
    /// Opened from the menu rather than after a failed start.
    on_request: bool,
    needs_wine: bool,
    tail: Load<Option<Arc<LogTail>>>,
    /// Where the game's own output starts in `tail`.
    output_start: usize,
    diagnosis: Load<Diagnosis>,
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
        on_request: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let b = backend(cx);
        let platform = b.game(&id).and_then(|g| g.installed_platform());
        let needs_wine = platform.is_some_and(|p| HostPlatform::current().needs_wine(p));
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
        // A running fix reports its steps through the game's status.
        let status_model = models(cx).statuses.clone();
        let statuses = cx.observe(&status_model, |this, _, cx| {
            if this.fixing.is_some() {
                cx.notify();
            }
        });
        let mut this = Self {
            id,
            exit_code,
            on_request,
            needs_wine,
            tail: Load::Loading,
            output_start: 0,
            diagnosis: Load::Loading,
            fixing: None,
            fix: None,
            wines: Load::Idle,
            wine: None,
            wine_select,
            _subscriptions: vec![subscription, statuses],
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
        let (diagnose_b, id, exit_code) = (b.clone(), self.id.clone(), self.exit_code);
        let diagnosis = irori_ui::run(cx, async move {
            diagnose_b.troubleshoot_launch(&id, exit_code).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let tail = tail.await;
            let fix = fix.await.ok().flatten();
            let diagnosis = diagnosis.await;
            let _ = this.update(cx, |this, cx| {
                this.tail = Load::from_result(tail.map(|t| t.map(Arc::new)));
                if let Some(Some(tail)) = this.tail.ready() {
                    this.output_start = irori_services::troubleshoot::game_output_start(
                        tail.lines.iter().map(|(_, l)| l.as_ref()),
                    );
                }
                if let Ok(d) = &diagnosis {
                    this.exit_code = d.exit_code;
                }
                this.diagnosis = Load::from_result(diagnosis);
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
        b.mark_fix_tried(&id, &fix);
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
            Fix::MapCount => {
                cx.write_to_clipboard(ClipboardItem::new_string(map_count_commands().join("\n")));
                ops::notify_success(window, cx, t("game.launch_problem.map_count_copied"));
            }
            Fix::Winetricks(verbs) => {
                let verbs: Vec<String> = verbs.iter().map(|v| v.to_string()).collect();
                self.fixing = Some(fix.clone());
                cx.notify();
                let task =
                    irori_ui::run(cx, async move { b.install_prefix_verbs(&id, verbs).await });
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
            Fix::MapCount => t("game.launch_problem.fix.map_count"),
        }
    }

    fn render_fixes(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let diagnosis = self.diagnosis.ready()?;
        if diagnosis.fixes.is_empty() {
            return None;
        }
        let busy = self.fixing.is_some();
        let any_tried = diagnosis.fixes.iter().any(|o| o.tried);
        let buttons = diagnosis
            .fixes
            .iter()
            .enumerate()
            .map(|(ix, offered)| {
                let fix = offered.fix.clone();
                let running = self.fixing.as_ref() == Some(&fix);
                let button = Button::new(("launch-problem-fix", ix))
                    .label(Self::fix_label(&fix))
                    .loading(running)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply_fix(fix.clone(), window, cx)
                    }));
                if ix == 0 && !offered.tried {
                    button.primary()
                } else {
                    button.outline()
                }
            })
            .collect::<Vec<_>>();
        // What the running fix is doing, as winetricks says it.
        let step = self
            .fixing
            .as_ref()
            .and_then(|_| models(cx).statuses.read(cx).get(&self.id))
            .and_then(|s| s.context.clone());
        let muted = cx.theme().muted_foreground;
        Some(
            Self::section(t("game.launch_problem.fixes"), cx)
                .child(h_flex().gap_2().flex_wrap().children(buttons))
                .when_some(step, |col, step| {
                    col.child(
                        div()
                            .w_full()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(muted)
                            .child(step),
                    )
                })
                .when(any_tried, |col| {
                    col.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("game.launch_problem.fix.tried_note")),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_finding(f: &Finding, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let mono = |text: SharedString| {
            div()
                .w_full()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(theme.muted_foreground)
                .font_family(theme.mono_font_family.clone())
                .child(text)
        };
        let mut col = v_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(finding_text(f))
            .children(f.evidence.iter().map(|l| mono(l.clone().into())));
        match &f.detail {
            Some(Detail::Libraries { installed, .. }) if !installed.is_empty() => {
                col = col.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(t_args(
                            "game.launch_problem.already_installed",
                            &[("verbs", &installed.join(", "))],
                        )),
                );
            }
            Some(Detail::MapCount { .. }) => {
                col = col
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t("game.launch_problem.map_count_how")),
                    )
                    .children(map_count_commands().map(|c| mono(c.into())));
            }
            _ => {}
        }
        h_flex()
            .gap_2()
            .items_start()
            .child(div().flex_shrink_0().child("•"))
            .child(col)
            .into_any_element()
    }

    fn render_hints(&self, cx: &App) -> AnyElement {
        let causes = Self::section(t("game.launch_problem.causes"), cx).text_sm();
        let diagnosis = match &self.diagnosis {
            Load::Ready(d) => d,
            Load::Failed(e) => {
                return causes
                    .child(
                        div()
                            .text_color(cx.theme().danger)
                            .child(SharedString::from(e.to_string())),
                    )
                    .into_any_element();
            }
            Load::Loading | Load::Idle => {
                return causes
                    .child(div().child(t("game.logs.reading")))
                    .into_any_element();
            }
        };
        let (confirmed, possible): (Vec<&Finding>, Vec<&Finding>) =
            diagnosis.findings.iter().partition(|f| f.confirmed);
        let confirmed_body: Vec<AnyElement> = if confirmed.is_empty() {
            vec![
                div()
                    .child(t("game.launch_problem.no_hints"))
                    .into_any_element(),
            ]
        } else {
            confirmed
                .iter()
                .map(|f| Self::render_finding(f, cx))
                .collect()
        };
        let worth_checking = (!possible.is_empty()).then(|| {
            Self::section(t("game.launch_problem.worth_checking"), cx)
                .text_sm()
                .children(possible.iter().map(|f| Self::render_finding(f, cx)))
        });
        v_flex()
            .gap_4()
            .child(causes.children(confirmed_body))
            .children(worth_checking)
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

    fn summary(&self) -> SharedString {
        let code = self.exit_code.map(|c| c.to_string());
        match (self.on_request, code) {
            (false, Some(code)) => t_args("game.launch_problem.summary_code", &[("code", &code)]),
            (false, None) => t("game.launch_problem.summary"),
            (true, Some(code)) => t_args(
                "game.launch_problem.summary_last_run_code",
                &[("code", &code)],
            ),
            (true, None) => t("game.launch_problem.summary_last_run"),
        }
    }
}

impl Render for LaunchProblem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (log_id, settings_id) = (self.id.clone(), self.id.clone());
        v_flex()
            .id("launch-problem")
            .w_full()
            .gap_4()
            .py_2()
            .child(div().text_sm().child(self.summary()))
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
                            .disabled(self.fixing.is_some())
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
    fn findings_say_what_the_checks_found() {
        crate::i18n::register();
        let finding = |hint, detail| Finding {
            hint,
            evidence: Vec::new(),
            detail,
            confirmed: true,
            fixes: Vec::new(),
        };
        let dlls = finding(
            LaunchHint::MissingLibrary,
            Some(Detail::Libraries {
                dlls: vec!["msvcp140.dll".into()],
                installed: vec![],
            }),
        );
        assert!(finding_text(&dlls).contains("msvcp140.dll"));
        let maps = finding(
            LaunchHint::MapCount,
            Some(Detail::MapCount { current: 65_530 }),
        );
        assert!(finding_text(&maps).contains("65530"));
        let denied = finding(
            LaunchHint::AntiCheat,
            Some(Detail::AntiCheat {
                names: vec!["BattlEye".into()],
                status: AntiCheatStatus::Denied,
            }),
        );
        assert!(finding_text(&denied).contains("BattlEye"));
        // Without details, the cause's own text.
        let plain = finding(LaunchHint::MissingLibrary, None);
        assert_eq!(finding_text(&plain), hint_text(LaunchHint::MissingLibrary));
        // Every fix the diagnosis can offer has a label.
        for fix in [
            Fix::Winetricks(vec!["vcrun2022"]),
            Fix::AntiCheatRuntimes,
            Fix::VerifyFiles,
            Fix::ClearTargetExe,
            Fix::SteamRuntime,
            Fix::ProgramSettings,
            Fix::MapCount,
        ] {
            assert!(!LaunchProblem::fix_label(&fix).is_empty());
        }
    }
}
