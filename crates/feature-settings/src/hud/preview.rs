//! Live Performance HUD preview card and safe `vkcube` Vulkan test launcher.
//!
//! All preview strings and parsed colors are precomputed into [`CachedPreview`]
//! whenever the profile, detected hardware, theme, or language changes so
//! `Render::render` allocates zero `String`s.

use std::path::PathBuf;
use std::process::Stdio;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Context, Hsla, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div,
    px, rems, rgb,
};
use irori_core::hud::{sanitize_hex6, sanitize_hud_label};
use irori_core::{
    HudBackend, HudColorPalette, HudOrientation, HudPosition, MangoHudProfile, MetalHudAlignment,
    MetalHudSettings, Paths, ThemeHudColors,
};
use irori_runtime::hud::hardware_linux::{LinuxHudEnv, default_gpu_short_label, resolve_pci_dev};
use irori_runtime::hud::mangohud_conf::{
    ConfRenderContext, render_mangohud_conf, resolve_palette_colors,
};
use irori_ui::backend;
use irori_ui::i18n::t;
use irori_ui::theme::theme_hud_colors;
use tokio::sync::oneshot;

use crate::i18n::LanguageStamp;

/// Deterministic sample frametime bar heights (in pixels) for the mini graph.
const FRAMETIME_BARS_PX: [f32; 20] = [
    6.0, 6.5, 6.0, 7.0, 6.0, 6.5, 9.0, 6.0, 6.5, 6.0, 6.0, 7.5, 6.5, 6.0, 6.5, 6.0, 8.0, 6.5, 6.0,
    6.0,
];

/// Sample Metal HUD bar heights (in pixels).
const METAL_BARS_PX: [f32; 16] = [
    5.0, 5.5, 5.0, 6.0, 5.5, 5.0, 7.0, 5.5, 5.0, 5.5, 6.5, 5.0, 5.5, 5.0, 5.5, 5.0,
];

struct PreviewLabels {
    title: SharedString,
    hidden_badge: SharedString,
    test_vulkan: SharedString,
    test_running: SharedString,
    test_failed: SharedString,
    metal_subtitle: SharedString,
}

impl PreviewLabels {
    fn load() -> Self {
        Self {
            title: t("settings.hud.preview.title"),
            hidden_badge: t("settings.hud.preview.hidden_badge"),
            test_vulkan: t("settings.hud.preview.test_vulkan"),
            test_running: t("settings.hud.preview.test_running"),
            test_failed: t("settings.hud.preview.test_failed"),
            metal_subtitle: t("settings.hud.preview.metal_subtitle"),
        }
    }
}

#[derive(Clone, Copy)]
struct PreviewPalette {
    bg: Hsla,
    text: Hsla,
    gpu: Hsla,
    cpu: Hsla,
    vram: Hsla,
    ram: Hsla,
    engine: Hsla,
    frametime: Hsla,
    battery: Hsla,
    accent: Hsla,
}

struct PreviewChip {
    text: SharedString,
    color: Hsla,
}

struct PreviewRow {
    label: SharedString,
    label_color: Hsla,
    chips: Vec<PreviewChip>,
}

struct CachedPreview {
    palette: PreviewPalette,
    rows: Vec<PreviewRow>,
    extras: Vec<PreviewChip>,
    minimal_fps_only: bool,
    fps_only_value: SharedString,
    fps_only_color: Hsla,
    show_frametime_graph: bool,
    horizontal: bool,
    position: HudPosition,
    round_corners: u8,
    hud_compact: bool,
    hud_no_margin: bool,
    alpha: f32,
    no_display: bool,
    metal_alignment: MetalHudAlignment,
}

struct RunningPreview {
    kill_tx: oneshot::Sender<()>,
    done_rx: oneshot::Receiver<()>,
}

