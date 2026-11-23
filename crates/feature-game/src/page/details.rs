//! The details panel beside the page's content: who made the game, how much
//! it was played, where it is installed and how long it takes to beat, as a
//! table of label and value rows.

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
use irori_core::util::{format_bytes, unix_now};
use irori_core::{GameInfo, GameMode};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use super::GamePage;
use crate::detail_rows::DetailRow;
use crate::hooks::HowLongToBeat;
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

/// Chips shown before the rest collapse into a "+N" chip.
const MAX_CHIPS: usize = 3;

/// `values` as chips, the ones past [`MAX_CHIPS`] counted in a last "+N".
/// A chip never grows past the row: a long name (a developer's, at a large
/// text size) wraps inside it.
fn chips(values: Vec<String>) -> AnyElement {
    let (shown, more) = chip_split(values.len(), MAX_CHIPS);
    h_flex()
        .w_full()
        .min_w_0()
        .gap_1()
        .flex_wrap()
        .children(values.into_iter().take(shown).map(|v| {
            Tag::secondary()
                .xsmall()
                .max_w_full()
                .min_w_0()
                .child(div().min_w_0().child(v))
        }))
        .when_some(more, |r, n| {
            r.child(
                Tag::secondary()
                    .xsmall()
                    .child(t_args("game.details.more", &[("count", &n.to_string())])),
            )
        })
        .into_any_element()
}

/// How many of `count` chips to show when at most `max` fit, and how many the
/// "+N" chip stands for. A lone extra shows itself rather than "+1".
fn chip_split(count: usize, max: usize) -> (usize, Option<usize>) {
    if count <= max + 1 {
        (count, None)
    } else {
        (max, Some(count - max))
    }
}

/// Steam tags shown as chips before the rest wait behind "+N".
const TAG_CHIPS: usize = 6;

/// The Steam tags worth a chip: those that no genre or game mode the page
/// already shows names (in the interface language or as Steam spells the
/// mode in English).
pub(super) fn tag_chips(tags: &[String], genres: &[String], modes: &[GameMode]) -> Vec<String> {
    use irori_services::search::tag_key;
    let shown: std::collections::HashSet<String> = genres
        .iter()
        .map(|g| tag_key(g))
        .chain(
            modes
                .iter()
                .map(|m| tag_key(&irori_ui::game_mode::label(*m))),
        )
        .collect();
    tags.iter()
        .filter(|tag| !shown.contains(&tag_key(tag)))
        .filter(|tag| {
            GameMode::from_search(tag).is_none_or(|named| !named.iter().all(|m| modes.contains(m)))
        })
        .cloned()
        .collect()
}

/// The pages the Links row offers besides the store page and the user's
/// own, as name and address: SteamDB for a game with a Steam app, then the
/// pages the compatibility sources found.
pub(super) fn known_links(
    steam_app: Option<u32>,
    compat: Option<&crate::hooks::CompatibilityInfo>,
) -> Vec<(String, String)> {
    let mut links = Vec::new();
    if let Some(app) = steam_app {
        links.push(("SteamDB".into(), format!("https://steamdb.info/app/{app}/")));
    }
    let Some(info) = compat else {
        return links;
    };
    if let Some(url) = &info.pcgamingwiki_url {
        links.push(("PCGamingWiki".into(), url.clone()));
    }
    // A search page isn't the game's.
    if let Some(url) = info.protondb_url.as_ref().filter(|u| u.contains("/app/")) {
        links.push(("ProtonDB".into(), url.clone()));
    }
    if let Some(url) = info
        .scores
        .iter()
        .find(|s| s.source == "IGDB")
        .and_then(|s| s.url.clone())
    {
        links.push(("IGDB".into(), url));
    }
    if let Some(url) = info.how_long_to_beat.as_ref().and_then(|h| h.url.clone()) {
        links.push(("HowLongToBeat".into(), url));
    }
    links
}

