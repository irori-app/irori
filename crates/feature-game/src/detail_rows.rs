//! Which rows the game page's Details show ("Edit rows…"): every kind of
//! row can be left out, for every game at once. The choice is the
//! `iroriUi.hiddenDetailRows` list of row ids; an id a later Irori writes
//! stays in it untouched.

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, v_flex};
use gpui_kit::{
    App, AppContext as _, Context, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div,
};
use irori_ui::i18n::t;
use irori_ui::{backend, models};

/// A kind of row in the Details panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetailRow {
    Developer,
    Publisher,
    Released,
    Genres,
    Platforms,
    Tags,
    SteamTags,
    Playtime,
    LastPlayed,
    FirstPlayed,
    Achievements,
    Account,
    Location,
    Platform,
    Version,
    Language,
    Wine,
    Prefix,
    CloudSaves,
    Offline,
    Launcher,
    HowLongToBeat,
    Steam,
    SteamReception,
    Links,
}

impl DetailRow {
    /// Every kind, in the panel's order.
    pub(crate) const ALL: [DetailRow; 25] = [
        DetailRow::Developer,
        DetailRow::Publisher,
        DetailRow::Released,
        DetailRow::Genres,
        DetailRow::Platforms,
        DetailRow::Tags,
        DetailRow::SteamTags,
        DetailRow::Playtime,
        DetailRow::LastPlayed,
        DetailRow::FirstPlayed,
        DetailRow::Achievements,
        DetailRow::Account,
        DetailRow::Location,
        DetailRow::Platform,
        DetailRow::Version,
        DetailRow::Language,
        DetailRow::Wine,
        DetailRow::Prefix,
        DetailRow::CloudSaves,
        DetailRow::Offline,
        DetailRow::Launcher,
        DetailRow::HowLongToBeat,
        DetailRow::Steam,
        DetailRow::SteamReception,
        DetailRow::Links,
    ];

    /// The id kept in `hiddenDetailRows`. Never change one: it is user data.
    pub(crate) fn id(self) -> &'static str {
        match self {
            DetailRow::Developer => "developer",
            DetailRow::Publisher => "publisher",
            DetailRow::Released => "released",
            DetailRow::Genres => "genres",
            DetailRow::Platforms => "platforms",
            DetailRow::Tags => "tags",
            DetailRow::SteamTags => "steam_tags",
            DetailRow::Playtime => "playtime",
            DetailRow::LastPlayed => "last_played",
            DetailRow::FirstPlayed => "first_played",
            DetailRow::Achievements => "achievements",
            DetailRow::Account => "account",
            DetailRow::Location => "location",
            DetailRow::Platform => "platform",
            DetailRow::Version => "version",
            DetailRow::Language => "language",
            DetailRow::Wine => "wine",
            DetailRow::Prefix => "prefix",
            DetailRow::CloudSaves => "cloud_saves",
            DetailRow::Offline => "offline",
            DetailRow::Launcher => "launcher",
            DetailRow::HowLongToBeat => "hltb",
            DetailRow::Steam => "steam",
            DetailRow::SteamReception => "steam_reception",
            DetailRow::Links => "links",
        }
    }

    /// The row's label, as the panel shows it.
    pub(crate) fn label(self) -> SharedString {
        match self {
            DetailRow::Developer => t("game.details.developer"),
            DetailRow::Publisher => t("game.details.publisher"),
            DetailRow::Released => t("game.details.released"),
            DetailRow::Genres => t("game.details.genres"),
            DetailRow::Platforms => t("game.info.platforms"),
            DetailRow::Tags => t("game.details.tags"),
            DetailRow::SteamTags => t("game.details.steam_tags"),
            DetailRow::Playtime => t("game.info.playtime"),
            DetailRow::LastPlayed => t("game.info.last_played"),
            DetailRow::FirstPlayed => t("game.info.first_played"),
            DetailRow::Achievements => t("game.header.achievements"),
            DetailRow::Account => t("game.info.account"),
            DetailRow::Location => t("game.common.location"),
            DetailRow::Platform => t("game.common.platform"),
            DetailRow::Version => t("game.common.version"),
            DetailRow::Language => t("game.common.language"),
            DetailRow::Wine => "Wine".into(),
            DetailRow::Prefix => t("game.common.prefix"),
            DetailRow::CloudSaves => t("game.info.cloud_saves"),
            DetailRow::Offline => t("game.info.offline_play"),
            DetailRow::Launcher => t("game.info.launcher"),
            DetailRow::HowLongToBeat => "HowLongToBeat".into(),
            DetailRow::Steam => "Steam".into(),
            DetailRow::SteamReception => t("game.reception.title"),
            DetailRow::Links => t("game.links.title"),
        }
    }

    /// Whether the panel shows this kind, by the hidden ids.
    pub(crate) fn shown_in(self, hidden: &[String]) -> bool {
        !hidden.iter().any(|h| h == self.id())
    }
}

