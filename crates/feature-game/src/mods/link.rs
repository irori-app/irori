//! Mod manager links from a mod site: Nexus Mods' `nxm://` ("Mod manager
//! download") and Thunderstore's `ror2mm://` ("Install with Mod Manager").
//! Neither says which installed game the mod is for, so a dialog asks, with
//! the likely game picked, and installs the mod there.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::radio::Radio;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, IntoElement, ParentElement, Render, SharedString, Styled,
    Window, div, rems,
};
use irori_core::{GameId, GameInfo};
use irori_mods::nexus::NxmLink;
use irori_mods::thunderstore::ModManagerLink;
use irori_services::{InstallOutcome, ModProgress};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use crate::ops::{notify_error, notify_success};

/// A link whose mod is waiting for a game.
#[derive(Debug, Clone)]
enum ModLink {
    Nexus(NxmLink),
    Thunderstore(ModManagerLink),
}

/// An `nxm://` link arrived (the site's "Mod manager download"): ask which
/// game it is for and install it there.
pub fn open_nxm_dialog(raw: &str, window: &mut Window, cx: &mut App) {
    match NxmLink::parse(raw) {
        Some(link) => open(ModLink::Nexus(link), window, cx),
        None => window.push_notification(Notification::error(t("game.mods.nexus_bad_link")), cx),
    }
}

/// A `ror2mm://` link arrived (Thunderstore's "Install with Mod Manager"):
/// ask which game it is for and install the package and what it needs.
pub fn open_thunderstore_link_dialog(raw: &str, window: &mut Window, cx: &mut App) {
    match ModManagerLink::parse(raw) {
        Some(link) => open(ModLink::Thunderstore(link), window, cx),
        None => window.push_notification(Notification::error(t("game.mods.ts_bad_link")), cx),
    }
}

fn open(link: ModLink, window: &mut Window, cx: &mut App) {
    let title = match &link {
        ModLink::Nexus(_) => t("game.mods.nxm_title"),
        ModLink::Thunderstore(_) => t("game.mods.ts_link_title"),
    };
    let picker = cx.new(|cx| LinkPicker::new(link, window, cx));
    window.open_dialog(cx, move |d, window, _| {
        d.title(title.clone())
            .w(window.rem_size() * 30.)
            .child(picker.clone())
    });
}

struct LinkPicker {
    link: ModLink,
    games: Vec<Arc<GameInfo>>,
    chosen: Option<GameId>,
    /// Asking Thunderstore which game the package is for.
    finding: bool,
    progress: Option<ModProgress>,
}

