//! Page header: the cover with the primary action, tools and rating under
//! it; beside it the title, a date · platform · genres line, the summary and
//! the play stats. The sticky bar keeps the primary action in reach after
//! scrolling.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _, DropdownButton};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::select::Select;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, BoxShadow, Context, Div, Hsla, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement, SharedString, Styled, StyledImage as _, Window, div, point, px, rems,
};
use irori_core::{GameInfo, StatusKind};
use irori_services::LaunchParams;
use irori_services::userdata::MAX_RATING;
use irori_ui::components::{GameCover, ProgressSummary, art_img};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{ICON_CONTRAST, art_tint, legible};
use irori_ui::{backend, models};

use super::GamePage;
use crate::state::PrimaryAction;
use crate::{manage, ops, saves};

/// How far the sticky bar slides down as it appears, in pixels.
const STICKY_DROP_PX: f32 = 24.0;
/// Width of the cover column.
const COVER_WIDTH: f32 = 15.;
/// Cover width on narrow pages, where the summary needs the room.
const COMPACT_COVER_WIDTH: f32 = 10.;
/// Pages narrower than this (in rem) get the compact cover.
const COMPACT_PAGE_REM: f32 = 52.;
/// Pages narrower than this (in rem) stack the cover above the title.
const STACKED_PAGE_REM: f32 = 36.;
/// Genres shown in the header's line under the title.
const HEADER_GENRES: usize = 3;

/// "Rate 3 stars", for a star button and the rating menu.
pub(super) fn rate_label(stars: u8) -> SharedString {
    let key = if stars == 1 {
        "game.rating.rate_one"
    } else {
        "game.rating.rate_other"
    };
    t_args(key, &[("count", &stars.to_string())])
}

impl GamePage {
    /// The colour of the game's backdrop art, once known.
    fn art_tint(&self, game: &GameInfo, cx: &mut Context<Self>) -> Option<Hsla> {
        let url = game.hero_art()?.to_owned();
        irori_ui::art::ambient(&url, cx).map(|a| art_tint(a.rgb, cx))
    }

    /// The page header: back link, then the cover column beside the title,
    /// details line, summary and stats.
    pub(super) fn render_header(
        &mut self,
        game: &Arc<GameInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let page_rem = self.page_rem(window).unwrap_or(f32::MAX);
        let stacked = page_rem < STACKED_PAGE_REM;
        let cover_width = if page_rem < COMPACT_PAGE_REM {
            COMPACT_COVER_WIDTH
        } else {
            COVER_WIDTH
        };
        let cover = self.render_cover_column(game, cover_width, cx);
        let info = self.render_info(game, cx);
        let status = self.render_status_line(game, cx);
        let menu_id = self.id.clone();
        let shortcuts = self.shortcuts_exist;
        let store_url = self.store_url(game);
        let page = cx.entity().downgrade();
        v_flex()
            .id("game-header")
            .w_full()
            .px_8()
            .pt_4()
            .pb_6()
            .gap_6()
            .child(h_flex().child(back_button(cx)))
            .child(
                div()
                    .w_full()
                    .flex()
                    .when_else(
                        stacked,
                        |d| d.flex_col().gap_6(),
                        |d| d.flex_row().items_start().gap_8(),
                    )
                    .child(cover)
                    .child(info),
            )
            .children(status)
            .context_menu(move |menu, window, cx| {
                let menu = super::menu::build_menu(
                    menu,
                    &menu_id,
                    shortcuts,
                    store_url.clone(),
                    Some(page.clone()),
                    window,
                    cx,
                );
                super::menu::rating_submenu(menu, &menu_id, window, cx)
            })
            .into_any_element()
    }

