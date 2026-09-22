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
        // Save unsaved edits in every idle thread of a project, so a sync carries the real work. Busy threads are skipped.
        "checkpoint_project" => {
            let root: String = arg(&p, "root")?;
            let message: String = arg::<Option<String>>(&p, "message")?.unwrap_or_else(|| "Checkpoint".into());
            let mut saved = 0usize;
            for w in state.persistence.list_workspaces().map_err(|e| e.to_string())? {
                if w.project_root != root || w.inline || w.shared_checkout || w.archived_at.is_some() { continue; }
                if workspaces::ensure_idle(state, &w).is_err() { continue; }
                let path = std::path::Path::new(&w.path);
                if !path.exists() { continue; }
                if state.worktrees.commit_all(path, &message).await.unwrap_or(false) { saved += 1; }
            }
            Ok(json!(saved))
        }
        "checkpoint" => out(workspaces::workspace_action(state, arg(&p, "id")?, "checkpoint".into(), arg::<Option<String>>(&p, "message")?.unwrap_or_else(|| "Checkpoint".into())).await?),
        "update_from_base" => out(workspaces::workspace_action(state, arg(&p, "id")?, "update".into(), String::new()).await?),

        "branches" => { let root: String = arg(&p, "root")?; out(services::git_ui::branches(&root).await?) }
        "thread_setup_check" => { let root: String = arg(&p, "root")?; out(services::thread_setup::check(&root).await?) }
        "thread_setup_initialize" => { let root: String = arg(&p, "root")?; services::thread_setup::initialize(&root).await?; Ok(Value::Null) }
        // A new empty Git project in this person's projects folder.
        "create_project" => {
            let name: String = arg(&p, "name")?;
            let slug = project_slug(&name)?;
            let path = server_projects_dir(state).join(&slug);
            if path.exists() { return Err(format!("There is already a project named {slug} on this server.")); }
            std::fs::create_dir_all(server_projects_dir(state)).map_err(|e| format!("Could not create the projects folder: {e}"))?;
            services::thread_setup::create(&path).await?;
            services::add_project(state, path.display().to_string()).await?;
            Ok(json!(path.display().to_string()))
        }

        // Git history between this core's copy of a project and a Mac's. `bundle_path` is filled in by the
        // server from an uploaded stream; a client can never name a file on this machine.
        "branch_tips" => { let root = own_project(state, &p).await?; out(services::project_sync::branch_tips(&root).await?) }
        "import_bundle" => {
            let root = own_project(state, &p).await?;
            let bundle: String = arg(&p, "bundle_path")?;
            out(services::project_sync::import_bundle(&root, std::path::Path::new(&bundle)).await?)
        }
        // A file attached from a Mac to a thread here. The server places it in the person's folder and
        // returns the path the agent should be told about; the name is sanitised, the file is a stream.
        "store_attachment" => {
            let bundle: String = arg(&p, "bundle_path")?;
            let name: String = arg(&p, "name")?;
            let safe: String = std::path::Path::new(&name).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into())
                .chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ' | '(' | ')') { c } else { '_' }).collect();
            let dir = state.paths.home_dir.join("attachments");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let target = dir.join(format!("{}-{}", &Uuid::new_v4().to_string()[..8], safe.trim()));
            std::fs::rename(&bundle, &target).or_else(|_| std::fs::copy(&bundle, &target).map(|_| ())).map_err(|e| e.to_string())?;
            Ok(json!(target.display().to_string()))
        }
        "import_project" => {
            let (name, bundle): (String, String) = (arg(&p, "name")?, arg(&p, "bundle_path")?);
            let path = server_projects_dir(state).join(project_slug(&name)?);
            services::project_sync::init_from_bundle(&path, std::path::Path::new(&bundle)).await?;
            services::add_project(state, path.display().to_string()).await?;
            Ok(json!(path.display().to_string()))
        }
        "export_bundle" => {
            let root = own_project(state, &p).await?;
            let have: Vec<String> = arg::<Option<Vec<String>>>(&p, "have")?.unwrap_or_default();
            let dir = state.paths.panel_dir.join("transfer");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let file = dir.join(format!("{}.bundle", Uuid::new_v4()));
            match services::project_sync::create_bundle(&root, &have, &file).await? {
                Some(path) => Ok(json!({ "bundle_path": path.display().to_string() })),
                None => Ok(json!({ "empty": true })),
            }
        }
        "clone_project" => {
            let url: String = arg(&p, "url")?;
            let name = arg::<Option<String>>(&p, "name")?.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| url.trim_end_matches('/').trim_end_matches(".git").rsplit(['/', ':']).next().unwrap_or("project").to_string());
            let path = server_projects_dir(state).join(project_slug(&name)?);
            services::project_sync::clone_url(url.trim(), &path).await?;
            services::add_project(state, path.display().to_string()).await?;
            Ok(json!(path.display().to_string()))
        }

        // Looking at a server project's files from a Mac. Paths are confined to the person's own folders.
        "list_dir" => {
            let dir = own_path(state, &p).await?;
            let show_ignored = arg::<Option<bool>>(&p, "show_hidden")?.unwrap_or(false);
            let mut entries = Vec::new();
            let mut reader = tokio::fs::read_dir(&dir).await.map_err(|e| e.to_string())?;
            while let Some(entry) = reader.next_entry().await.map_err(|e| e.to_string())? {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name == ".git" || (!show_ignored && name.starts_with('.')) { continue; }
                let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                entries.push(json!({ "name": name, "dir": is_dir }));
                if entries.len() >= 5000 { break; }
            }
            Ok(json!(entries))
        }
        "read_file" => {
            let file = own_path(state, &p).await?;
            let meta = tokio::fs::metadata(&file).await.map_err(|e| e.to_string())?;
            if !meta.is_file() { return Err("That is not a file.".into()); }
            if meta.len() > 2 * 1024 * 1024 { return Err("That file is too large to show here.".into()); }
            let bytes = tokio::fs::read(&file).await.map_err(|e| e.to_string())?;
            match String::from_utf8(bytes) { Ok(text) => Ok(json!({ "text": text })), Err(_) => Ok(json!({ "binary": true })) }
        }
        "start_dev_server" => { let root = own_path(state, &p).await?; out(services::start_dev_server(state, Some(root.display().to_string()), None, Some(false)).await?) }
        "stop_dev_server" => out(services::stop_dev_server(state).await?),
        "dev_server_status" => out(services::dev_server_status(state).await?),

        // What this core can run
        "list_backends" => out(services::list_backends(state).await?),
        "backend_auth_status" => out(services::backend_auth_status(state).await?),
        // The command that signs a provider in on this machine; the app runs it in a terminal here so the
        // person sees the link or code. The Mac's own login files are never sent.
        "login_command" => { let backend: String = arg(&p, "backend")?; grok_cli_wrapper::backend_auth::login_command(&backend).map(Value::from).ok_or_else(|| format!("{backend} is not installed on this server.")) }

        other => Err(format!("unknown method `{other}`")),
    }
}

