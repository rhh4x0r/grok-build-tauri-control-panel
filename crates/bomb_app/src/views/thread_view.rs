//! Center column: header · transcript · status line + meter · composer.
//! The composer is a styled placeholder until Phase 3 wires the input.

use std::time::Instant;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::models::app::AppModel;
use crate::models::thread::ThreadModel;
use crate::theme::{Layout, Ui};
use crate::views::meter::meter_bar;
use crate::views::motion::fade_in;
use crate::views::status_line::{status_line, StatusLineProps};
use crate::views::transcript::TranscriptView;

const BANNER: &str = r#"┌──────────────────────────────────────────┐
│  B O M B   C O D E                       │
│  agent control panel · grok · claude     │
└──────────────────────────────────────────┘"#;

pub struct ThreadView {
    model: Entity<AppModel>,
    transcript: Option<(String, Entity<TranscriptView>)>,
}

impl ThreadView {
    pub fn new(model: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |this, _, cx| {
            this.sync_transcript(cx);
            cx.notify()
        })
        .detach();
        let mut this = Self {
            model,
            transcript: None,
        };
        this.sync_transcript(cx);
        this
    }

    /// One TranscriptView per selected thread; recreated on switch so the
    /// entrance animation and scroll state start fresh.
    fn sync_transcript(&mut self, cx: &mut Context<Self>) {
        let selected = self.model.read(cx).selected_thread();
        match selected {
            None => self.transcript = None,
            Some(t) => {
                let id = t.read(cx).id();
                let same = self
                    .transcript
                    .as_ref()
                    .map(|(cur, _)| cur == &id)
                    .unwrap_or(false);
                if !same {
                    let view = cx.new(|cx| TranscriptView::new(t.clone(), cx));
                    cx.observe(&t, |_, _, cx| cx.notify()).detach();
                    self.transcript = Some((id, view));
                }
            }
        }
    }

    fn welcome(&self, ui: &Ui) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_4()
            .child(
                div()
                    .font_family(ui.mono.clone())
                    .text_sm()
                    .text_color(ui.text_faint)
                    .whitespace_nowrap()
                    .children(BANNER.lines().map(|l| div().child(l.to_string()))),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(ui.text_muted)
                    .child("Pick a project and press + to start a thread."),
            )
    }

    fn footer(&self, thread: &Entity<ThreadModel>, ui: &Ui, cx: &mut Context<Self>) -> impl IntoElement {
        let t = thread.read(cx);
        let branch = t
            .meta
            .worktree
            .as_deref()
            .and_then(|w| std::path::Path::new(w).file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "main".into());
        let ctx = t
            .thread
            .context_tokens
            .map(|n| format!("ctx {}", bomb_core::presence::format_count(n as usize)));
        let backend = t.meta.backend.clone();
        let model = t.meta.model.clone();
        let mode = t.meta.approval_mode.clone().unwrap_or_else(|| "plan".into());
        let (surface_raised, border, input_bg, solid, on_solid) =
            (ui.surface_raised, ui.border, ui.input_bg, ui.solid, ui.on_solid);
        let _ = surface_raised;

        div()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .px_6()
            .pb_4()
            .child(
                div()
                    .w_full()
                    .max_w(px(Layout::COMPOSER_MAX))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        // Composer frame (input arrives in Phase 3).
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap_2()
                            .h(px(52.))
                            .pl_5()
                            .pr_3()
                            .rounded(px(Layout::COMPOSER_RADIUS))
                            .border_1()
                            .border_color(border)
                            .bg(input_bg)
                            .when(!ui.dark, |el| el.shadow_sm())
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .text_color(ui.text_faint)
                                    .child("Do anything…"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .text_xs()
                                    .text_color(ui.text_muted)
                                    .child(div().size(px(7.)).rounded_full().bg(ui.backend(&backend)))
                                    .child(div().font_weight(FontWeight::MEDIUM).child(if model.is_empty() { backend } else { model }))
                                    .child(div().text_color(ui.text_faint).child(mode)),
                            )
                            .child(
                                div()
                                    .size(px(24.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(ui.text_muted)
                                    .child(div().size(px(15.)).child(Icon::from(Lucide::Paperclip))),
                            )
                            .child(
                                div()
                                    .size(px(28.))
                                    .rounded_full()
                                    .bg(solid)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .opacity(0.35)
                                    .child(div().size(px(14.)).text_color(on_solid).child(Icon::from(Lucide::ArrowUp))),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_2()
                            .text_xs()
                            .text_color(ui.text_faint)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(div().size(px(11.)).child(Icon::from(Lucide::Folder)))
                                    .child("Local checkout"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(div().size(px(11.)).child(Icon::from(Lucide::GitBranch)))
                                    .child(branch),
                            )
                            .child(div().flex_1())
                            .when_some(ctx, |el, c| el.child(c)),
                    ),
            )
    }
}

impl Render for ThreadView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::of(cx);
        let Some(thread) = self.model.read(cx).selected_thread() else {
            return div().size_full().child(self.welcome(&ui)).into_any_element();
        };
        let Some((tid, transcript)) = self.transcript.clone() else {
            return div().size_full().into_any_element();
        };
        let now = Instant::now();
        let (presence, explain_open, explain_text, explain_pending, has_explanations) = {
            let t = thread.read(cx);
            (
                t.thread.presence.clone(),
                t.explain_open,
                t.thread.explanations.last().map(|e| e.text.clone()),
                t.thread.explain_pending,
                !t.thread.explanations.is_empty(),
            )
        };
        let meter = presence.meter(now);
        let show_status = presence.visible();
        let thread_for_toggle = thread.clone();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(ui.bg)
            .child(fade_in(
                SharedString::from(format!("transcript-{tid}")),
                div().flex_1().min_h_0().child(transcript),
            ))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .px_6()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(Layout::COMPOSER_MAX))
                            .flex()
                            .flex_col()
                            .when(show_status, |el| {
                                el.child(status_line(
                                    StatusLineProps {
                                        presence: &presence,
                                        now,
                                        explain_open,
                                        explain_text,
                                        explain_pending,
                                        has_explanations,
                                    },
                                    move |_, _, cx| {
                                        thread_for_toggle.update(cx, |t, cx| {
                                            t.explain_open = !t.explain_open;
                                            cx.notify();
                                        });
                                    },
                                    &ui,
                                ))
                                .child(div().pb_2().child(meter_bar("meter", meter, &ui)))
                            })
                            .when(!show_status, |el| el.child(div().h(px(28.)))),
                    ),
            )
            .child(self.footer(&thread, &ui, cx))
            .into_any_element()
    }
}
