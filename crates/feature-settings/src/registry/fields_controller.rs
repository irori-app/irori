use super::options::{
    BIG_PICTURE_HOLD, IDLE_SCREEN, NAV_SOUNDS, NEEDS_CONTROLLER, NEEDS_OVERLAY, OVERLAY_HELP,
    OVERLAY_SIDE, OVERLAY_TIP,
};
use super::types::{
    FieldKind::{Choice, Panel, Text, Toggle},
    FieldSection::Controller,
    FieldSpec, PanelKind, TextKind, app,
};

pub(super) const FIELDS: &[FieldSpec] = &[
    // ---- Controller ----
    app(
        "panel.button-map",
        "settings.field.panel_button_map.label",
        Controller,
        "settings.group.buttons",
        Panel(PanelKind::ButtonMap),
    )
    .keywords(&[
        "gamepad",
        "controller",
        "buttons",
        "bindings",
        "remap",
        "rebind",
        "confirm",
        "nintendo",
        "glyphs",
        "layout",
    ]),
    app(
        "panel.controllers",
        "settings.field.panel_controllers.label",
        Controller,
        "settings.group.controller",
        Panel(PanelKind::Controllers),
    )
    .tooltip("settings.field.panel_controllers.tip")
    .keywords(&[
        "gamepad",
        "controller",
        "joystick",
        "xbox",
        "playstation",
        "dualsense",
        "steam deck",
        "battery",
    ]),
    app(
        "disableController",
        "settings.field.disable_controller.label",
        Controller,
        "settings.group.controller",
        Toggle,
    )
    .help("settings.field.disable_controller.help")
    .tooltip("settings.field.disable_controller.tip")
    .keywords(&["gamepad", "controller", "joystick"]),
    app(
        "iroriController.hidePointer",
        "settings.field.hide_pointer.label",
        Controller,
        "settings.group.controller",
        Toggle,
    )
    .help("settings.field.hide_pointer.help")
    .keywords(&[
        "mouse",
        "pointer",
        "cursor",
        "hide",
        "gamepad",
        "controller",
    ])
    .requires(NEEDS_CONTROLLER),
    app(
        "gamepadInitialRepeatDelay",
        "settings.field.gamepad_initial_repeat_delay.label",
        Controller,
        "settings.group.controller",
        Text(TextKind::Number { min: 50, max: 500 }),
    )
    .help("settings.field.gamepad_initial_repeat_delay.help")
    .tooltip("settings.field.gamepad_initial_repeat_delay.tip")
    .keywords(&["gamepad", "controller", "repeat", "delay"])
    .requires(NEEDS_CONTROLLER),
    app(
        "gamepadRepeatDelay",
        "settings.field.gamepad_repeat_delay.label",
        Controller,
        "settings.group.controller",
        Text(TextKind::Number { min: 5, max: 150 }),
    )
    .help("settings.field.gamepad_repeat_delay.help")
    .tooltip("settings.field.gamepad_repeat_delay.tip")
    .keywords(&["gamepad", "controller", "repeat", "frequency"])
    .requires(NEEDS_CONTROLLER),
    app(
        "iroriUi.bigPictureHoldButton",
        "settings.field.big_picture_hold_button.label",
        Controller,
        "settings.group.controller",
        Choice(BIG_PICTURE_HOLD),
    )
    .help("settings.field.big_picture_hold_button.help")
    .tooltip("settings.field.big_picture_hold_button.tip")
    .keywords(&[
        "gamepad",
        "controller",
        "hold",
        "start",
        "select",
        "console",
        "big picture",
    ])
    .requires(NEEDS_CONTROLLER),
    app(
        "iroriUi.bigPictureHoldMs",
        "settings.field.big_picture_hold_ms.label",
        Controller,
        "settings.group.controller",
        Text(TextKind::Number {
            min: 300,
            max: 3000,
        }),
    )
    .help("settings.field.big_picture_hold_ms.help")
    .tooltip("settings.field.big_picture_hold_ms.tip")
    .keywords(&["gamepad", "controller", "hold", "delay", "big picture"])
    .requires(NEEDS_CONTROLLER),
    app(
        "iroriOverlay.enabled",
        "settings.field.overlay.label",
        Controller,
        "settings.group.overlay",
        Toggle,
    )
    .help(OVERLAY_HELP)
    .tooltip(OVERLAY_TIP)
    .keywords(&[
        "overlay",
        "in-game",
        "guide",
        "screenshot",
        "volume",
        "gamepad",
        "controller",
    ])
    .requires(NEEDS_CONTROLLER),
    app(
        "iroriBigPicture.sounds",
        "settings.field.nav_sounds.label",
        Controller,
        "settings.group.big_picture",
        Choice(NAV_SOUNDS),
    )
    .help("settings.field.nav_sounds.help")
    .keywords(&[
        "sound",
        "sounds",
        "audio",
        "beep",
        "navigation",
        "big picture",
    ]),
    app(
        "iroriBigPicture.soundVolume",
        "settings.field.nav_sound_volume.label",
        Controller,
        "settings.group.big_picture",
        Text(TextKind::Number { min: 1, max: 100 }),
    )
    .help("settings.field.nav_sound_volume.help")
    .keywords(&["sound", "volume", "loudness", "big picture"]),
    app(
        "iroriBigPicture.idleMinutes",
        "settings.field.idle_minutes.label",
        Controller,
        "settings.group.big_picture",
        Text(TextKind::Number { min: 0, max: 1440 }),
    )
    .help("settings.field.idle_minutes.help")
    .keywords(&[
        "idle",
        "screensaver",
        "attract",
        "slideshow",
        "dim",
        "big picture",
    ]),
    app(
        "iroriBigPicture.idleScreen",
        "settings.field.idle_screen.label",
        Controller,
        "settings.group.big_picture",
        Choice(IDLE_SCREEN),
    )
    .keywords(&["idle", "screensaver", "slideshow", "dim", "big picture"]),
    app(
        "panel.overlay",
        "settings.field.panel_overlay.label",
        Controller,
        "settings.group.overlay",
        Panel(PanelKind::Overlay),
    )
    .keywords(&["overlay", "in-game", "layer-shell", "wayland", "controller"]),
    app(
        "iroriOverlay.side",
        "settings.field.overlay_side.label",
        Controller,
        "settings.group.overlay",
        Choice(OVERLAY_SIDE),
    )
    .keywords(&["overlay", "position", "left", "right", "centre", "center"])
    .requires(NEEDS_OVERLAY),
    app(
        "iroriOverlay.screenshotChord",
        "settings.field.overlay_screenshot_chord.label",
        Controller,
        "settings.group.overlay",
        Toggle,
    )
    .help("settings.field.overlay_screenshot_chord.help")
    .keywords(&["overlay", "guide", "rb", "r1", "screenshot", "shortcut"])
    .requires(NEEDS_OVERLAY),
    // Its own key, so it needs no overlay: Windows and macOS (Linux binds
    // `irori --screenshot` through the replay buffer's shortcuts).
    app(
        "iroriOverlay.screenshotKey",
        "settings.field.overlay_screenshot_key.label",
        Controller,
        "settings.group.overlay",
        Toggle,
    )
    .help("settings.field.overlay_screenshot_key.help")
    .keywords(&["screenshot", "shortcut", "hotkey", "key", "capture"]),
    app(
        "iroriOverlay.volumeChords",
        "settings.field.overlay_volume_chords.label",
        Controller,
        "settings.group.overlay",
        Toggle,
    )
    .help("settings.field.overlay_volume_chords.help")
    .keywords(&["overlay", "guide", "volume", "shortcut"])
    .requires(NEEDS_OVERLAY),
    app(
        "iroriOverlay.momentChord",
        "settings.field.overlay_moment_chord.label",
        Controller,
        "settings.group.overlay",
        Toggle,
    )
    .help("settings.field.overlay_moment_chord.help")
    .keywords(&[
        "overlay",
        "guide",
        "y",
        "triangle",
        "journal",
        "moment",
        "screenshot",
        "shortcut",
    ])
    .requires(NEEDS_OVERLAY),
    app(
        "iroriOverlay.volumeStep",
        "settings.field.overlay_volume_step.label",
        Controller,
        "settings.group.overlay",
        Text(TextKind::Number { min: 1, max: 25 }),
    )
    .help("settings.field.overlay_volume_step.help")
    .keywords(&["overlay", "volume", "step", "percent"])
    .requires(NEEDS_OVERLAY),
    app(
        "iroriOverlay.levelNotice",
        "settings.field.overlay_level_notice.label",
        Controller,
        "settings.group.overlay",
        Toggle,
    )
    .help("settings.field.overlay_level_notice.help")
    .keywords(&["overlay", "volume", "level", "osd", "notice"])
    .requires(NEEDS_OVERLAY),
    // Windows only (`supported_on_host`): Guide reaches Irori elsewhere.
    app(
        "iroriOverlay.backStart",
        "settings.field.overlay_back_start.label",
        Controller,
        "settings.group.overlay",
        Toggle,
    )
    .help("settings.field.overlay_back_start.help")
    .keywords(&[
        "overlay",
        "back",
        "start",
        "view",
        "menu",
        "select",
        "xinput",
        "controller",
    ])
    .requires(NEEDS_OVERLAY),
];
