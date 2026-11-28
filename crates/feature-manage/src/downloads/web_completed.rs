//! Finished direct-link and BitTorrent download rows rendered inside the
//! unified Completed / History section of [`DownloadsView`].

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, rems,
};
use irori_core::StatusKind;
use irori_services::{PostAction, WebDownload, WebDownloadId, WebDownloadKind, WebDownloadState};
use irori_ui::components::GameCover;
use irori_ui::i18n::{t, t_args};
use irori_ui::{Route, backend, models, navigate};

use super::DownloadsView;
use super::web_section::web_icon;
use crate::common::notify_error;
use crate::format;

fn web_kind_label(item: &WebDownload) -> SharedString {
    match (&item.post_action, item.kind) {
        (PostAction::InstallSetup { .. }, _) => t("manage.downloads.web.kind.installer"),
        (_, WebDownloadKind::DirectUrl) => t("manage.downloads.add.kind.link"),
        (_, WebDownloadKind::TorrentFile | WebDownloadKind::Magnet) => {
            t("manage.downloads.add.kind.torrent")
        }
    }
}

impl DownloadsView {
    /// One finished web download row rendered inside the unified Completed/History card.
    pub(super) fn render_completed_web_row(
        &self,
        item: &WebDownload,
        ix: usize,
        now: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;
        let muted_bg = theme.muted;
        let hover_bg = theme.muted.opacity(0.35);
        let radius_lg = theme.radius_lg;
        let danger = theme.danger;
        let id = item.id.clone();
        let key = id.as_str().to_string();
        let reveal_path = item.reveal_path();
        let library_game = match &item.post_action {
            PostAction::Play(game_id) => models(cx).library.read(cx).snapshot.get(game_id).cloned(),
            _ => None,
        };
        let playable_game = library_game.as_ref().is_some_and(|g| g.is_installed);
        let play_status = match &item.post_action {
            PostAction::Play(game_id) if playable_game => {
                models(cx).statuses.read(cx).get(game_id).map(|s| s.kind)
            }
            _ => None,
        };
        let when = item
            .finished_at
            .map(|at| format::relative_time(at, now).to_string())
            .unwrap_or_default();
        let size_bytes = if item.bytes_total > 0 {
            item.bytes_total
        } else {
            item.bytes_done
        };
        let meta = [
            Some(web_kind_label(item).to_string()),
            format::size(size_bytes),
            (!when.is_empty()).then_some(when),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");

        let status_badge = match item.state {
            WebDownloadState::Done => None,
            WebDownloadState::Failed => Some((
                IconName::CircleX,
                danger,
                danger.opacity(0.14),
                t("manage.downloads.history.failed"),
            )),
            _ => Some((
                IconName::Ban,
                muted,
                muted_bg,
                t("manage.downloads.history.canceled"),
            )),
        };

        let thumb: AnyElement = match (&item.post_action, library_game) {
            (PostAction::Play(game_id), Some(game)) if game.is_installed => {
                let open_id = game_id.clone();
                div()
                    .id(ElementId::Name(format!("web-cover-{key}").into()))
                    .flex_shrink_0()
                    .cursor_pointer()
                    .on_click(move |_, _, cx| navigate(Route::Game(open_id.clone()), cx))
                    .child(GameCover::new(game).width(rems(3.5)))
                    .into_any_element()
            }
            _ => {
                let icon = match item.post_action {
                    PostAction::InstallSetup { .. } => IconName::PackageOpen,
                    _ => web_icon(item.kind),
                };
                div()
                    .size(rems(2.5))
                    .flex_shrink_0()
                    .rounded(radius_lg)
                    .bg(muted_bg)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(icon).small().text_color(muted))
                    .into_any_element()
            }
        };

        // At most ONE primary CTA button per row.
        let primary_cta: Option<AnyElement> = match item.state {
            WebDownloadState::Done => match &item.post_action {
                PostAction::Play(game_id) if playable_game => {
                    let launching = play_status == Some(StatusKind::Launching);
                    let busy =
                        play_status.is_some_and(|k| k.is_busy() && k != StatusKind::Launching);
                    let play_id = game_id.clone();
                    (!busy).then(|| {
                        Button::new(ElementId::Name(format!("web-play-{key}").into()))
                            .small()
                            .primary()
                            .icon(IconName::Play)
                            .label(t("manage.downloads.history.play"))
                            .loading(launching)
                            .on_click(move |_, window, cx| {
                                if !launching {
                                    irori_ui::hooks::play_game(play_id.clone(), window, cx);
                                }
                            })
                            .into_any_element()
                    })
                }
                PostAction::InstallSetup { .. } => {
                    let install_id = id.clone();
                    let install_title = item.title.clone();
                    let installing = self.installing_web.contains(&id);
                    Some(
                        Button::new(ElementId::Name(format!("web-install-{key}").into()))
                            .small()
                            .primary()
                            .label(t("manage.downloads.web.install"))
                            .loading(installing)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.install_web_item(
                                    install_id.clone(),
                                    install_title.clone(),
                                    window,
                                    cx,
                                );
                            }))
                            .into_any_element(),
                    )
                }
                PostAction::Play(_) | PostAction::ShowInFolder | PostAction::None => {
                    let show_path = reveal_path.clone();
                    Some(
                        Button::new(ElementId::Name(format!("web-show-{key}").into()))
                            .small()
                            .outline()
                            .icon(IconName::FolderOpen)
                            .label(t("manage.downloads.files.show"))
                            .on_click(move |_, _, cx| cx.reveal_path(&show_path))
                            .into_any_element(),
                    )
                }
            },
            WebDownloadState::Failed | WebDownloadState::Canceled => {
                let retry_id = id.clone();
                Some(
                    Button::new(ElementId::Name(format!("web-retry-{key}").into()))
                        .small()
                        .outline()
                        .icon(IconName::RotateCcw)
                        .label(t("manage.downloads.history.retry"))
                        .on_click(move |_, window, cx| retry_web(retry_id.clone(), window, cx))
                        .into_any_element(),
                )
            }
            _ => None,
        };

        let title_el: AnyElement = match &item.post_action {
            PostAction::Play(game_id) if playable_game => {
                let gid = game_id.clone();
                div()
                    .id(ElementId::Name(
                        format!("web-title-open-{}", id.as_str()).into(),
                    ))
                    .text_base()
                    .font_semibold()
                    .truncate()
                    .cursor_pointer()
                    .on_click(move |_, _, cx| navigate(Route::Game(gid.clone()), cx))
                    .child(item.title.clone())
                    .into_any_element()
            }
            _ => div()
                .text_base()
                .font_semibold()
                .truncate()
                .child(item.title.clone())
                .into_any_element(),
        };

        let menu_reveal = reveal_path.clone();
        let menu_add_path = reveal_path;
        let menu_remove_id = id.clone();
        let menu_trash_id = id.clone();
        let offer_add_to_library = item.state == WebDownloadState::Done;

        h_flex()
            .id(ElementId::Name(format!("web-history-{key}").into()))
            .debug_selector(move || format!("web-history-{key}"))
            .gap_3()
            .px_4()
            .py_3()
            .items_center()
            .when(ix > 0, |d| d.border_t_1().border_color(border))
            .hover(move |s| s.bg(hover_bg))
            .child(thumb)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(title_el)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .min_w_0()
                            .when_some(status_badge, |row, (icon, color, pill_bg, label)| {
                                row.child(
                                    h_flex()
                                        .gap_1()
                                        .items_center()
                                        .flex_shrink_0()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_full()
                                        .bg(pill_bg)
                                        .text_xs()
                                        .font_medium()
                                        .text_color(color)
                                        .child(Icon::new(icon).xsmall().text_color(color))
                                        .child(label),
                                )
                            })
                            .child(div().text_sm().text_color(muted).truncate().child(meta)),
                    )
                    .when_some(item.status_note.clone(), |col, note| {
                        col.child(div().text_xs().text_color(muted).line_clamp(2).child(note))
                    })
                    .when_some(item.error.clone(), |col, err| {
                        col.child(div().text_xs().text_color(danger).line_clamp(2).child(err))
                    }),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .flex_shrink_0()
                    .items_center()
                    .children(primary_cta)
                    .child(
                        Button::new(ElementId::Name(
                            format!("web-history-menu-{}", id.as_str()).into(),
                        ))
                        .ghost()
                        .small()
                        .icon(IconName::EllipsisVertical)
                        .tooltip(t("manage.downloads.web.more"))
                        .dropdown_menu(move |mut menu, _, _| {
                            let show = menu_reveal.clone();
                            menu = menu.item(
                                PopupMenuItem::new(t("manage.downloads.files.show"))
                                    .icon(IconName::FolderOpen)
                                    .on_click(move |_, _, cx| cx.reveal_path(&show)),
                            );
                            if offer_add_to_library {
                                let add_path = menu_add_path.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(t("manage.downloads.web.add_to_library"))
                                        .icon(IconName::Plus)
                                        .on_click(move |_, window, cx| {
                                            irori_ui::hooks::add_to_library(
                                                add_path.clone(),
                                                window,
                                                cx,
                                            );
                                        }),
                                );
                            }
                            let remove_id = menu_remove_id.clone();
                            let trash_id = menu_trash_id.clone();
                            menu.separator()
                                .item(
                                    PopupMenuItem::new(t("manage.downloads.web.remove_list"))
                                        .icon(IconName::X)
                                        .on_click(move |_, window, cx| {
                                            remove_web(remove_id.clone(), false, window, cx)
                                        }),
                                )
                                .item(
                                    PopupMenuItem::new(t("manage.downloads.web.move_to_trash"))
                                        .icon(IconName::Trash)
                                        .on_click(move |_, window, cx| {
                                            remove_web(trash_id.clone(), true, window, cx)
                                        }),
                                )
                        }),
                    ),
            )
            .into_any_element()
    }

    fn install_web_item(
        &mut self,
        id: WebDownloadId,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.installing_web.insert(id.clone()) {
            return;
        }
        cx.notify();
        let b = backend(cx);
        let run_id = id.clone();
        let task = irori_ui::run(cx, async move { b.install_web_download(&run_id).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.installing_web.remove(&id);
                if let Err(err) = result {
                    notify_error(
                        t_args("manage.downloads.web.install_failed", &[("title", &title)]),
                        &err,
                        window,
                        cx,
                    );
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn retry_web(id: WebDownloadId, window: &mut Window, cx: &mut App) {
    let b = backend(cx);
    let task = irori_ui::run(cx, async move {
        tokio::task::spawn_blocking(move || b.retry_web_download(&id)).await?
    });
    window
        .spawn(cx, async move |cx| {
            if let Err(err) = task.await {
                let _ = cx.update(|window, cx| {
                    notify_error(t("manage.downloads.web.retry_failed"), &err, window, cx);
                });
            }
        })
        .detach();
}

fn remove_web(id: WebDownloadId, delete_files: bool, window: &mut Window, cx: &mut App) {
    let b = backend(cx);
    let task = irori_ui::run(
        cx,
        async move { b.remove_web_download(&id, delete_files).await },
    );
    window
        .spawn(cx, async move |cx| {
            if let Err(err) = task.await {
                let _ = cx.update(|window, cx| {
                    notify_error(t("manage.downloads.web.remove_failed"), &err, window, cx);
                });
            }
        })
        .detach();
}
