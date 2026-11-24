//! "How it starts" (a game's Launch section): the command, wrappers and
//! variables the next launch gets, from `Backend::how_it_starts`, which
//! builds them as the launch does without changing anything. Secret values
//! arrive masked. Read again when the game's settings change, and on
//! Refresh; "Copy as command" copies the line, masked values and all.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    ClipboardItem, Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, UniformListScrollHandle, Window, div, rems, uniform_list,
};
use irori_core::GameId;
use irori_services::how_it_starts::HowItStarts;
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::widgets::muted;

enum Load {
    Loading,
    Ready(HowItStarts),
    Failed(SharedString),
}

pub(crate) struct HowItStartsPanel {
    id: GameId,
    load: Load,
    /// The settings revision the preview was read at.
    revision: u64,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl HowItStartsPanel {
    pub(crate) fn new(id: GameId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = models(cx).settings.clone();
        let sub = cx.observe_in(&settings, window, |this: &mut Self, model, window, cx| {
            let model = model.read(cx);
            let ours = model.last_changed_game.as_ref() == Some(&this.id);
            if model.game_revision != this.revision && ours {
                this.reload(window, cx);
            }
        });
        let mut this = Self {
            id,
            load: Load::Loading,
            revision: models(cx).settings.read(cx).game_revision,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: vec![sub],
        };
        this.reload(window, cx);
        this
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.revision = models(cx).settings.read(cx).game_revision;
        self.load = Load::Loading;
        cx.notify();
        let (b, id) = (backend(cx), self.id.clone());
        let task = irori_ui::run(cx, async move { b.how_it_starts(&id).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.load = match result {
                    Ok(how) => Load::Ready(how),
                    Err(e) => Load::Failed(format!("{e:#}").into()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn copy(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Load::Ready(how) = &self.load else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(how.as_command()));
        let note = if how.masked > 0 {
            t("settings.how_it_starts.copied_masked")
        } else {
            t("settings.how_it_starts.copied")
        };
        window.push_notification(Notification::success(note), cx);
    }
}

impl Render for HowItStartsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let loading = matches!(self.load, Load::Loading);
        let actions = h_flex()
            .gap_2()
            .child(
                Button::new("how-it-starts-refresh")
                    .ghost()
                    .small()
                    .icon(IconName::RefreshCw)
                    .label(t("settings.how_it_starts.refresh"))
                    .loading(loading)
                    .disabled(loading)
                    .on_click(cx.listener(|this, _, window, cx| this.reload(window, cx))),
            )
            .child(
                Button::new("how-it-starts-copy")
                    .ghost()
                    .small()
                    .icon(IconName::Copy)
                    .label(t("settings.how_it_starts.copy"))
                    .disabled(!matches!(&self.load, Load::Ready(h) if !h.handed_over))
                    .on_click(cx.listener(|this, _, window, cx| this.copy(window, cx))),
            );
        let theme = cx.theme();
        let mono = theme.mono_font_family.clone();
        let (border, surface, danger) = (theme.border, theme.muted, theme.danger);
        let radius = theme.radius;
        let body = match &self.load {
            Load::Loading => muted(t("settings.how_it_starts.loading"), cx).into_any_element(),
            Load::Failed(e) => v_flex()
                .gap_1()
                .child(muted(t("settings.how_it_starts.failed"), cx))
                .child(div().text_xs().text_color(danger).child(e.clone()))
                .into_any_element(),
            Load::Ready(how) if how.handed_over => {
                muted(t("settings.how_it_starts.handed_over"), cx).into_any_element()
            }
            Load::Ready(how) => {
                let command = irori_runtime::wrappers::shell_join(&how.command);
                let env = how.env.clone();
                let count = env.len();
                let mut notes: Vec<SharedString> = Vec::new();
                if how.program_from_store {
                    notes.push(t("settings.how_it_starts.program_from_store"));
                }
                if how.sandboxed {
                    notes.push(t("settings.how_it_starts.sandboxed"));
                }
                if how.without_wine {
                    notes.push(t("settings.how_it_starts.without_wine"));
                }
                if how.rosetta {
                    notes.push(t("settings.how_it_starts.rosetta"));
                }
                if let Some(prefix) = &how.prefix {
                    notes.push(t_args(
                        "settings.how_it_starts.prefix",
                        &[("path", &prefix.display().to_string())],
                    ));
                }
                if how.masked > 0 {
                    notes.push(t("settings.how_it_starts.masked"));
                }
                notes.push(t("settings.how_it_starts.store_note"));
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .p_2()
                            .rounded(radius)
                            .border_1()
                            .border_color(border)
                            .bg(surface)
                            .font_family(mono.clone())
                            .text_xs()
                            .child(command),
                    )
                    .child(div().text_xs().font_medium().child(t_args(
                        "settings.how_it_starts.variables",
                        &[("count", &count.to_string())],
                    )))
                    .when(count > 0, |el| {
                        el.child(
                            div().rounded(radius).border_1().border_color(border).child(
                                uniform_list("how-it-starts-env", count, move |range, _, _| {
                                    range
                                        .filter_map(|i| env.get(i).cloned())
                                        .map(|(k, v)| {
                                            div()
                                                .px_2()
                                                .text_xs()
                                                .font_family(mono.clone())
                                                .whitespace_nowrap()
                                                .truncate()
                                                .child(format!("{k}={v}"))
                                        })
                                        .collect()
                                })
                                .track_scroll(&self.scroll)
                                .h(rems(12.)),
                            ),
                        )
                    })
                    .children(notes.into_iter().map(|n| muted(n, cx)))
                    .into_any_element()
            }
        };
        v_flex().gap_2().child(actions).child(body)
    }
}
