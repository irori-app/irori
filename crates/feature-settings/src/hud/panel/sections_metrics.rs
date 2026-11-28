use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, Context, Hsla, IntoElement as _, ParentElement as _, SharedString, Styled as _,
    div, px,
};

use crate::widgets::group_panel;

use super::HudPanel;
use super::helpers::{locked_metric_chip, metric_chip};

impl HudPanel {
    pub(super) fn render_metrics_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let (border, bg, muted_fg, mono_font) = {
            let theme = cx.theme();
            (
                theme.border,
                theme.background,
                theme.muted_foreground,
                theme.mono_font_family.clone(),
            )
        };
        let p = self.active_mangohud();
        let gpu_locked = p.gpu_stats_locked();
        let cpu_locked = p.cpu_stats_locked();

        let gpu_chips = h_flex()
            .flex_wrap()
            .gap_1p5()
            .child(if gpu_locked {
                locked_metric_chip(
                    "hud-metric-gpu-stats",
                    self.labels.metric_gpu_stats.clone(),
                    self.labels.metric_locked_auto.clone(),
                    self.labels.metric_gpu_stats_locked.clone(),
                    cx,
                )
            } else {
                metric_chip(
                    "hud-metric-gpu-stats",
                    self.labels.metric_gpu_stats.clone(),
                    self.labels.metric_gpu_stats_help.clone(),
                    p.gpu_stats,
                    cx.listener(|this, _, window, cx| {
                        this.edit_mangohud(window, cx, |p| {
                            p.gpu_stats = !p.gpu_stats;
                            p.mark_custom();
                        });
                    }),
                )
                .into_any_element()
            })
            .child(metric_chip(
                "hud-metric-vram",
                self.labels.metric_vram.clone(),
                self.labels.metric_vram_help.clone(),
                p.vram,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.vram = !p.vram;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-gpu-temp",
                self.labels.metric_gpu_temp.clone(),
                self.labels.metric_gpu_temp_help.clone(),
                p.gpu_temp,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gpu_temp = !p.gpu_temp;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-gpu-junction",
                self.labels.metric_gpu_junction_temp.clone(),
                self.labels.metric_gpu_junction_temp_help.clone(),
                p.gpu_junction_temp,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gpu_junction_temp = !p.gpu_junction_temp;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-gpu-mem-temp",
                self.labels.metric_gpu_mem_temp.clone(),
                self.labels.metric_gpu_mem_temp_help.clone(),
                p.gpu_mem_temp,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gpu_mem_temp = !p.gpu_mem_temp;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-gpu-clock",
                self.labels.metric_gpu_core_clock.clone(),
                self.labels.metric_gpu_core_clock_help.clone(),
                p.gpu_core_clock,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gpu_core_clock = !p.gpu_core_clock;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-gpu-mem-clock",
                self.labels.metric_gpu_mem_clock.clone(),
                self.labels.metric_gpu_mem_clock_help.clone(),
                p.gpu_mem_clock,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gpu_mem_clock = !p.gpu_mem_clock;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-gpu-power",
                self.labels.metric_gpu_power.clone(),
                self.labels.metric_gpu_power_help.clone(),
                p.gpu_power,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gpu_power = !p.gpu_power;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-gpu-fan",
                self.labels.metric_gpu_fan.clone(),
                self.labels.metric_gpu_fan_help.clone(),
                p.gpu_fan,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gpu_fan = !p.gpu_fan;
                        p.mark_custom();
                    });
                }),
            ));

        let cpu_chips = h_flex()
            .flex_wrap()
            .gap_1p5()
            .child(if cpu_locked {
                locked_metric_chip(
                    "hud-metric-cpu-stats",
                    self.labels.metric_cpu_stats.clone(),
                    self.labels.metric_locked_auto.clone(),
                    self.labels.metric_cpu_stats_locked.clone(),
                    cx,
                )
            } else {
                metric_chip(
                    "hud-metric-cpu-stats",
                    self.labels.metric_cpu_stats.clone(),
                    self.labels.metric_cpu_stats_help.clone(),
                    p.cpu_stats,
                    cx.listener(|this, _, window, cx| {
                        this.edit_mangohud(window, cx, |p| {
                            p.cpu_stats = !p.cpu_stats;
                            p.mark_custom();
                        });
                    }),
                )
                .into_any_element()
            })
            .child(metric_chip(
                "hud-metric-cpu-temp",
                self.labels.metric_cpu_temp.clone(),
                self.labels.metric_cpu_temp_help.clone(),
                p.cpu_temp,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.cpu_temp = !p.cpu_temp;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-cpu-mhz",
                self.labels.metric_cpu_mhz.clone(),
                self.labels.metric_cpu_mhz_help.clone(),
                p.cpu_mhz,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.cpu_mhz = !p.cpu_mhz;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-cpu-power",
                self.labels.metric_cpu_power.clone(),
                self.labels.metric_cpu_power_help.clone(),
                p.cpu_power,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.cpu_power = !p.cpu_power;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-ram",
                self.labels.metric_ram.clone(),
                self.labels.metric_ram_help.clone(),
                p.ram,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.ram = !p.ram;
                        p.mark_custom();
                    });
                }),
            ));

        let fps_chips = h_flex()
            .flex_wrap()
            .gap_1p5()
            .child(metric_chip(
                "hud-metric-fps",
                self.labels.metric_fps.clone(),
                self.labels.metric_fps_help.clone(),
                p.fps,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.fps = !p.fps;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-fps-avg",
                self.labels.metric_fps_avg.clone(),
                self.labels.metric_fps_avg_help.clone(),
                p.fps_avg,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.fps_avg = !p.fps_avg;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-fps-1-low",
                self.labels.metric_fps_1_low.clone(),
                self.labels.metric_fps_1_low_help.clone(),
                p.fps_1_low,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.fps_1_low = !p.fps_1_low;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-frametime",
                self.labels.metric_frametime.clone(),
                self.labels.metric_frametime_help.clone(),
                p.frametime,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.frametime = !p.frametime;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-frame-timing",
                self.labels.metric_frame_timing.clone(),
                self.labels.metric_frame_timing_help.clone(),
                p.frame_timing,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.frame_timing = !p.frame_timing;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-present-mode",
                self.labels.metric_present_mode.clone(),
                self.labels.metric_present_mode_help.clone(),
                p.present_mode,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.present_mode = !p.present_mode;
                        p.mark_custom();
                    });
                }),
            ));

        let fps_label_row = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_3()
            .pt_2p5()
            .border_t_1()
            .border_color(border.opacity(0.65))
            .when(!p.fps, |row| row.opacity(0.55))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_xs()
                            .font_medium()
                            .child(self.labels.metric_fps_use_label.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(self.labels.metric_fps_use_label_help.clone()),
                    ),
            )
            .child(
                Switch::new("hud-metric-fps-use-label")
                    .checked(p.fps_use_label)
                    .disabled(!p.fps)
                    .accessibility_label(self.labels.metric_fps_use_label.clone())
                    .on_change(cx.listener(|this, on: &bool, window, cx| {
                        this.edit_mangohud(window, cx, |p| {
                            p.fps_use_label = *on;
                        });
                    })),
            );

        let runtime_chips = h_flex()
            .flex_wrap()
            .gap_1p5()
            .child(metric_chip(
                "hud-metric-wine",
                self.labels.metric_wine.clone(),
                self.labels.metric_wine_help.clone(),
                p.wine,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.wine = !p.wine;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-winesync",
                self.labels.metric_winesync.clone(),
                self.labels.metric_winesync_help.clone(),
                p.winesync,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.winesync = !p.winesync;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-fsr",
                self.labels.metric_fsr.clone(),
                self.labels.metric_fsr_help.clone(),
                p.fsr,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.fsr = !p.fsr;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-hdr",
                self.labels.metric_hdr.clone(),
                self.labels.metric_hdr_help.clone(),
                p.hdr,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.hdr = !p.hdr;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-gamemode",
                self.labels.metric_gamemode.clone(),
                self.labels.metric_gamemode_help.clone(),
                p.gamemode,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gamemode = !p.gamemode;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-throttling",
                self.labels.metric_throttling_status.clone(),
                self.labels.metric_throttling_status_help.clone(),
                p.throttling_status,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.throttling_status = !p.throttling_status;
                        p.mark_custom();
                    });
                }),
            ))
            .child(metric_chip(
                "hud-metric-battery",
                self.labels.metric_gamepad_battery.clone(),
                self.labels.metric_gamepad_battery_help.clone(),
                p.gamepad_battery,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gamepad_battery = !p.gamepad_battery;
                        p.mark_custom();
                    });
                }),
            ));

        type ClickAction =
            Box<dyn Fn(&gpui_kit::ClickEvent, &mut gpui_kit::Window, &mut gpui_kit::App)>;

        let band = |title: SharedString,
                    count_badge: SharedString,
                    (on_count, total_count): (usize, usize),
                    dot_color: Hsla,
                    all_id: &'static str,
                    clear_id: &'static str,
                    on_all: ClickAction,
                    on_clear: ClickAction,
                    chips: gpui_kit::Div,
                    footer: Option<gpui_kit::Div>| {
            v_flex()
                .w_full()
                .px_4()
                .py_3()
                .gap_2p5()
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .size(px(10.))
                                        .flex_shrink_0()
                                        .rounded_full()
                                        .bg(dot_color)
                                        .border_1()
                                        .border_color(border),
                                )
                                .child(div().text_sm().font_medium().child(title))
                                .child(
                                    div()
                                        .px_1p5()
                                        .py(px(1.))
                                        .rounded(px(5.))
                                        .bg(bg)
                                        .border_1()
                                        .border_color(border)
                                        .font_family(mono_font.clone())
                                        .text_xs()
                                        .text_color(muted_fg)
                                        .child(count_badge),
                                ),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    Button::new(all_id)
                                        .ghost()
                                        .xsmall()
                                        .disabled(on_count == total_count)
                                        .label(self.labels.metrics_all.clone())
                                        .on_click(move |ev, window, cx| on_all(ev, window, cx)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(border)
                                        .child(SharedString::new_static("|")),
                                )
                                .child(
                                    Button::new(clear_id)
                                        .ghost()
                                        .xsmall()
                                        .disabled(on_count == 0)
                                        .label(self.labels.metrics_clear.clone())
                                        .on_click(move |ev, window, cx| on_clear(ev, window, cx)),
                                ),
                        ),
                )
                .child(chips)
                .when_some(footer, |col, extra| col.child(extra))
        };

        let [gpu_badge, cpu_badge, fps_badge, runtime_badge] =
            self.display.band_count_badges.clone();
        let [gpu_counts, cpu_counts, fps_counts, runtime_counts] = self.display.band_counts;
        let [gpu_dot, cpu_dot, fps_dot, runtime_dot] = self.display.band_dots;

        let header = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .pt_2()
            .pb_1()
            .child(irori_ui::style::caption(
                self.labels.group_visible_metrics.clone(),
                cx,
            ))
            .child(
                div()
                    .text_xs()
                    .font_family(mono_font.clone())
                    .text_color(muted_fg)
                    .child(self.display.metrics_total_active.clone()),
            );

        let panel = group_panel(cx)
            .child(band(
                self.labels.group_metrics_gpu.clone(),
                gpu_badge,
                gpu_counts,
                gpu_dot,
                "hud-band-gpu-all",
                "hud-band-gpu-clear",
                Box::new(cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gpu_stats = true;
                        p.gpu_temp = true;
                        p.gpu_junction_temp = true;
                        p.gpu_mem_temp = true;
                        p.gpu_core_clock = true;
                        p.gpu_mem_clock = true;
                        p.gpu_power = true;
                        p.gpu_fan = true;
                        p.vram = true;
                        p.mark_custom();
                    });
                })),
                Box::new(cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.gpu_stats = false;
                        p.gpu_temp = false;
                        p.gpu_junction_temp = false;
                        p.gpu_mem_temp = false;
                        p.gpu_core_clock = false;
                        p.gpu_mem_clock = false;
                        p.gpu_power = false;
                        p.gpu_fan = false;
                        p.vram = false;
                        p.mark_custom();
                    });
                })),
                gpu_chips,
                None,
            ))
            .child(div().border_t_1().border_color(border).child(band(
                self.labels.group_metrics_cpu.clone(),
                cpu_badge,
                cpu_counts,
                cpu_dot,
                "hud-band-cpu-all",
                "hud-band-cpu-clear",
                Box::new(cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.cpu_stats = true;
                        p.cpu_temp = true;
                        p.cpu_mhz = true;
                        p.cpu_power = true;
                        p.ram = true;
                        p.mark_custom();
                    });
                })),
                Box::new(cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.cpu_stats = false;
                        p.cpu_temp = false;
                        p.cpu_mhz = false;
                        p.cpu_power = false;
                        p.ram = false;
                        p.mark_custom();
                    });
                })),
                cpu_chips,
                None,
            )))
            .child(div().border_t_1().border_color(border).child(band(
                self.labels.group_metrics_fps.clone(),
                fps_badge,
                fps_counts,
                fps_dot,
                "hud-band-fps-all",
                "hud-band-fps-clear",
                Box::new(cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.fps = true;
                        p.fps_avg = true;
                        p.fps_1_low = true;
                        p.frametime = true;
                        p.frame_timing = true;
                        p.present_mode = true;
                        p.mark_custom();
                    });
                })),
                Box::new(cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.fps = false;
                        p.fps_avg = false;
                        p.fps_1_low = false;
                        p.frametime = false;
                        p.frame_timing = false;
                        p.present_mode = false;
                        p.mark_custom();
                    });
                })),
                fps_chips,
                Some(fps_label_row),
            )))
            .child(div().border_t_1().border_color(border).child(band(
                self.labels.group_metrics_runtime.clone(),
                runtime_badge,
                runtime_counts,
                runtime_dot,
                "hud-band-runtime-all",
                "hud-band-runtime-clear",
                Box::new(cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.wine = true;
                        p.winesync = true;
                        p.fsr = true;
                        p.hdr = true;
                        p.gamemode = true;
                        p.throttling_status = true;
                        p.gamepad_battery = true;
                        p.mark_custom();
                    });
                })),
                Box::new(cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.wine = false;
                        p.winesync = false;
                        p.fsr = false;
                        p.hdr = false;
                        p.gamemode = false;
                        p.throttling_status = false;
                        p.gamepad_battery = false;
                        p.mark_custom();
                    });
                })),
                runtime_chips,
                None,
            )));

        v_flex()
            .w_full()
            .child(header)
            .child(panel)
            .into_any_element()
    }
}
