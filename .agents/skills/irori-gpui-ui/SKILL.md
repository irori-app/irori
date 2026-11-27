---
name: irori-gpui-ui
description: Build and modify GPUI views, dialogs, themes, controller navigation, and UI tests in crates/ui, crates/feature-*, and crates/irori using gpui-kit 0.8.0.
---

# GPUI and GPUI Kit UI Engineering

Use this skill whenever changing views, sheets, dialogs, themes, layouts, controller focus, or UI tests in `crates/ui`, `crates/feature-library`, `crates/feature-game`, `crates/feature-manage`, `crates/feature-settings`, `crates/feature-bigpicture`, or `crates/irori`.

## Core rules

- Apply the 1-Click UX Litmus Test (`AGENTS.md` and `docs/UI_CONVENTIONS.md`) to every screen, dialog, and empty/error state: keep the primary path to 1 click ("Play", "Install", "Fix"), auto-fill safe defaults instead of building multi-step wizards, tuck expert knobs under progressive disclosure, pair failures with plain-language copy plus a direct 1-click recovery button, require an explicit 1-click action for destructive tasks or heavy downloads, and keep surfaces clean and fast in dark, light, and Big Picture modes.
- Verify every GPUI or GPUI Kit type, trait, icon, and builder method against the actual crate sources under `~/.cargo/registry/src/index.crates.io-*/{gpui-kit-0.8.0,gpui-component-0.8.0,gpui-base-0.8.0,gpui-pre-0.3.9}` before writing code. Never guess method names from other UI frameworks.
- Never run filesystem, SQLite, network, keyring, or subprocess calls on the GPUI UI thread, and never await a backend future directly without `irori_ui::run` or `irori_ui::run_infallible` (backend futures require the `irori-io` Tokio runtime).
- Never call `WindowHandle::update` directly inside a dialog or sheet callback targeting that same window; wrap re-entrant window updates in `cx.defer`. Always pass `cx` as the second argument to `window.push_notification(note, cx)`, and use `.ok_variant(ButtonVariant::Danger)` with a specific `.ok_text(...)` verb when confirming destructive actions.
- Subscribe only to the models a view renders (`models(cx)` in `crates/ui/src/models.rs`), store subscriptions in `_subscriptions: Vec<Subscription>`, and wait for `Event::LibraryUpdated` rather than raw `Event::LibraryChanged`.
- Keep `Render::render` pure and fast: never compile `Regex`, clone `LibrarySnapshot`, or re-sort game lists per frame, and virtualize any list or grid that can exceed roughly 50 items (`v_virtual_list`, `List`, `DataTable`).
- Style exclusively with `cx.theme().*` tokens and `rem` helpers (`p_4()`, `gap_3()`). Never use raw colors, and never use `px(...)` except for 1 px hairlines and artwork dimensions.
- On Linux CSD windows, read modal dimming from `theme::modal_dim` (`cx.theme().overlay` is transparent on Linux) and compute full-window overlays from `style::visible_bounds` / `style::visible_size` so nothing paints into the 20 px shadow margin. Use `glass::chrome(..)` and `glass::page(..)` for translucency.
- Put `min_w_0()` and `truncate()` on every flex child carrying variable-length or translated text, and leave 30% to 40% horizontal room for longer locales.
- Preserve full keyboard and controller reachability: never bind `F24` (used by `crates/feature-bigpicture/src/gamepad/pointer.rs`), and call `cx.propagate()` when arrow handlers reach the edge of a grid or list so gamepad focus never gets trapped.
- For UI tests (`#[gpui_kit::test]`, see `crates/feature-library/src/ui_tests.rs`), never enable `gpui-kit/test-support`; add `gpui = { package = "gpui-pre", version = "=0.3.9", features = ["test-support"] }` under `[dev-dependencies]` and run headless Linux tests under `xvfb-run -a`.
- For visual changes, inspect both Irori Night (dark) and Irori Day (light) using an isolated `IRORI_HOME` (on headless Linux you can capture both themes in one command with `website/scripts/screenshots/pr_shot.sh target/quick/irori irori://library /tmp/pr-shots`), or state plainly in the pull request that the screen was not visually rendered.

## Few-shot examples

<example>
<input>
Task: Confirm a destructive uninstall action in a GPUI view, run `Backend::uninstall` off the UI thread, and notify the user on error without triggering a re-entrant `WindowHandle::update` failure.
</input>
<output>
```rust
let view = cx.entity().downgrade();
let game_title = game.title.clone();
let game_id = game.id();

window.open_alert_dialog(cx, move |alert, _, _| {
    let view = view.clone();
    let game_id = game_id.clone();
    let game_title = game_title.clone();
    alert
        .title(t_args("game.uninstall.title", &[("title", &game_title)]))
        .description(t("game.uninstall.description"))
        .show_cancel(true)
        .cancel_text(t("game.common.cancel"))
        .ok_text(t("game.uninstall.confirm"))
        .ok_variant(ButtonVariant::Danger)
        .on_ok(move |_, window, cx| {
            let view = view.clone();
            let game_id = game_id.clone();
            let game_title = game_title.clone();
            let handle = window.window_handle();
            // Dialog callbacks run inside the window update; defer follow-up window access.
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, cx| {
                    let backend = irori_ui::backend(cx);
                    let args = RemoveArgs {
                        delete_files: true,
                        ..RemoveArgs::default()
                    };
                    let task = irori_ui::run(cx, async move {
                        backend.uninstall(&game_id, args).await
                    });
                    let _ = view.update(cx, |this, cx| {
                        this.busy = true;
                        cx.spawn_in(window, async move |this, cx| {
                            let result = task.await;
                            let _ = this.update_in(cx, |this, window, cx| {
                                this.busy = false;
                                if let Err(e) = result {
                                    window.push_notification(
                                        Notification::error(format!("{e:#}")).title(t_args(
                                            "game.uninstall.failed",
                                            &[("title", &game_title)],
                                        )),
                                        cx,
                                    );
                                }
                                cx.notify();
                            });
                        })
                        .detach();
                    });
                });
            });
            true
        })
});
```
</output>
</example>

<example>
<input>
Task: Render a compact header row showing a translated status label on the left and an action button on the right without clipping or overflowing on narrow windows or German/Russian text.
</input>
<output>
```rust
h_flex()
    .w_full()
    .items_center()
    .justify_between()
    .gap_3()
    .child(
        div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_color(cx.theme().muted_foreground)
            .child(t_args("manage.wine.prefix_owner", &[("title", &owner_title)])),
    )
    .child(
        Button::new("open-prefix")
            .small()
            .label(t("manage.wine.open_folder"))
            .on_click(cx.listener(Self::on_open_folder)),
    )
```
</output>
</example>
