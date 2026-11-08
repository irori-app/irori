//! The overlay's panel drawn in a test window over an in-memory store and
//! driven by controller inputs and keys: every tab, the journal editor and
//! the question before quitting.

use gpui_kit::component::WindowExt as _;
use gpui_kit::{
    AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions, px, size,
};
use irori_core::{GameId, Runner};

use super::model::{Choice, Tab};
use super::view::OverlayView;
use crate::gamepad::GamepadInput;
use crate::view::tests::{start_backend, wait_for};

fn open(cx: &mut TestAppContext) -> (gpui_kit::AnyWindowHandle, Entity<OverlayView>) {
    // The view reads from the real Tokio backend.
    cx.executor().allow_parking();
    let backend = start_backend();
    cx.update(|cx| {
        gpui_kit::init(cx);
        irori_ui::init(backend, cx);
        crate::register_catalogs();
        crate::bind_keys(cx);
    });
    let game = GameId {
        runner: Runner::Gog,
        app_name: "a".into(),
    };
    let opened = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(880.), px(620.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| OverlayView::new(game, window, cx)),
        )
        .expect("open test window")
    });
    cx.run_until_parked();
    opened
}

fn press(
    cx: &mut TestAppContext,
    handle: gpui_kit::AnyWindowHandle,
    view: &Entity<OverlayView>,
    input: GamepadInput,
) {
    cx.update_window(handle, |_, window, cx| {
        super::deliver(input, view, window, cx)
    })
    .unwrap();
    cx.run_until_parked();
}

#[gpui_kit::test]
fn tabs_journal_and_quit_question_follow_the_controller(cx: &mut TestAppContext) {
    let (handle, view) = open(cx);
    // The screenshots and achievements arrive from the backend.
    wait_for(cx, |cx| view.read(cx).loaded());
    let tab = |cx: &TestAppContext| cx.read(|cx| view.read(cx).nav().tab());

    for expected in [Tab::Screenshots, Tab::Achievements, Tab::Journal] {
        press(cx, handle, &view, GamepadInput::NextPage);
        assert_eq!(tab(cx), expected);
    }

    // Journal: A opens the note editor; B (Escape) closes it again.
    press(cx, handle, &view, GamepadInput::Confirm);
    let dialog = |cx: &mut TestAppContext| {
        cx.update_window(handle, |_, window, cx| window.has_active_dialog(cx))
            .unwrap()
    };
    assert!(dialog(cx), "the journal editor opened");
    press(cx, handle, &view, GamepadInput::Back);
    assert!(!dialog(cx), "Back closed the editor");

    // The keyboard reaches the overlay through its key context.
    cx.simulate_keystrokes(handle, "pagedown");
    assert_eq!(tab(cx), Tab::Home);

    for _ in 0..10 {
        press(cx, handle, &view, GamepadInput::Down);
    }
    press(cx, handle, &view, GamepadInput::Confirm);
    let confirm = |cx: &TestAppContext| cx.read(|cx| view.read(cx).nav().confirm());
    assert_eq!(confirm(cx), Some(Choice::Cancel), "Cancel is focused");
    press(cx, handle, &view, GamepadInput::Confirm);
    assert_eq!(confirm(cx), None, "Cancel kept the game");
}
