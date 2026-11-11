//! Nexus Mods: saving the user's API key, finding a mod's files from its
//! page link, and installing an `nxm://` link the site's "Mod manager
//! download" button opened.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::radio::Radio;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement, Render, SharedString, Styled, Subscription, Window, div, rems,
};
use irori_core::{GameId, GameInfo};
use irori_mods::nexus::{self, ModFile, ModInfo, NexusUser, NxmLink};
use irori_services::{InstallOutcome, ModProgress};
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use super::{Busy, ModsPanel};
use crate::load::Load;
use crate::ops::{notify_error, notify_success};

const KEY_PAGE: &str = "https://www.nexusmods.com/settings/api-keys";

/// The Nexus Mods dialog of the panel's game.
pub(super) fn open_nexus_dialog(panel: Entity<ModsPanel>, window: &mut Window, cx: &mut App) {
    let dialog = cx.new(|cx| NexusDialog::new(panel, window, cx));
    window.open_dialog(cx, move |d, window, _| {
        d.title("Nexus Mods")
            .w(window.rem_size() * 38.)
            .child(dialog.clone())
    });
}

struct NexusDialog {
    panel: Entity<ModsPanel>,
    user: Load<Option<NexusUser>>,
    key: Entity<InputState>,
    link: Entity<InputState>,
    saving: bool,
    /// The mod whose files are listed.
    found: Load<(String, u64, ModInfo, Vec<ModFile>)>,
    handler_set: bool,
    _subscriptions: Vec<Subscription>,
}

