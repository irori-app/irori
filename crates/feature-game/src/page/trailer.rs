//! The page's trailers: a section above the screenshots with the shared
//! trailer player ([`irori_ui::trailer`]), made once the store page names
//! trailers and kept for the page, so playing on survives re-renders.

use gpui_kit::component::v_flex;
use gpui_kit::{
    AnyElement, AppContext as _, Context, IntoElement, ParentElement as _, Styled as _, div, rems,
};
use irori_ui::i18n::t;
use irori_ui::trailer::TrailerPlayer;

use super::GamePage;

/// Widest the trailer is drawn, in rem.
const MAX_STAGE_REM: f32 = 48.;

impl GamePage {
    /// Make or update the player from the store page's trailers.
    pub(super) fn sync_trailer(&mut self, cx: &mut Context<Self>) {
        let videos = self
            .extra
            .ready()
            .map(|e| e.videos.clone())
            .unwrap_or_default();
        match &self.trailer {
            Some(player) => player.update(cx, |p, cx| p.set_videos(videos, cx)),
            None if !videos.is_empty() => {
                let id = self.id.clone();
                self.trailer = Some(cx.new(|cx| TrailerPlayer::new(id, videos, cx)));
            }
            None => {}
        }
    }

    /// Whether the page has a trailer to play.
    pub(crate) fn has_trailer(&self, cx: &gpui_kit::App) -> bool {
        self.trailer
            .as_ref()
            .is_some_and(|p| !p.read(cx).is_empty())
    }

    /// Play or pause the trailer (the menu, the palette, the keyboard).
    pub(super) fn toggle_trailer(&mut self, cx: &mut Context<Self>) {
        if let Some(player) = &self.trailer {
            player.update(cx, |p, cx| p.toggle(cx));
        }
    }

    /// The trailer section, or nothing when the store has no trailer.
    pub(super) fn render_trailer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let player = self.trailer.clone()?;
        let count = player.read(cx).len();
        if count == 0 {
            return None;
        }
        let title = if count == 1 {
            t("game.trailer.title_one")
        } else {
            t("game.trailer.title_other")
        };
        Some(
            v_flex()
                .w_full()
                .gap_3()
                .child(super::section_title(title, self.ink.text))
                // Beyond this a trailer's 480p frames would only blur.
                .child(div().w_full().max_w(rems(MAX_STAGE_REM)).child(player))
                .into_any_element(),
        )
    }
}
