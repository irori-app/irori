//! Right-hand native Markdown reader pane for `Stores -> News -> My games`:
//! clickable game header, live adaptive Play/Update/Stop/Install CTA, mute with
//! undo notification, Steam CDN banner, and `Open original` link.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, IntoElement, ObjectFit, ParentElement as _, Styled as _, StyledImage as _,
    div, rems,
};
use irori_core::UpdateRequest;
use irori_services::NewsKind;
use irori_ui::components::art_img;
use irori_ui::i18n::{t, t_args};
use irori_ui::rich_text::{prepare_steam_news, rich_text_keyed};
use irori_ui::style::panel;
use irori_ui::{Route, backend, hooks, models, navigate};

use super::NewsSection;
use super::my_games::{format_date, store_chip_label};
use crate::common::backend_op;

impl NewsSection {
    pub(super) fn render_reader_pane(
        &self,
        selected_idx: Option<usize>,
        now_ts: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let Some(entry) = selected_idx.and_then(|i| self.my_games.snapshot.entries.get(i)) else {
            return panel(cx)
                .w_full()
                .p_6()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(t("manage.news.select_prompt")),
                )
                .into_any_element();
        };

        let prep = prepare_steam_news(&entry.item.contents);
        let date = format_date(entry.item.date, now_ts);
        let meta = if !entry.item.author.trim().is_empty() && !date.is_empty() {
            format!("{date} · {}", entry.item.author.trim())
        } else if !date.is_empty() {
            date
        } else {
            entry.item.author.trim().to_string()
        };

        let lib = models(cx).library.read(cx);
        let is_playing = models(cx).statuses.read(cx).is_playing(&entry.game_id);
        let is_installed = lib
            .snapshot
            .get(&entry.game_id)
            .map(|g| g.is_installed)
            .unwrap_or(entry.is_installed);
        let has_update = lib.has_update(&entry.game_id);
        let game_id = entry.game_id.clone();
        let game_title = entry.title.clone();

        let action_btn = if is_playing {
            let stop_id = game_id.clone();
            Button::new("news-reader-stop")
                .small()
                .outline()
                .icon(IconName::CircleStop)
                .label(t("manage.news.action.stop"))
                .on_click(move |_, _, cx| {
                    let b = backend(cx);
                    let id = stop_id.clone();
                    irori_ui::run(cx, async move { b.stop(&id).await }).detach();
                })
        } else if is_installed && has_update && game_id.runner.manages_installs() {
            let upd_id = game_id.clone();
            let upd_title = game_title.clone();
            Button::new("news-reader-update")
                .small()
                .primary()
                .icon(IconName::Download)
                .label(t("manage.news.action.update"))
                .on_click(move |_, _, cx| {
                    let id = upd_id.clone();
                    let title = upd_title.clone();
                    backend_op(cx, move |b| {
                        b.enqueue_update(id, title, UpdateRequest::default());
                    });
                })
        } else if is_installed {
            let play_id = game_id.clone();
            Button::new("news-reader-play")
                .small()
                .primary()
                .icon(IconName::Play)
                .label(t("manage.news.action.play"))
                .on_click(move |_, window, cx| hooks::play_game(play_id.clone(), window, cx))
        } else {
            let inst_id = game_id.clone();
            Button::new("news-reader-install")
                .small()
                .outline()
                .icon(IconName::Download)
                .label(t("manage.news.action.install"))
                .on_click(move |_, window, cx| hooks::install_game(inst_id.clone(), window, cx))
        };

        let mute_id = game_id.clone();
        let mute_title = game_title.clone();
        let mute_view = cx.entity().downgrade();
        let mute_btn = Button::new("news-reader-mute")
            .small()
            .ghost()
            .icon(IconName::EyeOff)
            .label(t("manage.news.mute"))
            .tooltip(t("manage.news.mute_tooltip"))
            .on_click(move |_, window, cx| {
                let id_mute = mute_id.clone();
                backend_op(cx, move |b| b.mute_news(&id_mute, true));
                let _ = mute_view.update(cx, |this, cx| this.reload_cached_my_games(window, cx));

                let undo_id = mute_id.clone();
                let undo_view = mute_view.clone();
                let notice = t_args("manage.news.muted_notice", &[("game", mute_title.as_str())]);
                window.push_notification(
                    Notification::info(notice)
                        .action(move |_, _, cx| {
                            let note = cx.entity().downgrade();
                            let id_restore = undo_id.clone();
                            let v_restore = undo_view.clone();
                            Button::new("news-undo-mute")
                                .small()
                                .label(t("manage.news.undo"))
                                .on_click(move |_, window, cx| {
                                    let id_inner = id_restore.clone();
                                    backend_op(cx, move |b| b.mute_news(&id_inner, false));
                                    let _ = v_restore.update(cx, |this, cx| {
                                        this.reload_cached_my_games(window, cx)
                                    });
                                    let _ = note.update(cx, |n, cx| n.dismiss(window, cx));
                                })
                        })
                        .autohide(true),
                    cx,
                );
            });

        let nav_id = game_id.clone();
        let badge = match entry.item.kind {
            NewsKind::PatchNotes => Tag::success()
                .xsmall()
                .child(t("manage.news.filter.patches")),
            NewsKind::News => Tag::info().xsmall().child(t("manage.news.filter.news")),
        };
        let banner = prep
            .banner_url
            .as_deref()
            .and_then(|url| art_img(url, 800, false, cx))
            .map(|img| {
                img.w_full()
                    .max_h(rems(15.))
                    .rounded_md()
                    .object_fit(ObjectFit::Cover)
            });
        let source_footer = if entry.via_steam {
            t("manage.news.source_via_steam")
        } else {
            t("manage.news.source_steam")
        };
        let open_url = irori_ui::web_url(&entry.item.url);

        panel(cx)
            .w_full()
            .p_5()
            .gap_3p5()
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new("news-reader-game-link")
                                    .xsmall()
                                    .ghost()
                                    .label(entry.title.clone())
                                    .on_click(move |_, _, cx| {
                                        navigate(Route::Game(nav_id.clone()), cx);
                                    }),
                            )
                            .child(badge)
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(store_chip_label(entry.store, entry.via_steam)),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(action_btn)
                            .child(mute_btn),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xl()
                            .font_semibold()
                            .child(entry.item.title.clone()),
                    )
                    .when(!meta.is_empty(), |v| {
                        v.child(div().text_xs().text_color(muted).child(meta))
                    }),
            )
            .children(banner)
            .child(rich_text_keyed(
                format!("stores-news:{}:{}", entry.app_id, entry.item.gid),
                &prep.markdown,
            ))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .pt_2()
                    .child(div().text_xs().text_color(muted).child(source_footer))
                    .when_some(open_url, |r, url| {
                        r.child(
                            Button::new("news-reader-open-original")
                                .small()
                                .outline()
                                .icon(IconName::ExternalLink)
                                .label(t("manage.news.open_original"))
                                .on_click(move |_, _, cx| cx.open_url(&url)),
                        )
                    }),
            )
            .into_any_element()
    }
}
