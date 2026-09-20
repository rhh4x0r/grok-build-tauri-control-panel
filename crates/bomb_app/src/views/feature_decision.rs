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

/// The primary action follows the saved workflow state, never a guessed progress state.
#[derive(Debug, PartialEq, Eq)]
enum PrimaryDecision {
    Start,
    Approve,
    Resume,
    Retry,
    Answer,
    None,
}
fn primary_decision(
    feature: &FeatureState,
    run: &RunState,
    approved: bool,
    answering: bool,
) -> PrimaryDecision {
    match feature {
        FeatureState::Idea => PrimaryDecision::Start,
        FeatureState::Review if !approved => PrimaryDecision::Approve,
        FeatureState::Review if *run != RunState::Running => PrimaryDecision::Resume,
        FeatureState::NeedsInput if answering => PrimaryDecision::Answer,
        FeatureState::NeedsInput => PrimaryDecision::Retry,
        _ => PrimaryDecision::None,
    }
}
#[derive(Clone)]
struct DisplayedDecision {
    project: String,
    feature: String,
    key: String,
    run: RunState,
}
impl DisplayedDecision {
    fn is_current(&self, project: &str, feature: &str, key: &str, run: &RunState) -> bool {
        self.project == project && self.feature == feature && self.key == key && self.run == *run
    }
}

fn action_acknowledgement(action: &str, run: &RunState, answering: bool) -> &'static str {
    match action {
        "approve" if matches!(run, RunState::Running | RunState::Complete) => "Approval saved. Follow the feature status for local integration.",
        "approve" => "Approved for later. Resume the project when you are ready to add it.",
        "resume" if *run == RunState::Running => "Project resumed. Ready work and approved merges can continue.",
        "resume" => "Resume was requested. The project status has since changed.",
        "retry" if *run == RunState::Running => "Retry requested. The project is running; other ready work can continue too.",
        "retry" => "Retry requested. The project is no longer running; inspect its status before continuing.",
        "evidence" if *run == RunState::Running => "Evidence submitted for verification. The project is running; a pass is still required.",
        "evidence" => "Evidence submitted. A pass is still required; the project is no longer running.",
        "start" if *run == RunState::Running => "Feature queued. The project is running; other ready work can continue too.",
        "start" => "Feature queued. Inspect the project status before continuing.",
        _ if *run != RunState::Running && answering => "Answer saved. Resume the project to continue.",
        _ if *run != RunState::Running => "Changes requested. Resume the project to continue.",
        _ if answering => "Answer sent. This feature is queued to continue.",
        _ => "Changes requested. This feature will be checked again before approval.",
    }
}

