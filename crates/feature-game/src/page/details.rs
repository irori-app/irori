//! The details panel beside the page's content: who made the game, how much
//! it was played, where it is installed, how it runs on Linux and how long it
//! takes to beat, as a table of label and value rows.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Hsla, InteractiveElement as _, IntoElement, ParentElement,
    SharedString, Styled, div, px, relative, rems,
};
use irori_core::GameInfo;
use irori_core::util::{format_bytes, unix_now};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use super::GamePage;
use crate::hooks::{AnticheatStatus, HowLongToBeat, SteamDeckStatus};
use crate::i18n::{platform_label, playtime_label};
use crate::load::{calendar_date, format_day, parse_date};
use crate::ops;
use crate::saves::CloudSupport;
use crate::state::completion_percent;

/// Share of a row's width its label takes; labels wrap within it.
const LABEL_SHARE: f32 = 0.38;

/// A label and value row of the table.
fn row(label: impl Into<SharedString>, value: impl IntoElement, muted: Hsla) -> AnyElement {
    h_flex()
        .w_full()
        .items_start()
        .gap_3()
        .py_1p5()
        .text_sm()
        .child(
            div()
                .w(relative(LABEL_SHARE))
                .flex_shrink_0()
                .text_color(muted)
                .child(label.into()),
        )
        .child(div().flex_1().min_w_0().child(value))
        .into_any_element()
}

/// A plain text value, cut short when it doesn't fit.
fn text(value: impl Into<SharedString>) -> AnyElement {
    div().truncate().child(value.into()).into_any_element()
}

/// Rows separated into groups by a hairline.
fn group(rows: Vec<AnyElement>, cx: &App) -> Option<AnyElement> {
    (!rows.is_empty()).then(|| {
        v_flex()
            .w_full()
            .py_1()
            .border_t_1()
            .border_color(cx.theme().border)
            .children(rows)
            .into_any_element()
    })
}

impl GamePage {
    /// The details panel.
    pub(super) fn render_details_panel(
        &self,
        game: &Arc<GameInfo>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = self.ink.panel_muted;
        let credits = self.credit_rows(game, muted);
        let play = self.play_rows(muted, cx);
        let install = self.install_rows(game, muted, cx);
        let compat = self.compat_detail_rows(muted, cx);
        let theme = cx.theme();
        v_flex()
            .id("game-details")
            .w_full()
            .px_4()
            .pt_3()
            .pb_2()
            .rounded(theme.radius_lg)
            .bg(self.ink.panel)
            .border_1()
            .border_color(theme.border)
            .child(
                div()
                    .pb_2()
                    .child(irori_ui::style::caption(t("game.info.details"), cx)),
            )
            .children(group(credits, cx))
            .children(group(play, cx))
            .children(group(install, cx))
            .children(group(compat, cx))
            .into_any_element()
    }

