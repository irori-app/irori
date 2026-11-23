//! "Found on this computer": what the empty library shows instead of a bare
//! message. Steam with its state and the one action that state needs, the
//! emulators installed and the usual ROM folders found, the other
//! launchers Irori imports from, and the stores to log in to. Looking reads
//! the disk, so it runs off the UI thread when the view opens; nothing is
//! added or imported until the user clicks.
//!
//! The library draws it through `irori_ui::hooks::found_sources`, since
//! the library doesn't depend on the management screens.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Context, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, rems,
};
use irori_core::Runner;
use irori_ui::i18n::{t, t_args};
use irori_ui::{Route, models, navigate};

use super::launchers::{self, Imports, Launcher, import_status};
use crate::accounts::{StoreCard, store_cards, store_icon};
use crate::common::card;
use crate::login::open_login_dialog;

/// What Steam needs, as the empty library tells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SteamRow {
    /// Steam isn't on this computer.
    NotFound,
    /// Signed in to Irori: the whole library comes from the account.
    SignedIn(String),
    /// Steam is here and nothing reads its whole library yet: sign in, or
    /// (`client`) read it from the client.
    Local { client: bool },
}

/// Steam's row from the Steam store's state and the account.
pub(crate) fn steam_row(found: bool, user: Option<&str>, suggest_client: bool) -> SteamRow {
    match user {
        Some(user) => SteamRow::SignedIn(user.to_string()),
        None if !found => SteamRow::NotFound,
        None => SteamRow::Local {
            client: suggest_client,
        },
    }
}

/// The stores to offer a login for: those with accounts, turned on and
/// shown, not signed in. Steam has a row of its own.
pub(crate) fn stores_to_log_in(cards: &[StoreCard]) -> Vec<Runner> {
    cards
        .iter()
        .filter(|c| {
            c.runner != Runner::Steam && !c.local && !c.disabled && !c.hidden && c.user.is_none()
        })
        .map(|c| c.runner)
        .collect()
}

/// What the background look found.
struct Looked {
    /// Systems with ROMs not yet in the library.
    rom_systems: usize,
    /// Emulators installed.
    engines: usize,
}

/// The view the empty library shows.
pub struct FoundSourcesView {
    looked: Option<Looked>,
    launchers: Imports,
    _subscriptions: Vec<Subscription>,
}

/// Make the view for the library's empty state.
pub fn found_sources_view(window: &mut Window, cx: &mut App) -> AnyView {
    crate::i18n::register();
    cx.new(|cx| FoundSourcesView::new(window, cx)).into()
}

