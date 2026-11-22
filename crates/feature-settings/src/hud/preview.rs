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

struct PreviewCell {
    text: SharedString,
    color: Hsla,
}

/// One row of MangoHud's 3-column `legacy_layout=false` vertical grid.
struct PreviewGridRow {
    label: SharedString,
    label_color: Hsla,
    col2: Option<PreviewCell>,
    col3: Option<PreviewCell>,
}

/// One horizontal segment for `HudOrientation::Horizontal`.
struct PreviewHorizontalSegment {
    label: SharedString,
    label_color: Hsla,
    cells: Vec<PreviewCell>,
}

struct CachedPreview {
    palette: PreviewPalette,
    grid_rows: Vec<PreviewGridRow>,
    horizontal_segments: Vec<PreviewHorizontalSegment>,
    extras: Vec<PreviewCell>,
    minimal_fps_only: bool,
    fps_only_value: SharedString,
    fps_only_color: Hsla,
    show_frametime_graph: bool,
    frametime_minmax: SharedString,
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
        self.sync_live_vkcube_if_running(cx);
        cx.notify();
    }

    /// Rewrite `<data>/mangohud/preview.conf` in-place whenever a `vkcube`
    /// test window is already open so edits apply live via MangoHud `inotify`.
    pub fn sync_live_vkcube_if_running(&self, cx: &Context<Self>) {
        if self.running.is_none() || self.backend != HudBackend::MangoHud {
            return;
        }
        let conf_path: PathBuf = Paths::get().mangohud_config_file("preview");
        let conf_body = self.render_preview_conf(cx);
        backend(cx).spawn(async move {
            if let Err(err) = irori_runtime::hud::write_config_live(&conf_path, &conf_body).await {
                tracing::warn!("updating live vkcube preview config: {err:#}");
            }
        });
    }

    fn render_preview_conf(&self, cx: &Context<Self>) -> String {
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
        render_mangohud_conf(&self.profile, &ctx)
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
        let conf_body = self.render_preview_conf(cx);

        backend(cx).spawn(async move {
            if let Some(prev_run) = prev {
                let _ = prev_run.kill_tx.send(());
                let _ = prev_run.done_rx.await;
            }
            if let Err(err) = irori_runtime::hud::write_config_live(&conf_path, &conf_body).await {
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
        let card_pad_y = if c.hud_compact { px(6.) } else { px(9.) };

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
            for seg_data in &c.horizontal_segments {
                let mut seg = h_flex().items_center().gap_1p5();
                if !seg_data.label.is_empty() {
                    seg = seg.child(
                        div()
                            .max_w(rems(11.))
                            .truncate()
                            .font_semibold()
                            .text_color(seg_data.label_color)
                            .child(seg_data.label.clone()),
                    );
                }
                for cell in &seg_data.cells {
                    seg = seg.child(div().text_color(cell.color).child(cell.text.clone()));
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
                bar = bar.child(render_frametime_line_compact(c.palette.frametime));
            }
            bar
        } else {
            // MangoHud's `legacy_layout=false` 3-column table layout.
            let mut stack =
                v_flex()
                    .min_w(rems(17.5))
                    .gap(if c.hud_compact { px(1.) } else { px(3.) });
            for row in &c.grid_rows {
                let col1 = div()
                    .w(rems(6.8))
                    .max_w(rems(11.))
                    .truncate()
                    .font_semibold()
                    .text_color(row.label_color)
                    .child(row.label.clone());

                let col2 = h_flex()
                    .w(rems(5.2))
                    .justify_end()
                    .when_some(row.col2.as_ref(), |el, cell| {
                        el.text_color(cell.color).child(cell.text.clone())
                    });

                let col3 = h_flex()
                    .w(rems(5.2))
                    .justify_end()
                    .when_some(row.col3.as_ref(), |el, cell| {
                        el.text_color(cell.color).child(cell.text.clone())
                    });

                stack = stack.child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_2()
                        .child(col1)
                        .child(col2)
                        .child(col3),
                );
            }
            if !c.extras.is_empty() {
                let mut extra_row = h_flex().flex_wrap().gap_2().pt_1();
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
                stack = stack.child(render_frametime_graph_block(
                    c.palette.engine,
                    c.palette.text,
                    c.palette.frametime,
                    c.frametime_minmax.clone(),
                ));
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
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .truncate()
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
                    div().flex_shrink_0().child(
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
                    ),
                )
            });

        let mut stage = v_flex()
            .w_full()
            .min_h(rems(11.5))
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

fn render_frametime_graph_block(
    engine_color: Hsla,
    text_color: Hsla,
    line_color: Hsla,
    minmax: SharedString,
) -> impl IntoElement {
    v_flex()
        .w_full()
        .pt_1()
        .gap_1()
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .text_xs()
                        .text_color(engine_color)
                        .child(SharedString::new_static("Frametime")),
                )
                .child(div().text_xs().text_color(text_color).child(minmax)),
        )
        .child(
            div()
                .w_full()
                .h(px(18.))
                .px_1()
                .flex()
                .items_center()
                .child(div().w_full().h(px(2.)).rounded(px(1.)).bg(line_color)),
        )
}

