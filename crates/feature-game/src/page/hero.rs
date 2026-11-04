//! The page's header (back link, title or logo, and a date · platform ·
//! genres line) and its cover column: the cover with the primary action
//! under it at its width, the sandbox card, the game's tools, the user's
//! rating and a card of quick facts. The sticky bar keeps the primary action in reach once it
//! scrolls away.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::StatefulInteractiveElement as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::select::Select;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, BoxShadow, Context, Div, Hsla, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement, Pixels, Role, SharedString, Styled, StyledImage as _, canvas, div,
    point, px, rems,
};
use irori_core::{GameInfo, StatusKind};
use irori_services::LaunchParams;
use irori_services::userdata::MAX_RATING;
use irori_ui::components::{GameCover, ProgressSummary, art_img};
use irori_ui::i18n::{t, t_args};
use irori_ui::style::{ICON_CONTRAST, KeyboardFocusable as _, art_tint, legible};
use irori_ui::{backend, models};

use super::GamePage;
use crate::hooks::{AnticheatStatus, SteamDeckStatus};
use crate::state::PrimaryAction;
use crate::{manage, ops, saves};

/// How far the sticky bar slides down as it appears, in pixels.
const STICKY_DROP_PX: f32 = 24.0;
/// Genres shown in the header's line under the title.
const HEADER_GENRES: usize = 3;

