//! Visual tokens. Two designed appearances (not one inverted), following the
//! system: the gpui-kit component theme comes from `themes/bomb.json`, and
//! [`Ui`] carries the app's own tokens for the same mode (surfaces, text
//! tones, ink washes, the fuse gradient).
//!
//! Layout numbers live here too and never depend on which color is painted.

use gpui_kit::component::{ActiveTheme, Theme, ThemeMode, ThemeRegistry};
use gpui_kit::*;

const THEME_JSON: &str = include_str!("../themes/bomb.json");

/// Embedded Geist + Geist Mono (SIL OFL 1.1; see assets/fonts/licenses).
const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/Geist.ttf"),
    include_bytes!("../assets/fonts/Geist-Medium.ttf"),
    include_bytes!("../assets/fonts/Geist-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Geist-Bold.ttf"),
    include_bytes!("../assets/fonts/Geist-Italic.ttf"),
    include_bytes!("../assets/fonts/GeistMono.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Medium.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Bold.ttf"),
];

pub const FONT_SANS: &str = "Geist";
pub const FONT_MONO: &str = "Geist Mono";

/// Register both themes and match the current system appearance.
pub fn install(cx: &mut App) {
    if let Err(e) = cx
        .text_system()
        .add_fonts(FONTS.iter().map(|b| std::borrow::Cow::Borrowed(*b)).collect())
    {
        tracing::warn!(error = %e, "embedded fonts failed to register; falling back to system fonts");
    }
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
            {
                let theme = Theme::global_mut(cx);
                theme.mode = if dark { ThemeMode::Dark } else { ThemeMode::Light };
                theme.apply_config(&cfg);
                // The theme file has no syntax colors, and the kit's fallback is its light palette in both
                // modes (dark blue keywords on a dark panel). Use the palette made for the mode.
                theme.highlight_theme = if dark { gpui_kit::component::highlighter::HighlightTheme::default_dark() } else { gpui_kit::component::highlighter::HighlightTheme::default_light() };
                theme.font_family = FONT_SANS.into();
                theme.mono_font_family = FONT_MONO.into();
            }
            // Base components (inputs, resize handles) read their own token
            // set; project the component theme onto it or they keep defaults.
            Theme::sync_base(cx);
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
    pub const HEADER: f32 = 48.0;
    pub const SIDEBAR: f32 = 272.0;
    pub const BUBBLE_RADIUS: f32 = 16.0;
    pub const PANEL_RADIUS: f32 = 10.0;
    pub const COMPOSER_RADIUS: f32 = 26.0;
    pub const CONTENT_MAX: f32 = 736.0;
    /// Transcript body: 15px on a 24px line.
    pub const BODY_SIZE: f32 = 15.0;
    pub const BODY_LINE: f32 = 24.0;
    pub const COMPOSER_MAX: f32 = 768.0;
    pub const SPACE_XS: f32 = 4.0;
    pub const SPACE_SM: f32 = 8.0;
    /// Sidebar: rows 2px apart, sections 12px apart (Zeron).
    pub const SIDEBAR_LIST_GAP: f32 = 2.0;
    pub const SIDEBAR_SECTION_GAP: f32 = 12.0;
}

/// The one type scale. Views pick a step; nothing is smaller than `CAPTION`.
/// `themes/bomb.json` sets the kit's base size to 16, so its buttons and inputs land on the same steps
/// (small controls 14, captions 12).
pub struct Type;
impl Type {
    /// Timestamps, counters, paths under a title.
    pub const CAPTION: f32 = 12.0;
    /// Secondary lines, badges, toolbar labels.
    pub const SMALL: f32 = 13.0;
    /// Default interface text and buttons.
    pub const BODY: f32 = 14.0;
    /// Card and panel titles.
    pub const TITLE: f32 = 17.0;
    /// Page titles.
    pub const DISPLAY: f32 = 24.0;
}

/// App tokens for the active appearance.
#[derive(Clone)]
pub struct Ui {
    pub dark: bool,
    /// Main content panel (opaque fallback).
    pub bg: Hsla,
    /// The one tint the whole window paints over the background artwork
    /// (dark) — low enough that the bomb and embers read through; light:
    /// opaque white.
    pub glass: Hsla,
    pub hover: Hsla,
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
                glass: hsla(0.0, 0.0, 13.0 / 255.0, 0.42),
                hover: hsla(0.0, 0.0, 0.92, 0.11),
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

    /// Zeron's selection/hover wash: near-white on dark, near-black on light.
    pub fn wash(&self, alpha: f32) -> Hsla {
        if self.dark {
            hsla(0.0, 0.0, 0.92, alpha)
        } else {
            hsla(0.0, 0.0, 0.10, alpha)
        }
    }

    /// Selected row / card background (`glass_selected_bg`).
    pub fn selected_bg(&self) -> Hsla {
        if self.dark {
            self.wash(0.11)
        } else {
            self.wash(0.06)
        }
    }

    /// Hairline rule: white on dark, black (a touch stronger) on light.
    pub fn hairline(&self, alpha: f32) -> Hsla {
        if self.dark {
            hsla(0.0, 0.0, 1.0, alpha)
        } else {
            hsla(0.0, 0.0, 0.0, (alpha * 1.35).min(0.5))
        }
    }

    /// Sidebar sublines (project caption, branch): muted text at half alpha.
    pub fn subline(&self) -> Hsla {
        let mut c = self.text_muted;
        c.a = 0.5;
        c
    }

    /// `color` with its alpha scaled.
    pub fn alpha(c: Hsla, a: f32) -> Hsla {
        let mut c = c;
        c.a = a;
        c
    }

    /// Composer pill border (Zeron's frost variant, faint cool tint).
    pub fn pill_border(&self) -> Hsla {
        if self.dark {
            hsla(210.0 / 360.0, 0.18, 0.78, 0.09)
        } else {
            hsla(210.0 / 360.0, 0.18, 0.32, 0.10)
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

    /// Brand tint for a backend mark. Like Zeron: only Claude carries a
    /// color; Grok and OpenAI marks are neutral text.
    pub fn backend(&self, key: &str) -> Hsla {
        match (key, self.dark) {
            ("claude", true) => c(0xd97757),
            ("claude", false) => c(0xb85c3a),
            ("grok" | "codex", _) => self.text,
            _ => self.text_muted,
        }
    }
}

/// The window paints its own artwork in dark mode and flat white in light,
/// so the platform never needs to composite anything behind us.
pub fn window_background(_cx: &App) -> WindowBackgroundAppearance {
    WindowBackgroundAppearance::Opaque
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