impl FoundSourcesView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let m = models(cx).clone();
        let b = irori_ui::backend(cx);
        let look = irori_ui::run(cx, async move {
            let statuses = b.engine_statuses().await;
            let (_, plan) = b.rom_sources().await;
            Ok(Looked {
                rom_systems: plan.add.len(),
                engines: statuses.iter().filter(|s| s.is_installed()).count(),
            })
        });
        let found = launchers::detect_in_background(cx);
        cx.spawn_in(window, async move |this, cx| {
            let looked = look.await.ok();
            let found = found.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.looked = looked.or(Some(Looked {
                    rom_systems: 0,
                    engines: 0,
                }));
                this.launchers = Imports::new(&found);
                cx.notify();
            });
        })
        .detach();
        Self {
            looked: None,
            launchers: Imports::default(),
            _subscriptions: vec![
                cx.observe(&m.accounts, |_, _, cx| cx.notify()),
                cx.observe(&m.settings, |_, _, cx| cx.notify()),
            ],
        }
    }

    fn run_import(&mut self, launcher: Launcher, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.launchers.start(launcher, cx) else {
            return;
        };
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Ok(summary) = &result {
                    irori_ui::models::refresh_library(false, cx);
                    irori_ui::lutris_summary::open_lutris_summary(summary.clone(), window, cx);
                }
                this.launchers.finish(launcher, result);
                cx.notify();
            });
        })
        .detach();
    }

    fn row(
        &self,
        id: &'static str,
        icon: IconName,
        title: SharedString,
        detail: SharedString,
        action: Option<AnyElement>,
        cx: &App,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        h_flex()
            .id(id)
            .gap_3()
            .py_2()
            .child(Icon::new(icon).text_color(muted))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().font_medium().child(title))
                    .child(div().text_sm().text_color(muted).child(detail)),
            )
            .when_some(action, |d, a| d.child(div().flex_shrink_0().child(a)))
            .into_any_element()
    }

    fn render_steam(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = crate::steam_client_library_state(cx)?;
        let user = models(cx)
            .accounts
            .read(cx)
            .user(Runner::Steam)
            .map(|u| u.display_name.trim().to_string());
        let online = models(cx).accounts.read(cx).online;
        let row = steam_row(state.steam_found, user.as_deref(), state.suggest());
        let (detail, action): (SharedString, Option<AnyElement>) = match row {
            SteamRow::NotFound => (t("manage.found.steam.not_found"), None),
            SteamRow::SignedIn(user) => (
                t_args("manage.found.steam.signed_in", &[("user", &user)]),
                None,
            ),
            SteamRow::Local { client } => (
                t("manage.found.steam.local"),
                Some(
                    h_flex()
                        .gap_2()
                        .when(client, |d| {
                            d.child(
                                Button::new("found-steam-client")
                                    .small()
                                    .label(t("manage.found.steam.read_client"))
                                    .on_click(|_, window, cx| {
                                        crate::open_steam_library_dialog(window, cx)
                                    }),
                            )
                        })
                        .child(
                            Button::new("found-steam-sign-in")
                                .small()
                                .primary()
                                .icon(IconName::LogIn)
                                .label(t("manage.found.steam.sign_in"))
                                .disabled(!online)
                                .on_click(|_, window, cx| {
                                    open_login_dialog(Runner::Steam, window, cx)
                                }),
                        )
                        .into_any_element(),
                ),
            ),
        };
        Some(self.row(
            "found-steam",
            store_icon(Runner::Steam),
            "Steam".into(),
            detail,
            action,
            cx,
        ))
    }

    fn render_emulation(&self, cx: &App) -> AnyElement {
        let Some(looked) = &self.looked else {
            return self.row(
                "found-emulation",
                IconName::Joystick,
                t("manage.found.emulation.title"),
                t("manage.rom_sources.looking"),
                Some(Spinner::new().small().into_any_element()),
                cx,
            );
        };
        let mut detail = Vec::new();
        match looked.engines {
            0 => {}
            1 => detail.push(t("manage.found.emulation.engines_one")),
            n => detail.push(t_args(
                "manage.found.emulation.engines_other",
                &[("count", &n.to_string())],
            )),
        }
        match looked.rom_systems {
            0 => detail.push(t("manage.found.emulation.no_roms")),
            1 => detail.push(t("manage.rom_sources.found_one")),
            n => detail.push(t_args(
                "manage.rom_sources.found_other",
                &[("count", &n.to_string())],
            )),
        }
        let action = if looked.rom_systems > 0 {
            Button::new("found-roms")
                .small()
                .primary()
                .label(t("manage.rom_sources.review"))
                .on_click(|_, window, cx| crate::emulators::open_rom_sources_dialog(window, cx))
        } else {
            Button::new("found-emulation-setup")
                .small()
                .label(t("manage.found.emulation.set_up"))
                .on_click(|_, window, cx| crate::open_emulation_setup(window, cx))
        };
        self.row(
            "found-emulation",
            IconName::Joystick,
            t("manage.found.emulation.title"),
            detail
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(" ")
                .into(),
            Some(action.into_any_element()),
            cx,
        )
    }

    fn render_stores(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let stores = stores_to_log_in(&store_cards(cx));
        if stores.is_empty() {
            return None;
        }
        let online = models(cx).accounts.read(cx).online;
        let buttons = h_flex()
            .gap_2()
            .flex_wrap()
            .children(stores.into_iter().map(|runner| {
                Button::new(SharedString::from(format!("found-login-{}", runner.id())))
                    .small()
                    .icon(store_icon(runner))
                    .label(runner.store_name())
                    .disabled(!online)
                    .on_click(move |_, window, cx| open_login_dialog(runner, window, cx))
            }));
        Some(
            v_flex()
                .gap_2()
                .child(self.row(
                    "found-stores",
                    IconName::Store,
                    t("manage.found.stores.title"),
                    if online {
                        t("manage.found.stores.description")
                    } else {
                        t("manage.onboarding.stores.offline")
                    },
                    None,
                    cx,
                ))
                .child(div().pl(rems(2.25)).child(buttons))
                .into_any_element(),
        )
    }

    fn render_launchers(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let entries = self.launchers.entries.clone();
        entries
            .into_iter()
            .map(|(launcher, state)| {
                let view = cx.entity().downgrade();
                let action = import_status(launcher, &state, move |window, cx| {
                    let _ = view.update(cx, |this, cx| this.run_import(launcher, window, cx));
                });
                v_flex()
                    .gap_2()
                    .py_2()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                Icon::new(IconName::Import).text_color(cx.theme().muted_foreground),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().font_medium().child(launcher.name()))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(launcher.description()),
                                    ),
                            ),
                    )
                    .child(div().pl(rems(2.25)).child(action))
                    .into_any_element()
            })
            .collect()
    }
}

