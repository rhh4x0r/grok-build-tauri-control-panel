//! Top-level UI state: projects, threads, services, selection, composer
//! preferences, and every user action that talks to the backend.

use std::collections::{HashMap, VecDeque};
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
            temporary: false,
        }
    }
}

pub const APPROVAL_CYCLE: [&str; 4] = ["plan", "ask", "auto", "yolo"];

pub struct AppModel {
    pub projects: Vec<String>,
    pub active_project: Option<String>,
    pub thread_order: Vec<Uuid>,
    pub threads: HashMap<Uuid, Entity<ThreadModel>>,
    pub selected: Option<Uuid>,
    pub auth: Vec<BackendAuth>,
    /// Account usage limits (5h / weekly) per backend, refreshed slowly.
    pub usage: Vec<bomb_core::usage::AccountUsage>,
    pub backends: Vec<BackendInfo>,
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
    login_poll: Option<Task<()>>,
    dev_poll: Option<Task<()>>,
}

impl AppModel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            projects: Vec::new(),
            active_project: None,
            thread_order: Vec::new(),
            threads: HashMap::new(),
            selected: None,
            auth: Vec::new(),
            usage: Vec::new(),
            backends: Vec::new(),
            prefs: ComposerPrefs::default(),
            dev_server: None,
            mcp_names: Vec::new(),
            login: None,
            login_starting: false,
            last_error: None,
            toasts: VecDeque::new(),
            starting: false,
            login_poll: None,
            dev_poll: None,
        };
        this.refresh_all(cx);
        this.start_service_poll(cx);
        this
    }

    // ── refresh ─────────────────────────────────────────────────────────

    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        self.refresh_threads(cx);
        self.refresh_projects(cx);
        self.refresh_services(cx);
        self.refresh_usage(cx);
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
                        m.auth = list;
                        cx.notify();
                    }
                });
            },
        );
    }

    pub fn refresh_backends(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::list_backends(&state).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    if let Ok(list) = res {
                        // Keep the preferred backend runnable.
                        if !list.iter().any(|b| b.id == m.prefs.backend && b.available) {
                            if let Some(b) = list.iter().find(|b| b.available) {
                                m.prefs.backend = b.id.clone();
                                m.prefs.model = None;
                            }
                        }
                        m.backends = list;
                        cx.notify();
                    }
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
                        }
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
        self.usage.iter().find(|u| u.backend == backend && !u.windows.is_empty())
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

    // ── selection ───────────────────────────────────────────────────────

    pub fn select(&mut self, id: Option<Uuid>, cx: &mut Context<Self>) {
        if self.selected == id {
            return;
        }
        self.selected = id;
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
        self.selected = None;
        // Fresh thread → backend default model, never a stale id.
        self.prefs.model = None;
        cx.notify();
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
                    if let Ok(rows) = res {
                        if t.thread.entries.is_empty() {
                            t.thread.hydrate(&rows);
                            t.after_hydrate(cx);
                        }
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
        let prefs = self.prefs.clone();
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
                        )
                        .await
                    },
                    move |res, cx| {
                        if let Err(e) = res {
                            let _ = weak.update(cx, |t, cx| {
                                let ch = t.thread.note_system(&format!("send failed: {e}"));
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
                                            )
                                            .await
                                        },
                                        move |res, cx| {
                                            let _ = this.update(cx, |m, cx| {
                                                m.starting = false;
                                                if let Err(e) = res {
                                                    if let Some(w) = &weak {
                                                        let _ = w.update(cx, |t, cx| {
                                                            let ch = t.thread.note_system(&format!("send failed: {e}"));
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
        self.prefs.mode = mode.to_string();
        if let Some(id) = self.selected {
            let state = svc(cx);
            let mode = mode.to_string();
            spawn_service(
                cx,
                async move { services::set_approval_mode(&state, id.to_string(), mode).await },
                |_, _| {},
            );
        }
        cx.notify();
    }

    pub fn cycle_mode(&mut self, cx: &mut Context<Self>) {
        let i = APPROVAL_CYCLE
            .iter()
            .position(|m| *m == self.prefs.mode)
            .unwrap_or(0);
        let next = APPROVAL_CYCLE[(i + 1) % APPROVAL_CYCLE.len()];
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
        self.prefs.backend = backend.to_string();
        self.prefs.model = model;
        let (levels, _) = crate::views::brand::effort_levels(backend);
        if !levels.contains(&self.prefs.effort.as_str()) {
            self.prefs.effort = levels.iter().find(|l| **l == "high").or(levels.last()).map(|s| s.to_string()).unwrap_or_default();
        }
        cx.notify();
    }

    pub fn remove_thread(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::remove_session(&state, id.to_string(), Some(true)).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    match res {
                        Ok(()) => {
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
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::land_thread(&state, id.to_string()).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    match res {
                        Ok(r) if r.status == "landed" => m.toast(
                            ToastKind::Success,
                            format!("Landed {} into {} ({} files)", r.branch, r.target_branch, r.files.len()),
                        ),
                        Ok(r) => m.toast(
                            ToastKind::Warning,
                            format!("Conflicts with {}: run Sync so the agent can resolve them", r.target_branch),
                        ),
                        Err(e) => m.fail(e, cx),
                    }
                    m.refresh_threads(cx);
                });
            },
        );
    }

    pub fn sync_thread(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::sync_thread(&state, id.to_string()).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    match res {
                        Ok(r) if r.status == "synced" || r.status == "landed" => {
                            m.toast(ToastKind::Success, format!("Synced {} from {}", r.branch, r.target_branch))
                        }
                        Ok(r) => m.toast(
                            ToastKind::Warning,
                            format!("Sync hit conflicts in {} file(s); ask the agent to resolve them", r.files.len()),
                        ),
                        Err(e) => m.fail(e, cx),
                    }
                    m.refresh_threads(cx);
                });
            },
        );
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
        self.active_project = Some(root);
        self.selected = None;
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
    std::path::Path::new(root)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| root.to_string())
}
