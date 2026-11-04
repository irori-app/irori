//! The journal tab: the period's notes over the art of the latest one's
//! game, each a card with the game's cover, the day and its play time, the
//! note and the screenshots it links. A card opens the note to read or
//! edit; its game opens from the button beside it. The page is one list that
//! draws only the cards on screen.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex, v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _,
    Pixels, SharedString, Size, StatefulInteractiveElement as _, Styled as _, StyledImage as _,
    Window, div, px, rems, size,
};
use irori_services::LibrarySnapshot;
use irori_ui::components::{EmptyState, GameCover, art_img, physical_width};
use irori_ui::i18n::{t, t_args};
use irori_ui::journal::shot_url;
use irori_ui::style::KeyboardFocusable as _;
use irori_ui::{Route, models, navigate};

use super::hero::{HERO_REM, big_number, dotted, eyebrow};
use super::journal::{JournalPage, Note};
use super::period::Span;
use super::{StatsView, text};

/// The page's padding, in rem.
const PAD_REM: f32 = 1.5;
/// Height of a note's card with the space below it, in rem.
const NOTE_REM: f32 = 8.75;
/// Extra height of a card that shows screenshots, in rem.
const SHOTS_REM: f32 = 4.;
/// Most linked screenshots a card shows.
const CARD_SHOTS: usize = 5;
/// Width of a screenshot on a card, in rem.
const SHOT_REM: f32 = 6.;
/// Width of the game's cover on a card, in rem.
const COVER_REM: f32 = 4.5;

/// One entry of the journal's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum JournalItem {
    Hero,
    Note(usize),
    End,
}

impl JournalItem {
    fn height_rem(&self, page: &JournalPage) -> f32 {
        match self {
            JournalItem::Hero => PAD_REM + HERO_REM + PAD_REM,
            JournalItem::Note(ix) => match page.notes.get(*ix) {
                Some(n) if !n.entry.screenshots.is_empty() => NOTE_REM + SHOTS_REM,
                _ => NOTE_REM,
            },
            JournalItem::End => PAD_REM,
        }
    }
}

/// The list's entries for `page`.
pub(super) fn items(page: &JournalPage) -> Vec<JournalItem> {
    std::iter::once(JournalItem::Hero)
        .chain((0..page.notes.len()).map(JournalItem::Note))
        .chain(std::iter::once(JournalItem::End))
        .collect()
}

impl StatsView {
    /// The journal tab without notes in the period: the headline and how to
    /// write one, inside the page's usual scrolling body.
    pub(super) fn render_journal(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(page) = self.journal.clone() else {
            return div().into_any_element();
        };
        v_flex()
            .gap_6()
            .child(self.render_journal_hero(&page, window, cx))
            .child(
                EmptyState::new(
                    IconName::NotebookPen,
                    if page.period.span == Span::All {
                        t("library.stats.journal.empty.title_all")
                    } else {
                        t_args(
                            "library.stats.journal.empty.title",
                            &[("period", &text::period(&page.period))],
                        )
                    },
                )
                .description(t("library.stats.journal.empty.description")),
            )
            .into_any_element()
    }