impl NexusDialog {
    fn new(panel: Entity<ModsPanel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t("game.mods.nexus_key_placeholder"))
                .masked(true)
        });
        let link = cx.new(|cx| {
            InputState::new(window, cx).placeholder("https://www.nexusmods.com/…/mods/…")
        });
        let subscriptions = vec![
            cx.subscribe_in(&key, window, |this: &mut Self, _, e, window, cx| {
                if let InputEvent::PressEnter { .. } = e {
                    this.save_key(window, cx)
                }
            }),
            cx.subscribe_in(&link, window, |this: &mut Self, _, e, window, cx| {
                if let InputEvent::PressEnter { .. } = e {
                    this.find(window, cx)
                }
            }),
        ];
        let this = Self {
            panel,
            user: Load::Loading,
            key,
            link,
            saving: false,
            found: Load::Idle,
            handler_set: false,
            _subscriptions: subscriptions,
        };
        let b = backend(cx);
        let task = irori_ui::state::run_infallible(cx, async move { b.nexus_user().await });
        cx.spawn_in(window, async move |this, cx| {
            let user = task.await.flatten();
            let _ = this.update(cx, |this, cx| {
                this.user = Load::Ready(user);
                cx.notify();
            });
        })
        .detach();
        this
    }

    fn save_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.key.read(cx).value().to_string();
        if key.trim().is_empty() || self.saving {
            return;
        }
        self.saving = true;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.set_nexus_key(key).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.saving = false;
                match result {
                    Ok(user) => {
                        this.user = Load::Ready(Some(user));
                        this.key.update(cx, |k, cx| k.set_value("", window, cx));
                    }
                    Err(e) => notify_error(window, cx, t("game.mods.nexus_key_failed"), &e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn forget_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.forget_nexus_key().await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => {
                        this.user = Load::Ready(None);
                        this.found = Load::Idle;
                    }
                    Err(e) => notify_error(window, cx, t("game.mods.change_failed"), &e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn handle_links(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.handle_nxm_links().await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => {
                        this.handler_set = true;
                        notify_success(window, cx, t("game.mods.nexus_links_set"));
                    }
                    Err(e) => notify_error(window, cx, t("game.mods.nexus_links_failed"), &e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.link.read(cx).value().to_string();
        let parsed = nexus::parse_mod_url(&text)
            .or_else(|| NxmLink::parse(&text).map(|l| (l.domain, l.mod_id)));
        let Some((domain, mod_id)) = parsed else {
            self.found = Load::Failed(t("game.mods.nexus_bad_link"));
            cx.notify();
            return;
        };
        // An `nxm://` link pasted here installs that file directly.
        if let Some(link) = NxmLink::parse(&text) {
            self.install(link, window, cx);
            return;
        }
        self.found = Load::Loading;
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            let (info, files) = b.nexus_mod(&domain, mod_id).await?;
            Ok((domain, mod_id, info, files))
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.found = Load::from_result(result);
                cx.notify();
            });
        })
        .detach();
    }

    fn install(&mut self, link: NxmLink, window: &mut Window, cx: &mut Context<Self>) {
        window.close_dialog(cx);
        self.panel.update(cx, |panel, cx| {
            let b = backend(cx);
            let id = panel.id().clone();
            let progress = ModProgress::default();
            let busy = Busy {
                label: t("game.mods.nexus_downloading"),
                progress: Some(progress.clone()),
            };
            let task = irori_ui::run(
                cx,
                async move { b.install_nexus(&id, link, progress).await },
            );
            panel.run_install(busy, task, window, cx);
        });
    }

    fn render_key_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(t("game.mods.nexus_key_help")),
            )
            .child(Input::new(&self.key))
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("nexus-key-page")
                            .ghost()
                            .small()
                            .icon(IconName::ExternalLink)
                            .label(t("game.mods.nexus_get_key"))
                            .on_click(|_, _, cx| cx.open_url(KEY_PAGE)),
                    )
                    .child(
                        Button::new("nexus-key-save")
                            .primary()
                            .small()
                            .label(t("game.mods.nexus_save_key"))
                            .loading(self.saving)
                            .disabled(self.saving)
                            .on_click(cx.listener(|this, _, window, cx| this.save_key(window, cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_files(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = cx.theme();
        let premium = self
            .user
            .ready()
            .and_then(Option::as_ref)
            .is_some_and(|u| u.is_premium);
        let busy = self.panel.read(cx).is_busy();
        Some(match &self.found {
            Load::Idle => return None,
            Load::Loading => div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(t("game.mods.loading"))
                .into_any_element(),
            Load::Failed(e) => div()
                .text_sm()
                .text_color(theme.danger)
                .child(e.clone())
                .into_any_element(),
            Load::Ready((domain, mod_id, info, files)) => {
                let current: Vec<&ModFile> = files.iter().filter(|f| f.is_current()).collect();
                let mut col = v_flex().gap_2().child(
                    v_flex()
                        .gap_0p5()
                        .child(div().font_semibold().child(info.name.clone()))
                        .when(!info.summary.is_empty(), |c| {
                            c.child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(info.summary.clone()),
                            )
                        }),
                );
                if !premium {
                    col = col.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(t("game.mods.nexus_free_note")),
                    );
                }
                let mut rows = v_flex().gap_1();
                for (ix, f) in current.iter().take(40).enumerate() {
                    let link = NxmLink {
                        domain: domain.clone(),
                        mod_id: *mod_id,
                        file_id: f.file_id,
                        key: None,
                        expires: None,
                    };
                    let site = format!(
                        "https://www.nexusmods.com/{domain}/mods/{mod_id}?tab=files&file_id={}",
                        f.file_id
                    );
                    rows = rows.child(
                        h_flex()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(theme.border)
                            .items_center()
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().truncate().text_sm().child(f.name.clone()))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(
                                                [
                                                    (!f.version.is_empty())
                                                        .then(|| format!("v{}", f.version)),
                                                    Some(irori_core::util::format_bytes(
                                                        f.size_bytes,
                                                    )),
                                                ]
                                                .into_iter()
                                                .flatten()
                                                .collect::<Vec<_>>()
                                                .join(" · "),
                                            ),
                                    ),
                            )
                            .when(f.category == "MAIN", |r| {
                                r.child(Tag::secondary().xsmall().child(t("game.mods.nexus_main")))
                            })
                            .child(if premium {
                                Button::new(("nexus-install", ix))
                                    .small()
                                    .outline()
                                    .icon(IconName::Download)
                                    .label(t("game.mods.install"))
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.install(link.clone(), window, cx)
                                    }))
                                    .into_any_element()
                            } else {
                                Button::new(("nexus-site", ix))
                                    .small()
                                    .ghost()
                                    .icon(IconName::ExternalLink)
                                    .label(t("game.mods.nexus_on_site"))
                                    .on_click(move |_, _, cx| cx.open_url(&site))
                                    .into_any_element()
                            }),
                    );
                }
                col.child(
                    div()
                        .id("nexus-files")
                        .max_h(rems(20.))
                        .overflow_y_scrollbar()
                        .child(rows),
                )
                .into_any_element()
            }
        })
    }
}

