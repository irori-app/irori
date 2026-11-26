use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowOptions};
use irori_core::{
    GameHudMode, GameId, GlobalHudMode, HudBackend, HudColorPalette, HudPreset, MetalHudAlignment,
    Paths, Runner,
};
use irori_services::Backend;

use super::helpers::custom_fps_text;
use super::{HudPanel, HudPanelScope};

fn test_backend() -> Backend {
    static ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let root = tempfile::tempdir().expect("temp dir").keep();
        Paths::init(Paths::under(&root));
        root
    });
    Backend::start_for_tests(|_, _| Vec::new()).expect("backend")
}

fn open_hud(
    cx: &mut TestAppContext,
    scope: HudPanelScope,
    backend_override: Option<HudBackend>,
) -> (gpui_kit::AnyWindowHandle, Entity<HudPanel>) {
    cx.executor().allow_parking();
    let backend = test_backend();
    cx.update(|cx| {
        gpui_kit::init(cx);
        irori_ui::init(backend, cx);
        crate::init(cx);
    });
    let opened = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| HudPanel::new(scope, backend_override, window, cx))
        })
        .expect("open hud window")
    });
    cx.run_until_parked();
    opened
}

#[gpui_kit::test]
fn mangohud_global_and_metal_panels_render_without_panicking(cx: &mut TestAppContext) {
    let (handle, panel) = open_hud(cx, HudPanelScope::Global, Some(HudBackend::MangoHud));
    cx.update_window(handle, |_, window, cx| {
        panel.update(cx, |p, cx| {
            p.set_global_mode(GlobalHudMode::IroriProfile, window, cx);
            p.edit_mangohud(window, cx, |m| {
                m.palette = HudColorPalette::Custom;
                m.apply_preset(HudPreset::FullDiagnostics);
            });
        });
        window.refresh();
        let _ = window.draw(cx);
    })
    .unwrap();

    let (m_handle, m_panel) = open_hud(cx, HudPanelScope::Global, Some(HudBackend::MetalHud));
    cx.update_window(m_handle, |_, window, cx| {
        m_panel.update(cx, |p, cx| {
            p.set_global_mode(GlobalHudMode::IroriProfile, window, cx);
            p.edit_metal(cx, |m| m.alignment = MetalHudAlignment::BottomLeft);
        });
        window.refresh();
        let _ = window.draw(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn legacy_toggle_shows_on_and_off_clears_it(cx: &mut TestAppContext) {
    let (handle, panel) = open_hud(cx, HudPanelScope::Global, Some(HudBackend::MangoHud));
    cx.update_window(handle, |_, window, cx| {
        irori_ui::backend(cx).settings.update_app(|a| {
            a.hud.mode = GlobalHudMode::Off;
            a.game.show_mangohud = true;
        });
        panel.update(cx, |p, cx| {
            p.reload_from_settings(window, cx);
            assert!(p.global_on(), "legacy showMangohud counts as On");
            assert!(p.is_editing_profile());
            p.set_global_mode(GlobalHudMode::Off, window, cx);
            assert!(!p.global_on());
        });
        assert!(!irori_ui::backend(cx).settings.app().game.show_mangohud);
        let _ = window.draw(cx);
    })
    .unwrap();
}

#[test]
fn custom_fps_field_only_shows_values_outside_the_chips() {
    assert_eq!(custom_fps_text(None), "");
    assert_eq!(custom_fps_text(Some(144)), "");
    assert_eq!(custom_fps_text(Some(176)), "176");
}

#[gpui_kit::test]
fn per_game_custom_clones_global_and_hides_opengl_wrapper_for_non_native_and_steam(
    cx: &mut TestAppContext,
) {
    let native_id = GameId::new(Runner::Sideload, "hud-native-test");
    let (handle, panel) = open_hud(
        cx,
        HudPanelScope::PerGame {
            id: native_id.clone(),
            is_native_linux: true,
            is_steam: false,
        },
        Some(HudBackend::MangoHud),
    );
    cx.update_window(handle, |_, window, cx| {
        irori_ui::backend(cx).settings.update_app(|a| {
            a.hud.mode = GlobalHudMode::IroriProfile;
            a.hud.mangohud.fps_limit = Some(90);
        });
        panel.update(cx, |p, cx| {
            p.reload_from_settings(window, cx);
            assert!(p.show_opengl_legacy());
            p.set_game_mode(GameHudMode::Custom, window, cx);
            assert_eq!(p.active_mangohud().fps_limit, Some(90));
        });
        let _ = window.draw(cx);
    })
    .unwrap();

    let steam_id = GameId::new(Runner::Steam, "620");
    let (s_handle, s_panel) = open_hud(
        cx,
        HudPanelScope::PerGame {
            id: steam_id,
            is_native_linux: true,
            is_steam: true,
        },
        Some(HudBackend::MangoHud),
    );
    cx.update_window(s_handle, |_, window, cx| {
        s_panel.update(cx, |p, _| {
            assert!(
                !p.show_opengl_legacy(),
                "Steam games must never show LD_PRELOAD wrapper"
            );
        });
        let _ = window.draw(cx);
    })
    .unwrap();

    let wine_id = GameId::new(Runner::Sideload, "hud-wine-test");
    let (w_handle, w_panel) = open_hud(
        cx,
        HudPanelScope::PerGame {
            id: wine_id,
            is_native_linux: false,
            is_steam: false,
        },
        Some(HudBackend::MangoHud),
    );
    cx.update_window(w_handle, |_, window, cx| {
        w_panel.update(cx, |p, _| {
            assert!(
                !p.show_opengl_legacy(),
                "Wine/Proton games must never show LD_PRELOAD wrapper"
            );
        });
        let _ = window.draw(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn scope_notice_matches_global_and_third_party_launchers(cx: &mut TestAppContext) {
    let (g_handle, g_panel) = open_hud(cx, HudPanelScope::Global, Some(HudBackend::MangoHud));
    cx.update_window(g_handle, |_, window, cx| {
        g_panel.update(cx, |p, _| {
            assert_eq!(
                p.scope_notice().map(|(id, _)| id),
                Some("hud-global-stores-notice")
            );
        });
        let _ = window.draw(cx);
    })
    .unwrap();

    let (l_handle, l_panel) = open_hud(
        cx,
        HudPanelScope::PerGame {
            id: GameId::new(Runner::Launchers, "battlenet:fenris"),
            is_native_linux: false,
            is_steam: false,
        },
        Some(HudBackend::MangoHud),
    );
    cx.update_window(l_handle, |_, window, cx| {
        l_panel.update(cx, |p, _| {
            assert_eq!(
                p.scope_notice().map(|(id, _)| id),
                Some("hud-launcher-restart-notice")
            );
        });
        let _ = window.draw(cx);
    })
    .unwrap();

    for (id, is_steam) in [
        (GameId::new(Runner::Gog, "1207658924"), false),
        (GameId::new(Runner::Steam, "620"), true),
    ] {
        let (h, p) = open_hud(
            cx,
            HudPanelScope::PerGame {
                id,
                is_native_linux: false,
                is_steam,
            },
            Some(HudBackend::MangoHud),
        );
        cx.update_window(h, |_, window, cx| {
            p.update(cx, |panel, _| {
                assert!(panel.scope_notice().is_none());
            });
            let _ = window.draw(cx);
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn global_steam_snippet_tracks_global_mode(cx: &mut TestAppContext) {
    let (handle, panel) = open_hud(cx, HudPanelScope::Global, Some(HudBackend::MangoHud));
    cx.update_window(handle, |_, window, cx| {
        panel.update(cx, |p, cx| {
            p.set_global_mode(GlobalHudMode::Off, window, cx);
            assert!(p.display.steam_snippet.is_none());

            p.set_global_mode(GlobalHudMode::IroriProfile, window, cx);
            let snippet = p
                .display
                .steam_snippet
                .as_deref()
                .expect("global MangoHud On provides a Steam launch options snippet");
            assert!(snippet.contains("MANGOHUD=1 MANGOHUD_CONFIGFILE="));
            assert!(snippet.contains("global.conf"));
            assert!(snippet.ends_with("%command%"));
        });
        let _ = window.draw(cx);
    })
    .unwrap();

    let (m_handle, m_panel) = open_hud(cx, HudPanelScope::Global, Some(HudBackend::MetalHud));
    cx.update_window(m_handle, |_, window, cx| {
        m_panel.update(cx, |p, cx| {
            p.set_global_mode(GlobalHudMode::Off, window, cx);
            assert!(p.display.steam_snippet.is_none());

            p.set_global_mode(GlobalHudMode::IroriProfile, window, cx);
            let snippet = p
                .display
                .steam_snippet
                .as_deref()
                .expect("global Metal HUD On provides a Steam launch options snippet");
            assert!(snippet.contains("MTL_HUD_ENABLED=1 MTL_HUD_ALIGNMENT="));
            assert!(snippet.ends_with("%command%"));
        });
        let _ = window.draw(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn steam_hud_renders_side_by_side_and_opens_guide_dialog(cx: &mut TestAppContext) {
    let steam_id = GameId::new(Runner::Steam, "620");
    let (handle, panel) = open_hud(
        cx,
        HudPanelScope::PerGame {
            id: steam_id,
            is_native_linux: false,
            is_steam: true,
        },
        Some(HudBackend::MangoHud),
    );
    cx.update_window(handle, |_, window, cx| {
        irori_ui::backend(cx).settings.update_app(|a| {
            a.hud.mode = GlobalHudMode::IroriProfile;
        });
        panel.update(cx, |p, cx| {
            p.reload_from_settings(window, cx);
            assert!(p.display.steam_snippet.is_some());
        });
        window.refresh();
        let _ = window.draw(cx);

        let snippet = panel
            .read(cx)
            .display
            .steam_snippet
            .clone()
            .expect("Steam snippet present");
        crate::hud::steam_guide::open_steam_launch_guide_dialog(
            panel.downgrade(),
            snippet,
            Some("Portal 2".into()),
            "Copy launch options".into(),
            "Copied".into(),
            window,
            cx,
        );
        window.refresh();
        let _ = window.draw(cx);

        panel.update(cx, |p, cx| {
            p.set_game_mode(GameHudMode::Disabled, window, cx);
            assert_eq!(
                p.display.steam_snippet.as_deref(),
                Some("DISABLE_MANGOHUD=1 MANGOHUD=0 %command%")
            );
        });
        window.refresh();
        let _ = window.draw(cx);

        irori_ui::backend(cx).settings.update_app(|a| {
            a.hud.mode = GlobalHudMode::Off;
        });
        panel.update(cx, |p, cx| {
            p.set_game_mode(GameHudMode::InheritGlobal, window, cx);
            p.reload_from_settings(window, cx);
            assert!(p.display.steam_snippet.is_none());
        });
        window.refresh();
        let _ = window.draw(cx);
    })
    .unwrap();
}