/// Live stylized preview of the configured MangoHud or Metal Performance HUD.
pub struct HudPreview {
    backend: HudBackend,
    profile: MangoHudProfile,
    metal: MetalHudSettings,
    env: LinuxHudEnv,
    cache: CachedPreview,
    labels: PreviewLabels,
    language: LanguageStamp,
    theme_stamp: (Hsla, Hsla, Hsla),
    running: Option<RunningPreview>,
    starting: bool,
    active_run_id: u64,
}

impl Drop for HudPreview {
    fn drop(&mut self) {
        if let Some(running) = self.running.take() {
            // Dropping or sending on `kill_tx` wakes the background Tokio task
            // so it immediately kills and reaps the `vkcube` child process.
            let _ = running.kill_tx.send(());
        }
    }
}

impl HudPreview {
    pub fn new(
        backend: HudBackend,
        profile: MangoHudProfile,
        metal: MetalHudSettings,
        env: LinuxHudEnv,
        cx: &mut Context<Self>,
    ) -> Self {
        let theme = cx.theme();
        let theme_stamp = (theme.background, theme.foreground, theme.primary);
        let theme_colors = theme_hud_colors(theme);
        let cache = build_cache(&profile, &metal, &env, &theme_colors);
        Self {
            backend,
            profile,
            metal,
            env,
            cache,
            labels: PreviewLabels::load(),
            language: LanguageStamp::new(),
            theme_stamp,
            running: None,
            starting: false,
            active_run_id: 0,
        }
    }

    /// Update the previewed HUD configuration and recompute cached lines.
    pub fn set_state(
        &mut self,
        backend: HudBackend,
        profile: MangoHudProfile,
        metal: MetalHudSettings,
        env: LinuxHudEnv,
        cx: &mut Context<Self>,
    ) {
        self.backend = backend;
        self.profile = profile;
        self.metal = metal;
        self.env = env;
        self.rebuild_cache(cx);
        cx.notify();
    }

    fn rebuild_cache(&mut self, cx: &Context<Self>) {
        let theme = cx.theme();
        self.theme_stamp = (theme.background, theme.foreground, theme.primary);
        let theme_colors = theme_hud_colors(theme);
        self.cache = build_cache(&self.profile, &self.metal, &self.env, &theme_colors);
    }

    fn launch_vkcube_test(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(vkcube_path) = self.env.vkcube_bin_path.clone() else {
            return;
        };
        let prev = self.running.take();
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        let (done_tx, done_rx) = oneshot::channel::<()>();
        let (started_tx, started_rx) = oneshot::channel::<Result<(), String>>();
        let (exited_tx, exited_rx) = oneshot::channel::<()>();

        self.active_run_id = self.active_run_id.wrapping_add(1);
        let run_id = self.active_run_id;
        self.running = Some(RunningPreview { kill_tx, done_rx });
        self.starting = true;
        cx.notify();

        let conf_path: PathBuf = Paths::get().mangohud_config_file("preview");
        let theme_colors = theme_hud_colors(cx.theme());
        let ctx = ConfRenderContext {
            detected_gpu_label: default_gpu_short_label(
                &self.profile.gpu_selection,
                &self.env.gpus,
            ),
            detected_cpu_label: self.env.cpu_label.clone(),
            pci_dev: resolve_pci_dev(&self.profile.gpu_selection, &self.env.gpus),
            theme_colors: Some(theme_colors),
        };
        let conf_body = render_mangohud_conf(&self.profile, &ctx);

        backend(cx).spawn(async move {
            if let Some(prev_run) = prev {
                let _ = prev_run.kill_tx.send(());
                let _ = prev_run.done_rx.await;
            }
            if let Err(err) = irori_runtime::hud::write_config_atomic(&conf_path, &conf_body).await
            {
                let _ = started_tx.send(Err(err.to_string()));
                let _ = done_tx.send(());
                let _ = exited_tx.send(());
                return;
            }
            let mut cmd = tokio::process::Command::new(&vkcube_path);
            cmd.env("MANGOHUD", "1")
                .env("MANGOHUD_CONFIGFILE", &conf_path)
                .env_remove("MANGOHUD_CONFIG")
                .env_remove("DISABLE_MANGOHUD")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(err) => {
                    let _ = started_tx.send(Err(err.to_string()));
                    let _ = done_tx.send(());
                    let _ = exited_tx.send(());
                    return;
                }
            };
            let _ = started_tx.send(Ok(()));
            tokio::select! {
                _ = child.wait() => {}
                _ = kill_rx => {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                }
            }
            let _ = done_tx.send(());
            let _ = exited_tx.send(());
        });

