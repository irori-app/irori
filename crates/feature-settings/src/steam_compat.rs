//! The Steam Play group of a Steam game's Wine & Proton section (Linux):
//! which compatibility tool Steam runs the game with, written into Steam's
//! own settings through `irori_steam::SteamStore`, with the game's ProtonDB
//! rating and what it suggests next to the choice.
//!
//! Steam overwrites its settings when it exits, so a new choice is saved
//! only while Steam is closed. When it runs, Irori asks before closing it.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div,
};
use irori_core::GameId;
use irori_extras::wiki::{self, ProtonDbTier, StoreIds};
use irori_steam::{CompatToolState, SteamRunning, SteamStore};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::form::ChoiceItem;
use crate::i18n::LanguageStamp;
use crate::widgets::{RowState, field_row, group_heading, group_panel, muted};

/// The key of "Steam's default" among the choices.
const DEFAULT_KEY: &str = "";

enum Load {
    Loading,
    Ready(CompatToolState),
    Failed(SharedString),
}

/// What ProtonDB says about the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rating {
    Loading,
    Tier(ProtonDbTier),
    Unrated,
    Unreachable,
}

/// What the panel is doing now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Busy {
    Idle,
    Saving,
    Closing,
}

pub(crate) struct SteamCompatPanel {
    id: GameId,
    steam: Arc<SteamStore>,
    load: Load,
    rating: Rating,
    busy: Busy,
    select: Entity<SelectState<Vec<ChoiceItem>>>,
    /// The select's labels are rebuilt after a language change.
    language: LanguageStamp,
    _subscriptions: Vec<Subscription>,
}

impl SteamCompatPanel {
    pub(crate) fn new(
        id: GameId,
        steam: Arc<SteamStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let select = cx.new(|cx| SelectState::new(default_items(), None, window, cx));
        let sub = cx.subscribe_in(
            &select,
            window,
            |this, _, event: &SelectEvent<Vec<ChoiceItem>>, window, cx| {
                if let SelectEvent::Confirm(Some(key)) = event {
                    let tool = (key.as_ref() != DEFAULT_KEY).then(|| key.to_string());
                    this.choose(tool, window, cx);
                }
            },
        );
        let mut this = Self {
            id,
            steam,
            load: Load::Loading,
            rating: Rating::Loading,
            busy: Busy::Idle,
            select,
            language: LanguageStamp::new(),
            _subscriptions: vec![sub],
        };
        this.reload(window, cx);
        this.look_up_rating(cx);
        this
    }

    fn title(&self, cx: &Context<Self>) -> String {
        backend(cx)
            .game(&self.id)
            .map(|g| g.title.clone())
            .unwrap_or_else(|| self.id.app_name.to_string())
    }

    fn current(&self) -> Option<&str> {
        match &self.load {
            Load::Ready(state) => state.current.as_deref(),
            _ => None,
        }
    }

