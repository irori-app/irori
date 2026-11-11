//! What opens above everything: the quick menu, a sheet sliding in from the
//! right, and the question before uninstalling a game.

use gpui_kit::assets::IconName;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Icon, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, Context, InteractiveElement as _, IntoElement,
    ParentElement as _, Role, SharedString, StatefulInteractiveElement as _, Styled as _, div,
    rems,
};
use irori_ui::i18n::{t, t_args};
use irori_ui::motion::{STANDARD, fade_in};

use super::{BigPictureView, Colors, Overlay};
use crate::layout::Metrics;

/// Width of the quick menu sheet, in rem.
const SHEET_W: f32 = 26.0;

impl BigPictureView {
    #[allow(clippy::too_many_arguments)]
    fn overlay_row(
        id: SharedString,
        icon: IconName,
        label: SharedString,
        focused: bool,
        danger: bool,
        loading: bool,
        c: Colors,
        on_click: impl Fn(&mut Self, &mut gpui_kit::Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accent = if danger { c.danger } else { c.foreground };
        h_flex()
            .id(id)
            .role(Role::MenuItem)
            .aria_label(label.clone())
            .aria_selected(focused)
            .when(focused, |d| d.aria_active_descendant())
            .w_full()
            .h(rems(3.))
            .px_4()
            .gap_4()
            .items_center()
            .rounded(c.radius_lg)
            .cursor_pointer()
            .text_lg()
            .map(|d| {
                if focused {
                    d.bg(accent).text_color(c.background)
                } else {
                    // The row under the pointer shows it will take a click.
                    d.text_color(if danger { c.danger } else { c.foreground })
                        .hover(|s| s.bg(c.foreground.opacity(0.08)))
                }
            })
            .child(Icon::new(icon))
            .child(div().flex_1().min_w_0().truncate().child(label))
            .when(loading, |d| d.child(Spinner::new()))
            .on_click(cx.listener(move |this, _, window, cx| {
                on_click(this, window, cx);
                this.focus.focus(window, cx);
            }))
            .into_any_element()
    }

    /// A question before a destructive command: Cancel (choice 0) or the
    /// command (choice 1), centred over a scrim.
    #[allow(clippy::too_many_arguments)]
    fn confirm_dialog(
        id: &'static str,
        title: SharedString,
        description: SharedString,
        (confirm, icon, loading): (SharedString, IconName, bool),
        choice: usize,
        c: Colors,
        m: Metrics,
        on_confirm: impl Fn(&mut Self, &mut gpui_kit::Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let buttons = h_flex()
            .gap_3()
            .child(div().flex_1().child(Self::overlay_row(
                SharedString::from(format!("{id}-cancel")),
                IconName::X,
                t("bigpicture.uninstall.cancel"),
                choice == 0,
                false,
                false,
                c,
                |this, _, cx| {
                    this.overlay = None;
                    cx.notify();
                },
                cx,
            )))
            .child(div().flex_1().child(Self::overlay_row(
                SharedString::from(format!("{id}-confirm")),
                icon,
                confirm,
                choice == 1,
                true,
                loading,
                c,
                on_confirm,
                cx,
            )));
        let dialog = v_flex()
            .id(id)
            .role(Role::AlertDialog)
            .aria_label(title.clone())
            .w(rems(36.))
            .max_w(rems(m.width - 2.0 * m.gutter))
            .p_8()
            .gap_6()
            .rounded(c.radius_lg)
            .bg(c.popover)
            .border_1()
            .border_color(c.border)
            .shadow_lg()
            .child(
                v_flex()
                    .gap_2()
                    .child(div().text_2xl().font_bold().line_clamp(2).child(title))
                    .child(
                        div()
                            .text_lg()
                            .text_color(c.muted_foreground)
                            .child(description),
                    ),
            )
            .child(buttons);
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(fade_in(SharedString::from(format!("{id}-in")), dialog))
            .into_any_element()
    }

    pub(super) fn render_overlay(
        &self,
        c: Colors,
        m: Metrics,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let overlay = self.overlay.clone()?;
        let scrim = div()
            .id("bp-scrim")
            .absolute()
            .inset_0()
            .size_full()
            .bg(c.background.opacity(0.6))
            .on_click(cx.listener(|this, _, _, cx| {
                this.overlay = None;
                cx.notify();
            }));
        let panel = match overlay {
            Overlay::Menu { ix } => {
                let items = self.menu(cx);
                let ix = ix.min(items.len().saturating_sub(1));
                let rows: Vec<AnyElement> = items
                    .into_iter()
                    .enumerate()
                    .map(|(i, item)| {
                        Self::overlay_row(
                            item.id().into(),
                            item.icon(),
                            self.menu_label(item),
                            i == ix,
                            false,
                            item == crate::model::MenuItem::SoundOutput && self.switching_output,
                            c,
                            move |this, window, cx| {
                                this.overlay = Some(Overlay::Menu { ix: i });
                                this.activate_menu_item(item, window, cx);
                            },
                            cx,
                        )
                    })
                    .collect();
                let width = SHEET_W.min(m.width * 0.9);
                v_flex()
                    .id("bp-menu")
                    .role(Role::Dialog)
                    .aria_label(t("bigpicture.menu.title"))
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .w(rems(width))
                    .p_6()
                    .pt(rems(m.top_bar))
                    .gap_2()
                    .bg(c.popover)
                    .border_l_1()
                    .border_color(c.border)
                    .shadow_lg()
                    .child(
                        div()
                            .text_3xl()
                            .font_extrabold()
                            .child(t("bigpicture.menu.title")),
                    )
                    .child(
                        div()
                            .pb_4()
                            .min_w_0()
                            .truncate()
                            .text_color(c.muted_foreground)
                            .child(
                                Self::downloads_summary(cx)
                                    .unwrap_or_else(|| t("bigpicture.menu.no_downloads")),
                            ),
                    )
                    .children(rows)
                    .with_animation(
                        "bp-menu-in",
                        Animation::new(STANDARD).with_easing(gpui_kit::ease_out_quint()),
                        move |el, t| el.right(rems(-width * (1.0 - t))),
                    )
                    .into_any_element()
            }
            Overlay::Uninstall { id, choice } => {
                let title = self
                    .find_game(&id)
                    .map(|g| g.title.clone())
                    .unwrap_or_else(|| id.app_name.to_string());
                let loading = self.pending.contains(&id);
                Self::confirm_dialog(
                    "bp-uninstall",
                    t_args("bigpicture.uninstall.title", &[("title", title.as_str())]),
                    t("bigpicture.uninstall.description"),
                    (t("bigpicture.uninstall.confirm"), IconName::Trash, loading),
                    choice,
                    c,
                    m,
                    move |this, window, cx| {
                        this.overlay = None;
                        this.uninstall(id.clone(), window, cx);
                    },
                    cx,
                )
            }
            Overlay::Power { action, choice } => Self::confirm_dialog(
                "bp-power",
                super::system::power_title(action),
                self.power_description(action, cx),
                (
                    t(crate::model::power_label(action)),
                    crate::model::MenuItem::Power(action).icon(),
                    self.power_pending == Some(action),
                ),
                choice,
                c,
                m,
                move |this, window, cx| this.power(action, window, cx),
                cx,
            ),
            Overlay::DeleteTheme { id, choice } => {
                let name = self
                    .look
                    .user
                    .iter()
                    .find(|u| u.id == id)
                    .map(|u| u.theme.name.clone())
                    .unwrap_or_else(|| id.clone());
                Self::confirm_dialog(
                    "bp-delete-theme",
                    t_args("bigpicture.editor.delete_title", &[("name", name.as_str())]),
                    t("bigpicture.editor.delete_description"),
                    (t("bigpicture.editor.delete"), IconName::Trash, false),
                    choice,
                    c,
                    m,
                    move |this, window, cx| {
                        this.overlay = None;
                        this.delete_theme(id.clone(), window, cx);
                    },
                    cx,
                )
            }
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .size_full()
                .child(fade_in("bp-scrim-in", scrim))
                .child(panel)
                .into_any_element(),
        )
    }
}