    /// The portrait cover, the primary action under it at its width, then
    /// the game's tools and the user's rating.
    fn render_cover_column(
        &mut self,
        game: &Arc<GameInfo>,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let action = self.primary_action(cx).unwrap_or(PrimaryAction::Install);
        let halo = matches!(action, PrimaryAction::Play)
            .then(|| self.art_tint(game, cx))
            .flatten();
        let primary = self.render_primary(action, game, true, cx);
        // Play glows in the game's own colour; other actions stay plain.
        let primary = div()
            .rounded(cx.theme().radius)
            .when_some(halo, |d, tint| {
                d.shadow(vec![BoxShadow {
                    color: tint.opacity(0.45),
                    offset: point(px(0.), px(4.)),
                    blur_radius: px(24.),
                    spread_radius: px(0.),
                    inset: false,
                }])
            })
            .child(primary);
        let tools = self.render_tools(game, cx);
        let rating = self.render_rating(cx);
        // The column is as wide as the cover, or wider when a long action
        // label needs it; everything in it stretches to that width.
        v_flex()
            .flex_shrink_0()
            .min_w(rems(width))
            .gap_3()
            .child(
                div()
                    .w(rems(width))
                    .rounded_lg()
                    .overflow_hidden()
                    .shadow_lg()
                    .child(GameCover::new(game.clone()).width(rems(width))),
            )
            .child(primary)
            .child(tools)
            .child(rating)
            .into_any_element()
    }