        let failed_title = self.labels.test_failed.clone();
        cx.spawn_in(window, async move |this, cx| {
            let start_result = started_rx.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.active_run_id != run_id {
                    return;
                }
                this.starting = false;
                if let Ok(Err(err)) = start_result {
                    this.running = None;
                    window.push_notification(
                        Notification::error(SharedString::from(err)).title(failed_title),
                        cx,
                    );
                }
                cx.notify();
            });
            let _ = exited_rx.await;
            let _ = this.update(cx, |this, cx| {
                if this.active_run_id == run_id {
                    this.running = None;
                    this.starting = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn render_mangohud_card(&self, cx: &Context<Self>) -> impl IntoElement {
        let c = &self.cache;
        let mono = cx.theme().mono_font_family.clone();
        let card_pad_x = if c.hud_compact { px(8.) } else { px(12.) };
        let card_pad_y = if c.hud_compact { px(5.) } else { px(8.) };

        let mut card = div()
            .bg(c.palette.bg)
            .rounded(px(f32::from(c.round_corners)))
            .px(card_pad_x)
            .py(card_pad_y)
            .font_family(mono)
            .text_xs()
            .border_1()
            .border_color(gpui_kit::hsla(0., 0., 1., 0.08));

        let inner = if c.minimal_fps_only {
            div()
                .text_base()
                .font_bold()
                .text_color(c.fps_only_color)
                .child(c.fps_only_value.clone())
        } else if c.horizontal {
            let mut bar = h_flex().flex_wrap().items_center().gap_3();
            for row in &c.rows {
                let mut seg = h_flex().items_center().gap_1p5();
                if !row.label.is_empty() {
                    seg = seg.child(
                        div()
                            .max_w(rems(11.))
                            .truncate()
                            .font_semibold()
                            .text_color(row.label_color)
                            .child(row.label.clone()),
                    );
                }
                for chip in &row.chips {
                    seg = seg.child(div().text_color(chip.color).child(chip.text.clone()));
                }
                bar = bar.child(seg);
            }
            if !c.extras.is_empty() {
                let mut extra_seg = h_flex().items_center().gap_2();
                for chip in &c.extras {
                    extra_seg = extra_seg.child(
                        div()
                            .font_medium()
                            .text_color(chip.color)
                            .child(chip.text.clone()),
                    );
                }
                bar = bar.child(extra_seg);
            }
            if c.show_frametime_graph {
                bar = bar.child(render_frametime_bars(c.palette.frametime));
            }
            bar
        } else {
            let mut stack = v_flex().gap(if c.hud_compact { px(2.) } else { px(4.) });
            for row in &c.rows {
                let mut line = h_flex().items_center().gap_2().flex_wrap();
                if !row.label.is_empty() {
                    line = line.child(
                        div()
                            .min_w(rems(3.5))
                            .max_w(rems(11.))
                            .truncate()
                            .font_semibold()
                            .text_color(row.label_color)
                            .child(row.label.clone()),
                    );
                }
                for chip in &row.chips {
                    line = line.child(div().text_color(chip.color).child(chip.text.clone()));
                }
                stack = stack.child(line);
            }
            if !c.extras.is_empty() {
                let mut extra_row = h_flex().flex_wrap().gap_2().pt_0p5();
                for chip in &c.extras {
                    extra_row = extra_row.child(
                        div()
                            .font_medium()
                            .text_color(chip.color)
                            .child(chip.text.clone()),
                    );
                }
                stack = stack.child(extra_row);
            }
            if c.show_frametime_graph {
                stack = stack.child(
                    div()
                        .pt_1()
                        .child(render_frametime_bars(c.palette.frametime)),
                );
            }
            stack
        };

        card = card.child(inner.opacity(c.alpha));
        card
    }

    fn render_metal_card(&self, cx: &Context<Self>) -> impl IntoElement {
        let mono = cx.theme().mono_font_family.clone();
        let bg = gpui_kit::hsla(0., 0., 0.06, 0.84);
        let text = gpui_kit::hsla(0., 0., 0.96, 1.0);
        let dim = gpui_kit::hsla(0., 0., 0.72, 1.0);
        let accent = gpui_kit::hsla(0.38, 0.72, 0.56, 1.0);
        v_flex()
            .bg(bg)
            .rounded(px(8.))
            .px_3()
            .py_2()
            .gap_1()
            .font_family(mono)
            .text_xs()
            .border_1()
            .border_color(gpui_kit::hsla(0., 0., 1., 0.14))
            .child(
                h_flex()
                    .justify_between()
                    .gap_4()
                    .child(
                        div()
                            .font_semibold()
                            .text_color(text)
                            .child(SharedString::new_static("Metal")),
                    )
                    .child(
                        div()
                            .text_color(dim)
                            .child(SharedString::new_static("2560x1440")),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        div()
                            .font_bold()
                            .text_color(accent)
                            .child(SharedString::new_static("120.0 FPS")),
                    )
                    .child(
                        div()
                            .text_color(text)
                            .child(SharedString::new_static("8.3 ms")),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .text_color(dim)
                    .child(SharedString::new_static("GPU 6.1 ms"))
                    .child(SharedString::new_static("MEM 4.2 GB")),
            )
            .child(render_metal_bars(accent))
    }
}

impl Render for HudPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.language.changed() {
            self.labels = PreviewLabels::load();
        }
        let theme = cx.theme();
        let current_stamp = (theme.background, theme.foreground, theme.primary);
        if self.backend == HudBackend::MangoHud
            && self.profile.palette == HudColorPalette::SyncIroriTheme
            && current_stamp != self.theme_stamp
        {
            self.rebuild_cache(cx);
        }

        let theme = cx.theme();
        let border = theme.border;
        let radius = theme.radius_lg;
        let surface = theme.secondary;
        let muted_fg = theme.muted_foreground;
        let stage_bg = gpui_kit::hsla(0.62, 0.16, 0.08, 1.0);
        let stage_border = gpui_kit::hsla(0., 0., 1., 0.08);
        let can_test_vkcube =
            self.backend == HudBackend::MangoHud && self.env.vkcube_bin_path.is_some();
        let is_running = self.running.is_some();

        let header = h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .child(self.labels.title.clone()),
                    )
                    .when(
                        self.backend == HudBackend::MangoHud && self.cache.no_display,
                        |row| {
                            row.child(
                                Tag::secondary()
                                    .xsmall()
                                    .outline()
                                    .child(self.labels.hidden_badge.clone()),
                            )
                        },
                    )
                    .when(self.backend == HudBackend::MetalHud, |row| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(muted_fg)
                                .truncate()
                                .child(self.labels.metal_subtitle.clone()),
                        )
                    }),
            )
            .when(can_test_vkcube, |row| {
                row.child(
                    Button::new("hud-test-vkcube")
                        .outline()
                        .small()
                        .icon(IconName::Play)
                        .loading(self.starting)
                        .disabled(self.starting)
                        .label(if is_running {
                            self.labels.test_running.clone()
                        } else {
                            self.labels.test_vulkan.clone()
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.launch_vkcube_test(window, cx);
                        })),
                )
            });

        let mut stage = v_flex()
            .w_full()
            .min_h(rems(10.5))
            .rounded(theme.radius)
            .bg(stage_bg)
            .border_1()
            .border_color(stage_border)
            .overflow_hidden();

        stage = match self.backend {
            HudBackend::MangoHud => {
                let padded = if self.cache.hud_no_margin {
                    stage.p_0()
                } else {
                    stage.p_3()
                };
                align_mangohud(padded, self.cache.position).child(self.render_mangohud_card(cx))
            }
            HudBackend::MetalHud => align_metal(stage.p_3(), self.cache.metal_alignment)
                .child(self.render_metal_card(cx)),
            HudBackend::Unsupported => stage,
        };

        v_flex()
            .w_full()
            .p_4()
            .gap_3()
            .rounded(radius)
            .bg(surface)
            .border_1()
            .border_color(border)
            .child(header)
            .child(stage)
    }
}

