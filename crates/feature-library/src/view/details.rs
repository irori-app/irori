//! Metadata actions on the picked games, from the bulk bar's Details menu
//! and the command palette: "Fetch details again" (the wikis', IGDB's and
//! the stores' answers read anew, `Backend::refresh_details`) and "Find on
//! Steam by title" (each game linked to the Steam app of its exact title,
//! `Backend::link_by_title`). Both run in the background and say how they
//! went in a notification.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{Disableable as _, Sizable as _, WindowExt as _};
use gpui_kit::{App, Context, IntoElement, SharedString, Window};
use irori_core::GameId;
use irori_ui::backend;
use irori_ui::i18n::t;

use super::LibraryView;
use super::bulk::counted;
use super::remove::active_library;

/// How many games are picked, while the library is in selection mode with
/// any (the palette's Details commands).
pub fn picked_count(cx: &App) -> Option<usize> {
    let view = active_library(cx)?;
    let view = view.read(cx);
    if !view.selection.is_active() {
        return None;
    }
    let count = view.selection.games(&view.frame.games).len();
    (count > 0).then_some(count)
}

fn picked_ids(view: &LibraryView) -> Vec<GameId> {
    view.selection
        .games(&view.frame.games)
        .iter()
        .map(|g| g.id())
        .collect()
}

/// Read the picked games' details again (the palette's command).
pub fn refresh_picked_details(window: &mut Window, cx: &mut App) {
    if let Some(view) = active_library(cx) {
        view.update(cx, |this, cx| {
            let ids = picked_ids(this);
            this.refresh_details_of(ids, window, cx)
        });
    }
}

/// Link the picked games to Steam by title (the palette's command).
pub fn link_picked_to_steam(window: &mut Window, cx: &mut App) {
    if let Some(view) = active_library(cx) {
        view.update(cx, |this, cx| {
            let ids = picked_ids(this);
            this.link_to_steam(ids, window, cx)
        });
    }
}

/// What "Fetch details again" says when done: how many games were read,
/// failed and aren't on IGDB, each part only when it happened.
pub(crate) fn refreshed_message(report: &irori_services::DetailsReport) -> SharedString {
    let mut parts: Vec<String> = Vec::new();
    if report.refreshed > 0 {
        parts.push(
            counted(
                (
                    "library.bulk.details.refreshed_one",
                    "library.bulk.details.refreshed_other",
                ),
                report.refreshed,
                &[],
            )
            .to_string(),
        );
    }
    if report.failed > 0 {
        parts.push(
            counted(
                (
                    "library.bulk.details.failed_one",
                    "library.bulk.details.failed_other",
                ),
                report.failed,
                &[],
            )
            .to_string(),
        );
    }
    if report.not_on_igdb > 0 {
        parts.push(
            counted(
                (
                    "library.bulk.details.not_on_igdb_one",
                    "library.bulk.details.not_on_igdb_other",
                ),
                report.not_on_igdb,
                &[],
            )
            .to_string(),
        );
    }
    parts.join(" ").into()
}

/// What "Find on Steam by title" says when done.
pub(crate) fn linked_message(report: &irori_services::TitleLinkReport) -> SharedString {
    let mut parts = vec![
        counted(
            (
                "library.bulk.details.linked_one",
                "library.bulk.details.linked_other",
            ),
            report.linked,
            &[],
        )
        .to_string(),
    ];
    if report.not_found > 0 {
        parts.push(
            counted(
                (
                    "library.bulk.details.not_found_one",
                    "library.bulk.details.not_found_other",
                ),
                report.not_found,
                &[],
            )
            .to_string(),
        );
    }
    if report.skipped > 0 {
        parts.push(
            counted(
                (
                    "library.bulk.details.skipped_one",
                    "library.bulk.details.skipped_other",
                ),
                report.skipped,
                &[],
            )
            .to_string(),
        );
    }
    parts.join(" ").into()
}

