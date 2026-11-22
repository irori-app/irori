//! The Wine manager's Prefixes tab: every Wine prefix with the installed
//! games running in it, from [`Backend::wine_prefixes`]. A prefix a game
//! made per title can number in the hundreds, so the list is virtual.
//!
//! [`Backend::wine_prefixes`]: irori_services::Backend::wine_prefixes

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, v_flex, v_virtual_list,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, ElementId, IntoElement, ParentElement as _, Pixels, SharedString,
    Size, Styled as _, Window, div, px, rems, size,
};
use irori_services::prefixes::PrefixEntry;
use irori_ui::Route;
use irori_ui::components::EmptyState;
use irori_ui::i18n::{t, t_args};

use super::WineManagerView;

/// Height of one row in rem: name, path and who uses it.
const ROW_REMS: f32 = 4.75;

/// What the tab shows.
pub(super) enum PrefixState {
    Loading,
    Loaded {
        list: Rc<Vec<PrefixEntry>>,
        sizes: Rc<Vec<Size<Pixels>>>,
    },
}

/// Who uses a prefix, in one line: the installed games, else what it was.
pub(super) fn used_by(entry: &PrefixEntry) -> SharedString {
    let names: Vec<&str> = entry.games.iter().map(|(_, t)| t.as_str()).collect();
    match names.as_slice() {
        [] if entry.is_launcher || entry.is_default => t("manage.wine.prefixes.no_games"),
        [] if !entry.former.is_empty() => t_args(
            "manage.wine.prefixes.former",
            &[("games", &entry.former.join(", "))],
        ),
        [] => t("manage.wine.prefixes.unused"),
        [one] => t_args("manage.wine.prefixes.used_by", &[("games", one)]),
        many => t_args(
            "manage.wine.prefixes.used_by_many",
            &[
                ("count", &many.len().to_string()),
                ("games", &many.join(", ")),
            ],
        ),
    }
}

