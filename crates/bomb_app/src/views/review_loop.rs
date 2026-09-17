//! Thread-local Foundry progress and decisions.
use crate::{
    models::app::AppModel,
    runtime::{services, spawn_service},
    theme::Ui,
};
use bomb_foundry::{Run, RunStatus};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    input::{Input, InputState},
    Disableable, Icon, Sizable,
};
use gpui_kit::*;
use std::collections::HashSet;

pub struct ReviewLoopView {
    model: Entity<AppModel>,
    expanded: HashSet<String>,
    feedback: Entity<InputState>,
    feedback_open: bool,
    run_id: String,
    busy: bool,
    error: Option<String>,
}
impl ReviewLoopView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self {
            model,
            expanded: HashSet::new(),
            feedback: cx.new(|cx| {
                InputState::new(window, cx).placeholder("What should change before approval?")
            }),
            feedback_open: false,
            run_id: String::new(),
            busy: false,
            error: None,
        }
    }
    fn act(&mut self, run: Run, command: &'static str, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let feedback = self.feedback.read(cx).value().to_string();
        let token = run.gate_token();
        let origin = run.id.clone();
        let state = services(cx);
        let weak = cx.entity().downgrade();
        self.busy = true;
        self.error = None;
        spawn_service(
            cx,
            async move {
                if command == "revise" {
                    bomb_core::foundry::FoundryService::request_changes(
                        state, run.id, token, feedback,
                    )
                } else {
                    bomb_core::foundry::FoundryService::command(state, run.id, command, Some(token))
                }
            },
            move |result, cx| {
                let _ = weak.update(cx, |v, cx| {
                    if v.run_id != origin {
                        return;
                    }
                    v.busy = false;
                    match result {
                        Ok(()) => v.feedback_open = false,
                        Err(e) => v.error = Some(e),
                    }
                    cx.notify();
                });
            },
        );
        cx.notify();
    }
}
fn status(run: &Run) -> &'static str {
    match run.status {
        RunStatus::Ready | RunStatus::Running => "Review loop working",
        RunStatus::WaitingGate => "Review complete · Awaiting your approval",
        RunStatus::Completed if !run.gates.is_empty() => "Completed · Approved by you",
        RunStatus::Completed => "Completed",
        RunStatus::Paused => "Review loop paused",
        RunStatus::Blocked => "Review loop needs attention",
        RunStatus::Stopped => "Review loop stopped",
        RunStatus::Interrupted => "Review loop interrupted · Resume when ready",
    }
}
impl Render for ReviewLoopView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::of(cx);
        let run = self
            .model
            .read(cx)
            .selected
            .and_then(|id| services(cx).foundry.for_thread(&id.to_string()));
        let Some(run) = run else {
            return div().into_any_element();
        };
        if self.run_id != run.id {
            self.run_id = run.id.clone();
            self.busy = false;
            self.expanded.clear();
            self.feedback_open = false;
            self.error = None;
            self.feedback
                .update(cx, |s, cx| s.set_value("", window, cx));
        }
        let waiting = run.status == RunStatus::WaitingGate;
        let complete = run.status == RunStatus::Completed;
        let mut strip = div().flex().items_center().flex_wrap().gap_2();
        for (index, node) in run.document.graph.ordered().iter().enumerate() {
            let accepted = run.accepted.contains_key(&node.id);
            let icon = if accepted {
                Lucide::Check
            } else if index == run.cursor {
                Lucide::CircleDot
            } else {
                Lucide::Circle
            };
            let key = node.id.clone();
            strip = strip.child(
                Button::new(SharedString::from(format!("stage-{}", node.id)))
                    .ghost()
                    .small()
                    .icon(icon)
                    .label(node.title.clone())
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if !v.expanded.remove(&key) {
                            v.expanded.insert(key.clone());
                        }
                        cx.notify();
                    })),
            );
            if index + 1 < run.document.graph.nodes.len() {
                strip = strip.child(Icon::from(Lucide::ChevronRight).size(px(12.)));
            }
        }
        let mut content = div()
            .id("loop-stage-scroll")
            .max_h(px(240.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2();
        for node in run.document.graph.ordered() {
            let attempt = run
                .attempts
                .iter()
                .rev()
                .find(|a| a.node_id == node.id && !a.invalidated);
            let active = run.current().is_some_and(|n| n.id == node.id)
                && matches!(run.status, RunStatus::Running | RunStatus::Ready);
            if !self.expanded.contains(&node.id) && !active {
                continue;
            }
            if let Some(a) = attempt {
                let start = chrono::DateTime::parse_from_rfc3339(&a.started_at).ok();
                let end = a
                    .finished_at
                    .as_deref()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
                let elapsed = start
                    .map(|s| {
                        end.map(|e| (e - s).num_seconds())
                            .unwrap_or_else(|| {
                                (chrono::Utc::now() - s.with_timezone(&chrono::Utc)).num_seconds()
                            })
                            .max(0)
                    })
                    .unwrap_or(0);
                let mut card = div()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(ui.border)
                    .bg(ui.glass)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .items_center()
                            .child(super::brand::brand_mark(&a.backend, 14., true, &ui))
                            .child(format!(
                                "{} · {} · {}m {}s",
                                node.title,
                                a.model,
                                elapsed / 60,
                                elapsed % 60
                            )),
                    );
                if let Some(result) = &a.result {
                    card = card.child(div().text_sm().child(result.summary.clone()));
                    for criterion in &result.criteria {
                        card = card.child(
                            div().text_xs().text_color(ui.text_muted).child(format!(
                                "{} · {}",
                                criterion.status,
                                node.exit_criteria
                                    .get(criterion.criterion)
                                    .cloned()
                                    .unwrap_or_default()
                            )),
                        );
                        for evidence in &criterion.evidence {
                            card = card.child(
                                div()
                                    .text_xs()
                                    .text_color(ui.text_muted)
                                    .child(format!("• {evidence}")),
                            );
                        }
                    }
                    for path in &result.artifacts {
                        let path = path.clone();
                        let cwd = run.cwd.clone();
                        card = card.child(
                            Button::new(SharedString::from(format!("artifact-{}-{path}", a.id)))
                                .ghost()
                                .small()
                                .icon(Lucide::File)
                                .label(std::path::Path::new(&path).file_name().map(|n|n.to_string_lossy().into_owned()).unwrap_or_else(||path.clone()))
                                .tooltip(path.clone())
                                .on_click(move |_, _, cx| {
                                    super::transcript::open_link(
                                        &path,
                                        std::path::Path::new(&cwd),
                                        cx,
                                    )
                                }),
                        );
                    }
                } else {
                    card = card.child(div().text_sm().text_color(ui.text_muted).child(
                        a.error.clone().unwrap_or_else(|| {
                            "Working · activity appears in the conversation".into()
                        }),
                    ));
                }
                content = content.child(card);
            }
        }
        if waiting || complete {
            if let Some(result) = run
                .attempts
                .iter()
                .rev()
                .filter(|a| !a.invalidated)
                .find_map(|a| a.result.as_ref())
            {
                // Keep the review's limitations visible before approval, not just its verdict.
                let short = result
                    .summary
                    .split_once(". ")
                    .map(|(first, _)| format!("{first}."))
                    .unwrap_or_else(|| result.summary.clone());
                content = content.child(div().text_sm().child(short));
                let lower = result.summary.to_ascii_lowercase();
                if let Some(index) = lower.find("limitations") {
                    content = content.child(
                        div()
                            .p_2()
                            .rounded_lg()
                            .bg(ui.hover)
                            .text_sm()
                            .child(format!("Review notes · {}", &result.summary[index..])),
                    );
                }
                content=content.child(div().text_xs().text_color(ui.text_muted).child("Select Review above for checks, evidence and files. Approval accepts this result; it does not commit, merge or deploy."));
            }
        }
        if !run.note.is_empty() && !waiting && !complete {
            content = content.child(div().text_sm().child(run.note.clone()));
        }
        let mut actions = div().flex().items_center().flex_wrap().gap_2();
        for (label, cmd, enabled) in [
            ("Approve result", "approve", waiting),
            (
                "Extend retry limits",
                "extend",
                run.status == RunStatus::Paused && run.note.to_ascii_lowercase().contains("limit"),
            ),
            (
                "Pause",
                "pause",
                matches!(run.status, RunStatus::Ready | RunStatus::Running),
            ),
            (
                "Resume / retry",
                "resume",
                matches!(
                    run.status,
                    RunStatus::Paused | RunStatus::Blocked | RunStatus::Interrupted
                ),
            ),
            (
                "Stop",
                "stop",
                !matches!(run.status, RunStatus::Completed | RunStatus::Stopped),
            ),
        ] {
            if enabled {
                let r = run.clone();
                let b = Button::new(SharedString::from(format!("loop-{cmd}")))
                    .small()
                    .label(label)
                    .disabled(self.busy)
                    .on_click(cx.listener(move |v, _, _, cx| v.act(r.clone(), cmd, cx)));
                actions = actions.child(if cmd == "approve" {
                    b.primary()
                } else {
                    b.ghost()
                });
            }
        }
        if waiting {
            actions = actions.child(
                Button::new("request-loop-changes")
                    .ghost()
                    .small()
                    .icon(Lucide::Repeat)
                    .label("Request changes")
                    .disabled(self.busy)
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.feedback_open = !v.feedback_open;
                        cx.notify();
                    })),
            );
        }
        actions = actions
            .child(
                Button::new("loop-changes")
                    .ghost()
                    .small()
                    .label("View changes")
                    .on_click(cx.listener(|v, _, _, cx| {
                        if let Some(id) = v.model.read(cx).selected {
                            v.model.update(cx, |m, cx| m.land_thread(id, cx));
                        }
                    })),
            )
            .child(
                Button::new("loop-preview")
                    .ghost()
                    .small()
                    .label("Preview")
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::actions::ToggleDevPreview), cx)
                    }),
            )
            .child(
                Button::new("loop-details")
                    .ghost()
                    .small()
                    .label("Advanced details")
                    .on_click(cx.listener(|v, _, window, cx| {
                        v.model.update(cx, |m, _| m.foundry_show_runs = true);
                        window.dispatch_action(Box::new(crate::actions::OpenFoundry), cx);
                    })),
            );
        let mut panel = div()
            .mx_6()
            .my_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(ui.border)
            .bg(ui.glass)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::from(if complete {
                            Lucide::Check
                        } else if waiting {
                            Lucide::ShieldCheck
                        } else {
                            Lucide::Repeat
                        })
                        .size(px(16.)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(status(&run)),
                    ),
            )
            .child(strip)
            .child(content)
            .child(actions);
        if self.feedback_open && waiting {
            let r = run.clone();
            panel = panel.child(Input::new(&self.feedback)).child(
                Button::new("submit-loop-feedback")
                    .primary()
                    .small()
                    .label("Send changes & rerun review")
                    .disabled(self.busy)
                    .on_click(cx.listener(move |v, _, _, cx| v.act(r.clone(), "revise", cx))),
            );
        }
        if let Some(error) = &self.error {
            panel = panel.child(div().text_sm().child(error.clone()));
        }
        panel.into_any_element()
    }
}
