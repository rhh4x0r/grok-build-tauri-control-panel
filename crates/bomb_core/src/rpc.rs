//! The core's remote-callable surface.
//!
//! [`dispatch`] maps a method name and JSON parameters onto the same
//! `services::*` functions the desktop UI calls in-process. Only a curated set
//! is reachable: nothing that edits settings, MCP or memory, and no raw
//! key-value access (the settings table holds credentials).

use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::services::{self, project_overview, workspaces};
use crate::AppState;

fn arg<T: DeserializeOwned>(params: &Value, name: &str) -> Result<T, String> {
    serde_json::from_value(params.get(name).cloned().unwrap_or(Value::Null)).map_err(|e| format!("bad `{name}`: {e}"))
}

fn out<T: serde::Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

/// Run one request. `origin` names the calling client and is stamped on the prompts it sends.
pub async fn dispatch(state: &AppState, origin: &str, method: &str, p: Value) -> Result<Value, String> {
    match method {
        "ping" => Ok(json!({ "seq": state.journal.seq() })),

        // Threads
        "list_threads" => out(services::list_threads(state).await?),
        "snapshot" => {
            let id: String = arg(&p, "id")?;
            let snap = state.journal.snapshot(Uuid::parse_str(&id).map_err(|e| e.to_string())?)?;
            Ok(json!({ "as_of": snap.as_of, "rows": snap.rows, "pending_approvals": snap.pending_approvals }))
        }
        "start_session" => out(services::start_session(state, arg(&p, "cwd")?, arg(&p, "opts")?).await?),
        "start_mock_session" => out(services::start_mock_session(state, arg(&p, "cwd")?).await?),
        "wait_until_idle" => {
            let id: String = arg(&p, "id")?;
            let seconds: u64 = arg::<Option<u64>>(&p, "seconds")?.unwrap_or(60).min(600);
            services::wait_until_idle(state, &id, std::time::Duration::from_secs(seconds)).await?;
            Ok(Value::Null)
        }
        "send_prompt" => {
            let id: String = arg(&p, "id")?;
            let prompt: String = arg(&p, "prompt")?;
            services::send_prompt(
                state, id.clone(), prompt.clone(), arg(&p, "backend")?, arg(&p, "model")?, arg(&p, "approval_mode")?,
                arg(&p, "plan_mode")?, arg(&p, "always_approve")?, arg(&p, "images")?, arg(&p, "fast_mode")?, arg(&p, "effort")?,
            ).await?;
            // Other devices on this thread learn about the prompt; the sender already shows it.
            if let Ok(session_id) = Uuid::parse_str(&id) {
                state.event_bus.emit(grok_events::ControlEvent::UserMessage { session_id, text: prompt, origin: origin.to_string(), at: chrono::Utc::now() });
            }
            Ok(Value::Null)
        }
        "cancel_session" => { services::cancel_session(state, arg(&p, "id")?).await?; Ok(Value::Null) }
        "respond_approval" => {
            let (id, request_id): (String, String) = (arg(&p, "id")?, arg(&p, "request_id")?);
            // First answer wins; a second device answering the same request is not an error.
            match services::respond_approval(state, id, request_id, arg(&p, "option_id")?).await {
                Ok(()) => Ok(json!({ "already_resolved": false })),
                Err(e) if e.contains("no pending permission request") => Ok(json!({ "already_resolved": true })),
                Err(e) => Err(e),
            }
        }
        "set_approval_mode" => { services::set_approval_mode(state, arg(&p, "id")?, arg(&p, "mode")?).await?; Ok(Value::Null) }
        "set_plan_mode" => { services::set_plan_mode(state, arg(&p, "id")?, arg(&p, "enabled")?).await?; Ok(Value::Null) }
        "set_always_approve" => { services::set_always_approve(state, arg(&p, "id")?, arg(&p, "enabled")?).await?; Ok(Value::Null) }
        "add_session_allow_rule" => { services::add_session_allow_rule(state, arg(&p, "id")?, arg(&p, "pattern")?).await?; Ok(Value::Null) }
        "set_session_effort" => out(services::set_session_effort(state, arg(&p, "id")?, arg(&p, "effort")?).await?),
        "rename_thread" => { services::rename_thread(state, arg(&p, "id")?, arg(&p, "label")?).await?; Ok(Value::Null) }
        "remove_session" => { services::remove_session(state, arg(&p, "id")?, arg(&p, "remove_worktree")?).await?; Ok(Value::Null) }
        "agent_supports_images" => out(services::agent_supports_images(state, arg(&p, "id")?).await?),

        // Projects and their threads' standing
        "list_projects" => out(services::list_projects(state).await?),
        "add_project" => out(services::add_project(state, arg(&p, "path")?).await?),
        "remove_project" => out(services::remove_project(state, arg(&p, "path")?).await?),
        "list_workspaces" => out(workspaces::list_workspaces(state).await?),
        "review_workspace" => out(workspaces::review_workspace(state, arg(&p, "id")?).await?),
        "project_overview" => { let root: String = arg(&p, "root")?; out(project_overview::load_for_project(state, &root).await?) }
        "project_status" => { let root: String = arg(&p, "root")?; out(workspaces::project_status(&root).await?) }
        "merge_request" => out(workspaces::merge_request(state, arg(&p, "id")?).await?),
        "is_merged" => { let id: String = arg(&p, "id")?; out(workspaces::is_merged(state, &id).await?) }
        "close_feature" => out(workspaces::close_feature(state, arg(&p, "id")?).await?),
        "checkpoint" => out(workspaces::workspace_action(state, arg(&p, "id")?, "checkpoint".into(), arg::<Option<String>>(&p, "message")?.unwrap_or_else(|| "Checkpoint".into())).await?),
        "update_from_base" => out(workspaces::workspace_action(state, arg(&p, "id")?, "update".into(), String::new()).await?),

        "branches" => { let root: String = arg(&p, "root")?; out(services::git_ui::branches(&root).await?) }
        "thread_setup_check" => { let root: String = arg(&p, "root")?; out(services::thread_setup::check(&root).await?) }
        "thread_setup_initialize" => { let root: String = arg(&p, "root")?; services::thread_setup::initialize(&root).await?; Ok(Value::Null) }
        // A new empty Git project in this person's projects folder.
        "create_project" => {
            let name: String = arg(&p, "name")?;
            let slug: String = name.trim().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' }).collect::<String>().trim_matches('-').to_string();
            if slug.is_empty() || slug.len() > 64 { return Err("Give the project a short name using letters, digits or dashes.".into()); }
            let path = server_projects_dir(state).join(&slug);
            if path.exists() { return Err(format!("There is already a project named {slug} on this server.")); }
            std::fs::create_dir_all(server_projects_dir(state)).map_err(|e| format!("Could not create the projects folder: {e}"))?;
            services::thread_setup::create(&path).await?;
            services::add_project(state, path.display().to_string()).await?;
            Ok(json!(path.display().to_string()))
        }

        // What this core can run
        "list_backends" => out(services::list_backends(state).await?),
        "backend_auth_status" => out(services::backend_auth_status(state).await?),

        other => Err(format!("unknown method `{other}`")),
    }
}

/// Where a person's projects live on a server: `~/projects`.
pub fn server_projects_dir(state: &AppState) -> std::path::PathBuf {
    state.paths.home_dir.join("projects")
}
