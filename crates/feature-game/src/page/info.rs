//! The body below the header: the screenshot strip across the page, then the
//! tabs (about, achievements, compatibility, requirements, changelog, extras)
//! beside a side column with the details table and the play history.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::skeleton::Skeleton;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, InteractiveElement as _, IntoElement, ParentElement,
    SharedString, Styled, Window, div, rems,
};
use irori_core::{GameId, GameInfo};
use irori_ui::i18n::{t, t_args};
use irori_ui::models;

use super::{GamePage, InfoTab};
use once_cell::sync::Lazy;
use regex::Regex;

use crate::dlc;
use crate::i18n::status_label;
use crate::load::{Load, looks_like_html};
use crate::state::{ActionInputs, PrimaryAction, completion_percent};

/// Rich store text (HTML from GOG, Markdown elsewhere).
pub(super) fn rich_text(id: impl Into<gpui_kit::ElementId>, text: &str) -> AnyElement {
    let text = web_links_only(text);
    if looks_like_html(&text) {
        TextView::html(id, text).into_any_element()
    } else {
        TextView::markdown(id, text).into_any_element()
    }
}

/// `text` with every link that isn't a web address emptied. Store text comes
/// from publishers, and the text view opens links with the system, which
/// runs whatever handles `file:`, a network share or a custom scheme.
fn web_links_only(text: &str) -> String {
    static LINK: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r#"(?ix)
            (?P<before>
                \]\(\s*<?              # [text](address)
              | href\s*=\s*["']?       # <a href="address">
              | (?m:^\s*\[[^\]]+\]:\s*) # [reference]: address
              | <                     # <scheme:address>
            )
            (?P<url>[a-z\\/][^\s"'<>)]*)"#,
        )
        .expect("valid regex")
    });
    LINK.replace_all(text, |c: &regex::Captures| {
        let url = &c["url"];
        let before = &c["before"];
        let bare_tag = before == "<" && !url.contains(':');
        if bare_tag || irori_ui::web_url(url).is_some() {
            c[0].to_string()
        } else {
            format!("{before}#")
        }
    })
    .into_owned()
}

#[cfg(test)]
mod link_tests {
    use super::web_links_only;

    #[test]
    fn keeps_web_links_and_tags_and_empties_the_rest() {
        for kept in [
            "[site](https://example.org/a)",
            "<a href=\"http://example.org\">x</a>",
            "<p>text</p><br/>",
            "[ref]: https://example.org",
        ] {
            assert_eq!(web_links_only(kept), kept);
        }
        for (bad, secret) in [
            ("[x](file:///etc/passwd)", "passwd"),
            ("<a href=\"\\\\host\\share\\x.exe\">x</a>", "share"),
            ("<a href='smb://host/x'>x</a>", "smb"),
            ("[ref]: steam://run/1", "steam"),
            ("<ms-msdt:/id>", "msdt"),
        ] {
            assert!(!web_links_only(bad).contains(secret), "{bad}");
        }
    }
}

/// Width of the details column, in rem.
const SIDE_COLUMN_REM: f32 = 24.;

/// About how wide the main column is on a page `page_rem` wide, in rem: the
/// page less its insets, the other columns and the gaps between them.
pub(super) fn main_column_rem(page_rem: f32) -> f32 {
    let layout = Layout::for_page(page_rem);
    let side = if layout == Layout::Three {
        SIDE_COLUMN_REM + 2.
    } else {
        0.
    };
    let cover = if layout == Layout::Stacked {
        0.
    } else {
        layout.cover_rem() + 2.
    };
    // `px_8` on both sides, and the strip card's own padding.
    page_rem - 4. - 1.5 - cover - side
}

/// How the body's columns are arranged for the page's width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// Cover column, main column, details column.
    Three,
    /// Cover column and main column; the details close the main column.
    Two,
    /// Everything in one column, the cover column first.
    Stacked,
}

impl Layout {
    /// Pages at least this wide (in rem) get three columns.
    const THREE_REM: f32 = 84.;
    /// Pages at least this wide (in rem) get two columns.
    const TWO_REM: f32 = 50.;

    fn for_page(page_rem: f32) -> Self {
        if page_rem >= Self::THREE_REM {
            Layout::Three
        } else if page_rem >= Self::TWO_REM {
            Layout::Two
        } else {
            Layout::Stacked
        }
    }

    /// Width of the cover, in rem.
    fn cover_rem(self) -> f32 {
        match self {
            Layout::Three => 16.,
            Layout::Two => 13.,
            Layout::Stacked => 11.,
        }
    }
}

