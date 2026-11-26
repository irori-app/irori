//! Store accounts ([`Route::Accounts`](irori_ui::Route::Accounts)): every
//! account of each store with its game count, log in / log out / refresh
//! per account, and adding another account.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, Div, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, px, rems,
};
use irori_core::{Runner, ShownStores, StoreAccount};
use irori_ui::components::{PageHeader, SectionTitle};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::common::{
    backend_op, card, card_grid_capped, info_tip, notify_error, notify_success, plain_tooltip,
};
use crate::login::{open_account_login_dialog, open_login_dialog};

/// Stores with a card on the accounts page, in display order. Steam is
/// detected on this computer and also supports an optional account sign-in.
pub(crate) const ACCOUNT_STORES: [Runner; 7] = [
    Runner::Legendary,
    Runner::Gog,
    Runner::Nile,
    Runner::Zoom,
    Runner::Itch,
    Runner::Xbox,
    Runner::Steam,
];

/// Narrowest a connected account card gets before dropping a column, in rem.
const CONNECTED_CARD_REM: f32 = 22.;
/// Narrowest an available or inactive store card gets before dropping a column, in rem.
const COMPACT_CARD_REM: f32 = 20.;
/// Narrowest a Steam optional sub-panel gets before stacking vertically, in rem.
const STEAM_SUB_REM: f32 = 26.;
/// Comfortable maximum width of the Accounts page content, in rem.
const PAGE_MAX_WIDTH_REM: f32 = 74.;

/// Visual section a store card belongs to on the Accounts page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccountSection {
    Connected,
    Available,
    Inactive,
}

/// Group a store card by whether it is active/connected, ready to set up, or
/// hidden/turned off.
pub(crate) fn classify_card(card: &StoreCard) -> AccountSection {
    if card.hidden || card.disabled {
        AccountSection::Inactive
    } else if card.user.is_some()
        || !card.accounts.is_empty()
        || card.expired.is_some()
        || card
            .steam_client
            .as_ref()
            .is_some_and(|client| client.steam_found)
    {
        AccountSection::Connected
    } else {
        AccountSection::Available
    }
}

/// Subtle inset container inside a store card (account rows and Steam sub-panels).
fn inset_panel(cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .w_full()
        .p_3()
        .gap_2()
        .rounded(theme.radius)
        .bg(theme.background)
        .border_1()
        .border_color(theme.border)
}

/// Icon used for a store across the management screens: its mark where
/// Irori has one (`irori_ui::components::StoreLogo`), else a generic icon.
pub(crate) fn store_icon(runner: Runner) -> Icon {
    irori_ui::components::runner_icon(
        runner,
        match runner {
            Runner::Legendary => IconName::Gamepad2,
            Runner::Gog => IconName::Store,
            Runner::Nile => IconName::ShoppingBag,
            Runner::Zoom => IconName::Package,
            Runner::Itch => IconName::Gem,
            Runner::Xbox => IconName::GamepadDirectional,
            Runner::Steam => IconName::Joystick,
            Runner::Sideload => IconName::FolderOpen,
            Runner::Emulation => IconName::Gamepad,
            Runner::Launchers => IconName::Rocket,
        },
    )
}

/// Whether an opt-in store is turned on (stores without a switch always are).
pub(crate) fn store_enabled(app: &irori_core::AppSettings, runner: Runner) -> bool {
    match runner {
        Runner::Zoom => app.zoom_platform,
        Runner::Itch => app.itch_io,
        Runner::Xbox => app.xbox,
        _ => true,
    }
}

fn set_store_enabled(app: &mut irori_core::AppSettings, runner: Runner, on: bool) {
    match runner {
        Runner::Zoom => app.zoom_platform = on,
        Runner::Itch => app.itch_io = on,
        Runner::Xbox => app.xbox = on,
        _ => {}
    }
}

/// What a store card shows; derived from the models when they change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoreCard {
    pub runner: Runner,
    pub user: Option<String>,
    pub games: usize,
    /// The store (ZOOM Platform, itch.io) is turned off in the settings.
    pub disabled: bool,
    /// The user hid the store: its games and pages are left out.
    pub hidden: bool,
    /// The store refused the saved login, with its reason.
    pub expired: Option<String>,
    /// The store reads games installed on this computer; it has no login.
    pub local: bool,
    /// Every account of a store with accounts.
    pub accounts: Vec<StoreAccount>,
    /// Another account can log in beside the signed-in ones.
    pub can_add: bool,
    /// Steam without a signed-in account: reading its library from the
    /// Steam client ([`crate::steam_library`]).
    pub steam_client: Option<irori_steam::ClientLibraryState>,
    /// Steam: what the card says about the games hidden in Steam.
    pub steam_hidden: Option<SharedString>,
    /// Steam: the shortcut changes waiting for Steam to close.
    pub steam_waiting: Option<SharedString>,
}

