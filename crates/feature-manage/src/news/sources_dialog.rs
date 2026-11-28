//! News sources dialog for `Stores -> News -> Gaming news`: toggles the 5
//! built-in gaming-only RSS presets and adds or removes custom RSS/Atom feeds.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{AppContext as _, Context, ElementId, ParentElement as _, Styled as _, Window, div};
use irori_services::{CustomFeed, PRESET_FEEDS, validate_feed_url};
use irori_ui::i18n::t;

use super::{NewsSection, now_unix};
use crate::common::notify_success;

impl NewsSection {
    pub(super) fn open_sources_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name_input = cx
            .new(|cx| InputState::new(window, cx).placeholder(t("manage.news.gaming.custom_name")));
        let url_input = cx
            .new(|cx| InputState::new(window, cx).placeholder(t("manage.news.gaming.custom_url")));
        let view = cx.entity();

        window.open_dialog(cx, move |dialog, _, cx| {
            let cfg = view.read(cx).gaming.config.clone();
            let muted = cx.theme().muted_foreground;
            let border = cx.theme().border;
            let name_in = name_input.clone();
            let url_in = url_input.clone();

            let mut presets_list = v_flex().w_full().gap_2();
            for preset in PRESET_FEEDS {
                let enabled = !cfg.disabled_presets.iter().any(|d| d == preset.id);
                let preset_id = preset.id.to_string();
                let view_toggle = view.clone();
                presets_list = presets_list.child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .py_1()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_sm().font_medium().child(preset.name))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .truncate()
                                        .child(preset.url),
                                ),
                        )
                        .child(
                            Switch::new(ElementId::Name(format!("preset-sw-{}", preset.id).into()))
                                .checked(enabled)
                                .on_click(move |checked, window, cx| {
                                    let pid = preset_id.clone();
                                    let on = *checked;
                                    view_toggle.update(cx, |this, cx| {
                                        let mut next = this.gaming.config.clone();
                                        next.disabled_presets.retain(|d| d != &pid);
                                        if !on {
                                            next.disabled_presets.push(pid);
                                        }
                                        this.save_config_and_reload(next, window, cx);
                                    });
                                }),
                        ),
                );
            }

            let mut custom_list = v_flex().w_full().gap_2();
            for custom in &cfg.custom_feeds {
                let remove_id = custom.id.clone();
                let view_rm = view.clone();
                custom_list = custom_list.child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_sm().font_medium().child(custom.name.clone()))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .truncate()
                                        .child(custom.url.clone()),
                                ),
                        )
                        .child(
                            Button::new(ElementId::Name(format!("custom-rm-{}", custom.id).into()))
                                .xsmall()
                                .ghost()
                                .icon(IconName::Trash)
                                .label(t("manage.news.gaming.remove_feed"))
                                .on_click(move |_, window, cx| {
                                    let rid = remove_id.clone();
                                    view_rm.update(cx, |this, cx| {
                                        let mut next = this.gaming.config.clone();
                                        next.custom_feeds.retain(|f| f.id != rid);
                                        this.save_config_and_reload(next, window, cx);
                                    });
                                }),
                        ),
                );
            }

            let view_add = view.clone();
            let add_name = name_in.clone();
            let add_url = url_in.clone();

            dialog
                .title(t("manage.news.gaming.sources_title"))
                .child(
                    v_flex()
                        .w_full()
                        .gap_4()
                        .child(
                            v_flex()
                                .w_full()
                                .gap_1p5()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(muted)
                                        .child(t("manage.news.gaming.presets_heading")),
                                )
                                .child(presets_list),
                        )
                        .child(div().w_full().border_t_1().border_color(border))
                        .child(
                            v_flex()
                                .w_full()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(muted)
                                        .child(t("manage.news.gaming.custom_heading")),
                                )
                                .child(custom_list)
                                .child(Input::new(&name_in).small())
                                .child(Input::new(&url_in).small())
                                .child(
                                    Button::new("gaming-add-custom")
                                        .small()
                                        .outline()
                                        .icon(IconName::Plus)
                                        .label(t("manage.news.gaming.add_feed"))
                                        .on_click(move |_, window, cx| {
                                            let raw_url = add_url.read(cx).value().to_string();
                                            let valid_url = match validate_feed_url(&raw_url) {
                                                Ok(u) => u,
                                                Err(_) => {
                                                    window.push_notification(
                                                        Notification::error(t(
                                                            "manage.news.gaming.invalid_url",
                                                        )),
                                                        cx,
                                                    );
                                                    return;
                                                }
                                            };
                                            let raw_name =
                                                add_name.read(cx).value().trim().to_string();
                                            let label = if raw_name.is_empty() {
                                                valid_url.clone()
                                            } else {
                                                raw_name
                                            };
                                            let id = format!("custom-{}", now_unix());
                                            view_add.update(cx, |this, cx| {
                                                let mut next = this.gaming.config.clone();
                                                if !next
                                                    .custom_feeds
                                                    .iter()
                                                    .any(|f| f.url == valid_url)
                                                {
                                                    next.custom_feeds.push(CustomFeed {
                                                        id,
                                                        name: label,
                                                        url: valid_url,
                                                    });
                                                }
                                                this.save_config_and_reload(next, window, cx);
                                            });
                                            add_name.update(cx, |i, cx| {
                                                i.set_value("", window, cx);
                                            });
                                            add_url.update(cx, |i, cx| {
                                                i.set_value("", window, cx);
                                            });
                                            notify_success(
                                                t("manage.news.gaming.feed_saved"),
                                                window,
                                                cx,
                                            );
                                        }),
                                ),
                        ),
                )
                .footer(
                    DialogFooter::new().child(
                        Button::new("gaming-sources-done")
                            .primary()
                            .label(t("manage.news.gaming.close"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
                )
        });
    }
}
