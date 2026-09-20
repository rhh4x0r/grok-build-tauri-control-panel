//! Project conversation, board and result inspection share one durable run.
use crate::{
    models::app::{project_name, AppModel},
    runtime::{services, spawn_service},
    theme::Ui,
};
use bomb_core::services::{
    self as svc,
    features::Assignment,
    model_suggestions::Candidate,
    project_work::{self as work, *},
};
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    input::{InputEvent, Textarea, TextareaState},
    menu::{DropdownMenu, PopupMenuItem},
    Disableable, Sizable,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use uuid::Uuid;

#[derive(Clone, Copy)]
enum Choice {
    Planner,
    Task(usize, usize),
    Reviewer(usize),
    ExistingTask(usize, usize),
    ExistingReviewer(usize),
    Repair(usize),
    Role(usize),
}
type ViewContext = (&'static str, Option<String>, Point<Pixels>, bool);
pub struct ProjectWorkView {
    model: Entity<AppModel>,
    project: String,
    input: Entity<TextareaState>,
    decision: Entity<super::feature_decision::FeatureDecisionView>,
    guidelines: Entity<TextareaState>,
    defaults: svc::features::ProjectSettings,
    sync_guidelines: Option<String>,
    attention_cursor: usize,
    tab: &'static str,
    selected: Option<String>,

    detail: &'static str,
    settings: bool,
    needs_only: bool,
    busy: bool,
    loading: bool,
    message: Option<String>,
    diff: Option<String>,
    planner: Option<Assignment>,
    preview: Entity<super::preview::PreviewPanel>,
    scroll: ScrollHandle,
    last_message: Option<String>,
    pin_frames: u8,
    generation: u64,
    submission: Option<String>,
    clear_submission: Option<String>,
    preview_busy: bool,
    preview_message: Option<String>,
    preview_switch: bool,
    preview_generation: u64,
    expanded_plan: bool,
    show_checks: bool,
    show_history: bool,
    show_preview: bool,
    history_limit: usize,
    new_activity: bool,
    list_board: bool,
    selected_file: Option<String>,
    inspected_revision: Option<String>,
    excluded: std::collections::HashSet<String>,
    board_offset: Point<Pixels>,
    contexts: std::collections::HashMap<String, ViewContext>,
}
impl ProjectWorkView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Describe a feature, a bug, or what you want to build next…")
                .auto_grow(2, 6)
                .submit_on_enter(true)
        });
        let guidelines=cx.new(|cx|TextareaState::new(window,cx).placeholder("e.g. Fable for frontend, Codex for backend, Grok for review. Explain any preferences here…").auto_grow(2,4));
        let decision = cx
            .new(|cx| super::feature_decision::FeatureDecisionView::new(model.clone(), window, cx));
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(&input, window, |v, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::Change) && !v.project.is_empty() {
                if let Err(e) = work::save_draft_text(
                    &services(cx),
                    &v.project,
                    "project",
                    &v.input.read(cx).value(),
                ) {
                    v.message = Some(e);
                }
            }
            if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                v.describe(window, cx);
            }
        })
        .detach();
        let preview = cx.new(|cx| super::preview::PreviewPanel::new(model.clone(), cx));
        Self {
            model,
            project: String::new(),
            input,
            decision,
            guidelines,
            defaults: Default::default(),
            sync_guidelines: None,
            attention_cursor: 0,
            tab: "Conversation",
            selected: None,

            detail: "Result",
            settings: false,
            needs_only: false,
            busy: false,
            loading: false,
            message: None,
            diff: None,
            planner: None,
            preview,
            scroll: ScrollHandle::new(),
            last_message: None,
            pin_frames: 0,
            generation: 0,
            submission: None,
            clear_submission: None,
            preview_busy: false,
            preview_message: None,
            preview_switch: false,
            preview_generation: 0,
            expanded_plan: false,
            show_checks: false,
            show_history: false,
            show_preview: false,
            history_limit: 60,
            new_activity: false,
            list_board: true,
            selected_file: None,
            inspected_revision: None,
            excluded: Default::default(),
            board_offset: point(px(0.), px(0.)),
            contexts: Default::default(),
        }
    }
    pub fn open(&mut self, board: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.sync(window, cx);
        self.tab = if board { "Board" } else { "Conversation" };
        self.selected = None;
        if !board {
            self.input.update(cx, |s, cx| s.focus(window, cx));
        }
        cx.notify();
    }
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let project = self
            .model
            .read(cx)
            .active_project
            .clone()
            .unwrap_or_default();
        if project == self.project {
            return;
        }
        if !self.project.is_empty() {
            let _ = work::save_draft_text(
                &services(cx),
                &self.project,
                "project",
                &self.input.read(cx).value(),
            );
            self.contexts.insert(
                self.project.clone(),
                (
                    self.tab,
                    self.selected.clone(),
                    self.scroll.offset(),
                    self.needs_only,
                ),
            );
        }
        self.project = project.clone();
        let context = self.contexts.get(&project).cloned().unwrap_or((
            "Conversation",
            None,
            point(px(0.), px(0.)),
            false,
        ));
        self.tab = context.0;
        self.selected = context.1;
        self.scroll.set_offset(context.2);
        self.needs_only = context.3;
        self.generation += 1;
        self.preview_generation += 1;
        self.preview_busy = false;
        self.preview_message = None;
        self.preview_switch = false;
        self.submission = None;
        self.clear_submission = None;
        self.excluded.clear();
        self.show_history = false;
        self.show_preview = false;
        self.expanded_plan = false;
        self.diff = None;
        self.inspected_revision = None;
        self.message = None;
        self.planner = None;
        self.busy = false;
        let draft = work::draft_text(&services(cx), &project, "project");
        self.input
            .update(cx, |s, cx| s.set_value(draft, window, cx));
        if project.is_empty() {
            return;
        }
        self.loading = true;
        let state = services(cx);
        let weak = cx.entity().downgrade();
        spawn_service(
            cx,
            async move {
                let result = async {
                    let p = work::load(&state, &project)?;
                    let settings = svc::features::load(&state, &project).await?.settings;
                    Ok::<_, String>((p, settings))
                }
                .await;
                (project, result)
            },
            move |(project, result), cx| {
                let _ = weak.update(cx, |v, cx| {
                    if v.project != project {
                        return;
                    }
                    v.loading = false;
                    match result {
                        Ok((p, settings)) => {
                            v.planner = p.planner;
                            v.sync_guidelines = Some(settings.guidelines.clone());
                            v.defaults = settings;
                        }
                        Err(e) => v.message = Some(e),
                    }
                    cx.notify();
                });
            },
        );
    }
    fn snapshot(&self, cx: &App) -> ProjectWork {
        services(cx)
            .project_work
            .snapshot(&self.project)
            .unwrap_or_default()
    }
    fn candidates(&self, cx: &App) -> Vec<Candidate> {
        let m = self.model.read(cx);
        m.backends
            .iter()
            .filter(|b| {
                b.available
                    && b.model_error.is_none()
                    && m.auth
                        .iter()
                        .any(|a| a.backend == b.id && a.logged_in && a.runnable)
            })
            .flat_map(|b| {
                b.models.iter().map(|id| Candidate {
                    backend: b.id.clone(),
                    model: id.clone(),
                    label: format!(
                        "{} · {}",
                        b.model_names.get(id).unwrap_or(id),
                        b.display_name
                    ),
                    description: b.model_descriptions.get(id).cloned().unwrap_or_default(),
                })
            })
            .collect()
    }
    fn planner(&self, cx: &App) -> Option<Assignment> {
        self.planner.clone().or_else(|| {
            let m = self.model.read(cx);
            let candidates = self.candidates(cx);
            let c = candidates
                .iter()
                .find(|c| c.backend == m.prefs.backend && c.model == m.effective_model())
                .or(candidates.first())?;
            let mut a = Assignment::from_candidate(c);
            if super::brand::effort_levels(&a.backend)
                .0
                .contains(&m.prefs.effort.as_str())
            {
                a.effort = m.prefs.effort.clone();
            }
            Some(a)
        })
    }
    fn operation<F>(&mut self, f: F, cx: &mut Context<Self>)
    where
        F: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        self.busy = true;
        self.message = None;
        self.generation += 1;
        let generation = self.generation;
        let submission = self.submission.take();
        let project = self.project.clone();
        let weak = cx.entity().downgrade();
        spawn_service(cx, f, move |result, cx| {
            if result.is_ok() {
                if let Some(sent) = &submission {
                    let state = services(cx);
                    if work::draft_text(&state, &project, "project") == *sent {
                        let _ = work::save_draft_text(&state, &project, "project", "");
                    }
                }
            }
            let _ = weak.update(cx, |v, cx| {
                if v.project == project && v.generation == generation {
                    v.busy = false;
                    match result {
                        Err(e) => v.message = Some(e),
                        Ok(()) => v.clear_submission = submission,
                    }
                }
                v.model.update(cx, |m, cx| {
                    m.refresh_threads(cx);
                    m.refresh_workspaces(cx);
                });
                cx.notify();
            });
        });
        cx.notify();
    }
    fn describe(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let p = self.snapshot(cx);
        let text = self.input.read(cx).value().to_string();
        if !p.enabled || text.trim().is_empty() {
            return;
        }
        if let Some(control) = work::project_control(&text) {
            self.submission = Some(text);
            self.command(control, cx);
            return;
        }
        if self.busy || p.planning {
            return;
        }
        let Some(planner) = self.planner(cx) else {
            self.message = Some("Connect a model in Settings to plan work.".into());
            return;
        };
        let catalog = self.candidates(cx);
        let state = services(cx);
        let project = self.project.clone();
        self.submission = Some(text.clone());
        self.operation(
            async move { work::describe(state, project, text, planner, catalog).await },
            cx,
        );
    }
    fn answer_plan(
        &mut self,
        prompt: String,
        answer: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let p = self.snapshot(cx);
        if p.planning
            || self.busy
            || p.draft
                .as_ref()
                .and_then(|d| d.planning_question.as_ref())
                .is_none_or(|q| q.prompt != prompt)
        {
            self.message = Some(
                "The planning question changed. Review its current version before answering."
                    .into(),
            );
            cx.notify();
            return;
        }
        let current = self.input.read(cx).value().to_string();
        let reply = format!("For the planning question ‘{prompt}’: {answer}");
        self.input.update(cx, |s, cx| {
            s.set_value(
                if current.trim().is_empty() {
                    reply
                } else {
                    format!("{current}\n{reply}")
                },
                window,
                cx,
            );
            s.focus(window, cx);
        });
        if current.trim().is_empty() {
            self.describe(window, cx);
        } else {
            self.message =
                Some("Answer added to your unsent message. Review it, then Send.".into());
        }
        cx.notify();
    }
    fn accept(
        &mut self,
        start: bool,
        expected: String,
        selected: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(draft) = self.snapshot(cx).draft else {
            return;
        };
        let current_selected: Vec<_> = draft
            .features
            .iter()
            .filter(|f| !self.excluded.contains(&f.id))
            .map(|f| f.id.clone())
            .collect();
        if serde_json::to_string(&draft).ok().as_deref() != Some(expected.as_str())
            || current_selected != selected
        {
            self.message = Some(
                "The plan or selection changed. Review the current plan before starting.".into(),
            );
            cx.notify();
            return;
        }
        if draft
            .features
            .iter()
            .flat_map(|f| {
                f.tasks
                    .iter()
                    .map(|t| &t.assignment)
                    .chain(std::iter::once(&f.verification.reviewer))
            })
            .any(|a| {
                !self
                    .candidates(cx)
                    .iter()
                    .any(|c| c.backend == a.backend && c.model == a.model)
            })
        {
            self.message=Some("A proposed model is no longer connected. Choose a connected model before accepting.".into());
            cx.notify();
            return;
        }
        let state = services(cx);
        let project = self.project.clone();
        self.operation(
            async move { work::accept_selected_plan(state, project, expected, start, selected).await },
            cx,
        );
    }
    pub fn command(&mut self, command: &'static str, cx: &mut Context<Self>) {
        let state = services(cx);
        let project = self.project.clone();
        self.operation(
            async move { work::command(state, project, command).await },
            cx,
        );
    }
    fn enable(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let state = services(cx);
        let project = self.project.clone();
        self.operation(
            async move { work::enable(&state, &project, enabled).await },
            cx,
        );
    }
    fn policy(&mut self, policy: RunPolicy, cx: &mut Context<Self>) {
        if let Err(e) = work::set_policy(&services(cx), &self.project, policy) {
            self.message = Some(e);
        }
        cx.notify();
    }
    fn assignment(&mut self, choice: Choice, a: Assignment, cx: &mut Context<Self>) {
        if matches!(choice, Choice::Planner) {
            self.planner = Some(a);
            cx.notify();
            return;
        }
        if let Choice::Role(index) = choice {
            if let Some(role) = ["Frontend", "Backend", "Build", "Review"].get(index) {
                self.defaults.roles.insert((*role).into(), a);
            }
            cx.notify();
            return;
        }
        let p = self.snapshot(cx);
        if let Choice::ExistingTask(fi, ti) = choice {
            if let Some(f) = p.features.get(fi) {
                if let Ok(spec) = f.feature() {
                    if let Some(t) = spec.tasks.get(ti) {
                        if let Err(e) =
                            work::replace_assignment(&services(cx), &self.project, &f.id, &t.id, a)
                        {
                            self.message = Some(e);
                        }
                    }
                }
            }
            cx.notify();
            return;
        }
        if let Choice::ExistingReviewer(fi) | Choice::Repair(fi) = choice {
            if let Some(f) = p.features.get(fi) {
                if let Err(e) = work::replace_assignment(
                    &services(cx),
                    &self.project,
                    &f.id,
                    if matches!(choice, Choice::Repair(_)) {
                        "repair"
                    } else {
                        "reviewer"
                    },
                    a,
                ) {
                    self.message = Some(e);
                }
            }
            cx.notify();
            return;
        }
        if let Some(mut draft) = p.draft {
            match choice {
                Choice::Task(f, t) => draft.features[f].tasks[t].assignment = a,
                Choice::Reviewer(f) => draft.features[f].verification.reviewer = a,
                _ => {}
            }
            if let Err(e) = work::update_draft(&services(cx), &self.project, draft) {
                self.message = Some(e);
            }
        }
        cx.notify();
    }
    fn picker(
        &self,
        key: String,
        choice: Choice,
        a: Assignment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = self.model.read(cx).model_name(&a.backend, &a.model);
        let catalog = self.candidates(cx);
        let p = self.snapshot(cx);
        let immutable = match choice {
            Choice::ExistingTask(fi, ti) => p.features.get(fi).is_none_or(|f| {
                f.feature()
                    .ok()
                    .and_then(|s| {
                        s.tasks.get(ti).and_then(|t| f.tasks.get(&t.id)).map(|t| {
                            matches!(t.state, TaskState::Running | TaskState::Checkpointed)
                        })
                    })
                    .unwrap_or(true)
            }),
            Choice::ExistingReviewer(fi) | Choice::Repair(fi) => {
                p.features.get(fi).is_none_or(|f| {
                    f.state == FeatureState::Done
                        || p.active_jobs
                            .iter()
                            .any(|k| k == &f.id || k.starts_with(&format!("{}/", f.id)))
                })
            }
            _ => false,
        };
        let name = if catalog
            .iter()
            .any(|c| c.backend == a.backend && c.model == a.model)
        {
            name
        } else {
            format!("{name} · unavailable")
        };
        let weak = cx.entity().downgrade();
        let weak2 = weak.clone();
        let selected = a.clone();
        let levels = super::brand::effort_levels(&a.backend).0;
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .child(super::brand::brand_mark(&a.backend, 16., true, &Ui::of(cx)))
            .child(
                Button::new(SharedString::from(format!("{key}-model")))
                    .ghost()
                    .small()
                    .disabled(self.busy || immutable)
                    .label(name)
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut menu, _, _| {
                        for c in &catalog {
                            let weak = weak.clone();
                            let c = c.clone();
                            menu = menu.item(
                                PopupMenuItem::new(c.label.clone())
                                    .checked(
                                        c.backend == selected.backend && c.model == selected.model,
                                    )
                                    .on_click(move |_, _, cx| {
                                        let _ = weak.update(cx, |v, cx| {
                                            v.assignment(choice, Assignment::from_candidate(&c), cx)
                                        });
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .child(
                Button::new(SharedString::from(format!("{key}-effort")))
                    .ghost()
                    .small()
                    .disabled(self.busy || immutable)
                    .label(format!(
                        "Reasoning: {}",
                        super::brand::effort_label(&a.effort)
                    ))
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut menu, _, _| {
                        for level in levels {
                            let weak = weak2.clone();
                            let a = a.clone();
                            let level = *level;
                            menu = menu.item(
                                PopupMenuItem::new(super::brand::effort_label(level))
                                    .checked(a.effort == level)
                                    .on_click(move |_, _, cx| {
                                        let mut a = a.clone();
                                        a.effort = level.into();
                                        let _ = weak.update(cx, |v, cx| {
                                            v.assignment(choice, a.clone(), cx)
                                        });
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .into_any_element()
    }
    pub fn inspect(&mut self, id: String, cx: &mut Context<Self>) {
        if self.selected.is_none() {
            self.board_offset = self.scroll.offset();
        }
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.preview_generation += 1;
        self.preview_busy = false;
        self.preview_message = None;
        self.preview_switch = false;
        self.selected_file = None;
        self.inspected_revision = None;
        self.selected = Some(id);
        self.detail = "Result";
        self.show_preview = false;
        self.show_checks = false;
        self.diff = None;
        cx.notify();
    }
    fn open_thread(&self, id: Uuid, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| {
            m.features_open = false;
            m.select(Some(id), cx);
        });
    }
    fn changes(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let expected = self
            .snapshot(cx)
            .features
            .iter()
            .find(|f| f.id == id)
            .map(FeatureWork::decision_key);
        self.detail = "Changes";
        self.diff = None;
        let project = self.project.clone();
        let state = services(cx);
        let weak = cx.entity().downgrade();
        spawn_service(
            cx,
            async move {
                let result = work::diff(&state, &project, &id).await;
                (project, id, result)
            },
            move |(project, id, result), cx| {
                let _ = weak.update(cx, |v, cx| {
                    if v.project == project
                        && v.selected.as_ref() == Some(&id)
                        && v.snapshot(cx)
                            .features
                            .iter()
                            .find(|f| f.id == id)
                            .map(FeatureWork::decision_key)
                            == expected
                    {
                        v.diff = Some(result.unwrap_or_else(|e| e));
                    }
                    cx.notify();
                });
            },
        );
        cx.notify();
    }
    fn preview(&mut self, cx: &mut Context<Self>) {
        self.start_preview(false, cx);
    }
    fn start_preview(&mut self, switch: bool, cx: &mut Context<Self>) {
        self.detail = "Result";
        self.show_preview = true;
        if self.preview_busy {
            return;
        }
        let Some(id) = self.selected.clone() else {
            return;
        };
        let expected = self
            .snapshot(cx)
            .features
            .iter()
            .find(|f| f.id == id)
            .map(FeatureWork::decision_key);
        let state = services(cx);
        let project = self.project.clone();
        let path = match work::candidate_path(&state, &project, &id) {
            Ok(p) => p,
            Err(e) => {
                self.preview_message = Some(e);
                cx.notify();
                return;
            }
        };
        self.preview
            .update(cx, |p, cx| p.set_project_root(path.clone(), cx));
        self.preview_busy = true;
        self.preview_switch = false;
        self.preview_message = Some("Starting preview…".into());
        self.preview_generation += 1;
        let generation = self.preview_generation;
        let weak = cx.entity().downgrade();
        spawn_service(
            cx,
            async move {
                let current = state.dev_server.status().await;
                let result = if current.running && current.cwd.as_deref() == path.to_str() {
                    Ok(current)
                } else if current.running && !switch {
                    return (project,id,generation,Err("Another feature's preview is running. Switch preview to stop it and open this feature.".into()),true);
                } else {
                    state
                        .dev_server
                        .preview(&path, if switch { current.cwd } else { None })
                        .await
                };
                (project, id, generation, result, false)
            },
            move |(project, id, generation, result, needs_switch), cx| {
                let _=weak.update(cx,|v,cx| {
                if v.project!=project || v.selected.as_ref()!=Some(&id) || v.preview_generation!=generation || v.snapshot(cx).features.iter().find(|f| f.id == id).map(FeatureWork::decision_key) != expected {return;}
                v.preview_busy=false;v.preview_switch=needs_switch;
                match result {Ok(status)=>{v.preview_message=Some("Preview ready · this feature's working copy. Follow the test steps and verify connected services; fixtures may still be in use.".into());v.model.update(cx,|m,cx|{m.dev_server=Some(status);cx.notify();});},Err(e)=>v.preview_message=Some(e)}
                cx.notify();
            });
            },
        );
        cx.notify();
    }
    pub fn next_decision(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let p = self.snapshot(cx);
        let mut targets: Vec<(Option<String>, Option<Uuid>)> = Vec::new();
        if !p.questions.is_empty()
            || p.draft
                .as_ref()
                .is_some_and(|d| d.planning_question.is_some())
        {
            targets.push((Some("questions".into()), None));
        }
        if p.final_blocker.is_some() {
            targets.push((Some("final".into()), None));
        }
        for f in &p.features {
            if f.state == FeatureState::NeedsInput
                || (f.state == FeatureState::Review && f.approved_head.is_none())
            {
                targets.push((Some(f.id.clone()), None));
            }
        }
        for (sid, t) in &self.model.read(cx).threads {
            let t = t.read(cx);
            if t.meta.live && t.meta.project_root.as_deref() == Some(&self.project) {
                for _ in t.thread.open_approvals() {
                    targets.push((None, Some(*sid)));
                }
            }
        }
        if targets.is_empty() {
            self.message = Some("No unresolved decisions in this project.".into());
            cx.notify();
            return;
        }
        let (feature, thread) = targets[self.attention_cursor % targets.len()].clone();
        self.attention_cursor += 1;
        if let Some(sid) = thread {
            self.open_thread(sid, cx);
        } else if let Some(id) = feature {
            if id == "questions" {
                self.selected = None;
                self.tab = "Conversation";
                self.input.update(cx, |s, cx| s.focus(window, cx));
            } else if id == "final" {
                self.selected = None;
                self.needs_only = true;
                self.tab = "Board";
                self.scroll.set_offset(point(px(0.), px(0.)));
            } else {
                self.inspect(id, cx);
            }
        }
        cx.notify();
    }
    fn save_defaults(&mut self, cx: &mut Context<Self>) {
        let state = services(cx);
        let project = self.project.clone();
        let mut settings = self.defaults.clone();
        settings.enabled = self.snapshot(cx).enabled;
        settings.guidelines = self.guidelines.read(cx).value().to_string();
        self.operation(
            async move { svc::features::save_settings(&state, &project, settings).await },
            cx,
        );
    }
    fn discard(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = work::discard_proposal(&services(cx), &self.project) {
            self.message = Some(e);
        }
        self.excluded.clear();
        cx.notify();
    }
    fn retry_final(&mut self, cx: &mut Context<Self>) {
        let Some(b) = self.snapshot(cx).final_blocker else {
            return;
        };
        let state = services(cx);
        let project = self.project.clone();
        self.operation(
            async move { work::retry_final(state, project, b.id).await },
            cx,
        );
    }
}
#[path = "project_work_render.rs"]
mod render;