impl WineManagerView {
    /// Read the prefixes again (on opening the tab, Refresh, and library
    /// or settings changes while it was read once).
    pub(super) fn load_prefixes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prefixes.is_none() {
            self.prefixes = Some(PrefixState::Loading);
            cx.notify();
        }
        let backend = irori_ui::backend(cx);
        let task =
            irori_ui::state::run_infallible(cx, async move { backend.wine_prefixes().await });
        cx.spawn_in(window, async move |this, cx| {
            let list = task.await.unwrap_or_default();
            let _ = this.update_in(cx, |this, window, cx| {
                let row = window.rem_size() * ROW_REMS;
                this.prefixes = Some(PrefixState::Loaded {
                    sizes: Rc::new(vec![size(px(0.), row); list.len()]),
                    list: Rc::new(list),
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn render_prefix_rows(
        &mut self,
        range: Range<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(PrefixState::Loaded { list, .. }) = &self.prefixes else {
            return Vec::new();
        };
        let list = list.clone();
        list[range.start..range.end.min(list.len())]
            .iter()
            .enumerate()
            .map(|(i, entry)| prefix_row(range.start + i, entry, cx))
            .collect()
    }

    /// The tab's body, which owns its scrolling.
    pub(super) fn render_prefixes(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        match &self.prefixes {
            None | Some(PrefixState::Loading) => h_flex()
                .px_6()
                .py_4()
                .gap_2()
                .text_sm()
                .text_color(muted)
                .child(Spinner::new().small())
                .child(t("manage.wine.prefixes.loading"))
                .into_any_element(),
            Some(PrefixState::Loaded { list, .. }) if list.is_empty() => div()
                .px_6()
                .py_4()
                .child(
                    EmptyState::new(IconName::Wine, t("manage.wine.prefixes.empty.title"))
                        .description(t("manage.wine.prefixes.empty.description")),
                )
                .into_any_element(),
            Some(PrefixState::Loaded { list, sizes }) => {
                let unused = list.iter().filter(|e| e.unused()).count();
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .px_6()
                    .pt_4()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(div().flex_1().min_w_0().text_sm().text_color(muted).child(
                                if unused == 0 {
                                    t("manage.wine.prefixes.hint")
                                } else {
                                    t_args(
                                        "manage.wine.prefixes.hint_unused",
                                        &[("count", &unused.to_string())],
                                    )
                                },
                            ))
                            .when(unused > 0, |row| {
                                row.child(
                                    Button::new("prefixes-storage")
                                        .outline()
                                        .small()
                                        .icon(IconName::HardDrive)
                                        .label(t("manage.wine.prefixes.open_storage"))
                                        .on_click(|_, _, cx| {
                                            irori_ui::navigate(
                                                Route::Settings("storage".into()),
                                                cx,
                                            )
                                        }),
                                )
                            }),
                    )
                    .child(
                        v_virtual_list(
                            cx.entity(),
                            "wine-prefixes",
                            sizes.clone(),
                            |this, range, window, cx| this.render_prefix_rows(range, window, cx),
                        )
                        .flex_1()
                        .min_h_0()
                        .w_full(),
                    )
                    .into_any_element()
            }
        }
    }
}

fn prefix_row(ix: usize, entry: &PrefixEntry, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let path = entry.path.clone();
    h_flex()
        .h(rems(ROW_REMS))
        .gap_3()
        .px_3()
        .when(ix > 0, |r| r.border_t_1().border_color(theme.border))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    h_flex()
                        .gap_2()
                        .min_w_0()
                        .child(div().min_w_0().font_medium().truncate().child(entry.name()))
                        .when(entry.is_default, |r| {
                            r.child(
                                Tag::secondary()
                                    .xsmall()
                                    .child(t("manage.wine.prefixes.default")),
                            )
                        })
                        .when(entry.is_launcher, |r| {
                            r.child(
                                Tag::secondary()
                                    .xsmall()
                                    .child(t("manage.wine.prefixes.launcher")),
                            )
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .truncate()
                        .child(entry.path.display().to_string()),
                )
                .child(
                    div()
                        .text_sm()
                        .truncate()
                        .when(entry.unused(), |d| d.text_color(theme.muted_foreground))
                        .child(used_by(entry)),
                ),
        )
        .child(
            Button::new(ElementId::Name(format!("prefix-open-{ix}").into()))
                .ghost()
                .small()
                .icon(IconName::FolderOpen)
                .label(t("manage.wine.prefixes.open_folder"))
                .on_click(move |_, _, cx| cx.open_with_system(&path)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::{GameId, Runner};
    use std::path::PathBuf;

    fn entry(games: &[&str], former: &[&str]) -> PrefixEntry {
        PrefixEntry {
            path: PathBuf::from("/p/x"),
            games: games
                .iter()
                .map(|g| (GameId::new(Runner::Gog, *g), g.to_string()))
                .collect(),
            former: former.iter().map(|s| s.to_string()).collect(),
            is_default: false,
            is_launcher: false,
        }
    }

    #[test]
    fn says_who_uses_a_prefix() {
        crate::i18n::register();
        assert_eq!(
            used_by(&entry(&["Hades"], &[])),
            t_args("manage.wine.prefixes.used_by", &[("games", "Hades")])
        );
        assert_eq!(
            used_by(&entry(&["A", "B"], &["C"])),
            t_args(
                "manage.wine.prefixes.used_by_many",
                &[("count", "2"), ("games", "A, B")]
            )
        );
        assert_eq!(
            used_by(&entry(&[], &["Old"])),
            t_args("manage.wine.prefixes.former", &[("games", "Old")])
        );
        assert_eq!(used_by(&entry(&[], &[])), t("manage.wine.prefixes.unused"));
        let mut launcher = entry(&[], &["Old"]);
        launcher.is_launcher = true;
        assert_eq!(used_by(&launcher), t("manage.wine.prefixes.no_games"));
    }
}
