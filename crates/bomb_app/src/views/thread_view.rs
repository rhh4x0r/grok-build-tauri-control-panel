//! Center column. Phase 1: welcome banner or a plain dump of the selected
//! thread's entries; Phase 2 replaces the body with the real transcript.

use bomb_core::transcript::{Body, Role};
use gpui_kit::component::ActiveTheme;
use gpui_kit::*;

use crate::models::app::AppModel;
use crate::theme::{c, Palette};

const BANNER: &str = r#"┌──────────────────────────────────────────┐
│  B O M B   C O D E                       │
│  agent control panel · grok · claude     │
└──────────────────────────────────────────┘"#;

pub struct ThreadView {
    model: Entity<AppModel>,
}

impl ThreadView {
    pub fn new(model: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |this, _, cx| {
            this.observe_selected(cx);
            cx.notify()
        })
        .detach();
        let mut this = Self { model };
        this.observe_selected(cx);
        this
    }

    fn observe_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(t) = self.model.read(cx).selected_thread() {
            cx.observe(&t, |_, _, cx| cx.notify()).detach();
        }
    }

    fn welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_4()
            .child(
                div()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_sm()
                    .text_color(c(Palette::ACCENT))
                    .whitespace_nowrap()
                    .children(BANNER.lines().map(|l| div().child(l.to_string()))),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(c(Palette::MUTED))
                    .child("Pick a project and press + to start a thread."),
            )
    }
}

impl Render for ThreadView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(thread) = self.model.read(cx).selected_thread() else {
            return div().size_full().child(self.welcome(cx)).into_any_element();
        };
        let t = thread.read(cx);
        let mono = cx.theme().mono_font_family.clone();
        let rows: Vec<AnyElement> = t
            .thread
            .entries
            .iter()
            .map(|e| {
                let (rail, label) = match e.role {
                    Role::You => (Palette::USER, "you"),
                    Role::Agent => (Palette::AGENT, "agent"),
                    Role::Thought => (Palette::FUSE, "think"),
                    Role::Tool => (Palette::TOOL, "tool"),
                    Role::Plan => (Palette::ACCENT, "plan"),
                    Role::Approval => (Palette::WARN, "ask"),
                    Role::System => (Palette::DIM, "sys"),
                    Role::Error => (Palette::ERR, "err"),
                };
                let body = match &e.body {
                    Body::Text(s) => s.clone(),
                    Body::Tool(r) => format!("{} [{}] {}", r.name, r.status, r.args),
                    Body::Plan(p) => p.markdown.clone(),
                    Body::Approval(a) => format!("{} — {}", a.tool, a.summary),
                };
                div()
                    .flex()
                    .gap_3()
                    .px_4()
                    .py_1()
                    .border_l_2()
                    .border_color(c(rail))
                    .child(
                        div()
                            .w(px(40.))
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(c(rail))
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_family(mono.clone())
                            .whitespace_normal()
                            .child(body),
                    )
                    .into_any_element()
            })
            .collect();
        let status = format!(
            "{}  ·  {}",
            t.thread.presence.label(std::time::Instant::now()),
            t.meta.status
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("transcript")
                    .flex_1()
                    .overflow_y_scroll()
                    .py_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(rows),
            )
            .child(
                div()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(c(Palette::BORDER))
                    .text_xs()
                    .text_color(c(Palette::MUTED))
                    .child(status),
            )
            .into_any_element()
    }
}
