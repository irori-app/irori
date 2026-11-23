//! The Details panel's Steam reception rows: the store's review verdict
//! with its share of positive reviews, and how many people play the game
//! now, for any game with a Steam app (its own, or the one it is linked
//! to). Read when the page opens (`Backend::steam_reception`, kept a few
//! minutes), never in the background; out of reach shows nothing.

use gpui_kit::component::Sizable as _;
use gpui_kit::component::h_flex;
use gpui_kit::component::tag::Tag;
use gpui_kit::{
    AnyElement, Context, Hsla, IntoElement, ParentElement as _, Styled as _, Window, div,
};
use irori_services::steam_reception::SteamReception;
use irori_ui::backend;
use irori_ui::i18n::{t, t_args};

use super::GamePage;
use crate::load::Load;

/// `n` with its digits grouped as the interface language groups them.
pub(crate) fn grouped(n: u64) -> String {
    group_digits(n, separator(&t("game.reception.grouping")))
}

/// The separator a language writes a thousand with (`1,000` → `,`).
fn separator(thousand: &str) -> &str {
    thousand
        .strip_prefix('1')
        .and_then(|rest| rest.strip_suffix("000"))
        .unwrap_or(",")
}

/// `n` with its digits grouped in threes by `separator`.
fn group_digits(n: u64, separator: &str) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 * separator.len());
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push_str(separator);
        }
        out.push(c);
    }
    out
}

/// The key of Steam's verdict for a review score (1–9; 0 when there are
/// too few reviews for one). Steam's own words are English only.
fn verdict_key(score: u8) -> &'static str {
    match score {
        9.. => "game.reception.verdict.overwhelmingly_positive",
        8 => "game.reception.verdict.very_positive",
        7 => "game.reception.verdict.positive",
        6 => "game.reception.verdict.mostly_positive",
        5 => "game.reception.verdict.mixed",
        4 => "game.reception.verdict.mostly_negative",
        3 => "game.reception.verdict.negative",
        2 => "game.reception.verdict.very_negative",
        1 => "game.reception.verdict.overwhelmingly_negative",
        0 => "game.reception.verdict.few",
    }
}

/// The tag colour of a review score: Steam's own green, amber and red
/// bands, by the verdict.
fn verdict_tag(score: u8) -> Tag {
    match score {
        7.. => Tag::success(),
        5..7 => Tag::warning(),
        1..5 => Tag::danger(),
        0 => Tag::secondary(),
    }
}

impl GamePage {
    /// Ask Steam how the game is received, when it has a Steam app.
    pub(super) fn load_reception(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(game) = self.game.clone() else {
            return;
        };
        if self.steam_link.app_id().is_none() {
            self.reception = Load::Idle;
            return;
        }
        let b = backend(cx);
        let task = irori_ui::run(cx, async move { b.steam_reception(&game).await });
        self.reception = Load::Loading;
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.reception = Load::from_result(result);
                cx.notify();
            });
        })
        .detach();
    }

    /// The review and player rows, when Steam said anything.
    pub(super) fn reception_rows(
        &self,
        row: &dyn Fn(gpui_kit::SharedString, AnyElement, Hsla) -> AnyElement,
        muted: Hsla,
    ) -> Vec<AnyElement> {
        let Some(Some(reception)) = self.reception.ready() else {
            return Vec::new();
        };
        reception_values(reception)
            .into_iter()
            .map(|(label, value)| row(label, value, muted))
            .collect()
    }
}

/// The rows' labels and values.
fn reception_values(r: &SteamReception) -> Vec<(gpui_kit::SharedString, AnyElement)> {
    let mut rows = Vec::new();
    if let Some(reviews) = r.reviews.as_ref().filter(|r| r.total > 0) {
        let verdict = t(verdict_key(reviews.score));
        let share = reviews.percent().map(|p| {
            t_args(
                "game.reception.share",
                &[
                    ("percent", &p.to_string()),
                    ("count", &grouped(reviews.total)),
                ],
            )
        });
        rows.push((
            t("game.reception.reviews"),
            h_flex()
                .gap_2()
                .flex_wrap()
                .min_w_0()
                .child(verdict_tag(reviews.score).xsmall().child(verdict))
                .children(share.map(|s| div().text_xs().child(s)))
                .into_any_element(),
        ));
    }
    if let Some(players) = r.players {
        rows.push((
            t("game.reception.players"),
            div().truncate().child(grouped(players)).into_any_element(),
        ));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_name_their_separator_by_a_thousand() {
        assert_eq!(separator("1,000"), ",");
        assert_eq!(separator("1.000"), ".");
        assert_eq!(separator("1\u{a0}000"), "\u{a0}");
        assert_eq!(separator("1000"), "");
        assert_eq!(separator("one thousand"), ",");
    }

    #[test]
    fn digits_group_in_threes() {
        assert_eq!(group_digits(0, ","), "0");
        assert_eq!(group_digits(999, ","), "999");
        assert_eq!(group_digits(1000, ","), "1,000");
        assert_eq!(group_digits(1_234_567, "."), "1.234.567");
        assert_eq!(group_digits(12_345, "\u{202f}"), "12\u{202f}345");
    }
}
