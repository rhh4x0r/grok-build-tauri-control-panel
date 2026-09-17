//! Visual tokens. Two designed appearances (not one inverted), following the
//! system: the gpui-kit component theme comes from `themes/bomb.json`, and
//! [`Ui`] carries the app's own tokens for the same mode (surfaces, text
//! tones, ink washes, the fuse gradient).
//!
//! Layout numbers live here too and never depend on which color is painted.

use gpui_kit::component::{ActiveTheme, Theme, ThemeMode, ThemeRegistry};
use gpui_kit::*;

const THEME_JSON: &str = include_str!("../themes/bomb.json");

/// Register both themes and match the current system appearance.
pub fn install(cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    if let Err(e) = registry.load_themes_from_str(THEME_JSON) {
        tracing::warn!(error = %e, "bomb theme failed to parse; using built-in themes");
    }
    let dark = cx.window_appearance().is_dark();
    apply_mode(dark, cx);
}

/// Switch the component theme to the named Bomb variant for `dark`.
pub fn apply_mode(dark: bool, cx: &mut App) {
    let name = if dark { "Bomb Dark" } else { "Bomb Light" };
    let cfg = ThemeRegistry::global(cx).themes().get(name).cloned();
    match cfg {
        Some(cfg) => {
            let theme = Theme::global_mut(cx);
            theme.mode = if dark { ThemeMode::Dark } else { ThemeMode::Light };
            theme.apply_config(&cfg);
            theme.mono_font_family = "Menlo".into();
        }
        None => Theme::change(
            if dark { ThemeMode::Dark } else { ThemeMode::Light },
            None,
            cx,
        ),
    }
}

/// Call each frame from the root view: follows macOS appearance changes.
pub fn follow_system(window: &Window, cx: &mut App) {
    let want_dark = window.appearance().is_dark();
    if cx.theme().mode.is_dark() != want_dark {
        apply_mode(want_dark, cx);
    }
}

pub trait AppearanceExt {
    fn is_dark(&self) -> bool;
}

impl AppearanceExt for WindowAppearance {
    fn is_dark(&self) -> bool {
        matches!(self, WindowAppearance::Dark | WindowAppearance::VibrantDark)
    }
}

/// Layout constants (Zeron-derived).
pub struct Layout;
impl Layout {
    pub const TITLEBAR: f32 = 38.0;
    pub const HEADER: f32 = 44.0;
    pub const SIDEBAR: f32 = 256.0;
    pub const BUBBLE_RADIUS: f32 = 16.0;
    pub const PANEL_RADIUS: f32 = 10.0;
    pub const COMPOSER_RADIUS: f32 = 26.0;
    pub const CONTENT_MAX: f32 = 880.0;
    pub const COMPOSER_MAX: f32 = 768.0;
}

/// App tokens for the active appearance.
#[derive(Clone)]
pub struct Ui {
    pub dark: bool,
    /// Main content panel (opaque fallback).
    pub bg: Hsla,
    /// The one tint the whole window paints over the blurred desktop. Dark:
    /// `surface` at 80% (Zeron's GLASS_ALPHA); light: opaque white.
    pub glass: Hsla,
    pub hover: Hsla,
    pub active: Hsla,
    pub border: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,
    /// Max-contrast plate (send button) and its label color.
    pub solid: Hsla,
    pub on_solid: Hsla,
    pub accent: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
    pub success: Hsla,
    pub input_bg: Hsla,
    pub bubble: Hsla,
    pub mono: SharedString,
}

impl Ui {
    pub fn of(cx: &App) -> Self {
        let dark = cx.theme().mode.is_dark();
        let mono = cx.theme().mono_font_family.clone();
        if dark {
            Self {
                dark,
                bg: grey(0x06),
                glass: hsla(0.0, 0.0, 13.0 / 255.0, 0.80),
                hover: hsla(0.0, 0.0, 0.92, 0.11),
                active: hsla(0.0, 0.0, 0.92, 0.16),
                border: hsla(0.0, 0.0, 1.0, 0.08),
                text: neutral(0.922),
                text_muted: neutral(0.708),
                text_faint: neutral(0.556),
                solid: neutral(0.922),
                on_solid: grey(0x0e),
                accent: c(0x818cf8),
                danger: c(0xf87171),
                warning: c(0xfbbf24),
                success: c(0x34d399),
                input_bg: hsla(0.0, 0.0, 1.0, 0.03),
                bubble: hsla(0.0, 0.0, 1.0, 0.08),
                mono,
            }
        } else {
            Self {
                dark,
                bg: grey(0xff),
                glass: grey(0xff),
                hover: hsla(0.0, 0.0, 0.10, 0.06),
                active: hsla(0.0, 0.0, 0.10, 0.10),
                border: hsla(0.0, 0.0, 0.0, 0.10),
                text: neutral(0.25),
                text_muted: neutral(0.439),
                text_faint: neutral(0.535),
                solid: neutral(0.205),
                on_solid: neutral(0.985),
                accent: c(0x4f46e5),
                danger: c(0xdc2626),
                warning: c(0xb45309),
                success: c(0x059669),
                input_bg: grey(0xff),
                bubble: hsla(0.0, 0.0, 0.0, 0.06),
                mono,
            }
        }
    }

    /// Translucent fill ink: white on dark, black on light.
    pub fn ink(&self, alpha: f32) -> Hsla {
        if self.dark {
            hsla(0.0, 0.0, 1.0, alpha)
        } else {
            hsla(0.0, 0.0, 0.0, alpha * 0.8)
        }
    }

    pub fn backend(&self, key: &str) -> Hsla {
        match (key, self.dark) {
            ("grok", true) => c(0x8be28b),
            ("grok", false) => c(0x2f8f3a),
            ("claude", true) => c(0xd97757),
            ("claude", false) => c(0xb85c3a),
            ("codex", true) => c(0x74aa9c),
            ("codex", false) => c(0x3f8a78),
            _ => self.text_muted,
        }
    }
}

/// How the platform composites behind our paint: blurred desktop in dark
/// (frosted glass), opaque in light.
pub fn window_background(cx: &App) -> WindowBackgroundAppearance {
    if cx.window_appearance().is_dark() {
        WindowBackgroundAppearance::Blurred
    } else {
        WindowBackgroundAppearance::Opaque
    }
}

/// Fuse colors are the one thing that stays the same in both appearances.
pub struct Fuse;
impl Fuse {
    pub const ORANGE: u32 = 0xfb923c;
    pub const HOT: u32 = 0xfde047;
    pub const WHITE: u32 = 0xfff7ed;
    pub const GREEN: u32 = 0x86efac;
}

pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

pub fn ca(hex: u32, alpha: f32) -> Hsla {
    let mut h: Hsla = rgb(hex).into();
    h.a = alpha;
    h
}

fn grey(v: u8) -> Hsla {
    hsla(0.0, 0.0, v as f32 / 255.0, 1.0)
}

fn neutral(l: f32) -> Hsla {
    hsla(0.0, 0.0, l, 1.0)
}
