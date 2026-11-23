//! One frame rate limit per launch, through one limiter.
//!
//! The limit is Heroic's `enableDXVKFpsLimit` and `DXVKFpsCap`, whatever
//! limits it: settings imported from Heroic keep their cap. On Linux the
//! limiter is `iroriFpsLimiter`: gamescope (`-r`), MangoHud (`fps_limit`,
//! which Irori can change while the game runs, `crate::mangohud`) or DXVK
//! (`DXVK_CONFIG`'s `maxFrameRate`, plus `DXVK_FRAME_RATE` for older DXVK and
//! `VKD3D_FRAME_RATE` for Direct3D 12; `env::apply_fps_limit`, Direct3D games
//! under Wine and Proton only).
//! Automatic picks gamescope when the game runs in it, else MangoHud when
//! it shows, else DXVK for a Windows game, else MangoHud loaded hidden (the
//! only limiter a native game has). A limiter chosen that can't apply to
//! this launch falls back to the automatic choice. Elsewhere DXVK is the
//! only limiter, as before.
//!
//! [`apply`] returns the settings the rest of the launch reads, with only
//! the chosen limiter's switch on, so one limit never runs through two
//! limiters.

use irori_core::GameSettings;
use irori_core::settings::FpsLimiter;

/// The limiter a launch uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limiter {
    Gamescope,
    MangoHud,
    Dxvk,
}

/// A launch's frame limit: how many frames a second, through what.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameLimit {
    pub limiter: Limiter,
    pub fps: u32,
}

/// What a launch offers the limiters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LimitContext {
    /// Linux, where gamescope and MangoHud exist.
    pub linux: bool,
    /// The game runs in gamescope.
    pub gamescope: bool,
    /// MangoHud is installed (and can be loaded).
    pub mangohud: bool,
    /// The game runs through Wine or Proton, where DXVK can be.
    pub wine: bool,
}

/// A frame limit as entered: a whole number above 0, else none.
pub fn parse_fps(value: &str) -> Option<u32> {
    value.trim().parse::<u32>().ok().filter(|n| *n > 0)
}

/// The limit `settings` ask for in `cx`, and through what.
pub fn resolve(settings: &GameSettings, cx: LimitContext) -> Option<FrameLimit> {
    if !settings.enable_dxvk_fps_limit {
        return None;
    }
    let fps = parse_fps(&settings.dxvk_fps_cap)?;
    let limiter = if !cx.linux {
        Limiter::Dxvk
    } else {
        let can = |limiter: Limiter| match limiter {
            Limiter::Gamescope => cx.gamescope,
            Limiter::MangoHud => cx.mangohud,
            Limiter::Dxvk => cx.wine,
        };
        let chosen = match settings.fps_limiter {
            FpsLimiter::Auto => None,
            FpsLimiter::Gamescope => Some(Limiter::Gamescope),
            FpsLimiter::MangoHud => Some(Limiter::MangoHud),
            FpsLimiter::Dxvk => Some(Limiter::Dxvk),
        };
        match chosen.filter(|l| can(*l)) {
            Some(limiter) => limiter,
            None if cx.gamescope => Limiter::Gamescope,
            None if cx.mangohud && settings.show_mangohud => Limiter::MangoHud,
            None if cx.wine => Limiter::Dxvk,
            None if cx.mangohud => Limiter::MangoHud,
            None => return None,
        }
    };
    Some(FrameLimit { limiter, fps })
}

