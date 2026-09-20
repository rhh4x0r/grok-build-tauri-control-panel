//! The same feature decision, actions and saved feedback in project and thread views.
use crate::{
    models::app::AppModel,
    runtime::{services, spawn_service},
    theme::Ui,
};
use bomb_core::services::project_work::{self as work, FeatureState, RunState, WorkStage};
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    input::{InputEvent, Textarea, TextareaState},
    Disableable, Sizable,
};
use gpui_kit::*;

pub struct FeatureDecisionView {
    model: Entity<AppModel>,
    context: Option<(String, String)>,
    input: Entity<TextareaState>,
    editing: bool,
    evidence: bool,
    busy: bool,
    message: Option<String>,
    clear: Option<String>,
    generation: u64,
}
impl FeatureDecisionView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(
                    "Describe the change, or provide the verification evidence requested above…",
                )
                .auto_grow(2, 5)
        });
        cx.subscribe(&input, |v, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                if let Some((project, id)) = &v.context {
                    if let Err(e) =
                        work::save_draft_text(&services(cx), project, id, &v.input.read(cx).value())
                    {
                        v.message = Some(e);
                    }
                }
                cx.notify();
            }
        })
        .detach();
        Self {
            model,
            context: None,
            input,
            editing: false,
            evidence: false,
            busy: false,
            message: None,
            clear: None,
            generation: 0,
        }
    }
    pub fn set_context(
        &mut self,
        project: String,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.context.as_ref() == Some(&(project.clone(), id.clone())) {
            return;
        }
        let text = work::draft_text(&services(cx), &project, &id);
        self.context = Some((project, id));
        self.generation += 1;
        self.busy = false;
        self.message = None;
        self.clear = None;
        self.editing = !text.is_empty();
        self.evidence = false;
        self.input.update(cx, |s, cx| s.set_value(text, window, cx));
    }
    fn act(&mut self, action: &'static str, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some((project, id)) = self.context.clone() else {
            return;
        };
        let state = services(cx);
        let Some(p) = state.project_work.snapshot(&project) else {
            return;
        };
        let Some(f) = p.features.iter().find(|f| f.id == id).cloned() else {
            return;
        };
        let text = self.input.read(cx).value().to_string();
        if matches!(action, "feedback" | "evidence") && text.trim().is_empty() {
            return;
        }
        let clear = matches!(action, "feedback" | "evidence").then_some(text.clone());
        let context = (project.clone(), id.clone());
        self.busy = true;
        self.message = None;
        self.generation += 1;
        let generation = self.generation;
        let weak = cx.entity().downgrade();
        spawn_service(
            cx,
            async move {
                match action {
                    "approve" => {
                        work::approve(
                            state,
                            project,
                            id,
                            f.review.ok_or("Review is not ready")?.candidate,
                        )
                        .await
                    }
                    "retry" => work::retry_stage(state, project, id, f.decision_key(), None).await,
                    "evidence" => {
                        work::retry_stage(state, project, id, f.decision_key(), Some(text)).await
                    }
                    "start" => work::start_features(state, project, vec![id]).await,
                    _ => work::keep_working(state, project, id, text).await,
                }
            },
            move |result, cx| {
                if result.is_ok() {
                    if let Some(sent) = &clear {
                        let state = services(cx);
                        if work::draft_text(&state, &context.0, &context.1) == *sent {
                            let _ = work::save_draft_text(&state, &context.0, &context.1, "");
                        }
                    }
                }
                let _ = weak.update(cx, |v, cx| {
                    if v.generation == generation && v.context.as_ref() == Some(&context) {
                        v.busy = false;
                        match result {
                            Ok(()) => {
                                v.clear = clear;
                                v.editing = false;
                                v.message = Some(
                                    "Saved. The feature status above reflects what happens next."
                                        .into(),
                                );
                            }
                            Err(e) => v.message = Some(e),
                        }
                    }
                    cx.notify();
                });
            },
        );
        cx.notify();
    }
}
impl Render for FeatureDecisionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(text) = self.clear.take() {
            if self.input.read(cx).value().as_ref() == text {
                self.input.update(cx, |s, cx| s.set_value("", window, cx));
            }
        }
        let ui = Ui::of(cx);
        let Some((project, id)) = self.context.clone() else {
            return div().into_any_element();
        };
        let Some(p) = services(cx).project_work.snapshot(&project) else {
            return div().into_any_element();
        };
        let Some(f) = p.features.iter().find(|f| f.id == id) else {
            return div().into_any_element();
        };
        let active = p
            .active_jobs
            .iter()
            .any(|k| k == &id || k.starts_with(&format!("{id}/")));
        let answering = f
            .blocker
            .as_ref()
            .is_some_and(|b| b.kind == work::BlockerKind::Input);
        let actionable =
            matches!(f.state, FeatureState::Review | FeatureState::NeedsInput) && !active;
        let mut card = super::brand::handoff_card(&ui)
            .min_w_0()
            .overflow_hidden()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(f.title()))
                    .child(
                        div()
                            .text_color(ui.text_muted)
                            .child(f.status_label().to_string()),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .whitespace_normal()
                    .child(p.waiting_reason(f)),
            );
        if let Some(b) = &f.blocker {
            card = card.child(div().text_xs().text_color(ui.text_muted).child(format!(
                "Stopped at {:?} · completed work is saved",
                b.stage
            )));
            if matches!(b.stage, WorkStage::Review | WorkStage::Checks) {
                card=card.child(div().text_sm().child("Resolve access or attach evidence below, then retry verification. Evidence is reviewed; it does not automatically pass the feature."));
            }
        }
        let finished = f
            .tasks
            .values()
            .filter(|t| t.state == work::TaskState::Checkpointed)
            .count();
        let passed = f.checks.iter().filter(|c| c.exit_code == Some(0)).count();
        card = card.child(div().text_xs().text_color(ui.text_muted).child(format!(
            "{finished}/{} builds finished · {passed}/{} checks passed · {} dependent features",
            f.tasks.len(),
            f.checks.len(),
            p.unblocks(&id)
        )));
        if let Some(review) = &f.review {
            card = card
                .child(div().text_sm().child(review.summary.clone()))
                .child(div().text_xs().text_color(ui.text_muted).child(format!(
                        "AI review passed · revision {} · {} · {}",
                        &review.candidate[..8.min(review.candidate.len())],
                        self.model
                            .read(cx)
                            .model_name(&review.reviewer.backend, &review.reviewer.model),
                        super::brand::effort_label(&review.reviewer.effort)
                    )));
        }
        let mut actions = div().flex().flex_wrap().gap_2();
        if f.state == FeatureState::Idea {
            actions = actions.child(
                Button::new("decision-start")
                    .primary()
                    .small()
                    .label("Start this feature")
                    .disabled(self.busy)
                    .on_click(cx.listener(|v, _, _, cx| v.act("start", cx))),
            );
        }
        if f.state == FeatureState::Review {
            actions = actions.child(
                Button::new("decision-approve")
                    .primary()
                    .small()
                    .label(if f.approved_head.is_some() {
                        "Approved · merge queued"
                    } else if p.state == RunState::Running {
                        "Approve & merge"
                    } else {
                        "Approve · merge when resumed"
                    })
                    .disabled(self.busy || f.approved_head.is_some() || active)
                    .on_click(cx.listener(|v, _, _, cx| v.act("approve", cx))),
            );
        }
        if f.state == FeatureState::NeedsInput {
            actions = actions.child(
                Button::new("decision-retry")
                    .outline()
                    .small()
                    .label(
                        f.blocker
                            .as_ref()
                            .map(|b| b.retry_label())
                            .unwrap_or("Retry unfinished stage"),
                    )
                    .disabled(self.busy || active)
                    .on_click(cx.listener(|v, _, _, cx| v.act("retry", cx))),
            );
            if f.blocker
                .as_ref()
                .is_some_and(|b| matches!(b.stage, WorkStage::Checks | WorkStage::Review))
                || (f.blocker.is_none() && finished == f.tasks.len())
            {
                actions = actions.child(
                    Button::new("decision-evidence")
                        .outline()
                        .small()
                        .label("Provide verification evidence")
                        .disabled(self.busy || active)
                        .on_click(cx.listener(|v, _, w, cx| {
                            v.editing = true;
                            v.evidence = true;
                            v.input.update(cx, |s, cx| s.focus(w, cx));
                            cx.notify();
                        })),
                );
            }
        }
        if actionable {
            actions = actions.child(
                Button::new("decision-feedback")
                    .ghost()
                    .small()
                    .label(if answering {
                        "Answer question"
                    } else {
                        "Request changes"
                    })
                    .on_click(cx.listener(|v, _, w, cx| {
                        v.editing = true;
                        v.evidence = false;
                        v.input.update(cx, |s, cx| s.focus(w, cx));
                        cx.notify();
                    })),
            );
        }
        let model = self.model.clone();
        let target = id.clone();
        let root = project.clone();
        actions = actions.child(
            Button::new("decision-inspect")
                .ghost()
                .small()
                .label("Result · changes · test")
                .on_click(move |_, _, cx| {
                    model.update(cx, |m, cx| {
                        m.active_project = Some(root.clone());
                        m.project_feature_request = Some(target.clone());
                        cx.notify();
                    })
                }),
        );
        let model = self.model.clone();
        let root = project.clone();
        actions = actions.child(
            Button::new("decision-new-work")
                .ghost()
                .small()
                .label("Add new project work")
                .on_click(move |_, w, cx| {
                    model.update(cx, |m, cx| m.set_active_project(root.clone(), cx));
                    w.dispatch_action(Box::new(crate::actions::NewFeature), cx);
                }),
        );
        card = card.child(actions);
        if self.editing {
            card = card
                .child(div().text_xs().child(if self.evidence {
                    "Verification evidence · this feature"
                } else if answering {
                    "Your answer · this feature"
                } else {
                    "Requested changes · this feature"
                }))
                .child(Textarea::new(&self.input))
                .child(
                    Button::new("decision-submit")
                        .primary()
                        .small()
                        .label(if self.evidence {
                            "Save evidence & retry verification"
                        } else if answering {
                            "Send answer"
                        } else {
                            "Send requested changes"
                        })
                        .disabled(
                            self.busy
                                || !actionable
                                || self.input.read(cx).value().trim().is_empty(),
                        )
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.act(if v.evidence { "evidence" } else { "feedback" }, cx)
                        })),
                );
        }
        if let Some(message) = &self.message {
            card = card.child(div().text_sm().whitespace_normal().child(message.clone()));
        }
        card.into_any_element()
    }
}