    /// Read the tools and the game's choice again.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let steam = self.steam.clone();
        let app = self.id.app_name.to_string();
        let task = irori_ui::run(cx, async move { steam.compat_tools(&app).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(state) => {
                        let items = items(&state);
                        let current = state.current.clone().unwrap_or_default();
                        this.select.update(cx, |s, cx| {
                            s.set_items(items, window, cx);
                            s.set_selected_value(&SharedString::from(current), window, cx);
                        });
                        this.load = Load::Ready(state);
                    }
                    Err(e) => this.load = Load::Failed(format!("{e:#}").into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn look_up_rating(&mut self, cx: &mut Context<Self>) {
        // Only games of the library are looked up; the settings of any
        // other say the game is unknown.
        let Some(game) = backend(cx).game(&self.id) else {
            self.rating = Rating::Unrated;
            return;
        };
        let app = self.id.app_name.to_string();
        let task = irori_ui::run(cx, async move {
            // The game page usually looked the game up already.
            if let Some(info) = wiki::cached(&StoreIds::of(&game)).await
                && let Some(tier) = info.steam.as_ref().and_then(|s| s.protondb)
            {
                return Ok(Some(tier));
            }
            wiki::sources::protondb(&app).await
        });
        cx.spawn(async move |this, cx| {
            let rating = match task.await {
                Ok(Some(ProtonDbTier::Unknown)) | Ok(None) => Rating::Unrated,
                Ok(Some(tier)) => Rating::Tier(tier),
                Err(e) => {
                    tracing::debug!("ProtonDB: {e:#}");
                    Rating::Unreachable
                }
            };
            this.update(cx, |this, cx| {
                this.rating = rating;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Show the saved choice in the select again.
    fn show_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = SharedString::from(self.current().unwrap_or_default().to_string());
        self.select
            .update(cx, |s, cx| s.set_selected_value(&current, window, cx));
    }

    /// The name the select shows for `tool`.
    fn label_of(&self, tool: Option<&str>) -> SharedString {
        let Some(tool) = tool else {
            return t("settings.steam_compat.default");
        };
        match &self.load {
            Load::Ready(state) => state
                .tools
                .iter()
                .find(|t| t.name == tool)
                .map(|t| SharedString::from(t.display_name.clone()))
                .unwrap_or_else(|| tool.to_string().into()),
            _ => tool.to_string().into(),
        }
    }

    fn choose(&mut self, tool: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Load::Ready(state) = &self.load else {
            return;
        };
        if self.busy != Busy::Idle || state.current == tool {
            return;
        }
        if state.steam_running {
            // Until Steam is closed and the choice saved, the select keeps
            // showing what Steam has.
            self.show_current(window, cx);
            self.ask_to_close_steam(tool, window, cx);
        } else {
            self.save(tool, false, window, cx);
        }
    }

    fn ask_to_close_steam(
        &mut self,
        tool: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let can_close = matches!(&self.load, Load::Ready(s) if s.can_close_steam);
        if !can_close {
            window.push_notification(
                Notification::error(t("settings.steam_compat.session"))
                    .title(t("settings.steam_compat.save_failed")),
                cx,
            );
            return;
        }
        let title = self.title(cx);
        let label = self.label_of(tool.as_deref());
        let panel = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let panel = panel.clone();
            let tool = tool.clone();
            alert
                .title(t_args(
                    "settings.steam_compat.close.title",
                    &[("title", &title)],
                ))
                .description(t_args(
                    "settings.steam_compat.close.description",
                    &[("tool", &label)],
                ))
                .show_cancel(true)
                .cancel_text(t("settings.common.cancel"))
                .ok_text(t("settings.steam_compat.close.confirm"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let tool = tool.clone();
                    panel
                        .update(cx, |p, cx| p.save(tool, true, window, cx))
                        .ok();
                    true
                })
        });
    }

    /// Save `tool`, closing Steam first when `close_steam`.
    fn save(
        &mut self,
        tool: Option<String>,
        close_steam: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.busy = if close_steam {
            Busy::Closing
        } else {
            Busy::Saving
        };
        cx.notify();
        let steam = self.steam.clone();
        let app = self.id.app_name.to_string();
        let chosen = tool.clone();
        let task = irori_ui::run(cx, async move {
            if close_steam {
                steam.close_steam().await?;
            }
            steam.set_compat_tool(&app, chosen.as_deref()).await
        });
        let title = self.title(cx);
        let label = self.label_of(tool.as_deref());
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = Busy::Idle;
                match result {
                    Ok(()) => {
                        if let Load::Ready(state) = &mut this.load {
                            state.current = tool.clone();
                            state.steam_running = false;
                        }
                        let message = match &tool {
                            Some(_) => t_args(
                                "settings.steam_compat.saved",
                                &[("title", &title), ("tool", &label)],
                            ),
                            None => {
                                t_args("settings.steam_compat.saved_default", &[("title", &title)])
                            }
                        };
                        window.push_notification(Notification::success(message), cx);
                    }
                    Err(e) if e.downcast_ref::<SteamRunning>().is_some() && !close_steam => {
                        // Steam started since the panel looked.
                        if let Load::Ready(state) = &mut this.load {
                            state.steam_running = true;
                        }
                        this.show_current(window, cx);
                        this.ask_to_close_steam(tool, window, cx);
                    }
                    Err(e) => {
                        this.show_current(window, cx);
                        window.push_notification(
                            Notification::error(format!("{e:#}"))
                                .title(t("settings.steam_compat.save_failed")),
                            cx,
                        );
                        // Steam may have closed, or not: show what is true now.
                        if close_steam {
                            this.reload(window, cx);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The tools and choice shown, once read.
    #[cfg(test)]
    pub(crate) fn state_for_test(&self) -> Option<&CompatToolState> {
        match &self.load {
            Load::Ready(state) => Some(state),
            _ => None,
        }
    }

    /// The choice the select shows.
    #[cfg(test)]
    pub(crate) fn selected_for_test(&self, cx: &gpui_kit::App) -> Option<String> {
        self.select.read(cx).selected_value().map(|v| v.to_string())
    }

    fn render_rating(&self, cx: &mut Context<Self>) -> AnyElement {
        let help = match self.rating {
            Rating::Loading => t("settings.steam_compat.protondb.loading"),
            Rating::Unrated => t("settings.steam_compat.protondb.none"),
            Rating::Unreachable => t("settings.steam_compat.protondb.unavailable"),
            Rating::Tier(tier) => t(advice_key(tier)),
        };
        let url = format!("https://www.protondb.com/app/{}", self.id.app_name);
        let reports = Button::new("steam-compat-protondb")
            .outline()
            .small()
            .icon(IconName::ExternalLink)
            .label(t("settings.steam_compat.protondb.reports"))
            .on_click(move |_, _, cx| {
                if let Some(url) = irori_ui::web_url(&url) {
                    cx.open_url(&url);
                }
            })
            .into_any_element();
        field_row(
            RowState {
                label: "ProtonDB".into(),
                help,
                ..Default::default()
            },
            reports,
            cx,
        )
        .into_any_element()
    }

    fn render_choice(&self, state: &CompatToolState, cx: &mut Context<Self>) -> AnyElement {
        let note = if self.busy == Busy::Closing {
            Some(t("settings.steam_compat.closing"))
        } else if self.busy == Busy::Saving {
            Some(t("settings.steam_compat.saving"))
        } else if state.steam_running && !state.can_close_steam {
            Some(t("settings.steam_compat.session"))
        } else if state.steam_running {
            Some(t("settings.steam_compat.running"))
        } else if state.tools.is_empty() {
            Some(t("settings.steam_compat.none_found"))
        } else {
            None
        };
        let control = Select::new(&self.select)
            .id("steam-compat-tool")
            .accessibility_label(t("settings.steam_compat.label"))
            .disabled(self.busy != Busy::Idle)
            .w_64()
            .into_any_element();
        field_row(
            RowState {
                label: t("settings.steam_compat.label"),
                help: t("settings.steam_compat.help"),
                disabled_reason: note,
                ..Default::default()
            },
            control,
            cx,
        )
        .into_any_element()
    }
}

/// The advice for a ProtonDB tier.
fn advice_key(tier: ProtonDbTier) -> &'static str {
    match tier {
        ProtonDbTier::Native => "settings.steam_compat.protondb.native",
        ProtonDbTier::Platinum => "settings.steam_compat.protondb.platinum",
        ProtonDbTier::Gold => "settings.steam_compat.protondb.gold",
        ProtonDbTier::Silver => "settings.steam_compat.protondb.silver",
        ProtonDbTier::Bronze => "settings.steam_compat.protondb.bronze",
        ProtonDbTier::Borked => "settings.steam_compat.protondb.borked",
        ProtonDbTier::Pending | ProtonDbTier::Unknown => "settings.steam_compat.protondb.pending",
    }
}

fn default_items() -> Vec<ChoiceItem> {
    vec![ChoiceItem::new(
        DEFAULT_KEY,
        t("settings.steam_compat.default"),
    )]
}

/// "Steam's default", every installed tool, and the current choice when it
/// is no longer installed.
fn items(state: &CompatToolState) -> Vec<ChoiceItem> {
    let mut items = default_items();
    items.extend(
        state
            .tools
            .iter()
            .map(|tool| ChoiceItem::new(tool.name.clone(), tool.display_name.clone())),
    );
    if let Some(current) = &state.current
        && !state.tools.iter().any(|tool| &tool.name == current)
    {
        items.push(ChoiceItem::new(
            current.clone(),
            t_args("settings.steam_compat.missing", &[("name", current)]),
        ));
    }
    items
}

impl Render for SteamCompatPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.language.changed() {
            cx.defer_in(window, |this, window, cx| this.reload(window, cx));
        }
        let body: AnyElement = match &self.load {
            Load::Loading => div()
                .px_4()
                .py_3()
                .child(muted(t("settings.common.loading"), cx))
                .into_any_element(),
            Load::Failed(message) => v_flex()
                .gap_2()
                .px_4()
                .py_3()
                .child(
                    Alert::warning("steam-compat-failed", message.clone())
                        .title(t("settings.steam_compat.load_failed")),
                )
                .child(
                    h_flex().child(
                        Button::new("steam-compat-retry")
                            .outline()
                            .small()
                            .icon(IconName::RefreshCw)
                            .label(t("settings.common.refresh"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.load = Load::Loading;
                                this.reload(window, cx);
                            })),
                    ),
                )
                .into_any_element(),
            Load::Ready(state) => {
                let state = state.clone();
                self.render_choice(&state, cx)
            }
        };
        let muted_fg = cx.theme().muted_foreground;
        let footer = h_flex()
            .gap_3()
            .px_4()
            .py_3()
            .items_start()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(Icon::new(IconName::Info).small().text_color(muted_fg))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(muted(t("settings.steam_compat.footer"), cx)),
            )
            .child(
                Button::new("steam-compat-refresh")
                    .ghost()
                    .xsmall()
                    .icon(IconName::RefreshCw)
                    .label(t("settings.common.refresh"))
                    .disabled(self.busy != Busy::Idle)
                    .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
            );
        v_flex()
            .w_full()
            .child(group_heading(t("settings.steam_compat.heading"), cx))
            .child(
                group_panel(cx)
                    .child(body)
                    .child(
                        div()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .child(self.render_rating(cx)),
                    )
                    .when(matches!(self.load, Load::Ready(_)), |d| d.child(footer)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_steam::{CompatTool, ToolSource};

    fn state(current: Option<&str>) -> CompatToolState {
        CompatToolState {
            tools: vec![
                CompatTool {
                    name: "proton_experimental".into(),
                    display_name: "Proton Experimental".into(),
                    source: ToolSource::Valve,
                },
                CompatTool {
                    name: "GE-Proton10-4".into(),
                    display_name: "GE-Proton10-4".into(),
                    source: ToolSource::Custom("/s/compatibilitytools.d/GE-Proton10-4".into()),
                },
            ],
            current: current.map(str::to_string),
            steam_running: false,
            can_close_steam: true,
        }
    }

    fn keys(items: &[ChoiceItem]) -> Vec<String> {
        use gpui_kit::component::select::SelectItem as _;
        items.iter().map(|i| i.value().to_string()).collect()
    }

    #[test]
    fn offers_the_default_then_every_tool() {
        assert_eq!(
            keys(&items(&state(Some("proton_experimental")))),
            ["", "proton_experimental", "GE-Proton10-4"]
        );
        // A tool that was removed since stays visible as the current choice.
        assert_eq!(
            keys(&items(&state(Some("GE-Proton9-1")))),
            ["", "proton_experimental", "GE-Proton10-4", "GE-Proton9-1"]
        );
    }

    #[test]
    fn every_tier_has_advice() {
        for tier in [
            ProtonDbTier::Native,
            ProtonDbTier::Platinum,
            ProtonDbTier::Gold,
            ProtonDbTier::Silver,
            ProtonDbTier::Bronze,
            ProtonDbTier::Borked,
            ProtonDbTier::Pending,
            ProtonDbTier::Unknown,
        ] {
            assert!(!crate::i18n::english(advice_key(tier)).is_empty());
        }
    }
}
