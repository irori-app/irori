//! Per-game artwork panel of the Artwork section: replace the store's cover,
//! background and logo with a file, a SteamGridDB image or one of the
//! user's screenshots of the game, and fetch the store's details again.
//!
//! Choices are the library's art overrides (`UserData::overrides`), the
//! same ones the "Edit details" dialog writes; the store never sees them.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
    WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, Context, ElementId, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, PathPromptOptions, Rems, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, Subscription, Window, div,
    rems,
};
use irori_core::{GameId, GameInfo, GameOverrides};
use irori_ui::art_picker::{self, ArtSlot};
use irori_ui::components::{ArtImage, art_image, art_loading, physical_width};
use irori_ui::i18n::{t, t_args};
use irori_ui::{backend, models};

use crate::i18n::LanguageStamp;
use crate::widgets::muted;

/// The art this panel replaces, in display order.
const SLOTS: [ArtSlot; 3] = [ArtSlot::Cover, ArtSlot::Background, ArtSlot::Logo];
/// Screenshots offered at once, newest first. Older ones are rarely the
/// picture someone wants as art, and each costs a thumbnail.
const MAX_SCREENSHOTS: usize = 24;
/// Image formats the art store reads.
const IMAGE_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

/// Where a slot's art comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Store,
    /// A local file (picked, or a screenshot).
    File,
    /// An address on the web (SteamGridDB).
    Web,
    None,
}

/// The override field holding `slot`.
fn override_of(o: &GameOverrides, slot: ArtSlot) -> Option<&str> {
    match slot {
        ArtSlot::Cover => o.art_cover.as_deref(),
        ArtSlot::Background => o.art_background.as_deref(),
        ArtSlot::Logo => o.art_logo.as_deref(),
    }
}

/// The store's own art for `slot`.
fn store_art(game: &GameInfo, slot: ArtSlot) -> Option<&str> {
    match slot {
        ArtSlot::Cover => game.cover_art(),
        ArtSlot::Background => game.art_background.as_deref(),
        ArtSlot::Logo => game.art_logo.as_deref(),
    }
    .filter(|s| !s.is_empty())
}

/// `overrides` with `slot` set to `value`. A value equal to the store's art
/// (or empty) removes the override, so later store changes apply again.
pub(crate) fn with_art(
    overrides: &GameOverrides,
    store: Option<&GameInfo>,
    slot: ArtSlot,
    value: Option<String>,
) -> GameOverrides {
    let original = store.and_then(|g| store_art(g, slot));
    let value = value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && Some(v.as_str()) != original);
    let mut next = overrides.clone();
    match slot {
        ArtSlot::Cover => next.art_cover = value,
        ArtSlot::Background => next.art_background = value,
        ArtSlot::Logo => next.art_logo = value,
    }
    next
}

/// Where the art shown for `slot` comes from.
pub(crate) fn source(overrides: &GameOverrides, store: Option<&GameInfo>, slot: ArtSlot) -> Source {
    match override_of(overrides, slot) {
        Some(v) if v.starts_with("http://") || v.starts_with("https://") => Source::Web,
        Some(_) => Source::File,
        None if store.and_then(|g| store_art(g, slot)).is_some() => Source::Store,
        None => Source::None,
    }
}

/// Whether a picked file is an image the art store can show.
pub(crate) fn is_image(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

fn slot_id(slot: ArtSlot) -> &'static str {
    match slot {
        ArtSlot::Cover => "cover",
        ArtSlot::Background => "background",
        ArtSlot::Logo => "logo",
    }
}

fn slot_label(slot: ArtSlot) -> SharedString {
    t(match slot {
        ArtSlot::Cover => "settings.art.cover",
        ArtSlot::Background => "settings.art.background",
        ArtSlot::Logo => "settings.art.logo",
    })
}

/// Preview size in rem: a 2:3 cover, a wide background, a logo strip.
fn preview_size(slot: ArtSlot) -> (f32, f32) {
    match slot {
        ArtSlot::Cover => (5.0, 7.5),
        ArtSlot::Background => (13.33, 7.5),
        ArtSlot::Logo => (13.33, 7.5),
    }
}

/// Screenshots of the game, once listed.
enum Shots {
    NotLoaded,
    Loading,
    Loaded(Vec<String>),
}

pub(crate) struct ArtworkPanel {
    id: GameId,
    shots: Shots,
    /// The slot a screenshot is being chosen for.
    choosing: Option<ArtSlot>,
    refetching: bool,
    language: LanguageStamp,
    _subscriptions: Vec<Subscription>,
}

