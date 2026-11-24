//! "Earlier saves" in a game's cloud-saves sheet: the snapshots Irori took
//! before syncs that could replace the saves (`irori_services::save_snapshots`),
//! each restorable after a confirmation that names the game and the day.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::{
    Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div,
};
use irori_core::GameId;
use irori_core::util::format_bytes;
use irori_services::save_snapshots::SaveSnapshot;
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::ops::notify_error;

/// Snapshots listed before the list stops; the setting keeps 50 at most.
const SHOWN: usize = 50;

/// The earlier saves of one game.
pub struct SaveSnapshotsSection {
    id: GameId,
    title: SharedString,
    /// `None` while they are read.
    snapshots: Option<Vec<SaveSnapshot>>,
    /// The snapshot being put back.
    restoring: Option<String>,
}

impl SaveSnapshotsSection {
    pub fn new(id: GameId, title: SharedString, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            id,
            title,
            snapshots: None,
            restoring: None,
        };
        this.reload(cx);
        this
    }

    /// Read the snapshots again (after a sync took one).
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let b = backend(cx);
        let id = self.id.clone();
        let task = irori_ui::state::run_infallible(cx, async move { b.save_snapshots(&id).await });
        cx.spawn(async move |this, cx| {
            let snapshots = task.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.snapshots = Some(snapshots);
                cx.notify();
            });
        })
        .detach();
    }

    fn confirm_restore(
        &mut self,
        snapshot: &SaveSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let when = when(snapshot);
        let title = t_args(
            "game.saves.snapshot_restore_title",
            &[("title", &self.title), ("date", &when)],
        );
        let id = snapshot.id.clone();
        let this = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, id) = (this.clone(), id.clone());
            alert
                .title(title.clone())
                .description(t("game.saves.snapshot_restore_description"))
                .confirm()
                .ok_text(t("game.saves.snapshot_restore"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let id = id.clone();
                    this.update(cx, |s, cx| s.restore(id, window, cx));
                    true
                })
        });
    }

    fn restore(&mut self, snapshot: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.restoring.is_some() {
            return;
        }
        self.restoring = Some(snapshot.clone());
        cx.notify();
        let b = backend(cx);
        let id = self.id.clone();
        let task = irori_ui::run(
            cx,
            async move { b.restore_save_snapshot(&id, &snapshot).await },
        );
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.restoring = None;
                match result {
                    Ok(files) => window.push_notification(
                        gpui_kit::component::notification::Notification::success(t_args(
                            if files == 1 {
                                "game.saves.snapshot_restored_one"
                            } else {
                                "game.saves.snapshot_restored_other"
                            },
                            &[("count", &files.to_string()), ("title", &this.title)],
                        )),
                        cx,
                    ),
                    Err(e) => notify_error(window, cx, t("game.saves.snapshot_restore_failed"), &e),
                }
                this.reload(cx);
            });
        })
        .detach();
    }
}

/// "Oct 11, 2026, 14:05" in the interface language, in local time.
fn when(snapshot: &SaveSnapshot) -> String {
    match chrono::DateTime::parse_from_rfc3339(&snapshot.created) {
        Ok(time) => {
            let local = time.with_timezone(&chrono::Local);
            t_args(
                "game.saves.snapshot_when",
                &[
                    (
                        "date",
                        &irori_ui::dates::calendar_date(local.date_naive(), true),
                    ),
                    ("time", &local.format("%H:%M").to_string()),
                ],
            )
            .to_string()
        }
        Err(_) => snapshot.created.clone(),
    }
}

/// What a snapshot was taken before.
fn reason(snapshot: &SaveSnapshot) -> SharedString {
    t(match snapshot.reason.as_str() {
        "download" => "game.saves.snapshot_reason.download",
        "force-download" => "game.saves.snapshot_reason.force_download",
        "force-upload" => "game.saves.snapshot_reason.force_upload",
        "restore" => "game.saves.snapshot_reason.restore",
        _ => "game.saves.snapshot_reason.auto",
    })
}

impl Render for SaveSnapshotsSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let body = match &self.snapshots {
            None => div()
                .text_sm()
                .text_color(muted)
                .child(t("game.saves.snapshots_loading"))
                .into_any_element(),
            Some(list) if list.is_empty() => div()
                .text_sm()
                .text_color(muted)
                .child(t("game.saves.snapshots_none"))
                .into_any_element(),
            Some(list) => v_flex()
                .gap_2()
                .children(list.iter().take(SHOWN).enumerate().map(|(ix, s)| {
                    let snapshot = s.clone();
                    let busy = self.restoring.as_deref() == Some(s.id.as_str());
                    h_flex()
                        .gap_3()
                        .min_w_0()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(div().text_sm().truncate().child(when(s)))
                                .child(div().text_xs().truncate().text_color(muted).child(t_args(
                                    "game.saves.snapshot_details",
                                    &[("reason", &reason(s)), ("size", &format_bytes(s.bytes))],
                                ))),
                        )
                        .child(
                            Button::new(SharedString::from(format!("snapshot-restore-{ix}")))
                                .outline()
                                .small()
                                .icon(IconName::ArchiveRestore)
                                .label(t("game.saves.snapshot_restore"))
                                .loading(busy)
                                .disabled(self.restoring.is_some())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.confirm_restore(&snapshot, window, cx)
                                })),
                        )
                        .into_any_element()
                }))
                .into_any_element(),
        };
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(t("game.saves.snapshots")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t("game.saves.snapshots_help")),
            )
            .child(body)
    }
}