    /// The journal tab with notes: one list of the headline and the cards.
    pub(super) fn render_journal_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(page) = self.journal.clone() else {
            return div().into_any_element();
        };
        let rem = window.rem_size();
        let sizes: Vec<Size<Pixels>> = items(&page)
            .iter()
            .map(|item| size(px(0.), rem * item.height_rem(&page)))
            .collect();
        if *self.journal_sizes != sizes {
            self.journal_sizes = Rc::new(sizes);
        }
        v_virtual_list(
            cx.entity(),
            "stats-journal",
            self.journal_sizes.clone(),
            |this, range, window, cx| this.render_journal_items(range, window, cx),
        )
        .track_scroll(&self.journal_scroll)
        .size_full()
        .into_any_element()
    }

    fn render_journal_items(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(page) = self.journal.clone() else {
            return Vec::new();
        };
        let list = items(&page);
        let library = models(cx).library.read(cx).snapshot.clone();
        range
            .filter_map(|ix| {
                let item = list.get(ix)?;
                let content = match item {
                    JournalItem::Hero => self.render_journal_hero(&page, window, cx),
                    JournalItem::Note(n) => {
                        self.render_note(*n, page.notes.get(*n)?, &library, window, cx)
                    }
                    JournalItem::End => div().into_any_element(),
                };
                Some(
                    div()
                        .w_full()
                        .h(rems(item.height_rem(&page)))
                        .px(rems(PAD_REM))
                        .when(*item == JournalItem::Hero, |d| d.pt(rems(PAD_REM)))
                        .child(content)
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// The art frame over the latest note's game, with the period's count.
    fn render_journal_hero(
        &self,
        page: &JournalPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let library = models(cx).library.read(cx).snapshot.clone();
        let latest = page.notes.first();
        let game = latest.and_then(|n| library.get(&n.id).cloned());
        let credit = latest.zip(game.as_ref()).map(|(note, game)| {
            let why = t_args(
                "library.stats.journal.latest",
                &[
                    ("title", &game.title),
                    ("date", &text::short_date(note.day)),
                ],
            );
            self.render_art_credit(game, why, cx)
        });
        let muted = cx.theme().muted_foreground;
        let headline = v_flex()
            .gap_2()
            .min_w_0()
            .child(eyebrow(
                t_args(
                    "library.stats.journal.eyebrow",
                    &[("period", &text::period(&page.period))],
                ),
                cx,
            ))
            .child(big_number(page.notes.len().to_string().into()))
            .when(!page.notes.is_empty(), |d| {
                d.child(
                    h_flex()
                        .flex_wrap()
                        .gap_x_3()
                        .gap_y_1()
                        .items_center()
                        .text_sm()
                        .children(dotted(
                            vec![
                                text::count(
                                    "library.stats.journal.notes_one",
                                    "library.stats.journal.notes_other",
                                    page.notes.len() as u64,
                                ),
                                text::count(
                                    "library.stats.album.games_one",
                                    "library.stats.album.games_other",
                                    page.games as u64,
                                ),
                            ],
                            muted,
                        )),
                )
            })
            .into_any_element();
        let art = game.and_then(|g| g.hero_art().map(str::to_owned));
        self.render_hero(art, headline, credit, window, cx)
    }

    fn render_note(
        &self,
        ix: usize,
        note: &Note,
        library: &LibrarySnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (radius, fill) = (theme.radius, theme.muted);
        let game = library.get(&note.id).cloned();
        let title = game
            .as_ref()
            .map_or_else(|| note.id.app_name.to_string(), |g| g.title.clone());
        let mut facts = vec![text::long_date(note.day)];
        if note.seconds > 0 {
            facts.push(t_args(
                "library.stats.journal.played",
                &[("duration", &text::duration(note.seconds))],
            ));
        }
        let width = physical_width(rems(SHOT_REM), window);
        let shots: Vec<AnyElement> = note
            .entry
            .screenshots
            .iter()
            .take(CARD_SHOTS)
            .map(|path| {
                let frame = div()
                    .w(rems(SHOT_REM))
                    .h(rems(SHOT_REM * 9. / 16.))
                    .flex_shrink_0()
                    .rounded(radius)
                    .overflow_hidden()
                    .bg(fill);
                match art_img(&shot_url(path), width, false, cx) {
                    Some(img) => frame
                        .child(img.size_full().object_fit(ObjectFit::Cover))
                        .into_any_element(),
                    None => frame.into_any_element(),
                }
            })
            .collect();
        let heading = (!note.entry.title.is_empty()).then(|| note.entry.title.clone());
        let id = note.id.clone();
        let day = note.day;
        let date = text::long_date(day);
        let open_title = title.clone();
        let game_id = note.id.clone();
        let theme = cx.theme();
        let (card_bg, hover) = (theme.secondary.opacity(0.5), theme.secondary);
        h_flex()
            .id(("journal-note", ix))
            .keyboard_focusable(cx)
            .w_full()
            .h(rems(
                if shots.is_empty() {
                    NOTE_REM
                } else {
                    NOTE_REM + SHOTS_REM
                } - 1.,
            ))
            .items_start()
            .gap_4()
            .p_4()
            .rounded(theme.radius_lg)
            .border_1()
            .border_color(theme.border)
            .bg(card_bg)
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .on_click(cx.listener(move |_, _, window, cx| {
                irori_ui::journal::open_editor(id.clone(), day, &open_title, &date, window, cx)
            }))
            .children(game.map(|g| GameCover::new(g).width(rems(COVER_REM)).into_any_element()))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(theme.muted_foreground)
                                    .child(SharedString::from(title.to_uppercase())),
                            )
                            .child(
                                h_flex()
                                    .flex_shrink_0()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .children(dotted(facts, theme.muted_foreground)),
                            ),
                    )
                    .when_some(heading, |c, h| {
                        c.child(div().w_full().truncate().font_semibold().child(h))
                    })
                    .child(
                        div()
                            .w_full()
                            .text_sm()
                            .line_clamp(if note.entry.title.is_empty() { 3 } else { 2 })
                            .child(note.entry.text.clone()),
                    )
                    .when(!shots.is_empty(), |c| {
                        c.child(h_flex().pt_2().gap_2().children(shots))
                    }),
            )
            .child(
                Button::new(("journal-open-game", ix))
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(IconName::Gamepad2))
                    .tooltip(t("library.stats.journal.open_game"))
                    .accessibility_label(t("library.stats.journal.open_game"))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        navigate(Route::Game(game_id.clone()), cx)
                    }),
            )
            .into_any_element()
    }
}
