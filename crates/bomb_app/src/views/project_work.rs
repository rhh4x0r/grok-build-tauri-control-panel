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
}
pub struct ProjectWorkView {
    model: Entity<AppModel>,
    project: String,
    input: Entity<TextareaState>,
    feedback: Entity<TextareaState>,
    tab: &'static str,
    selected: Option<String>,
    feedback_context: Option<String>,
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
        let feedback = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(
                    "What should change? Describe what you saw or answer the agent’s question…",
                )
                .auto_grow(2, 5)
        });
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        cx.subscribe_in(&input, window, |v, _, event: &InputEvent, window, cx| {
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
            feedback,
            tab: "Conversation",
            selected: None,
            feedback_context: None,
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
        }
    }
    pub fn open(&mut self, board: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = if board { "Board" } else { "Conversation" };
        self.selected = None;
        self.sync(window, cx);
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
        self.project = project.clone();
        self.selected = None;
        self.diff = None;
        self.message = None;
        self.planner = None;
        self.busy = false;
        self.input.update(cx, |s, cx| s.set_value("", window, cx));
        if project.is_empty() {
            return;
        }
        self.loading = true;
        let state = services(cx);
        let weak = cx.entity().downgrade();
        spawn_service(
            cx,
            async move {
                let result = work::load(&state, &project);
                (project, result)
            },
            move |(project, result), cx| {
                let _ = weak.update(cx, |v, cx| {
                    if v.project != project {
                        return;
                    }
                    v.loading = false;
                    match result {
                        Ok(p) => v.planner = p.planner,
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
        let project = self.project.clone();
        let weak = cx.entity().downgrade();
        spawn_service(cx, f, move |result, cx| {
            let _ = weak.update(cx, |v, cx| {
                if v.project == project {
                    v.busy = false;
                    if let Err(e) = result {
                        v.message = Some(e);
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
    fn describe(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let p = self.snapshot(cx);
        if self.busy || p.planning || !p.enabled {
            return;
        }
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        let Some(planner) = self.planner(cx) else {
            self.message = Some("Connect a model in Settings to plan work.".into());
            return;
        };
        let catalog = self.candidates(cx);
        let state = services(cx);
        let project = self.project.clone();
        self.input.update(cx, |s, cx| s.set_value("", window, cx));
        self.operation(
            async move { work::describe(state, project, text, planner, catalog).await },
            cx,
        );
    }
    fn accept(&mut self, start: bool, cx: &mut Context<Self>) {
        let Some(draft) = self.snapshot(cx).draft else {
            return;
        };
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
        let expected = serde_json::to_string(&draft).unwrap_or_default();
        let state = services(cx);
        let project = self.project.clone();
        self.operation(
            async move { work::accept_plan(state, project, expected, start).await },
            cx,
        );
    }
    fn command(&mut self, command: &'static str, cx: &mut Context<Self>) {
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
        if let Some(mut draft) = self.snapshot(cx).draft {
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
                    .disabled(self.busy)
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
                    .disabled(self.busy)
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
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.selected = Some(id);
        self.detail = "Result";
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
                    if v.project == project && v.selected.as_ref() == Some(&id) {
                        v.diff = Some(result.unwrap_or_else(|e| e));
                    }
                    cx.notify();
                });
            },
        );
        cx.notify();
    }
    fn preview(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let state = services(cx);
        let project = self.project.clone();
        let Ok(path) = work::candidate_path(&state, &project, &id) else {
            return;
        };
        self.detail = "Preview";
        self.preview
            .update(cx, |p, cx| p.set_project_root(path.clone(), cx));
        let weak = cx.entity().downgrade();
        spawn_service(
            cx,
            async move {
                let current = state.dev_server.status().await;
                if current.running && current.cwd.as_deref() != path.to_str() {
                    return Err(
                        "Another preview is running. Stop it before previewing this candidate."
                            .to_string(),
                    );
                }
                svc::start_dev_server(
                    &state,
                    Some(path.to_string_lossy().into_owned()),
                    None,
                    Some(false),
                )
                .await
            },
            move |result, cx| {
                let _ = weak.update(cx, |v, cx| {
                    match result {
                        Ok(status) => v.model.update(cx, |m, cx| {
                            m.dev_server = Some(status);
                            cx.notify();
                        }),
                        Err(e) => v.message = Some(e),
                    }
                    cx.notify();
                });
            },
        );
        cx.notify();
    }
    fn approve(&mut self, f: FeatureWork, cx: &mut Context<Self>) {
        let Some(review) = f.review else {
            return;
        };
        let state = services(cx);
        let project = self.project.clone();
        self.operation(
            async move { work::approve(state, project, f.id, review.candidate).await },
            cx,
        );
    }
    fn revise(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let feedback = self.feedback.read(cx).value().to_string();
        let state = services(cx);
        let project = self.project.clone();
        self.feedback
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.operation(
            async move { work::keep_working(state, project, id, feedback).await },
            cx,
        );
    }
}
#[path = "project_work_render.rs"]
mod render;
