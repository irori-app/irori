//! "Review duplicate added games": the added games that are probably one
//! game twice (`Backend::duplicate_games`: one added by hand and one
//! imported from another launcher, or a program added twice), each pair
//! with why. Keeping one merges what the user kept about the other into it
//! (playtime, sessions, status, journal, rating, categories, favourite) and
//! takes the other out of the library, its files left alone; "Keep both"
//! stops offering the pair.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::{
    App, AppContext as _, Context, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Window, div, rems,
};
use irori_core::GameId;
use irori_services::duplicates::{Duplicate, DuplicateReason};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

/// Open the review of duplicate added games.
pub fn open_duplicates_dialog(window: &mut Window, cx: &mut App) {
    crate::i18n::register();
    let dialog = cx.new(|cx| DuplicatesDialog::new(window, cx));
    let width = window.rem_size() * 40.;
    window.open_dialog(cx, move |d, _, _| {
        d.title(t("library.duplicates.title"))
            .width(width)
            .child(dialog.clone())
    });
}

struct DuplicatesDialog {
    /// `None` while the pairs are looked for.
    pairs: Option<Vec<Duplicate>>,
    /// The pair being merged or kept.
    busy: Option<usize>,
}

impl DuplicatesDialog {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = Self {
            pairs: None,
            busy: None,
        };
        this.load(window, cx);
        this
    }

    fn load(&self, window: &mut Window, cx: &mut Context<Self>) {
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            let b2 = b.clone();
            Ok(b.spawn_blocking(move || b2.duplicate_games()).await?)
        });
        cx.spawn_in(window, async move |this, cx| {
            let pairs = task.await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.pairs = Some(pairs);
                this.busy = None;
                cx.notify();
            });
        })
        .detach();
    }

    /// Keep `keep` (merging `other` into it), or with `None` keep both.
    fn resolve(
        &mut self,
        index: usize,
        keep: Option<(GameId, GameId)>,
        pair: (GameId, GameId),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(index);
        cx.notify();
        let b = backend(cx);
        let task = irori_ui::run(cx, async move {
            match keep {
                Some((keep, other)) => b.merge_duplicate(&keep, &other).await,
                None => {
                    let b2 = b.clone();
                    b.spawn_blocking(move || b2.keep_duplicates(&pair.0, &pair.1))
                        .await?
                }
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(e) = result {
                    this.busy = None;
                    crate::notify_error(t("library.duplicates.failed"), &e, window, cx);
                    cx.notify();
                } else {
                    this.load(window, cx);
                }
            });
        })
        .detach();
    }
}

fn reason_text(reason: DuplicateReason) -> SharedString {
    t(match reason {
        DuplicateReason::SameProgram => "library.duplicates.reason.program",
        DuplicateReason::SameFolder => "library.duplicates.reason.folder",
        DuplicateReason::SamePrefix => "library.duplicates.reason.prefix",
        DuplicateReason::SameTitle => "library.duplicates.reason.title",
    })
}

impl Render for DuplicatesDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let Some(pairs) = &self.pairs else {
            return div()
                .text_sm()
                .text_color(muted)
                .child(t("library.duplicates.looking"))
                .into_any_element();
        };
        if pairs.is_empty() {
            return div()
                .text_sm()
                .text_color(muted)
                .child(t("library.duplicates.none"))
                .into_any_element();
        }
        let library = models(cx).library.read(cx).snapshot.clone();
        let title = |id: &GameId| -> String {
            library
                .get(id)
                .map(|g| g.title.clone())
                .unwrap_or_else(|| id.app_name.to_string())
        };
        let busy = self.busy;
        let rows = pairs.iter().enumerate().map(|(i, d)| {
            let (a, b) = (d.keep.clone(), d.other.clone());
            let (ta, tb) = (title(&a), title(&b));
            let disabled = busy.is_some();
            v_flex()
                .id(ElementId::Name(format!("duplicate-{i}").into()))
                .gap_2()
                .py_2()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(
                    v_flex()
                        .min_w_0()
                        .child(div().font_semibold().truncate().child(ta.clone()))
                        .child(div().truncate().child(tb.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(reason_text(d.reason)),
                        ),
                )
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new(("duplicate-keep-both", i))
                                .ghost()
                                .small()
                                .label(t("library.duplicates.keep_both"))
                                .disabled(disabled)
                                .on_click({
                                    let pair = (a.clone(), b.clone());
                                    cx.listener(move |this, _, window, cx| {
                                        this.resolve(i, None, pair.clone(), window, cx)
                                    })
                                }),
                        )
                        .child(
                            Button::new(("duplicate-keep-other", i))
                                .outline()
                                .small()
                                .label(t_args("library.duplicates.keep", &[("title", &tb)]))
                                .disabled(disabled)
                                .on_click({
                                    let (a, b) = (a.clone(), b.clone());
                                    cx.listener(move |this, _, window, cx| {
                                        let pair = (a.clone(), b.clone());
                                        this.resolve(
                                            i,
                                            Some((b.clone(), a.clone())),
                                            pair,
                                            window,
                                            cx,
                                        )
                                    })
                                }),
                        )
                        .child(
                            Button::new(("duplicate-keep", i))
                                .primary()
                                .small()
                                .label(t_args("library.duplicates.keep", &[("title", &ta)]))
                                .loading(busy == Some(i))
                                .disabled(disabled)
                                .on_click({
                                    let (a, b) = (a.clone(), b.clone());
                                    cx.listener(move |this, _, window, cx| {
                                        let pair = (a.clone(), b.clone());
                                        this.resolve(
                                            i,
                                            Some((a.clone(), b.clone())),
                                            pair,
                                            window,
                                            cx,
                                        )
                                    })
                                }),
                        ),
                )
        });
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(t("library.duplicates.description")),
            )
            .child(
                v_flex()
                    .max_h(rems(28.))
                    .overflow_y_scrollbar()
                    .children(rows),
            )
            .into_any_element()
    }
}
