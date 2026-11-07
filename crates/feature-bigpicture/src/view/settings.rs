//! A game's settings inside Big Picture: the settings feature's own view
//! (through `irori_ui::hooks::game_settings`) over the page, between the
//! top bar and the hints, so Big Picture keeps showing while they are open.
//!
//! The panel sits outside the `BigPicture` key context, so the arrows,
//! Enter and typing reach the settings' own controls (a controller's
//! directions move between them as on any other screen). Escape, and so a
//! controller's Back, closes it and returns to the game's page.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    AnyElement, AnyView, Context, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Role, StatefulInteractiveElement as _, Styled as _, Window, div, rems,
};
use irori_core::GameId;
use irori_ui::i18n::t;
use irori_ui::{Route, nav};

use super::{BigPictureView, Colors};
use crate::layout::Metrics;

/// The settings of one game, shown over its page.
pub(super) struct SettingsPanel {
    pub(super) id: GameId,
    view: AnyView,
    /// Focused when the panel opens; the next direction moves into the
    /// settings from here.
    focus: FocusHandle,
}

impl BigPictureView {
    /// Open the settings of `id` over its page. Without the hook (tests
    /// that don't install it) the settings page outside Big Picture opens.
    pub(super) fn open_game_settings(
        &mut self,
        id: GameId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity().downgrade();
        let on_close: irori_ui::hooks::CloseFn = std::rc::Rc::new(move |window, cx| {
            // Back is clicked inside the settings view; Big Picture is
            // another entity, so updating it here is fine.
            let _ = this.update(cx, |this, cx| this.close_settings(window, cx));
        });
        let Some(view) = irori_ui::hooks::game_settings(id.clone(), on_close, window, cx) else {
            nav::navigate(Route::GameSettings(id, "general".into()), cx);
            return;
        };
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        // While this is set, `on_focus_lost` leaves the focus with the panel.
        self.settings = Some(SettingsPanel { id, view, focus });
        cx.notify();
    }

    /// Close the settings and return to the game's page.
    pub(super) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.take().is_some() {
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    /// Whether a game's settings are open.
    pub fn is_settings_open(&self) -> bool {
        self.settings.is_some()
    }

    /// Give the keyboard back to the settings (an input reached Big
    /// Picture's root while they show, after a click on its top bar).
    pub(super) fn focus_settings(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(panel) = &self.settings {
            panel.focus.focus(window, cx);
        }
    }

    pub(super) fn render_settings(
        &self,
        c: Colors,
        m: Metrics,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let panel = self.settings.as_ref()?;
        let theme = cx.theme();
        Some(
            div()
                .id("bp-settings")
                .role(Role::Dialog)
                .aria_label(t("bigpicture.page.settings"))
                .key_context(crate::SETTINGS_CONTEXT)
                .track_focus(&panel.focus)
                .on_action(cx.listener(|this, _: &crate::CloseSettings, window, cx| {
                    this.close_settings(window, cx)
                }))
                .absolute()
                .left(rems(m.gutter))
                .right(rems(m.gutter))
                .top(rems(m.top_bar))
                .bottom(rems(m.hints))
                .mb_4()
                .overflow_hidden()
                .rounded(c.radius_lg)
                .border_1()
                .border_color(theme.border)
                // The settings draw in the interface theme, so they sit on
                // its own opaque background whatever Big Picture's look.
                .bg(theme.background)
                .text_color(theme.foreground)
                .child(panel.view.clone())
                .into_any_element(),
        )
    }
}