fn render_frametime_line_compact(color: Hsla) -> impl IntoElement {
    div()
        .w(px(48.))
        .h(px(12.))
        .flex()
        .items_center()
        .child(div().w_full().h(px(2.)).rounded(px(1.)).bg(color))
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

fn push_wrapped_grid_rows(
    grid_rows: &mut Vec<PreviewGridRow>,
    label: SharedString,
    label_color: Hsla,
    cells: Vec<PreviewCell>,
) {
    let mut iter = cells.into_iter();
    let mut first = true;
    while let Some(c2) = iter.next() {
        let c3 = iter.next();
        grid_rows.push(PreviewGridRow {
            label: if first {
                label.clone()
            } else {
                SharedString::new_static("")
            },
            label_color,
            col2: Some(c2),
            col3: c3,
        });
        first = false;
    }
}

fn fps_value_color(sim_fps: u32, profile: &MangoHudProfile, palette: &PreviewPalette) -> Hsla {
    if !profile.fps_color_change {
        return palette.text;
    }
    if sim_fps <= 30 {
        hex_to_hsla("B22222", "B22222", 1.0)
    } else if sim_fps <= 60 {
        hex_to_hsla("FDFD09", "FDFD09", 1.0)
    } else {
        hex_to_hsla("39F900", "39F900", 1.0)
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

    let sim_fps = profile.fps_limit.filter(|&v| v > 0).unwrap_or(144);
    let sim_ms = 1000.0_f32 / (sim_fps as f32);
    let fps_color = fps_value_color(sim_fps, profile, &palette);
    let fps_str: SharedString = format!("{sim_fps} FPS").into();
    let ms_str: SharedString = format!("{sim_ms:.1} ms").into();
    let frametime_minmax: SharedString = format!(
        "min: {:.1}ms, max: {:.1}ms",
        (sim_ms * 0.96).max(0.1),
        sim_ms * 1.04
    )
    .into();

    let mut grid_rows = Vec::new();
    let mut horizontal_segments = Vec::new();
    let mut extras = Vec::new();

    // GPU block (2 value columns per row, wrapping extra metrics onto a continuation line).
    if profile.effective_gpu_stats() {
        let mut cells = vec![PreviewCell {
            text: SharedString::new_static("98%"),
            color: palette.text,
        }];
        if profile.gpu_temp {
            cells.push(PreviewCell {
                text: SharedString::new_static("68°C"),
                color: palette.text,
            });
        }
        if profile.gpu_junction_temp {
            cells.push(PreviewCell {
                text: SharedString::new_static("79°C Jnc"),
                color: palette.text,
            });
        }
        // When VRAM is off, keep VRAM temp visible inside the GPU block; when VRAM is on,
        // MangoHud displays `gpu_mem_temp` in column 3 of the VRAM row.
        if profile.gpu_mem_temp && !profile.vram {
            cells.push(PreviewCell {
                text: SharedString::new_static("74°C Mem"),
                color: palette.text,
            });
        }
        if profile.gpu_core_clock {
            cells.push(PreviewCell {
                text: SharedString::new_static("2640 MHz"),
                color: palette.text,
            });
        }
        if profile.gpu_power {
            cells.push(PreviewCell {
                text: SharedString::new_static("245.0 W"),
                color: palette.text,
            });
        }
        if profile.gpu_fan {
            cells.push(PreviewCell {
                text: SharedString::new_static("1850 RPM"),
                color: palette.text,
            });
        }
        let h_cells: Vec<PreviewCell> = cells
            .iter()
            .map(|c| PreviewCell {
                text: c.text.clone(),
                color: c.color,
            })
            .collect();
        push_wrapped_grid_rows(&mut grid_rows, gpu_label.clone(), palette.gpu, cells);
        horizontal_segments.push(PreviewHorizontalSegment {
            label: gpu_label,
            label_color: palette.gpu,
            cells: h_cells,
        });
    }

    // VRAM row (`VRAM  8.4 GiB  74°C` matching real MangoHud).
    if profile.vram {
        let vram_cell = PreviewCell {
            text: SharedString::new_static("8.4 GiB"),
            color: palette.text,
        };
        let mem_temp_cell = profile.gpu_mem_temp.then_some(PreviewCell {
            text: SharedString::new_static("74°C"),
            color: palette.text,
        });
        let mut h_cells = vec![PreviewCell {
            text: vram_cell.text.clone(),
            color: vram_cell.color,
        }];
        if let Some(ref mt) = mem_temp_cell {
            h_cells.push(PreviewCell {
                text: mt.text.clone(),
                color: mt.color,
            });
        }
        grid_rows.push(PreviewGridRow {
            label: SharedString::new_static("VRAM"),
            label_color: palette.vram,
            col2: Some(vram_cell),
            col3: mem_temp_cell,
        });
        horizontal_segments.push(PreviewHorizontalSegment {
            label: SharedString::new_static("VRAM"),
            label_color: palette.vram,
            cells: h_cells,
        });
    }

    // CPU block (2 value columns per row, wrapping clock/power onto a continuation line).
    if profile.effective_cpu_stats() {
        let mut cells = vec![PreviewCell {
            text: SharedString::new_static("42%"),
            color: palette.text,
        }];
        if profile.cpu_temp {
            cells.push(PreviewCell {
                text: SharedString::new_static("61°C"),
                color: palette.text,
            });
        }
        if profile.cpu_mhz {
            cells.push(PreviewCell {
                text: SharedString::new_static("4950 MHz"),
                color: palette.text,
            });
        }
        if profile.cpu_power {
            cells.push(PreviewCell {
                text: SharedString::new_static("65.0 W"),
                color: palette.text,
            });
        }
        let h_cells: Vec<PreviewCell> = cells
            .iter()
            .map(|c| PreviewCell {
                text: c.text.clone(),
                color: c.color,
            })
            .collect();
        push_wrapped_grid_rows(&mut grid_rows, cpu_label.clone(), palette.cpu, cells);
        horizontal_segments.push(PreviewHorizontalSegment {
            label: cpu_label,
            label_color: palette.cpu,
            cells: h_cells,
        });
    }

    // RAM row.
    if profile.ram {
        grid_rows.push(PreviewGridRow {
            label: SharedString::new_static("RAM"),
            label_color: palette.ram,
            col2: Some(PreviewCell {
                text: SharedString::new_static("14.2 GiB"),
                color: palette.text,
            }),
            col3: None,
        });
        horizontal_segments.push(PreviewHorizontalSegment {
            label: SharedString::new_static("RAM"),
            label_color: palette.ram,
            cells: vec![PreviewCell {
                text: SharedString::new_static("14.2 GiB"),
                color: palette.text,
            }],
        });
    }

    // FPS & Frametime block (mirrors MangoHud's `VULKAN` or `FPS` label + `fps_metrics`).
    if profile.fps || profile.frametime || profile.present_mode {
        let mut cells = Vec::new();
        if profile.fps {
            cells.push(PreviewCell {
                text: fps_str.clone(),
                color: fps_color,
            });
        }
        if profile.frametime {
            cells.push(PreviewCell {
                text: ms_str,
                color: palette.text,
            });
        }
        if profile.present_mode {
            cells.push(PreviewCell {
                text: SharedString::new_static("Mailbox"),
                color: palette.text,
            });
        }
        let row_label = if profile.fps_use_label {
            SharedString::new_static("FPS")
        } else {
            SharedString::new_static("VULKAN")
        };
        let h_cells: Vec<PreviewCell> = cells
            .iter()
            .map(|c| PreviewCell {
                text: c.text.clone(),
                color: c.color,
            })
            .collect();
        push_wrapped_grid_rows(&mut grid_rows, row_label.clone(), palette.engine, cells);
        horizontal_segments.push(PreviewHorizontalSegment {
            label: row_label,
            label_color: palette.engine,
            cells: h_cells,
        });
    }

    if profile.fps_avg || profile.fps_1_low {
        let avg_fps = ((sim_fps as f32) * 0.98).round().max(1.0) as u32;
        let low_1_fps = ((sim_fps as f32) * 0.85).round().max(1.0) as u32;
        let mut h_cells = Vec::new();
        if profile.fps_avg {
            let avg_text: SharedString = format!("{avg_fps} FPS").into();
            grid_rows.push(PreviewGridRow {
                label: SharedString::new_static("AVG"),
                label_color: palette.engine,
                col2: Some(PreviewCell {
                    text: avg_text.clone(),
                    color: palette.text,
                }),
                col3: None,
            });
            h_cells.push(PreviewCell {
                text: format!("AVG {avg_fps}").into(),
                color: palette.text,
            });
        }
        if profile.fps_1_low {
            let low_text: SharedString = format!("{low_1_fps} FPS").into();
            grid_rows.push(PreviewGridRow {
                label: SharedString::new_static("1%"),
                label_color: palette.engine,
                col2: Some(PreviewCell {
                    text: low_text,
                    color: palette.text,
                }),
                col3: None,
            });
            h_cells.push(PreviewCell {
                text: format!("1% {low_1_fps}").into(),
                color: palette.text,
            });
        }
        horizontal_segments.push(PreviewHorizontalSegment {
            label: SharedString::new_static(""),
            label_color: palette.engine,
            cells: h_cells,
        });
    }

    if profile.wine {
        extras.push(PreviewCell {
            text: SharedString::new_static("Proton 9.0"),
            color: palette.accent,
        });
    }
    if profile.winesync {
        extras.push(PreviewCell {
            text: SharedString::new_static("NTSYNC"),
            color: palette.accent,
        });
    }
    if profile.fsr {
        extras.push(PreviewCell {
            text: SharedString::new_static("FSR ON"),
            color: palette.gpu,
        });
    }
    if profile.hdr {
        extras.push(PreviewCell {
            text: SharedString::new_static("HDR ON"),
            color: palette.accent,
        });
    }
    if profile.gamemode {
        extras.push(PreviewCell {
            text: SharedString::new_static("GAMEMODE ON"),
            color: palette.gpu,
        });
    }
    if profile.throttling_status {
        extras.push(PreviewCell {
            text: SharedString::new_static("THR: NO"),
            color: palette.text,
        });
    }
    if profile.gamepad_battery {
        extras.push(PreviewCell {
            text: SharedString::new_static("PAD 85%"),
            color: palette.battery,
        });
    }

    let minimal_fps_only = profile.is_minimal_fps_only();

    CachedPreview {
        palette,
        grid_rows,
        horizontal_segments,
        extras,
        minimal_fps_only,
        fps_only_value: fps_str,
        fps_only_color: fps_color,
        show_frametime_graph: profile.frame_timing && !minimal_fps_only,
        frametime_minmax,
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
    fn preview_cache_reflects_fps_limit_and_label_and_metrics() {
        let mut profile = MangoHudProfile {
            fps: true,
            frametime: true,
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
        let fps_row = cache
            .grid_rows
            .iter()
            .find(|r| r.label.as_ref() == "VULKAN")
            .expect("VULKAN row");
        assert_eq!(fps_row.col2.as_ref().unwrap().text.as_ref(), "144 FPS");
        assert_eq!(fps_row.col3.as_ref().unwrap().text.as_ref(), "6.9 ms");

        profile.fps_limit = Some(30);
        profile.fps_use_label = true;
        profile.fps_avg = true;
        profile.fps_1_low = true;
        let cache = build_cache(
            &profile,
            &MetalHudSettings::default(),
            &LinuxHudEnv::default(),
            &theme,
        );
        let fps_row = cache
            .grid_rows
            .iter()
            .find(|r| r.label.as_ref() == "FPS")
            .expect("FPS label row");
        assert_eq!(fps_row.col2.as_ref().unwrap().text.as_ref(), "30 FPS");
        assert_eq!(fps_row.col3.as_ref().unwrap().text.as_ref(), "33.3 ms");
        assert!(cache.grid_rows.iter().any(|r| r.label.as_ref() == "AVG"));
        assert!(cache.grid_rows.iter().any(|r| r.label.as_ref() == "1%"));
    }

    #[test]
    fn preview_places_vram_temp_on_vram_row() {
        let mut profile = MangoHudProfile::from_preset(irori_core::HudPreset::BalancedGaming);
        profile.vram = true;
        profile.gpu_mem_temp = true;
        let cache = build_cache(
            &profile,
            &MetalHudSettings::default(),
            &LinuxHudEnv::default(),
            &irori_core::ThemeHudColors::default(),
        );
        let vram_row = cache
            .grid_rows
            .iter()
            .find(|r| r.label.as_ref() == "VRAM")
            .expect("VRAM row");
        assert_eq!(vram_row.col2.as_ref().unwrap().text.as_ref(), "8.4 GiB");
        assert_eq!(vram_row.col3.as_ref().unwrap().text.as_ref(), "74°C");
    }
}