fn align_mangohud(stage: gpui_kit::Div, pos: HudPosition) -> gpui_kit::Div {
    match pos {
        HudPosition::TopLeft => stage.justify_start().items_start(),
        HudPosition::TopCenter => stage.justify_start().items_center(),
        HudPosition::TopRight => stage.justify_start().items_end(),
        HudPosition::MiddleLeft => stage.justify_center().items_start(),
        HudPosition::MiddleRight => stage.justify_center().items_end(),
        HudPosition::BottomLeft => stage.justify_end().items_start(),
        HudPosition::BottomCenter => stage.justify_end().items_center(),
        HudPosition::BottomRight => stage.justify_end().items_end(),
    }
}

fn align_metal(stage: gpui_kit::Div, align: MetalHudAlignment) -> gpui_kit::Div {
    match align {
        MetalHudAlignment::TopLeft => stage.justify_start().items_start(),
        MetalHudAlignment::TopRight => stage.justify_start().items_end(),
        MetalHudAlignment::BottomLeft => stage.justify_end().items_start(),
        MetalHudAlignment::BottomRight => stage.justify_end().items_end(),
    }
}

fn render_frametime_bars(color: Hsla) -> impl IntoElement {
    h_flex().items_end().gap(px(2.)).h(px(14.)).children(
        FRAMETIME_BARS_PX
            .iter()
            .map(|&h| div().w(px(3.)).h(px(h)).rounded(px(1.)).bg(color)),
    )
}

