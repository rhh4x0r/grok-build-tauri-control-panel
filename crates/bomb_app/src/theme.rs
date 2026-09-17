//! Bomb Code visual tokens: the gpui-kit theme (from `themes/bomb-dark.json`)
//! plus the handful of colors the component theme has no slot for (transcript
//! role rails, the fuse gradient, backend brand colors).

use gpui_kit::component::{Theme, ThemeRegistry};
use gpui_kit::*;

const THEME_JSON: &str = include_str!("../themes/bomb-dark.json");

/// Install the Bomb Dark theme as the active theme.
pub fn install(cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    if let Err(e) = registry.load_themes_from_str(THEME_JSON) {
        tracing::warn!(error = %e, "bomb theme failed to parse; using default dark");
        Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
        return;
    }
    let cfg = registry.themes().get("Bomb Dark").cloned();
    match cfg {
        Some(cfg) => Theme::global_mut(cx).apply_config(&cfg),
        None => Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx),
    }
    let theme = Theme::global_mut(cx);
    theme.mono_font_family = mono_font_family();
}

fn mono_font_family() -> SharedString {
    // First installed wins; all are metric-compatible enough for a transcript.
    for candidate in ["JetBrains Mono", "SF Mono", "IBM Plex Mono", "Menlo"] {
        return candidate.into();
    }
    "Menlo".into()
}

/// Colors outside the component theme.
pub struct Palette;

impl Palette {
    pub const BG: u32 = 0x0c0e12;
    pub const BG_ELEV: u32 = 0x12151c;
    pub const BG_SOFT: u32 = 0x161a22;
    pub const BORDER: u32 = 0x252a35;
    pub const BORDER_HI: u32 = 0x343b4a;
    pub const TEXT: u32 = 0xe8ecf4;
    pub const MUTED: u32 = 0x8b93a7;
    pub const DIM: u32 = 0x5c6578;
    pub const ACCENT: u32 = 0x7dd3fc;
    pub const OK: u32 = 0x4ade80;
    pub const WARN: u32 = 0xfbbf24;
    pub const ERR: u32 = 0xfb7185;
    /// Transcript role rails.
    pub const USER: u32 = 0xc4b5fd;
    pub const TOOL: u32 = 0xfcd34d;
    pub const AGENT: u32 = 0x86efac;
    /// Fuse gradient.
    pub const FUSE: u32 = 0xfb923c;
    pub const FUSE_HOT: u32 = 0xfde047;
    pub const FUSE_WHITE: u32 = 0xfff7ed;
    /// Meter track.
    pub const TRACK: u32 = 0x0a0d12;

    pub fn backend(key: &str) -> u32 {
        match key {
            "grok" => 0x8be28b,
            "claude" => 0xd97757,
            "codex" => 0x74aa9c,
            _ => Self::MUTED,
        }
    }
}

pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

pub fn ca(hex: u32, alpha: f32) -> Hsla {
    let mut h: Hsla = rgb(hex).into();
    h.a = alpha;
    h
}
