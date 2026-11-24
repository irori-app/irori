//! The Controllers tab: each connected controller with its family and
//! battery, and turning a Bluetooth one off where the system allows it
//! (`irori_services::bluetooth`). Turning off takes A twice on the same
//! row, so a stray press never cuts a controller off mid-game.

use super::*;
use crate::gamepad::Battery;

/// Whether the controller can be turned off from here.
fn can_turn_off(controller: &ControllerInfo) -> bool {
    irori_services::bluetooth::DISCONNECTS && controller.bluetooth.is_some()
}

impl OverlayView {
    /// A on a controller's row: the first press asks for a second, the
    /// second turns it off.
    pub(super) fn controller_pressed(
        &mut self,
        row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(address) = self
            .controllers
            .get(row)
            .filter(|c| can_turn_off(c))
            .and_then(|c| c.bluetooth.clone())
        else {
            self.armed = None;
            return;
        };
        if self.armed != Some(row) {
            self.armed = Some(row);
            self.controller_error = None;
            return;
        }
        self.armed = None;
        let backend = irori_ui::backend(cx);
        let task = irori_ui::run(
            cx,
            async move { backend.disconnect_bluetooth(&address).await },
        );
        // Once off, the reader lists the controllers again and the row goes.
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.controller_error = result.err().map(|e| {
                    t_args(
                        "bigpicture.overlay.turn_off_failed",
                        &[("error", format!("{e:#}").as_str())],
                    )
                });
                cx.notify();
            });
        }));
    }

    pub(super) fn render_controllers(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.controllers.is_empty() {
            return self.empty(
                IconName::Gamepad2,
                t("bigpicture.overlay.no_controllers"),
                Some(t("bigpicture.overlay.no_controllers_hint")),
                cx,
            );
        }
        let button = self.kind(cx).glyph(GamepadInput::Confirm);
        let rows: Vec<AnyElement> = self
            .controllers
            .clone()
            .into_iter()
            .enumerate()
            .map(|(ix, c)| {
                let battery: Option<SharedString> = match c.battery {
                    Battery::Wired => Some(t("bigpicture.overlay.wired")),
                    other => other.label().map(SharedString::from),
                };
                let low = c.battery.is_low();
                let theme = cx.theme();
                let (muted, danger) = (theme.muted_foreground, theme.danger);
                let off: Option<SharedString> = can_turn_off(&c).then(|| {
                    if self.armed == Some(ix) {
                        t_args("bigpicture.overlay.turn_off_again", &[("button", button)])
                    } else {
                        t("bigpicture.overlay.turn_off")
                    }
                });
                let trailing = h_flex()
                    .flex_shrink_0()
                    .gap_3()
                    .items_center()
                    .text_sm()
                    .child(div().text_color(muted).child(c.kind.label()))
                    .when_some(battery, |d, b| {
                        d.child(div().text_color(if low { danger } else { muted }).child(b))
                    })
                    .when_some(off, |d, off| {
                        d.child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .text_color(if self.armed == Some(ix) {
                                    danger
                                } else {
                                    muted
                                })
                                .child(Icon::new(IconName::BluetoothOff))
                                .child(off),
                        )
                    })
                    .into_any_element();
                let icon = if c.bluetooth.is_some() {
                    IconName::Bluetooth
                } else {
                    IconName::Gamepad2
                };
                self.row(
                    Tab::Controllers,
                    ix,
                    icon,
                    c.name.clone().into(),
                    Some(trailing),
                    cx,
                )
            })
            .collect();
        let theme = cx.theme();
        v_flex()
            .id("overlay-controllers")
            .size_full()
            .gap_1()
            .overflow_y_scroll()
            .children(rows)
            .when_some(self.controller_error.clone(), |d, e| {
                d.child(div().pt_2().text_sm().text_color(theme.danger).child(e))
            })
            .into_any_element()
    }
}