/// Characters of a changelog shown before "Show full changelog".
const CHANGELOG_EXCERPT_CHARS: usize = 12_000;

/// The start of `text` up to the first heading after `budget` characters, and
/// whether anything was cut. Cutting at a heading keeps entries whole.
pub(super) fn changelog_excerpt(text: &str, budget: usize) -> (&str, bool) {
    if text.len() <= budget {
        return (text, false);
    }
    let html = looks_like_html(text);
    let heading_at = |i: usize| {
        let rest = &text[i..];
        if html {
            let b = rest.as_bytes();
            b.len() > 3
                && b[0] == b'<'
                && (b[1] == b'h' || b[1] == b'H')
                && (b'1'..=b'6').contains(&b[2])
        } else {
            rest.starts_with('#') && (i == 0 || text.as_bytes()[i - 1] == b'\n')
        }
    };
    let cut = text
        .char_indices()
        .map(|(i, _)| i)
        .skip_while(|i| *i < budget)
        .find(|i| heading_at(*i));
    match cut {
        Some(i) => (&text[..i], true),
        None => (text, false),
    }
}

impl GamePage {
    fn tabs(&self) -> Vec<(InfoTab, SharedString)> {
        let mut tabs = vec![(InfoTab::About, t("game.info.tab_about"))];
        if let Some(list) = self.achievements.ready().filter(|a| !a.is_empty()) {
            let unlocked = list.iter().filter(|a| a.is_unlocked()).count();
            tabs.push((
                InfoTab::Achievements,
                t_args(
                    "game.info.tab_achievements",
                    &[(
                        "percent",
                        &completion_percent(unlocked, list.len()).to_string(),
                    )],
                ),
            ));
        }
        let extra = self.extra.ready();
        if self.has_compat() {
            tabs.push((InfoTab::Compatibility, t("game.compat.title")));
        }
        if extra.is_some_and(|e| !e.requirements.is_empty()) {
            tabs.push((InfoTab::Requirements, t("game.info.tab_requirements")));
        }
        if extra.is_some_and(|e| e.changelog.as_ref().is_some_and(|c| !c.trim().is_empty())) {
            tabs.push((InfoTab::Changelog, t("game.info.tab_changelog")));
        }
        // GOG offers offline installers and goodies for every game it sells.
        if self.id.runner == irori_core::Runner::Gog {
            tabs.push((InfoTab::Extras, t("game.info.tab_extras")));
        }
        if self.game.as_ref().is_some_and(|g| crate::mods::applies(g)) {
            tabs.push((InfoTab::Mods, t("game.info.tab_mods")));
        }
        tabs
    }

    pub(super) fn render_body(
        &mut self,
        game: &Arc<GameInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tabs = self.tabs();
        if !tabs.iter().any(|(t, _)| *t == self.tab) {
            self.tab = InfoTab::About;
        }
        let selected = tabs.iter().position(|(t, _)| *t == self.tab).unwrap_or(0);
        let kinds: Vec<InfoTab> = tabs.iter().map(|(t, _)| *t).collect();
        let bar = TabBar::new("game-tabs")
            .underline()
            .selected_index(selected)
            .children(tabs.into_iter().map(|(_, label)| Tab::new().label(label)))
            .on_click(cx.listener(move |this, ix: &usize, window, cx| {
                if let Some(t) = kinds.get(*ix) {
                    this.tab = *t;
                    if *t == InfoTab::Extras {
                        this.load_extras(window, cx);
                    }
                    cx.notify();
                }
            }));
        let content = match self.tab {
            InfoTab::About => self.render_about(game, window, cx),
            InfoTab::Achievements => self.render_achievements(window, cx),
            InfoTab::Compatibility => self
                .render_compat(cx)
                .unwrap_or_else(|| div().into_any_element()),
            InfoTab::Requirements => self.render_requirements(cx),
            InfoTab::Changelog => self.render_changelog(cx),
            InfoTab::Extras => self.render_extras(game, window, cx),
            InfoTab::Mods => self.render_mods(game, window, cx),
        };
        let strip = self.render_screenshot_strip(window, cx);
        let details = self.render_details_panel(game, cx);
        let timeline = self.render_timeline(window, cx);
        let related = self.render_related(game, cx);
        let page = self.page_rem(window).unwrap_or(f32::MAX);
        let layout = Layout::for_page(page);
        let cover = self.render_cover_column(game, layout.cover_rem(), cx);
        let main = v_flex()
            .flex_1()
            .min_w_0()
            .w_full()
            .gap_8()
            .children(strip)
            .child(v_flex().w_full().gap_4().child(bar).child(content))
            .child(timeline)
            .children(related);
        let side = v_flex()
            .gap_4()
            .child(details)
            .children(self.overlay.clone());
        let row = |d: gpui_kit::Div| d.w_full().flex().flex_row().items_start().gap_8();
        let body = match layout {
            Layout::Three => row(div())
                .child(cover)
                .child(main)
                .child(side.w(rems(SIDE_COLUMN_REM)).flex_shrink_0()),
            Layout::Two => row(div()).child(cover).child(main.child(side.w_full())),
            Layout::Stacked => div()
                .w_full()
                .flex()
                .flex_col()
                .items_start()
                .gap_8()
                .child(cover)
                .child(main)
                .child(side.w_full()),
        };
        div()
            .id("game-body")
            .w_full()
            .flex_shrink_0()
            .px_8()
            .pb_8()
            .child(body)
            .into_any_element()
    }

