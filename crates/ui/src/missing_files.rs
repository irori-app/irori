//! A game started while its files aren't where its install says (a drive
//! not connected, a folder moved, renamed or deleted). Instead of a bare
//! error the dialog offers the two ways out: point the install at the files'
//! new place and play, or forget the install, which deletes nothing. Every
//! place that starts a game reaches this through
//! [`crate::save_conflict::resolve_launch_error`].

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{App, ParentElement as _, PathPromptOptions, Styled as _, Window, div};
use irori_core::{GameFilesMissing, GameId, Runner};
use irori_services::{Backend, LaunchParams};

use crate::i18n::{t, t_args};
use crate::state::{backend, run};

/// Handle missing game files in `error`. Returns whether it did.
pub(crate) fn resolve(
    id: &GameId,
    params: &LaunchParams,
    error: &anyhow::Error,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some(missing) = GameFilesMissing::of(error).cloned() else {
        return false;
    };
    open(id.clone(), params.clone(), missing, window, cx);
    true
}

fn open(
    id: GameId,
    params: LaunchParams,
    missing: GameFilesMissing,
    window: &mut Window,
    cx: &mut App,
) {
    let width = window.rem_size() * 32.;
    let forget = Backend::can_forget_install(&id);
    window.open_dialog(cx, move |dialog, _, cx| {
        let muted = cx.theme().muted_foreground;
        let title = missing.title.clone();
        let body = match &missing.path {
            Some(path) => t_args(
                "ui.missing_files.body_at",
                &[("path", &path.display().to_string())],
            ),
            None => t("ui.missing_files.body"),
        };
        let (locate_id, locate_params) = (id.clone(), params.clone());
        let forget_id = id.clone();
        let forget_title = title.clone();
        dialog
            .title(t_args("ui.missing_files.title", &[("title", &title)]))
            .w(width)
            .child(v_flex().gap_3().child(div().text_sm().child(body)).child(
                div().text_xs().text_color(muted).child(if forget {
                    t("ui.missing_files.choices")
                } else {
                    t("ui.missing_files.choices_added")
                }),
            ))
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("missing-files-close")
                            .ghost()
                            .label(t("ui.missing_files.not_now"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .when(forget, |footer| {
                        footer.child(
                            Button::new("missing-files-forget")
                                .danger()
                                .label(t("ui.missing_files.forget"))
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    forget_install(forget_id.clone(), &forget_title, window, cx);
                                }),
                        )
                    })
                    .child(
                        Button::new("missing-files-locate")
                            .primary()
                            .label(t("ui.missing_files.locate"))
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                locate(locate_id.clone(), locate_params.clone(), window, cx);
                            }),
                    ),
            )
    });
}

/// Ask where the files are now, record it, and start the game from there.
/// An added game is pointed at its program (a file, or a folder holding
/// one); a store game at its folder.
fn locate(id: GameId, params: LaunchParams, window: &mut Window, cx: &mut App) {
    let program = id.runner == Runner::Sideload;
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: program,
        directories: true,
        multiple: false,
        prompt: Some(t("ui.missing_files.pick")),
    });
    let b = backend(cx);
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let Ok(task) = cx.update(|_, cx| {
                let (b, id) = (b.clone(), id.clone());
                run(cx, async move {
                    if program {
                        let platform = b
                            .game(&id)
                            .and_then(|g| g.installed_platform())
                            .unwrap_or_else(|| irori_core::HostPlatform::current().native());
                        b.import(&id, &path, platform).await
                    } else {
                        b.set_install_path(&id, &path).await
                    }
                })
            }) else {
                return;
            };
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(()) => {
                    let title = backend(cx)
                        .game(&id)
                        .map(|g| g.title.clone())
                        .unwrap_or_else(|| id.app_name.to_string());
                    crate::save_conflict::play_again(id.clone(), params.clone(), &title, window, cx)
                }
                Err(e) => window.push_notification(
                    Notification::error(format!("{e:#}"))
                        .title(t("ui.missing_files.locate_failed")),
                    cx,
                ),
            });
        })
        .detach();
}

fn forget_install(id: GameId, title: &str, window: &mut Window, cx: &mut App) {
    let b = backend(cx);
    let (done, failed) = (
        t_args("ui.missing_files.forgotten", &[("title", title)]),
        t_args("ui.missing_files.forget_failed", &[("title", title)]),
    );
    let task = run(cx, async move { b.forget_install(&id).await });
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(()) => window.push_notification(Notification::success(done), cx),
                Err(e) => window
                    .push_notification(Notification::error(format!("{e:#}")).title(failed), cx),
            });
        })
        .detach();
}