/// Cards for the stores this build has, in display order.
pub(crate) fn store_cards(cx: &App) -> Vec<StoreCard> {
    let b = backend(cx);
    let m = models(cx);
    let accounts = m.accounts.read(cx);
    let library = m.library.read(cx);
    let app = &m.settings.read(cx).app;
    let shown = app.ui.stores;
    let mut counts: BTreeMap<Runner, usize> = BTreeMap::new();
    for g in library.snapshot.all_games() {
        *counts.entry(g.runner()).or_default() += 1;
    }
    ACCOUNT_STORES
        .iter()
        .filter(|r| b.store(**r).is_some())
        .map(|&runner| StoreCard {
            runner,
            user: accounts.user(runner).map(|u| {
                let name = u.display_name.trim();
                if name.is_empty() {
                    t("manage.accounts.signed_in").to_string()
                } else {
                    name.to_string()
                }
            }),
            games: counts.get(&runner).copied().unwrap_or(0),
            disabled: !store_enabled(app, runner),
            hidden: !shown.shows(runner),
            expired: accounts.login_expired(runner).map(str::to_string),
            local: !runner.has_account(),
            accounts: accounts.accounts(runner).to_vec(),
            can_add: b.store(runner).is_some_and(|s| s.can_add_account()),
            steam_client: (runner == Runner::Steam)
                .then(|| crate::steam_library::steam_client_library_state(cx))
                .flatten()
                .filter(|s| !s.signed_in),
            steam_hidden: (runner == Runner::Steam)
                .then(|| crate::steam_library::steam_hidden_games(cx))
                .flatten()
                .and_then(|s| {
                    crate::steam_library::hidden_games_text(&s, library.snapshot.hidden_locked)
                }),
            steam_waiting: (runner == Runner::Steam)
                .then(|| steam_waiting_text(b.steam_shortcuts_waiting()))
                .flatten(),
        })
        .collect()
}

/// What the Steam card says about shortcut changes waiting for Steam to
/// close; nothing when none wait.
pub(crate) fn steam_waiting_text(waiting: usize) -> Option<SharedString> {
    match waiting {
        0 => None,
        1 => Some(t("manage.steam_shortcuts.waiting_one")),
        n => Some(t_args(
            "manage.steam_shortcuts.waiting_other",
            &[("count", &n.to_string())],
        )),
    }
}

/// An account's name, or a stand-in when the store gave none.
fn account_name(account: &StoreAccount) -> String {
    let name = account.display_name.trim();
    if name.is_empty() {
        t("manage.accounts.signed_in").to_string()
    } else {
        name.to_string()
    }
}

/// A store's own card actions use this in place of an account's slot.
const STORE_WIDE: &str = "";

/// One card per store with its account state and actions.
pub struct AccountsView {
    cards: Vec<StoreCard>,
    /// Measured widths of each grid at the last layout.
    connected_grid_width: Rc<Cell<Pixels>>,
    steam_sub_width: Rc<Cell<Pixels>>,
    available_grid_width: Rc<Cell<Pixels>>,
    inactive_grid_width: Rc<Cell<Pixels>>,
    online: bool,
    /// Accounts being refreshed or logged out, by store and slot.
    refreshing: BTreeSet<(Runner, String)>,
    logging_out: BTreeSet<(Runner, String)>,
    steam_ops: gpui_kit::Entity<crate::steam_library::SteamLibraryOps>,
    _subscriptions: Vec<Subscription>,
}