/// Where a person's projects live on a server: `~/projects`.
pub fn server_projects_dir(state: &AppState) -> std::path::PathBuf {
    state.paths.home_dir.join("projects")
}

/// A folder-safe project name.
fn project_slug(name: &str) -> Result<String, String> {
    let slug: String = name.trim().chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' }).collect::<String>().trim_matches('-').to_string();
    if slug.is_empty() || slug.len() > 64 { return Err("Give the project a short name using letters, digits or dashes.".into()); }
    Ok(slug)
}

/// The `root` parameter, but only if it is one of this core's registered projects.
async fn own_project(state: &AppState, params: &Value) -> Result<String, String> {
    let root: String = arg(params, "root")?;
    if services::list_projects(state).await?.contains(&root) { Ok(root) } else { Err("That is not one of your projects on this server.".into()) }
}

/// `root` (a project or thread folder of this core) joined with the optional relative `path`, confined to that folder.
pub async fn own_path(state: &AppState, params: &Value) -> Result<std::path::PathBuf, String> {
    let root: String = arg(params, "root")?;
    let mut allowed = services::list_projects(state).await?;
    allowed.extend(state.persistence.list_workspaces().map_err(|e| e.to_string())?.into_iter().map(|w| w.path));
    if !allowed.contains(&root) { return Err("That is not one of your folders on this server.".into()); }
    let base = std::fs::canonicalize(&root).map_err(|e| e.to_string())?;
    let relative: String = arg::<Option<String>>(params, "path")?.unwrap_or_default();
    let relative = std::path::Path::new(&relative);
    if relative.is_absolute() || relative.components().any(|c| matches!(c, std::path::Component::ParentDir)) { return Err("Paths must stay inside the project.".into()); }
    // Resolve symlinks before checking, so a link cannot lead outside.
    let full = std::fs::canonicalize(base.join(relative)).map_err(|e| e.to_string())?;
    if !full.starts_with(&base) { return Err("Paths must stay inside the project.".into()); }
    Ok(full)
}