fn render_metal_bars(color: Hsla) -> impl IntoElement {
    h_flex().items_end().gap(px(2.)).h(px(10.)).children(
        METAL_BARS_PX
            .iter()
            .map(|&h| div().w(px(3.)).h(px(h)).rounded(px(1.)).bg(color)),
    )
}

fn hex_to_hsla(hex: &str, fallback: &str, alpha: f32) -> Hsla {
    let clean = sanitize_hex6(hex, fallback);
    let val = u32::from_str_radix(&clean, 16).unwrap_or(0xFFFFFF);
    let mut color: Hsla = rgb(val).into();
    color.a = alpha.clamp(0.0, 1.0);
    color
}

fn resolve_preview_palette(
    profile: &MangoHudProfile,
    theme_colors: &ThemeHudColors,
) -> PreviewPalette {
    let colors = match profile.palette {
        HudColorPalette::MangoHudDefault => ThemeHudColors::default(),
        HudColorPalette::SyncIroriTheme => resolve_palette_colors(profile, Some(theme_colors))
            .unwrap_or_else(|| theme_colors.clone()),
        HudColorPalette::Monochrome => resolve_palette_colors(profile, Some(theme_colors))
            .unwrap_or_else(ThemeHudColors::monochrome),
        HudColorPalette::Custom => resolve_palette_colors(profile, Some(theme_colors))
            .unwrap_or_else(|| profile.custom_colors.clone()),
    };
    let d = ThemeHudColors::default();
    PreviewPalette {
        bg: hex_to_hsla(
            &colors.background_color,
            &d.background_color,
            profile.background_alpha,
        ),
        text: hex_to_hsla(&colors.text_color, &d.text_color, 1.0),
        gpu: hex_to_hsla(&colors.gpu_color, &d.gpu_color, 1.0),
        cpu: hex_to_hsla(&colors.cpu_color, &d.cpu_color, 1.0),
        vram: hex_to_hsla(&colors.vram_color, &d.vram_color, 1.0),
        ram: hex_to_hsla(&colors.ram_color, &d.ram_color, 1.0),
        engine: hex_to_hsla(&colors.engine_color, &d.engine_color, 1.0),
        frametime: hex_to_hsla(&colors.frametime_color, &d.frametime_color, 1.0),
        battery: hex_to_hsla(&colors.battery_color, &d.battery_color, 1.0),
        accent: hex_to_hsla(&colors.accent_color, &d.accent_color, 1.0),
    }
}

