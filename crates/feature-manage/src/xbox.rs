//! The Xbox store's own parts of the accounts screen: the profile with its
//! Game Pass membership, and "Offline play", which shows and changes whether
//! this computer is the Microsoft account's designated offline PC
//! (`irori_xbox::XboxStore::offline_pc`). Microsoft allows one offline PC
//! per account and a few changes a year, so a change is confirmed first.

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Global, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div,
};
use irori_ui::i18n::{t, t_args};
use irori_xbox::XboxStore;
use serde_json::Value;

use crate::common::{notify_error, notify_success};

struct XboxHandle(Arc<XboxStore>);
impl Global for XboxHandle {}

/// Give the management screens the Xbox store (the backend exposes stores
/// only as `dyn Store`).
pub fn install_xbox_store(store: Arc<XboxStore>, cx: &mut App) {
    cx.set_global(XboxHandle(store));
}

pub(crate) fn xbox_store(cx: &App) -> Option<Arc<XboxStore>> {
    cx.try_global::<XboxHandle>().map(|h| h.0.clone())
}

/// Whether "Offline play" can be shown: the Store account is signed in.
pub fn xbox_offline_available(cx: &App) -> bool {
    xbox_store(cx).is_some_and(|s| s.signed_in())
}

/// Open "Offline play" for the signed-in Store account.
pub fn open_offline_play_dialog(window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let Some(store) = xbox_store(cx) else {
        return;
    };
    let view = cx.new(|cx| OfflinePlay::new(store, window, cx));
    let width = window.rem_size() * 32.;
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title(t("manage.xbox.offline.title"))
            .w(width)
            .child(view.clone())
    });
}

/// The designation as Microsoft reports it.
#[derive(Debug, Clone, PartialEq)]
struct Designation {
    enabled: bool,
    remaining: Option<u64>,
    max: Option<u64>,
    period_days: Option<u64>,
    next_change: Option<String>,
}

impl Designation {
    fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            enabled: value["enabled"].as_bool()?,
            remaining: value["remainingToggles"].as_u64(),
            max: value["maxToggles"].as_u64(),
            period_days: value["periodDays"].as_u64(),
            next_change: value["nextChangeAt"]
                .as_str()
                .map(|d| d.chars().take(10).collect()),
        })
    }

    /// Whether Microsoft allows another change now.
    fn can_change(&self) -> bool {
        self.remaining != Some(0)
    }
}

enum Load {
    Loading,
    Ready(Designation),
    Failed(SharedString),
}

struct OfflinePlay {
    store: Arc<XboxStore>,
    load: Load,
    /// The change being asked for, before the user confirmed it.
    confirming: Option<bool>,
    busy: bool,
    profile: Option<Value>,
}

impl OfflinePlay {
    fn new(store: Arc<XboxStore>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            store,
            load: Load::Loading,
            confirming: None,
            busy: false,
            profile: None,
        };
        this.request(None, window, cx);
        let store = this.store.clone();
        let task = irori_ui::run(cx, async move { store.profile().await });
        cx.spawn(async move |this, cx| {
            if let Ok(profile) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.profile = Some(profile);
                    cx.notify();
                });
            }
        })
        .detach();
        this
    }

    /// Read (`set` = None) or change the designation.
    fn request(&mut self, set: Option<bool>, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = true;
        self.confirming = None;
        cx.notify();
        let store = self.store.clone();
        let task = irori_ui::run(cx, async move { store.offline_pc(set).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                if set.is_some() {
                    match &result {
                        Ok(_) => notify_success(t("manage.xbox.offline.changed"), window, cx),
                        Err(e) => notify_error(t("manage.xbox.offline.failed"), e, window, cx),
                    }
                }
                this.load = match result {
                    Ok(value) => match Designation::from_json(&value) {
                        Some(d) => Load::Ready(d),
                        None => Load::Failed(t("manage.xbox.offline.unreadable")),
                    },
                    Err(e) => Load::Failed(format!("{e:#}").into()),
                };
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for OfflinePlay {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        let _ = window;
        let muted = cx.theme().muted_foreground;
        let membership = self.profile.as_ref().map(|p| {
            let gamertag = p["gamertag"].as_str().unwrap_or_default().to_string();
            let active = p["subscription"]["status"] == "active";
            v_flex()
                .gap_0p5()
                .when(!gamertag.is_empty(), |c| {
                    c.child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(gamertag),
                    )
                })
                .child(div().text_xs().text_color(muted).child(if active {
                    t("manage.xbox.game_pass.active")
                } else {
                    t("manage.xbox.game_pass.none")
                }))
        });
        let body = match &self.load {
            Load::Loading => h_flex()
                .gap_2()
                .text_sm()
                .text_color(muted)
                .child(Spinner::new().small())
                .child(t("manage.xbox.offline.checking"))
                .into_any_element(),
            Load::Failed(message) => div()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(message.clone())
                .into_any_element(),
            Load::Ready(d) => {
                let state = if d.enabled {
                    t("manage.xbox.offline.enabled")
                } else {
                    t("manage.xbox.offline.disabled")
                };
                let limits = match (d.remaining, d.max, d.period_days) {
                    (Some(left), Some(max), Some(days)) => Some(t_args(
                        "manage.xbox.offline.limits",
                        &[
                            ("left", &left.to_string()),
                            ("max", &max.to_string()),
                            ("days", &days.to_string()),
                        ],
                    )),
                    _ => None,
                };
                let next = d
                    .next_change
                    .clone()
                    .filter(|_| !d.can_change())
                    .map(|date| t_args("manage.xbox.offline.next_change", &[("date", &date)]));
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child(state))
                    .children(limits.map(|l| div().text_xs().text_color(muted).child(l)))
                    .children(next.map(|n| div().text_xs().text_color(muted).child(n)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("manage.xbox.offline.explain")),
                    )
                    .into_any_element()
            }
        };
        let confirm = self.confirming.map(|turn_on| {
            div()
                .p_3()
                .rounded(cx.theme().radius)
                .bg(cx.theme().muted)
                .text_sm()
                .child(if turn_on {
                    t("manage.xbox.offline.confirm_on")
                } else {
                    t("manage.xbox.offline.confirm_off")
                })
        });
        let ready = match &self.load {
            Load::Ready(d) => Some(d.clone()),
            _ => None,
        };
        let mut footer = DialogFooter::new().child(
            Button::new("xbox-offline-close")
                .ghost()
                .label(t("manage.xbox.offline.close"))
                .on_click(|_, window, cx| window.close_dialog(cx)),
        );
        if let Some(d) = ready {
            let turn_on = !d.enabled;
            let label = if turn_on {
                t("manage.xbox.offline.turn_on")
            } else {
                t("manage.xbox.offline.turn_off")
            };
            if self.confirming == Some(turn_on) {
                footer = footer.child(
                    Button::new("xbox-offline-confirm")
                        .with_variant(if turn_on {
                            ButtonVariant::Primary
                        } else {
                            ButtonVariant::Danger
                        })
                        .label(label)
                        .loading(self.busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.request(Some(turn_on), window, cx)
                        })),
                );
            } else {
                footer = footer.child(
                    Button::new("xbox-offline-change")
                        .outline()
                        .label(label)
                        .disabled(self.busy || !d.can_change())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.confirming = Some(turn_on);
                            cx.notify();
                        })),
                );
            }
        }
        v_flex()
            .gap_4()
            .children(membership)
            .child(body)
            .children(confirm)
            .child(footer)
    }
}
