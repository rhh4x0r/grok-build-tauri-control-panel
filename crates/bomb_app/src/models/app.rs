//! Top-level UI state: projects, threads, services, selection, composer
//! preferences, and every user action that talks to the backend.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use bomb_core::devserver::DevServerStatus;
use bomb_core::services::{self, BackendInfo, ImageInput};
use bomb_core::transcript::ImageAttachment;
use bomb_core::ControlEvent;
use gpui_kit::*;
use grok_cli_wrapper::{BackendAuth, LoginPhase, LoginSessionState};
use grok_control_core::{ApprovalMode, SpawnOptions};
use grok_persistence::ThreadDto;
use tracing::debug;
use uuid::Uuid;

use crate::models::thread::ThreadModel;
use crate::runtime::{services as svc, spawn_service};

/// The one `AppModel` entity, reachable from any view.
pub struct AppModelHandle(pub Entity<AppModel>);
impl Global for AppModelHandle {}

const ARCHIVED_KEY: &str = "archived_threads";

#[derive(Debug, Clone)]
pub struct ProjectGroup {
    pub root: String,
    pub name: String,
    pub threads: Vec<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

/// What the composer sends with each prompt.
#[derive(Debug, Clone)]
pub struct ComposerPrefs {
    pub backend: String,
    pub model: Option<String>,
    /// plan | ask | auto | yolo
    pub mode: String,
    pub worktree: bool,
    pub mcp_servers: Vec<String>,
    /// low | medium | high (Grok only today).
    pub effort: String,
    pub fast_mode: Option<bool>,
    /// Temporary chat: run in the project checkout with no worktree, and
    /// don't keep the thread when it's deleted. (Today: no worktree.)
    pub temporary: bool,
}

impl Default for ComposerPrefs {
    fn default() -> Self {
        Self {
            backend: "grok".into(),
            model: None,
            mode: "plan".into(),
            worktree: true,
            mcp_servers: Vec::new(),
            effort: "high".into(),
            fast_mode: Some(false),
            temporary: false,
        }
    }
}

pub const APPROVAL_CYCLE: [&str; 4] = ["plan", "ask", "auto", "yolo"];

pub struct AppModel {
    pub projects: Vec<String>,
    pub workspaces: Vec<grok_persistence::WorkspaceRecord>,
    pub project_status: HashMap<String, bomb_core::services::workspaces::ProjectStatus>,
    pub project_overviews: HashMap<String, Result<bomb_core::services::project_overview::ProjectOverview, String>>,
    pub overview_loading: HashSet<String>,
    pub overview_branch: Option<String>,
    pub active_workspace: Option<String>,
    pub source_thread: Option<String>,
    pub review: Option<bomb_core::services::workspaces::WorkspaceReview>,
    pub review_open: bool,
    pub review_loading: bool,
    pub active_project: Option<String>,
    pub thread_order: Vec<Uuid>,
    pub threads: HashMap<Uuid, Entity<ThreadModel>>,
    pub selected: Option<Uuid>,
    pub new_thread_open: bool,
    pub file_reveal_request: Option<std::path::PathBuf>,
    pub auth: Vec<BackendAuth>,
    /// Account usage limits (5h / weekly) per backend, refreshed slowly.
    pub usage: Vec<bomb_core::usage::AccountUsage>,
    pub backends: Vec<BackendInfo>,
    pub models_loading: bool,
    pub prefs: ComposerPrefs,
    pub dev_server: Option<DevServerStatus>,
    /// Names of enabled MCP servers (for the composer picker).
    pub mcp_names: Vec<String>,
    pub login: Option<LoginSessionState>,
    /// A device-code login was requested and the first status is pending.
    pub login_starting: bool,
    pub last_error: Option<String>,
    /// Pending toasts; the root view drains them into the notification layer.
    pub toasts: VecDeque<(ToastKind, String)>,
    /// A prompt is in flight for a not-yet-created thread.
    pub starting: bool,
    /// Threads hidden from the sidebar (kv "archived_threads"). Nothing is
    /// deleted; the group's Archived shelf lists them.
    pub archived: HashSet<Uuid>,
    login_poll: Option<Task<()>>,
    dev_poll: Option<Task<()>>,
}

impl AppModel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            projects: Vec::new(),
            workspaces: Vec::new(),
            project_status: HashMap::new(),
            project_overviews: HashMap::new(),
            overview_loading: HashSet::new(),
            overview_branch: None,
            active_workspace: None,
            source_thread: None,
            review: None,
            review_open: false,
            review_loading: false,
            active_project: None,
            thread_order: Vec::new(),
            threads: HashMap::new(),
            selected: None,
            new_thread_open: false,
            file_reveal_request: None,
            auth: Vec::new(),
            usage: Vec::new(),
            backends: Vec::new(),
            models_loading: false,
            prefs: ComposerPrefs::default(),
            dev_server: None,
            mcp_names: Vec::new(),
            login: None,
            login_starting: false,
            last_error: None,
            toasts: VecDeque::new(),
            starting: false,
            archived: HashSet::new(),
            login_poll: None,
            dev_poll: None,
        };
        this.refresh_all(cx);
        this.refresh_project_status(false, cx);
        this.start_service_poll(cx);
        this
    }

    pub fn refresh_project_overview(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.active_project.clone() else { return; };
        if !self.overview_loading.insert(root.clone()) { return; }
        let key = root.clone();
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::project_overview::load(&root).await }, move |result, cx| {
            let _ = this.update(cx, |m, cx| {
                m.overview_loading.remove(&key);
                m.project_overviews.insert(key, result);
                cx.notify();
            });
        });
        cx.notify();
    }

    pub fn initialize_project_git(&mut self, root: String, cx: &mut Context<Self>) {
        if !self.overview_loading.insert(root.clone()) { return; }
        let key = root.clone();
        let this = cx.entity().downgrade();
        spawn_service(cx, async move {
            services::project_overview::initialize_repository(&root).await?;
            services::project_overview::load(&root).await
        }, move |result, cx| {
            let _ = this.update(cx, |m, cx| {
                m.overview_loading.remove(&key);
                match result {
                    Ok(overview) => {
                        m.project_overviews.insert(key, Ok(overview));
                        m.toast(ToastKind::Success, "Git repository ready");
                        m.refresh_project_status(false, cx);
                    }
                    Err(error) => m.fail(error, cx),
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    pub fn refresh_project_status(&mut self, fetch: bool, cx: &mut Context<Self>) {
        let state = svc(cx); let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::workspaces::refresh_projects(&state, fetch).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| { if let Ok(rows) = res { m.project_status = rows.into_iter().collect(); } cx.notify(); });
        });
    }

    pub fn pull_project(&mut self, root: String, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::workspaces::pull_project(root).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| { match res { Ok(note) => m.toast(ToastKind::Success, note), Err(e) => m.fail(e, cx) } m.refresh_project_status(false, cx); m.refresh_project_overview(cx); cx.notify(); });
        });
    }

    pub fn refresh_workspaces(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::workspaces::list_workspaces(&state).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                match res {
                    Ok(rows) => {
                        m.workspaces = rows;
                        if let Some(id) = m.selected {
                            m.active_workspace = m.workspaces.iter().find(|w| w.threads.contains(&id.to_string())).map(|w| w.id.clone());
                        }
                        m.refresh_review(cx);
                    }
                    Err(e) => m.fail(e, cx),
                }
                cx.notify();
            });
        });
    }

    pub fn open_workspace(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(w) = self.workspaces.iter().find(|w| w.id == id).cloned() {
            self.active_project = Some(w.project_root);
            self.select(w.threads.first().and_then(|t| Uuid::parse_str(t).ok()), cx);
            self.active_workspace = Some(id);
            if self.selected.is_none() {
                self.new_thread_open = true;
                if self.prefs.mode == "yolo" { self.prefs.mode = "plan".into(); }
            }
            self.prefs.temporary = w.inline;
            self.prefs.worktree = !w.inline;
            if w.inline { self.prefs.mode = "plan".into(); }
            self.refresh_review(cx);
            cx.notify();
        }
    }

    pub fn workspace_from_inline(&mut self, cx: &mut Context<Self>) {
        let source = self.selected.map(|id| id.to_string());
        self.new_thread(cx);
        self.source_thread = source;
        self.prefs.mode = "ask".into();
        self.toast(ToastKind::Info, "Your next message starts a thread with this conversation as context.");
        cx.notify();
    }

    pub fn new_workspace_thread(&mut self, cx: &mut Context<Self>) {
        self.new_thread_open = true;
        self.prefs.fast_mode = Some(false);
        if self.prefs.mode == "yolo" { self.prefs.mode = "plan".into(); }
        self.selected = None;
        cx.notify();
    }

    /// An intent choice for a new conversation, not a Git-mode switch.
    pub fn set_new_intent(&mut self, questions: bool, cx: &mut Context<Self>) {
        self.prefs.temporary = questions;
        self.prefs.worktree = !questions;
        if questions { self.prefs.mode = "plan".into(); }
        cx.notify();
    }

    pub fn inline_project(&mut self, root: String, cx: &mut Context<Self>) {
        if let Some(w) = self.workspaces.iter().find(|w| w.project_root == root && w.inline) {
            self.open_workspace(w.id.clone(), cx);
        } else {
            self.set_active_project(root, cx);
            self.new_thread_open = true;
            self.set_new_intent(true, cx);
            cx.notify();
        }
    }

    pub fn refresh_review(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active_workspace.clone() else { self.review = None; return; };
        if self.workspaces.iter().any(|w| w.id == id && w.archived_at.is_some()) { self.review = None; return; }
        self.review_loading = true;
        let state = svc(cx);
        let this = cx.entity().downgrade();
        let selected = id.clone();
        spawn_service(cx, async move { services::workspaces::review_workspace(&state, id).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                if m.active_workspace.as_deref() != Some(&selected) { return; }
                m.review_loading = false;
                match res { Ok(r) => m.review = Some(r), Err(e) => { m.review = None; m.last_error = Some(e); } }
                cx.notify();
            });
        });
    }

    pub fn run_workspace_action(&mut self, id: String, action: String, value: String, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move {
            if action == "archive" { services::workspaces::archive_workspace(&state, id).await.map(|_| "Thread archived; branch and conversations kept".into()) }
            else if action == "rename" { services::workspaces::rename_workspace(&state, id, value).await.map(|_| "Thread renamed".into()) }
            else { services::workspaces::workspace_action(&state, id, action, value).await }
        }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                match res { Ok(note) => m.toast(ToastKind::Success, note), Err(e) => m.fail(e, cx) }
                m.refresh_threads(cx);
                cx.notify();
            });
        });
    }

    pub fn fetch_project(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.active_project.clone() else { return; };
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::workspaces::fetch_project(root).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                match res { Ok(()) => m.toast(ToastKind::Success, "Project fetched"), Err(e) => m.fail(e, cx) }
                m.refresh_review(cx); m.refresh_project_status(false, cx); m.refresh_project_overview(cx); cx.notify();
            });
        });
    }

    // ── refresh ─────────────────────────────────────────────────────────

    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        self.refresh_threads(cx);
        self.refresh_projects(cx);
        self.refresh_services(cx);
        self.refresh_usage(cx);
        self.load_archived(cx);
        self.refresh_backends(cx);
        self.refresh_dev_server(cx);
        self.refresh_mcp_names(cx);
    }

    pub fn refresh_mcp_names(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::list_mcp_servers(&state).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    if let Ok(list) = res {
                        m.mcp_names = list.into_iter().filter(|s| s.enabled).map(|s| s.name).collect();
                        cx.notify();
                    }
                });
            },
        );
    }

    pub fn refresh_threads(&mut self, cx: &mut Context<Self>) {
        self.refresh_workspaces(cx);
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::list_threads(&state).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| match res {
                    Ok(list) => m.set_threads(list, cx),
                    Err(e) => m.fail(e, cx),
                });
            },
        );
    }

    pub fn refresh_projects(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::list_projects(&state).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    if let Ok(list) = res {
                        m.projects = list;
                        if m.active_project.is_none() {
                            m.active_project = m.projects.first().cloned();
                        }
                        cx.notify();
                    }
                });
            },
        );
    }

    pub fn refresh_services(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::backend_auth_status(&state).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    if let Ok(list) = res {
                        let auth_changed = list.iter().any(|a| m.auth.iter().find(|old| old.backend == a.backend).is_some_and(|old| old.logged_in != a.logged_in));
                        m.auth = list;
                        if auth_changed { m.refresh_backends(cx); }
                        cx.notify();
                    }
                });
            },
        );
    }

    pub fn refresh_backends(&mut self, cx: &mut Context<Self>) {
        if self.models_loading { return; }
        self.models_loading = true;
        cx.notify();
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::list_backends(&state).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    m.models_loading = false;
                    if let Ok(list) = res {
                        // Keep the preferred backend runnable.
                        if !list.iter().any(|b| b.id == m.prefs.backend && b.available) {
                            if let Some(b) = list.iter().find(|b| b.available) {
                                m.prefs.backend = b.id.clone();
                                m.prefs.model = None;
                            }
                        }
                        m.backends = list;
                    }
                    cx.notify();
                });
            },
        );
    }

    pub fn refresh_dev_server(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::dev_server_status(&state).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    if let Ok(s) = res {
                        let running = s.running;
                        m.dev_server = Some(s);
                        if running {
                            m.ensure_dev_poll(cx);
                        }
                        cx.notify();
                    }
                });
            },
        );
    }

    /// Services change rarely; a slow poll keeps the footer honest after a
    /// sign-in that happened in a terminal.
    fn start_service_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |weak, cx| {
            let mut tick: u32 = 0;
            loop {
                cx.background_executor().timer(Duration::from_secs(20)).await;
                tick += 1;
                // Usage limits move slowly and hit vendor APIs: every 2 minutes.
                let usage = tick.is_multiple_of(6);
                if weak
                    .update(cx, |m, cx| {
                        m.refresh_services(cx);
                        if usage {
                            m.refresh_usage(cx);
                            m.refresh_review(cx);
                        }
                        if tick.is_multiple_of(15) { m.refresh_project_status(true, cx); }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn refresh_usage(&mut self, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        spawn_service(cx, services::account_usage(), move |list, cx| {
            let _ = this.update(cx, |m, cx| {
                if m.usage != list {
                    m.usage = list;
                    cx.notify();
                }
            });
        });
    }

    pub fn usage_for(&self, backend: &str) -> Option<&bomb_core::usage::AccountUsage> {
        self.usage.iter().find(|u| u.backend == backend)
    }

    fn ensure_dev_poll(&mut self, cx: &mut Context<Self>) {
        if self.dev_poll.is_some() {
            return;
        }
        self.dev_poll = Some(cx.spawn(async move |weak, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let state = cx.update(|cx| svc(cx));
                let status = {
                    let (tx, rx) = async_channel::bounded(1);
                    let handle = cx.update(|cx| cx.global::<crate::runtime::Tokio>().0.clone());
                    handle.spawn(async move {
                        let _ = tx.send(services::dev_server_status(&state).await).await;
                    });
                    rx.recv().await
                };
                let keep = match status {
                    Ok(Ok(s)) => {
                        let running = s.running;
                        weak.update(cx, |m, cx| {
                            m.dev_server = Some(s);
                            if !running {
                                m.dev_poll = None;
                            }
                            cx.notify();
                        })
                        .map(|_| running)
                        .unwrap_or(false)
                    }
                    _ => false,
                };
                if !keep {
                    break;
                }
            }
        }));
    }

    fn set_threads(&mut self, list: Vec<ThreadDto>, cx: &mut Context<Self>) {
        let mut order = Vec::with_capacity(list.len());
        for dto in list {
            let Ok(id) = Uuid::parse_str(&dto.id) else {
                continue;
            };
            order.push(id);
            match self.threads.get(&id) {
                Some(entity) => entity.update(cx, |t, cx| {
                    t.meta = dto;
                    cx.notify();
                }),
                None => {
                    let entity = cx.new(|_| ThreadModel::new(dto));
                    self.threads.insert(id, entity);
                }
            }
        }
        self.threads.retain(|id, _| order.contains(id));
        self.thread_order = order;
        if let Some(sel) = self.selected {
            if !self.threads.contains_key(&sel) {
                self.selected = None;
            }
        }
        cx.notify();
    }

    fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        tracing::warn!(%message, "backend call failed");
        self.last_error = Some(message.clone());
        self.toast(ToastKind::Error, message);
        cx.notify();
    }

    pub fn toast(&mut self, kind: ToastKind, message: impl Into<String>) {
        self.toasts.push_back((kind, message.into()));
    }

    // ── queries ─────────────────────────────────────────────────────────

    /// Threads grouped by project root, in list order.
    pub fn groups(&self, cx: &App) -> Vec<ProjectGroup> {
        let mut groups: Vec<ProjectGroup> = Vec::new();
        for id in &self.thread_order {
            let Some(t) = self.threads.get(id) else { continue };
            let meta = &t.read(cx).meta;
            let root = meta
                .project_root
                .clone()
                .unwrap_or_else(|| meta.cwd.clone());
            match groups.iter_mut().find(|g| g.root == root) {
                Some(g) => g.threads.push(*id),
                None => groups.push(ProjectGroup {
                    name: project_name(&root),
                    root,
                    threads: vec![*id],
                }),
            }
        }
        for p in &self.projects {
            if !groups.iter().any(|g| &g.root == p) {
                groups.push(ProjectGroup {
                    name: project_name(p),
                    root: p.clone(),
                    threads: Vec::new(),
                });
            }
        }
        groups
    }

    pub fn selected_thread(&self) -> Option<Entity<ThreadModel>> {
        self.selected.and_then(|id| self.threads.get(&id).cloned())
    }

    pub fn backend_info(&self, id: &str) -> Option<&BackendInfo> {
        self.backends.iter().find(|b| b.id == id)
    }

    /// Model shown in the composer: explicit choice, else the backend default.
    pub fn effective_model(&self) -> String {
        self.prefs
            .model
            .clone()
            .or_else(|| self.backend_info(&self.prefs.backend).map(|b| b.default_model.clone()))
            .unwrap_or_default()
    }

    pub fn model_ready(&self) -> bool {
        let selected = self.effective_model();
        self.backend_info(&self.prefs.backend).is_some_and(|backend| backend.models.contains(&selected))
    }

    pub fn model_name(&self, backend: &str, model: &str) -> String {
        self.backend_info(backend).and_then(|b|b.model_names.get(model)).cloned()
            .unwrap_or_else(|| crate::views::brand::pretty_model(model))
    }

    // ── selection ───────────────────────────────────────────────────────

    pub fn select(&mut self, id: Option<Uuid>, cx: &mut Context<Self>) {
        if self.selected == id {
            return;
        }
        self.selected = id;
        self.prefs.fast_mode = Some(false);
        self.new_thread_open = false;
        self.review = None;
        self.active_workspace = id.and_then(|id| self.workspaces.iter().find(|w| w.threads.contains(&id.to_string())).map(|w| w.id.clone()));
        if let Some(w) = self.active_workspace.as_deref().and_then(|id| self.workspaces.iter().find(|w| w.id == id)) {
            self.prefs.temporary = w.inline;
            self.prefs.worktree = !w.inline;
        }
        self.refresh_review(cx);
        if let Some(id) = id {
            if let Some(t) = self.threads.get(&id) {
                let meta = t.read(cx).meta.clone();
                self.active_project = Some(meta.project_root.clone().unwrap_or(meta.cwd.clone()));
                // The composer follows the thread's own backend/model/mode,
                // but never a model id the backend no longer offers.
                self.prefs.backend = meta.backend.clone();
                let known = self
                    .backend_info(&meta.backend)
                    .map(|b| b.models.contains(&meta.model) || b.default_model == meta.model)
                    .unwrap_or(false);
                self.prefs.model = if meta.model.is_empty() || !known { None } else { Some(meta.model.clone()) };
                if let Some(mode) = meta.approval_mode.clone() {
                    self.prefs.mode = mode;
                }
            }
            self.hydrate(id, cx);
            let state = svc(cx);
            spawn_service(
                cx,
                async move { services::explainer_focus(&state, Some(id.to_string())).await },
                |_, _| {},
            );
        }
        cx.notify();
    }

    /// Deselect: the composer starts a fresh thread in the active project.
    pub fn new_thread(&mut self, cx: &mut Context<Self>) {
        self.prefs.fast_mode = Some(false);
        self.new_thread_open = true;
        if self.prefs.mode == "yolo" { self.prefs.mode = "plan".into(); }
        self.source_thread = None;
        self.active_workspace = None;
        self.prefs.temporary = false;
        self.prefs.worktree = true;
        self.selected = None;
        // Fresh thread → backend default model, never a stale id.
        self.prefs.model = None;
        cx.notify();
    }

    /// A folder-free chat still needs a private working directory for tools.
    pub fn temporary_chat(&mut self, cx: &mut Context<Self>) {
        let Some(home) = std::env::var_os("HOME") else { return; };
        let base = std::path::PathBuf::from(home).join(".bombcode/chats");
        let weak = cx.entity().downgrade();
        spawn_service(cx, async move {
            services::scratch::create(&base).await
        }, move |result, cx| {
            let _ = weak.update(cx, |m, cx| match result {
                Ok(path) => {
                    m.new_thread(cx);
                    m.active_project = Some(path.to_string_lossy().into_owned());
                    m.prefs.temporary = false;
                    m.prefs.mode = "plan".into();
                    m.prefs.worktree = true;
                    cx.notify();
                }
                Err(error) => { m.toast(ToastKind::Error, format!("Could not start temporary chat: {error}")); cx.notify(); }
            });
        });
    }

    /// Return to the welcome screen without removing projects or conversations.
    pub fn open_home(&mut self, cx: &mut Context<Self>) {
        self.new_thread(cx);
        self.active_project = None;
        self.review = None;
        self.review_open = false;
    }

    fn hydrate(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let Some(entity) = self.threads.get(&id).cloned() else {
            return;
        };
        let needs = {
            let t = entity.read(cx);
            !t.hydrated && !t.loading
        };
        if !needs {
            return;
        }
        // A thread that already streamed in this session is the truth; the
        // saved copy can only be older. Never replace live entries with it.
        if !entity.read(cx).thread.entries.is_empty() {
            entity.update(cx, |t, _| t.hydrated = true);
            return;
        }
        entity.update(cx, |t, _| t.loading = true);
        let state = svc(cx);
        let weak = entity.downgrade();
        spawn_service(
            cx,
            async move { services::get_session_transcript(&state, id.to_string()).await },
            move |res, cx| {
                let _ = weak.update(cx, |t, cx| {
                    t.loading = false;
                    t.hydrated = true;
                    match res {
                        Ok(rows) => {
                            tracing::debug!(%id, rows = rows.len(), live = t.thread.entries.len(), "thread hydrated");
                            if t.thread.entries.is_empty() {
                                t.thread.hydrate(&rows);
                                t.after_hydrate(cx);
                            }
                        }
                        Err(e) => tracing::warn!(%id, error = %e, "transcript load failed"),
                    }
                    cx.notify();
                });
            },
        );
    }

    // ── events ──────────────────────────────────────────────────────────

    pub fn apply_events(&mut self, batch: Vec<ControlEvent>, cx: &mut Context<Self>) {
        let mut need_refresh = false;
        for ev in batch {
            match &ev {
                ControlEvent::SessionCreated { session_id, .. } => {
                    if !self.threads.contains_key(session_id) {
                        need_refresh = true;
                    }
                }
                ControlEvent::SessionStatusChanged { .. }
                | ControlEvent::SessionCancelled { .. }
                | ControlEvent::SessionCompleted { .. } => need_refresh = true,
                ControlEvent::Error {
                    session_id: None,
                    message,
                    ..
                } => {
                    self.last_error = Some(message.clone());
                    self.toast(ToastKind::Error, message.clone());
                    cx.notify();
                }
                ControlEvent::McpChanged { .. } => self.refresh_mcp_names(cx),
                ControlEvent::Raw { payload, session_id: Some(_) }
                    if payload.get("channel").and_then(|c| c.as_str()) == Some("thread") =>
                {
                    need_refresh = true;
                }
                _ => {}
            }
            if let Some(sid) = session_of(&ev) {
                if let Some(t) = self.threads.get(&sid) {
                    t.update(cx, |t, cx| {
                        t.apply(&ev, cx);
                    });
                } else {
                    debug!(%sid, "event for unknown thread");
                    need_refresh = true;
                }
            }
        }
        if need_refresh {
            self.refresh_threads(cx);
        }
    }

    // ── prompts / sessions ──────────────────────────────────────────────

    /// Send to the selected thread, or start a new one in the active project.
    pub fn send_prompt(&mut self, text: String, images: Vec<ImageInput>, cx: &mut Context<Self>) {
        if std::env::var("BOMB_SMOKE").ok().as_deref() != Some("1") && !self.model_ready() {
            self.toast(ToastKind::Warning, "Choose a model from the provider's current list. Open the model picker and refresh if needed.");
            cx.notify(); return;
        }
        if self.active_workspace.as_deref().is_some_and(|id| self.workspaces.iter().any(|w| w.id == id && w.archived_at.is_some())) {
            self.fail("This thread is archived. Start a new thread to make changes.".into(), cx); return;
        }
        let mut prefs = self.prefs.clone();
        let model = self.effective_model();
        prefs.model = (!model.is_empty()).then_some(model);
        let attachments: Vec<ImageAttachment> = images
            .iter()
            .map(|i| ImageAttachment {
                mime_type: i.mime_type.clone(),
                data: i.data.clone(),
                name: i.name.clone(),
            })
            .collect();
        match self.selected_thread() {
            Some(t) => {
                let id = t.read(cx).id();
                t.update(cx, |t, cx| {
                    let ch = t.thread.note_prompt(&text, attachments, std::time::Instant::now());
                    t.absorb(&ch, cx);
                });
                let state = svc(cx);
                let this = cx.entity().downgrade();
                let weak = t.downgrade();
                spawn_service(
                    cx,
                    async move {
                        services::send_prompt(
                            &state,
                            id,
                            text,
                            Some(prefs.backend),
                            prefs.model,
                            Some(prefs.mode),
                            None,
                            None,
                            Some(images),
                            prefs.fast_mode,
                            Some(prefs.effort),
                        )
                        .await
                    },
                    move |res, cx| {
                        if let Err(e) = res {
                            let _ = weak.update(cx, |t, cx| {
                                let ch = t.thread.note_failure(&format!("send failed: {e}"), std::time::Instant::now());
                                t.absorb(&ch, cx);
                            });
                            let _ = this.update(cx, |m, cx| m.fail(e, cx));
                        }
                    },
                );
            }
            None => {
                let Some(cwd) = self.active_project.clone() else {
                    self.toast(ToastKind::Warning, "Open a project first (⌘O).");
                    cx.notify();
                    return;
                };
                self.starting = true;
                cx.notify();
                let state = svc(cx);
                let this = cx.entity().downgrade();
                let backend = grok_config::Backend::from_key(&prefs.backend).unwrap_or_default();
                let approval_mode = match prefs.mode.as_str() {
                    "plan" => Some(ApprovalMode::Plan),
                    "ask" => Some(ApprovalMode::Ask),
                    "auto" => Some(ApprovalMode::Auto),
                    "yolo" => Some(ApprovalMode::Yolo),
                    _ => None,
                };
                // Spawn with the model the composer shows, never the stale
                // config default ("grok-4").
                let model = Some(self.effective_model()).filter(|m| !m.trim().is_empty());
                let opts = SpawnOptions {
                    backend,
                    model: model.clone(),
                    approval_mode,
                    isolate_worktree: prefs.worktree && !prefs.temporary,
                    workspace_id: self.active_workspace.clone(),
                    source_thread: self.source_thread.take(),
                    prompt: Some(text.clone()),
                    project_root: Some(cwd.clone()),
                    mcp_server_names: prefs.mcp_servers.clone(),
                    effort: Some(prefs.effort.clone()),
                    ..Default::default()
                };
                // Two steps so the thread is selected (and the prompt visible)
                // the moment it exists, even if the send then fails.
                let state2 = state.clone();
                let text2 = text.clone();
                let images2 = images.clone();
                let mut prefs2 = prefs.clone();
                prefs2.model = model;
                spawn_service(
                    cx,
                    async move { services::start_session(&state, cwd, opts).await },
                    move |res, cx| {
                        let _ = this.update(cx, |m, cx| match res {
                            Ok(started) => {
                                if let Ok(id) = Uuid::parse_str(&started.id) {
                                    m.selected = Some(id);
                                    if !m.threads.contains_key(&id) {
                                        let dto = ThreadDto {
                                            models_used: Vec::new(),
                                            id: started.id.clone(),
                                            cwd: String::new(),
                                            mode: "acp".into(),
                                            model: prefs2.model.clone().unwrap_or_default(),
                                            backend: prefs2.backend.clone(),
                                            status: "starting".into(),
                                            live: true,
                                            message_count: 0,
                                            created_at: chrono::Utc::now().to_rfc3339(),
                                            updated_at: chrono::Utc::now().to_rfc3339(),
                                            worktree: None,
                                            mcp_servers: Vec::new(),
                                            label: None,
                                            approval_mode: Some(prefs2.mode.clone()),
                                            project_root: m.active_project.clone(),
                                            brain_mode: None,
                                        };
                                        let entity = cx.new(|_| ThreadModel::new(dto));
                                        entity.update(cx, |t, _| { t.hydrated = true; });
                                        m.threads.insert(id, entity);
                                        m.thread_order.insert(0, id);
                                    }
                                    if let Some(t) = m.threads.get(&id) {
                                        let atts: Vec<ImageAttachment> = images2
                                            .iter()
                                            .map(|i| ImageAttachment { mime_type: i.mime_type.clone(), data: i.data.clone(), name: i.name.clone() })
                                            .collect();
                                        t.update(cx, |t, cx| {
                                            let ch = t.thread.note_prompt(&text2, atts, std::time::Instant::now());
                                            t.absorb(&ch, cx);
                                        });
                                    }
                                    cx.notify();
                                    let sid = started.id.clone();
                                    let weak = m.threads.get(&id).map(|t| t.downgrade());
                                    let this = cx.entity().downgrade();
                                    spawn_service(
                                        cx,
                                        async move {
                                            // The ACP handshake is still in flight right
                                            // after start_session; sending now is refused.
                                            services::wait_until_idle(
                                                &state2,
                                                &sid,
                                                std::time::Duration::from_secs(90),
                                            )
                                            .await?;
                                            services::send_prompt(
                                                &state2,
                                                sid,
                                                text2,
                                                Some(prefs2.backend),
                                                prefs2.model,
                                                Some(prefs2.mode),
                                                None,
                                                None,
                                                Some(images2),
                                                prefs2.fast_mode,
                                                Some(prefs2.effort),
                                            )
                                            .await
                                        },
                                        move |res, cx| {
                                            let _ = this.update(cx, |m, cx| {
                                                m.starting = false;
                                                if let Err(e) = res {
                                                    if let Some(w) = &weak {
                                                        let _ = w.update(cx, |t, cx| {
                                                            let ch = t.thread.note_failure(&format!("send failed: {e}"), std::time::Instant::now());
                                                            t.absorb(&ch, cx);
                                                        });
                                                    }
                                                    m.fail(e, cx);
                                                }
                                                m.refresh_threads(cx);
                                            });
                                        },
                                    );
                                } else {
                                    m.starting = false;
                                    m.refresh_threads(cx);
                                }
                            }
                            Err(e) => {
                                m.starting = false;
                                m.fail(e, cx);
                            }
                        });
                    },
                );
            }
        }
    }

    /// Pin a reply into project memory.
    pub fn remember(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(id) = self.selected else { return };
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::remember(&state, id.to_string(), text).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    match res {
                        Ok(_) => m.toast(ToastKind::Success, "Saved to project memory"),
                        Err(e) => m.fail(e, cx),
                    }
                    cx.notify();
                });
            },
        );
    }

    /// Plan handoff: switch to `backend`/`model`, drop to auto approvals, and
    /// ask the (possibly different) agent to implement the plan.
    pub fn code_plan_with(&mut self, backend: &str, model: Option<String>, cx: &mut Context<Self>) {
        if self.active_workspace.as_deref().is_some_and(|id| self.workspaces.iter().any(|w| w.id == id && w.inline)) { self.workspace_from_inline(cx); }
        self.set_backend(backend, model, cx);
        self.set_mode("auto", cx);
        self.send_prompt("Implement the plan above. Work through it step by step and report when done.".into(), Vec::new(), cx);
    }

    pub fn cancel_selected(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected else { return };
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::cancel_session(&state, id.to_string()).await },
            move |res, cx| {
                if let Err(e) = res {
                    let _ = this.update(cx, |m, cx| m.fail(e, cx));
                }
            },
        );
    }

    pub fn set_mode(&mut self, mode: &str, cx: &mut Context<Self>) {
        if mode != "plan" && (self.prefs.temporary || self.active_workspace.as_deref().is_some_and(|id| self.workspaces.iter().any(|w| w.id == id && w.inline))) {
            self.toast(ToastKind::Info, "This conversation is for questions. Choose Make changes to continue in a thread.");
            cx.notify(); return;
        }
        let previous = self.prefs.mode.clone();
        self.prefs.mode = mode.to_string();
        if let Some(id) = self.selected {
            if let Some(t) = self.threads.get(&id) {
                t.update(cx, |t, cx| { t.meta.approval_mode = Some(mode.to_string()); cx.notify(); });
            }
            let state = svc(cx);
            let requested = mode.to_string();
            let mode = requested.clone();
            let this = cx.entity().downgrade();
            spawn_service(cx, async move { services::set_approval_mode(&state, id.to_string(), mode).await }, move |result, cx| {
                let _ = this.update(cx, |m, cx| {
                    if let Err(error) = result {
                        if m.selected == Some(id) && m.prefs.mode == requested { m.prefs.mode = previous.clone(); }
                        if let Some(t) = m.threads.get(&id) {
                            t.update(cx, |t, cx| {
                                if t.meta.approval_mode.as_deref() == Some(&requested) { t.meta.approval_mode = Some(previous); }
                                cx.notify();
                            });
                        }
                        m.fail(error, cx);
                    }
                    cx.notify();
                });
            });
        }
        cx.notify();
    }

    pub fn provider_mode_options(&self, cx: &App) -> serde_json::Value {
        self.selected.and_then(|id| self.threads.get(&id)).filter(|t| t.read(cx).meta.model == self.effective_model()).map(|t| &t.read(cx).provider_modes)
            .filter(|v| v.get("backend").and_then(|v|v.as_str()) == Some(self.prefs.backend.as_str()))
            .cloned().unwrap_or_default()
    }

    pub fn cycle_mode(&mut self, cx: &mut Context<Self>) {
        let options = self.provider_mode_options(cx);
        let mut next = next_shortcut_mode(&self.prefs.mode);
        for _ in 0..3 {
            if crate::views::composer::provider_mode_presentation(next, &options).is_some() { break; }
            next = next_shortcut_mode(next);
        }
        self.set_mode(next, cx);
    }

    pub fn toggle_mcp_pref(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(i) = self.prefs.mcp_servers.iter().position(|n| n == name) {
            self.prefs.mcp_servers.remove(i);
        } else {
            self.prefs.mcp_servers.push(name.to_string());
        }
        cx.notify();
    }

    pub fn set_effort(&mut self, effort: &str, cx: &mut Context<Self>) {
        self.prefs.effort = effort.to_string();
        // Live threads whose agent exposes an `effort` option take it now.
        if let Some(t) = self.selected_thread() {
            let (id, live) = {
                let t = t.read(cx);
                (t.id(), t.meta.live)
            };
            if live {
                let state = svc(cx);
                let e = effort.to_string();
                let this = cx.entity().downgrade();
                spawn_service(
                    cx,
                    async move { services::set_session_effort(&state, id, e).await },
                    move |res, cx| {
                        if let Ok(true) = res {
                            let _ = this.update(cx, |m, cx| {
                                m.toast(ToastKind::Info, "Reasoning effort updated for this thread");
                                cx.notify();
                            });
                        }
                    },
                );
            }
        }
        cx.notify();
    }

    pub fn set_backend(&mut self, backend: &str, model: Option<String>, cx: &mut Context<Self>) {
        if self.prefs.backend != backend || self.prefs.model != model { self.prefs.fast_mode = Some(false); }
        self.prefs.backend = backend.to_string();
        self.prefs.model = model;
        let (levels, _) = crate::views::brand::effort_levels(backend);
        if !levels.contains(&self.prefs.effort.as_str()) {
            self.prefs.effort = levels.iter().find(|l| **l == "high").or(levels.last()).map(|s| s.to_string()).unwrap_or_default();
        }
        cx.notify();
    }

    pub fn load_archived(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::kv_get(&state, ARCHIVED_KEY).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    if let Ok(Some(raw)) = res {
                        m.archived = raw
                            .split(',')
                            .filter_map(|s| Uuid::parse_str(s.trim()).ok())
                            .collect();
                        cx.notify();
                    }
                });
            },
        );
    }

    fn save_archived(&self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let raw = self.archived.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",");
        spawn_service(cx, async move { services::kv_set(&state, ARCHIVED_KEY, &raw).await }, |_, _| {});
    }

    /// Hide a thread without deleting anything.
    pub fn archive_thread(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.archived.insert(id);
        if self.selected == Some(id) {
            self.selected = None;
        }
        self.save_archived(cx);
        cx.notify();
    }

    pub fn unarchive_thread(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.archived.remove(&id);
        self.save_archived(cx);
        cx.notify();
    }

    pub fn remove_thread(&mut self, id: Uuid, cx: &mut Context<Self>) {
        tracing::info!(%id, "delete: removing thread");
        self.archived.remove(&id);
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::remove_session(&state, id.to_string(), Some(true)).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    match res {
                        Ok(()) => {
                            tracing::info!(%id, "delete: removed");
                            if m.selected == Some(id) {
                                m.selected = None;
                            }
                            m.threads.remove(&id);
                            m.thread_order.retain(|x| *x != id);
                        }
                        Err(e) => m.fail(e, cx),
                    }
                    m.refresh_threads(cx);
                });
            },
        );
    }

    pub fn rename_thread(&mut self, id: Uuid, label: String, cx: &mut Context<Self>) {
        if let Some(t) = self.threads.get(&id) {
            t.update(cx, |t, cx| {
                t.thread.label = Some(label.clone());
                cx.notify();
            });
        }
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::rename_thread(&state, id.to_string(), label).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| match res {
                    Ok(()) => m.refresh_threads(cx),
                    Err(e) => m.fail(e, cx),
                });
            },
        );
    }

    pub fn land_thread(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.select(Some(id), cx);
        self.review_open = true;
        self.refresh_review(cx);
    }

    pub fn sync_thread(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if let Some(w) = self.workspaces.iter().find(|w| w.threads.contains(&id.to_string())) {
            self.run_workspace_action(w.id.clone(), "update".into(), String::new(), cx);
        }
    }

    // ── projects ────────────────────────────────────────────────────────

    pub fn open_project(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |weak, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = weak.update(cx, |m, cx| m.add_project(path, cx));
        })
        .detach();
    }

    pub fn add_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let p = path.display().to_string();
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::add_project(&state, p.clone()).await.map(|list| (p, list)) },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| match res {
                    Ok((p, list)) => {
                        m.projects = list;
                        m.active_project = Some(p);
                        m.selected = None;
                        cx.notify();
                    }
                    Err(e) => m.fail(e, cx),
                });
            },
        );
    }

    pub fn set_active_project(&mut self, root: String, cx: &mut Context<Self>) {
        self.new_thread_open = false;
        if self.prefs.mode == "yolo" { self.prefs.mode = "plan".into(); }
        self.source_thread = None;
        self.active_project = Some(root);
        self.overview_branch = None;
        self.refresh_project_overview(cx);
        self.refresh_project_status(false, cx);
        self.active_workspace = None;
        self.review = None;
        self.review_open = false;
        self.selected = None;
        self.prefs.temporary = false;
        self.prefs.worktree = true;
        cx.notify();
    }

    pub fn reveal_project(&mut self, cx: &mut Context<Self>) {
        let cwd = self.active_project.clone();
        let sid = self.selected.map(|s| s.to_string());
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::reveal_project(&state, cwd, sid).await },
            move |res, cx| {
                if let Err(e) = res {
                    let _ = this.update(cx, |m, cx| m.fail(e, cx));
                }
            },
        );
    }

    // ── dev server ──────────────────────────────────────────────────────

    pub fn dev_server_toggle(&mut self, cx: &mut Context<Self>) {
        let running = self.dev_server.as_ref().map(|s| s.running).unwrap_or(false);
        let state = svc(cx);
        let this = cx.entity().downgrade();
        if running {
            spawn_service(
                cx,
                async move { services::stop_dev_server(&state).await },
                move |res, cx| {
                    let _ = this.update(cx, |m, cx| match res {
                        Ok(s) => {
                            m.dev_server = Some(s);
                            m.toast(ToastKind::Info, "Dev server stopped");
                            cx.notify();
                        }
                        Err(e) => m.fail(e, cx),
                    });
                },
            );
        } else {
            let cwd = self.active_project.clone();
            let sid = self.selected.map(|s| s.to_string());
            spawn_service(
                cx,
                async move { services::start_dev_server(&state, cwd, sid, Some(true)).await },
                move |res, cx| {
                    let _ = this.update(cx, |m, cx| match res {
                        Ok(s) => {
                            let msg = s.url.clone().map(|u| format!("Dev server at {u}")).unwrap_or(s.message.clone());
                            m.dev_server = Some(s);
                            m.ensure_dev_poll(cx);
                            m.toast(ToastKind::Success, msg);
                            cx.notify();
                        }
                        Err(e) => m.fail(e, cx),
                    });
                },
            );
        }
    }

    pub fn dev_server_open(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::open_dev_server(&state).await },
            move |res, cx| {
                if let Err(e) = res {
                    let _ = this.update(cx, |m, cx| m.fail(e, cx));
                }
            },
        );
    }

    // ── sign-in ─────────────────────────────────────────────────────────

    /// Grok signs in inside the app (device code); other CLIs open a terminal.
    pub fn sign_in(&mut self, backend: &str, cx: &mut Context<Self>) {
        if backend == "grok" {
            self.start_grok_login(cx);
            return;
        }
        let b = backend.to_string();
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::open_backend_login(b, false).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| match res {
                    Ok(()) => m.toast(ToastKind::Info, "Sign in from the terminal window that just opened."),
                    Err(e) => m.fail(e, cx),
                });
            },
        );
    }

    pub fn sign_out(&mut self, backend: &str, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        let b = backend.to_string();
        spawn_service(
            cx,
            async move {
                if b == "grok" {
                    services::logout_grok(&state).await.map(|_| ())
                } else {
                    services::open_backend_login(b, true).await
                }
            },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    if let Err(e) = res {
                        m.fail(e, cx);
                    }
                    m.refresh_services(cx);
                });
            },
        );
    }

    pub fn start_grok_login(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        self.login = None;
        self.login_starting = true;
        cx.notify();
        spawn_service(
            cx,
            async move { services::start_grok_login(&state).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    m.login_starting = false;
                    match res {
                        Ok(s) => {
                            m.login = Some(s);
                            m.ensure_login_poll(cx);
                            cx.notify();
                        }
                        Err(e) => {
                            m.login = None;
                            m.fail(e, cx);
                        }
                    }
                });
            },
        );
    }

    fn ensure_login_poll(&mut self, cx: &mut Context<Self>) {
        if self.login_poll.is_some() {
            return;
        }
        self.login_poll = Some(cx.spawn(async move |weak, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let state = cx.update(|cx| svc(cx));
                let handle = cx.update(|cx| cx.global::<crate::runtime::Tokio>().0.clone());
                let (tx, rx) = async_channel::bounded(1);
                handle.spawn(async move {
                    let _ = tx.send(services::grok_login_status(&state).await).await;
                });
                let keep = match rx.recv().await {
                    Ok(Ok(s)) => {
                        let done = matches!(s.phase, LoginPhase::Completed | LoginPhase::Failed) || !s.active;
                        weak.update(cx, |m, cx| {
                            if m.login.is_some() || m.login_starting {
                                m.login = Some(s);
                            }
                            if done {
                                m.login_poll = None;
                                m.refresh_services(cx);
                            }
                            cx.notify();
                        })
                        .map(|_| !done)
                        .unwrap_or(false)
                    }
                    _ => false,
                };
                if !keep {
                    break;
                }
            }
        }));
    }

    pub fn submit_login_code(&mut self, code: String, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::submit_grok_login_code(&state, code).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| match res {
                    Ok(s) => {
                        m.login = Some(s);
                        cx.notify();
                    }
                    Err(e) => m.fail(e, cx),
                });
            },
        );
    }

    pub fn open_login_url(&mut self, cx: &mut Context<Self>) {
        if let Some(url) = self.login.as_ref().and_then(|l| l.login_url.clone()) {
            cx.open_url(&url);
        }
    }

    pub fn cancel_login(&mut self, cx: &mut Context<Self>) {
        self.login = None;
        self.login_poll = None;
        let state = svc(cx);
        spawn_service(cx, async move { services::cancel_grok_login(&state).await }, |_, _| {});
        cx.notify();
    }

    pub fn close_login(&mut self, cx: &mut Context<Self>) {
        self.login = None;
        self.login_poll = None;
        self.refresh_services(cx);
        cx.notify();
    }

    pub fn new_mock_session(&mut self, cx: &mut Context<Self>) {
        let cwd = self
            .active_project
            .clone()
            .or_else(|| std::env::var("HOME").ok())
            .unwrap_or_else(|| "/tmp".into());
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::start_mock_session(&state, cwd).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| match res {
                    Ok(r) => {
                        if let Ok(id) = Uuid::parse_str(&r.id) {
                            m.selected = Some(id);
                        }
                        m.refresh_threads(cx);
                    }
                    Err(e) => m.fail(e, cx),
                });
            },
        );
    }
}

