//! Top-level UI state: projects, threads, services, selection.

use std::collections::HashMap;

use bomb_core::services;
use bomb_core::ControlEvent;
use gpui_kit::*;
use grok_cli_wrapper::BackendAuth;
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
    pub collapsed: bool,
}

pub struct AppModel {
    pub projects: Vec<String>,
    pub active_project: Option<String>,
    pub thread_order: Vec<Uuid>,
    pub threads: HashMap<Uuid, Entity<ThreadModel>>,
    pub selected: Option<Uuid>,
    pub auth: Vec<BackendAuth>,
    pub last_error: Option<String>,
    collapsed: HashMap<String, bool>,
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
            last_error: None,
            collapsed: HashMap::new(),
        };
        this.refresh_all(cx);
        this
    }

    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        self.refresh_threads(cx);
        self.refresh_projects(cx);
        self.refresh_services(cx);
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
        self.last_error = Some(message);
        cx.notify();
    }

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
                    collapsed: self.collapsed.get(&root).copied().unwrap_or(false),
                    root,
                    threads: vec![*id],
                }),
            }
        }
        // Known projects without threads still get a (empty) group.
        for p in &self.projects {
            if !groups.iter().any(|g| &g.root == p) {
                groups.push(ProjectGroup {
                    name: project_name(p),
                    collapsed: self.collapsed.get(p).copied().unwrap_or(false),
                    root: p.clone(),
                    threads: Vec::new(),
                });
            }
        }
        groups
    }

    pub fn toggle_group(&mut self, root: &str, cx: &mut Context<Self>) {
        let v = self.collapsed.entry(root.to_string()).or_insert(false);
        *v = !*v;
        cx.notify();
    }

    pub fn select(&mut self, id: Option<Uuid>, cx: &mut Context<Self>) {
        if self.selected == id {
            return;
        }
        self.selected = id;
        if let Some(id) = id {
            if let Some(t) = self.threads.get(&id) {
                let meta = &t.read(cx).meta;
                self.active_project = Some(
                    meta.project_root
                        .clone()
                        .unwrap_or_else(|| meta.cwd.clone()),
                );
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

    /// Load the persisted transcript for a thread once.
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
                        // A live stream may already have more than the DB.
                        if t.thread.entries.len() <= rows.len() {
                            t.thread.hydrate(&rows);
                            t.after_hydrate(cx);
                        }
                    }
                    cx.notify();
                });
            },
        );
    }

    pub fn selected_thread(&self) -> Option<Entity<ThreadModel>> {
        self.selected.and_then(|id| self.threads.get(&id).cloned())
    }

    /// Fold a batch of backend events into the thread models.
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
                    cx.notify();
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
        ControlEvent::Error { session_id, .. } | ControlEvent::Raw { session_id, .. } => {
            *session_id
        }
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