/// Letters and digits only, lower case: `Stardew Valley` and
/// `stardewvalley` match.
fn squash(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

impl LinkPicker {
    fn new(link: ModLink, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let library = irori_ui::models(cx).library.read(cx).snapshot.clone();
        // Every copy: the one the mod is for may not be its entry's default.
        let mut games: Vec<Arc<GameInfo>> = library
            .all_games()
            .filter(|g| irori_services::supports_mods(g))
            .cloned()
            .collect();
        let mut chosen = None;
        let mut finding = false;
        match &link {
            ModLink::Nexus(nxm) => {
                // The game Nexus names first, then the rest by title.
                let domain = squash(&nxm.domain);
                games.sort_by_key(|g| (squash(&g.title) != domain, g.sort_title.clone()));
                chosen = games
                    .first()
                    .filter(|g| squash(&g.title) == domain)
                    .map(|g| g.id());
            }
            ModLink::Thunderstore(ts) => {
                games.sort_by_key(|g| g.sort_title.clone());
                finding = !games.is_empty();
                let b = backend(cx);
                let (ts, ids) = (ts.clone(), games.iter().map(|g| g.id()).collect());
                let task =
                    irori_ui::run(cx, async move { b.thunderstore_link_game(&ts, ids).await });
                cx.spawn_in(window, async move |this, cx| {
                    let found = task.await;
                    let _ = this.update(cx, |this, cx| {
                        this.finding = false;
                        // Unknown: the user picks the game.
                        if let Ok(Some(id)) = found {
                            this.pick_first(id);
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        Self {
            link,
            games,
            chosen,
            finding,
            progress: None,
        }
    }

    /// Choose `id` (unless the user already chose) and list it first.
    fn pick_first(&mut self, id: GameId) {
        if self.chosen.is_none() {
            self.chosen = Some(id.clone());
        }
        if let Some(ix) = self.games.iter().position(|g| g.id() == id) {
            let game = self.games.remove(ix);
            self.games.insert(0, game);
        }
    }

    fn install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.chosen.clone() else {
            return;
        };
        let progress = ModProgress::default();
        self.progress = Some(progress.clone());
        cx.notify();
        let b = backend(cx);
        let link = self.link.clone();
        let task = irori_ui::run(cx, async move {
            match link {
                ModLink::Nexus(link) => b.install_nexus(&id, link, progress).await,
                ModLink::Thunderstore(link) => {
                    b.install_thunderstore_link(&id, &link, progress).await
                }
            }
        });
        let ticker = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            drop(ticker);
            let _ = this.update_in(cx, |this, window, cx| {
                this.progress = None;
                match result {
                    Ok(InstallOutcome::Installed { name }) => {
                        window.close_dialog(cx);
                        notify_success(
                            window,
                            cx,
                            t_args("game.mods.installed", &[("name", &name)]),
                        );
                    }
                    Ok(InstallOutcome::Choose(prompt)) => {
                        window.close_dialog(cx);
                        window.defer(cx, move |window, cx| {
                            super::fomod::open_fomod_dialog(prompt, window, cx)
                        });
                    }
                    Err(e) => notify_error(window, cx, t("game.mods.install_failed"), &e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn body(&self) -> SharedString {
        match &self.link {
            ModLink::Nexus(link) => t_args("game.mods.nxm_body", &[("game", &link.domain)]),
            ModLink::Thunderstore(link) => t_args(
                "game.mods.ts_link_body",
                &[("name", &link.name.replace('_', " "))],
            ),
        }
    }
}

/// Height of one game in the picker, in rems.
const GAME_ROW_REMS: f32 = 2.;

impl Render for LinkPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let installing = self.progress.is_some();
        let fraction = self.progress.as_ref().and_then(|p| p.fraction());
        let row_h = window.rem_size() * GAME_ROW_REMS;
        let count = self.games.len();
        let sizes = std::rc::Rc::new(vec![gpui_kit::size(gpui_kit::px(0.), row_h); count]);
        let list = gpui_kit::component::v_virtual_list(
            cx.entity(),
            "link-games",
            sizes,
            move |this: &mut Self, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|ix| {
                        let g = this.games.get(ix)?;
                        let id = g.id();
                        let checked = this.chosen.as_ref() == Some(&id);
                        let label: SharedString = g.title.clone().into();
                        Some(
                            div()
                                .h(rems(GAME_ROW_REMS))
                                .flex()
                                .items_center()
                                .child(
                                    Radio::new(("link-game", ix))
                                        .label(label)
                                        .checked(checked)
                                        .disabled(installing)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.chosen = Some(id.clone());
                                            cx.notify();
                                        })),
                                )
                                .into_any_element(),
                        )
                    })
                    .collect()
            },
        )
        .w_full()
        .h(rems(GAME_ROW_REMS * count.min(8) as f32));
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(self.body()),
            )
            .when(self.finding, |col| {
                col.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(t("game.mods.ts_link_finding")),
                )
            })
            .child(if self.games.is_empty() {
                div()
                    .text_sm()
                    .child(t("game.mods.nxm_no_games"))
                    .into_any_element()
            } else {
                list.into_any_element()
            })
            .when_some(fraction, |col, f| {
                col.child(
                    gpui_kit::component::progress::Progress::new("link-progress").value(f * 100.),
                )
            })
            .child(
                h_flex().justify_end().child(
                    Button::new("link-install")
                        .primary()
                        .icon(IconName::Download)
                        .label(t("game.mods.install"))
                        .loading(installing)
                        .disabled(installing || self.chosen.is_none())
                        .on_click(cx.listener(|this, _, window, cx| this.install(window, cx))),
                ),
            )
    }
}