/// Radius that turns a button into a pill.
const PILL: Pixels = px(999.);

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

    /// The page header: back link, store, title or logo, the details line and
    /// region tags.
    pub(super) fn render_header(
        &mut self,
        game: &Arc<GameInfo>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ink = self.ink;
        // Store logos are drawn for dark backgrounds (often white lettering);
        // light themes show the title as text instead.
        let logo = game
            .art_logo
            .clone()
            .filter(|l| !l.is_empty() && cx.theme().mode.is_dark())
            .and_then(|url| art_img(&url, 720, true, cx));
        let heading = match logo {
            Some(logo) => div()
                .h(rems(6.))
                .max_w_full()
                .overflow_hidden()
                .child(
                    logo.image_cache(&self.image_cache)
                        .h(rems(6.))
                        .max_w(rems(28.))
                        .object_fit(ObjectFit::Contain),
                )
                .into_any_element(),
            None => div()
                .text_size(rems(2.75))
                .line_height(rems(3.1))
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
        let store = game
            .system
            .as_deref()
            .and_then(irori_engines::systems::get)
            .map_or(game.runner().store_name(), |s| s.name.as_str());
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
            .gap_3()
            .child(h_flex().pb_2().child(back_button(cx)))
            .child(super::caption(store, ink.muted))
            .child(heading)
            .children(meta.map(|m| div().text_color(ink.muted).line_clamp(2).child(m)))
            .children(tags)
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

    /// The portrait cover, the primary action under it at its width, progress
    /// or hints, the game's tools, the user's rating and the facts card.
    pub(super) fn render_cover_column(
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
        // Where the primary action ends, in the page's scrolled content: the
        // sticky bar shows once it has scrolled out of view.
        let measure = Rc::clone(&self.header_height);
        let scroll = self.scroll.clone();
        let marker = canvas(
            move |bounds, _, _| {
                measure.set(bounds.bottom() - scroll.bounds().top() - scroll.offset().y);
            },
            |_, _, _, _| {},
        )
        .w_full()
        .h(px(0.));
        let status = self.render_status_line(game, cx);
        let sandbox = self.render_sandbox(cx);
        let tools = self.render_tools(game, cx);
        let rating = self.render_rating(cx);
        let facts = self.render_facts(game, cx);
        // Everything in the column takes the cover's width; long action
        // labels end in an ellipsis.
        v_flex()
            .flex_shrink_0()
            .w(rems(width))
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
            .child(marker)
            .children(status)
            .children(sandbox)
            .child(tools)
            .child(rating)
            .children(facts)
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

    /// Score, store and how the game runs on Linux and the Steam Deck, as a
    /// small card under the rating; nothing when none of it is known.
    fn render_facts(&self, game: &GameInfo, cx: &Context<Self>) -> Option<AnyElement> {
        let ink = self.ink;
        let theme = cx.theme();
        let row = |label: SharedString, value: AnyElement| {
            h_flex()
                .w_full()
                .items_start()
                .gap_3()
                .py_1p5()
                .text_sm()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(ink.panel_muted)
                        .child(label),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .max_w(gpui_kit::relative(0.6))
                        .child(value),
                )
                .into_any_element()
        };
        let mut rows = Vec::new();
        let compat = self.compat.ready();
        let score = compat
            .and_then(|c| c.scores.first())
            .map(|s| (s.source.clone(), s.score))
            .or_else(|| game.rating.map(|r| ("GOG".to_string(), r)));
        if let Some((source, score)) = score {
            let tag = match score {
                75.. => Tag::success(),
                50..75 => Tag::warning(),
                _ => Tag::danger(),
            };
            rows.push(row(
                t("game.facts.score"),
                h_flex()
                    .gap_2()
                    .child(div().text_xs().text_color(ink.panel_muted).child(source))
                    .child(tag.xsmall().child(score.to_string()))
                    .into_any_element(),
            ));
        }
        let store = match self.id.runner {
            irori_core::Runner::Sideload => t("game.details.added_by_you"),
            runner => runner.store_name().into(),
        };
        rows.push(row(
            t("game.details.store"),
            div().truncate().child(store).into_any_element(),
        ));
        if let Some(c) = compat {
            let mut linux: Vec<AnyElement> = Vec::new();
            if let Some(tier) = &c.protondb_tier {
                let color = match tier.as_str() {
                    "platinum" | "gold" | "native" => theme.success,
                    "silver" | "bronze" => theme.warning,
                    "borked" => theme.danger,
                    _ => theme.muted_foreground,
                };
                linux.push(
                    h_flex()
                        .gap_1()
                        .child(div().size(px(7.)).rounded_full().bg(color))
                        .child(super::compat::tier_label(tier))
                        .into_any_element(),
                );
            }
            if !linux.is_empty() {
                rows.push(row(
                    t("game.facts.on_linux"),
                    v_flex()
                        .items_end()
                        .gap_0p5()
                        .children(linux)
                        .into_any_element(),
                ));
            }
            if let Some(ac) = &c.anticheat {
                let color = match ac.status {
                    AnticheatStatus::Supported | AnticheatStatus::Running => theme.success,
                    AnticheatStatus::Broken | AnticheatStatus::Denied => theme.danger,
                    _ => theme.warning,
                };
                let color = legible(color, &[ink.panel], irori_ui::style::TEXT_CONTRAST, cx);
                rows.push(row(
                    t("game.compat.anticheat"),
                    div()
                        .text_color(color)
                        .child(ac.status.label())
                        .into_any_element(),
                ));
            }
            if let Some(deck) = c.steam_deck.filter(|d| *d != SteamDeckStatus::Unknown) {
                rows.push(row(
                    "Steam Deck".into(),
                    div().child(deck.label()).into_any_element(),
                ));
            }
        }
        Some(
            v_flex()
                .w_full()
                .px_4()
                .py_1()
                .rounded(theme.radius_lg)
                .bg(ink.panel)
                .border_1()
                .border_color(theme.border)
                .children(rows)
                .into_any_element(),
        )
    }

    /// Favourite, settings, cloud saves and more actions, as icon buttons.
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
            .gap_2()
            .flex_wrap()
            .child(irori_ui::play_status::button(
                "game-play-status",
                &self.id,
                cx,
            ))
            .child(
                Button::new("game-favourite")
                    .outline()
                    .rounded(PILL)
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
                        .outline()
                        .rounded(PILL)
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
                        .outline()
                        .rounded(PILL)
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
                    .outline()
                    .rounded(PILL)
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

    /// Play as a pill in the theme's text colour, the inverse of the page, so
    /// the main action stands out over any art in light themes and dark.
    /// gpui-kit's own buttons only take translucent custom fills.
    fn render_play_pill(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (fill, ink) = (theme.foreground, theme.background);
        let label = t("game.action.play");
        div()
            .id("primary-action")
            .role(Role::Button)
            .aria_label(label.clone())
            .keyboard_focusable(cx)
            .flex_1()
            .min_w_0()
            .h(rems(2.75))
            .px_5()
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .rounded(PILL)
            .bg(fill)
            .text_color(ink)
            .font_semibold()
            .shadow_md()
            .cursor_pointer()
            .hover(|s| s.bg(fill.opacity(0.88)))
            .active(|s| s.bg(fill.opacity(0.76)))
            .child(Icon::new(IconName::Play).text_color(ink))
            .child(div().min_w_0().truncate().child(label))
            .on_click(cx.listener(|this, _, window, cx| this.run_primary(window, cx)))
            .into_any_element()
    }

    /// Play as a split button: launch options (when the store offers
    /// several) and one-off variants (verbose logs, no update check).
    fn render_play_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let play = self.render_play_pill(cx);
        let page = cx.entity();
        let options = if self.launch_options.len() > 1 {
            self.launch_options.clone()
        } else {
            Vec::new()
        };
        if options.is_empty() && !self.id.runner.manages_installs() {
            return play;
        }
        let current = self.selected_option.clone();
        let emulated = self.id.runner == irori_core::Runner::Emulation;
        let id = self.id.clone();
        let more = Button::new("play-with")
            .outline()
            .rounded(PILL)
            .large()
            .icon(IconName::ChevronDown)
            .accessibility_label(t("game.hero.play_with"))
            .tooltip(t("game.hero.play_with"))
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
            });
        h_flex()
            .w_full()
            .gap_2()
            .child(play)
            .child(more)
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