    fn render_about(
        &mut self,
        game: &Arc<GameInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let description = match &self.extra {
            Load::Ready(e) => e
                .description
                .clone()
                .or_else(|| e.short_description.clone())
                .or_else(|| game.description.clone()),
            _ => game.description.clone(),
        }
        .filter(|d| !d.trim().is_empty());
        let body: AnyElement = match (&self.extra, description) {
            (_, Some(text)) => rich_text("game-description", &text),
            (Load::Loading, None) => v_flex()
                .gap_2()
                .child(Skeleton::new().h_4().w_full())
                .child(Skeleton::new().h_4().w_full())
                .child(Skeleton::new().h_4().w(rems(16.)))
                .into_any_element(),
            (Load::Failed(e), None) => v_flex()
                .gap_2()
                .items_start()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t_args(
                            "game.info.store_details_failed",
                            &[("error", e.as_ref())],
                        )),
                )
                .child(
                    Button::new("retry-extra")
                        .small()
                        .outline()
                        .icon(IconName::RefreshCw)
                        .label(t("game.common.retry"))
                        .on_click(cx.listener(|this, _, window, cx| this.load_extra(window, cx))),
                )
                .into_any_element(),
            _ => div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t("game.info.no_description"))
                .into_any_element(),
        };
        let dlcs = self.render_dlcs(game, window, cx);
        v_flex()
            .max_w(rems(52.))
            .gap_6()
            .child(body)
            .children(dlcs)
            .children(self.render_redmod(game, cx))
            .into_any_element()
    }

    fn render_mods(
        &mut self,
        game: &Arc<GameInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let panel = match &self.mods {
            Some(panel) => panel.clone(),
            None => {
                let game = game.clone();
                let panel = cx.new(|cx| crate::mods::ModsPanel::new(game, window, cx));
                self.mods = Some(panel.clone());
                panel
            }
        };
        panel.into_any_element()
    }

    /// Entry to REDmod management for Cyberpunk 2077 on GOG.
    fn render_redmod(&self, game: &GameInfo, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !crate::redmod::applies(game, cx) {
            return None;
        }
        let id = self.id.clone();
        Some(
            v_flex()
                .gap_2()
                .child(super::caption("REDmod", self.ink.muted))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t("game.info.redmod_description")),
                )
                .child(
                    h_flex().child(
                        Button::new("open-redmod")
                            .small()
                            .outline()
                            .icon(IconName::Puzzle)
                            .label(t("game.info.manage_mods"))
                            .on_click(move |_, window, cx| {
                                crate::redmod::open_redmod_sheet(id.clone(), window, cx)
                            }),
                    ),
                )
                .into_any_element(),
        )
    }

    fn render_dlcs(
        &self,
        game: &Arc<GameInfo>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if game.dlcs.is_empty() {
            return None;
        }
        let m = models(cx);
        let queue = &m.downloads.read(cx).queue;
        let statuses = m.statuses.read(cx);
        let base_inputs = ActionInputs::from_game(game, statuses.kind(&self.id), queue, false);
        let base_idle = PrimaryAction::from_inputs(&base_inputs).is_idle();
        let editable = game.is_installed && dlc::dlcs_editable(game);
        let mut col = v_flex().gap_2().child(super::caption(
            t_args(
                "game.info.dlc_title",
                &[("count", &game.dlcs.len().to_string())],
            ),
            self.ink.muted,
        ));
        if !game.is_installed {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t("game.info.dlc_choose_on_install")),
            );
        } else if !editable {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t_args(
                        "game.dlc.not_editable",
                        &[("store", game.runner().store_name())],
                    )),
            );
        }
        for d in &game.dlcs {
            let dlc_id = GameId::new(game.runner(), d.app_name.as_str());
            let busy = statuses.kind(&dlc_id).filter(|k| k.is_busy());
            let queued = queue.active.iter().any(|i| i.id == dlc_id);
            let state: Option<SharedString> = busy
                .map(status_label)
                .or_else(|| queued.then(|| status_label(irori_core::StatusKind::Queued)));
            let base = game.clone();
            let info = d.clone();
            col = col.child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .py_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .child(d.title.clone()),
                    )
                    .when_some(state, |row, s| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(s),
                        )
                    })
                    .when(editable, |row| {
                        row.child(
                            Switch::new(SharedString::from(format!("dlc-toggle-{}", d.app_name)))
                                .checked(d.is_installed)
                                .accessibility_label(t_args(
                                    "game.info.dlc_install_toggle",
                                    &[("title", &d.title)],
                                ))
                                .disabled(
                                    !base_idle
                                        || busy.is_some()
                                        || queued
                                        || !d.owned && !d.is_installed,
                                )
                                .on_change(move |on: &bool, window, cx| {
                                    dlc::toggle_dlc(&base, &info, *on, window, cx)
                                }),
                        )
                    })
                    .when(!editable, |row| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if d.is_installed {
                                    t("game.info.dlc_installed")
                                } else if d.owned {
                                    t("game.info.dlc_owned")
                                } else {
                                    SharedString::default()
                                }),
                        )
                    }),
            );
        }
        Some(col.into_any_element())
    }

    fn render_requirements(&self, cx: &App) -> AnyElement {
        let Some(extra) = self.extra.ready() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let header = h_flex()
            .w_full()
            .gap_4()
            .pb_2()
            .border_b_1()
            .border_color(border)
            .text_xs()
            .font_semibold()
            .text_color(muted)
            .child(div().w(rems(8.)).flex_shrink_0())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(t("game.info.requirements_minimum")),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(t("game.info.requirements_recommended")),
            );
        v_flex()
            .gap_2()
            .child(header)
            .children(extra.requirements.iter().map(|r| {
                h_flex()
                    .w_full()
                    .gap_4()
                    .items_start()
                    .text_sm()
                    .child(
                        div()
                            .w(rems(8.))
                            .flex_shrink_0()
                            .text_color(muted)
                            .child(r.title.clone()),
                    )
                    .child(div().flex_1().min_w_0().child(r.minimum.clone()))
                    .child(div().flex_1().min_w_0().child(r.recommended.clone()))
            }))
            .into_any_element()
    }

    fn render_changelog(&self, cx: &mut Context<Self>) -> AnyElement {
        let text = self
            .extra
            .ready()
            .and_then(|e| e.changelog.clone())
            .unwrap_or_default();
        // Long changelogs (GOG keeps a game's whole history) are slow to lay out,
        // so the latest entries come first and the rest on request.
        let (shown, cut) = if self.changelog_expanded {
            (text.as_str(), false)
        } else {
            changelog_excerpt(&text, CHANGELOG_EXCERPT_CHARS)
        };
        v_flex()
            .max_w(rems(48.))
            .gap_4()
            .items_start()
            .child(rich_text(
                if cut {
                    "game-changelog-latest"
                } else {
                    "game-changelog"
                },
                shown,
            ))
            .when(cut, |col| {
                col.child(
                    Button::new("changelog-show-all")
                        .outline()
                        .label(t("game.info.changelog_show_all"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.changelog_expanded = true;
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Layout, changelog_excerpt};

    #[test]
    fn columns_follow_the_page_width() {
        assert_eq!(Layout::for_page(120.), Layout::Three);
        assert_eq!(Layout::for_page(84.), Layout::Three);
        assert_eq!(Layout::for_page(70.), Layout::Two);
        assert_eq!(Layout::for_page(30.), Layout::Stacked);
    }

    #[test]
    fn changelog_excerpt_cuts_at_a_heading() {
        let html = "<h4>2.0</h4><p>aaaa</p><h4>1.9</h4><p>bbbb</p><h4>1.8</h4>";
        assert_eq!(changelog_excerpt(html, 1000), (html, false));
        assert_eq!(
            changelog_excerpt(html, 10),
            ("<h4>2.0</h4><p>aaaa</p>", true)
        );
        let md = "# 2.0\nfixes\n# 1.9\nmore";
        assert_eq!(changelog_excerpt(md, 3), ("# 2.0\nfixes\n", true));
        assert!(!changelog_excerpt("no headings at all here", 3).1);
    }
}
