//! An x86 game on an Arm64 Linux computer that has no x86 emulator set up
//! (`irori_core::X86EmulatorMissing`). Irori doesn't start it, as it would
//! only fail; the dialog says why and leads to FEX, which registers itself
//! so every x86 game and its launchers run from then on. Every place that
//! starts a game reaches this through
//! [`crate::save_conflict::resolve_launch_error`].

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::{App, ParentElement as _, Styled as _, Window, div};
use irori_core::{GameId, X86EmulatorMissing};

use crate::i18n::{t, t_args};
use crate::state::backend;
use crate::web_url;

/// FEX's own site, which explains installing it on each distribution.
const FEX_URL: &str = "https://fex-emu.com/";

/// Handle a missing x86 emulator in `error`. Returns whether it did.
pub(crate) fn resolve(
    id: &GameId,
    error: &anyhow::Error,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some(missing) = X86EmulatorMissing::of(error).cloned() else {
        return false;
    };
    let title = backend(cx)
        .game(id)
        .map(|g| g.title.clone())
        .unwrap_or_else(|| id.app_name.to_string());
    let program = missing.program.display().to_string();
    let width = window.rem_size() * 32.;
    window.open_dialog(cx, move |dialog, _, cx| {
        let muted = cx.theme().muted_foreground;
        dialog
            .title(t_args("ui.x86.title", &[("title", &title)]))
            .w(width)
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .child(t_args("ui.x86.body", &[("title", &title)])),
                    )
                    .child(div().text_sm().child(t("ui.x86.fix")))
                    .child(
                        div()
                            .min_w_0()
                            .text_xs()
                            .text_color(muted)
                            .child(t_args("ui.x86.program", &[("program", &program)])),
                    ),
            )
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("x86-problem-close")
                            .ghost()
                            .label(t("ui.x86.close"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("x86-problem-guide")
                            .primary()
                            .label(t("ui.x86.guide"))
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                open_guide(window, cx);
                            }),
                    ),
            )
    });
    true
}

fn open_guide(window: &mut Window, cx: &mut App) {
    if let Some(url) = web_url(FEX_URL)
        && let Err(e) = open::that_detached(&url)
    {
        window.push_notification(
            Notification::error(format!("{url}: {e}")).title(t("ui.backups.open_link_failed")),
            cx,
        );
    }
}
