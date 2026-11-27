use gpui_kit::{AppContext as _, Entity, TestAppContext, WindowOptions};
use irori_core::{
    FpsLimitMethod, GameHudMode, GameId, GlobalHudMode, HudBackend, HudColorPalette, HudPreset,
    MetalHudAlignment, Paths, Runner,
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
fn per_game_custom_clones_global_and_shows_opengl_compat_for_native_and_wine_only(
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
            assert!(p.show_opengl_compat());
            p.set_game_mode(GameHudMode::Custom, window, cx);
            assert_eq!(p.active_mangohud().fps_limit, Some(90));
            assert!(p.active_mangohud().uses_opengl_wrapper());
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
                p.show_opengl_compat(),
                "Wine/Proton games show OpenGL compatibility controls for Windows OpenGL titles"
            );
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
                !p.show_opengl_compat(),
                "Steam games must never show LD_PRELOAD wrapper controls"
            );
        });
        let _ = window.draw(cx);
    })
    .unwrap();

    let launcher_id = GameId::new(Runner::Launchers, "battlenet:fenris");
    let (l_handle, l_panel) = open_hud(
        cx,
        HudPanelScope::PerGame {
            id: launcher_id,
            is_native_linux: false,
            is_steam: false,
        },
        Some(HudBackend::MangoHud),
    );
    cx.update_window(l_handle, |_, window, cx| {
        l_panel.update(cx, |p, _| {
            assert!(
                !p.show_opengl_compat(),
                "Third-party launchers use env-only HUD injection without LD_PRELOAD wrapping"
            );
        });
        let _ = window.draw(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn scope_notice_only_shows_for_third_party_launchers(cx: &mut TestAppContext) {
    let (g_handle, g_panel) = open_hud(cx, HudPanelScope::Global, Some(HudBackend::MangoHud));
    cx.update_window(g_handle, |_, window, cx| {
        g_panel.update(cx, |p, _| {
            assert!(
                p.scope_notice().is_none(),
                "Global scope must not duplicate Steam guidance with an extra alert banner"
            );
            assert!(
                !p.steam_expanded,
                "Global Steam launch options start collapsed"
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
            p.env.mangohud_bin_path = Some(std::path::PathBuf::from("/usr/bin/mangohud"));
            p.rebuild_display(cx);
            let snippet = p
                .display
                .steam_snippet
                .as_deref()
                .expect("global MangoHud On provides a Steam launch options snippet");
            assert!(snippet.contains("MANGOHUD_CONFIGFILE="));
            assert!(snippet.contains("global.conf"));
            assert!(snippet.contains("MANGOHUD_PRESETSFILE="));
            assert!(snippet.contains("global_presets.conf"));
            assert!(snippet.ends_with("mangohud --dlsym %command%"));

            p.env.mangohud_bin_path = None;
            p.rebuild_display(cx);
            let fallback = p
                .display
                .steam_snippet
                .as_deref()
                .expect("global MangoHud On falls back to MANGOHUD=1 without wrapper binary");
            assert!(fallback.contains("MANGOHUD=1 MANGOHUD_CONFIGFILE="));
            assert!(fallback.contains("global.conf"));
            assert!(fallback.contains("MANGOHUD_PRESETSFILE="));
            assert!(fallback.contains("global_presets.conf"));
            assert!(fallback.ends_with("%command%"));
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
fn steam_hud_bar_defaults_expanded_per_game_and_opens_guide_dialog(cx: &mut TestAppContext) {
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
            assert!(p.steam_expanded, "Per-game Steam bar starts expanded");
            assert!(p.display.steam_snippet.is_some());
            p.steam_expanded = false;
        });
        window.refresh();
        let _ = window.draw(cx);

        panel.update(cx, |p, _| {
            p.steam_expanded = true;
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

#[gpui_kit::test]
fn layout_tab_renders_both_shortcuts_and_persists_toggle_preset(cx: &mut TestAppContext) {
    let (handle, panel) = open_hud(cx, HudPanelScope::Global, Some(HudBackend::MangoHud));
    cx.update_window(handle, |_, window, cx| {
        panel.update(cx, |p, cx| {
            p.set_global_mode(GlobalHudMode::IroriProfile, window, cx);
            p.active_tab = super::HudTab::Layout;
            assert_eq!(
                p.shortcut.read(cx).combo().as_ref(),
                irori_core::hud::DEFAULT_TOGGLE_HUD
            );
            assert_eq!(
                p.preset_shortcut.read(cx).combo().as_ref(),
                irori_core::hud::DEFAULT_TOGGLE_PRESET
            );
            p.edit_mangohud(window, cx, |m| {
                m.toggle_preset = "Shift_L+F9".into();
            });
            assert_eq!(p.preset_shortcut.read(cx).combo().as_ref(), "Shift_L+F9");
        });
        assert_eq!(
            irori_ui::backend(cx)
                .settings
                .app()
                .hud
                .mangohud
                .toggle_preset,
            "Shift_L+F9"
        );
        window.refresh();
        let _ = window.draw(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn per_game_custom_horizontal_saves_globally_and_orientation_preserves_both_slots(
    cx: &mut TestAppContext,
) {
    let game_a = GameId::new(Runner::Gog, "hud-global-custom-a");
    let (a_handle, a_panel) = open_hud(
        cx,
        HudPanelScope::PerGame {
            id: game_a,
            is_native_linux: false,
            is_steam: false,
        },
        Some(HudBackend::MangoHud),
    );
    cx.update_window(a_handle, |_, window, cx| {
        a_panel.update(cx, |p, cx| {
            p.set_game_mode(GameHudMode::Custom, window, cx);
            // Customize vertical Custom slot first.
            p.edit_mangohud(window, cx, |m| {
                m.apply_preset(HudPreset::BalancedGaming);
                m.gpu_fan = true;
                m.mark_custom();
            });
            assert_eq!(p.active_mangohud().preset, HudPreset::Custom);

            // Switch to HorizontalCompact and customize it -> CustomHorizontal.
            p.edit_mangohud(window, cx, |m| {
                m.apply_preset(HudPreset::HorizontalCompact);
                m.cpu_power = true;
                m.mark_custom();
            });
            assert_eq!(p.active_mangohud().preset, HudPreset::CustomHorizontal);

            // Toggling orientation via Layout switches slots without clobbering either.
            p.edit_mangohud(window, cx, |m| {
                m.set_orientation(irori_core::HudOrientation::Vertical);
            });
            assert_eq!(p.active_mangohud().preset, HudPreset::Custom);
            assert!(p.active_mangohud().gpu_fan);
            assert!(!p.active_mangohud().cpu_power);

            p.edit_mangohud(window, cx, |m| {
                m.set_orientation(irori_core::HudOrientation::Horizontal);
            });
            assert_eq!(p.active_mangohud().preset, HudPreset::CustomHorizontal);
            assert!(p.active_mangohud().cpu_power);
        });
        let saved_app = irori_ui::backend(cx).settings.app().hud.clone();
        assert!(
            saved_app
                .custom_preset
                .as_ref()
                .is_some_and(|s| s.gpu_fan && s.orientation == irori_core::HudOrientation::Vertical)
        );
        assert!(
            saved_app
                .custom_horizontal_preset
                .as_ref()
                .is_some_and(|s| {
                    s.cpu_power && s.orientation == irori_core::HudOrientation::Horizontal
                })
        );
        window.refresh();
        let _ = window.draw(cx);
    })
    .unwrap();

    // Opening the Global panel in the same session immediately sees both
    // globally saved custom slots.
    let (g_handle, g_panel) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| {
                HudPanel::new(
                    HudPanelScope::Global,
                    Some(HudBackend::MangoHud),
                    window,
                    cx,
                )
            })
        })
        .expect("open global hud window")
    });
    cx.run_until_parked();
    cx.update_window(g_handle, |_, window, cx| {
        g_panel.update(cx, |p, cx| {
            p.set_global_mode(GlobalHudMode::IroriProfile, window, cx);
            p.edit_mangohud(window, cx, |m| m.apply_preset(HudPreset::CustomHorizontal));
            assert!(p.active_mangohud().cpu_power);
            assert_eq!(
                p.active_mangohud().orientation,
                irori_core::HudOrientation::Horizontal
            );
            p.edit_mangohud(window, cx, |m| m.apply_preset(HudPreset::Custom));
            assert!(p.active_mangohud().gpu_fan);
            assert_eq!(
                p.active_mangohud().orientation,
                irori_core::HudOrientation::Vertical
            );
        });
        window.refresh();
        let _ = window.draw(cx);
    })
    .unwrap();

    // Another game with its own custom mode also inherits both shared custom slots.
    let game_b = GameId::new(Runner::Gog, "hud-global-custom-b");
    let (b_handle, b_panel) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| {
                HudPanel::new(
                    HudPanelScope::PerGame {
                        id: game_b,
                        is_native_linux: false,
                        is_steam: false,
                    },
                    Some(HudBackend::MangoHud),
                    window,
                    cx,
                )
            })
        })
        .expect("open second game hud window")
    });
    cx.run_until_parked();
    cx.update_window(b_handle, |_, window, cx| {
        b_panel.update(cx, |p, cx| {
            p.set_game_mode(GameHudMode::Custom, window, cx);
            p.edit_mangohud(window, cx, |m| m.apply_preset(HudPreset::CustomHorizontal));
            assert!(p.active_mangohud().cpu_power);
            assert!(p.active_mangohud().has_custom_preset());
            assert!(p.active_mangohud().has_custom_horizontal_preset());
        });
        window.refresh();
        let _ = window.draw(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn appearance_custom_palette_opens_color_wheel_and_updates_slot(cx: &mut TestAppContext) {
    let (handle, panel) = open_hud(cx, HudPanelScope::Global, Some(HudBackend::MangoHud));
    cx.update_window(handle, |_, window, cx| {
        panel.update(cx, |p, cx| {
            p.set_global_mode(GlobalHudMode::IroriProfile, window, cx);
            p.active_tab = super::HudTab::Appearance;
            p.edit_mangohud(window, cx, |m| {
                m.palette = HudColorPalette::Custom;
            });
            p.on_color_popover_change(super::ColorSlot::Gpu, true, window, cx);
            assert_eq!(p.open_color_slot, Some(super::ColorSlot::Gpu));
            p.edit_mangohud_color(super::ColorSlot::Gpu, "FF3366".into(), true, window, cx);
            assert_eq!(p.active_mangohud().custom_colors.gpu_color, "FF3366");
            assert_eq!(
                p.color_inputs[super::ColorSlot::Gpu as usize]
                    .read(cx)
                    .value()
                    .as_ref(),
                "FF3366"
            );
        });
        window.refresh();
        let _ = window.draw(cx);

        assert_eq!(
            irori_ui::backend(cx)
                .settings
                .app()
                .hud
                .mangohud
                .custom_colors
                .gpu_color,
            "FF3366"
        );

        panel.update(cx, |p, cx| {
            p.reset_custom_colors(window, cx);
            assert_eq!(
                p.active_mangohud().custom_colors,
                irori_core::ThemeHudColors::default()
            );
        });
        window.refresh();
        let _ = window.draw(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn pinned_preview_scenes_cycle_stepper_and_shortcut_conflict_render_cleanly(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let backend = test_backend();
    cx.update(|cx| {
        gpui_kit::init(cx);
        irori_ui::init(backend, cx);
        crate::init(cx);
    });
    let (handle, panel) = cx.update(|cx| {
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| {
                HudPanel::new(
                    HudPanelScope::Global,
                    Some(HudBackend::MangoHud),
                    window,
                    cx,
                )
                .with_pinned_preview(true)
            })
        })
        .expect("open pinned hud window")
    });
    cx.run_until_parked();

    cx.update_window(handle, |_, window, cx| {
        panel.update(cx, |p, cx| {
            p.set_global_mode(GlobalHudMode::IroriProfile, window, cx);
            p.preview.update(cx, |prev, cx| {
                prev.set_scene(crate::hud::preview::PreviewScene::Bright, cx);
                prev.step_cycle(1, cx);
                prev.set_scene(crate::hud::preview::PreviewScene::Mixed, cx);
            });
            // Trigger shortcut conflict detection and low-contrast detection.
            p.active_tab = super::HudTab::Layout;
            assert!(!p.has_shortcut_conflict());
            p.edit_mangohud(window, cx, |m| {
                m.position = irori_core::HudPosition::BottomRight;
                m.fps_limit = Some(120);
                m.toggle_preset = m.toggle_hud.clone();
            });
            assert!(p.has_shortcut_conflict());
        });
        window.refresh();
        let _ = window.draw(cx);

        panel.update(cx, |p, cx| {
            p.active_tab = super::HudTab::Appearance;
            p.edit_mangohud(window, cx, |m| {
                m.palette = HudColorPalette::Custom;
                m.custom_colors.background_color = "111111".into();
                m.custom_colors.text_color = "222222".into();
            });
            assert!(p.display.low_contrast_notice.is_some());
        });
        window.refresh();
        let _ = window.draw(cx);
    })
    .unwrap();
}

#[gpui_kit::test]
fn frame_pacing_can_be_changed_and_persisted_while_fps_limit_is_off(cx: &mut TestAppContext) {
    let (handle, panel) = open_hud(cx, HudPanelScope::Global, Some(HudBackend::MangoHud));
    cx.update_window(handle, |_, window, cx| {
        panel.update(cx, |p, cx| {
            p.set_global_mode(GlobalHudMode::IroriProfile, window, cx);
            p.edit_mangohud(window, cx, |m| {
                m.fps_limit = None;
                m.fps_limit_method = FpsLimitMethod::Early;
            });
            assert_eq!(p.active_mangohud().fps_limit, None);
            assert_eq!(p.active_mangohud().fps_limit_method, FpsLimitMethod::Early);
        });
        let saved = irori_ui::backend(cx).settings.app().hud.mangohud.clone();
        assert_eq!(saved.fps_limit, None);
        assert_eq!(saved.fps_limit_method, FpsLimitMethod::Early);
        window.refresh();
        let _ = window.draw(cx);
    })
    .unwrap();
}