impl ArtworkPanel {
    pub(crate) fn new(id: GameId, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let library = models(cx).library.clone();
        let art = irori_ui::art::art_store(cx);
        Self {
            id,
            shots: Shots::NotLoaded,
            choosing: None,
            refetching: false,
            language: LanguageStamp::new(),
            _subscriptions: vec![
                cx.observe(&library, |_, _, cx| cx.notify()),
                cx.observe(&art, |_, _, cx| cx.notify()),
            ],
        }
    }

    fn overrides(&self, cx: &App) -> GameOverrides {
        models(cx)
            .library
            .read(cx)
            .user
            .overrides
            .get(&self.id.key())
            .cloned()
            .unwrap_or_default()
    }

    /// The game as its store reports it, without the user's overrides.
    fn store_game(&self, cx: &App) -> Option<Arc<GameInfo>> {
        let b = backend(cx);
        b.store(self.id.runner)
            .and_then(|s| s.game(&self.id.app_name))
            .or_else(|| b.game(&self.id))
    }

    fn title(&self, cx: &App) -> String {
        backend(cx)
            .game(&self.id)
            .map(|g| g.title.clone())
            .unwrap_or_else(|| self.id.app_name.to_string())
    }

    fn set_art(&mut self, slot: ArtSlot, value: Option<String>, cx: &mut Context<Self>) {
        let store = self.store_game(cx);
        let next = with_art(&self.overrides(cx), store.as_deref(), slot, value);
        let b = backend(cx);
        let id = self.id.clone();
        irori_ui::state::run_infallible(cx, async move { b.userdata.set_overrides(&id, next) })
            .detach();
        self.choosing = None;
        cx.notify();
    }

