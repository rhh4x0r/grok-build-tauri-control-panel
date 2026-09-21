//! One handle for "the core that owns this project": the in-process one, or a
//! paired server. Methods mirror `bomb_core::services`; the server variant
//! sends the same call through `bomb_core::rpc` and rewrites paths so server
//! folders stay namespaced (`bomb-server://…`) on this side.

use std::sync::Arc;

use bomb_core::services::{self, project_overview::ProjectOverview, workspaces::{ProjectStatus, WorkspaceReview}, BackendInfo, ImageInput};
use bomb_core::{AppState, ControlEvent};
use grok_cli_wrapper::BackendAuth;
use grok_control_core::SpawnOptions;
use grok_persistence::{ThreadDto, TranscriptEntry, WorkspaceRecord};
use serde_json::{json, Value};

use crate::remote::RemoteCore;

#[derive(Clone)]
pub enum Core {
    Local(Arc<AppState>),
    Remote(Arc<RemoteCore>),
}

/// A thread's saved history plus what is still waiting on the user.
pub struct ThreadSnapshot {
    pub rows: Vec<TranscriptEntry>,
    pub pending_approvals: Vec<ControlEvent>,
}

const NOT_ON_SERVER: &str = "That isn’t available for projects on a server yet.";

impl Core {
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote(_))
    }

    fn adopt_thread(remote: &RemoteCore, mut t: ThreadDto) -> ThreadDto {
        t.project_root = Some(remote.root(t.project_root.as_deref().unwrap_or(&t.cwd)));
        t.cwd = remote.root(&t.cwd);
        t.worktree = t.worktree.map(|w| remote.root(&w));
        t
    }

    fn adopt_workspace(remote: &RemoteCore, mut w: WorkspaceRecord) -> WorkspaceRecord {
        w.project_root = remote.root(&w.project_root);
        w.path = remote.root(&w.path);
        w
    }

    pub async fn list_threads(&self) -> Result<Vec<ThreadDto>, String> {
        match self {
            Self::Local(state) => services::list_threads(state).await,
            Self::Remote(r) => Ok(r.call::<Vec<ThreadDto>>("list_threads", Value::Null).await?.into_iter().map(|t| Self::adopt_thread(r, t)).collect()),
        }
    }

    pub async fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>, String> {
        match self {
            Self::Local(state) => services::workspaces::list_workspaces(state).await,
            Self::Remote(r) => Ok(r.call::<Vec<WorkspaceRecord>>("list_workspaces", Value::Null).await?.into_iter().map(|w| Self::adopt_workspace(r, w)).collect()),
        }
    }

    pub async fn list_projects(&self) -> Result<Vec<String>, String> {
        match self {
            Self::Local(state) => services::list_projects(state).await,
            Self::Remote(r) => Ok(r.call::<Vec<String>>("list_projects", Value::Null).await?.iter().map(|p| r.root(p)).collect()),
        }
    }

    pub async fn snapshot(&self, id: String) -> Result<ThreadSnapshot, String> {
        match self {
            Self::Local(state) => {
                let snap = state.journal.snapshot(uuid::Uuid::parse_str(&id).map_err(|e| e.to_string())?)?;
                Ok(ThreadSnapshot { rows: snap.rows, pending_approvals: snap.pending_approvals })
            }
            Self::Remote(r) => {
                let v = r.request("snapshot", json!({ "id": id })).await?;
                Ok(ThreadSnapshot {
                    rows: serde_json::from_value(v["rows"].clone()).map_err(|e| e.to_string())?,
                    pending_approvals: serde_json::from_value(v["pending_approvals"].clone()).unwrap_or_default(),
                })
            }
        }
    }

    pub async fn start_session(&self, root: String, mut opts: SpawnOptions) -> Result<String, String> {
        match self {
            Self::Local(state) => Ok(services::start_session(state, root, opts).await?.id),
            Self::Remote(r) => {
                opts.project_root = opts.project_root.map(|p| r.path_of(&p).to_string());
                Ok(r.call::<services::SessionIdResponse>("start_session", json!({ "cwd": r.path_of(&root), "opts": opts })).await?.id)
            }
        }
    }

    pub async fn wait_until_idle(&self, id: &str, timeout: std::time::Duration) -> Result<(), String> {
        match self {
            Self::Local(state) => services::wait_until_idle(state, id, timeout).await,
            Self::Remote(r) => r.request("wait_until_idle", json!({ "id": id, "seconds": timeout.as_secs() })).await.map(|_| ()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_prompt(&self, id: String, prompt: String, backend: Option<String>, model: Option<String>, approval_mode: Option<String>, plan_mode: Option<bool>, always_approve: Option<bool>, images: Option<Vec<ImageInput>>, fast_mode: Option<bool>, effort: Option<String>) -> Result<(), String> {
        match self {
            Self::Local(state) => services::send_prompt(state, id, prompt, backend, model, approval_mode, plan_mode, always_approve, images, fast_mode, effort).await,
            Self::Remote(r) => r.request("send_prompt", json!({ "id": id, "prompt": prompt, "backend": backend, "model": model, "approval_mode": approval_mode, "plan_mode": plan_mode, "always_approve": always_approve, "images": images, "fast_mode": fast_mode, "effort": effort })).await.map(|_| ()),
        }
    }

    pub async fn cancel_session(&self, id: String) -> Result<(), String> {
        match self {
            Self::Local(state) => services::cancel_session(state, id).await,
            Self::Remote(r) => r.request("cancel_session", json!({ "id": id })).await.map(|_| ()),
        }
    }

    pub async fn respond_approval(&self, id: String, request_id: String, option_id: Option<String>) -> Result<(), String> {
        match self {
            Self::Local(state) => services::respond_approval(state, id, request_id, option_id).await,
            // Another Mac may have answered first; that is not an error.
            Self::Remote(r) => r.request("respond_approval", json!({ "id": id, "request_id": request_id, "option_id": option_id })).await.map(|_| ()),
        }
    }

    pub async fn add_session_allow_rule(&self, id: String, pattern: String) -> Result<(), String> {
        match self {
            Self::Local(state) => services::add_session_allow_rule(state, id, pattern).await,
            Self::Remote(r) => r.request("add_session_allow_rule", json!({ "id": id, "pattern": pattern })).await.map(|_| ()),
        }
    }

    pub async fn set_approval_mode(&self, id: String, mode: String) -> Result<(), String> {
        match self {
            Self::Local(state) => services::set_approval_mode(state, id, mode).await,
            Self::Remote(r) => r.request("set_approval_mode", json!({ "id": id, "mode": mode })).await.map(|_| ()),
        }
    }

    pub async fn set_session_effort(&self, id: String, effort: String) -> Result<bool, String> {
        match self {
            Self::Local(state) => services::set_session_effort(state, id, effort).await,
            Self::Remote(r) => r.call("set_session_effort", json!({ "id": id, "effort": effort })).await,
        }
    }

    pub async fn rename_thread(&self, id: String, label: String) -> Result<(), String> {
        match self {
            Self::Local(state) => services::rename_thread(state, id, label).await,
            Self::Remote(r) => r.request("rename_thread", json!({ "id": id, "label": label })).await.map(|_| ()),
        }
    }

    pub async fn remove_session(&self, id: String, remove_worktree: Option<bool>) -> Result<(), String> {
        match self {
            Self::Local(state) => services::remove_session(state, id, remove_worktree).await,
            Self::Remote(r) => r.request("remove_session", json!({ "id": id, "remove_worktree": remove_worktree })).await.map(|_| ()),
        }
    }

    pub async fn review_workspace(&self, id: String) -> Result<WorkspaceReview, String> {
        match self {
            Self::Local(state) => services::workspaces::review_workspace(state, id).await,
            Self::Remote(r) => r.call("review_workspace", json!({ "id": id })).await,
        }
    }

    pub async fn project_overview(&self, root: &str) -> Result<ProjectOverview, String> {
        match self {
            Self::Local(state) => services::project_overview::load_for_project(state, root).await,
            Self::Remote(r) => r.call("project_overview", json!({ "root": r.path_of(root) })).await,
        }
    }

    pub async fn project_status(&self, root: &str) -> Result<ProjectStatus, String> {
        match self {
            Self::Local(_) => services::workspaces::project_status(root).await,
            Self::Remote(r) => r.call("project_status", json!({ "root": r.path_of(root) })).await,
        }
    }

    pub async fn merge_request(&self, id: String) -> Result<String, String> {
        match self {
            Self::Local(state) => services::workspaces::merge_request(state, id).await,
            Self::Remote(r) => r.call("merge_request", json!({ "id": id })).await,
        }
    }

    pub async fn is_merged(&self, id: &str) -> Result<bool, String> {
        match self {
            Self::Local(state) => services::workspaces::is_merged(state, id).await,
            Self::Remote(r) => r.call("is_merged", json!({ "id": id })).await,
        }
    }

    pub async fn close_feature(&self, id: String) -> Result<String, String> {
        match self {
            Self::Local(state) => services::workspaces::close_feature(state, id).await,
            Self::Remote(r) => r.call("close_feature", json!({ "id": id })).await,
        }
    }

    /// Git actions from the Changes panel. A server offers only the two the thread flow needs.
    pub async fn workspace_action(&self, id: String, action: String, value: String) -> Result<String, String> {
        match self {
            Self::Local(state) => match action.as_str() {
                "archive" => services::workspaces::archive_workspace(state, id).await.map(|_| "Thread archived; branch and conversations kept".into()),
                "rename" => services::workspaces::rename_workspace(state, id, value).await.map(|_| "Thread renamed".into()),
                _ => services::workspaces::workspace_action(state, id, action, value).await,
            },
            Self::Remote(r) => match action.as_str() {
                "checkpoint" => r.call("checkpoint", json!({ "id": id, "message": value })).await,
                "update" => r.call("update_from_base", json!({ "id": id })).await,
                _ => Err(NOT_ON_SERVER.into()),
            },
        }
    }

    pub async fn branches(&self, root: &str) -> Result<services::git_ui::BranchChoices, String> {
        match self {
            Self::Local(_) => services::git_ui::branches(root).await,
            Self::Remote(r) => r.call("branches", json!({ "root": r.path_of(root) })).await,
        }
    }

    pub async fn thread_setup_check(&self, root: &str) -> Result<services::thread_setup::Readiness, String> {
        match self {
            Self::Local(_) => services::thread_setup::check(root).await,
            Self::Remote(r) => r.call("thread_setup_check", json!({ "root": r.path_of(root) })).await,
        }
    }

    pub async fn thread_setup_initialize(&self, root: &str) -> Result<(), String> {
        match self {
            Self::Local(_) => services::thread_setup::initialize(root).await,
            Self::Remote(r) => r.request("thread_setup_initialize", json!({ "root": r.path_of(root) })).await.map(|_| ()),
        }
    }

    pub async fn list_backends(&self) -> Result<Vec<BackendInfo>, String> {
        match self {
            Self::Local(state) => services::list_backends(state).await,
            Self::Remote(r) => r.call("list_backends", Value::Null).await,
        }
    }

    pub async fn backend_auth_status(&self) -> Result<Vec<BackendAuth>, String> {
        match self {
            Self::Local(state) => services::backend_auth_status(state).await,
            Self::Remote(r) => r.call("backend_auth_status", Value::Null).await,
        }
    }
}