pub struct FeatureDecisionView {
    model: Entity<AppModel>,
    context: Option<(String, String)>,
    input: Entity<TextareaState>,
    editing: bool,
    details: bool,
    embedded: bool,
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
            details: false,
            embedded: false,
            evidence: false,
            busy: false,
            message: None,
            clear: None,
            generation: 0,
        }
    }
    /// Embedded result inspectors already provide navigation to changes and testing.
    pub fn set_embedded(&mut self, embedded: bool) {
        self.embedded = embedded;
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
        self.details = false;
        self.input.update(cx, |s, cx| s.set_value(text, window, cx));
    }
    fn act(&mut self, action: &'static str, expected: &DisplayedDecision, cx: &mut Context<Self>) {
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
        if !expected.is_current(&project, &id, &f.decision_key(), &p.state) {
            self.message = Some("This decision changed. Review the current result and project status, then try again. Your draft is preserved.".into());
            cx.notify();
            return;
        }
        let text = self.input.read(cx).value().to_string();
        if matches!(action, "feedback" | "evidence") && text.trim().is_empty() {
            return;
        }
        let answering = f
            .blocker
            .as_ref()
            .is_some_and(|b| b.kind == work::BlockerKind::Input);
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
                    "resume" => work::command(state, project, "resume").await,
                    "retry" => work::retry_stage(state, project, id, f.decision_key(), None).await,
                    "evidence" => {
                        work::retry_stage(state, project, id, f.decision_key(), Some(text)).await
                    }
                    "start" => work::start_features(state, project, vec![id]).await,
                    _ => work::keep_working(state, project, id, text).await,
                }
            },
            move |result, cx| {
                let current_run = services(cx)
                    .project_work
                    .snapshot(&context.0)
                    .map(|p| p.state)
                    .unwrap_or(RunState::Interrupted);
                let acknowledgement = action_acknowledgement(action, &current_run, answering);
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
                                v.message = Some(acknowledgement.into());
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
        let running = p.state == RunState::Running;
        let primary = primary_decision(&f.state, &p.state, f.approved_head.is_some(), answering);
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
                        div().text_color(ui.text_muted).child(
                            match f.state {
                                FeatureState::Review if f.approved_head.is_some() => "Approved",
                                FeatureState::Review => "Ready to review",
                                FeatureState::Landing => "Adding to project",
                                FeatureState::Done => "Added to project",
                                _ => f.status_label(),
                            }
                            .to_string(),
                        ),
                    ),
            );
        let reason = if f.state == FeatureState::Done {
            if p.state == RunState::Complete {
                "Added locally. Final project checks passed.".to_string()
            } else {
                "Added locally. Final project checks are not yet complete.".to_string()
            }
        } else {
            p.waiting_reason(f)
        };
        if !reason.is_empty() {
            card = card.child(div().text_sm().whitespace_normal().child(reason));
        }
        let finished = f
            .tasks
            .values()
            .filter(|t| t.state == work::TaskState::Checkpointed)
            .count();
        let passed = f.checks.iter().filter(|c| c.exit_code == Some(0)).count();
        let failed = f
            .checks
            .iter()
            .filter(|c| c.exit_code.is_some_and(|code| code != 0))
            .count();
        let uncompleted = f.checks.iter().filter(|c| c.exit_code.is_none()).count();
        let expected = f
            .feature()
            .ok()
            .and_then(|spec| spec.verification)
            .map(|verification| verification.checks)
            .unwrap_or_default();
        let missing = expected
            .iter()
            .filter(|command| !f.checks.iter().any(|c| &c.command == *command))
            .count();
        // A zero denominator must never read as a successful verification.
        let check_summary = if f.checks.is_empty() {
            if expected.is_empty() {
                "No automated checks configured".to_string()
            } else {
                format!("{} checks not run", expected.len())
            }
        } else if failed > 0 || uncompleted > 0 || missing > 0 {
            format!(
                "Checks: {passed} passed · {failed} failed · {} not completed",
                uncompleted + missing
            )
        } else {
            format!("{passed} checks passed")
        };
        card = card.child(
            div()
                .text_sm()
                .text_color(if failed > 0 || uncompleted > 0 || missing > 0 {
                    ui.warning
                } else {
                    ui.text_muted
                })
                .child(check_summary),
        );
        if let Some(review) = &f.review {
            card = card.child(
                div()
                    .text_sm()
                    .whitespace_normal()
                    .child(review.summary.clone()),
            );
        }
        if f.state == FeatureState::Review {
            if let Some(review) = &f.review {
                card = card.child(
                    div()
                        .text_xs()
                        .text_color(ui.text_muted)
                        .whitespace_normal()
                        .child(format!(
                            "Adds reviewed version {} to local {}. Nothing is pushed or deployed.",
                            &review.candidate[..8.min(review.candidate.len())],
                            p.policy.target
                        )),
                );
            }
        }
        if f.state == FeatureState::NeedsInput {
            if let Some(blocker) = &f.blocker {
                if blocker.stage == WorkStage::Merge {
                    card = card.child(div().text_xs().text_color(ui.text_muted).whitespace_normal()
                        .child("Inspect the project’s Git view before retrying. Local edits and branches are preserved."));
                }
                if matches!(blocker.stage, WorkStage::Review | WorkStage::Checks) && !answering {
                    card = card.child(div().text_xs().text_color(ui.text_muted).whitespace_normal()
                        .child("Fix the issue, or attach evidence for the reviewer. Evidence still needs verification."));
                }
            }
            if !running && !answering {
                card = card.child(div().text_sm().whitespace_normal()
                    .child("Retrying also resumes the whole project, including other ready work and approved merges."));
            }
        }
        if f.state == FeatureState::Idea && !running {
            card = card.child(
                div()
                    .text_xs()
                    .whitespace_normal()
                    .child("Starting this feature also resumes other ready project work."),
            );
        }
        let displayed = DisplayedDecision {
            project: project.clone(),
            feature: id.clone(),
            key: f.decision_key(),
            run: p.state.clone(),
        };
        let mut actions = div().flex().flex_wrap().gap_2();
        if primary == PrimaryDecision::Start {
            let expected = displayed.clone();
            actions = actions.child(
                Button::new("decision-start")
                    .primary()
                    .small()
                    .label("Start this feature")
                    .disabled(self.busy)
                    .on_click(cx.listener(move |v, _, _, cx| v.act("start", &expected, cx))),
            );
        }
        if primary == PrimaryDecision::Approve {
            let expected = displayed.clone();
            actions = actions.child(
                Button::new("decision-approve")
                    .primary()
                    .small()
                    .label(if running {
                        "Approve & add"
                    } else {
                        "Approve for later"
                    })
                    .disabled(self.busy || active)
                    .on_click(cx.listener(move |v, _, _, cx| v.act("approve", &expected, cx))),
            );
        }
        if primary == PrimaryDecision::Resume {
            let expected = displayed.clone();
            card =
                card.child(div().text_xs().whitespace_normal().child(
                    "Resume starts other ready work and approved merges across the project.",
                ));
            actions = actions.child(
                Button::new("decision-resume")
                    .primary()
                    .small()
                    .label("Resume project")
                    .disabled(self.busy)
                    .on_click(cx.listener(move |v, _, _, cx| v.act("resume", &expected, cx))),
            );
        }
        if primary == PrimaryDecision::Retry {
            let expected = displayed.clone();
            let stage_label = f
                .blocker
                .as_ref()
                .map(|b| b.retry_label())
                .unwrap_or("Retry unfinished stage");
            actions = actions.child(
                Button::new("decision-retry")
                    .primary()
                    .small()
                    .label(if running {
                        stage_label.to_string()
                    } else {
                        format!("{stage_label} & resume project")
                    })
                    .disabled(self.busy || active)
                    .on_click(cx.listener(move |v, _, _, cx| v.act("retry", &expected, cx))),
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
                        .label("Attach evidence")
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
            let feedback = Button::new("decision-feedback")
                .small()
                .label(if answering {
                    "Answer question"
                } else {
                    "Request changes"
                })
                .disabled(self.busy)
                .on_click(cx.listener(|v, _, w, cx| {
                    v.editing = true;
                    v.evidence = false;
                    v.input.update(cx, |s, cx| s.focus(w, cx));
                    cx.notify();
                }));
            actions = actions.child(if primary == PrimaryDecision::Answer {
                feedback.primary()
            } else {
                feedback.outline()
            });
        }
        if !self.embedded {
            let model = self.model.clone();
            let target = id.clone();
            let root = project.clone();
            actions = actions.child(
                Button::new("decision-inspect")
                    .ghost()
                    .small()
                    .label("Open result")
                    .on_click(move |_, _, cx| {
                        model.update(cx, |m, cx| {
                            m.active_project = Some(root.clone());
                            m.project_feature_request = Some(target.clone());
                            cx.notify();
                        })
                    }),
            );
        }
        actions = actions.child(
            Button::new("decision-details")
                .ghost()
                .small()
                .label(if self.details {
                    "Hide details"
                } else {
                    "Details"
                })
                .on_click(cx.listener(|v, _, _, cx| {
                    v.details = !v.details;
                    cx.notify();
                })),
        );
        card = card.child(actions);
        if self.details {
            card = card.child(
                div()
                    .text_xs()
                    .text_color(ui.text_muted)
                    .whitespace_normal()
                    .child(format!(
                        "{finished}/{} tasks finished · {} dependent features",
                        f.tasks.len(),
                        p.unblocks(&id)
                    )),
            );
            if let Some(review) = &f.review {
                card = card.child(
                    div()
                        .text_xs()
                        .text_color(ui.text_muted)
                        .whitespace_normal()
                        .child(format!(
                            "Review passed · revision {} · {} · {}",
                            review.candidate,
                            self.model
                                .read(cx)
                                .model_name(&review.reviewer.backend, &review.reviewer.model),
                            super::brand::effort_label(&review.reviewer.effort)
                        )),
                );
            }
            for check in &f.checks {
                card = card.child(div().text_xs().whitespace_normal().child(format!(
                    "{} {} · {}",
                    check.command.program,
                    check.command.args.join(" "),
                    match check.exit_code {
                        Some(0) => "Passed".to_string(),
                        Some(code) => format!("Failed (exit {code})"),
                        None => "Not completed".to_string(),
                    }
                )));
            }
        }
        if self.editing {
            let expected = displayed.clone();
            if !running && !self.evidence {
                card = card.child(div().text_xs().text_color(ui.text_muted).child(
                    "Your message will be saved. Work will wait until you resume the project.",
                ));
            }
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
                            if running {
                                "Submit evidence & retry"
                            } else {
                                "Submit evidence & resume project"
                            }
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
                        .on_click(cx.listener(move |v, _, _, cx| {
                            v.act(
                                if v.evidence { "evidence" } else { "feedback" },
                                &expected,
                                cx,
                            )
                        })),
                );
        }
        if let Some(message) = &self.message {
            card = card.child(div().text_sm().whitespace_normal().child(message.clone()));
        }
        card.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{action_acknowledgement, primary_decision, DisplayedDecision, PrimaryDecision};
    use bomb_core::services::project_work::{FeatureState, RunState};

    #[test]
    fn approval_has_distinct_before_and_after_actions_when_paused() {
        for run in [RunState::Paused, RunState::Stopped, RunState::Interrupted] {
            assert_eq!(
                primary_decision(&FeatureState::Review, &run, false, false),
                PrimaryDecision::Approve
            );
            assert_eq!(
                primary_decision(&FeatureState::Review, &run, true, false),
                PrimaryDecision::Resume
            );
            assert!(action_acknowledgement("approve", &run, false).contains("for later"));
            assert!(action_acknowledgement("feedback", &run, false).contains("Resume"));
        }
        assert_eq!(
            primary_decision(&FeatureState::Review, &RunState::Running, true, false),
            PrimaryDecision::None
        );
        assert!(
            action_acknowledgement("approve", &RunState::Running, false).contains("Approval saved")
        );
    }

    #[test]
    fn input_blocker_requires_an_answer_instead_of_a_blind_retry() {
        for run in [RunState::Running, RunState::Paused, RunState::Stopped] {
            assert_eq!(
                primary_decision(&FeatureState::NeedsInput, &run, false, true),
                PrimaryDecision::Answer
            );
            assert_eq!(
                primary_decision(&FeatureState::NeedsInput, &run, false, false),
                PrimaryDecision::Retry
            );
        }
        for state in [
            FeatureState::Working,
            FeatureState::Checking,
            FeatureState::Landing,
            FeatureState::Done,
        ] {
            assert_eq!(
                primary_decision(&state, &RunState::Running, false, false),
                PrimaryDecision::None
            );
        }
    }

    #[test]
    fn recovery_acknowledgements_disclose_project_resume_without_claiming_success() {
        for action in ["retry", "evidence"] {
            for run in [RunState::Paused, RunState::Stopped, RunState::Interrupted] {
                let text = action_acknowledgement(action, &run, false);
                assert!(text.contains("no longer running"));
                assert!(!text.contains("passed"));
                assert!(!text.contains("merged"));
            }
        }
        assert!(
            action_acknowledgement("evidence", &RunState::Running, false)
                .contains("a pass is still required")
        );
        assert!(
            action_acknowledgement("retry", &RunState::Running, false).contains("other ready work")
        );
    }
    #[test]
    fn stale_displayed_candidate_or_run_state_cannot_dispatch_a_decision() {
        let shown = DisplayedDecision {
            project: "/project".into(),
            feature: "F-1".into(),
            key: "reviewed-a".into(),
            run: RunState::Paused,
        };
        assert!(shown.is_current("/project", "F-1", "reviewed-a", &RunState::Paused));
        // The same approval drawn as "for later" must not become an immediate merge.
        assert!(!shown.is_current("/project", "F-1", "reviewed-a", &RunState::Running));
        assert!(!shown.is_current("/project", "F-1", "reviewed-b", &RunState::Paused));
        // Late events from a previously selected feature cannot affect the current one.
        assert!(!shown.is_current("/project", "F-2", "reviewed-a", &RunState::Paused));
        assert!(!shown.is_current("/another", "F-1", "reviewed-a", &RunState::Paused));
    }
}
