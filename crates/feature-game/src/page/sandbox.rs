//! The sandbox card under the primary action, for a game set to run in a
//! bubblewrap sandbox: a shield with what is blocked ("No network · No home
//! folder"), "Running sandboxed" while it plays, and, opened with a click or
//! Enter, what the game can't reach and what it can, with its own home folder
//! one click away.

use gpui_kit::StatefulInteractiveElement as _;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement, Role, SharedString,
    Styled, div, px,
};
use irori_core::SandboxSettings;
use irori_ui::i18n::t;
use irori_ui::style::{ICON_CONTRAST, KeyboardFocusable as _, TEXT_CONTRAST, legible};
use irori_ui::{backend, models};

use super::GamePage;
use crate::ops;

/// The sandbox setting of `id` as it stands now (`Default` where games of
/// its store can't run sandboxed).
pub(super) fn configured(id: &irori_core::GameId, cx: &gpui_kit::App) -> SandboxSettings {
    if irori_runtime::sandbox::applies_to(id.runner) {
        backend(cx).settings.game(id).sandbox
    } else {
        SandboxSettings::default()
    }
}

impl GamePage {
    /// The card, when the game runs or will run sandboxed. A game running
    /// shows the sandbox it was started in, which a settings change only
    /// replaces at the next start.
    pub(super) fn render_sandbox(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let playing = models(cx).statuses.read(cx).is_playing(&self.id);
        let running = playing
            .then(|| backend(cx).running_sandbox(&self.id))
            .flatten();
        let sandbox = running.unwrap_or(self.sandbox);
        if !sandbox.is_enabled() || (playing && running.is_none()) {
            return None;
        }
        let ink = self.ink;
        let theme = cx.theme();
        let accent = legible(theme.success, &[ink.panel], ICON_CONTRAST, cx);
        let title = if running.is_some() {
            t("game.sandbox.running")
        } else {
            t("game.sandbox.title")
        };
        let blocked: Vec<SharedString> = [
            sandbox.no_network.then(|| t("game.sandbox.no_network")),
            sandbox.no_home.then(|| t("game.sandbox.no_home")),
        ]
        .into_iter()
        .flatten()
        .collect();
        let summary = blocked.join(" · ");
        let open = self.sandbox_open;
        let header = h_flex()
            .id("game-sandbox-toggle")
            .role(Role::Button)
            .aria_label(t("game.sandbox.details"))
            .w_full()
            .min_w_0()
            .gap_3()
            .px_3()
            .py_2()
            .rounded(theme.radius_lg)
            .cursor_pointer()
            .keyboard_focusable(cx)
            .hover(|s| s.bg(theme.list_hover))
            .on_click(cx.listener(|this, _, _, cx| {
                this.sandbox_open = !this.sandbox_open;
                cx.notify();
            }))
            .child(
                Icon::new(IconName::ShieldCheck)
                    .text_color(accent)
                    .flex_shrink_0(),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(ink.text)
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(ink.panel_muted)
                            .line_clamp(2)
                            .child(summary),
                    ),
            )
            .child(
                Icon::new(if open {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .small()
                .text_color(ink.panel_muted)
                .flex_shrink_0(),
            );
        Some(
            v_flex()
                .w_full()
                .rounded(theme.radius_lg)
                .bg(ink.panel)
                .border_1()
                .border_color(theme.border)
                .child(header)
                .when(open, |card| {
                    card.child(self.render_sandbox_details(sandbox, cx))
                })
                .into_any_element(),
        )
    }

    fn render_sandbox_details(
        &self,
        sandbox: SandboxSettings,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ink = self.ink;
        let theme = cx.theme();
        let blocked = legible(theme.danger, &[ink.panel], ICON_CONTRAST, cx);
        let allowed = legible(theme.success, &[ink.panel], ICON_CONTRAST, cx);
        let item = |icon: IconName, color, text: SharedString| {
            h_flex()
                .items_start()
                .gap_2()
                .text_xs()
                .child(
                    div()
                        .pt(px(1.))
                        .flex_shrink_0()
                        .child(Icon::new(icon).xsmall().text_color(color)),
                )
                .child(div().flex_1().min_w_0().text_color(ink.text).child(text))
        };
        let heading = |text: SharedString| {
            div()
                .text_xs()
                .font_semibold()
                .text_color(ink.panel_muted)
                .child(text)
        };
        let mut cant = vec![];
        if sandbox.no_network {
            cant.push(item(
                IconName::WifiOff,
                blocked,
                t("game.sandbox.cant_network"),
            ));
        }
        if sandbox.no_home {
            cant.push(item(IconName::Lock, blocked, t("game.sandbox.cant_home")));
        }
        cant.push(item(IconName::Ban, blocked, t("game.sandbox.cant_session")));
        let mut can = vec![item(
            IconName::FolderOpen,
            allowed,
            t("game.sandbox.can_game"),
        )];
        if sandbox.no_home {
            can.push(item(IconName::Folder, allowed, t("game.sandbox.can_home")));
        }
        can.push(item(
            IconName::Monitor,
            allowed,
            t("game.sandbox.can_devices"),
        ));
        // Under X11 every window shares one server, which the game needs to
        // draw at all.
        let x11 = std::env::var_os("WAYLAND_DISPLAY").is_none();
        let note = x11.then(|| {
            div()
                .text_xs()
                .text_color(legible(theme.warning, &[ink.panel], TEXT_CONTRAST, cx))
                .child(t("game.sandbox.x11"))
        });
        let home = irori_runtime::sandbox::sandbox_home(&self.id);
        let settings_id = self.id.clone();
        v_flex()
            .px_3()
            .pb_3()
            .pt_1()
            .gap_2()
            .child(heading(t("game.sandbox.cant_reach")))
            .children(cant)
            .child(heading(t("game.sandbox.can_reach")))
            .children(can)
            .children(note)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .pt_1()
                    .when(sandbox.no_home, |row| {
                        row.child(
                            Button::new("game-sandbox-home")
                                .small()
                                .ghost()
                                .icon(IconName::FolderOpen)
                                .label(t("game.sandbox.open_home"))
                                .on_click(move |_, window, cx| {
                                    // Made at the first sandboxed start; it
                                    // opens before that too.
                                    let dir = home.clone();
                                    let made = irori_ui::run(cx, {
                                        let dir = dir.clone();
                                        async move { Ok(tokio::fs::create_dir_all(&dir).await?) }
                                    });
                                    window
                                        .spawn(cx, async move |cx| {
                                            let _ = made.await;
                                            let _ = cx.update(|window, cx| {
                                                ops::open_path(dir, window, cx)
                                            });
                                        })
                                        .detach();
                                }),
                        )
                    })
                    .child(
                        Button::new("game-sandbox-settings")
                            .small()
                            .ghost()
                            .icon(IconName::Settings)
                            .label(t("game.sandbox.change"))
                            .on_click(move |_, _, cx| {
                                ops::open_settings(&settings_id, "launch", cx)
                            }),
                    ),
            )
            .into_any_element()
    }
}