impl LibraryView {
    /// The bulk bar's Details menu for the picked `ids`.
    pub(super) fn details_button(
        &self,
        ids: Rc<Vec<GameId>>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let view = cx.entity().downgrade();
        Button::new("bulk-details")
            .small()
            .outline()
            .icon(IconName::RefreshCw)
            .label(t("library.bulk.details.menu"))
            .disabled(ids.is_empty())
            .dropdown_menu(move |menu, _, _| {
                let (refresh, link) = (view.clone(), view.clone());
                let (refresh_ids, link_ids) = (ids.clone(), ids.clone());
                menu.item(
                    PopupMenuItem::new(t("library.bulk.details.refresh"))
                        .icon(IconName::RefreshCw)
                        .on_click(move |_, window, cx| {
                            let ids = refresh_ids.to_vec();
                            let _ = refresh
                                .update(cx, |this, cx| this.refresh_details_of(ids, window, cx));
                        }),
                )
                .item(
                    PopupMenuItem::new(t("library.bulk.details.link_steam"))
                        .icon(IconName::Link)
                        .on_click(move |_, window, cx| {
                            let ids = link_ids.to_vec();
                            let _ = link.update(cx, |this, cx| this.link_to_steam(ids, window, cx));
                        }),
                )
            })
    }

    pub(super) fn refresh_details_of(
        &mut self,
        ids: Vec<GameId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        window.push_notification(
            Notification::info(counted(
                (
                    "library.bulk.details.refreshing_one",
                    "library.bulk.details.refreshing_other",
                ),
                ids.len(),
                &[],
            )),
            cx,
        );
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { Ok(b.refresh_details(&ids).await) });
        cx.spawn_in(window, async move |_, cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(report) if report.refreshed == 0 && report.failed > 0 => window
                    .push_notification(
                        Notification::error(report.error.clone().unwrap_or_default())
                            .title(refreshed_message(&report)),
                        cx,
                    ),
                Ok(report) => {
                    window.push_notification(Notification::success(refreshed_message(&report)), cx)
                }
                Err(e) => crate::notify_error(t("library.bulk.details.refresh"), &e, window, cx),
            });
        })
        .detach();
    }

    pub(super) fn link_to_steam(
        &mut self,
        ids: Vec<GameId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ids.is_empty() {
            return;
        }
        window.push_notification(
            Notification::info(counted(
                (
                    "library.bulk.details.linking_one",
                    "library.bulk.details.linking_other",
                ),
                ids.len(),
                &[],
            )),
            cx,
        );
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { Ok(b.link_by_title(&ids).await) });
        cx.spawn_in(window, async move |_, cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(report) => match &report.failed {
                    Some(e) => window.push_notification(
                        Notification::error(e.clone()).title(t("library.bulk.details.link_failed")),
                        cx,
                    ),
                    None => {
                        window.push_notification(Notification::success(linked_message(&report)), cx)
                    }
                },
                Err(e) => {
                    crate::notify_error(t("library.bulk.details.link_failed"), &e, window, cx)
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_services::{DetailsReport, TitleLinkReport};

    #[test]
    fn reports_say_only_what_happened() {
        crate::i18n::register();
        let report = DetailsReport {
            refreshed: 3,
            ..Default::default()
        };
        assert_eq!(
            refreshed_message(&report),
            "Read the details of 3 games again."
        );
        let report = DetailsReport {
            refreshed: 1,
            failed: 2,
            not_on_igdb: 1,
            error: None,
        };
        let text = refreshed_message(&report);
        assert!(text.contains("1 game again") && text.contains('2') && text.contains("IGDB"));
        let linked = TitleLinkReport {
            linked: 2,
            not_found: 1,
            skipped: 0,
            failed: None,
        };
        let text = linked_message(&linked);
        assert!(text.starts_with("Linked 2 games") && !text.contains("left out"));
    }
}
