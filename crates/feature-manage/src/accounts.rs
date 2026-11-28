//! Store accounts ([`Route::Accounts`](irori_ui::Route::Accounts)): every
//! account of each store with its game count, sync / log out / add-account
//! actions, Steam's unified local/client/account sources, and compact tiles
//! for stores not yet connected.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
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
/// Narrowest an available or inactive store tile gets before dropping a column, in rem.
const COMPACT_CARD_REM: f32 = 18.;
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

#[derive(Clone, Copy)]
enum StatusTone {
    Connected,
    Local,
    Warning,
    Muted,
}

/// Low-glare status pill with a leading indicator dot, using theme tokens only.
fn status_pill(label: SharedString, tone: StatusTone, cx: &App) -> Div {
    let theme = cx.theme();
    let (dot, fg, bg, border) = match tone {
        StatusTone::Connected => (
            theme.success,
            theme.success,
            theme.success.opacity(0.12),
            theme.success.opacity(0.24),
        ),
        StatusTone::Local => (
            theme.info,
            theme.info,
            theme.info.opacity(0.12),
            theme.info.opacity(0.24),
        ),
        StatusTone::Warning => (
            theme.warning,
            theme.warning,
            theme.warning.opacity(0.14),
            theme.warning.opacity(0.28),
        ),
        StatusTone::Muted => (
            theme.muted_foreground,
            theme.muted_foreground,
            theme.muted.opacity(0.55),
            theme.border,
        ),
    };
    h_flex()
        .items_center()
        .gap_1p5()
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(bg)
        .border_1()
        .border_color(border)
        .text_xs()
        .font_medium()
        .text_color(fg)
        .flex_shrink_0()
        .child(div().w_1p5().h_1p5().rounded_full().bg(dot))
        .child(label)
}

/// Framed icon container for a store mark.
fn store_icon_tile(runner: Runner, muted_icon: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let color = if muted_icon {
        theme.muted_foreground
    } else {
        theme.foreground
    };
    div()
        .w_8()
        .h_8()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme.radius)
        .bg(theme.background)
        .border_1()
        .border_color(theme.border)
        .flex_shrink_0()
        .child(store_icon(runner).text_color(color))
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
    /// Steam: full explanation of games hidden in Steam (shown in tooltip).
    pub steam_hidden: Option<SharedString>,
    /// Steam: compact header chip label for games hidden in Steam.
    pub steam_hidden_chip: Option<SharedString>,
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
        .map(|&runner| {
            let steam_hidden_state =
                (runner == Runner::Steam).then(|| crate::steam_library::steam_hidden_games(cx));
            let steam_hidden_state = steam_hidden_state.flatten();
            StoreCard {
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
                steam_hidden: steam_hidden_state.as_ref().and_then(|s| {
                    crate::steam_library::hidden_games_text(s, library.snapshot.hidden_locked)
                }),
                steam_hidden_chip: steam_hidden_state.as_ref().and_then(|s| {
                    crate::steam_library::hidden_games_chip(s, library.snapshot.hidden_locked)
                }),
                steam_waiting: (runner == Runner::Steam)
                    .then(|| steam_waiting_text(b.steam_shortcuts_waiting()))
                    .flatten(),
            }
        })
        .collect()
}

