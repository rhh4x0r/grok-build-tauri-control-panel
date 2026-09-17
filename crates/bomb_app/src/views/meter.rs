//! The fuse meter: a 3px bar under the status line. Indeterminate = a 36%-wide
//! orange→yellow→white segment with an orange glow sweeping left to right on a
//! 1.6s loop (translateX(-100%) → translateX(220%) of its own width, as the
//! original CSS did). Progress/tools = a green→yellow fill. Stall = flat amber.

use bomb_core::presence::MeterMode;
use gpui_kit::*;

use crate::theme::{c, ca, Fuse, Ui};
use crate::views::motion::SWEEP;

const HEIGHT: f32 = 3.0;
const SEGMENT: f32 = 0.36;

fn fuse_gradient() -> Background {
    linear_gradient(
        90.,
        linear_color_stop(c(Fuse::ORANGE), 0.0),
        linear_color_stop(c(Fuse::WHITE), 1.0),
    )
}

fn agent_gradient() -> Background {
    linear_gradient(
        90.,
        linear_color_stop(c(Fuse::GREEN), 0.0),
        linear_color_stop(c(Fuse::HOT), 1.0),
    )
}

fn glow(color: u32, blur: f32, alpha: f32) -> BoxShadow {
    BoxShadow {
        color: ca(color, alpha),
        offset: point(px(0.), px(0.)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    }
}

pub fn meter_bar(id: impl Into<ElementId>, mode: MeterMode, ui: &Ui) -> impl IntoElement {
    let id = id.into();
    let track = div()
        .relative()
        .w_full()
        .h(px(HEIGHT))
        .rounded(px(2.))
        .bg(ui.ink(0.06))
        .overflow_hidden();

    match mode {
        MeterMode::Idle => track,
        MeterMode::Indeterminate => track.child(
            div()
                .absolute()
                .top_0()
                .h_full()
                .w(relative(SEGMENT))
                .bg(fuse_gradient())
                .shadow(vec![glow(Fuse::ORANGE, 8., 0.7)])
                .with_animation(
                    id,
                    Animation::new(SWEEP).repeat().with_max_fps(60.),
                    |el, t| {
                        let from = -SEGMENT;
                        let to = SEGMENT * 2.2;
                        el.left(relative(from + t * (to - from)))
                    },
                ),
        ),
        MeterMode::Progress(p) | MeterMode::Tools(p) => track.child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .h_full()
                .w(relative(p.clamp(0.02, 1.0)))
                .bg(agent_gradient())
                .shadow(vec![glow(Fuse::GREEN, 6., 0.4)]),
        ),
        MeterMode::Stall => track.child(div().w_full().h_full().bg(ui.warning.opacity(0.55))),
        MeterMode::Complete => track.child(div().w_full().h_full().bg(ui.success)),
        MeterMode::Error => track.child(div().w_full().h_full().bg(ui.danger)),
    }
}