    /// Developer, publisher, release date and store.
    fn credit_rows(&self, game: &GameInfo, muted: Hsla) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        if let Some(d) = game.developer.as_ref().filter(|d| !d.is_empty()) {
            rows.push(row(t("game.details.developer"), text(d.clone()), muted));
        }
        if let Some(p) = game
            .publisher
            .as_ref()
            .filter(|p| !p.is_empty() && game.developer.as_ref() != Some(p))
        {
            rows.push(row(t("game.details.publisher"), text(p.clone()), muted));
        }
        if let Some(date) = self.release_date(game) {
            rows.push(row(t("game.details.released"), text(date), muted));
        }
        let store = match self.id.runner {
            irori_core::Runner::Sideload => t("game.details.added_by_you"),
            runner => runner.store_name().into(),
        };
        rows.push(row(t("game.details.store"), text(store), muted));
        rows
    }

    /// The release date in the interface language, or as the store wrote it
    /// when it isn't a date.
    pub(super) fn release_date(&self, game: &GameInfo) -> Option<String> {
        let raw = self
            .extra
            .ready()
            .and_then(|e| e.release_date.clone())
            .or_else(|| game.release_date.clone())?;
        match parse_date(&raw) {
            Some(date) => Some(calendar_date(date, true)),
            None => Some(raw.trim().to_string()).filter(|d| d.len() >= 4),
        }
    }

    /// Play time, first and last played, achievements and the account.
    fn play_rows(&self, muted: Hsla, cx: &Context<Self>) -> Vec<AnyElement> {
        let record = models(cx)
            .library
            .read(cx)
            .user
            .playtime
            .get(&self.id.key())
            .cloned();
        let now = unix_now() as i64;
        let minutes = record.as_ref().map_or(0, |r| r.total_minutes);
        let played = record.as_ref().is_some_and(|r| r.last_played.is_some());
        let mut rows = vec![row(
            t("game.info.playtime"),
            text(playtime_label(minutes, played)),
            muted,
        )];
        if let Some(last) = record.as_ref().and_then(|r| r.last_played.as_deref()) {
            rows.push(row(
                t("game.info.last_played"),
                text(format_day(last, now)),
                muted,
            ));
        }
        if let Some(first) = record.as_ref().and_then(|r| r.first_played.as_deref()) {
            rows.push(row(
                t("game.info.first_played"),
                text(format_day(first, now)),
                muted,
            ));
        }
        if let Some(list) = self.achievements.ready().filter(|a| !a.is_empty()) {
            let unlocked = list.iter().filter(|a| a.unlocked_at.is_some()).count();
            let percent = completion_percent(unlocked, list.len());
            rows.push(row(
                t("game.header.achievements"),
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_shrink_0()
                            .child(format!("{unlocked} / {}", list.len())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(rems(3.))
                            .child(Progress::new("details-achievements").value(percent as f32)),
                    ),
                muted,
            ));
        }
        if self.id.runner.manages_installs()
            && let Some(account) = backend(cx)
                .store(self.id.runner)
                .and_then(|s| s.game_account(&self.id.app_name))
        {
            rows.push(row(
                t("game.info.account"),
                text(account.display_name),
                muted,
            ));
        }
        rows
    }

    /// Folder and size, version, language, Wine, saves and offline play.
    fn install_rows(&self, game: &GameInfo, muted: Hsla, cx: &Context<Self>) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        // Steam picks the build and Proton, syncs saves and decides about
        // offline play for its games; Irori only knows where they are.
        let managed = self.id.runner.manages_installs();
        let b = backend(cx);
        match &game.install {
            Some(install) if game.is_installed => {
                if let Some(path) = game.install_path() {
                    let size = if install.install_size_bytes > 0 {
                        format_bytes(install.install_size_bytes)
                    } else {
                        install.install_size.clone()
                    };
                    let id = self.id.clone();
                    rows.push(row(
                        t("game.common.location"),
                        v_flex()
                            .items_start()
                            .gap_0p5()
                            .child(
                                Button::new("open-install-path")
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::FolderOpen)
                                    .label(path.display().to_string())
                                    .tooltip(t("game.common.open_folder"))
                                    .max_w_full()
                                    .on_click(move |_, window, cx| {
                                        ops::open_install_folder(&id, window, cx)
                                    }),
                            )
                            .when(!size.is_empty(), |c| {
                                c.child(div().text_xs().text_color(muted).child(size))
                            }),
                        muted,
                    ));
                }
                if managed {
                    rows.push(row(
                        t("game.common.platform"),
                        text(platform_label(install.platform)),
                        muted,
                    ));
                }
                let mut version = install.version.clone();
                if let Some(branch) = install.branch.as_ref().filter(|b| !b.is_empty()) {
                    version = format!("{version} ({branch})");
                }
                if install.pinned_version {
                    version =
                        t_args("game.info.version_pinned", &[("version", &version)]).to_string();
                }
                if !version.trim().is_empty() {
                    rows.push(row(t("game.common.version"), text(version), muted));
                }
                if let Some(language) = &install.language {
                    rows.push(row(
                        t("game.common.language"),
                        text(crate::load::language_name(language)),
                        muted,
                    ));
                }
                if install.platform.needs_wine() {
                    let settings = b.settings.game(&self.id);
                    let wine = if settings.wine_version.is_set() {
                        settings.wine_version.name.clone()
                    } else {
                        t("game.info.not_set").to_string()
                    };
                    rows.push(row("Wine", text(wine), muted));
                    let prefix = irori_runtime::prefix_path(&b.settings.app(), &settings, game);
                    rows.push(row(
                        t("game.common.prefix"),
                        text(prefix.display().to_string()),
                        muted,
                    ));
                }
            }
            _ => {
                let platforms = crate::install::installable_platforms(game)
                    .iter()
                    .map(|p| platform_label(*p).to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                if !platforms.is_empty() {
                    rows.push(row(t("game.info.platforms"), text(platforms), muted));
                }
            }
        }
        if !managed {
            rows.push(row(
                t("game.info.launcher"),
                text(self.id.runner.store_name()),
                muted,
            ));
            return rows;
        }
        let store_supports = b
            .store(self.id.runner)
            .is_some_and(|s| s.supports_cloud_saves());
        let saves = match CloudSupport::of(game, store_supports) {
            CloudSupport::Supported => {
                if b.settings.game(&self.id).auto_sync_saves {
                    t("game.info.saves_auto")
                } else {
                    t("game.info.saves_manual")
                }
            }
            CloudSupport::NotInstalled => t("game.info.saves_available"),
            CloudSupport::NativeBuild => t("game.info.saves_windows_only"),
            CloudSupport::StoreUnsupported | CloudSupport::GameUnsupported => {
                t("game.info.saves_not_supported")
            }
        };
        rows.push(row(t("game.info.cloud_saves"), text(saves), muted));
        rows.push(row(
            t("game.info.offline_play"),
            text(if game.can_run_offline {
                t("game.info.offline_supported")
            } else {
                t("game.info.offline_needs_connection")
            }),
            muted,
        ));
        if let Some(app) = &game.third_party_managed_app {
            rows.push(row(t("game.info.launcher"), text(app.clone()), muted));
        }
        rows
    }

    /// ProtonDB, Steam Deck and anticheat as badges, and HowLongToBeat.
    fn compat_detail_rows(&self, muted: Hsla, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(info) = self.compat.ready() else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        let mut badges: Vec<AnyElement> = Vec::new();
        if let Some(tier) = &info.protondb_tier {
            let tag = match tier.as_str() {
                "platinum" | "gold" | "native" => Tag::success(),
                "silver" | "bronze" => Tag::warning(),
                "borked" => Tag::danger(),
                _ => Tag::secondary(),
            };
            badges.push(
                tag.small()
                    .child(t_args(
                        "game.header.protondb",
                        &[("tier", &super::compat::tier_label(tier))],
                    ))
                    .into_any_element(),
            );
        }
        if let Some(deck) = info.steam_deck.filter(|d| *d != SteamDeckStatus::Unknown) {
            let tag = match deck {
                SteamDeckStatus::Verified => Tag::success(),
                SteamDeckStatus::Playable => Tag::warning(),
                _ => Tag::danger(),
            };
            badges.push(
                tag.small()
                    .child(t_args("game.header.deck", &[("status", &deck.label())]))
                    .into_any_element(),
            );
        }
        if let Some(ac) = &info.anticheat {
            let tag = match ac.status {
                AnticheatStatus::Supported | AnticheatStatus::Running => Tag::success(),
                AnticheatStatus::Broken | AnticheatStatus::Denied => Tag::danger(),
                _ => Tag::warning(),
            };
            badges.push(
                tag.small()
                    .child(t_args(
                        "game.details.anticheat",
                        &[("status", &ac.status.label())],
                    ))
                    .into_any_element(),
            );
        }
        if !badges.is_empty() {
            rows.push(row(
                t("game.compat.title"),
                h_flex().gap_1().flex_wrap().children(badges),
                muted,
            ));
        }
        if let Some(h) = info
            .how_long_to_beat
            .as_ref()
            .filter(|h| h.main.or(h.main_extra).or(h.completionist).is_some())
        {
            let minutes = models(cx).library.read(cx).playtime_minutes(&self.id);
            rows.push(
                v_flex()
                    .w_full()
                    .gap_2()
                    .py_1p5()
                    .child(div().text_sm().text_color(muted).child("HowLongToBeat"))
                    .child(hltb_bar(h, minutes as f32 / 60., muted, cx))
                    .into_any_element(),
            );
        }
        rows
    }
}

