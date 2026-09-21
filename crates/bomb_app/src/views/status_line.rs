//! The one status line: bomb sprite (mood) · phase label · elapsed · last
//! tool, with an "explain" toggle on the right and the explainer strip below.

use std::time::Instant;

use bomb_core::presence::{format_elapsed, Mood, Presence};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::{ca, Fuse, Ui};
use crate::views::motion::{bounce, breathe};

pub struct StatusLineProps<'a> {
    pub presence: &'a Presence,
    pub now: Instant,
    pub explain_open: bool,
    pub explain_text: Option<String>,
    pub explain_pending: bool,
    pub has_explanations: bool,
}

fn mood_color(mood: Mood, ui: &Ui) -> Hsla {
    match mood {
        Mood::Idle => ui.text_faint,
        Mood::Thinking => crate::theme::c(Fuse::ORANGE),
        Mood::Tooling => crate::theme::c(Fuse::HOT),
        Mood::Stream => crate::theme::c(Fuse::GREEN),
        Mood::Wait => ui.warning,
        Mood::Boom => ui.success,
        Mood::Error => ui.danger,
    }
}

fn sprite(mood: Mood, active: bool, ui: &Ui) -> AnyElement {
    let mut halo_color = mood_color(mood, ui);
    halo_color.a = if active { 0.55 } else { 0.0 };
    let halo = div()
        .absolute()
        .inset_0()
        .rounded_full()
        .shadow(vec![BoxShadow {
            color: halo_color,
            offset: point(px(0.), px(0.)),
            blur_radius: px(10.),
            spread_radius: px(1.),
            inset: false,
        }]);
    let base = div()
        .relative()
        .size(px(18.))
        .flex_shrink_0()
        .child(halo)
        .child(img("assets/logo-status.png").size(px(18.)).absolute().inset_0());
    match mood {
        Mood::Thinking => bounce("sprite-think", 2.0, base).into_any_element(),
        Mood::Tooling => breathe("sprite-tool", 0.6, base).into_any_element(),
        Mood::Stream => breathe("sprite-stream", 0.8, base).into_any_element(),
        _ => base.into_any_element(),
    }
}

pub fn status_line(
    props: StatusLineProps<'_>,
    on_toggle_explain: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ui: &Ui,
) -> impl IntoElement {
    let p = props.presence;
    let active = p.turn_active();
    let failed = p.phase == bomb_core::presence::Phase::Error;
    let mood = p.mood(props.now);
    let label = p.label(props.now);
    let elapsed = p
        .elapsed(props.now)
        .filter(|_| p.visible())
        .map(format_elapsed)
        .unwrap_or_default();
    let last_tool = p
        .last_tool_summary()
        .filter(|_| active && !label.starts_with("Running"))
        .unwrap_or_default();
    let stalled = p.stalled(props.now);

    let mut bits: Vec<AnyElement> = Vec::new();
    bits.push(
        div()
            .min_w_0()
            .overflow_hidden()
            .text_ellipsis()
            .whitespace_nowrap()
            .text_size(px(crate::theme::Type::SMALL))
            .font_weight(FontWeight::MEDIUM)
            .text_color(if p.visible() { ui.text } else { ui.text_faint })
            .child(label)
            .into_any_element(),
    );
    let muted = |s: String, ui: &Ui| {
        div()
            .min_w_0()
            .overflow_hidden()
            .text_ellipsis()
            .whitespace_nowrap()
            .text_size(px(crate::theme::Type::SMALL))
            .text_color(ui.text_muted)
            .child(s)
            .into_any_element()
    };
    if !elapsed.is_empty() {
        bits.push(dot(ui));
        bits.push(muted(elapsed, ui));
    }
    if !last_tool.is_empty() {
        bits.push(dot(ui));
        bits.push(muted(last_tool, ui));
    }
    if let Some(n) = p.context_tokens.filter(|_| active) {
        bits.push(dot(ui));
        bits.push(muted(
            format!("ctx {}", bomb_core::presence::format_count(n as usize)),
            ui,
        ));
    }
    if stalled {
        bits.push(dot(ui));
        bits.push(
            div()
                .text_size(px(crate::theme::Type::SMALL))
                .text_color(ui.warning)
                .child("no signal for a while")
                .into_any_element(),
        );
    }

    let hover = ui.hover;
    let toggle_color = if props.explain_open {
        ui.text
    } else {
        ui.text_faint
    };
    let text_muted = ui.text_muted;
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .min_w_0()
                .flex_shrink_0()
                .overflow_hidden()
                .flex()
                .items_center()
                .gap_2()
                .h(px(28.))
                .child(sprite(mood, active, ui))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .flex()
                        .items_center()
                        .gap_2()
                        .children(bits),
                )
                .when(true, |el| {
                    let _ = props.has_explanations;
                    el.child(
                        div()
                            .id("explain-toggle")
                            .flex_shrink_0()
                            .whitespace_nowrap()
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .py_0p5()
                            .rounded(px(6.))
                            .text_size(px(crate::theme::Type::SMALL))
                            .text_color(toggle_color)
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover))
                            .on_click(on_toggle_explain)
                            .child(match (failed, props.explain_open) {
                                (true, true) => "Details ▾",
                                (true, false) => "Details ▸",
                                (false, true) => "explain ▾",
                                (false, false) => "explain ▸",
                            }),
                    )
                }),
        )
        .when(props.explain_open, |el| {
            el.child(
                div()
                    .id("status-explanation")
                    .min_w_0()
                    .max_h(px(180.))
                    .overflow_y_scroll()
                    .overflow_x_hidden()
                    .whitespace_normal()
                    .pb_2()
                    .text_sm()
                    .text_color(text_muted)
                    .child(if failed && !p.note.is_empty() {
                        p.note.clone()
                    } else if props.explain_pending && props.explain_text.is_none() {
                        "thinking…".to_string()
                    } else {
                        props.explain_text.clone().unwrap_or_else(|| {
                            "No narration yet. The narrator explains this thread's activity every few seconds while the agent works (Settings → General → Explainer).".into()
                        })
                    }),
            )
        })
}

fn dot(ui: &Ui) -> AnyElement {
    let _ = ca;
    div()
        .text_size(px(crate::theme::Type::SMALL))
        .text_color(ui.text_faint)
        .child("·")
        .into_any_element()
}