impl Render for NexusDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let files = self.render_files(cx);
        let theme = cx.theme();
        let user = match &self.user {
            Load::Ready(u) => u.clone(),
            _ => {
                return v_flex().child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(t("game.mods.loading")),
                );
            }
        };
        let Some(user) = user else {
            return v_flex().child(self.render_key_form(cx));
        };
        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(t_args("game.mods.nexus_signed_in", &[("name", &user.name)])),
                    )
                    .child(if user.is_premium {
                        Tag::info().xsmall().child("Premium")
                    } else {
                        Tag::secondary().xsmall().child(t("game.mods.nexus_free"))
                    })
                    .child(
                        Button::new("nexus-forget")
                            .xsmall()
                            .ghost()
                            .label(t("game.mods.nexus_forget"))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.forget_key(window, cx)),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().child(t("game.mods.nexus_link_label")))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(Input::new(&self.link)))
                            .child(
                                Button::new("nexus-find")
                                    .outline()
                                    .small()
                                    .label(t("game.mods.nexus_find"))
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.find(window, cx)),
                                    ),
                            ),
                    ),
            )
            .children(files)
            .when(irori_services::mods::can_handle_nxm_links(), |col| {
                col.child(
                    h_flex()
                        .gap_2()
                        .pt_3()
                        .border_t_1()
                        .border_color(theme.border)
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(t("game.mods.nexus_links_help")),
                        )
                        .child(
                            Button::new("nexus-handler")
                                .xsmall()
                                .outline()
                                .label(if self.handler_set {
                                    t("game.mods.nexus_links_done")
                                } else {
                                    t("game.mods.nexus_links_button")
                                })
                                .disabled(self.handler_set)
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.handle_links(window, cx)
                                    }),
                                ),
                        ),
                )
            })
    }
}

/// An `nxm://` link arrived (the site's "Mod manager download"): ask which
/// game it is for and install it there.
pub fn open_nxm_dialog(raw: &str, window: &mut Window, cx: &mut App) {
    let Some(link) = NxmLink::parse(raw) else {
        window.push_notification(
            gpui_kit::component::notification::Notification::error(t("game.mods.nexus_bad_link")),
            cx,
        );
        return;
    };
    let picker = cx.new(|cx| NxmPicker::new(link, cx));
    window.open_dialog(cx, move |d, window, _| {
        d.title(t("game.mods.nxm_title"))
            .w(window.rem_size() * 30.)
            .child(picker.clone())
    });
}

struct NxmPicker {
    link: NxmLink,
    games: Vec<Arc<GameInfo>>,
    chosen: Option<GameId>,
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

impl NxmPicker {
    fn new(link: NxmLink, cx: &mut Context<Self>) -> Self {
        let library = irori_ui::models(cx).library.read(cx).snapshot.clone();
        // Every copy: the one the mod is for may not be its entry's default.
        let mut games: Vec<Arc<GameInfo>> = library
            .all_games()
            .filter(|g| irori_services::supports_mods(g))
            .cloned()
            .collect();
        let domain = squash(&link.domain);
        // The game Nexus names first, then the rest by title.
        games.sort_by_key(|g| (squash(&g.title) != domain, g.sort_title.clone()));
        let chosen = games
            .first()
            .filter(|g| squash(&g.title) == domain)
            .map(|g| g.id());
        Self {
            link,
            games,
            chosen,
            progress: None,
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
        let task = irori_ui::run(
            cx,
            async move { b.install_nexus(&id, link, progress).await },
        );
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
                        let prompt = prompt.clone();
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
}

/// Height of one game in the picker, in rems.
const GAME_ROW_REMS: f32 = 2.;

impl Render for NxmPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let installing = self.progress.is_some();
        let fraction = self.progress.as_ref().and_then(|p| p.fraction());
        let row_h = window.rem_size() * GAME_ROW_REMS;
        let count = self.games.len();
        let sizes = std::rc::Rc::new(vec![gpui_kit::size(gpui_kit::px(0.), row_h); count]);
        let list = gpui_kit::component::v_virtual_list(
            cx.entity(),
            "nxm-games",
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
                                    Radio::new(("nxm-game", ix))
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
                    .child(t_args("game.mods.nxm_body", &[("game", &self.link.domain)])),
            )
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
                    gpui_kit::component::progress::Progress::new("nxm-progress").value(f * 100.),
                )
            })
            .child(
                h_flex().justify_end().child(
                    Button::new("nxm-install")
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
