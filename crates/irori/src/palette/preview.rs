//! The pane beside the palette's list: the highlighted game's cover, store,
//! state, playtime, last session and play status, or for a command its
//! icon, its name and its key. A view of its own, so moving the highlight
//! redraws only the pane.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, StyledExt as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Action, AnyElement, App, Context, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window, div, rems,
};
use irori_core::GameId;
use irori_ui::components::GameCover;
use irori_ui::i18n::{t, t_args};
use irori_ui::models;
use irori_ui::theme::SizeStep;

use super::PaletteTarget;
use super::view::Host;
use super::view::{Row, state_label};
use crate::actions;

/// The window command a palette row also has a key for.
fn shortcut(target: &PaletteTarget) -> Option<Box<dyn Action>> {
    use irori_ui::Route;
    Some(match target {
        PaletteTarget::Route(Route::Library) => Box::new(actions::GoLibrary),
        PaletteTarget::Route(Route::Downloads) => Box::new(actions::GoDownloads),
        PaletteTarget::Route(Route::Stores) => Box::new(actions::GoStores),
        PaletteTarget::Route(Route::Stats) => Box::new(actions::GoStats),
        PaletteTarget::Route(Route::WineManager) => Box::new(actions::GoWine),
        PaletteTarget::Route(Route::Emulators) => Box::new(actions::GoEmulators),
        PaletteTarget::Route(Route::Accounts) => Box::new(actions::GoAccounts),
        PaletteTarget::Route(Route::Settings(_)) => Box::new(actions::GoSettings),
        PaletteTarget::Route(Route::BigPicture) => Box::new(actions::EnterBigPicture),
        PaletteTarget::Refresh => Box::new(actions::RefreshLibrary),
        PaletteTarget::Random => Box::new(actions::PickRandomGame),
        PaletteTarget::InterfaceSize(SizeStep::Larger) => Box::new(actions::ZoomIn),
        PaletteTarget::InterfaceSize(SizeStep::Smaller) => Box::new(actions::ZoomOut),
        PaletteTarget::InterfaceSize(SizeStep::Reset) => Box::new(actions::ResetZoom),
        _ => return None,
    })
}

/// What the keys do on a game row.
fn game_hint(host: Host) -> SharedString {
    match host {
        Host::Dialog => t("shell.palette.preview_enter_open"),
        Host::Window => t("shell.palette.preview_enter_play"),
    }
}

/// Lists a game's actions (the palette's second level).
pub(super) type ShowActions = Rc<dyn Fn(&GameId, &mut Window, &mut App)>;

pub(super) struct PreviewPane {
    host: Host,
    show_actions: ShowActions,
    row: Option<Row>,
    _subscriptions: Vec<Subscription>,
}

impl PreviewPane {
    pub(super) fn new(host: Host, show_actions: ShowActions, cx: &mut Context<Self>) -> Self {
        let m = models(cx).clone();
        Self {
            host,
            show_actions,
            row: None,
            _subscriptions: vec![
                cx.observe(&m.library, |_, _, cx| cx.notify()),
                cx.observe(&m.statuses, |_, _, cx| cx.notify()),
            ],
        }
    }

    /// Preview `row` (nothing for `None`).
    pub(super) fn show(&mut self, row: Option<Row>, cx: &mut Context<Self>) {
        self.row = row;
        cx.notify();
    }

    fn game(&self, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let Some(game) = row.previewed_game().cloned() else {
            return div().into_any_element();
        };
        // On the first level a game row also offers the game's actions.
        let actions = row.game.is_some().then(|| self.show_actions.clone());
        let id = game.id();
        let (playtime, last, status, favourite) = {
            let library = models(cx).library.read(cx);
            let key = id.key();
            (
                irori_library::format::playtime(library.playtime_minutes(&id)),
                irori_library::format::last_played(
                    library.last_played(&id),
                    irori_library::format::now(),
                ),
                library.user.effective_status(&key),
                library.is_favourite(&id),
            )
        };
        let kind = models(cx).statuses.read(cx).kind(&id);
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let line = |text: SharedString| div().text_sm().text_color(muted).child(text);
        v_flex()
            .gap_2()
            .child(
                div()
                    .flex()
                    .justify_center()
                    .child(GameCover::new(game.clone()).width(rems(8.))),
            )
            .child(
                div()
                    .font_semibold()
                    .line_clamp(2)
                    .child(game.title.clone()),
            )
            .child(line(
                format!(
                    "{} · {}",
                    game.runner().store_name(),
                    state_label(&game, kind)
                )
                .into(),
            ))
            .child(line(match playtime {
                Some(time) => t_args("shell.palette.preview_playtime", &[("time", &time)]),
                None => t("shell.palette.preview_never_played"),
            }))
            .when_some(last, |pane, when| {
                pane.child(line(t_args(
                    "shell.palette.preview_last_played",
                    &[("when", &when)],
                )))
            })
            .child(
                div()
                    .flex()
                    .gap_1()
                    .items_center()
                    .text_sm()
                    .child(Icon::new(irori_ui::play_status::icon(status)).small())
                    .child(irori_ui::play_status::label(status))
                    .when(favourite, |row| {
                        row.child(Icon::new(IconName::Star).small().text_color(muted))
                    }),
            )
            .when_some(actions, |pane, show| {
                pane.child(
                    div()
                        .pt_2()
                        .text_xs()
                        .text_color(muted)
                        .child(game_hint(self.host)),
                )
                .child(
                    Button::new("palette-game-actions")
                        .small()
                        .outline()
                        .icon(IconName::ChevronRight)
                        .label(t("shell.palette.actions"))
                        .on_click(move |_, window, cx| show(&id, window, cx)),
                )
            })
            .into_any_element()
    }

    fn command(&self, row: &Row, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let key =
            shortcut(&row.target).and_then(|a| Kbd::binding_for_action(a.as_ref(), None, window));
        v_flex()
            .gap_2()
            .when_some(row.entry.icon, |pane, icon| {
                pane.child(Icon::new(icon).large().text_color(muted))
            })
            .child(div().font_semibold().child(row.entry.label.clone()))
            .when_some(key, |pane, key| {
                pane.child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .text_sm()
                        .text_color(muted)
                        .child(t("shell.palette.preview_shortcut"))
                        .child(key),
                )
            })
            .child(
                div()
                    .pt_2()
                    .text_xs()
                    .text_color(muted)
                    .child(t("shell.palette.preview_run")),
            )
            .into_any_element()
    }
}

impl Render for PreviewPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let content = match self.row.clone() {
            Some(row) if row.previewed_game().is_some() => self.game(&row, cx),
            Some(row) => self.command(&row, window, cx),
            None => div().into_any_element(),
        };
        v_flex()
            .w(rems(15.))
            .flex_shrink_0()
            .min_h_0()
            .p_4()
            .gap_3()
            .border_l_1()
            .border_color(border)
            .child(content)
    }
}