    fn choose_file(&mut self, slot: ArtSlot, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(t("settings.art.choose_file_prompt")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let picked = match rx.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                Ok(Err(e)) => {
                    this.update_in(cx, |_, window, cx| {
                        window.push_notification(
                            Notification::error(format!("{e:#}"))
                                .title(t("settings.error.file_picker")),
                            cx,
                        )
                    })
                    .ok();
                    None
                }
                _ => None,
            };
            let Some(path) = picked else { return };
            this.update_in(cx, |this, window, cx| {
                if is_image(&path) {
                    this.set_art(slot, Some(path.to_string_lossy().into_owned()), cx);
                } else {
                    window.push_notification(
                        Notification::error(t("settings.art.not_an_image"))
                            .title(t("settings.art.choose_failed")),
                        cx,
                    );
                }
            })
            .ok();
        })
        .detach();
    }

    fn search_online(&mut self, slot: ArtSlot, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let title = self.title(cx);
        art_picker::open(
            slot,
            &title,
            std::rc::Rc::new(move |url, _, cx| {
                this.update(cx, |this, cx| this.set_art(slot, Some(url), cx))
                    .ok();
            }),
            window,
            cx,
        );
    }

    fn choose_screenshot(&mut self, slot: ArtSlot, window: &mut Window, cx: &mut Context<Self>) {
        if self.choosing == Some(slot) {
            self.choosing = None;
            cx.notify();
            return;
        }
        self.choosing = Some(slot);
        if matches!(self.shots, Shots::NotLoaded) {
            self.shots = Shots::Loading;
            let b = backend(cx);
            let key = self.id.key();
            let lister = b.clone();
            let job = b.spawn_blocking(move || {
                lister
                    .screenshots()
                    .for_game(&key)
                    .take(MAX_SCREENSHOTS)
                    .map(|s| s.path.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
            });
            let task = irori_ui::run(cx, async move { Ok(job.await?) });
            cx.spawn_in(window, async move |this, cx| {
                let shots = task.await.unwrap_or_default();
                this.update_in(cx, |this, _, cx| {
                    this.shots = Shots::Loaded(shots);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    fn refetch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.refetching {
            return;
        }
        self.refetching = true;
        // The art as shown before: a store may replace an image under the
        // same address, and edited local files keep theirs.
        let mut urls: Vec<String> = Vec::new();
        let games = [self.store_game(cx), backend(cx).game(&self.id)];
        for game in games.iter().flatten() {
            urls.extend(
                [
                    &game.art_cover,
                    &game.art_square,
                    &game.art_background,
                    &game.art_logo,
                    &game.art_icon,
                ]
                .into_iter()
                .flatten()
                .cloned(),
            );
        }
        let b = backend(cx);
        let runner = self.id.runner;
        let title = self.title(cx);
        let task = irori_ui::run(cx, async move { b.refresh_store(runner, true).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.refetching = false;
                match result {
                    Ok(()) => {
                        let art = irori_ui::art::art_store(cx);
                        art.update(cx, |s, cx| s.forget(urls, cx));
                        window.push_notification(
                            Notification::success(t_args(
                                "settings.art.refetched",
                                &[("title", &title)],
                            )),
                            cx,
                        );
                    }
                    Err(e) => window.push_notification(
                        Notification::error(format!("{e:#}"))
                            .title(t("settings.art.refetch_failed")),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn preview(
        &self,
        slot: ArtSlot,
        url: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (w, h) = preview_size(slot);
        let logo = slot == ArtSlot::Logo;
        let theme = cx.theme();
        let frame = div()
            .w(rems(w))
            .h(rems(h))
            .flex_shrink_0()
            .overflow_hidden()
            .rounded(theme.radius)
            .bg(theme.muted);
        let image = url.map(|u| art_image(u, physical_width(Rems(w), window), logo, cx));
        match image {
            Some(ArtImage::Ready(img)) => frame
                .when(logo, |f| f.p_2())
                .child(img.size_full().object_fit(if logo {
                    ObjectFit::Contain
                } else {
                    ObjectFit::Cover
                }))
                .into_any_element(),
            Some(ArtImage::Loading) => frame.child(art_loading()).into_any_element(),
            _ => frame
                .child(
                    v_flex()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .text_color(cx.theme().muted_foreground)
                        .child(Icon::new(IconName::ImageOff)),
                )
                .into_any_element(),
        }
    }

    fn render_slot(
        &self,
        slot: ArtSlot,
        overrides: &GameOverrides,
        store: Option<&GameInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = slot_id(slot);
        let id = |part: &str| ElementId::Name(format!("art-{part}-{name}").into());
        let shown = override_of(overrides, slot).or_else(|| store.and_then(|g| store_art(g, slot)));
        let source = source(overrides, store, slot);
        let source_text = t(match source {
            Source::Store => "settings.art.source.store",
            Source::File => "settings.art.source.file",
            Source::Web => "settings.art.source.web",
            Source::None => "settings.art.source.none",
        });
        let custom = matches!(source, Source::File | Source::Web);
        let online = art_picker::available(cx);
        let choosing = self.choosing == Some(slot);
        let buttons = h_flex()
            .gap_2()
            .flex_wrap()
            .child(
                Button::new(id("file"))
                    .outline()
                    .small()
                    .icon(IconName::FolderOpen)
                    .label(t("settings.art.choose_file"))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.choose_file(slot, window, cx)),
                    ),
            )
            .child(
                Button::new(id("online"))
                    .outline()
                    .small()
                    .icon(IconName::Globe)
                    .label(t("settings.art.search_online"))
                    .disabled(!online)
                    .when(!online, |b| b.tooltip(t("settings.art.search_unavailable")))
                    .on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.search_online(slot, window, cx)
                        }),
                    ),
            )
            .child(
                Button::new(id("shot"))
                    .outline()
                    .small()
                    .icon(IconName::Camera)
                    .label(t("settings.art.use_screenshot"))
                    .selected(choosing)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose_screenshot(slot, window, cx)
                    })),
            )
            .when(custom, |row| {
                row.child(
                    Button::new(id("reset"))
                        .ghost()
                        .small()
                        .icon(IconName::RotateCcw)
                        .label(t("settings.art.use_store"))
                        .on_click(cx.listener(move |this, _, _, cx| this.set_art(slot, None, cx))),
                )
            });
        let text = v_flex()
            .flex_1()
            .min_w_0()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(div().text_sm().font_medium().child(slot_label(slot)))
                    .child(if custom {
                        Tag::info().xsmall().child(source_text)
                    } else {
                        Tag::secondary().xsmall().outline().child(source_text)
                    }),
            )
            .when_some(
                override_of(overrides, slot).filter(|_| source == Source::File),
                |v, path| v.child(muted(path.to_string(), cx).text_xs().truncate()),
            )
            .child(buttons);
        v_flex()
            .id(id("slot"))
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .w_full()
                    .gap_4()
                    .items_start()
                    .child(self.preview(slot, shown, window, cx))
                    .child(text),
            )
            .when(choosing, |v| v.child(self.render_shots(slot, window, cx)))
            .into_any_element()
    }

    fn render_shots(
        &self,
        slot: ArtSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let shots = match &self.shots {
            Shots::NotLoaded | Shots::Loading => {
                return muted(t("settings.art.screenshots_loading"), cx).into_any_element();
            }
            Shots::Loaded(shots) if shots.is_empty() => {
                return muted(t("settings.art.no_screenshots"), cx).into_any_element();
            }
            Shots::Loaded(shots) => shots.clone(),
        };
        let (w, h) = (8.0, 4.5);
        let width = physical_width(Rems(w), window);
        let radius = cx.theme().radius;
        let ring = cx.theme().ring;
        let muted_bg = cx.theme().muted;
        let tiles: Vec<AnyElement> = shots
            .into_iter()
            .enumerate()
            .map(|(ix, path)| {
                let image = art_image(&path, width, false, cx);
                let label = t_args("settings.art.screenshot_n", &[("n", &(ix + 1).to_string())]);
                let tile = div()
                    .id(ElementId::Name(
                        format!("art-shot-{}-{ix}", slot_id(slot)).into(),
                    ))
                    .w(rems(w))
                    .h(rems(h))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .rounded(radius)
                    .bg(muted_bg)
                    .cursor_pointer()
                    .border_2()
                    .border_color(gpui_kit::transparent_black())
                    .hover(|s| s.border_color(ring))
                    .focus(|s| s.border_color(ring))
                    .tab_stop(true)
                    .aria_label(label);
                let tile = match image {
                    ArtImage::Ready(img) => {
                        tile.child(img.size_full().object_fit(ObjectFit::Cover))
                    }
                    ArtImage::Loading => tile.child(art_loading()),
                    ArtImage::Missing => tile,
                };
                let pick = path.clone();
                tile.on_click(
                    cx.listener(move |this, _, _, cx| this.set_art(slot, Some(pick.clone()), cx)),
                )
                .into_any_element()
            })
            .collect();
        v_flex()
            .gap_2()
            .child(muted(t("settings.art.pick_screenshot"), cx).text_xs())
            .child(h_flex().gap_2().flex_wrap().children(tiles))
            .into_any_element()
    }
}