impl AccountsView {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::i18n::register();
        let m = models(cx).clone();
        let steam_ops = crate::steam_library::steam_library_ops(cx);
        let subscriptions = vec![
            cx.observe(&m.accounts, |this, _, cx| this.rebuild(cx)),
            cx.observe(&m.library, |this, _, cx| this.rebuild(cx)),
            cx.observe(&m.settings, |this, _, cx| this.rebuild(cx)),
            cx.observe(&steam_ops, |_, _, cx| cx.notify()),
        ];
        let mut this = Self {
            cards: Vec::new(),
            connected_grid_width: Rc::new(Cell::new(px(0.))),
            steam_sub_width: Rc::new(Cell::new(px(0.))),
            available_grid_width: Rc::new(Cell::new(px(0.))),
            inactive_grid_width: Rc::new(Cell::new(px(0.))),
            online: true,
            refreshing: BTreeSet::new(),
            logging_out: BTreeSet::new(),
            steam_ops,
            _subscriptions: subscriptions,
        };
        this.rebuild(cx);
        this
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let cards = store_cards(cx);
        let online = models(cx).accounts.read(cx).online;
        if cards != self.cards || online != self.online {
            self.cards = cards;
            self.online = online;
            cx.notify();
        }
    }

    /// Refresh one account (`slot`), or the whole store for [`STORE_WIDE`].
    fn refresh(
        &mut self,
        runner: Runner,
        slot: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = (runner, slot.clone());
        if !self.refreshing.insert(key.clone()) {
            return;
        }
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            if slot == STORE_WIDE {
                b.refresh_store(runner, true).await?;
            } else {
                b.refresh_account(runner, &slot, true).await?;
            }
            b.refresh_updates().await;
            Ok(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.refreshing.remove(&key);
                if let Err(e) = result {
                    notify_error(
                        t_args(
                            "manage.accounts.refresh_failed",
                            &[("store", runner.store_name())],
                        ),
                        &e,
                        window,
                        cx,
                    );
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Ask before logging out of `account`; with several accounts, name it.
    fn confirm_logout(
        &mut self,
        runner: Runner,
        account: StoreAccount,
        several: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        let store = runner.store_name();
        let name = account_name(&account);
        let title = if several {
            t_args(
                "manage.accounts.logout_account_confirm.title",
                &[("name", &name), ("store", store)],
            )
        } else {
            t_args("manage.accounts.logout_confirm.title", &[("store", store)])
        };
        let description = match account.installed {
            _ if runner.has_optional_account() => t_args(
                "manage.accounts.optional_logout_description",
                &[("store", store)],
            ),
            0 => t("manage.accounts.logout_confirm.description"),
            1 if several => t("manage.accounts.logout_account_confirm.installed_one"),
            n if several => t_args(
                "manage.accounts.logout_account_confirm.installed_other",
                &[("count", &n.to_string())],
            ),
            _ => t("manage.accounts.logout_confirm.description"),
        };
        let slot = account.slot.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            let slot = slot.clone();
            let name = name.clone();
            alert
                .confirm()
                .title(title.clone())
                .description(description.clone())
                .ok_text(t("manage.accounts.logout_confirm.ok"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let (slot, name) = (slot.clone(), several.then(|| name.clone()));
                    let _ = view.update(cx, |this, cx| this.logout(runner, slot, name, window, cx));
                    true
                })
        });
    }

    /// Log out of the account in `slot`; `name` names it in the outcome.
    fn logout(
        &mut self,
        runner: Runner,
        slot: String,
        name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = backend(cx).store(runner) else {
            return;
        };
        let key = (runner, slot.clone());
        if !self.logging_out.insert(key.clone()) {
            return;
        }
        cx.notify();
        let task = irori_ui::run(cx, async move { store.account_logout(&slot).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.logging_out.remove(&key);
                let store = runner.store_name();
                match result {
                    Ok(()) => {
                        backend(cx).forget_expired_login(runner);
                        let message = match &name {
                            Some(name) => t_args(
                                "manage.accounts.logged_out_account",
                                &[("name", name), ("store", store)],
                            ),
                            None => t_args("manage.accounts.logged_out", &[("store", store)]),
                        };
                        notify_success(message, window, cx);
                        irori_ui::models::refresh_library(false, cx);
                    }
                    Err(e) => notify_error(
                        t_args(
                            "manage.accounts.logout_failed",
                            &[("store", runner.store_name())],
                        ),
                        &e,
                        window,
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn hide_button(runner: Runner) -> Option<AnyElement> {
        ShownStores::can_hide(runner).then(|| {
            let tip_id = ElementId::Name(format!("hide-tip-{}", runner.id()).into());
            div()
                .id(tip_id)
                .tooltip(move |window, cx| {
                    plain_tooltip(t("manage.accounts.hide_tooltip"), window, cx)
                })
                .child(
                    Button::new("hide")
                        .ghost()
                        .icon(IconName::EyeOff)
                        .label(t("manage.accounts.hide"))
                        .on_click(move |_, _, cx| {
                            backend_op(cx, move |b| {
                                b.settings.update_app(|s| s.ui.stores.set(runner, false))
                            })
                        }),
                )
                .into_any_element()
        })
    }

    fn render_inactive_card(&self, card_data: &StoreCard, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let runner = card_data.runner;
        let state_tag = if card_data.hidden {
            Tag::secondary()
                .small()
                .child(t("manage.accounts.state.hidden"))
        } else {
            Tag::secondary()
                .small()
                .child(t("manage.accounts.state.turned_off"))
        };
        let description = if card_data.hidden && card_data.local {
            t("manage.accounts.local_hidden_detail")
        } else if card_data.hidden {
            t("manage.accounts.hidden_detail")
        } else {
            t_args(
                "manage.accounts.turned_off",
                &[("store", runner.store_name())],
            )
        };
        let action: AnyElement = if card_data.hidden {
            Button::new("show")
                .small()
                .outline()
                .icon(IconName::Eye)
                .label(t("manage.accounts.show"))
                .on_click(move |_, _, cx| {
                    backend_op(cx, move |b| {
                        b.settings.update_app(|s| s.ui.stores.set(runner, true))
                    })
                })
                .into_any_element()
        } else {
            Button::new("enable")
                .small()
                .outline()
                .label(t("manage.accounts.turn_on"))
                .on_click(move |_, _, cx| {
                    backend_op(cx, move |b| {
                        b.settings
                            .update_app(|s| set_store_enabled(s, runner, true))
                    })
                })
                .into_any_element()
        };
        card(cx)
            .id(ElementId::Name(format!("account-{}", runner.id()).into()))
            .debug_selector(move || format!("account-{}", runner.id()))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(store_icon(runner).text_color(theme.muted_foreground))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_semibold()
                            .truncate()
                            .child(runner.store_name()),
                    )
                    .child(state_tag),
            )
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(description),
            )
            .child(h_flex().pt_1().child(action))
            .into_any_element()
    }

    fn render_card(
        &self,
        card_data: &StoreCard,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if card_data.hidden || card_data.disabled {
            return self.render_inactive_card(card_data, cx);
        }
        if card_data.local {
            return self.render_local_card(card_data, window, cx);
        }
        let (muted, border, radius, background, warning) = {
            let theme = cx.theme();
            (
                theme.muted_foreground,
                theme.border,
                theme.radius,
                theme.background,
                theme.warning,
            )
        };
        let runner = card_data.runner;
        let any_expired = card_data.accounts.iter().any(|a| a.expired.is_some());
        let state_tag = if card_data.expired.is_some() || any_expired {
            Tag::warning()
                .small()
                .child(t("manage.accounts.state.expired"))
        } else if card_data.user.is_some() {
            Tag::success()
                .small()
                .child(t("manage.accounts.state.signed_in"))
        } else {
            Tag::secondary()
                .small()
                .child(t("manage.accounts.state.signed_out"))
        };
        let detail: AnyElement = if !card_data.accounts.is_empty() {
            let several = card_data.accounts.len() > 1;
            v_flex()
                .gap_2p5()
                .children(
                    card_data
                        .accounts
                        .iter()
                        .map(|account| self.render_account(runner, account, several, cx)),
                )
                .into_any_element()
        } else if let Some(reason) = &card_data.expired {
            v_flex()
                .gap_1()
                .p_3()
                .rounded(radius)
                .bg(background)
                .border_1()
                .border_color(warning)
                .child(
                    h_flex()
                        .gap_2()
                        .items_start()
                        .child(
                            div().pt_0p5().child(
                                Icon::new(IconName::TriangleAlert)
                                    .small()
                                    .text_color(warning),
                            ),
                        )
                        .child(div().flex_1().min_w_0().text_sm().child(t_args(
                            "manage.accounts.expired_hint",
                            &[("store", runner.store_name())],
                        ))),
                )
                .child(div().text_xs().text_color(muted).child(reason.clone()))
                .into_any_element()
        } else {
            div()
                .text_sm()
                .text_color(muted)
                .child(t_args(
                    "manage.accounts.login_hint",
                    &[("store", runner.short_name())],
                ))
                .into_any_element()
        };
        let hide = Self::hide_button(runner);
        let signed_in = card_data.accounts.iter().any(|a| a.signed_in);
        let primary_actions: AnyElement = if signed_in {
            h_flex()
                .flex_wrap()
                .gap_2()
                .when(card_data.can_add, |row| {
                    row.child(
                        Button::new("add-account")
                            .debug_selector(|| "add-account".into())
                            .outline()
                            .icon(IconName::Plus)
                            .label(t("manage.accounts.add_account"))
                            .disabled(!self.online)
                            .on_click(move |_, window, cx| {
                                open_account_login_dialog(runner, None, window, cx)
                            }),
                    )
                })
                .into_any_element()
        } else {
            let label = if card_data.expired.is_some() {
                t("manage.accounts.login_again")
            } else {
                t("manage.accounts.login")
            };
            Button::new("login")
                .primary()
                .icon(IconName::LogIn)
                .label(label)
                .disabled(!self.online)
                .on_click(move |_, window, cx| open_login_dialog(runner, window, cx))
                .into_any_element()
        };
        let footer = h_flex()
            .w_full()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .pt_2()
            .border_t_1()
            .border_color(border)
            .child(primary_actions)
            .children(hide)
            .into_any_element();
        Self::card_frame(runner, vec![state_tag], detail, footer, cx)
    }

    /// One account of a store: its name, what it holds and its actions.
    fn render_account(
        &self,
        runner: Runner,
        account: &StoreAccount,
        several: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let key = (runner, account.slot.clone());
        let refreshing = self.refreshing.contains(&key);
        let logging_out = self.logging_out.contains(&key);
        let name = account_name(account);
        let (detail, warn) = if account.expired.is_some() {
            (t("manage.accounts.account_expired"), true)
        } else if account.signed_in {
            let games = match account.games {
                1 => t("manage.accounts.games_one"),
                n => t_args("manage.accounts.games_other", &[("count", &n.to_string())]),
            };
            (games, false)
        } else {
            let installed = match account.installed {
                1 => t("manage.accounts.signed_out_installed_one"),
                n => t_args(
                    "manage.accounts.signed_out_installed_other",
                    &[("count", &n.to_string())],
                ),
            };
            (installed, false)
        };
        let login = |id: &'static str, label: SharedString, primary: bool| {
            let slot = account.slot.clone();
            Button::new(id)
                .small()
                .when(primary, |b| b.primary())
                .icon(IconName::LogIn)
                .label(label)
                .disabled(!self.online)
                .on_click(move |_, window, cx| {
                    open_account_login_dialog(runner, Some(slot.clone()), window, cx)
                })
        };
        let logout = {
            let account = account.clone();
            Button::new("logout")
                .small()
                .ghost()
                .icon(IconName::LogOut)
                .tooltip(t("manage.accounts.logout"))
                .loading(logging_out)
                .disabled(logging_out)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.confirm_logout(runner, account.clone(), several, window, cx)
                }))
        };
        let actions = if account.expired.is_some() {
            h_flex()
                .gap_1()
                .child(login("login-again", t("manage.accounts.login_again"), true))
                .child(logout)
        } else if account.signed_in {
            let slot = account.slot.clone();
            h_flex()
                .gap_1()
                .when(
                    runner == Runner::Xbox
                        && account.slot == irori_core::store::MAIN_ACCOUNT
                        && crate::xbox::xbox_offline_available(cx),
                    |row| {
                        row.child(
                            Button::new("xbox-offline-play")
                                .small()
                                .ghost()
                                .icon(IconName::HardDrive)
                                .label(t("manage.xbox.offline.button"))
                                .on_click(|_, window, cx| {
                                    crate::xbox::open_offline_play_dialog(window, cx)
                                }),
                        )
                    },
                )
                .child(
                    Button::new("refresh")
                        .small()
                        .ghost()
                        .icon(IconName::RefreshCw)
                        .tooltip(t("manage.accounts.refresh"))
                        .loading(refreshing)
                        .disabled(!self.online || refreshing)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.refresh(runner, slot.clone(), window, cx)
                        })),
                )
                .child(logout)
        } else {
            h_flex()
                .gap_1()
                .child(login("login", t("manage.accounts.login"), false))
        };
        let id = format!("account-{}-{}", runner.id(), account.slot);
        h_flex()
            .id(ElementId::Name(id.clone().into()))
            .debug_selector(move || id.clone())
            .gap_2p5()
            .px_3()
            .py_2p5()
            .rounded(theme.radius)
            .bg(theme.background)
            .border_1()
            .border_color(if warn { theme.warning } else { theme.border })
            .items_center()
            .child(
                Icon::new(IconName::CircleUser)
                    .text_color(if warn {
                        theme.warning
                    } else {
                        theme.muted_foreground
                    })
                    .flex_shrink_0(),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_medium().truncate().child(name))
                    .child(
                        div()
                            .text_xs()
                            .text_color(if warn {
                                theme.warning
                            } else {
                                theme.muted_foreground
                            })
                            .child(detail),
                    ),
            )
            .child(actions.flex_shrink_0())
            .into_any_element()
    }

    fn card_frame(
        runner: Runner,
        state_tags: Vec<Tag>,
        detail: AnyElement,
        actions: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        card(cx)
            .id(ElementId::Name(format!("account-{}", runner.id()).into()))
            .debug_selector(move || format!("account-{}", runner.id()))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(store_icon(runner).text_color(theme.muted_foreground))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_semibold()
                            .truncate()
                            .child(runner.store_name()),
                    )
                    .child(h_flex().gap_1p5().flex_wrap().children(state_tags)),
            )
            // Takes the card's spare height, so the actions sit at the bottom
            // of every card in a row.
            .child(div().flex_1().min_h_10().child(detail))
            .child(actions)
            .into_any_element()
    }

    /// A store detected locally (Steam): installed games on disk, optional
    /// web/API account connection, client library reading, and rescan/hide.
    fn render_local_card(
        &self,
        card_data: &StoreCard,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let runner = card_data.runner;
        let found = card_data.user.is_some();
        let refreshing = self.refreshing.contains(&(runner, STORE_WIDE.to_string()));
        let muted = theme.muted_foreground;
        let account_expired =
            card_data.expired.is_some() || card_data.accounts.iter().any(|a| a.expired.is_some());
        let account_signed_in = card_data.accounts.iter().any(|a| a.signed_in);
        let mut state_tags = Vec::new();
        if found {
            state_tags.push(
                Tag::success()
                    .small()
                    .child(t("manage.accounts.state.local")),
            );
        } else {
            state_tags.push(
                Tag::secondary()
                    .small()
                    .child(t("manage.accounts.state.not_found")),
            );
        }
        if account_expired {
            state_tags.push(
                Tag::warning()
                    .small()
                    .child(t("manage.accounts.state.expired")),
            );
        } else if account_signed_in {
            state_tags.push(
                Tag::success()
                    .small()
                    .child(t("manage.accounts.state.signed_in")),
            );
        }

        let local_summary = if found {
            v_flex()
                .gap_0p5()
                .child(div().font_medium().child(match card_data.games {
                    1 => t("manage.accounts.installed_one"),
                    n => t_args(
                        "manage.accounts.installed_other",
                        &[("count", &n.to_string())],
                    ),
                }))
                .child(div().text_sm().text_color(muted).child(t_args(
                    "manage.accounts.local_hint",
                    &[("store", runner.store_name())],
                )))
                .into_any_element()
        } else {
            div()
                .text_sm()
                .text_color(muted)
                .child(t_args(
                    "manage.accounts.local_missing",
                    &[("store", runner.store_name())],
                ))
                .into_any_element()
        };

        let footer = h_flex()
            .w_full()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_2()
            .pt_2()
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .id(ElementId::Name(
                        format!("rescan-tip-{}", runner.id()).into(),
                    ))
                    .tooltip(move |window, cx| {
                        plain_tooltip(t("manage.accounts.rescan_tooltip"), window, cx)
                    })
                    .child(
                        Button::new("rescan")
                            .icon(IconName::RefreshCw)
                            .label(t("manage.accounts.rescan"))
                            .loading(refreshing)
                            .disabled(refreshing)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.refresh(runner, STORE_WIDE.to_string(), window, cx)
                            })),
                    ),
            )
            .children(Self::hide_button(runner))
            .into_any_element();

        let detail = if runner.has_optional_account() {
            let reading = self.steam_ops.read(cx).reading;
            let optional_panel = self.render_optional_account(card_data, cx);
            let client_panel = card_data
                .steam_client
                .as_ref()
                .filter(|s| s.steam_found)
                .map(|s| crate::steam_library::render_section(s, reading, cx));
            let integrations: AnyElement = match client_panel {
                Some(client) => card_grid_capped(
                    &self.steam_sub_width,
                    STEAM_SUB_REM,
                    2,
                    vec![optional_panel, client],
                    window,
                )
                .into_any_element(),
                None => optional_panel,
            };
            v_flex()
                .gap_3()
                .child(local_summary)
                .children(card_data.steam_hidden.clone().map(|text| {
                    h_flex()
                        .debug_selector(|| "steam-hidden-note".into())
                        .gap_1p5()
                        .items_start()
                        .text_xs()
                        .text_color(muted)
                        .child(div().pt_0p5().child(Icon::new(IconName::Info).xsmall()))
                        .child(div().flex_1().min_w_0().child(text))
                }))
                .children(card_data.steam_waiting.clone().map(|text| {
                    h_flex()
                        .debug_selector(|| "steam-shortcuts-waiting".into())
                        .gap_2()
                        .min_w_0()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(Icon::new(IconName::Clock).small())
                        .child(div().min_w_0().child(text))
                }))
                .child(integrations)
                .into_any_element()
        } else {
            local_summary
        };
        Self::card_frame(runner, state_tags, detail, footer, cx)
    }

    /// The account part of a store that works without one (Steam): who is
    /// logged in and how many games the account adds, or why to log in.
    fn render_optional_account(&self, card_data: &StoreCard, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let runner = card_data.runner;
        let store = runner.store_name();
        let muted = theme.muted_foreground;
        let account = card_data.accounts.iter().find(|a| a.signed_in).cloned();
        let logging_out = account
            .as_ref()
            .is_some_and(|a| self.logging_out.contains(&(runner, a.slot.clone())));
        let expired = card_data.expired.is_some();

        let body: AnyElement = match (&card_data.expired, account) {
            (Some(reason), account) => v_flex()
                .gap_2p5()
                .items_start()
                .child(
                    h_flex()
                        .gap_2()
                        .items_start()
                        .child(
                            div().pt_0p5().child(
                                Icon::new(IconName::TriangleAlert)
                                    .small()
                                    .text_color(theme.warning),
                            ),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_0p5()
                                .child(div().text_sm().child(t_args(
                                    "manage.accounts.expired_hint",
                                    &[("store", store)],
                                )))
                                .when(!reason.is_empty(), |col| {
                                    col.child(
                                        div().text_xs().text_color(muted).child(reason.clone()),
                                    )
                                }),
                        ),
                )
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new("optional-login")
                                .small()
                                .primary()
                                .icon(IconName::LogIn)
                                .label(t("manage.accounts.login_again"))
                                .on_click(move |_, window, cx| {
                                    open_login_dialog(runner, window, cx)
                                }),
                        )
                        .when_some(account, |row, acc| {
                            row.child(
                                Button::new("optional-logout")
                                    .small()
                                    .ghost()
                                    .icon(IconName::LogOut)
                                    .label(t("manage.accounts.logout"))
                                    .loading(logging_out)
                                    .disabled(logging_out)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.confirm_logout(runner, acc.clone(), false, window, cx)
                                    })),
                            )
                        }),
                )
                .into_any_element(),
            (None, Some(account)) => {
                let name = account_name(&account);
                let games = match account.games {
                    1 => t("manage.accounts.optional_games_one"),
                    n => t_args(
                        "manage.accounts.optional_games_other",
                        &[("count", &n.to_string())],
                    ),
                };
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .min_w_0()
                                    .child(Icon::new(IconName::CircleUser).small())
                                    .child(div().min_w_0().truncate().font_medium().child(name)),
                            )
                            .child(div().text_sm().text_color(muted).child(games)),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_shrink_0()
                            .when(
                                runner == Runner::Steam
                                    && crate::friends::steam_friends_available(cx),
                                |row| {
                                    row.child(
                                        Button::new("optional-friends")
                                            .small()
                                            .ghost()
                                            .icon(IconName::Users)
                                            .label(t("manage.friends.button"))
                                            .on_click(|_, window, cx| {
                                                crate::friends::open_steam_friends_dialog(
                                                    window, cx,
                                                )
                                            }),
                                    )
                                },
                            )
                            .child(
                                Button::new("optional-logout")
                                    .small()
                                    .ghost()
                                    .icon(IconName::LogOut)
                                    .label(t("manage.accounts.logout"))
                                    .loading(logging_out)
                                    .disabled(logging_out)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.confirm_logout(
                                            runner,
                                            account.clone(),
                                            false,
                                            window,
                                            cx,
                                        )
                                    })),
                            ),
                    )
                    .into_any_element()
            }
            (None, None) => v_flex()
                .flex_1()
                .justify_between()
                .gap_3()
                .items_start()
                .child(div().text_sm().text_color(muted).child(t_args(
                    "manage.accounts.optional_login_hint",
                    &[("store", store)],
                )))
                .child(
                    Button::new("optional-login")
                        .small()
                        .outline()
                        .icon(IconName::LogIn)
                        .label(t("manage.accounts.login"))
                        .on_click(move |_, window, cx| open_login_dialog(runner, window, cx)),
                )
                .into_any_element(),
        };

        inset_panel(cx)
            .when(expired, |panel| panel.border_color(theme.warning))
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .child(t("manage.accounts.steam_account_title")),
                    )
                    .child(info_tip(
                        "steam-account",
                        t("manage.accounts.steam_account_tip"),
                        cx,
                    )),
            )
            .child(body)
            .into_any_element()
    }
}

