//! Center column: header · transcript (or welcome) · status line + meter ·
//! composer.

use std::time::Instant;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use uuid::Uuid;

use crate::models::app::AppModel;
use crate::models::thread::ThreadModel;
use crate::theme::{Layout, Ui};
use crate::views::composer::ComposerView;
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
    composer: Entity<ComposerView>,
}

impl ThreadView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |this, _, cx| {
            this.sync_transcript(cx);
            cx.notify()
        })
        .detach();
        let composer = cx.new(|cx| ComposerView::new(model.clone(), window, cx));
        let mut this = Self {
            model,
            transcript: None,
            composer,
        };
        this.sync_transcript(cx);
        this
    }

    pub fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |c, cx| c.focus(window, cx));
    }

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

    fn welcome(&self, ui: &Ui, cx: &mut Context<Self>) -> impl IntoElement {
        let project = self
            .model
            .read(cx)
            .active_project
            .as_deref()
            .map(crate::models::app::project_name);
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
            .child(div().text_sm().text_color(ui.text_muted).child(match project {
                Some(p) => format!("New thread in {p}. Type below to start."),
                None => "Open a project (⌘O) to start a thread.".to_string(),
            }))
    }

    fn header(&self, thread: &Entity<ThreadModel>, ui: &Ui, cx: &mut Context<Self>) -> impl IntoElement {
        let t = thread.read(cx);
        let id = Uuid::parse_str(&t.meta.id).ok();
        let backend = t.meta.backend.clone();
        let model = t.meta.model.clone();
        let has_worktree = t.meta.worktree.is_some();
        let branch = t
            .meta
            .worktree
            .as_deref()
            .and_then(|w| std::path::Path::new(w).file_name())
            .map(|s| s.to_string_lossy().to_string());
        let live = t.meta.live;
        let brain = t.meta.brain_mode.clone();
        let dev = self.model.read(cx).dev_server.clone();
        let dev_running = dev.as_ref().map(|d| d.running).unwrap_or(false);
        let dev_url = dev.as_ref().and_then(|d| d.url.clone());
        let app = self.model.clone();
        let hover = ui.hover;

        let chip = |id: &'static str, label: String, ui: &Ui| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .h(px(24.))
                .px_2()
                .rounded(px(6.))
                .text_xs()
                .text_color(ui.text_muted)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(label)
        };

        div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(Layout::HEADER))
            .px_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(ui.text_faint)
                    .child(div().size(px(7.)).rounded_full().bg(ui.backend(&backend)))
                    .child(if model.is_empty() { backend } else { model })
                    .child("·")
                    .child(if live { brain.unwrap_or_else(|| "live".into()) } else { "saved".into() }),
            )
            .child(div().flex_1())
            .when(has_worktree, |el| {
                let app_land = app.clone();
                let app_sync = app.clone();
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .text_color(ui.text_faint)
                        .child(div().size(px(11.)).child(Icon::from(Lucide::GitBranch)))
                        .child(branch.clone().unwrap_or_default()),
                )
                .child(chip("sync", "Sync".into(), ui).on_click(move |_, _, cx| {
                    if let Some(id) = id {
                        app_sync.update(cx, |m, cx| m.sync_thread(id, cx));
                    }
                }))
                .child(chip("land", "Land".into(), ui).on_click(move |_, _, cx| {
                    if let Some(id) = id {
                        app_land.update(cx, |m, cx| m.land_thread(id, cx));
                    }
                }))
            })
            .child({
                let app = app.clone();
                chip(
                    "dev-toggle",
                    if dev_running { "Stop dev server".into() } else { "Dev server".into() },
                    ui,
                )
                .child(div().size(px(12.)).child(Icon::from(if dev_running { Lucide::Square } else { Lucide::Play })))
                .on_click(move |_, _, cx| app.update(cx, |m, cx| m.dev_server_toggle(cx)))
            })
            .when_some(dev_url.filter(|_| dev_running), |el, url| {
                let app = app.clone();
                el.child(
                    chip("dev-open", url, ui)
                        .text_color(ui.accent)
                        .on_click(move |_, _, cx| app.update(cx, |m, cx| m.dev_server_open(cx))),
                )
            })
    }
}

impl Render for ThreadView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::of(cx);
        let thread = self.model.read(cx).selected_thread();
        let composer = self.composer.clone();

        let Some(thread) = thread else {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(div().flex_1().min_h_0().child(self.welcome(&ui, cx)))
                .child(composer)
                .into_any_element();
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
            .child(self.header(&thread, &ui, cx))
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
                            .when(!show_status, |el| el.child(div().h(px(20.)))),
                    ),
            )
            .child(composer)
            .into_any_element()
    }
}