/// `settings` with only `limit`'s limiter on: DXVK's cap off unless DXVK
/// limits, gamescope's own limit set to it when gamescope limits and the
/// user set none there.
pub fn apply(settings: &GameSettings, limit: Option<FrameLimit>) -> GameSettings {
    let mut out = settings.clone();
    let limiter = limit.map(|l| l.limiter);
    if limiter != Some(Limiter::Dxvk) {
        out.enable_dxvk_fps_limit = false;
    }
    if let Some(FrameLimit {
        limiter: Limiter::Gamescope,
        fps,
    }) = limit
        && !(out.gamescope.enable_limiter && parse_fps(&out.gamescope.fps_limiter).is_some())
    {
        out.gamescope.enable_limiter = true;
        out.gamescope.fps_limiter = fps.to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limited(fps: &str, limiter: FpsLimiter) -> GameSettings {
        GameSettings {
            enable_dxvk_fps_limit: true,
            dxvk_fps_cap: fps.into(),
            fps_limiter: limiter,
            ..Default::default()
        }
    }

    const LINUX: LimitContext = LimitContext {
        linux: true,
        gamescope: false,
        mangohud: false,
        wine: false,
    };

    #[test]
    fn automatic_picks_gamescope_then_a_shown_mangohud_then_dxvk() {
        let auto = limited("60", FpsLimiter::Auto);
        let at = |cx| resolve(&auto, cx).map(|l| l.limiter);
        let all = LimitContext {
            gamescope: true,
            mangohud: true,
            wine: true,
            ..LINUX
        };
        assert_eq!(at(all), Some(Limiter::Gamescope));
        let no_scope = LimitContext {
            gamescope: false,
            ..all
        };
        // MangoHud installed but not shown: DXVK limits a Windows game.
        assert_eq!(at(no_scope), Some(Limiter::Dxvk));
        let shown = GameSettings {
            show_mangohud: true,
            ..auto.clone()
        };
        assert_eq!(
            resolve(&shown, no_scope).map(|l| l.limiter),
            Some(Limiter::MangoHud)
        );
        // A native game: only MangoHud, loaded hidden, can limit it.
        let native = LimitContext {
            mangohud: true,
            ..LINUX
        };
        assert_eq!(at(native), Some(Limiter::MangoHud));
        assert_eq!(at(LINUX), None, "nothing to limit with");
        assert_eq!(resolve(&auto, all).map(|l| l.fps), Some(60));
    }

    #[test]
    fn a_chosen_limiter_applies_when_it_can() {
        let cx = LimitContext {
            gamescope: true,
            mangohud: true,
            wine: true,
            ..LINUX
        };
        let at = |limiter, cx| resolve(&limited("90", limiter), cx).map(|l| l.limiter);
        assert_eq!(at(FpsLimiter::Dxvk, cx), Some(Limiter::Dxvk));
        assert_eq!(at(FpsLimiter::MangoHud, cx), Some(Limiter::MangoHud));
        let native = LimitContext { wine: false, ..cx };
        assert_eq!(
            at(FpsLimiter::Dxvk, native),
            Some(Limiter::Gamescope),
            "no DXVK for a native game: automatic"
        );
        // Off, unreadable or 0: no limit at all.
        let mut off = limited("90", FpsLimiter::MangoHud);
        off.enable_dxvk_fps_limit = false;
        assert_eq!(resolve(&off, cx), None);
        assert_eq!(resolve(&limited("sixty", FpsLimiter::Auto), cx), None);
        assert_eq!(resolve(&limited("0", FpsLimiter::Auto), cx), None);
        // Away from Linux DXVK is the only limiter.
        let mac = LimitContext::default();
        assert_eq!(at(FpsLimiter::MangoHud, mac), Some(Limiter::Dxvk));
    }

    #[test]
    fn only_the_chosen_limiter_is_switched_on() {
        let gs = limited("72", FpsLimiter::Auto);
        let dxvk = apply(
            &gs,
            Some(FrameLimit {
                limiter: Limiter::Dxvk,
                fps: 72,
            }),
        );
        assert!(dxvk.enable_dxvk_fps_limit);
        let mango = apply(
            &gs,
            Some(FrameLimit {
                limiter: Limiter::MangoHud,
                fps: 72,
            }),
        );
        assert!(!mango.enable_dxvk_fps_limit, "DXVK doesn't limit too");
        assert!(!mango.gamescope.enable_limiter);
        let scope = apply(
            &gs,
            Some(FrameLimit {
                limiter: Limiter::Gamescope,
                fps: 72,
            }),
        );
        assert!(scope.gamescope.enable_limiter && !scope.enable_dxvk_fps_limit);
        assert_eq!(scope.gamescope.fps_limiter, "72");
        // gamescope's own limit, when the user set one there, stays.
        let mut own = gs.clone();
        own.gamescope.enable_limiter = true;
        own.gamescope.fps_limiter = "45".into();
        let kept = apply(
            &own,
            Some(FrameLimit {
                limiter: Limiter::Gamescope,
                fps: 72,
            }),
        );
        assert_eq!(kept.gamescope.fps_limiter, "45");
        assert!(!apply(&gs, None).enable_dxvk_fps_limit);
    }
}
