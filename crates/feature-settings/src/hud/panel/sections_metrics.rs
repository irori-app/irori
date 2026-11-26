use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, IntoElement as _, ParentElement as _, SharedString, Styled as _, div,
};

use crate::widgets::group_panel;

use super::HudPanel;
use super::helpers::{locked_metric_chip, metric_chip};

impl HudPanel {
    pub(super) fn render_metrics_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let border = cx.theme().border;
        let muted_fg = cx.theme().muted_foreground;
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
                    self.labels.metric_gpu_stats_locked.clone(),
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
            })
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
            ))
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
            ));

        let cpu_chips = h_flex()
            .flex_wrap()
            .gap_1p5()
            .child(if cpu_locked {
                locked_metric_chip(
                    "hud-metric-cpu-stats",
                    self.labels.metric_cpu_stats.clone(),
                    self.labels.metric_cpu_stats_locked.clone(),
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
            ))
            .child(metric_chip(
                "hud-metric-fps-use-label",
                self.labels.metric_fps_use_label.clone(),
                self.labels.metric_fps_use_label_help.clone(),
                p.fps_use_label,
                cx.listener(|this, _, window, cx| {
                    this.edit_mangohud(window, cx, |p| {
                        p.fps_use_label = !p.fps_use_label;
                        p.mark_custom();
                    });
                }),
            ));

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

        let band = |title: SharedString, chips: gpui_kit::Div| {
            v_flex()
                .w_full()
                .px_4()
                .py_3()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(muted_fg)
                        .child(title),
                )
                .child(chips)
        };

        let [gpu_title, cpu_title, fps_title, runtime_title] = self.display.band_titles.clone();
        group_panel(cx)
            .child(band(gpu_title, gpu_chips))
            .child(
                div()
                    .border_t_1()
                    .border_color(border)
                    .child(band(cpu_title, cpu_chips)),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(border)
                    .child(band(fps_title, fps_chips)),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(border)
                    .child(band(runtime_title, runtime_chips)),
            )
            .into_any_element()
    }
}