/// The install's size as the store gives it, else as measured on disk
/// (marked as such), else nothing.
fn install_size_text(store_bytes: u64, store_text: &str, measured: Option<u64>) -> String {
    if store_bytes > 0 {
        return format_bytes(store_bytes);
    }
    if !store_text.trim().is_empty() {
        return store_text.to_string();
    }
    match measured.filter(|b| *b > 0) {
        Some(bytes) => t_args(
            "game.details.measured_size",
            &[("size", &format_bytes(bytes))],
        )
        .to_string(),
        None => String::new(),
    }
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
        let hidden = crate::detail_rows::hidden(cx);
        let shows = |row: DetailRow| row.shown_in(&hidden);
        let credits = self.credit_rows(game, muted, &shows);
        let tags = self.tag_rows(game, muted, &shows);
        let steam_tags = if shows(DetailRow::SteamTags) {
            self.steam_tag_rows(game, muted, cx)
        } else {
            Vec::new()
        };
        let play = self.play_rows(muted, &shows, cx);
        let install = self.install_rows(game, muted, &shows, cx);
        let compat = if shows(DetailRow::HowLongToBeat) {
            self.compat_detail_rows(muted, cx)
        } else {
            Vec::new()
        };
        let steam = if shows(DetailRow::Steam) {
            self.steam_rows(muted)
        } else {
            Vec::new()
        };
        let links = if shows(DetailRow::Links) {
            self.link_rows(game, muted, cx)
        } else {
            Vec::new()
        };
        let edit_rows = Button::new("details-edit-rows")
            .ghost()
            .xsmall()
            .icon(IconName::Settings2)
            .tooltip(t("game.details.edit_rows"))
            .on_click(|_, window, cx| crate::detail_rows::open_detail_rows_dialog(window, cx));
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
                h_flex()
                    .w_full()
                    .pb_2()
                    .gap_2()
                    .justify_between()
                    .child(
                        div()
                            .min_w_0()
                            .child(super::section_title(t("game.info.details"), self.ink.text)),
                    )
                    .child(edit_rows),
            )
            .children(group(credits, cx))
            .children(group(tags, cx))
            .children(group(steam_tags, cx))
            .children(group(play, cx))
            .children(group(install, cx))
            .children(group(compat, cx))
            .children(group(steam, cx))
            .children(group(links, cx))
            .into_any_element()
    }

    /// The Steam app a game of another store is linked to, with "Link to
    /// Steam" to change it.
    fn steam_rows(&self, muted: Hsla) -> Vec<AnyElement> {
        if matches!(
            self.id.runner,
            irori_core::Runner::Steam | irori_core::Runner::Emulation
        ) {
            return Vec::new();
        }
        let link = self.steam_link;
        let value: AnyElement = match link.app_id() {
            Some(app) => {
                let label = t_args("game.steam_link.app", &[("id", &app.to_string())]);
                match irori_ui::web_url(&format!("https://store.steampowered.com/app/{app}/")) {
                    Some(url) => gpui_kit::component::link::Link::new("details-steam-app")
                        .href(url)
                        .child(label)
                        .into_any_element(),
                    None => text(label),
                }
            }
            None if link == irori_services::SteamLink::NoneChosen => {
                text(t("game.steam_link.none"))
            }
            None => text(t("game.steam_link.not_linked")),
        };
        let id = self.id.clone();
        let change = Button::new("details-steam-link")
            .ghost()
            .xsmall()
            .label(if link.app_id().is_some() {
                t("game.steam_link.change")
            } else {
                t("game.steam_link.link")
            })
            .tooltip(crate::steam_match::describe(link))
            .on_click(move |_, window, cx| {
                crate::steam_match::open_steam_link_dialog(id.clone(), window, cx)
            });
        vec![row(
            "Steam",
            h_flex()
                .w_full()
                .gap_2()
                .justify_between()
                .child(div().min_w_0().truncate().child(value))
                .child(change),
            muted,
        )]
    }

    /// Developer, publisher and release date.
    fn credit_rows(
        &self,
        game: &GameInfo,
        muted: Hsla,
        shows: &dyn Fn(DetailRow) -> bool,
    ) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        if let Some(d) = game
            .developer
            .as_ref()
            .filter(|d| !d.is_empty() && shows(DetailRow::Developer))
        {
            rows.push(row(
                t("game.details.developer"),
                chips(vec![d.clone()]),
                muted,
            ));
        }
        if let Some(p) = game.publisher.as_ref().filter(|p| {
            !p.is_empty() && game.developer.as_ref() != Some(p) && shows(DetailRow::Publisher)
        }) {
            rows.push(row(
                t("game.details.publisher"),
                chips(vec![p.clone()]),
                muted,
            ));
        }
        if let Some(date) = self
            .release_date(game)
            .filter(|_| shows(DetailRow::Released))
        {
            rows.push(row(t("game.details.released"), text(date), muted));
        }
        rows
    }

    /// Genres, the platforms the game is offered for, and a ROM's region and
    /// revision, as chips.
    fn tag_rows(
        &self,
        game: &GameInfo,
        muted: Hsla,
        shows: &dyn Fn(DetailRow) -> bool,
    ) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        let genres = self
            .extra
            .ready()
            .map(|e| e.genres.clone())
            .filter(|g| !g.is_empty())
            .unwrap_or_else(|| game.genres.clone());
        if !genres.is_empty() && shows(DetailRow::Genres) {
            rows.push(row(t("game.details.genres"), chips(genres), muted));
        }
        if self.id.runner != irori_core::Runner::Emulation && shows(DetailRow::Platforms) {
            let platforms: Vec<String> = crate::install::installable_platforms(game)
                .iter()
                .map(|p| platform_label(*p).to_string())
                .collect();
            if !platforms.is_empty() {
                rows.push(row(t("game.info.platforms"), chips(platforms), muted));
            }
        }
        if !game.tags.is_empty() && shows(DetailRow::Tags) {
            rows.push(row(t("game.details.tags"), chips(game.tags.clone()), muted));
        }
        rows
    }

    /// The game's Steam tags as chips, each showing the library's games with
    /// that tag; the strongest first, the rest behind "+N".
    fn steam_tag_rows(&self, game: &GameInfo, muted: Hsla, cx: &Context<Self>) -> Vec<AnyElement> {
        let genres = self
            .extra
            .ready()
            .map(|e| e.genres.clone())
            .filter(|g| !g.is_empty())
            .unwrap_or_else(|| game.genres.clone());
        let tags = tag_chips(&game.store_tags, &genres, &game.game_modes);
        if tags.is_empty() {
            return Vec::new();
        }
        let (shown, more) = if self.tags_expanded {
            (tags.len(), None)
        } else {
            chip_split(tags.len(), TAG_CHIPS)
        };
        let chips = tags.into_iter().take(shown).enumerate().map(|(i, tag)| {
            let label: SharedString = tag.into();
            let event = label.clone();
            Button::new(("steam-tag", i))
                .xsmall()
                .outline()
                .label(label.clone())
                .tooltip(t_args("game.details.show_tag", &[("tag", &label)]))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(super::GamePageEvent::ShowTag(event.clone()))
                }))
        });
        let toggle = match more {
            Some(n) => Some(
                Button::new("steam-tags-more")
                    .xsmall()
                    .ghost()
                    .label(t_args("game.details.more", &[("count", &n.to_string())])),
            ),
            None if self.tags_expanded => Some(
                Button::new("steam-tags-more")
                    .xsmall()
                    .ghost()
                    .label(t("game.details.fewer")),
            ),
            None => None,
        }
        .map(|b| {
            b.on_click(cx.listener(|this, _, _, cx| {
                this.tags_expanded = !this.tags_expanded;
                cx.notify();
            }))
        });
        vec![row(
            t("game.details.steam_tags"),
            h_flex()
                .w_full()
                .min_w_0()
                .gap_1()
                .flex_wrap()
                .children(chips)
                .children(toggle),
            muted,
        )]
    }

    /// The Links row: the store page, the pages sources know of and the
    /// user's own links, each a button opening it, with "Edit links…".
    fn link_rows(&self, game: &GameInfo, muted: Hsla, cx: &Context<Self>) -> Vec<AnyElement> {
        let store_url = game
            .store_url
            .clone()
            .or_else(|| self.extra.ready().and_then(|e| e.store_url.clone()))
            .filter(|u| !u.is_empty());
        let mut buttons: Vec<AnyElement> = Vec::new();
        if let Some(url) = store_url {
            let runner = self.id.runner;
            buttons.push(
                Button::new("link-store-page")
                    .xsmall()
                    .ghost()
                    .icon(IconName::Store)
                    .label(t("game.menu.store_page"))
                    .on_click(move |_, window, cx| {
                        irori_ui::hooks::open_store_page(runner, url.clone(), window, cx)
                    })
                    .into_any_element(),
            );
        }
        let own = crate::links::of(&self.id, cx);
        let pages = known_links(self.steam_link.app_id(), self.compat.ready())
            .into_iter()
            .chain(own.iter().map(|l| (l.label(), l.url.clone())));
        for (i, (name, url)) in pages.enumerate() {
            let Some(url) = irori_ui::web_url(&url) else {
                continue;
            };
            buttons.push(
                Button::new(("game-link", i))
                    .xsmall()
                    .ghost()
                    .icon(IconName::ExternalLink)
                    .label(name)
                    .tooltip(url.to_string())
                    .on_click(move |_, _, cx| cx.open_url(&url))
                    .into_any_element(),
            );
        }
        let id = self.id.clone();
        buttons.push(
            Button::new("game-links-edit")
                .xsmall()
                .ghost()
                .icon(IconName::Pencil)
                .tooltip(t("game.links.edit"))
                .on_click(move |_, window, cx| {
                    crate::links::open_links_dialog(id.clone(), window, cx)
                })
                .into_any_element(),
        );
        vec![row(
            t("game.links.title"),
            h_flex()
                .w_full()
                .min_w_0()
                .gap_1()
                .flex_wrap()
                .children(buttons),
            muted,
        )]
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
    fn play_rows(
        &self,
        muted: Hsla,
        shows: &dyn Fn(DetailRow) -> bool,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
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
        let mut rows = Vec::new();
        if shows(DetailRow::Playtime) {
            rows.push(row(
                t("game.info.playtime"),
                text(playtime_label(minutes, played)),
                muted,
            ));
        }
        if let Some(last) = record
            .as_ref()
            .and_then(|r| r.last_played.as_deref())
            .filter(|_| shows(DetailRow::LastPlayed))
        {
            rows.push(row(
                t("game.info.last_played"),
                text(format_day(last, now)),
                muted,
            ));
        }
        if let Some(first) = record
            .as_ref()
            .and_then(|r| r.first_played.as_deref())
            .filter(|_| shows(DetailRow::FirstPlayed))
        {
            rows.push(row(
                t("game.info.first_played"),
                text(format_day(first, now)),
                muted,
            ));
        }
        if let Some(list) = self
            .achievements
            .ready()
            .filter(|a| !a.is_empty() && shows(DetailRow::Achievements))
        {
            let unlocked = list.iter().filter(|a| a.is_unlocked()).count();
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
            && shows(DetailRow::Account)
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
    fn install_rows(
        &self,
        game: &GameInfo,
        muted: Hsla,
        shows: &dyn Fn(DetailRow) -> bool,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let mut rows = Vec::new();
        // Steam picks the build and Proton, syncs saves and decides about
        // offline play for its games; Irori only knows where they are.
        let managed = self.id.runner.manages_installs();
        let b = backend(cx);
        match &game.install {
            Some(install) if game.is_installed => {
                if let Some(path) = game.install_path().filter(|_| shows(DetailRow::Location)) {
                    let size = install_size_text(
                        install.install_size_bytes,
                        &install.install_size,
                        self.measured_size,
                    );
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
                if managed && shows(DetailRow::Platform) {
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
                if !version.trim().is_empty() && shows(DetailRow::Version) {
                    rows.push(row(t("game.common.version"), text(version), muted));
                }
                if let Some(language) = install
                    .language
                    .as_ref()
                    .filter(|_| shows(DetailRow::Language))
                {
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
                    if shows(DetailRow::Wine) {
                        rows.push(row("Wine", text(wine), muted));
                    }
                    if shows(DetailRow::Prefix) {
                        let prefix = irori_runtime::prefix_path(&b.settings.app(), &settings, game);
                        rows.push(row(
                            t("game.common.prefix"),
                            text(prefix.display().to_string()),
                            muted,
                        ));
                    }
                }
            }
            // The platforms on offer are among the game's tags.
            _ => {}
        }
        if !managed {
            if shows(DetailRow::Launcher) {
                rows.push(row(
                    t("game.info.launcher"),
                    text(self.id.runner.store_name()),
                    muted,
                ));
            }
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
        if shows(DetailRow::CloudSaves) {
            rows.push(row(t("game.info.cloud_saves"), text(saves), muted));
        }
        if shows(DetailRow::Offline) {
            rows.push(row(
                t("game.info.offline_play"),
                text(if game.can_run_offline {
                    t("game.info.offline_supported")
                } else {
                    t("game.info.offline_needs_connection")
                }),
                muted,
            ));
        }
        if let Some(app) = game
            .third_party_managed_app
            .as_ref()
            .filter(|_| shows(DetailRow::Launcher))
        {
            rows.push(row(t("game.info.launcher"), text(app.clone()), muted));
        }
        rows
    }

    /// HowLongToBeat, as a bar marking the user's own play time, with
    /// "Change match"; without times, a way to find them.
    fn compat_detail_rows(&self, muted: Hsla, cx: &Context<Self>) -> Vec<AnyElement> {
        let Some(info) = self.compat.ready() else {
            return Vec::new();
        };
        let page = cx.entity().downgrade();
        let id = self.id.clone();
        let title = models(cx)
            .library
            .read(cx)
            .snapshot
            .get(&self.id)
            .map(|g| g.title.clone())
            .unwrap_or_else(|| self.id.app_name.to_string());
        let change = move |label: &'static str| {
            let (page, id, title) = (page.clone(), id.clone(), title.clone());
            Button::new("details-hltb-match")
                .ghost()
                .xsmall()
                .label(t(label))
                .on_click(move |_, window, cx| {
                    let page = page.clone();
                    let on_chosen: crate::hltb_match::OnChosen =
                        std::rc::Rc::new(move |window, cx| {
                            page.update(cx, |this, cx| this.reload_compat(window, cx))
                                .ok();
                        });
                    crate::hltb_match::open_hltb_match_dialog(
                        id.clone(),
                        title.clone(),
                        on_chosen,
                        window,
                        cx,
                    );
                })
        };
        let mut rows = Vec::new();
        match info
            .how_long_to_beat
            .as_ref()
            .filter(|h| h.main.or(h.main_extra).or(h.completionist).is_some())
        {
            Some(h) => {
                let minutes = models(cx).library.read(cx).playtime_minutes(&self.id);
                rows.push(
                    v_flex()
                        .w_full()
                        .gap_2()
                        .py_1p5()
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .gap_2()
                                .child(div().text_sm().text_color(muted).child("HowLongToBeat"))
                                .child(change("game.hltb.change_match")),
                        )
                        .child(hltb_bar(h, minutes as f32 / 60., muted, cx))
                        .into_any_element(),
                );
            }
            None => rows.push(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .py_1p5()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .text_color(muted)
                            .child(t("game.hltb.none_found")),
                    )
                    .child(change("game.hltb.find"))
                    .into_any_element(),
            ),
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

#[cfg(test)]
mod tests {
    use super::chip_split;
    use irori_core::GameMode;

    #[test]
    fn tag_chips_leave_out_what_genres_and_modes_say() {
        crate::i18n::register();
        let tags: Vec<String> = [
            "Roguelike",
            "Action",
            "Co-op",
            "Singleplayer",
            "Great Soundtrack",
        ]
        .map(String::from)
        .to_vec();
        let genres = vec!["action".to_string()];
        assert_eq!(
            super::tag_chips(&tags, &genres, &[GameMode::Single, GameMode::Coop]),
            ["Roguelike", "Great Soundtrack"]
        );
        // A mode the page doesn't show keeps its tag.
        assert_eq!(
            super::tag_chips(&tags, &[], &[GameMode::Single]),
            ["Roguelike", "Action", "Co-op", "Great Soundtrack"]
        );
    }

    #[test]
    fn links_name_the_pages_sources_found() {
        use crate::hooks::{CompatibilityInfo, ReviewScore};
        assert!(super::known_links(None, None).is_empty());
        let info = CompatibilityInfo {
            pcgamingwiki_url: Some("https://www.pcgamingwiki.com/wiki/?curid=1".into()),
            protondb_url: Some("https://www.protondb.com/search?q=Hades".into()),
            scores: vec![ReviewScore::new(
                "IGDB",
                90,
                Some("https://www.igdb.com/games/hades".into()),
            )],
            ..Default::default()
        };
        let names: Vec<String> = super::known_links(Some(1145360), Some(&info))
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            names,
            ["SteamDB", "PCGamingWiki", "IGDB"],
            "not a search page"
        );
    }

    #[test]
    fn the_measured_size_stands_in_for_the_stores() {
        crate::i18n::register();
        use irori_core::util::format_bytes;
        let gib = 3 * 1024 * 1024 * 1024;
        assert_eq!(
            super::install_size_text(gib, "", Some(1)),
            format_bytes(gib)
        );
        assert_eq!(super::install_size_text(0, "2 GB", Some(gib)), "2 GB");
        let measured = super::install_size_text(0, " ", Some(gib));
        assert!(measured.contains(&format_bytes(gib)), "{measured}");
        assert_eq!(super::install_size_text(0, "", None), "");
        assert_eq!(super::install_size_text(0, "", Some(0)), "");
    }

    #[test]
    fn chips_past_the_limit_collapse() {
        assert_eq!(chip_split(2, 3), (2, None));
        assert_eq!(chip_split(4, 3), (4, None));
        assert_eq!(chip_split(12, 3), (3, Some(9)));
    }
}
