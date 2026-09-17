//! Backend brand marks (SVG, `currentColor`), used wherever a thread, service
//! or model is named. Colors follow the vendors' identities in dark mode and
//! drop a step in light mode for contrast.

use gpui_kit::*;

use crate::theme::Ui;

pub fn mark_path(backend: &str) -> Option<&'static str> {
    match backend {
        "grok" => Some("assets/icons/grok-mark.svg"),
        "claude" => Some("assets/icons/claude-mark.svg"),
        "codex" => Some("assets/icons/openai-mark.svg"),
        _ => None,
    }
}

/// The mark at `size` px, tinted with the backend color (or muted when
/// `tinted` is false, as in Zeron's neutral sidebar rows).
pub fn brand_mark(backend: &str, size: f32, tinted: bool, ui: &Ui) -> AnyElement {
    let color = if tinted { ui.backend(backend) } else { ui.text_muted };
    match mark_path(backend) {
        Some(path) => svg()
            .path(path)
            .size(px(size))
            .flex_shrink_0()
            .text_color(color)
            .into_any_element(),
        None => div()
            .size(px(size * 0.5))
            .rounded_full()
            .flex_shrink_0()
            .bg(color)
            .into_any_element(),
    }
}