impl Render for AccountsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.cards.clone();
        let expired_stores: Vec<&str> = snapshot
            .iter()
            .filter(|c| {
                !c.hidden
                    && !c.disabled
                    && (c.expired.is_some() || c.accounts.iter().any(|a| a.expired.is_some()))
            })
            .map(|c| c.runner.store_name())
            .collect();

        let mut connected_cards = Vec::new();
        let mut connected_steam: Option<AnyElement> = None;
        let mut available_cards = Vec::new();
        let mut inactive_cards = Vec::new();

        for card_data in &snapshot {
            let rendered = self.render_card(card_data, window, cx);
            match classify_card(card_data) {
                AccountSection::Connected => {
                    if card_data.runner == Runner::Steam {
                        connected_steam = Some(rendered);
                    } else {
                        connected_cards.push(rendered);
                    }
                }
                AccountSection::Available => available_cards.push(rendered),
                AccountSection::Inactive => inactive_cards.push(rendered),
            }
        }

        let has_connected = !connected_cards.is_empty() || connected_steam.is_some();

        v_flex()
            .id("accounts-page")
            .size_full()
            .min_h_0()
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .w_full()
                    .max_w(rems(PAGE_MAX_WIDTH_REM))
                    .px_6()
                    .py_6()
                    .gap_6()
                    .child(
                        PageHeader::new(t("manage.accounts.title"))
                            .description(t("manage.accounts.description")),
                    )
                    .when(!self.online, |this| {
                        this.child(Alert::warning(
                            "accounts-offline",
                            t("manage.accounts.offline"),
                        ))
                    })
                    .when(!expired_stores.is_empty(), |this| {
                        this.child(Alert::warning(
                            "accounts-expired-banner",
                            t_args(
                                "manage.accounts.expired_banner",
                                &[("stores", &expired_stores.join(", "))],
                            ),
                        ))
                    })
                    .when(has_connected, |this| {
                        this.child(
                            v_flex()
                                .gap_3()
                                .child(SectionTitle::new(t("manage.accounts.section.connected")))
                                .when(!connected_cards.is_empty(), |sec| {
                                    sec.child(card_grid_capped(
                                        &self.connected_grid_width,
                                        CONNECTED_CARD_REM,
                                        2,
                                        connected_cards,
                                        window,
                                    ))
                                })
                                .children(connected_steam),
                        )
                    })
                    .when(!available_cards.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .gap_3()
                                .child(SectionTitle::new(t("manage.accounts.section.available")))
                                .child(card_grid_capped(
                                    &self.available_grid_width,
                                    COMPACT_CARD_REM,
                                    3,
                                    available_cards,
                                    window,
                                )),
                        )
                    })
                    .when(!inactive_cards.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .gap_3()
                                .child(SectionTitle::new(t("manage.accounts.section.inactive")))
                                .child(card_grid_capped(
                                    &self.inactive_grid_width,
                                    COMPACT_CARD_REM,
                                    3,
                                    inactive_cards,
                                    window,
                                )),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_card(runner: Runner) -> StoreCard {
        StoreCard {
            runner,
            user: None,
            games: 0,
            disabled: false,
            hidden: false,
            expired: None,
            local: !runner.has_account(),
            accounts: Vec::new(),
            can_add: false,
            steam_client: None,
            steam_hidden: None,
            steam_waiting: None,
        }
    }

    #[test]
    fn classifies_stores_by_state() {
        let mut connected = empty_card(Runner::Gog);
        connected.user = Some("player".into());
        assert_eq!(classify_card(&connected), AccountSection::Connected);

        let available = empty_card(Runner::Nile);
        assert_eq!(classify_card(&available), AccountSection::Available);

        let mut hidden = connected.clone();
        hidden.hidden = true;
        assert_eq!(classify_card(&hidden), AccountSection::Inactive);

        let mut turned_off = empty_card(Runner::Itch);
        turned_off.disabled = true;
        assert_eq!(classify_card(&turned_off), AccountSection::Inactive);

        let steam_missing = empty_card(Runner::Steam);
        assert_eq!(classify_card(&steam_missing), AccountSection::Available);

        let mut steam_expired = empty_card(Runner::Steam);
        steam_expired.user = Some("On disk".into());
        steam_expired.expired = Some("Token expired".into());
        assert_eq!(classify_card(&steam_expired), AccountSection::Connected);
    }
}
