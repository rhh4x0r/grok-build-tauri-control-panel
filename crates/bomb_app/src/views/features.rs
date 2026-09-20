//! Project feature board and inline task authoring. No secondary windows.
use crate::{
    models::app::{project_name, AppModel},
    runtime::{services, spawn_service},
    theme::Ui,
};
use bomb_core::services::{
    features::{self, Assignment, Feature, FeatureTask, ProjectBoard, Record},
    model_suggestions::{self as routing, Candidate},
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    input::{Input, InputState, Textarea, TextareaState},
    menu::{DropdownMenu, PopupMenuItem},
    Disableable, Sizable,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

struct TaskEditor {
    task: FeatureTask,
    title: Entity<InputState>,
    brief: Entity<TextareaState>,
    proposal: Option<Assignment>,
    feedback: Option<String>,
}
struct Editor {
    feature: Feature,
    revision: Option<String>,
    title: Entity<InputState>,
    brief: Entity<TextareaState>,
    tasks: Vec<TaskEditor>,
    undo: Option<String>,
}
pub struct FeaturesView {
    model: Entity<AppModel>,
    project: String,
    source_thread: Option<Uuid>,
    board: ProjectBoard,
    editor: Option<Editor>,
    drafts: HashMap<String, Editor>,
    guidelines: Entity<TextareaState>,
    message: Option<String>,
    busy: bool,
    loading: bool,
    suggesting: bool,
    generation: u64,
    launching: HashSet<String>,
    new_after_load: bool,
    preferences_open: bool,
    show_completed: bool,
    enhanced: Option<String>,
    sync_guidelines: bool,
    suggestion_signature: Option<String>,
}
impl FeaturesView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        let guidelines = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(
                    "e.g. Fable for frontend, Codex for backend, a cheap Grok model for review",
                )
                .auto_grow(2, 5)
        });
        cx.observe(&guidelines, |_, _, cx| cx.notify()).detach();
        Self {
            model,
            project: String::new(),
            source_thread: None,
            board: ProjectBoard::default(),
            editor: None,
            drafts: HashMap::new(),
            guidelines,
            message: None,
            busy: false,
            loading: false,
            suggesting: false,
            generation: 0,
            launching: HashSet::new(),
            new_after_load: false,
            preferences_open: false,
            show_completed: false,
            enhanced: None,
            sync_guidelines: false,
            suggestion_signature: None,
        }
    }
    pub fn open(&mut self, new: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.model.read(cx).active_project.clone() else {
            return;
        };
        if self.busy || self.loading {
            self.message =
                Some("Finish the current feature operation before changing projects.".into());
            cx.notify();
            return;
        }
        if project == self.project && self.editor.is_some() {
            self.message=Some("Your unfinished feature draft is still here. Save it or return to the feature list before starting another.".into());
            cx.notify();
            return;
        }
        if let Some(editor) = self.editor.take() {
            self.drafts.insert(self.project.clone(), editor);
        }
        self.editor = self.drafts.remove(&project);
        self.suggesting = false;
        self.sync_guidelines = true;
        self.enhanced = None;
        self.suggestion_signature = None;
        self.project = project;
        self.source_thread = self.model.read(cx).selected;
        self.generation += 1;
        self.new_after_load = new;
        self.message = None;
        self.board = ProjectBoard::default();
        self.guidelines
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.reload(cx);
    }
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let state = services(cx);
        let project = self.project.clone();
        let generation = self.generation;
        let weak = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { features::load(&state, &project).await },
            move |result, cx| {
                let _ = weak.update(cx, |v, cx| {
                    if v.generation != generation {
                        return;
                    }
                    v.loading = false;
                    match result {
                        Ok(board) => v.board = board,
                        Err(e) => v.message = Some(e),
                    }
                    cx.notify();
                });
            },
        );
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
    fn routing_enabled(&self, cx: &App) -> bool {
        let state = services(cx);
        state
            .config
            .try_read()
            .ok()
            .is_some_and(|c| c.model_suggestions.enabled)
            && routing::thread_enabled(&state.persistence, self.source_thread)
    }
    fn default_assignment(&self, role: &str, cx: &App) -> Option<Assignment> {
        if let Some(a) = self.board.settings.roles.get(role) {
            return Some(a.clone());
        }
        let m = self.model.read(cx);
        self.candidates(cx)
            .iter()
            .find(|c| c.backend == m.prefs.backend && c.model == m.effective_model())
            .map(|c| {
                let mut a = Assignment::from_candidate(c);
                if super::brand::effort_levels(&a.backend)
                    .0
                    .contains(&m.prefs.effort.as_str())
                {
                    a.effort = m.prefs.effort.clone();
                }
                a
            })
    }
    fn task_editor(
        &self,
        task: FeatureTask,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> TaskEditor {
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Task name"));
        title.update(cx, |s, cx| s.set_value(task.title.clone(), window, cx));
        let brief=cx.new(|cx|TextareaState::new(window,cx).placeholder("What should this task do? Include its edit area, assumptions and focused checks.").auto_grow(2,7));
        brief.update(cx, |s, cx| s.set_value(task.brief.clone(), window, cx));
        cx.observe(&title, |_, _, cx| cx.notify()).detach();
        cx.observe(&brief, |_, _, cx| cx.notify()).detach();
        TaskEditor {
            task,
            title,
            brief,
            proposal: None,
            feedback: None,
        }
    }
    fn edit(&mut self, record: Option<Record>, window: &mut Window, cx: &mut Context<Self>) {
        let (mut feature, revision) = record
            .map(|r| (r.feature, Some(r.revision)))
            .unwrap_or_else(|| (Feature::new(), None));
        if feature.tasks.is_empty() && revision.is_none() {
            let mut t = FeatureTask::new("Build");
            t.assignment = self.default_assignment("Build", cx);
            feature.tasks.push(t);
        }
        let title = cx.new(|cx| InputState::new(window, cx).placeholder("Feature name"));
        title.update(cx, |s, cx| s.set_value(feature.title.clone(), window, cx));
        let brief=cx.new(|cx|TextareaState::new(window,cx).placeholder("What do you want to build? Describe the outcome, what done looks like, and anything to leave out.").auto_grow(4,14));
        brief.update(cx, |s, cx| s.set_value(feature.brief.clone(), window, cx));
        cx.observe(&title, |_, _, cx| cx.notify()).detach();
        cx.observe(&brief, |_, _, cx| cx.notify()).detach();
        let tasks = feature
            .tasks
            .iter()
            .cloned()
            .map(|t| self.task_editor(t, window, cx))
            .collect();
        self.enhanced = None;
        self.suggestion_signature = None;
        self.editor = Some(Editor {
            feature,
            revision,
            title,
            brief,
            tasks,
            undo: None,
        });
        self.generation += 1;
        self.suggesting = false;
        self.message = None;
        cx.notify();
    }
    fn draft(&self, cx: &App) -> Option<Feature> {
        let e = self.editor.as_ref()?;
        let mut f = e.feature.clone();
        f.title = e.title.read(cx).value().to_string();
        f.brief = e.brief.read(cx).value().to_string();
        f.tasks = e
            .tasks
            .iter()
            .map(|t| {
                let mut task = t.task.clone();
                task.title = t.title.read(cx).value().to_string();
                task.brief = t.brief.read(cx).value().to_string();
                task
            })
            .collect();
        Some(f)
    }
    fn signature(&self, cx: &App) -> String {
        format!(
            "{:?}|{}|{:?}|{}|{:?}|{:?}",
            self.draft(cx),
            self.guidelines.read(cx).value(),
            self.candidates(cx),
            self.routing_enabled(cx),
            services(cx)
                .config
                .try_read()
                .ok()
                .map(|c| format!("{:?}", c.model_suggestions)),
            self.board.settings.roles
        )
    }
    fn add_task(&mut self, role: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut task = FeatureTask::new(role);
        task.assignment = self.default_assignment(role, cx);
        if role == "Frontend" {
            task.brief="Build the interface and interaction states. Start with provisional mock data when useful; isolate data access behind an adapter. Record what still needs real API wiring.".into();
        }
        if role == "Backend" {
            task.brief="Implement backend behavior and focused checks. Propose the interface needed by the UI; record assumptions and integration work.".into();
        }
        if role == "Review" {
            task.brief="Review the selected task against the feature criteria. Report concrete defects, check results, and any untested behavior.".into();
        }
        let task = self.task_editor(task, window, cx);
        if let Some(e) = &mut self.editor {
            e.tasks.push(task);
        }
        cx.notify();
    }
    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.draft(cx) else {
            return;
        };
        let expected = self.editor.as_ref().and_then(|e| e.revision.clone());
        self.busy = true;
        let state = services(cx);
        let project = self.project.clone();
        let generation = self.generation;
        let weak = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { features::save(&state, &project, f, expected).await },
            move |result, cx| {
                let _=weak.update(cx,|v,cx|{
            if generation!=v.generation{return;}v.busy=false;match result{Ok(_)=>{v.editor=None;v.message=Some("Feature saved. Start independent tasks below; each gets its own worktree.".into());v.reload(cx);},Err(e)=>v.message=Some(e)}cx.notify();
        });
            },
        );
        cx.notify();
    }
    fn save_preferences(&mut self, enabled: bool, save_roles: bool, cx: &mut Context<Self>) {
        let mut settings = self.board.settings.clone();
        settings.enabled = enabled;
        settings.guidelines = self.guidelines.read(cx).value().to_string();
        if save_roles {
            if let Some(f) = self.draft(cx) {
                let mut choices = std::collections::BTreeMap::new();
                for t in f.tasks {
                    if let Some(a) = t.assignment {
                        if choices.get(&t.role).is_some_and(|existing| existing != &a) {
                            self.message=Some(format!("The {} tasks use different models or efforts. Use one choice per role when saving defaults.",t.role));
                            cx.notify();
                            return;
                        }
                        choices.insert(t.role, a);
                    }
                }
                settings.roles.extend(choices);
            }
        }
        let state = services(cx);
        let project = self.project.clone();
        let generation = self.generation;
        let weak = cx.entity().downgrade();
        self.busy = true;
        spawn_service(
            cx,
            async move { features::save_settings(&state, &project, settings).await },
            move |result, cx| {
                let _ = weak.update(cx, |v, cx| {
                    if v.generation != generation {
                        return;
                    }
                    v.busy = false;
                    match result {
                        Ok(()) => {
                            v.message = Some(
                                if enabled {
                                    "Project preferences saved. Ordinary threads remain available."
                                } else {
                                    "Feature tracking is off. Your records and running threads are kept."
                                }
                                .into(),
                            );
                            v.reload(cx);
                        }
                        Err(e) => v.message = Some(e),
                    }
                    cx.notify();
                });
            },
        );
    }
    fn suggest(&mut self, cx: &mut Context<Self>) {
        let Some(mut f) = self.draft(cx) else {
            return;
        };
        f.tasks.retain(|t| {
            !self
                .board
                .runs
                .get(&features::task_key(&f.id, &t.id))
                .is_some_and(|r| r.thread.is_some())
        });
        if !self.routing_enabled(cx) {
            return;
        }
        let candidates = self.candidates(cx);
        if candidates.len() < 2 {
            self.message = Some("Connect at least two models to compare task assignments.".into());
            cx.notify();
            return;
        }
        let signature = self.signature(cx);
        let generation = self.generation;
        let preferences = format!(
            "{}\nSaved role assignments: {:?}",
            self.guidelines.read(cx).value(),
            self.board.settings.roles
        );
        let source = self.source_thread;
        let state = services(cx);
        let weak = cx.entity().downgrade();
        self.suggesting = true;
        spawn_service(
            cx,
            async move {
                let mut jobs = tokio::task::JoinSet::new();
                for task in &f.tasks {
                    let current = task
                        .assignment
                        .as_ref()
                        .and_then(|a| {
                            candidates
                                .iter()
                                .find(|c| c.backend == a.backend && c.model == a.model)
                        })
                        .cloned()
                        .unwrap_or_else(|| candidates[0].clone());
                    let id = task.id.clone();
                    let prompt=format!("Assign a model to the {} part of this feature.\nFeature: {}\n{}\nTask: {}\n{}",task.role,f.title,f.brief,task.title,task.brief);
                    let state = state.clone();
                    let candidates = candidates.clone();
                    let preferences = preferences.clone();
                    jobs.spawn(async move {
                        let r = routing::suggest_with_guidelines(
                            &state,
                            source,
                            prompt,
                            current,
                            candidates,
                            preferences,
                        )
                        .await;
                        (id, r)
                    });
                }
                let mut results = vec![];
                while let Some(r) = jobs.join_next().await {
                    if let Ok(r) = r {
                        results.push(r);
                    }
                }
                results
            },
            move |results, cx| {
                let _=weak.update(cx,|v,cx|{if v.generation!=generation{return;}v.suggesting=false;
            if v.signature(cx)!=signature{v.message=Some("The brief, models or routing settings changed. Request fresh suggestions for this draft.".into());cx.notify();return;}
            v.suggestion_signature=Some(signature.clone());if let Some(e)=&mut v.editor{for (id,r) in results{if let Some(t)=e.tasks.iter_mut().find(|t|t.task.id==id){match r{Ok(r)=>{t.proposal=r.suggestion.map(|s|Assignment::from_candidate(&s.candidate));t.feedback=Some(r.message);},Err(e)=>t.feedback=Some(e)}}}}
            cx.notify();});
            },
        );
        cx.notify();
    }
    fn enhance(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.draft(cx) else {
            return;
        };
        if f.brief.trim().is_empty() {
            return;
        }
        let m = self.model.read(cx);
        let backend = m.prefs.backend.clone();
        let model = m.effective_model();
        let original = f.brief;
        let options=bomb_foundry::PromptOptions{depth:"fast-draft".into(),target:match backend.as_str(){"claude"=>"claude-code","codex"=>"openai-codex",_=>"grok-build"}.into(),work_type:"implementation".into(),autonomy:"Improve this brief only; do not execute, allocate tasks, change models or permissions.".into(),sources:vec![]};
        let state = services(cx);
        let generation = self.generation;
        let weak = cx.entity().downgrade();
        self.busy = true;
        spawn_service(
            cx,
            async move {
                let r = bomb_core::foundry::generate_prompt(
                    state,
                    original.clone(),
                    backend,
                    model,
                    options,
                )
                .await;
                (original, r)
            },
            move |(_original, result), cx| {
                let _=weak.update(cx,|v,cx|{if generation!=v.generation{return;}v.busy=false;
            match result{Ok(text)=>{v.enhanced=Some(text);v.message=Some("Enhanced brief is ready. Apply it below or keep your current draft.".into());},Err(e)=>v.message=Some(e)}cx.notify();});
            },
        );
    }
    fn start(&mut self, f: String, t: String, cx: &mut Context<Self>) {
        let key = features::task_key(&f, &t);
        if !self.launching.insert(key.clone()) {
            return;
        }
        let state = services(cx);
        let project = self.project.clone();
        let generation = self.generation;
        let weak = cx.entity().downgrade();
        spawn_service(
            cx,
            async move {
                let result = features::start_task(&state, &project, &f, &t).await;
                let threads = bomb_core::services::list_threads(&state).await;
                let board = features::load(&state, &project).await;
                (result, threads, board, state, project, f, t)
            },
            move |(result, threads, board, state, project, f, t), cx| {
                let _ = weak.update(cx, |v, cx| {
                    v.launching.remove(&key);
                    if v.generation == generation {
                        if let Err(e) = &result {
                            v.message = Some(e.clone());
                        }
                        v.reload(cx);
                    }
                    v.model.update(cx, |m, cx| {
                        if let Ok(threads) = threads {
                            m.set_threads(threads, cx);
                        }
                        if let (Ok(id), Ok(board)) = (&result, &board) {
                            if let (Some(entity), Some(prompt)) = (
                                m.threads.get(id),
                                board
                                    .runs
                                    .get(&features::task_key(&f, &t))
                                    .and_then(|r| r.prompt.as_ref()),
                            ) {
                                entity.update(cx, |thread, cx| {
                                    let changes = thread.thread.note_prompt(
                                        prompt,
                                        vec![],
                                        std::time::Instant::now(),
                                    );
                                    thread.absorb(&changes, cx);
                                });
                            }
                        }
                        m.refresh_workspaces(cx);
                    });
                    cx.notify();
                });
                if let Ok(id) = result {
                    let weak = weak.clone();
                    spawn_service(
                        cx,
                        async move { features::send_task(&state, &project, &f, &t, id).await },
                        move |result, cx| {
                            let _=weak.update(cx,|v,cx|{if v.generation==generation{if let Err(e)=result{v.message=Some(format!("Task needs attention: {e}. Open its thread to continue."));}v.reload(cx);cx.notify();}});
                        },
                    );
                }
            },
        );
        cx.notify();
    }
    fn open_thread(&self, id: Uuid, cx: &mut Context<Self>) {
        let effort = self
            .board
            .runs
            .values()
            .find(|r| r.thread == Some(id))
            .and_then(|r| r.applied.as_ref().or(r.assignment.as_ref()))
            .map(|a| a.effort.clone());
        self.model.update(cx, |m, cx| {
            m.features_open = false;
            if let Some(effort) = effort {
                m.prefs.effort = effort;
            }
            m.select(Some(id), cx);
            cx.notify();
        });
    }
    fn ready(&mut self, f: String, t: String, cx: &mut Context<Self>) {
        let state = services(cx);
        let project = self.project.clone();
        let weak = cx.entity().downgrade();
        let generation = self.generation;
        spawn_service(
            cx,
            async move { features::mark_ready(&state, &project, &f, &t).await },
            move |result, cx| {
                let _=weak.update(cx,|v,cx|{if generation!=v.generation{return;}v.message=Some(match result{Ok(())=>"Checkpoint marked Ready for review. It has not been integrated or approved.".into(),Err(e)=>e});v.reload(cx);cx.notify();});
            },
        );
    }
    fn attach(&mut self, f: String, t: String, id: Uuid, cx: &mut Context<Self>) {
        let state = services(cx);
        let project = self.project.clone();
        let weak = cx.entity().downgrade();
        let generation = self.generation;
        spawn_service(
            cx,
            async move { features::attach(&state, &project, &f, &t, id).await },
            move |result, cx| {
                let _ = weak.update(cx, |v, cx| {
                    if generation != v.generation {
                        return;
                    }
                    v.message = Some(match result {
                        Ok(()) => {
                            "Existing thread linked. Its current workspace and model are unchanged."
                                .into()
                        }
                        Err(e) => e,
                    });
                    v.reload(cx);
                    cx.notify();
                });
            },
        );
    }
    fn set_disposition(&mut self, feature: String, status: &str, cx: &mut Context<Self>) {
        let state = services(cx);
        let project = self.project.clone();
        let weak = cx.entity().downgrade();
        let generation = self.generation;
        let status = status.to_string();
        spawn_service(
            cx,
            async move { features::set_disposition(&state, &project, &feature, &status).await },
            move |result, cx| {
                let _ = weak.update(cx, |v, cx| {
                    if generation != v.generation {
                        return;
                    }
                    if let Err(e) = result {
                        v.message = Some(e);
                    }
                    v.reload(cx);
                    cx.notify();
                });
            },
        );
    }
    fn create_guide(&mut self, cx: &mut Context<Self>) {
        let state = services(cx);
        let project = self.project.clone();
        let weak = cx.entity().downgrade();
        let generation = self.generation;
        self.busy = true;
        spawn_service(
            cx,
            async move { features::create_guide(&state, &project).await },
            move |result, cx| {
                let _=weak.update(cx,|v,cx|{if generation!=v.generation{return;}v.busy=false;v.message=Some(match result{Ok(())=>"Created plan/PROJECT.md. Fill in the project map and check commands; existing instructions are unchanged.".into(),Err(e)=>e});cx.notify();});
            },
        );
    }
    fn snapshot(&mut self, cx: &mut Context<Self>) {
        let state = services(cx);
        let project = self.project.clone();
        let weak = cx.entity().downgrade();
        let generation = self.generation;
        self.busy = true;
        spawn_service(
            cx,
            async move { features::snapshot(&state, &project).await },
            move |result, cx| {
                let _ = weak.update(cx, |v, cx| {
                    if generation != v.generation {
                        return;
                    }
                    v.busy = false;
                    v.message = Some(match result {
                        Ok(()) => "Saved plan/STATUS.md with local checkpoint evidence.".into(),
                        Err(e) => e,
                    });
                    cx.notify();
                });
            },
        );
    }
}

#[path = "feature_board.rs"]
mod board;
#[path = "feature_editor.rs"]
mod editor;