/// Consolidated summary text for the page header (e.g. `3 stores · 1,521 games`).
pub(crate) fn connected_summary_text(cards: &[StoreCard]) -> Option<SharedString> {
    let mut stores = 0usize;
    let mut games = 0usize;
    for card in cards {
        if classify_card(card) == AccountSection::Connected {
            stores += 1;
            games += card.games;
        }
    }
    if stores == 0 {
        return None;
    }
    let stores_label = match stores {
        1 => t("manage.accounts.stores_one"),
        n => t_args("manage.accounts.stores_other", &[("count", &n.to_string())]),
    };
    let games_label = match games {
        1 => t("manage.accounts.games_one"),
        n => t_args("manage.accounts.games_other", &[("count", &n.to_string())]),
    };
    Some(t_args(
        "manage.accounts.summary",
        &[("stores", &stores_label), ("games", &games_label)],
    ))
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
    available_grid_width: Rc<Cell<Pixels>>,
    inactive_grid_width: Rc<Cell<Pixels>>,
    online: bool,
    syncing_all: bool,
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
            available_grid_width: Rc::new(Cell::new(px(0.))),
            inactive_grid_width: Rc::new(Cell::new(px(0.))),
            online: true,
            syncing_all: false,
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

    /// Refresh every connected store in one action.
    fn refresh_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.syncing_all || !self.online {
            return;
        }
        self.syncing_all = true;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            b.refresh_all(true).await;
            Ok(())
        });
        cx.spawn_in(window, async move |this, cx| {
            let _ = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.syncing_all = false;
                notify_success(t("manage.accounts.synced_all"), window, cx);
                cx.notify();
            });
        })
        .detach();
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

    /// Store-level overflow menu (add another account and/or hide from library).
    fn store_overflow_menu(&self, runner: Runner, can_add: bool) -> Option<AnyElement> {
        let can_hide = ShownStores::can_hide(runner);
        if !can_add && !can_hide {
            return None;
        }
        let online = self.online;
        let menu_id = ElementId::Name(format!("store-menu-{}", runner.id()).into());
        Some(
            Button::new(menu_id)
                .xsmall()
                .ghost()
                .icon(IconName::Ellipsis)
                .tooltip(t("manage.accounts.more_options"))
                .dropdown_menu(move |mut menu, _, _| {
                    if can_add {
                        menu = menu.item(
                            PopupMenuItem::new(t("manage.accounts.add_account"))
                                .icon(IconName::Plus)
                                .disabled(!online)
                                .on_click(move |_, window, cx| {
                                    open_account_login_dialog(runner, None, window, cx)
                                }),
                        );
                    }
                    if can_hide {
                        if can_add {
                            menu = menu.separator();
                        }
                        menu = menu.item(
                            PopupMenuItem::new(t("manage.accounts.hide"))
                                .icon(IconName::EyeOff)
                                .on_click(move |_, _, cx| {
                                    backend_op(cx, move |b| {
                                        b.settings.update_app(|s| s.ui.stores.set(runner, false))
                                    })
                                }),
                        );
                    }
                    menu
                })
                .into_any_element(),
        )
    }

    /// Compact horizontal tile for a disconnected store ready to connect.
    fn render_available_card(&self, card_data: &StoreCard, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let runner = card_data.runner;
        let overflow = self.store_overflow_menu(runner, false);
        h_flex()
            .id(ElementId::Name(format!("account-{}", runner.id()).into()))
            .debug_selector(move || format!("account-{}", runner.id()))
            .w_full()
            .px_4()
            .py_3p5()
            .gap_3()
            .items_center()
            .justify_between()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(theme.group_box)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_3()
                    .items_center()
                    .child(store_icon_tile(runner, true, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_semibold()
                            .truncate()
                            .child(runner.store_name()),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .flex_shrink_0()
                    .child(
                        Button::new("login")
                            .small()
                            .outline()
                            .icon(IconName::LogIn)
                            .label(t("manage.accounts.connect"))
                            .disabled(!self.online)
                            .on_click(move |_, window, cx| open_login_dialog(runner, window, cx)),
                    )
                    .children(overflow),
            )
            .into_any_element()
    }

    fn render_inactive_card(&self, card_data: &StoreCard, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let runner = card_data.runner;
        let state_label = if card_data.hidden {
            t("manage.accounts.state.hidden")
        } else {
            t("manage.accounts.state.turned_off")
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
                    .gap_2p5()
                    .items_center()
                    .child(store_icon_tile(runner, true, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_semibold()
                            .truncate()
                            .child(runner.store_name()),
                    )
                    .child(status_pill(state_label, StatusTone::Muted, cx)),
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

    fn render_card(&self, card_data: &StoreCard, cx: &mut Context<Self>) -> AnyElement {
        if card_data.hidden || card_data.disabled {
            return self.render_inactive_card(card_data, cx);
        }
        if card_data.local {
            return self.render_local_card(card_data, cx);
        }
        if classify_card(card_data) == AccountSection::Available {
            return self.render_available_card(card_data, cx);
        }

        let (muted, radius, background, warning) = {
            let theme = cx.theme();
            (
                theme.muted_foreground,
                theme.radius,
                theme.background,
                theme.warning,
            )
        };
        let runner = card_data.runner;
        let any_expired = card_data.accounts.iter().any(|a| a.expired.is_some());
        let state_pill = if card_data.expired.is_some() || any_expired {
            status_pill(t("manage.accounts.state.expired"), StatusTone::Warning, cx)
        } else {
            status_pill(
                t("manage.accounts.state.signed_in"),
                StatusTone::Connected,
                cx,
            )
        };
        let signed_in = card_data.accounts.iter().any(|a| a.signed_in);
        let overflow = self.store_overflow_menu(runner, signed_in && card_data.can_add);

        let detail: AnyElement = if !card_data.accounts.is_empty() {
            let several = card_data.accounts.len() > 1;
            v_flex()
                .gap_2()
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
                .when(!reason.is_empty(), |col| {
                    col.child(div().text_xs().text_color(muted).child(reason.clone()))
                })
                .into_any_element()
        } else {
            div().into_any_element()
        };

        let multi_summary = (card_data.accounts.len() > 1).then(|| {
            let accounts_label = t_args(
                "manage.accounts.accounts_count_other",
                &[("count", &card_data.accounts.len().to_string())],
            );
            let total_games: usize = card_data.accounts.iter().map(|a| a.games).sum();
            let games_label = match total_games {
                1 => t("manage.accounts.games_one"),
                n => t_args("manage.accounts.games_other", &[("count", &n.to_string())]),
            };
            div()
                .text_xs()
                .text_color(muted)
                .child(t_args(
                    "manage.accounts.accounts_summary",
                    &[("accounts", &accounts_label), ("games", &games_label)],
                ))
                .into_any_element()
        });

        let footer: Option<AnyElement> = if signed_in {
            (card_data.can_add || multi_summary.is_some()).then(|| {
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .when(card_data.can_add, |row| {
                        row.child(
                            Button::new("add-account")
                                .debug_selector(|| "add-account".into())
                                .small()
                                .ghost()
                                .icon(IconName::Plus)
                                .label(t("manage.accounts.add_account"))
                                .disabled(!self.online)
                                .on_click(move |_, window, cx| {
                                    open_account_login_dialog(runner, None, window, cx)
                                }),
                        )
                    })
                    .children(multi_summary)
                    .into_any_element()
            })
        } else {
            Some(
                h_flex()
                    .w_full()
                    .child(
                        Button::new("login")
                            .small()
                            .primary()
                            .icon(IconName::LogIn)
                            .label(t("manage.accounts.login_again"))
                            .disabled(!self.online)
                            .on_click(move |_, window, cx| open_login_dialog(runner, window, cx)),
                    )
                    .into_any_element(),
            )
        };

        card(cx)
            .id(ElementId::Name(format!("account-{}", runner.id()).into()))
            .debug_selector(move || format!("account-{}", runner.id()))
            .child(
                h_flex()
                    .gap_2p5()
                    .items_center()
                    .child(store_icon_tile(runner, false, cx))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .flex_wrap()
                            .child(div().font_semibold().truncate().child(runner.store_name()))
                            .child(state_pill),
                    )
                    .children(overflow),
            )
            .child(div().flex_1().child(detail))
            .children(footer)
            .into_any_element()
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
        let refreshing = self.refreshing.contains(&key) || self.syncing_all;
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
                .when(!primary, |b| b.outline())
                .icon(IconName::LogIn)
                .label(label)
                .disabled(!self.online)
                .on_click(move |_, window, cx| {
                    open_account_login_dialog(runner, Some(slot.clone()), window, cx)
                })
        };
        let account_overflow = {
            let view = cx.entity().downgrade();
            let account = account.clone();
            let menu_id =
                ElementId::Name(format!("acc-more-{}-{}", runner.id(), account.slot).into());
            Button::new(menu_id)
                .xsmall()
                .ghost()
                .icon(IconName::Ellipsis)
                .tooltip(t("manage.accounts.account_actions"))
                .loading(logging_out)
                .disabled(logging_out)
                .dropdown_menu(move |menu, _, _| {
                    let view = view.clone();
                    let account = account.clone();
                    menu.item(
                        PopupMenuItem::new(t("manage.accounts.logout"))
                            .icon(IconName::LogOut)
                            .disabled(logging_out)
                            .on_click(move |_, window, cx| {
                                let _ = view.update(cx, |this, cx| {
                                    this.confirm_logout(
                                        runner,
                                        account.clone(),
                                        several,
                                        window,
                                        cx,
                                    )
                                });
                            }),
                    )
                })
        };
        let actions = if account.expired.is_some() {
            h_flex()
                .gap_1()
                .items_center()
                .child(login("login-again", t("manage.accounts.login_again"), true))
                .child(account_overflow)
        } else if account.signed_in {
            let slot = account.slot.clone();
            h_flex()
                .gap_1()
                .items_center()
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
                        .label(t("manage.accounts.sync"))
                        .tooltip(t("manage.accounts.refresh"))
                        .loading(refreshing)
                        .disabled(!self.online || refreshing)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.refresh(runner, slot.clone(), window, cx)
                        })),
                )
                .child(account_overflow)
        } else {
            h_flex()
                .gap_1()
                .items_center()
                .child(login("login", t("manage.accounts.login"), false))
        };
        let avatar_bg = if warn {
            theme.warning.opacity(0.14)
        } else {
            theme.muted.opacity(0.55)
        };
        let avatar_border = if warn {
            theme.warning.opacity(0.32)
        } else {
            theme.border
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
                div()
                    .w_8()
                    .h_8()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(avatar_bg)
                    .border_1()
                    .border_color(avatar_border)
                    .flex_shrink_0()
                    .child(Icon::new(IconName::CircleUser).small().text_color(if warn {
                        theme.warning
                    } else {
                        theme.muted_foreground
                    })),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().font_medium().truncate().child(name))
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

    /// A store detected locally (Steam): unified source rows for local
    /// installs, Steam client library reads, and optional Steam account login.
    fn render_local_card(&self, card_data: &StoreCard, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let runner = card_data.runner;
        let found = card_data.user.is_some();
        let refreshing =
            self.refreshing.contains(&(runner, STORE_WIDE.to_string())) || self.syncing_all;
        let muted = theme.muted_foreground;
        let border = theme.border;
        let radius = theme.radius;
        let background = theme.background;
        let info_color = theme.info;
        let info_bg = theme.info.opacity(0.12);
        let info_border = theme.info.opacity(0.24);
        let muted_tile_bg = theme.muted.opacity(0.45);
        let account_expired =
            card_data.expired.is_some() || card_data.accounts.iter().any(|a| a.expired.is_some());
        let account_signed_in = card_data.accounts.iter().any(|a| a.signed_in);

        let state_pill = if account_expired {
            status_pill(t("manage.accounts.state.expired"), StatusTone::Warning, cx)
        } else if account_signed_in {
            status_pill(
                t("manage.accounts.state.signed_in"),
                StatusTone::Connected,
                cx,
            )
        } else if found {
            status_pill(t("manage.accounts.state.local"), StatusTone::Local, cx)
        } else {
            status_pill(t("manage.accounts.state.not_found"), StatusTone::Muted, cx)
        };

        let hidden_chip = card_data.steam_hidden.clone().map(|full_text| {
            let chip_label = card_data
                .steam_hidden_chip
                .clone()
                .unwrap_or_else(|| t("manage.steam_hidden.chip_unknown"));
            div()
                .id(ElementId::Name("steam-hidden-note".into()))
                .debug_selector(|| "steam-hidden-note".into())
                .tooltip(move |window, cx| plain_tooltip(full_text.clone(), window, cx))
                .child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .px_2()
                        .py_1()
                        .rounded(radius)
                        .text_xs()
                        .text_color(muted)
                        .child(Icon::new(IconName::Info).xsmall())
                        .child(chip_label),
                )
        });

        let overflow = self.store_overflow_menu(runner, false);

        let local_count_label = found.then(|| match card_data.games {
            1 => t("manage.accounts.installed_one"),
            n => t_args(
                "manage.accounts.installed_other",
                &[("count", &n.to_string())],
            ),
        });
        let local_subtitle = if found {
            t_args(
                "manage.accounts.local_hint",
                &[("store", runner.store_name())],
            )
        } else {
            t_args(
                "manage.accounts.local_missing",
                &[("store", runner.store_name())],
            )
        };

        let local_source_row = h_flex()
            .w_full()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .px_3p5()
            .py_3()
            .rounded(radius)
            .bg(background)
            .border_1()
            .border_color(border)
            .child(
                h_flex()
                    .flex_1()
                    .min_w(rems(16.))
                    .items_start()
                    .gap_3()
                    .child(
                        div()
                            .w_8()
                            .h_8()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(radius)
                            .bg(if found { info_bg } else { muted_tile_bg })
                            .border_1()
                            .border_color(if found { info_border } else { border })
                            .flex_shrink_0()
                            .child(Icon::new(IconName::HardDrive).small().text_color(if found {
                                info_color
                            } else {
                                muted
                            })),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .flex_wrap()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_medium()
                                            .child(t("manage.accounts.local_source_title")),
                                    )
                                    .when_some(local_count_label, |row, count| {
                                        row.child(
                                            div()
                                                .text_xs()
                                                .font_medium()
                                                .text_color(info_color)
                                                .child(count),
                                        )
                                    }),
                            )
                            .child(div().text_xs().text_color(muted).child(local_subtitle)),
                    ),
            )
            .child(
                div()
                    .id(ElementId::Name(
                        format!("rescan-tip-{}", runner.id()).into(),
                    ))
                    .flex_shrink_0()
                    .tooltip(move |window, cx| {
                        plain_tooltip(t("manage.accounts.rescan_tooltip"), window, cx)
                    })
                    .child(
                        Button::new("rescan")
                            .small()
                            .outline()
                            .icon(IconName::RefreshCw)
                            .label(t("manage.accounts.rescan"))
                            .loading(refreshing)
                            .disabled(refreshing)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.refresh(runner, STORE_WIDE.to_string(), window, cx)
                            })),
                    ),
            );

        let reading = self.steam_ops.read(cx).reading;
        let client_row = runner.has_optional_account().then(|| {
            card_data
                .steam_client
                .as_ref()
                .filter(|s| s.steam_found)
                .map(|s| crate::steam_library::render_section(s, reading, cx))
        });
        let optional_row = runner
            .has_optional_account()
            .then(|| self.render_optional_account(card_data, cx));

        card(cx)
            .id(ElementId::Name(format!("account-{}", runner.id()).into()))
            .debug_selector(move || format!("account-{}", runner.id()))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2p5()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_2p5()
                            .items_center()
                            .child(store_icon_tile(runner, !found && !account_signed_in, cx))
                            .child(div().font_semibold().child(runner.store_name()))
                            .child(state_pill),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .children(hidden_chip)
                            .children(overflow),
                    ),
            )
            .children(card_data.steam_waiting.clone().map(|text| {
                h_flex()
                    .debug_selector(|| "steam-shortcuts-waiting".into())
                    .gap_2()
                    .min_w_0()
                    .text_xs()
                    .text_color(muted)
                    .child(Icon::new(IconName::Clock).xsmall())
                    .child(div().min_w_0().child(text))
            }))
            .child(
                v_flex()
                    .gap_2()
                    .child(local_source_row)
                    .children(client_row.flatten())
                    .children(optional_row),
            )
            .into_any_element()
    }

    /// The account part of a store that works without one (Steam): who is
    /// logged in and how many games the account adds, or why to log in.
    fn render_optional_account(&self, card_data: &StoreCard, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let runner = card_data.runner;
        let store = runner.store_name();
        let muted = theme.muted_foreground;
        let border = theme.border;
        let radius = theme.radius;
        let background = theme.background;
        let warning = theme.warning;
        let success = theme.success;
        let account = card_data.accounts.iter().find(|a| a.signed_in).cloned();
        let logging_out = account
            .as_ref()
            .is_some_and(|a| self.logging_out.contains(&(runner, a.slot.clone())));

        match (&card_data.expired, account) {
            (Some(reason), account) => {
                let logout_menu = account.map(|acc| {
                    let view = cx.entity().downgrade();
                    Button::new("optional-more")
                        .xsmall()
                        .ghost()
                        .icon(IconName::Ellipsis)
                        .tooltip(t("manage.accounts.account_actions"))
                        .loading(logging_out)
                        .disabled(logging_out)
                        .dropdown_menu(move |menu, _, _| {
                            let view = view.clone();
                            let acc = acc.clone();
                            menu.item(
                                PopupMenuItem::new(t("manage.accounts.logout"))
                                    .icon(IconName::LogOut)
                                    .disabled(logging_out)
                                    .on_click(move |_, window, cx| {
                                        let _ = view.update(cx, |this, cx| {
                                            this.confirm_logout(
                                                runner,
                                                acc.clone(),
                                                false,
                                                window,
                                                cx,
                                            )
                                        });
                                    }),
                            )
                        })
                });
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .px_3p5()
                    .py_3()
                    .rounded(radius)
                    .bg(background)
                    .border_1()
                    .border_color(warning)
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w(rems(16.))
                            .items_start()
                            .gap_3()
                            .child(
                                div()
                                    .w_8()
                                    .h_8()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(radius)
                                    .bg(warning.opacity(0.14))
                                    .border_1()
                                    .border_color(warning.opacity(0.32))
                                    .flex_shrink_0()
                                    .child(
                                        Icon::new(IconName::TriangleAlert)
                                            .small()
                                            .text_color(warning),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(
                                        h_flex()
                                            .gap_1()
                                            .items_center()
                                            .child(
                                                div().text_sm().font_medium().child(t(
                                                    "manage.accounts.steam_account_title",
                                                )),
                                            )
                                            .child(info_tip(
                                                "steam-account",
                                                t("manage.accounts.steam_account_tip"),
                                                cx,
                                            )),
                                    )
                                    .child(div().text_xs().child(t_args(
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
                            .gap_1()
                            .items_center()
                            .flex_shrink_0()
                            .child(
                                Button::new("optional-login")
                                    .small()
                                    .primary()
                                    .icon(IconName::LogIn)
                                    .label(t("manage.accounts.login_again"))
                                    .disabled(!self.online)
                                    .on_click(move |_, window, cx| {
                                        open_login_dialog(runner, window, cx)
                                    }),
                            )
                            .children(logout_menu),
                    )
                    .into_any_element()
            }
            (None, Some(account)) => {
                let name = account_name(&account);
                let games = match account.games {
                    1 => t("manage.accounts.optional_games_one"),
                    n => t_args(
                        "manage.accounts.optional_games_other",
                        &[("count", &n.to_string())],
                    ),
                };
                let view = cx.entity().downgrade();
                let logout_acc = account.clone();
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .px_3p5()
                    .py_3()
                    .rounded(radius)
                    .bg(background)
                    .border_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w(rems(16.))
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .w_8()
                                    .h_8()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(success.opacity(0.12))
                                    .border_1()
                                    .border_color(success.opacity(0.24))
                                    .flex_shrink_0()
                                    .child(
                                        Icon::new(IconName::CircleUser).small().text_color(success),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .flex_wrap()
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_medium()
                                                    .truncate()
                                                    .child(name),
                                            )
                                            .child(
                                                div().text_xs().text_color(success).child(t(
                                                    "manage.accounts.steam_account_title",
                                                )),
                                            ),
                                    )
                                    .child(div().text_xs().text_color(muted).child(games)),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
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
                                Button::new("optional-more")
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Ellipsis)
                                    .tooltip(t("manage.accounts.account_actions"))
                                    .loading(logging_out)
                                    .disabled(logging_out)
                                    .dropdown_menu(move |menu, _, _| {
                                        let view = view.clone();
                                        let acc = logout_acc.clone();
                                        menu.item(
                                            PopupMenuItem::new(t("manage.accounts.logout"))
                                                .icon(IconName::LogOut)
                                                .disabled(logging_out)
                                                .on_click(move |_, window, cx| {
                                                    let _ = view.update(cx, |this, cx| {
                                                        this.confirm_logout(
                                                            runner,
                                                            acc.clone(),
                                                            false,
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                }),
                                        )
                                    }),
                            ),
                    )
                    .into_any_element()
            }
            (None, None) => h_flex()
                .w_full()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_3()
                .px_3p5()
                .py_3()
                .rounded(radius)
                .bg(background.opacity(0.6))
                .border_1()
                .border_color(border)
                .child(
                    h_flex()
                        .flex_1()
                        .min_w(rems(16.))
                        .items_start()
                        .gap_3()
                        .child(
                            div()
                                .w_8()
                                .h_8()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(radius)
                                .bg(theme.muted.opacity(0.45))
                                .border_1()
                                .border_color(border)
                                .flex_shrink_0()
                                .child(Icon::new(IconName::Cloud).small().text_color(muted)),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_0p5()
                                .child(
                                    h_flex()
                                        .flex_wrap()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_medium()
                                                .child(t("manage.accounts.steam_account_title")),
                                        )
                                        .child(
                                            div()
                                                .px_1p5()
                                                .py_0p5()
                                                .rounded(radius / 2.)
                                                .bg(theme.muted.opacity(0.6))
                                                .border_1()
                                                .border_color(border)
                                                .text_xs()
                                                .text_color(muted)
                                                .child(t("manage.accounts.optional_badge")),
                                        )
                                        .child(info_tip(
                                            "steam-account",
                                            t("manage.accounts.steam_account_tip"),
                                            cx,
                                        )),
                                )
                                .child(div().text_xs().text_color(muted).child(t_args(
                                    "manage.accounts.optional_login_hint",
                                    &[("store", store)],
                                ))),
                        ),
                )
                .child(
                    Button::new("optional-login")
                        .small()
                        .outline()
                        .icon(IconName::LogIn)
                        .label(t("manage.accounts.login"))
                        .disabled(!self.online)
                        .on_click(move |_, window, cx| open_login_dialog(runner, window, cx)),
                )
                .into_any_element(),
        }
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
            let rendered = self.render_card(card_data, cx);
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
        let summary_text = connected_summary_text(&snapshot);
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let summary_bg = theme.muted.opacity(0.6);
        let border = theme.border;

        let mut header = PageHeader::new(t("manage.accounts.title"))
            .description(t("manage.accounts.description"));
        if let Some(summary) = summary_text {
            header = header.action(
                div()
                    .debug_selector(|| "accounts-summary".into())
                    .px_2p5()
                    .py_1()
                    .rounded_full()
                    .bg(summary_bg)
                    .border_1()
                    .border_color(border)
                    .text_xs()
                    .font_medium()
                    .text_color(muted)
                    .child(summary),
            );
        }
        if has_connected {
            header = header.action(
                Button::new("accounts-sync-all")
                    .debug_selector(|| "accounts-sync-all".into())
                    .small()
                    .outline()
                    .icon(IconName::RefreshCw)
                    .label(t("manage.accounts.sync_all"))
                    .loading(self.syncing_all)
                    .disabled(!self.online || self.syncing_all)
                    .on_click(cx.listener(|this, _, window, cx| this.refresh_all(window, cx))),
            );
        }

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
                    .child(header)
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
                                .child(
                                    SectionTitle::new(t("manage.accounts.section.available"))
                                        .trailing(
                                            div()
                                                .text_xs()
                                                .text_color(muted)
                                                .child(t("manage.accounts.available_hint")),
                                        ),
                                )
                                .child(card_grid_capped(
                                    &self.available_grid_width,
                                    COMPACT_CARD_REM,
                                    2,
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
                                    2,
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
            steam_hidden_chip: None,
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

    #[test]
    fn summarizes_connected_stores_only() {
        crate::i18n::register();
        let mut epic = empty_card(Runner::Legendary);
        epic.user = Some("Player".into());
        epic.games = 588;

        let mut gog = empty_card(Runner::Gog);
        gog.user = Some("Player".into());
        gog.games = 439;

        let mut hidden = empty_card(Runner::Nile);
        hidden.user = Some("Player".into());
        hidden.hidden = true;
        hidden.games = 50;

        let disconnected = empty_card(Runner::Zoom);

        let summary = connected_summary_text(&[epic, gog, hidden, disconnected]).unwrap();
        assert!(summary.contains("2 stores"), "got {summary}");
        assert!(summary.contains("1027 games"), "got {summary}");
        assert_eq!(connected_summary_text(&[empty_card(Runner::Nile)]), None);
    }
}