fn build_cache(
    profile: &MangoHudProfile,
    metal: &MetalHudSettings,
    env: &LinuxHudEnv,
    theme_colors: &ThemeHudColors,
) -> CachedPreview {
    let palette = resolve_preview_palette(profile, theme_colors);
    let gpu_label: SharedString = {
        let custom = sanitize_hud_label(&profile.gpu_label_override);
        if !custom.is_empty() {
            custom.into()
        } else {
            let detected = default_gpu_short_label(&profile.gpu_selection, &env.gpus);
            if detected.trim().is_empty() {
                SharedString::new_static("GPU")
            } else {
                detected.into()
            }
        }
    };
    let cpu_label: SharedString = {
        let custom = sanitize_hud_label(&profile.cpu_label_override);
        if !custom.is_empty() {
            custom.into()
        } else if !env.cpu_label.trim().is_empty() {
            env.cpu_label.clone().into()
        } else {
            SharedString::new_static("CPU")
        }
    };

    let fps_color = if profile.fps_color_change {
        hex_to_hsla("39F900", "39F900", 1.0)
    } else {
        palette.text
    };

    let mut rows = Vec::new();
    let mut extras = Vec::new();

    if profile.effective_gpu_stats() {
        let mut chips = vec![PreviewChip {
            text: SharedString::new_static("98%"),
            color: palette.text,
        }];
        if profile.gpu_temp {
            chips.push(PreviewChip {
                text: SharedString::new_static("68°C"),
                color: palette.text,
            });
        }
        if profile.gpu_junction_temp {
            chips.push(PreviewChip {
                text: SharedString::new_static("Jnc 79°C"),
                color: palette.text,
            });
        }
        if profile.gpu_mem_temp {
            chips.push(PreviewChip {
                text: SharedString::new_static("Mem 74°C"),
                color: palette.text,
            });
        }
        if profile.gpu_core_clock {
            chips.push(PreviewChip {
                text: SharedString::new_static("2640 MHz"),
                color: palette.text,
            });
        }
        if profile.gpu_power {
            chips.push(PreviewChip {
                text: SharedString::new_static("245W"),
                color: palette.text,
            });
        }
        if profile.gpu_fan {
            chips.push(PreviewChip {
                text: SharedString::new_static("1850 RPM"),
                color: palette.text,
            });
        }
        rows.push(PreviewRow {
            label: gpu_label,
            label_color: palette.gpu,
            chips,
        });
    }

    if profile.vram {
        rows.push(PreviewRow {
            label: SharedString::new_static("VRAM"),
            label_color: palette.vram,
            chips: vec![PreviewChip {
                text: SharedString::new_static("8.4 GB"),
                color: palette.text,
            }],
        });
    }

    if profile.effective_cpu_stats() {
        let mut chips = vec![PreviewChip {
            text: SharedString::new_static("42%"),
            color: palette.text,
        }];
        if profile.cpu_temp {
            chips.push(PreviewChip {
                text: SharedString::new_static("61°C"),
                color: palette.text,
            });
        }
        if profile.cpu_mhz {
            chips.push(PreviewChip {
                text: SharedString::new_static("4950 MHz"),
                color: palette.text,
            });
        }
        if profile.cpu_power {
            chips.push(PreviewChip {
                text: SharedString::new_static("65W"),
                color: palette.text,
            });
        }
        rows.push(PreviewRow {
            label: cpu_label,
            label_color: palette.cpu,
            chips,
        });
    }

    if profile.ram {
        rows.push(PreviewRow {
            label: SharedString::new_static("RAM"),
            label_color: palette.ram,
            chips: vec![PreviewChip {
                text: SharedString::new_static("14.2 GB"),
                color: palette.text,
            }],
        });
    }

    if profile.fps || profile.frametime || profile.present_mode {
        let mut chips = Vec::new();
        if profile.fps {
            chips.push(PreviewChip {
                text: SharedString::new_static("144 FPS"),
                color: fps_color,
            });
        }
        if profile.frametime {
            chips.push(PreviewChip {
                text: SharedString::new_static("6.9 ms"),
                color: palette.frametime,
            });
        }
        if profile.present_mode {
            chips.push(PreviewChip {
                text: SharedString::new_static("Mailbox"),
                color: palette.text,
            });
        }
        if let Some(cap) = profile.fps_limit.filter(|&v| v > 0) {
            chips.push(PreviewChip {
                text: SharedString::from(format!("CAP {cap}")),
                color: palette.accent,
            });
        }
        rows.push(PreviewRow {
            label: SharedString::new_static("VULKAN"),
            label_color: palette.engine,
            chips,
        });
    }

    if profile.wine {
        extras.push(PreviewChip {
            text: SharedString::new_static("Proton 9.0"),
            color: palette.accent,
        });
    }
    if profile.winesync {
        extras.push(PreviewChip {
            text: SharedString::new_static("NTSYNC"),
            color: palette.accent,
        });
    }
    if profile.fsr {
        extras.push(PreviewChip {
            text: SharedString::new_static("FSR ON"),
            color: palette.gpu,
        });
    }
    if profile.hdr {
        extras.push(PreviewChip {
            text: SharedString::new_static("HDR ON"),
            color: palette.accent,
        });
    }
    if profile.gamemode {
        extras.push(PreviewChip {
            text: SharedString::new_static("GAMEMODE ON"),
            color: palette.gpu,
        });
    }
    if profile.throttling_status {
        extras.push(PreviewChip {
            text: SharedString::new_static("THR: NO"),
            color: palette.text,
        });
    }
    if profile.gamepad_battery {
        extras.push(PreviewChip {
            text: SharedString::new_static("PAD 85%"),
            color: palette.battery,
        });
    }

    let minimal_fps_only = profile.is_minimal_fps_only();

    CachedPreview {
        palette,
        rows,
        extras,
        minimal_fps_only,
        fps_only_value: SharedString::new_static("144 FPS"),
        fps_only_color: fps_color,
        show_frametime_graph: profile.frame_timing && !minimal_fps_only,
        horizontal: profile.orientation == HudOrientation::Horizontal,
        position: profile.position,
        round_corners: profile.round_corners,
        hud_compact: profile.hud_compact,
        hud_no_margin: profile.hud_no_margin,
        alpha: profile.alpha.clamp(0.1, 1.0),
        no_display: profile.no_display,
        metal_alignment: metal.alignment,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_cache_omits_zero_fps_limit_cap() {
        let mut profile = MangoHudProfile {
            fps: true,
            fps_limit: Some(0),
            ..Default::default()
        };
        let theme = irori_core::ThemeHudColors::default();
        let cache = build_cache(
            &profile,
            &MetalHudSettings::default(),
            &LinuxHudEnv::default(),
            &theme,
        );
        let has_cap_zero = cache
            .rows
            .iter()
            .flat_map(|r| &r.chips)
            .any(|chip| chip.text.as_ref() == "CAP 0");
        assert!(!has_cap_zero, "fps_limit=Some(0) must not emit CAP 0");

        profile.fps_limit = Some(120);
        let cache = build_cache(
            &profile,
            &MetalHudSettings::default(),
            &LinuxHudEnv::default(),
            &theme,
        );
        let has_cap_120 = cache
            .rows
            .iter()
            .flat_map(|r| &r.chips)
            .any(|chip| chip.text.as_ref() == "CAP 120");
        assert!(has_cap_120, "fps_limit=Some(120) must emit CAP 120");
    }
}