fn session_of(ev: &ControlEvent) -> Option<Uuid> {
    match ev {
        ControlEvent::SessionCreated { session_id, .. }
        | ControlEvent::SessionStatusChanged { session_id, .. }
        | ControlEvent::SessionCancelled { session_id, .. }
        | ControlEvent::SessionCompleted { session_id, .. }
        | ControlEvent::ToolCall { session_id, .. }
        | ControlEvent::PlanUpdate { session_id, .. }
        | ControlEvent::AgentMessage { session_id, .. }
        | ControlEvent::ApprovalRequired { session_id, .. }
        | ControlEvent::ApprovalResolved { session_id, .. } => Some(*session_id),
        ControlEvent::Error { session_id, .. } | ControlEvent::Raw { session_id, .. } => *session_id,
        _ => None,
    }
}

pub fn project_name(root: &str) -> String {
    if root.contains("/.bombcode/chats/") { return "Temporary chat".into(); }
    std::path::Path::new(root)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| root.to_string())
}

fn next_shortcut_mode(current: &str) -> &'static str {
    match current { "plan" => "ask", "ask" => "auto", _ => "plan" }
}

#[cfg(test)]
mod mode_shortcut_tests {
    #[test]
    fn shortcut_never_enters_full_access() {
        assert_eq!(super::next_shortcut_mode("plan"), "ask");
        assert_eq!(super::next_shortcut_mode("ask"), "auto");
        assert_eq!(super::next_shortcut_mode("auto"), "plan");
        assert_eq!(super::next_shortcut_mode("yolo"), "plan");
    }
}