impl Render for FoundSourcesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let steam = self.render_steam(cx);
        let stores = self.render_stores(cx);
        let launchers = self.render_launchers(cx);
        let emulation = self.render_emulation(cx);
        let signed_in = models(cx).accounts.read(cx).any_logged_in();
        v_flex()
            .w_full()
            .max_w(rems(36.))
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xl()
                            .font_semibold()
                            .child(t("manage.found.title")),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t("manage.found.description")),
                    ),
            )
            .child(
                card(cx)
                    .gap_0()
                    .children(steam)
                    .children(stores)
                    .child(emulation)
                    .children(launchers),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .when(signed_in, |d| {
                        d.child(
                            Tag::secondary()
                                .xsmall()
                                .child(t("manage.found.loading_hint")),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("found-getting-started")
                            .small()
                            .ghost()
                            .icon(IconName::Sparkles)
                            .label(t("manage.hub.title"))
                            .on_click(|_, window, cx| super::hub::open_getting_started(window, cx)),
                    )
                    .child(
                        Button::new("found-accounts")
                            .small()
                            .ghost()
                            .label(t("manage.found.accounts"))
                            .on_click(|_, _, cx| navigate(Route::Accounts, cx)),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steam_says_the_one_thing_it_needs() {
        assert_eq!(steam_row(false, None, false), SteamRow::NotFound);
        assert_eq!(
            steam_row(true, None, true),
            SteamRow::Local { client: true }
        );
        assert_eq!(
            steam_row(true, Some("gaben"), true),
            SteamRow::SignedIn("gaben".into())
        );
        // A signed-in account counts even where the folder isn't found
        // (another Steam install).
        assert_eq!(
            steam_row(false, Some("gaben"), false),
            SteamRow::SignedIn("gaben".into())
        );
    }

    fn card(runner: Runner) -> StoreCard {
        StoreCard {
            runner,
            user: None,
            games: 0,
            disabled: false,
            hidden: false,
            expired: None,
            local: false,
            accounts: Vec::new(),
            can_add: false,
            steam_client: None,
            steam_hidden: None,
        }
    }

    #[test]
    fn only_signed_out_stores_that_are_on_are_offered() {
        let mut gog = card(Runner::Gog);
        gog.user = Some("me".into());
        let mut zoom = card(Runner::Zoom);
        zoom.disabled = true;
        let mut itch = card(Runner::Itch);
        itch.hidden = true;
        let cards = [
            card(Runner::Legendary),
            gog,
            card(Runner::Nile),
            zoom,
            itch,
            card(Runner::Steam),
        ];
        assert_eq!(stores_to_log_in(&cards), [Runner::Legendary, Runner::Nile]);
    }
}