/// `hidden` with `row` left out (`shown` false) or shown again; other ids,
/// a later Irori's among them, stay as they were.
fn toggled(hidden: &[String], row: DetailRow, shown: bool) -> Vec<String> {
    let mut out: Vec<String> = hidden
        .iter()
        .filter(|h| h.as_str() != row.id())
        .cloned()
        .collect();
    if !shown {
        out.push(row.id().to_string());
    }
    out
}

/// The hidden ids the settings hold now.
pub(crate) fn hidden(cx: &App) -> Vec<String> {
    models(cx)
        .settings
        .read(cx)
        .app
        .ui
        .hidden_detail_rows
        .clone()
}

/// Open "Edit rows…".
pub fn open_detail_rows_dialog(window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let dialog = cx.new(|cx| DetailRowsDialog { hidden: hidden(cx) });
    let width = window.rem_size() * 24.;
    window.open_dialog(cx, move |d, _, _| {
        d.title(t("game.details.edit_rows_title"))
            .width(width)
            .child(dialog.clone())
    });
}

struct DetailRowsDialog {
    hidden: Vec<String>,
}

impl DetailRowsDialog {
    fn set(&mut self, row: DetailRow, shown: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.hidden = toggled(&self.hidden, row, shown);
        cx.notify();
        let (b, hidden) = (backend(cx), self.hidden.clone());
        // The settings file is written off the UI thread.
        let task = irori_ui::run(cx, async move {
            let saver = b.clone();
            b.spawn_blocking(move || {
                saver
                    .settings
                    .update_app(|s| s.ui.hidden_detail_rows = hidden)
            })
            .await
            .map_err(anyhow::Error::from)
        });
        cx.spawn_in(window, async move |_, cx| {
            if let Err(e) = task.await {
                let _ = cx.update(|window, cx| {
                    window.push_notification(
                        Notification::error(format!("{e:#}")).title(t("game.details.rows_failed")),
                        cx,
                    )
                });
            }
        })
        .detach();
    }
}

impl Render for DetailRowsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = DetailRow::ALL.into_iter().enumerate().map(|(i, row)| {
            Checkbox::new(("detail-row", i))
                .label(row.label())
                .checked(row.shown_in(&self.hidden))
                .on_change(cx.listener(move |this, shown: &bool, window, cx| {
                    this.set(row, *shown, window, cx)
                }))
        });
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t("game.details.edit_rows_help")),
            )
            .child(v_flex().gap_1p5().children(rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_ids_are_unique_and_hiding_keeps_unknown_ids() {
        let mut ids: Vec<&str> = DetailRow::ALL.iter().map(|r| r.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), DetailRow::ALL.len());

        let hidden = vec!["from_a_later_irori".to_string()];
        let hidden = toggled(&hidden, DetailRow::Publisher, false);
        assert!(!DetailRow::Publisher.shown_in(&hidden));
        assert!(DetailRow::Developer.shown_in(&hidden));
        // Hiding twice keeps one entry; showing again removes it.
        let again = toggled(&hidden, DetailRow::Publisher, false);
        assert_eq!(again, hidden);
        let shown = toggled(&hidden, DetailRow::Publisher, true);
        assert_eq!(shown, ["from_a_later_irori"]);
    }
}