    /// Store, title, the details line, region tags, the summary and stats.
    fn render_info(&mut self, game: &Arc<GameInfo>, cx: &mut Context<Self>) -> AnyElement {
        let ink = self.ink;
        let logo = game
            .art_logo
            .clone()
            .filter(|l| !l.is_empty())
            .and_then(|url| art_img(&url, 720, true, cx));
        let heading = match logo {
            Some(logo) => div()
                .h(rems(7.))
                .max_w_full()
                .overflow_hidden()
                .child(
                    logo.image_cache(&self.image_cache)
                        .h(rems(7.))
                        .max_w(rems(28.))
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element(),
            None => div()
                .text_size(rems(3.))
                .line_height(rems(3.4))
                .font_extrabold()
                .text_color(ink.text)
                .line_clamp(2)
                .child(game.title.clone())
                .into_any_element(),
        };
        let meta = self.render_meta_line(game);
        let tags = (!game.tags.is_empty()).then(|| {
            h_flex()
                .gap_2()
                .flex_wrap()
                .children(game.tags.iter().map(|g| Tag::secondary().child(g.clone())))
        });
        let summary = self.render_summary(game, cx);
        let stats = self.render_stats(cx);
        let store = game
            .system
            .as_deref()
            .and_then(irori_engines::systems::get)
            .map_or(game.runner().store_name(), |s| s.name.as_str());
        v_flex()
            .flex_1()
            .min_w_0()
            .gap_3()
            .pt_2()
            .child(
                div()
                    .text_xs()
                    .font_bold()
                    .text_color(ink.muted)
                    .child(SharedString::from(store.to_uppercase())),
            )
            .child(heading)
            .children(meta.map(|m| div().text_color(ink.muted).line_clamp(2).child(m)))
            .children(tags)
            .children(summary)
            .child(stats)
            .into_any_element()
    }

    /// "Feb 25, 2022 · Windows, Linux · Role-playing · Adventure".
    fn render_meta_line(&self, game: &GameInfo) -> Option<String> {
        let date = self.release_date(game);
        let platforms = match &game.install {
            Some(install) if game.is_installed => {
                Some(crate::i18n::platform_label(install.platform).to_string())
            }
            _ => Some(
                crate::install::installable_platforms(game)
                    .iter()
                    .map(|p| crate::i18n::platform_label(*p).to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        }
        .filter(|p| !p.is_empty() && game.runner() != irori_core::Runner::Emulation);
        let genres = self
            .extra
            .ready()
            .map(|e| e.genres.clone())
            .filter(|g| !g.is_empty())
            .unwrap_or_else(|| game.genres.clone());
        let parts: Vec<String> = date
            .into_iter()
            .chain(platforms)
            .chain(genres.into_iter().take(HEADER_GENRES))
            .collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// The store's short description, with a link to the full text.
    fn render_summary(&self, game: &GameInfo, cx: &mut Context<Self>) -> Option<AnyElement> {
        let extra = self.extra.ready();
        let short = extra.and_then(|e| e.short_description.clone());
        let full = extra
            .and_then(|e| e.description.clone())
            .or_else(|| game.description.clone());
        // GOG's short "lead" is often a promotion (pre-order offers), not a summary.
        let text = if game.runner() == irori_core::Runner::Gog {
            full.or(short)
        } else {
            short.or(full)
        }
        .map(|t| super::info::plain_text(&t))
        .filter(|t| !t.is_empty())?;
        Some(
            v_flex()
                .max_w(rems(48.))
                .gap_1()
                .items_start()
                .child(
                    div()
                        .text_color(self.ink.text)
                        .opacity(0.92)
                        .line_clamp(3)
                        .child(text),
                )
                .child(
                    Button::new("read-more")
                        .link()
                        .small()
                        .label(t("game.header.read_more"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.tab = super::InfoTab::About;
                            cx.notify();
                        })),
                )
                .into_any_element(),
        )
    }

    /// Play time, last played and achievement progress, under captions.
    fn render_stats(&self, cx: &Context<Self>) -> AnyElement {
        let ink = self.ink;
        let stat = |label: SharedString, value: AnyElement| {
            v_flex()
                .gap_0p5()
                .min_w_0()
                .child(
                    div()
                        .text_xs()
                        .font_bold()
                        .text_color(ink.muted)
                        .child(SharedString::from(label.to_uppercase())),
                )
                .child(
                    div()
                        .text_lg()
                        .font_semibold()
                        .text_color(ink.text)
                        .truncate()
                        .child(value),
                )
        };
        let record = models(cx)
            .library
            .read(cx)
            .user
            .playtime
            .get(&self.id.key())
            .cloned();
        let minutes = record.as_ref().map(|r| r.total_minutes).unwrap_or(0);
        let last = record.and_then(|r| r.last_played);
        let now = irori_core::util::unix_now() as i64;
        let mut row = h_flex().pt_2().gap_x_8().gap_y_3().flex_wrap().child(stat(
            t("game.info.playtime"),
            crate::i18n::playtime_label(minutes, last.is_some()).into_any_element(),
        ));
        if let Some(last) = last {
            row = row.child(stat(
                t("game.info.last_played"),
                crate::load::format_day(&last, now).into_any_element(),
            ));
        }
        if let Some(list) = self.achievements.ready().filter(|a| !a.is_empty()) {
            let unlocked = list.iter().filter(|a| a.unlocked_at.is_some()).count();
            let percent = crate::state::completion_percent(unlocked, list.len());
            row = row.child(stat(
                t("game.header.achievements"),
                h_flex()
                    .gap_2()
                    .child(format!("{unlocked} / {}", list.len()))
                    .child(
                        div()
                            .w(rems(6.))
                            .child(Progress::new("achievement-progress").value(percent as f32)),
                    )
                    .into_any_element(),
            ));
        }
        row.into_any_element()
    }

    /// Cloud saves, settings, favourite and more actions, as icon buttons.
    fn render_tools(&self, game: &Arc<GameInfo>, cx: &mut Context<Self>) -> AnyElement {
        let fav = models(cx).library.read(cx).is_favourite(&self.id);
        let settings_id = self.id.clone();
        let saves_id = self.id.clone();
        let menu_id = self.id.clone();
        let shortcuts = self.shortcuts_exist;
        let store_url = self.store_url(game);
        let page = cx.entity().downgrade();
        let store_supports = backend(cx)
            .store(self.id.runner)
            .is_some_and(|s| s.supports_cloud_saves());
        let cloud = saves::CloudSupport::of(game, store_supports);
        h_flex()
            .gap_1()
            .justify_center()
            .child(irori_ui::play_status::button(
                "game-play-status",
                &self.id,
                cx,
            ))
            .child(
                Button::new("game-favourite")
                    .ghost()
                    .icon(if fav {
                        IconName::StarFill
                    } else {
                        IconName::Star
                    })
                    .accessibility_label(if fav {
                        t("game.hero.favourite")
                    } else {
                        t("game.hero.add_favourite")
                    })
                    .tooltip(if fav {
                        t("game.hero.favourite")
                    } else {
                        t("game.hero.add_favourite")
                    })
                    .selected(fav)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_favourite(cx))),
            )
            // Steam applies its own launch settings to its games.
            .when(self.id.runner.manages_installs(), |row| {
                row.child(
                    Button::new("game-settings")
                        .ghost()
                        .icon(IconName::Settings2)
                        .accessibility_label(t("game.hero.settings"))
                        .tooltip(t("game.hero.settings"))
                        .on_click(move |_, _, cx| ops::open_settings(&settings_id, "general", cx)),
                )
            })
            .when(cloud.offers_sheet(), |row| {
                let tooltip = cloud
                    .explanation(game)
                    .unwrap_or_else(|| t("game.hero.cloud_saves").to_string());
                row.child(
                    Button::new("game-cloud-saves")
                        .ghost()
                        .icon(IconName::Cloud)
                        .accessibility_label(t("game.hero.cloud_saves"))
                        .tooltip(tooltip)
                        .disabled(cloud != saves::CloudSupport::Supported)
                        .on_click(move |_, window, cx| {
                            saves::open_cloud_saves_sheet(saves_id.clone(), window, cx)
                        }),
                )
            })
            .child(
                Button::new("game-more")
                    .ghost()
                    .icon(IconName::Ellipsis)
                    .accessibility_label(t("game.hero.more_actions"))
                    .tooltip(t("game.hero.more_actions"))
                    .dropdown_menu(move |menu, window, cx| {
                        let menu = super::menu::build_menu(
                            menu,
                            &menu_id,
                            shortcuts,
                            store_url.clone(),
                            Some(page.clone()),
                            window,
                            cx,
                        );
                        super::menu::rating_submenu(menu, &menu_id, window, cx)
                    }),
            )
            .into_any_element()
    }

    /// "Your rating" and five stars. Choosing the current rating again
    /// clears it.
    fn render_rating(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = models(cx).library.read(cx).rating(&self.id).unwrap_or(0);
        let ink = self.ink;
        let lit = legible(cx.theme().warning, &ink.surfaces, ICON_CONTRAST, cx);
        let unlit = legible(
            cx.theme().muted_foreground,
            &ink.surfaces,
            ICON_CONTRAST,
            cx,
        );
        let stars = (1..=MAX_RATING).map(|stars| {
            let on = stars <= current;
            let id = self.id.clone();
            Button::new(("game-rating", stars as usize))
                .ghost()
                .xsmall()
                .icon(
                    Icon::new(if on {
                        IconName::StarFill
                    } else {
                        IconName::Star
                    })
                    .text_color(if on { lit } else { unlit }),
                )
                .accessibility_label(rate_label(stars))
                .tooltip(if stars == current {
                    t("game.rating.clear")
                } else {
                    rate_label(stars)
                })
                .on_click(move |_, _, cx| {
                    let next = (stars != current).then_some(stars);
                    ops::set_rating(&id, next, cx);
                })
        });
        h_flex()
            .w_full()
            .gap_2()
            .justify_between()
            .flex_wrap()
            .child(
                div()
                    .text_xs()
                    .text_color(ink.muted)
                    .child(t("game.rating.label")),
            )
            .child(h_flex().gap_0p5().children(stars))
            .into_any_element()
    }

    /// Slim bar pinned to the top once the header scrolled away: cover, title, action.
    pub(super) fn render_sticky_bar(
        &mut self,
        game: &Arc<GameInfo>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let action = self.primary_action(cx).unwrap_or(PrimaryAction::Install);
        let primary = self.render_primary(action, game, false, cx);
        let thumb = game
            .cover_art()
            .map(str::to_owned)
            .and_then(|url| art_img(&url, 96, false, cx));
        let theme = cx.theme();
        let bar = h_flex()
            .id("game-sticky-bar")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .px_8()
            .py_2()
            .gap_3()
            .bg(theme.background)
            .border_b_1()
            .border_color(theme.border)
            .shadow_md()
            .when_some(thumb, |row, img| {
                row.child(
                    img.image_cache(&self.image_cache)
                        .h(rems(3.))
                        .w(rems(2.))
                        .rounded_sm()
                        .object_fit(ObjectFit::Cover),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_semibold()
                    .child(game.title.clone()),
            )
            .child(primary);
        irori_ui::motion::drop_in("game-sticky-bar-in", bar, STICKY_DROP_PX)
    }

    fn store_url(&self, game: &GameInfo) -> Option<String> {
        self.extra
            .ready()
            .and_then(|e| e.store_url.clone())
            .or_else(|| game.store_url.clone())
    }

    /// The primary action and its companions, side by side or, `stacked`,
    /// one above the other at the column's width.
    fn render_primary(
        &mut self,
        action: PrimaryAction,
        game: &GameInfo,
        stacked: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = self.id.clone();
        let row: Div = if stacked {
            v_flex().gap_2()
        } else {
            h_flex().gap_2()
        };
        match action {
            PrimaryAction::Install => row
                .child(
                    Button::new("primary-action")
                        .primary()
                        .large()
                        .icon(IconName::Download)
                        .label(if id.runner.manages_installs() {
                            t("game.common.install_ellipsis")
                        } else {
                            t_args(
                                "game.menu.install_in_client",
                                &[("store", id.runner.store_name())],
                            )
                        })
                        .on_click(cx.listener(|this, _, window, cx| this.run_primary(window, cx))),
                )
                .into_any_element(),
            PrimaryAction::Play => row
                .child(self.render_play_button(cx))
                .when(self.launch_options.len() > 1, |r| {
                    r.child(
                        div().when(!stacked, |d| d.w(rems(14.))).child(
                            Select::new(&self.launch_select)
                                .placeholder(t("game.hero.launch_option_placeholder")),
                        ),
                    )
                })
                .into_any_element(),
            PrimaryAction::Update => row
                .child(
                    Button::new("primary-action")
                        .primary()
                        .large()
                        .icon(IconName::CloudDownload)
                        .label(t("game.action.update"))
                        .on_click(cx.listener(|this, _, window, cx| this.run_primary(window, cx))),
                )
                .child(
                    Button::new("play-without-update")
                        .large()
                        .outline()
                        .icon(IconName::Play)
                        .label(t("game.action.play"))
                        .tooltip(t("game.hero.play_without_update"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            let params = LaunchParams {
                                option: this.selected_option.clone(),
                                skip_version_check: true,
                                ..Default::default()
                            };
                            this.play_with(params, window, cx);
                        })),
                )
                .into_any_element(),
            PrimaryAction::Stop => row
                .child(
                    Button::new("primary-action")
                        .large()
                        .icon(IconName::Square)
                        .label(t("game.action.stop"))
                        .loading(self.pending.contains("stop"))
                        .on_click(cx.listener(|this, _, window, cx| this.run_primary(window, cx))),
                )
                .child(
                    Button::new("running-log")
                        .large()
                        .ghost()
                        .icon(IconName::ScrollText)
                        .label(t("game.menu.open_log"))
                        .on_click(cx.listener(|this, _, window, cx| {
                            crate::open_log_sheet(this.id.clone(), window, cx)
                        })),
                )
                .into_any_element(),
            PrimaryAction::Launching => row
                .child(
                    Button::new("primary-action")
                        .primary()
                        .large()
                        .label(t("game.action.launching"))
                        .loading(true)
                        .disabled(true),
                )
                .into_any_element(),
            PrimaryAction::Downloading { update } => row
                .child(
                    Button::new("primary-action")
                        .large()
                        .outline()
                        .icon(IconName::Pause)
                        .label(t("game.hero.pause"))
                        .tooltip(t("game.hero.pause_tooltip"))
                        .on_click(|_, _, cx| ops::pause_downloads(cx)),
                )
                .child(
                    Button::new("cancel-download")
                        .large()
                        .ghost()
                        .icon(IconName::X)
                        .label(if update {
                            t("game.hero.cancel_update")
                        } else {
                            t("game.hero.cancel_install")
                        })
                        .on_click(move |_, window, cx| {
                            if update {
                                ops::cancel_download(&id, true, cx);
                            } else {
                                manage::open_cancel_download_dialog(id.clone(), window, cx);
                            }
                        }),
                )
                .into_any_element(),
            PrimaryAction::Resume => {
                let installed = game.is_installed;
                row.child(
                    Button::new("primary-action")
                        .primary()
                        .large()
                        .icon(IconName::Play)
                        .label(t("game.action.resume"))
                        .on_click(cx.listener(|this, _, window, cx| this.run_primary(window, cx))),
                )
                .child(
                    Button::new("cancel-download")
                        .large()
                        .ghost()
                        .icon(IconName::X)
                        .label(t("game.hero.cancel_install"))
                        .on_click(move |_, window, cx| {
                            if installed {
                                ops::cancel_download(&id, true, cx);
                            } else {
                                manage::open_cancel_download_dialog(id.clone(), window, cx);
                            }
                        }),
                )
                .into_any_element()
            }
            PrimaryAction::Queued => {
                let position = models(cx)
                    .downloads
                    .read(cx)
                    .queue
                    .active
                    .iter()
                    .position(|i| i.id == self.id);
                row.child(
                    Button::new("primary-action")
                        .large()
                        .outline()
                        .icon(IconName::X)
                        .label(t("game.action.remove_from_queue"))
                        .on_click(cx.listener(|this, _, window, cx| this.run_primary(window, cx))),
                )
                .when_some(position, |r, p| {
                    r.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t_args(
                                "game.hero.queue_position",
                                &[("position", &(p + 1).to_string())],
                            )),
                    )
                })
                .into_any_element()
            }
            PrimaryAction::Busy(kind) => row
                .child(
                    Button::new("primary-action")
                        .large()
                        .label(crate::i18n::status_label(kind))
                        .loading(true)
                        .disabled(true),
                )
                .into_any_element(),
            PrimaryAction::Missing => row
                .child(
                    Button::new("primary-action")
                        .primary()
                        .large()
                        .icon(IconName::FolderSearch)
                        .label(t("game.action.locate_files"))
                        .on_click(cx.listener(|this, _, window, cx| this.run_primary(window, cx))),
                )
                .into_any_element(),
            PrimaryAction::NotInstallable | PrimaryAction::Unsupported => row
                .child(
                    Button::new("primary-action")
                        .large()
                        .icon(IconName::Ban)
                        .label(action.label())
                        .disabled(true),
                )
                .into_any_element(),
        }
    }

