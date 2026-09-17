//! Motion budget for Bomb Code. One vocabulary, few durations:
//!
//! - `fade_in`: entrances (thread switch, new rows) — 220ms, 4px rise.
//! - `FOLD`: tool-group / chip expand — 140ms ease-out (gpui-kit Collapsible
//!   drives this with a spring under `motion_id`).
//! - `SWEEP`: the fuse meter's indeterminate sweep — 1.6s linear, repeating.
//!
//! gpui replays an element animation from zero whenever the element remounts,
//! so entrance ids are keyed by the thing entering (thread id, entry id), never
//! by position.

use std::time::Duration;

use gpui_kit::*;

pub const FADE_IN: Duration = Duration::from_millis(220);
pub const SWEEP: Duration = Duration::from_millis(1600);
pub const PULSE: Duration = Duration::from_millis(1400);

/// CSS `cubic-bezier(0.16, 1, 0.3, 1)`-ish: fast start, soft landing.
pub fn ease_out_expo(t: f32) -> f32 {
    if t >= 1.0 {
        1.0
    } else {
        1.0 - 2f32.powf(-10.0 * t)
    }
}

/// Fade + 4px rise on mount.
pub fn fade_in<E>(id: impl Into<ElementId>, element: E) -> AnimationElement<E>
where
    E: Styled + IntoElement + 'static,
{
    element.with_animation(
        id,
        Animation::new(FADE_IN).with_easing(ease_out_expo),
        |el, t| el.relative().opacity(t).top(px(4.0 * (1.0 - t))),
    )
}

/// Gentle opacity breathe between `lo` and 1.0, repeating.
pub fn breathe<E>(id: impl Into<ElementId>, lo: f32, element: E) -> AnimationElement<E>
where
    E: Styled + IntoElement + 'static,
{
    element.with_animation(
        id,
        Animation::new(PULSE)
            .repeat()
            .with_easing(pulsating_between(lo, 1.0))
            .with_max_fps(30.),
        |el, t| el.opacity(t),
    )
}

/// Small vertical bounce, repeating (the "thinking" sprite).
pub fn bounce<E>(id: impl Into<ElementId>, amplitude: f32, element: E) -> AnimationElement<E>
where
    E: Styled + IntoElement + 'static,
{
    element.with_animation(
        id,
        Animation::new(Duration::from_millis(900))
            .repeat()
            .with_easing(pulsating_between(0.0, 1.0))
            .with_max_fps(30.),
        move |el, t| el.relative().top(px(-amplitude * t)),
    )
}

/// Image reveal: 700ms fade from transparent (the dot grid holds the frame
/// until then).
pub fn reveal<E>(id: impl Into<ElementId>, element: E) -> AnimationElement<E>
where
    E: Styled + IntoElement + 'static,
{
    element.with_animation(
        id,
        Animation::new(Duration::from_millis(700)).with_easing(ease_out_expo),
        |el, t| el.opacity(t),
    )
}
