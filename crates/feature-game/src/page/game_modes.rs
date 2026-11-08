//! The game-modes card under the age rating: a chip per mode the game
//! plays in, each showing the library's games of that mode, and a button
//! that edits them (the Modify dialog).

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::{AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, div};
use irori_core::{GameInfo, GameMode};
use irori_ui::i18n::{t, t_args};

use super::{GamePage, GamePageEvent};

/// The modes to show: the library entry's (which carries the user's own
/// choice and what was learned), else those of the store page just read,
/// unless the user chose the game has none (`own`).
pub(super) fn shown(game: &GameInfo, page: Option<&[GameMode]>, own: bool) -> Vec<GameMode> {
    if game.game_modes.is_empty() && !own {
        page.map(<[GameMode]>::to_vec).unwrap_or_default()
    } else {
        game.game_modes.clone()
    }
}

impl GamePage {
    /// The card, when any mode is known.
    pub(super) fn render_game_modes(
        &self,
        game: &GameInfo,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let own = irori_ui::models(cx)
            .library
            .read(cx)
            .user
            .overrides
            .get(&self.id.key())
            .is_some_and(|o| o.game_modes.is_some());
        let modes = shown(
            game,
            self.extra.ready().map(|e| e.game_modes.as_slice()),
            own,
        );
        if modes.is_empty() {
            return None;
        }
        let ink = self.ink;
        let theme = cx.theme();
        let id = self.id.clone();
        let chips = modes.into_iter().map(|mode| {
            let label = irori_ui::game_mode::label(mode);
            Button::new(SharedString::from(format!("game-mode-{}", mode.id())))
                .xsmall()
                .outline()
                .icon(Icon::new(irori_ui::game_mode::icon(mode)))
                .label(label.clone())
                .tooltip(t_args("game.modes.show", &[("mode", &label)]))
                .on_click(
                    cx.listener(move |_, _, _, cx| cx.emit(GamePageEvent::ShowGameMode(mode))),
                )
        });
        Some(
            v_flex()
                .w_full()
                .gap_1p5()
                .px_3()
                .py_2()
                .rounded(theme.radius_lg)
                .bg(ink.panel)
                .border_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_between()
                        .child(
                            div()
                                .min_w_0()
                                .text_xs()
                                .text_color(ink.panel_muted)
                                .truncate()
                                .child(irori_ui::game_mode::title()),
                        )
                        .child(
                            Button::new("game-modes-edit")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Pencil)
                                .tooltip(t("game.modes.edit"))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(GamePageEvent::RequestEdit(id.clone()))
                                })),
                        ),
                )
                .child(h_flex().w_full().flex_wrap().gap_1().children(chips))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_entry_s_modes_come_first() {
        let game = GameInfo {
            game_modes: vec![GameMode::Coop],
            ..Default::default()
        };
        let page = [GameMode::Single];
        assert_eq!(shown(&game, Some(&page), false), [GameMode::Coop]);
        let bare = GameInfo::default();
        assert_eq!(shown(&bare, Some(&page), false), [GameMode::Single]);
        assert!(shown(&bare, None, false).is_empty());
        // The user said it has none.
        assert!(shown(&bare, Some(&page), true).is_empty());
    }
}