/// Main story, main + extras and completionist as one bar in three segments,
/// with a mark where the user's own play time reaches.
fn hltb_bar(h: &HowLongToBeat, played_hours: f32, muted: Hsla, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let values = [h.main, h.main_extra, h.completionist].map(|v| v.unwrap_or(0.).max(0.));
    let total = values.iter().copied().fold(0., f32::max).max(0.1);
    // Each segment covers what the previous one doesn't.
    let mut start = 0.;
    let colors = [theme.chart_1, theme.chart_2, theme.chart_3];
    let mut bar = div()
        .relative()
        .w_full()
        .h(px(8.))
        .rounded_full()
        .overflow_hidden()
        .bg(theme.muted);
    for (value, color) in values.iter().zip(colors) {
        if *value > start {
            bar = bar.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(relative(start / total))
                    .w(relative((value - start) / total))
                    .bg(color),
            );
            start = *value;
        }
    }
    let marker = (played_hours > 0.).then(|| {
        div()
            .absolute()
            .top(px(-3.))
            .h(px(14.))
            .w(px(2.))
            .left(relative((played_hours / total).min(1.)))
            .bg(theme.foreground)
    });
    let labels = [
        "game.details.hltb_main",
        "game.details.hltb_extra",
        "game.details.hltb_complete",
    ];
    let legend = labels
        .iter()
        .zip(values)
        .zip(colors)
        .filter(|((_, v), _)| *v > 0.)
        .map(|((key, value), color)| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    h_flex()
                        .gap_1()
                        .min_w_0()
                        .child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(color))
                        .child(div().truncate().text_xs().text_color(muted).child(t(key))),
                )
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(super::compat::hours(Some(value))),
                )
        });
    let link = h.url.as_deref().and_then(irori_ui::web_url).map(|url| {
        gpui_kit::component::link::Link::new("details-hltb")
            .href(url)
            .text_xs()
            .child(t("game.compat.open_page"))
    });
    v_flex()
        .w_full()
        .gap_2()
        .child(div().relative().w_full().child(bar).children(marker))
        .child(h_flex().w_full().gap_3().items_start().children(legend))
        .when(played_hours > 0., |c| {
            c.child(
                h_flex()
                    .gap_1()
                    .text_xs()
                    .text_color(muted)
                    .child(div().w(px(2.)).h(px(10.)).bg(theme.foreground))
                    .child(t_args(
                        "game.details.hltb_you",
                        &[(
                            "duration",
                            &playtime_label((played_hours * 60.) as u64, true),
                        )],
                    )),
            )
        })
        .children(link)
        .into_any_element()
}