impl Render for ArtworkPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.language.changed();
        let overrides = self.overrides(cx);
        let store = self.store_game(cx);
        let border = cx.theme().border;
        let slots: Vec<AnyElement> = SLOTS
            .into_iter()
            .map(|slot| self.render_slot(slot, &overrides, store.as_deref(), window, cx))
            .collect();
        v_flex()
            .w_full()
            .gap_4()
            .children(slots.into_iter().enumerate().map(|(ix, slot)| {
                div()
                    .w_full()
                    .when(ix > 0, |d| d.pt_4().border_t_1().border_color(border))
                    .child(slot)
            }))
            .child(
                h_flex()
                    .w_full()
                    .gap_4()
                    .pt_4()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_medium()
                                    .child(t("settings.art.refetch")),
                            )
                            .child(
                                muted(
                                    t_args(
                                        "settings.art.refetch_help",
                                        &[("store", self.id.runner.store_name())],
                                    ),
                                    cx,
                                )
                                .text_xs(),
                            ),
                    )
                    .child(
                        Button::new("art-refetch")
                            .outline()
                            .icon(IconName::RefreshCw)
                            .label(t("settings.art.refetch_action"))
                            .loading(self.refetching)
                            .disabled(self.refetching)
                            .on_click(cx.listener(|this, _, window, cx| this.refetch(window, cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> GameInfo {
        GameInfo {
            title: "Hades".into(),
            art_cover: Some("https://cdn/cover.jpg".into()),
            art_logo: Some("https://cdn/logo.png".into()),
            ..Default::default()
        }
    }

    #[test]
    fn choosing_art_keeps_the_other_overrides() {
        let before = GameOverrides {
            title: Some("Hades (2018)".into()),
            art_square: Some("/sq.png".into()),
            ..Default::default()
        };
        let g = store();
        let next = with_art(
            &before,
            Some(&g),
            ArtSlot::Background,
            Some(" /bg.png ".into()),
        );
        assert_eq!(next.art_background.as_deref(), Some("/bg.png"));
        assert_eq!(next.title.as_deref(), Some("Hades (2018)"));
        assert_eq!(next.art_square.as_deref(), Some("/sq.png"));
        assert_eq!(source(&next, Some(&g), ArtSlot::Background), Source::File);
        assert_eq!(source(&next, Some(&g), ArtSlot::Cover), Source::Store);
        assert_eq!(source(&next, None, ArtSlot::Cover), Source::None);
    }

    #[test]
    fn the_stores_own_art_is_no_override() {
        let g = store();
        let o = with_art(
            &GameOverrides::default(),
            Some(&g),
            ArtSlot::Logo,
            Some("https://cdn/logo.png".into()),
        );
        assert!(o.is_empty());
        let o = with_art(
            &GameOverrides::default(),
            Some(&g),
            ArtSlot::Cover,
            Some("https://cdn2.steamgriddb.com/grid/x.png".into()),
        );
        assert_eq!(source(&o, Some(&g), ArtSlot::Cover), Source::Web);
        // Resetting removes it again.
        assert!(with_art(&o, Some(&g), ArtSlot::Cover, None).is_empty());
    }

    #[test]
    fn only_images_are_accepted() {
        use std::path::Path;
        assert!(is_image(Path::new("/a/b/Cover.PNG")));
        assert!(is_image(Path::new("shot.webp")));
        assert!(!is_image(Path::new("game.exe")));
        assert!(!is_image(Path::new("noext")));
    }
}