    /// Play as a split button: launch options (when the store offers
    /// several) and one-off variants (verbose logs, no update check).
    fn render_play_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let play = Button::new("primary-action")
            .primary()
            .large()
            .flex_1()
            .icon(IconName::Play)
            .label(t("game.action.play"))
            .on_click(cx.listener(|this, _, window, cx| this.run_primary(window, cx)));
        let page = cx.entity();
        let options = if self.launch_options.len() > 1 {
            self.launch_options.clone()
        } else {
            Vec::new()
        };
        if options.is_empty() && !self.id.runner.manages_installs() {
            return play.into_any_element();
        }
        let current = self.selected_option.clone();
        let emulated = self.id.runner == irori_core::Runner::Emulation;
        let id = self.id.clone();
        DropdownButton::new("play-split")
            .primary()
            .large()
            .w_full()
            .button(play)
            .menu_accessibility_label(t("game.hero.play_with"))
            .dropdown_menu(move |mut menu, _, _| {
                menu = menu.label(t("game.hero.play_with"));
                for option in &options {
                    let page = page.clone();
                    let chosen = option.clone();
                    menu = menu.item(
                        PopupMenuItem::new(option.label())
                            .checked(current.as_ref() == Some(option))
                            .on_click(move |_, window, cx| {
                                let chosen = chosen.clone();
                                page.update(cx, |p, cx| p.play(Some(chosen), window, cx));
                            }),
                    );
                }
                if !options.is_empty() {
                    menu = menu.separator();
                }
                if emulated {
                    let id = id.clone();
                    menu = menu
                        .item(
                            PopupMenuItem::new(t("game.play_with.menu"))
                                .icon(IconName::Joystick)
                                .on_click(move |_, window, cx| {
                                    crate::open_play_with_dialog(id.clone(), window, cx)
                                }),
                        )
                        .separator();
                }
                for (label, icon, params) in super::menu::PLAY_VARIANTS {
                    let page = page.clone();
                    let option = current.clone();
                    menu = menu.item(PopupMenuItem::new(t(label)).icon(icon).on_click(
                        move |_, window, cx| {
                            let p = params(LaunchParams {
                                option: option.clone(),
                                ..Default::default()
                            });
                            page.update(cx, |page, cx| page.play_with(p, window, cx));
                        },
                    ));
                }
                menu
            })
            .into_any_element()
    }

    /// Progress, busy detail or hints below the actions.
    fn render_status_line(&self, game: &GameInfo, cx: &Context<Self>) -> Option<AnyElement> {
        let m = models(cx);
        let status = m.statuses.read(cx).get(&self.id).cloned();
        if let Some(status) = status.filter(|s| {
            s.kind.is_busy() && !matches!(s.kind, StatusKind::Playing | StatusKind::Queued)
        }) {
            return Some(
                div()
                    .max_w(rems(36.))
                    .child(ProgressSummary::new("game-progress", status))
                    .into_any_element(),
            );
        }
        let muted = cx.theme().muted_foreground;
        let online = m.accounts.read(cx).online;
        if game.is_installed && !online && !game.can_run_offline {
            return Some(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(cx.theme().warning)
                    .child(Icon::new(IconName::WifiOff).small())
                    .child(t("game.page.offline_warning"))
                    .into_any_element(),
            );
        }
        if !self.files_available && game.is_installed {
            let path = game
                .install_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            return Some(
                div()
                    .text_sm()
                    .text_color(cx.theme().warning)
                    .child(t_args("game.hero.files_missing", &[("path", &path)]))
                    .into_any_element(),
            );
        }
        if game.is_installed && online && !game.can_run_offline {
            return Some(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t("game.hero.needs_connection"))
                    .into_any_element(),
            );
        }
        None
    }
}

/// Returns to where the page was opened from: the library when that is where
/// the user came from or there is no history (opened from a link), "Back"
/// otherwise.
fn back_button(cx: &App) -> Button {
    let previous = irori_ui::nav::navigator(cx).read(cx).previous().cloned();
    let button = Button::new("game-back").ghost().icon(IconName::ArrowLeft);
    match previous {
        None => button
            .label(t("game.page.library"))
            .tooltip(t("game.page.back_to_library"))
            .on_click(|_, _, cx| irori_ui::navigate(irori_ui::Route::Library, cx)),
        Some(route) => button
            .label(if route == irori_ui::Route::Library {
                t("game.page.library")
            } else {
                t("game.page.back")
            })
            .tooltip(t("game.page.back"))
            .on_click(|_, _, cx| irori_ui::nav::go_back(cx)),
    }
}
