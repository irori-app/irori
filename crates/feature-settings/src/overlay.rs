//! The "Overlay on this computer" panel of General → In-game overlay: what
//! the overlay can do on this system and desktop (from
//! `irori_bigpicture::overlay::support`, the table the overlay itself goes
//! by) and the controllers it would read.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div,
};
use irori_bigpicture::gamepad;
use irori_bigpicture::overlay::support::{Os, Support, shortcut_label};
use irori_ui::i18n::{t, t_args};

use crate::widgets::panel;

/// The desktop session, as far as the overlay's surface cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Session {
    Wayland,
    X11,
    /// macOS, Windows: one kind of window manager.
    Native,
}

/// One line of the panel: whether it is good news, its translation key and
/// the key's arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusLine {
    pub ok: bool,
    pub key: &'static str,
    pub args: Vec<(&'static str, String)>,
}

fn line(ok: bool, key: &'static str) -> StatusLine {
    StatusLine {
        ok,
        key,
        args: Vec::new(),
    }
}

/// What the panel says on `os` in `session` with `controllers` connected.
pub(crate) fn status_lines(os: Os, session: Session, controllers: usize) -> Vec<StatusLine> {
    let support = Support::of(os);
    if !support.available {
        return vec![line(false, "settings.overlay.status.unavailable")];
    }
    let surface = match (os, session) {
        (Os::Linux, Session::X11) => "settings.overlay.status.surface_x11",
        (Os::Linux, _) => "settings.overlay.status.surface_wayland",
        (Os::MacOs, _) => "settings.overlay.status.surface_macos",
        _ => "settings.overlay.status.surface_windows",
    };
    let mut lines = vec![line(true, surface)];
    lines.push(match (controllers, support.holds_controllers) {
        (0, _) => line(false, "settings.overlay.status.no_controllers"),
        (n, held) => StatusLine {
            ok: held,
            key: if held {
                "settings.overlay.status.controllers_held"
            } else {
                "settings.overlay.status.controllers_shared"
            },
            args: vec![("count", n.to_string())],
        },
    });
    lines.push(if support.game_volume {
        line(true, "settings.overlay.status.game_volume")
    } else {
        line(false, "settings.overlay.status.system_volume_only")
    });
    lines.push(if support.shortcut {
        StatusLine {
            ok: true,
            key: "settings.overlay.status.shortcut",
            args: vec![("keys", shortcut_label(os))],
        }
    } else {
        line(true, "settings.overlay.status.guide_only")
    });
    if support.back_start {
        lines.push(line(true, "settings.overlay.status.back_start"));
    }
    lines
}

/// Shows [`status_lines`] for this computer, following controllers live.
pub(crate) struct OverlayPanel {
    controllers: usize,
    _subscriptions: Vec<Subscription>,
}

impl OverlayPanel {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(model) = gamepad::model(cx) {
            subscriptions.push(cx.observe(&model, |this, _, cx| {
                this.controllers = gamepad::controllers().len();
                cx.notify();
            }));
        }
        Self {
            controllers: gamepad::controllers().len(),
            _subscriptions: subscriptions,
        }
    }
}

impl Render for OverlayPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let session = match cx.compositor_name() {
            "Wayland" => Session::Wayland,
            "X11" => Session::X11,
            _ => Session::Native,
        };
        let lines = status_lines(Os::current(), session, self.controllers);
        let theme = cx.theme();
        let (border, muted_fg, success) = (theme.border, theme.muted_foreground, theme.success);
        let rows = lines.into_iter().enumerate().map(|(ix, l)| {
            let args: Vec<(&str, &str)> = l.args.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let text: SharedString = if args.is_empty() {
                t(l.key)
            } else {
                t_args(l.key, &args)
            };
            h_flex()
                .id(ElementId::Name(format!("overlay-status-{ix}").into()))
                .gap_3()
                .px_3()
                .py_2()
                .items_start()
                .when(ix > 0, |r| r.border_t_1().border_color(border))
                .child(
                    Icon::new(if l.ok {
                        IconName::CircleCheck
                    } else {
                        IconName::Info
                    })
                    .small()
                    .flex_shrink_0()
                    .text_color(if l.ok { success } else { muted_fg }),
                )
                .child(div().flex_1().min_w_0().text_sm().child(text))
        });
        v_flex().w_full().child(panel(cx).children(rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(lines: &[StatusLine]) -> Vec<&'static str> {
        lines.iter().map(|l| l.key).collect()
    }

    #[test]
    fn says_what_this_system_offers() {
        let linux = status_lines(Os::Linux, Session::Wayland, 2);
        assert_eq!(
            keys(&linux),
            [
                "settings.overlay.status.surface_wayland",
                "settings.overlay.status.controllers_held",
                "settings.overlay.status.game_volume",
                "settings.overlay.status.guide_only",
            ]
        );
        assert_eq!(linux[1].args, [("count", "2".to_string())]);
        assert!(linux.iter().all(|l| l.ok));
        let x11 = status_lines(Os::Linux, Session::X11, 0);
        assert_eq!(x11[0].key, "settings.overlay.status.surface_x11");
        assert_eq!(x11[1].key, "settings.overlay.status.no_controllers");
        assert!(!x11[1].ok);

        let mac = status_lines(Os::MacOs, Session::Native, 1);
        assert_eq!(mac[1].key, "settings.overlay.status.controllers_shared");
        assert!(!mac[1].ok, "the game sees the presses too");
        assert_eq!(mac[2].key, "settings.overlay.status.system_volume_only");
        assert_eq!(mac[3].args, [("keys", "⌃⇧O".to_string())]);
        let windows = status_lines(Os::Windows, Session::Native, 1);
        assert_eq!(windows[0].key, "settings.overlay.status.surface_windows");
        assert_eq!(windows[3].args, [("keys", "Ctrl+Shift+O".to_string())]);
        assert_eq!(windows[4].key, "settings.overlay.status.back_start");
        assert_eq!(linux.len(), 4, "Guide reaches Irori on Linux");

        let other = status_lines(Os::Other, Session::Native, 3);
        assert_eq!(keys(&other), ["settings.overlay.status.unavailable"]);
    }

    #[test]
    fn every_status_line_has_english_text() {
        crate::i18n::register();
        for (os, session) in [
            (Os::Linux, Session::Wayland),
            (Os::Linux, Session::X11),
            (Os::MacOs, Session::Native),
            (Os::Windows, Session::Native),
            (Os::Other, Session::Native),
        ] {
            for count in [0, 1] {
                for l in status_lines(os, session, count) {
                    assert_ne!(t(l.key).as_ref(), l.key, "{}", l.key);
                }
            }
        }
    }
}
