//! A game set to run in a sandbox whose sandbox can't start: bubblewrap is
//! missing, Irori runs as a Flatpak, or the system refuses the namespaces.
//! Irori never plays it without the sandbox instead; the dialog says why,
//! how to fix it, and leads to the setting. Every place that starts a game
//! reaches this through [`crate::save_conflict::resolve_launch_error`].

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::{App, ParentElement as _, Styled as _, Window, div};
use irori_core::{GameId, SandboxProblem, SandboxUnavailable};

use crate::i18n::{t, t_args};
use crate::nav::{Route, navigate};
use crate::state::backend;

/// The commands that install bubblewrap on the common distributions, shown
/// as written.
const INSTALL_COMMANDS: [&str; 4] = [
    "sudo apt install bubblewrap",
    "sudo dnf install bubblewrap",
    "sudo pacman -S bubblewrap",
    "sudo zypper install bubblewrap",
];

/// Handle a sandbox that couldn't start in `error`. Returns whether it did.
pub(crate) fn resolve(
    id: &GameId,
    error: &anyhow::Error,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some(problem) = SandboxUnavailable::of(error).map(|e| e.problem.clone()) else {
        return false;
    };
    let title = backend(cx)
        .game(id)
        .map(|g| g.title.clone())
        .unwrap_or_else(|| id.app_name.to_string());
    let width = window.rem_size() * 32.;
    let id = id.clone();
    window.open_dialog(cx, move |dialog, _, cx| {
        let muted = cx.theme().muted_foreground;
        let (body, detail) = match &problem {
            SandboxProblem::Missing => (t_args("ui.sandbox.missing", &[("title", &title)]), None),
            SandboxProblem::Flatpak => (t_args("ui.sandbox.flatpak", &[("title", &title)]), None),
            SandboxProblem::Refused { detail } => (
                t_args("ui.sandbox.refused", &[("title", &title)]),
                Some(t_args("ui.sandbox.detail", &[("detail", detail)])),
            ),
            SandboxProblem::Unsupported => (t("ui.sandbox.unsupported"), None),
        };
        let commands = matches!(problem, SandboxProblem::Missing).then(|| {
            v_flex()
                .gap_1()
                .p_3()
                .rounded(cx.theme().radius)
                .bg(cx.theme().muted)
                .font_family(cx.theme().mono_font_family.clone())
                .text_xs()
                .children(INSTALL_COMMANDS.map(|c| div().child(c)))
        });
        let id = id.clone();
        dialog
            .title(t("ui.sandbox.title"))
            .w(width)
            .child(
                v_flex()
                    .gap_3()
                    .child(div().text_sm().child(body))
                    .children(commands)
                    .children(detail.map(|d| div().text_xs().text_color(muted).child(d)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("ui.sandbox.not_run")),
                    ),
            )
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("sandbox-problem-close")
                            .ghost()
                            .label(t("ui.sandbox.close"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("sandbox-problem-settings")
                            .primary()
                            .label(t("ui.sandbox.settings"))
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                navigate(Route::GameSettings(id.clone(), "launch".into()), cx);
                            }),
                    ),
            )
    });
    true
}
