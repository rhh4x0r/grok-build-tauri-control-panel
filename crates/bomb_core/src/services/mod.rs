//! Service layer: every former Tauri command as a plain async fn over `&AppState`.

pub mod project_overview;
pub mod thread_setup;
pub mod prompt_sources;
pub mod workspaces;
pub mod git_ui;

use std::path::PathBuf;

use chrono::Utc;
use serde::Serialize;

use uuid::Uuid;

use grok_config::{DiscoveryReport, GrokConfig};
use grok_control_core::{AgentHandleSnapshot, SpawnOptions};
use grok_diff::{DiffCapture, DiffEngine, DiffSummary};
use grok_events::ControlEvent;
use grok_extensions::ExtensionEntry;
use grok_mcp::{
    AddMcpRequest, DoctorReport, McpCatalogEntry, McpCredential, McpServerConfigExt, McpToolInfo,
    UpdateMcpRequest,
};
use grok_memory::MemoryEntry;
use grok_permissions::{
    builtin_presets, PermissionController, PermissionDecision, PermissionPreset,
};
use grok_persistence::{SessionRecord, ThreadDto, TranscriptEntry};
use grok_scheduler::{ScheduleKind, ScheduledJob};
use grok_worktree::{CreateWorktreeRequest, WorktreeInfo};

use crate::state::AppState;

fn err(e: impl ToString) -> String {
    e.to_string()
}

// ── Phase 0: Discovery & Config ──────────────────────────────────────────

pub async fn discover_environment() -> Result<DiscoveryReport, String> {
    grok_config::discover_environment().map_err(err)
}

pub async fn get_config(state: &AppState) -> Result<GrokConfig, String> {
    Ok(state.config.read().await.clone())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendInfo {
    pub id: String,
    pub display_name: String,
    pub available: bool,
    /// "binary:/abs/path" | "npx" | null when unavailable.
    pub via: Option<String>,
    /// Why the backend is unavailable, when it is.
    pub reason: Option<String>,
    pub default_model: String,
    pub models: Vec<String>,
    pub model_names: std::collections::HashMap<String, String>,
    pub model_descriptions: std::collections::HashMap<String, String>,
    pub model_error: Option<String>,
    pub commands: Option<serde_json::Value>,
    pub supports_headless: bool,
}

pub async fn list_backends(state: &AppState) -> Result<Vec<BackendInfo>, String> {
    let cfg = state.config.read().await.clone();
    let (grok, claude, codex) = tokio::join!(
        model_catalog::discover(state, grok_config::Backend::Grok, &cfg),
        model_catalog::discover(state, grok_config::Backend::Claude, &cfg),
        model_catalog::discover(state, grok_config::Backend::Codex, &cfg),
    );
    Ok(vec![grok, claude, codex])
}

pub async fn save_config(state: &AppState, config: GrokConfig) -> Result<(), String> {
    {
        let mut cfg = state.config.write().await;
        *cfg = config;
        cfg.save(&state.paths.config_file).map_err(err)?;
    }
    Ok(())
}

pub async fn capture_baseline(
    state: &AppState,
) -> Result<grok_cli_wrapper::BaselineSnapshot, String> {
    Ok(state.grok_cli.capture_baseline().await)
}

// ── Auth / Grok login ────────────────────────────────────────────────────

pub async fn get_auth_status() -> Result<grok_cli_wrapper::AuthStatus, String> {
    Ok(grok_cli_wrapper::GrokCli::auth_status())
}

/// Sign-in state for every backend: which services can actually run right now.
pub async fn backend_auth_status(
    state: &AppState,
) -> Result<Vec<grok_cli_wrapper::BackendAuth>, String> {
    let cfg = state.config.read().await.clone();
    Ok(grok_cli_wrapper::backend_auth::all(&cfg).await)
}

/// Hand a backend's sign-in (or sign-out) off to a real terminal window.
///
/// `claude auth login` and `codex login` drive a browser flow and expect a TTY,
/// so we cannot run them headless the way we drive grok's device-code flow.
/// The panel polls `backend_auth_status` afterwards to notice the result.
pub async fn open_backend_login(backend: String, logout: bool) -> Result<(), String> {
    let cmd = if logout {
        grok_cli_wrapper::backend_auth::logout_command(&backend)
    } else {
        grok_cli_wrapper::backend_auth::login_command(&backend)
    }
    .ok_or_else(|| format!("no way to sign in to {backend}: install its CLI, or npx"))?;

    spawn_in_terminal(&cmd).map_err(|e| format!("could not open a terminal: {e}"))
}

/// Open `cmd` in the platform's terminal. The command string is built by
/// `backend_auth` from a fixed table, never from user input.
fn spawn_in_terminal(cmd: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        // `osascript` keeps the window open and focused so the user can see the
        // browser prompt and any error the CLI prints.
        let script = format!(
            r#"tell application "Terminal"
                 activate
                 do script "{cmd}"
               end tell"#
        );
        std::process::Command::new("osascript")
            .args(["-e", &script])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = cmd;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "terminal hand-off is only wired up for macOS; run the command yourself",
        ))
    }
}

/// Start interactive login (device-code). Returns immediately with URL + confirm code.
pub async fn start_grok_login(
    state: &AppState,
) -> Result<grok_cli_wrapper::LoginSessionState, String> {
    state.login.start_device_login().await.map_err(err)
}

/// Fallback OAuth browser login start.
pub async fn start_grok_login_oauth(
    state: &AppState,
) -> Result<grok_cli_wrapper::LoginSessionState, String> {
    state.login.start_oauth_login().await.map_err(err)
}

/// Poll login session (phase, confirm code, logged-in status).
pub async fn grok_login_status(
    state: &AppState,
) -> Result<grok_cli_wrapper::LoginSessionState, String> {
    Ok(state.login.state().await)
}

/// Paste a verification code from the browser into the running login process.
pub async fn submit_grok_login_code(
    state: &AppState,
    code: String,
) -> Result<grok_cli_wrapper::LoginSessionState, String> {
    state.login.submit_code(&code).await.map_err(err)
}

pub async fn open_grok_login_url(state: &AppState) -> Result<Option<String>, String> {
    state.login.open_login_url().await.map_err(err)
}

pub async fn cancel_grok_login(state: &AppState) -> Result<(), String> {
    state.login.cancel().await;
    Ok(())
}

pub async fn logout_grok(state: &AppState) -> Result<grok_cli_wrapper::AuthStatus, String> {
    state.login.cancel().await;
    state.grok_cli.logout().await.map_err(err)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub grok_binary: String,
    pub grok_binary_exists: bool,
    pub grok_version: Option<String>,
    pub home_dir: String,
    pub config_path: String,
    pub worktrees_dir: String,
    pub default_cwd: String,
    pub session_count: usize,
    pub mcp_count: usize,
    pub xai_api_key_present: bool,
    pub ready: bool,
    pub message: String,
}

pub async fn get_runtime_status(state: &AppState) -> Result<RuntimeStatus, String> {
    let binary = state.grok_cli.grok_path.clone();
    let exists = binary.is_file();
    let version = if exists {
        state.grok_cli.version().await.ok()
    } else {
        None
    };
    let default_cwd = std::env::var("HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::current_dir())
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    // Prefer last used cwd from persistence.
    let default_cwd = state
        .persistence
        .get_kv("last_cwd")
        .ok()
        .flatten()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or(default_cwd);

    let mcp_count = state.mcp.list().await.len();
    let session_count = state.registry.session_count();
    let xai = std::env::var("XAI_API_KEY")
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let auth = grok_cli_wrapper::GrokCli::auth_status();

    let (ready, message) = if !exists {
        (
            false,
            "Grok Build CLI not found. Install it, then restart the panel.".into(),
        )
    } else if version.is_none() {
        (
            false,
            format!(
                "Found {} but `grok version` failed. Check permissions.",
                binary.display()
            ),
        )
    } else if !auth.logged_in && !xai {
        (false, "Not signed in — use Log in with Grok.".into())
    } else {
        let who = auth.email.clone().unwrap_or_else(|| "Grok".into());
        (
            true,
            format!("Ready · {who} · {}", version.as_deref().unwrap_or("?")),
        )
    };

    Ok(RuntimeStatus {
        grok_binary: binary.display().to_string(),
        grok_binary_exists: exists,
        grok_version: version,
        home_dir: state.paths.home_dir.display().to_string(),
        config_path: state.paths.config_file.display().to_string(),
        worktrees_dir: state.paths.worktrees_dir.display().to_string(),
        default_cwd: default_cwd.display().to_string(),
        session_count,
        mcp_count,
        xai_api_key_present: xai,
        ready,
        message,
    })
}

pub async fn set_last_cwd(state: &AppState, cwd: String) -> Result<(), String> {
    let path = PathBuf::from(&cwd);
    if !path.is_absolute() || !path.is_dir() {
        return Err("cwd must be an absolute existing directory".into());
    }
    state.persistence.set_kv("last_cwd", &cwd).map_err(err)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFolderResult {
    pub path: String,
    pub name: String,
    pub created: bool,
}

/// Create a new project folder under a parent directory (default: ~/Projects or home).
pub async fn create_project_folder(
    state: &AppState,
    name: String,
    parent: Option<String>,
) -> Result<CreateFolderResult, String> {
    let slug = sanitize_folder_name(&name)?;
    let parent_dir = resolve_projects_parent(parent)?;
    std::fs::create_dir_all(&parent_dir).map_err(err)?;
    let path = parent_dir.join(&slug);
    let created = if path.exists() {
        if !path.is_dir() {
            return Err(format!(
                "path exists and is not a directory: {}",
                path.display()
            ));
        }
        false
    } else {
        std::fs::create_dir_all(&path).map_err(err)?;
        true
    };
    let path_str = path.display().to_string();
    let _ = state.persistence.set_kv("last_cwd", &path_str);
    Ok(CreateFolderResult {
        path: path_str,
        name: slug,
        created,
    })
}

fn sanitize_folder_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("folder name is empty".into());
    }
    // Keep letters, numbers, dash, underscore, space -> dash
    let mut out = String::new();
    for c in trimmed.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c.to_ascii_lowercase());
        } else if (c.is_whitespace() || c == '/' || c == '\\')
            && !out.ends_with('-')
            && !out.is_empty()
        {
            out.push('-');
        }
        // drop other punctuation
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        return Err("folder name has no usable characters".into());
    }
    if out == "." || out == ".." {
        return Err("invalid folder name".into());
    }
    if out.len() > 80 {
        return Err("folder name too long".into());
    }
    Ok(out)
}

fn resolve_projects_parent(parent: Option<String>) -> Result<PathBuf, String> {
    if let Some(p) = parent {
        let path = PathBuf::from(p);
        if !path.is_absolute() {
            return Err("parent must be absolute".into());
        }
        return Ok(path);
    }
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| "HOME not set".to_string())?;
    // Prefer existing project roots
    for candidate in ["Projects", "projects", "Code", "code", "Developer", "dev"] {
        let p = home.join(candidate);
        if p.is_dir() {
            return Ok(p);
        }
    }
    // Default: ~/Projects (create on demand by caller)
    Ok(home.join("Projects"))
}

// ── Phase 1: Sessions ────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct SessionIdResponse {
    pub id: String,
}

pub async fn start_session(
    state: &AppState,
    cwd: String,
    mut opts: SpawnOptions,
) -> Result<SessionIdResponse, String> {
    // Resolve MCP attachments via McpManager (names + auto + high-risk approval).
    let mut mcp_skipped = Vec::new();
    if !opts.mcp_server_names.is_empty() || opts.include_auto_mcp {
        let resolution = state
            .mcp
            .session_mcp_payload(
                &opts.mcp_server_names,
                &opts.approved_high_risk_mcp,
                opts.include_auto_mcp,
            )
            .await
            .map_err(err)?;
        opts.mcp_servers = resolution.payload;
        opts.mcp_server_names = resolution.attached_names;
        mcp_skipped = resolution.skipped;
    }
    let _gate = state.workspace_gate.lock().await;
    let id = Uuid::new_v4();
    let requested_cwd = cwd.clone();
    let mut spawn_cwd = cwd.clone();
    let mut isolation_note = None;
    let mut workspace_record = None;
    if let Some(wid) = opts.workspace_id.clone() {
        let w = workspaces::workspace(state, &wid)?;
        if w.archived_at.is_some() {
            return Err("This thread is archived".into());
        }
        workspaces::ensure_idle(state, &w)?;
        spawn_cwd = w.path.clone();
        opts.project_root = Some(w.project_root.clone());
        opts.isolate_worktree = false;
        opts.worktree = if w.inline { None } else { Some(w.name.clone()) };
        opts.read_only = w.inline || w.read_only;
        workspace_record = Some(w);
    } else if opts.mode == grok_control_core::AgentMode::Acp {
        let root = std::path::Path::new(&cwd);
        if !root.is_absolute() || !root.is_dir() {
            return Err("Choose an existing absolute project folder".into());
        }
        let shared = !opts.isolate_worktree || opts.checkout_branch.is_some();
        let inline = !opts.isolate_worktree && !opts.edit_checkout && opts.checkout_branch.is_none();
        if !inline && !grok_worktree::is_git_repo(root).await {
            return Err("Create a Git repository before starting a thread, or choose Inline to ask read-only questions".into());
        }
        let base = if let Some(base)=&opts.base_ref {
            grok_worktree::run_git(root,&["show-ref","--verify",&format!("refs/heads/{base}")]).await.map_err(err)?;
            base.clone()
        } else { workspaces::default_branch(root).await.unwrap_or_else(|_| "HEAD".into()) };
        let name = opts
            .prompt
            .as_deref()
            .map(prompt_slug)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "New thread".into());
        let branch;
        if let Some(existing_branch)=opts.checkout_branch.clone() {
            grok_worktree::run_git(root,&["check-ref-format","--branch",&existing_branch]).await.map_err(err)?;
            grok_worktree::run_git(root,&["show-ref","--verify",&format!("refs/heads/{existing_branch}")]).await.map_err(err)?;
            if let Some(wt)=state.worktrees.list(root).await.map_err(err)?.into_iter().find(|w|w.branch.as_deref()==Some(&existing_branch)) {
                spawn_cwd=wt.path.to_string_lossy().into_owned();
            } else {
                let target=state.paths.worktrees_dir.join(format!("existing-{}",&id.to_string()[..8]));
                tokio::fs::create_dir_all(&state.paths.worktrees_dir).await.map_err(err)?;
                grok_worktree::run_git(root,&["worktree","add",target.to_str().ok_or("Invalid worktree path")?,&existing_branch]).await.map_err(err)?;
                spawn_cwd=target.to_string_lossy().into_owned();
            }
            branch=existing_branch;
            opts.worktree=Some(branch.clone());
        } else if !shared {
            let slug: String = name
                .to_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .take(35)
                .collect();
            let wt = state
                .worktrees
                .create(
                    root,
                    CreateWorktreeRequest {
                        name: format!("{}-{}", slug.trim_matches('-'), &id.to_string()[..8]),
                        base_ref: Some(base.clone()),
                        prefer_grok_cli: false,
                    },
                )
                .await
                .map_err(|e| {
                    format!("Could not create thread: {e}. The project folder was not changed.")
                })?;
            spawn_cwd = wt.path.display().to_string();
            branch = wt.branch.unwrap_or_default();
            opts.worktree = Some(wt.name);
            isolation_note = Some(format!(
                "Thread created · {branch} · automatic checkpoints on"
            ));
        } else {
            branch = state
                .worktrees
                .current_branch(root)
                .await
                .unwrap_or_default();
            opts.read_only |= inline;
        }
        for other in state.persistence.list_workspaces().map_err(err)?.iter().filter(|w|w.path==spawn_cwd) {workspaces::ensure_idle(state,other)?;}
        opts.project_root = Some(cwd.clone());
        let existing = state
            .persistence
            .list_workspaces()
            .map_err(err)?
            .into_iter()
            .find(|w| w.path == spawn_cwd && w.inline == inline && w.read_only == opts.read_only);
        workspace_record = Some(existing.unwrap_or(grok_persistence::WorkspaceRecord {
            id: Uuid::new_v4().to_string(),
            project_root: cwd.clone(),
            name: if inline {
                "Inline (read-only)".into()
            } else {
                name
            },
            path: spawn_cwd.clone(),
            branch,
            base_ref: base,
            created_at: Utc::now().to_rfc3339(),
            archived_at: None,
            inline,
            shared_checkout: shared,
            read_only: opts.read_only,
            threads: vec![],
        }));
    }
    if opts.read_only {
        enforce_inline(&mut opts);
    }
    if opts.mode == grok_control_core::AgentMode::Acp {
        opts.prompt = None;
    }

    // Durable memory rides along: global notes + this project's notes are
    // injected with the thread's first prompt.
    let memory_context = build_memory_context(state, &requested_cwd).await;
    let source_id = opts
        .source_thread
        .as_deref()
        .and_then(|id| Uuid::parse_str(id).ok());
    let connect_opts = grok_control_core::ConnectOpts {
        resume_acp_session_id: None,
        transcript_context: source_id.and_then(|source| build_transcript_context(state, source)),
        memory_context,
    };
    if let Some(w) = &workspace_record {
        state.persistence.save_workspace(w).map_err(err)?;
    }
    state
        .registry
        .spawn_agent_preallocated(id, &spawn_cwd, opts, connect_opts)
        .await
        .map_err(err)?;
    if let Some(note) = isolation_note {
        let _ = state
            .persistence
            .append_message(id, "system", &note, Utc::now());
        state.event_bus.emit(grok_events::ControlEvent::Raw {
            session_id: Some(id),
            payload: serde_json::json!({ "channel": "term", "stream": "worktree", "line": note }),
        });
    }
    // Tell the thread why a server was left out — otherwise it just looks broken.
    for s in &mcp_skipped {
        let msg = format!("⚠ MCP `{}` skipped: {}", s.name, s.reason);
        let _ = state
            .persistence
            .append_message(id, "system", &msg, Utc::now());
        state.event_bus.emit(grok_events::ControlEvent::Raw {
            session_id: Some(id),
            payload: serde_json::json!({ "channel": "term", "stream": "mcp", "line": msg }),
        });
    }
    let _ = state.persistence.set_kv("last_cwd", &cwd);
    persist_session(state, id).await;
    if let Some(source) = source_id {
        for entry in state.persistence.transcript_entries(source).map_err(err)? {
            let _ = state
                .persistence
                .append_message(id, &entry.role, entry.body, Utc::now());
        }
    }
    if let Some(w) = workspace_record {
        state.persistence.save_workspace(&w).map_err(err)?;
        state.persistence.attach_workspace(id, &w.id).map_err(err)?;
    }
    Ok(SessionIdResponse { id: id.to_string() })
}

pub async fn start_mock_session(
    state: &AppState,
    cwd: String,
) -> Result<SessionIdResponse, String> {
    let id = state.registry.spawn_mock(&cwd).await.map_err(err)?;
    let _ = state.persistence.set_kv("last_cwd", &cwd);
    persist_session(state, id).await;
    Ok(SessionIdResponse { id: id.to_string() })
}

pub async fn list_sessions(
    state: &AppState,
) -> Result<Vec<grok_control_core::SessionMetadata>, String> {
    Ok(state.registry.list_sessions())
}

/// Live + SQLite-restored threads for the UI thread list.
pub async fn list_threads(state: &AppState) -> Result<Vec<ThreadDto>, String> {
    Ok(build_thread_list(state))
}

pub async fn get_session(state: &AppState, id: String) -> Result<AgentHandleSnapshot, String> {
    let id = Uuid::parse_str(&id).map_err(err)?;
    state.registry.get_snapshot(id).map_err(err)
}

/// Load transcript history from SQLite (works for live and restored threads).
pub async fn get_session_transcript(
    state: &AppState,
    id: String,
) -> Result<Vec<TranscriptEntry>, String> {
    let id = Uuid::parse_str(&id).map_err(err)?;
    state.persistence.transcript_entries(id).map_err(err)
}

/// An image attached to a prompt from the composer. `data` is base64 with no
/// `data:` URI prefix; `name` is only for the transcript breadcrumb.
#[derive(Debug, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ImageInput {
    pub mime_type: String,
    pub data: String,
    pub name: Option<String>,
}

/// Whether the live agent for a thread accepts image prompts. Unknown / not-live
/// threads answer `true` so the composer never blocks attaching pre-emptively.
pub async fn agent_supports_images(state: &AppState, id: String) -> Result<bool, String> {
    let id = Uuid::parse_str(&id).map_err(err)?;
    Ok(state
        .registry
        .image_prompts_supported(id)
        .await
        .unwrap_or(true))
}

#[allow(clippy::too_many_arguments)]
/// Block until a freshly spawned session has finished connecting (Idle) or
/// died (Failed). `send_prompt` refuses a session that is still Starting, so
/// the new-thread flow calls this between `start_session` and the first send.
pub async fn wait_until_idle(
    state: &AppState,
    id: &str,
    timeout: std::time::Duration,
) -> Result<(), String> {
    use grok_events::SessionStatus;
    let id = Uuid::parse_str(id).map_err(err)?;
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let status = state
            .registry
            .get_snapshot(id)
            .map_err(err)?
            .metadata
            .status;
        match status {
            SessionStatus::Starting => {}
            SessionStatus::Failed => {
                return Err("the agent failed to start — check Services".into())
            }
            _ if state.registry.is_ready(id) => return Ok(()),
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("the agent is taking too long to start".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    }
}

/// Small UI preferences persisted in the kv table (e.g. archived threads).
pub async fn kv_get(state: &AppState, key: &str) -> Result<Option<String>, String> {
    state.persistence.get_kv(key).map_err(err)
}

pub async fn kv_set(state: &AppState, key: &str, value: &str) -> Result<(), String> {
    state.persistence.set_kv(key, value).map_err(err)
}

/// Account usage limits for every backend that exposes them.
pub async fn account_usage() -> Vec<crate::usage::AccountUsage> {
    crate::usage::all().await
}

#[allow(clippy::too_many_arguments)]
pub async fn send_prompt(
    state: &AppState,
    id: String,
    prompt: String,
    backend: Option<String>,
    model: Option<String>,
    approval_mode: Option<String>,
    plan_mode: Option<bool>,
    always_approve: Option<bool>,
    images: Option<Vec<ImageInput>>,
    fast_mode: Option<bool>,
    effort: Option<String>,
) -> Result<(), String> {
    let id = Uuid::parse_str(&id).map_err(err)?;
    if state.foundry.for_thread(&id.to_string()).is_some_and(|r| !matches!(r.status, bomb_foundry::RunStatus::Completed | bomb_foundry::RunStatus::Stopped)) {
        return Err("This thread has a Foundry run. Stop the run before sending a separate prompt.".into());
    }
    let _gate = state.workspace_gate.lock().await;
    let cwd = state.registry.get_snapshot(id).ok().map(|s|s.metadata.cwd)
        .or_else(||state.persistence.get_session(id).ok().map(|s|s.cwd));
    if cwd.as_deref().is_some_and(|cwd|state.foundry.owns_cwd(cwd)) { return Err("A Foundry run owns this folder. Stop it before sending another prompt.".into()); }
    let workspace = state.persistence.workspace_for_session(id).map_err(err)?;
    if let Some(w) = &workspace {
        if w.archived_at.is_some() {
            return Err("This thread is archived".into());
        }
        workspaces::ensure_idle(state, w)?;
    }
    let images = images.unwrap_or_default();

    let want_backend = backend.as_deref().and_then(grok_config::Backend::from_key);
    let want_model = model.filter(|m| {
        let t = m.trim();
        !t.is_empty()
    });

    let requested_model = want_model.clone();
    let mut switch_notice = None;
    // Switching backend/model mid-thread: restart the thread under the new
    // agent. Cross-agent session/load can't work, so the resume ladder lands
    // on history-only and injects the prior transcript as context.
    if state.registry.is_live(id) {
        let snap = state.registry.get_snapshot(id).map_err(err)?;
        let cur = &snap.metadata;
        let backend_changed = want_backend.is_some_and(|b| b != cur.backend);
        let model_changed = want_model
            .as_deref()
            .is_some_and(|m| !m.eq_ignore_ascii_case(&cur.model) && cur.model != "mock");
        let needs_read_only = workspace.as_ref().is_some_and(|w| w.inline || w.read_only) && !cur.read_only;
        let connection_failed = state.registry.connection_failed(id);
        if connection_failed
            || needs_read_only
            || backend_changed
            || (model_changed && cur.mode == grok_control_core::AgentMode::Acp)
        {
            // A failed placeholder has no newly confirmed ACP session to save.
            // Preserve the last durable session ID/history for the resume ladder.
            if !connection_failed { persist_session(state, id).await; }
            state.registry.retire_session(id).await.map_err(err)?;
            switch_notice = resume_saved_session(
                state,
                id,
                want_backend,
                want_model.clone(),
                approval_mode.clone(),
                plan_mode,
                always_approve,
            )
            .await?;
        }
    }

    // Saved threads after reboot have history in SQLite but no live ACP process.
    // Auto-resume so "Send" picks up the same thread id + transcript.
    if !state.registry.is_live(id) {
        switch_notice = resume_saved_session(
            state,
            id,
            want_backend,
            want_model,
            approval_mode.clone(),
            plan_mode,
            always_approve,
        )
        .await?;
    }

    // Smart thread naming on the FIRST prompt: instant word-slug, then an
    // async narrator-provider title upgrade.
    let needs_label = state
        .registry
        .get_snapshot(id)
        .map(|s| s.metadata.label.is_none())
        .unwrap_or(false);
    if needs_label {
        let slug = prompt_slug(&prompt);
        if !slug.is_empty() {
            let _ = state.registry.set_label(id, &slug);
            emit_thread_label(state, id, &slug);
        }
        let explainer = state.explainer.clone();
        let registry = state.registry.clone();
        let bus = state.event_bus.clone();
        let persistence = state.persistence.clone();
        let prompt_for_title = prompt.clone();
        tokio::spawn(async move {
            if let Ok(title) = explainer.generate_title(&prompt_for_title).await {
                if !title.is_empty() && registry.set_label(id, &title).is_ok() {
                    bus.emit(grok_events::ControlEvent::Raw {
                        session_id: Some(id),
                        payload: serde_json::json!({
                            "channel": "thread", "kind": "label", "label": title,
                        }),
                    });
                    // Persist the upgraded label into the session record.
                    if let Ok(snap) = registry.get_snapshot(id) {
                        let _ = persistence.upsert_session(&SessionRecord {
                            id,
                            cwd: snap.metadata.cwd.clone(),
                            mode: "acp".into(),
                            model: snap.metadata.model.clone(),
                            status: format!("{:?}", snap.metadata.status).to_lowercase(),
                            worktree: snap.metadata.worktree.clone(),
                            acp_session_id: snap.metadata.acp_session_id.clone(),
                            metadata_json: serde_json::to_string(&snap)
                                .unwrap_or_else(|_| "{}".into()),
                            created_at: snap.metadata.created_at,
                            updated_at: Utc::now(),
                            message_count: 0,
                        });
                    }
                }
            }
        });
    }

    let effort_result = if let Some(effort) = effort.as_deref() {
        state.registry.set_effort(id, effort).await.map_err(err)
    } else { Ok(false) };
    let actual_effort = state.registry.current_effort(id).await;
    let _=state.persistence.set_kv(&format!("session-effort/{id}"),actual_effort.as_deref().unwrap_or_default());
    let active = state.registry.get_snapshot(id).map_err(err)?.metadata;
    let variant_note = requested_model.as_ref().filter(|wanted| active.backend == grok_config::Backend::Claude && wanted.strip_suffix("[1m]").is_some_and(|base| base.eq_ignore_ascii_case(&active.model)))
        .map(|wanted| format!("Requested {wanted}; this session offers {}. Using its advertised model variant.", active.model));
    if switch_notice.is_none() && variant_note.is_some() {
        switch_notice = Some(format!("Switched model: {} · {} → {} · {}.", active.backend.key(), requested_model.as_deref().unwrap_or_default(), active.backend.key(), active.model));
    }
    if let Some(line) = switch_notice {
        let reasoning = if effort_result.is_err() {
            format!("Reasoning selection could not be applied; current effort: {}. Prompt not sent.", actual_effort.as_deref().unwrap_or("not reported"))
        } else { match (&actual_effort, &effort) {
            (Some(actual), Some(requested)) if !actual.eq_ignore_ascii_case(requested) =>
                format!("Reasoning: {actual} (provider adjusted from {requested})."),
            (Some(actual), _) => format!("Reasoning: {actual}."),
            (None, Some(requested)) => format!("Reasoning: not reported by this agent (requested {requested})."),
            (None, None) => "Reasoning: not reported by this agent.".into(),
        }};
        let line = format!("{line} {} {reasoning}", variant_note.unwrap_or_default());
        state.persistence.append_message(id, "system", &line, Utc::now()).map_err(err)?;
        state.event_bus.emit(ControlEvent::Raw {
            session_id: Some(id),
            payload: serde_json::json!({"channel":"thread", "kind":"model_switch", "line":line, "effort":actual_effort, "backend":active.backend.key(), "model":active.model}),
        });
    }
    effort_result?;
    if let Some(enabled) = fast_mode {
        if let Some((option, values, _)) = state.registry.speed_option(id).await.map_err(err)? {
            let value = speed_value(&values, enabled).ok_or_else(|| "The connected agent does not offer the selected speed. Refresh provider models and try again.".to_string())?;
            if !state
                .registry
                .set_speed_option(id, &option, &value)
                .await
                .map_err(err)?
            {
                return Err("The agent could not apply the selected speed. Refresh provider models and try again.".into());
            }
        } else if enabled {
            return Err("Fast mode is not exposed for this model by the connected agent. Turn Fast mode off and send again.".into());
        }
    }
    let prompt_len = prompt.len();
    let acp_images: Vec<grok_acp::PromptImage> = images
        .iter()
        .map(|i| grok_acp::PromptImage {
            mime_type: i.mime_type.clone(),
            data: i.data.clone(),
        })
        .collect();
    let mut completion = state.event_bus.subscribe();
    let turn_guard = workspace
        .as_ref()
        .map(|w| workspaces::WorkspaceTurn::new(state.workspace_turns.clone(), &w.path))
        .transpose()?;
    state
        .registry
        .send_prompt_with_images(id, &prompt, &acp_images)
        .await
        .map_err(err)?;
    if let Some(w) = workspace.filter(|w| !w.inline && !w.read_only && !w.shared_checkout) {
        let manager = state.worktrees.clone();
        let db = state.persistence.clone();
        let bus = state.event_bus.clone();
        let gate = state.workspace_gate.clone();
        let summary = prompt_slug(&prompt);
        tokio::spawn(async move {
            let _turn_guard = turn_guard;
            let mut completed = false;
            loop {
                match completion.recv().await {
                    Ok(ControlEvent::Raw {
                        session_id: Some(sid),
                        payload,
                    }) if sid == id
                        && payload.get("turn_complete").and_then(|v| v.as_bool()) == Some(true) =>
                    {
                        completed = true;
                    }
                    Ok(ControlEvent::SessionStatusChanged {
                        session_id, status, ..
                    }) if session_id == id
                        && matches!(
                            status,
                            grok_events::SessionStatus::Idle
                                | grok_events::SessionStatus::Failed
                                | grok_events::SessionStatus::Cancelled
                        ) =>
                    {
                        let _guard = gate.lock().await;
                        if completed && status == grok_events::SessionStatus::Idle {
                            let message = format!("{}\n\nBomb-Thread: {id}", summary);
                            match manager
                                .commit_all(std::path::Path::new(&w.path), &message)
                                .await
                            {
                                Ok(true) => {
                                    let note = format!("Checkpoint saved: {summary}");
                                    let _ = db.append_message(id, "system", &note, Utc::now());
                                    bus.emit(ControlEvent::Raw { session_id: Some(id), payload: serde_json::json!({"channel":"thread", "kind":"checkpoint", "line":note}) });
                                }
                                Ok(false) => {}
                                Err(e) => bus.emit_error(
                                    Some(id),
                                    format!("Checkpoint failed; your files are still saved: {e}"),
                                ),
                            }
                        }
                        break;
                    }
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => break,
                }
            }
        });
    }
    // User message — durable immediately (agent side streams via event bus).
    // Note in the transcript that images rode along, so a reloaded thread does
    // not read as if only text was sent.
    let durable = if images.is_empty() {
        prompt.clone()
    } else {
        let names: Vec<String> = images
            .iter()
            .enumerate()
            .map(|(n, i)| i.name.clone().unwrap_or_else(|| format!("image {}", n + 1)))
            .collect();
        format!("{prompt}\n\n[attached: {}]", names.join(", "))
            .trim()
            .to_string()
    };
    let _ = state
        .persistence
        .append_message(id, "prompt", &durable, Utc::now());
    // Terminal breadcrumb so center column never looks idle after send.
    let _ = state.persistence.append_message(
        id,
        "system",
        format!("→ prompt accepted ({prompt_len} chars) · agent stream open"),
        Utc::now(),
    );
    persist_session(state, id).await;
    Ok(())
}

/// Bring a SQLite thread back online under the same id.
/// Ladder: session/load → session/resume → session/new + transcript inject.
/// Overrides switch the thread to a different backend/model; a backend switch
/// drops the prior ACP session id (it belongs to another agent) so the ladder
/// goes straight to history-only transcript injection.
async fn resume_saved_session(
    state: &AppState,
    id: Uuid,
    override_backend: Option<grok_config::Backend>,
    override_model: Option<String>,
    approval_mode: Option<String>,
    plan_mode: Option<bool>,
    always_approve: Option<bool>,
) -> Result<Option<String>, String> {
    let rec = state
        .persistence
        .get_session(id)
        .map_err(|e| format!("cannot resume thread — {e}"))?;

    if rec.cwd.trim().is_empty() {
        return Err("cannot resume: saved thread has no project path".into());
    }
    if !PathBuf::from(&rec.cwd).is_dir() {
        return Err(format!("cannot resume: project path missing — {}", rec.cwd));
    }

    let mut opts = SpawnOptions {
        mode: if rec.mode.eq_ignore_ascii_case("headless") {
            grok_control_core::AgentMode::Headless
        } else {
            grok_control_core::AgentMode::Acp
        },
        effort: Some(state.config.read().await.default_effort.clone()),
        ..SpawnOptions::default()
    };
    let recorded_backend = extract_backend_from_meta(&rec.metadata_json);
    model_history::record(&state.persistence, id, recorded_backend.key(), &rec.model);
    opts.backend = override_backend.unwrap_or(recorded_backend);
    let backend_switched = opts.backend != recorded_backend;
    opts.model = override_model.or_else(|| {
        if rec.model.is_empty() || backend_switched {
            // Old model id belongs to the other vendor; let config pick.
            None
        } else {
            Some(rec.model.clone())
        }
    });
    opts.worktree = rec.worktree.clone();
    // Preserve the project link so Land/Sync keep working after a restart.
    // Never re-isolate on resume: the stored cwd already IS the worktree.
    opts.isolate_worktree = false;
    opts.project_root = extract_meta_string(&rec.metadata_json, "projectRoot")
        .or_else(|| extract_meta_string(&rec.metadata_json, "project_root"));
    // Honor the caller's current stance. An explicit approval_mode wins;
    // otherwise fall back to the legacy booleans (default: plan on, yolo off).
    opts.approval_mode = approval_mode.as_deref().and_then(|m| match m {
        "plan" => Some(grok_control_core::ApprovalMode::Plan),
        "auto" => Some(grok_control_core::ApprovalMode::Auto),
        "yolo" | "always_approve" => Some(grok_control_core::ApprovalMode::Yolo),
        "ask" | "default" => Some(grok_control_core::ApprovalMode::Ask),
        _ => None,
    });
    opts.always_approve = always_approve.unwrap_or(false);
    opts.plan_mode = if opts.always_approve {
        false
    } else {
        plan_mode.unwrap_or(false)
    };
    opts.mcp_server_names = extract_mcp_from_meta(&rec.metadata_json);
    // Re-apply the high-risk approvals granted when the thread was created —
    // resuming must not silently drop approved servers (e.g. playwright).
    opts.approved_high_risk_mcp = extract_approved_mcp_from_meta(&rec.metadata_json);
    if matches!(opts.mode, grok_control_core::AgentMode::Headless) {
        opts.mode = grok_control_core::AgentMode::Acp;
    }

    if !opts.mcp_server_names.is_empty() {
        let resolution = state
            .mcp
            .session_mcp_payload(&opts.mcp_server_names, &opts.approved_high_risk_mcp, false)
            .await
            .map_err(|e| format!("MCP resolution failed on resume: {e}"))?;
        opts.mcp_servers = resolution.payload;
        opts.mcp_server_names = resolution.attached_names;
        for s in &resolution.skipped {
            let _ = state.persistence.append_message(
                id,
                "system",
                format!("⚠ MCP `{}` skipped on resume: {}", s.name, s.reason),
                Utc::now(),
            );
        }
    }

    let transcript_context = build_transcript_context(state, id);
    let memory_root = opts.project_root.clone().unwrap_or_else(|| rec.cwd.clone());
    let connect_opts = grok_control_core::ConnectOpts {
        // A prior ACP session id from another agent can't be loaded/resumed.
        resume_acp_session_id: if backend_switched {
            None
        } else {
            rec.acp_session_id.clone().filter(|s| !s.is_empty())
        },
        transcript_context: transcript_context.clone(),
        memory_context: build_memory_context(state, &memory_root).await,
    };

    if state
        .persistence
        .workspace_for_session(id)
        .map_err(err)?
        .is_some_and(|w| w.inline || w.read_only)
    {
        enforce_inline(&mut opts);
    }
    let brain = state
        .registry
        .resume_session(id, &rec.cwd, opts, Some(rec.created_at), connect_opts)
        .await
        .map_err(err)?;

    let msg = match brain {
        grok_control_core::BrainMode::FullBrain => {
            "🧠 full brain — agent reloaded prior ACP session (true continuity)"
        }
        grok_control_core::BrainMode::HistoryOnly => {
            "📜 history-only — new ACP process; prior transcript will be injected on next send"
        }
        grok_control_core::BrainMode::Fresh => {
            "agent resumed — fresh ACP session (no prior agent id / history pack)"
        }
    };
    let _ = state
        .persistence
        .append_message(id, "system", msg, Utc::now());
    persist_session(state, id).await;
    if let Ok(snapshot) = state.registry.get_snapshot(id) {
        let current = &snapshot.metadata;
        if recorded_backend != current.backend || !rec.model.eq_ignore_ascii_case(&current.model) {
            let continuity = match brain {
                grok_control_core::BrainMode::FullBrain => "Previous agent session restored.",
                grok_control_core::BrainMode::HistoryOnly => {
                    "Recent conversation history and project memory carried over."
                }
                grok_control_core::BrainMode::Fresh => {
                    "Fresh agent session; no prior history was available."
                }
            };
            let line = format!(
                "Switched model: {} · {} → {} · {}. {continuity}",
                recorded_backend.key(),
                rec.model,
                current.backend.key(),
                current.model
            );
            return Ok(Some(line));
        }
    }
    Ok(None)
}

/// Cheap instant label: first significant words of the prompt.
fn prompt_slug(prompt: &str) -> String {
    const STOP: &[&str] = &[
        "a", "an", "the", "to", "of", "in", "on", "for", "and", "or", "is", "it", "that", "this",
        "please", "can", "you", "me", "my", "i", "we",
    ];
    let words: Vec<&str> = prompt
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty() && !STOP.contains(&w.to_lowercase().as_str()))
        .take(5)
        .collect();
    let mut s = words.join(" ");
    if s.chars().count() > 42 {
        s = s.chars().take(42).collect::<String>() + "…";
    }
    s
}

fn emit_thread_label(state: &AppState, id: Uuid, label: &str) {
    state.event_bus.emit(grok_events::ControlEvent::Raw {
        session_id: Some(id),
        payload: serde_json::json!({ "channel": "thread", "kind": "label", "label": label }),
    });
}

/// Pack recent transcript for history-only rehydration (bounded).
/// Stable, readable memory scope key for a project folder.
pub(crate) fn project_memory_scope(project_root: &str) -> String {
    use std::hash::{Hash, Hasher};
    let clean = project_root.trim_end_matches('/');
    let base = clean
        .split('/')
        .rfind(|s| !s.is_empty())
        .unwrap_or("project");
    let mut h = std::collections::hash_map::DefaultHasher::new();
    clean.hash(&mut h);
    format!("{base}-{:06x}", h.finish() & 0xff_ffff)
}

/// Global notes + the project's notes, capped, for first-prompt injection.
async fn build_memory_context(state: &AppState, project_root: &str) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(g) = state.memory.context_pack("global", 1500).await {
        parts.push(format!("Global:\n{g}"));
    }
    let scope = project_memory_scope(project_root);
    if let Some(p) = state.memory.context_pack(&scope, 2500).await {
        parts.push(format!("This project:\n{p}"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn build_transcript_context(state: &AppState, id: Uuid) -> Option<String> {
    let entries = state.persistence.transcript_entries(id).ok()?;
    if entries.is_empty() {
        return None;
    }
    // Keep last ~40 turns, cap total chars.
    const MAX_ENTRIES: usize = 40;
    const MAX_CHARS: usize = 24_000;
    let slice = if entries.len() > MAX_ENTRIES {
        &entries[entries.len() - MAX_ENTRIES..]
    } else {
        &entries[..]
    };
    let mut out = String::new();
    for e in slice {
        let role = e.role.as_str();
        // Skip pure system noise
        if role == "system"
            && (e.body.contains("resumed")
                || e.body.contains("full brain")
                || e.body.contains("history-only")
                || e.body.contains("injected"))
        {
            continue;
        }
        let line = format!(
            "[{}] {}\n",
            role,
            e.body.chars().take(2000).collect::<String>()
        );
        if out.len() + line.len() > MAX_CHARS {
            break;
        }
        out.push_str(&line);
    }
    if out.trim().is_empty() {
        None
    } else {
        Some(out)
    }
}

pub async fn cancel_session(state: &AppState, id: String) -> Result<(), String> {
    if let Some(child)=state.foundry.stop_thread(&id)? { state.registry.cancel_session(child).await.map_err(err)?; }
    let id = Uuid::parse_str(&id).map_err(err)?;
    state.registry.cancel_session(id).await.map_err(err)?;
    persist_session(state, id).await;
    Ok(())
}

pub async fn remove_session(
    state: &AppState,
    id: String,
    remove_worktree: Option<bool>,
) -> Result<(), String> {
    let id = Uuid::parse_str(&id).map_err(err)?;
    if let Some(child)=state.foundry.stop_thread(&id.to_string())? { state.registry.cancel_session(child).await.map_err(err)?; }
    // Capture worktree context before the records disappear.
    let wt_ctx = if remove_worktree.unwrap_or(false)
        && state
            .persistence
            .workspace_for_session(id)
            .map_err(err)?
            .is_none()
    {
        thread_worktree_context(state, id).await.ok()
    } else {
        None
    };
    // Live handle may be gone after reboot — still wipe SQLite memory.
    let _ = state.registry.remove_session(id).await;
    state.persistence.delete_session(id).map_err(err)?;
    if let Some((worktree, root, _branch, _)) = wt_ctx {
        // Only remove managed worktrees (never the project root itself).
        if worktree != root && worktree.starts_with(state.worktrees.worktrees_root()) {
            if let Err(e) = state
                .worktrees
                .remove(&root, &worktree.display().to_string(), true)
                .await
            {
                tracing::warn!(error = %e, "worktree removal failed after thread delete");
            }
        }
    }
    Ok(())
}

pub async fn set_plan_mode(state: &AppState, id: String, enabled: bool) -> Result<(), String> {
    let id = Uuid::parse_str(&id).map_err(err)?;
    if !enabled
        && state
            .persistence
            .workspace_for_session(id)
            .map_err(err)?
            .is_some_and(|w| w.inline || w.read_only)
    {
        return Err("Inline is read-only. Create a thread to make changes.".into());
    }
    state.registry.set_plan_mode(id, enabled).await.map_err(err)
}

// ── Explainer (right-panel ELI12 narrator) ───────────────────────────────

pub async fn explainer_focus(state: &AppState, id: Option<String>) -> Result<(), String> {
    let uuid = match id.as_deref().filter(|s| !s.is_empty()) {
        Some(s) => Some(Uuid::parse_str(s).map_err(err)?),
        None => None,
    };
    state.explainer.set_focus(uuid).await;
    Ok(())
}

pub async fn set_explainer_provider(
    state: &AppState,
    backend: Option<String>,
    model: Option<String>,
) -> Result<(), String> {
    state
        .explainer
        .set_provider(backend.clone(), model.clone())
        .await;
    {
        let mut cfg = state.config.write().await;
        if backend.is_some() {
            cfg.explainer_backend = backend;
        }
        if model.is_some() {
            cfg.explainer_model = model;
        }
        cfg.save(&state.paths.config_file).map_err(err)?;
    }
    Ok(())
}

pub async fn set_explainer_enabled(state: &AppState, enabled: bool) -> Result<bool, String> {
    state.explainer.set_enabled(enabled);
    {
        let mut cfg = state.config.write().await;
        cfg.explainer_enabled = enabled;
        cfg.save(&state.paths.config_file).map_err(err)?;
    }
    Ok(state.explainer.enabled())
}

/// Set a live session's approval stance: plan | ask | auto | yolo.
/// Apply a reasoning effort to a live thread. Ok(false) = the agent does
/// not expose one (Grok/Codex take it at spawn time instead).
pub async fn set_session_effort(
    state: &AppState,
    id: String,
    effort: String,
) -> Result<bool, String> {
    let id = Uuid::parse_str(&id).map_err(err)?;
    let applied=state.registry.set_effort(id, &effort).await.map_err(err)?;
    let actual=state.registry.current_effort(id).await.unwrap_or_default();
    state.persistence.set_kv(&format!("session-effort/{id}"),&actual).map_err(err)?;
    Ok(applied)
}

pub async fn set_approval_mode(state: &AppState, id: String, mode: String) -> Result<(), String> {
    let id = Uuid::parse_str(&id).map_err(err)?;
    if mode != "plan"
        && state
            .persistence
            .workspace_for_session(id)
            .map_err(err)?
            .is_some_and(|w| w.inline || w.read_only)
    {
        return Err("Inline is read-only. Create a thread to make changes.".into());
    }
    let mode = match mode.to_lowercase().as_str() {
        "plan" => grok_control_core::ApprovalMode::Plan,
        "auto" => grok_control_core::ApprovalMode::Auto,
        "yolo" | "always_approve" => grok_control_core::ApprovalMode::Yolo,
        "ask" | "default" => grok_control_core::ApprovalMode::Ask,
        other => return Err(format!("unknown approval mode: {other}")),
    };
    if state.registry.is_live(id) {
        state
            .registry
            .set_approval_mode(id, mode)
            .await
            .map_err(err)?;
        persist_session(state, id).await;
    } else {
        let mut record = state.persistence.get_session(id).map_err(err)?;
        let mut snapshot: serde_json::Value =
            serde_json::from_str(&record.metadata_json).map_err(err)?;
        let metadata = snapshot
            .get_mut("metadata")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("Saved thread metadata is missing")?;
        metadata.insert(
            "approvalMode".into(),
            serde_json::to_value(mode).map_err(err)?,
        );
        metadata.insert(
            "planMode".into(),
            (mode == grok_control_core::ApprovalMode::Plan).into(),
        );
        metadata.insert(
            "alwaysApprove".into(),
            (mode == grok_control_core::ApprovalMode::Yolo).into(),
        );
        record.metadata_json = serde_json::to_string(&snapshot).map_err(err)?;
        record.updated_at = Utc::now();
        state.persistence.upsert_session(&record).map_err(err)?;
    }
    Ok(())
}

/// "Always allow this" from an approval card — auto-approves matching
/// requests for the rest of the session.
pub async fn add_session_allow_rule(
    state: &AppState,
    id: String,
    pattern: String,
) -> Result<(), String> {
    let id = Uuid::parse_str(&id).map_err(err)?;
    let pattern = pattern.trim().to_string();
    if pattern.is_empty() {
        return Err("empty rule".into());
    }
    state
        .registry
        .add_session_allow_rule(id, pattern.clone())
        .await
        .map_err(err)?;
    let _ = state.persistence.append_message(
        id,
        "system",
        format!("✓ always allowing `{pattern}` for the rest of this session"),
        Utc::now(),
    );
    Ok(())
}

pub async fn set_always_approve(state: &AppState, id: String, enabled: bool) -> Result<(), String> {
    let id = Uuid::parse_str(&id).map_err(err)?;
    if enabled
        && state
            .persistence
            .workspace_for_session(id)
            .map_err(err)?
            .is_some_and(|w| w.inline || w.read_only)
    {
        return Err("Inline is read-only. Create a thread to make changes.".into());
    }

    state
        .registry
        .set_always_approve(id, enabled)
        .await
        .map_err(err)
}

pub async fn respond_approval(
    state: &AppState,
    id: String,
    request_id: String,
    option_id: Option<String>,
) -> Result<(), String> {
    let id = Uuid::parse_str(&id).map_err(err)?;
    let id = state.foundry.approval_session(id);
    state
        .registry
        .respond_approval(id, &request_id, option_id.as_deref())
        .await
        .map_err(err)
}

/// Rename a thread (manual override of the smart name). Works for live and
/// saved threads; manual names are never overwritten by the auto-titler
/// (which only fires when a thread has no label at its first prompt).
pub async fn rename_thread(state: &AppState, id: String, label: String) -> Result<(), String> {
    let id = Uuid::parse_str(&id).map_err(err)?;
    let label = label.trim();
    if label.is_empty() {
        return Err("name cannot be empty".into());
    }
    let label: String = label.chars().take(60).collect();

    if state.registry.set_label(id, &label).is_ok() {
        persist_session(state, id).await;
    } else {
        // Saved thread: patch the label inside the persisted metadata.
        let mut rec = state.persistence.get_session(id).map_err(err)?;
        let mut v: serde_json::Value =
            serde_json::from_str(&rec.metadata_json).unwrap_or_else(|_| serde_json::json!({}));
        if !v.get("metadata").map(|m| m.is_object()).unwrap_or(false) {
            v["metadata"] = serde_json::json!({});
        }
        v["metadata"]["label"] = serde_json::json!(label);
        rec.metadata_json = v.to_string();
        rec.updated_at = Utc::now();
        state.persistence.upsert_session(&rec).map_err(err)?;
    }
    emit_thread_label(state, id, &label);
    Ok(())
}

// ── Projects (persisted folder list for the sidebar) ─────────────────────

pub async fn list_projects(state: &AppState) -> Result<Vec<String>, String> {
    Ok(state
        .persistence
        .get_kv("projects")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default())
}

pub async fn add_project(state: &AppState, path: String) -> Result<Vec<String>, String> {
    let path = path.trim().trim_end_matches('/').to_string();
    if path.is_empty() || !PathBuf::from(&path).is_dir() {
        return Err(format!("not a folder: {path}"));
    }
    let mut list = list_projects(state).await?;
    if !list.contains(&path) {
        list.push(path);
        list.sort();
        state
            .persistence
            .set_kv("projects", &serde_json::to_string(&list).map_err(err)?)
            .map_err(err)?;
    }
    Ok(list)
}

pub async fn remove_project(state: &AppState, path: String) -> Result<Vec<String>, String> {
    let mut list = list_projects(state).await?;
    list.retain(|p| p != &path);
    state
        .persistence
        .set_kv("projects", &serde_json::to_string(&list).map_err(err)?)
        .map_err(err)?;
    Ok(list)
}

// ── Thread land / sync (worktree merge flow) ─────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadMergeResult {
    /// landed | needs_sync | synced | conflicts
    pub status: String,
    pub files: Vec<String>,
    pub branch: String,
    pub target_branch: String,
}

/// Worktree path + project root + branch for a thread, or a friendly error.
async fn thread_worktree_context(
    state: &AppState,
    id: Uuid,
) -> Result<(PathBuf, PathBuf, String, String), String> {
    // Live metadata first; fall back to the persisted record.
    let (cwd, project_root, label) = match state.registry.get_snapshot(id) {
        Ok(snap) => (
            snap.metadata.cwd.clone(),
            snap.metadata.project_root.clone(),
            snap.metadata.label.clone().unwrap_or_default(),
        ),
        Err(_) => {
            let rec = state.persistence.get_session(id).map_err(err)?;
            let root = serde_json::from_str::<serde_json::Value>(&rec.metadata_json)
                .ok()
                .and_then(|v| {
                    v.pointer("/metadata/projectRoot")
                        .or_else(|| v.pointer("/metadata/project_root"))
                        .and_then(|p| p.as_str())
                        .map(String::from)
                });
            (rec.cwd, root, String::new())
        }
    };
    let project_root = project_root
        .filter(|p| !p.is_empty())
        .ok_or("this thread has no isolated worktree (it works directly in the project folder)")?;
    let worktree = PathBuf::from(&cwd);
    let root = PathBuf::from(&project_root);
    let branch = state
        .worktrees
        .current_branch(&worktree)
        .await
        .map_err(err)?;
    Ok((worktree, root, branch, label))
}

/// Merge the thread's worktree branch back into the project's current branch.
pub async fn land_thread(state: &AppState, id: String) -> Result<ThreadMergeResult, String> {
    let id = Uuid::parse_str(&id).map_err(err)?;
    let (worktree, root, branch, label) = thread_worktree_context(state, id).await?;
    let title = if label.is_empty() {
        branch.clone()
    } else {
        label.clone()
    };

    let _ = state
        .worktrees
        .commit_all(&worktree, &format!("thread {title}: work in progress"))
        .await
        .map_err(err)?;

    if !state.worktrees.is_clean(&root).await.map_err(err)? {
        return Err(format!(
            "the project folder has uncommitted changes — commit or stash them in {} first",
            root.display()
        ));
    }
    let target_branch = state.worktrees.current_branch(&root).await.map_err(err)?;

    match state
        .worktrees
        .merge(&root, &branch, &format!("land thread: {title}"))
        .await
        .map_err(err)?
    {
        grok_worktree::MergeOutcome::Merged => {
            let msg = format!("⬆ landed into {target_branch} ✓");
            let _ = state
                .persistence
                .append_message(id, "system", &msg, Utc::now());
            state.event_bus.emit(grok_events::ControlEvent::Raw {
                session_id: Some(id),
                payload: serde_json::json!({ "channel": "term", "stream": "worktree", "line": msg }),
            });
            Ok(ThreadMergeResult {
                status: "landed".into(),
                files: vec![],
                branch,
                target_branch,
            })
        }
        grok_worktree::MergeOutcome::Conflicts { files } => {
            // Never leave the user's main checkout mid-merge.
            state.worktrees.merge_abort(&root).await;
            let msg = format!(
                "⚠ landing hit conflicts in {} — run Sync so this thread's agent can resolve them, then land again",
                files.join(", ")
            );
            let _ = state
                .persistence
                .append_message(id, "system", &msg, Utc::now());
            state.event_bus.emit(grok_events::ControlEvent::Raw {
                session_id: Some(id),
                payload: serde_json::json!({ "channel": "term", "stream": "worktree", "line": msg }),
            });
            Ok(ThreadMergeResult {
                status: "needs_sync".into(),
                files,
                branch,
                target_branch,
            })
        }
    }
}

/// Merge the project's current branch INTO the thread's worktree. Conflicts
/// stay in the worktree where the thread's own agent can resolve them.
pub async fn sync_thread(state: &AppState, id: String) -> Result<ThreadMergeResult, String> {
    let id = Uuid::parse_str(&id).map_err(err)?;
    let (worktree, root, branch, label) = thread_worktree_context(state, id).await?;
    let title = if label.is_empty() {
        branch.clone()
    } else {
        label.clone()
    };
    let target_branch = state.worktrees.current_branch(&root).await.map_err(err)?;

    let _ = state
        .worktrees
        .commit_all(&worktree, &format!("thread {title}: work in progress"))
        .await
        .map_err(err)?;

    match state
        .worktrees
        .merge(
            &worktree,
            &target_branch,
            &format!("sync from {target_branch}"),
        )
        .await
        .map_err(err)?
    {
        grok_worktree::MergeOutcome::Merged => {
            let msg = format!("⟳ synced from {target_branch} ✓");
            let _ = state
                .persistence
                .append_message(id, "system", &msg, Utc::now());
            state.event_bus.emit(grok_events::ControlEvent::Raw {
                session_id: Some(id),
                payload: serde_json::json!({ "channel": "term", "stream": "worktree", "line": msg }),
            });
            Ok(ThreadMergeResult {
                status: "synced".into(),
                files: vec![],
                branch,
                target_branch,
            })
        }
        grok_worktree::MergeOutcome::Conflicts { files } => {
            let msg = format!(
                "⚠ merge conflicts from {target_branch} left in this worktree: {} — ask this thread's agent to resolve and commit them",
                files.join(", ")
            );
            let _ = state
                .persistence
                .append_message(id, "system", &msg, Utc::now());
            state.event_bus.emit(grok_events::ControlEvent::Raw {
                session_id: Some(id),
                payload: serde_json::json!({ "channel": "term", "stream": "worktree", "line": msg }),
            });
            Ok(ThreadMergeResult {
                status: "conflicts".into(),
                files,
                branch,
                target_branch,
            })
        }
    }
}

// ── Phase 2: Worktrees & Permissions ─────────────────────────────────────

pub async fn list_worktrees(state: &AppState, repo: String) -> Result<Vec<WorktreeInfo>, String> {
    state
        .worktrees
        .list(PathBuf::from(repo).as_path())
        .await
        .map_err(err)
}

pub async fn create_worktree(
    state: &AppState,
    repo: String,
    name: String,
    base_ref: Option<String>,
) -> Result<WorktreeInfo, String> {
    state
        .worktrees
        .create(
            PathBuf::from(repo).as_path(),
            CreateWorktreeRequest {
                name,
                base_ref,
                // Pure git, same as thread isolation — one layout for all
                // managed worktrees (the CLI path used its own location).
                prefer_grok_cli: false,
            },
        )
        .await
        .map_err(err)
}

pub async fn remove_worktree(
    state: &AppState,
    repo: String,
    name: String,
    force: bool,
) -> Result<(), String> {
    state
        .worktrees
        .remove(PathBuf::from(repo).as_path(), &name, force)
        .await
        .map_err(err)
}

pub async fn prune_worktrees(state: &AppState, repo: String) -> Result<String, String> {
    state
        .worktrees
        .prune(PathBuf::from(repo).as_path())
        .await
        .map_err(err)
}

pub async fn worktree_diff(state: &AppState, path: String) -> Result<String, String> {
    state
        .worktrees
        .diff(PathBuf::from(path).as_path())
        .await
        .map_err(err)
}

pub async fn list_permission_presets() -> Result<Vec<PermissionPreset>, String> {
    Ok(builtin_presets())
}

#[derive(Debug, Serialize)]
pub struct PermissionEvalResult {
    pub decision: PermissionDecision,
}

pub async fn evaluate_permission(
    state: &AppState,
    tool: String,
    detail: String,
    preset: Option<String>,
) -> Result<PermissionEvalResult, String> {
    let cfg = state.config.read().await;
    let ctl = if let Some(name) = preset {
        let presets = builtin_presets();
        let p = presets
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| format!("unknown preset: {name}"))?;
        PermissionController::with_preset(p)
    } else {
        PermissionController::from_defaults(&cfg.permissions, cfg.sandbox_profile)
    };
    Ok(PermissionEvalResult {
        decision: ctl.evaluate(&tool, &detail),
    })
}

// ── Phase 3: Extensions / MCP / Memory / Scheduler ───────────────────────

pub async fn list_extensions(state: &AppState) -> Result<Vec<ExtensionEntry>, String> {
    Ok(state.extensions.list_all().await)
}

/// Legacy simple add — prefers full `add_mcp_server` for catalog/security.
pub async fn add_mcp(
    state: &AppState,
    name: String,
    command: String,
    args: Vec<String>,
    enabled: bool,
) -> Result<(), String> {
    state
        .mcp
        .add(AddMcpRequest {
            name,
            kind: Some("custom".into()),
            transport: Some("stdio".into()),
            command: Some(command),
            args: Some(args),
            url: None,
            env: None,
            enabled: Some(enabled),
            scope: None,
            description: None,
            allowed_paths: None,
            read_only: None,
            auto_attach: None,
            requires_approval: Some(true),
            from_catalog: None,
            headers: None,
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            rate_limit_per_min: None,
            credential_keys: None,
        })
        .await
        .map_err(err)?;
    Ok(())
}

pub async fn remove_mcp(state: &AppState, name: String) -> Result<(), String> {
    state.mcp.remove(&name).await.map_err(err)
}

pub async fn toggle_mcp(state: &AppState, name: String, enabled: bool) -> Result<(), String> {
    state.mcp.set_enabled(&name, enabled).await.map_err(err)
}

// ── Full MCP manager surface ─────────────────────────────────────────────

pub async fn list_mcp_servers(state: &AppState) -> Result<Vec<McpServerConfigExt>, String> {
    Ok(state.mcp.list().await)
}

pub async fn get_mcp_server(state: &AppState, name: String) -> Result<McpServerConfigExt, String> {
    state.mcp.get(&name).await.map_err(err)
}

pub async fn add_mcp_server(
    state: &AppState,
    request: AddMcpRequest,
) -> Result<McpServerConfigExt, String> {
    state.mcp.add(request).await.map_err(err)
}

pub async fn update_mcp_server(
    state: &AppState,
    request: UpdateMcpRequest,
) -> Result<McpServerConfigExt, String> {
    state.mcp.update(request).await.map_err(err)
}

pub async fn remove_mcp_server(state: &AppState, name: String) -> Result<(), String> {
    state.mcp.remove(&name).await.map_err(err)
}

pub async fn doctor_mcp_server(
    state: &AppState,
    name: Option<String>,
) -> Result<Vec<DoctorReport>, String> {
    state.mcp.doctor(name.as_deref()).await.map_err(err)
}

pub async fn list_mcp_tools(
    state: &AppState,
    name: Option<String>,
) -> Result<Vec<McpToolInfo>, String> {
    state.mcp.list_tools(name.as_deref()).await.map_err(err)
}

pub async fn list_mcp_catalog() -> Result<Vec<McpCatalogEntry>, String> {
    Ok(grok_mcp::builtin_catalog())
}

pub async fn set_mcp_credential(
    state: &AppState,
    key: String,
    value: String,
) -> Result<(), String> {
    state.mcp.set_credential(&key, &value).await.map_err(err)
}

pub async fn list_mcp_credentials(state: &AppState) -> Result<Vec<McpCredential>, String> {
    state.mcp.list_credentials_masked().await.map_err(err)
}

pub async fn remove_mcp_credential(state: &AppState, key: String) -> Result<(), String> {
    state.mcp.credentials().remove(&key).map_err(err)
}

pub async fn suggest_mcp_for_project(
    state: &AppState,
    git_remote: Option<String>,
    branch: Option<String>,
) -> Result<Vec<String>, String> {
    Ok(state
        .mcp
        .suggest_for_project(git_remote.as_deref(), branch.as_deref())
        .await)
}

pub async fn preview_session_mcp(
    state: &AppState,
    names: Vec<String>,
    approved_high_risk: Vec<String>,
    include_auto: bool,
) -> Result<serde_json::Value, String> {
    let res = state
        .mcp
        .session_mcp_payload(&names, &approved_high_risk, include_auto)
        .await
        .map_err(err)?;
    // Webview gets masked secrets only.
    let masked: Vec<serde_json::Value> = res
        .payload
        .iter()
        .map(grok_mcp::mask_payload_for_preview)
        .collect();
    Ok(serde_json::json!({
        "servers": masked,
        "attached": res.attached_names,
        "skipped": res.skipped,
    }))
}

pub async fn add_skill(
    state: &AppState,
    name: String,
    path: Option<String>,
    description: Option<String>,
    enabled: bool,
) -> Result<(), String> {
    state
        .extensions
        .add_skill(name, path.map(PathBuf::from), description, enabled)
        .await
        .map_err(err)
}

pub async fn remove_skill(state: &AppState, name: String) -> Result<(), String> {
    state.extensions.remove_skill(&name).await.map_err(err)
}

pub async fn extensions_doctor(state: &AppState) -> Result<String, String> {
    state.extensions.doctor().await.map_err(err)
}

pub async fn memory_list(
    state: &AppState,
    scope: Option<String>,
) -> Result<Vec<MemoryEntry>, String> {
    Ok(state.memory.list(scope.as_deref()).await)
}

pub async fn memory_add(
    state: &AppState,
    scope: String,
    content: String,
    tags: Vec<String>,
) -> Result<MemoryEntry, String> {
    state.memory.add(scope, content, tags).await.map_err(err)
}

pub async fn memory_remove(state: &AppState, id: String) -> Result<(), String> {
    state.memory.remove(&id).await.map_err(err)
}

pub async fn memory_flush(state: &AppState, scope: String) -> Result<String, String> {
    state.memory.flush_markdown(&scope).await.map_err(err)
}

/// Digest: LLM-summarize a scope's notes into one compact entry (tagged
/// `digest`). Originals stay — the user deletes what's superseded.
pub async fn memory_digest(state: &AppState, scope: String) -> Result<MemoryEntry, String> {
    let pack = state
        .memory
        .context_pack(&scope, 6000)
        .await
        .ok_or("no notes in this scope to digest")?;
    let summary = state
        .explainer
        .summarize(&format!(
            "Condense these project notes into at most 8 short bullet points, \
             merging duplicates and dropping anything obsolete. Keep concrete \
             facts (commands, versions, decisions). Output only the bullets.\n\n{pack}"
        ))
        .await?;
    state
        .memory
        .add(&scope, summary, vec!["digest".into()])
        .await
        .map_err(err)
}

/// Scope key the given project folder maps to (for the Memory view).
pub async fn project_scope(path: String) -> Result<String, String> {
    if path.trim().is_empty() {
        return Ok("global".into());
    }
    Ok(project_memory_scope(&path))
}

/// Pin a piece of transcript text into the project's durable memory.
pub async fn remember(
    state: &AppState,
    id: String,
    content: String,
) -> Result<MemoryEntry, String> {
    let id = Uuid::parse_str(&id).map_err(err)?;
    let content = content.trim();
    if content.is_empty() {
        return Err("nothing to remember".into());
    }
    let content: String = content.chars().take(600).collect();
    // Scope by the thread's project (fall back to its cwd, then global).
    let root = state
        .registry
        .get_snapshot(id)
        .ok()
        .map(|s| s.metadata.project_root.clone().unwrap_or(s.metadata.cwd))
        .or_else(|| state.persistence.get_session(id).ok().map(|r| r.cwd))
        .filter(|s| !s.is_empty());
    let scope = root
        .map(|r| project_memory_scope(&r))
        .unwrap_or_else(|| "global".into());
    state
        .memory
        .add(&scope, content, vec!["remembered".into()])
        .await
        .map_err(err)
}

pub async fn scheduler_list(state: &AppState) -> Result<Vec<ScheduledJob>, String> {
    Ok(state.scheduler.list().await)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerAddRequest {
    pub name: String,
    pub prompt: String,
    pub interval_secs: Option<u64>,
    pub cron: Option<String>,
    pub once_delay_secs: Option<u64>,
    pub cwd: Option<String>,
    pub max_runs: Option<u64>,
}

pub async fn scheduler_add(
    state: &AppState,
    request: SchedulerAddRequest,
) -> Result<ScheduledJob, String> {
    let schedule = if let Some(expr) = request.cron {
        ScheduleKind::Cron { expr }
    } else if let Some(d) = request.once_delay_secs {
        ScheduleKind::Once { delay_secs: d }
    } else {
        ScheduleKind::Interval {
            secs: request.interval_secs.unwrap_or(3600),
        }
    };
    state
        .scheduler
        .add(
            request.name,
            request.prompt,
            schedule,
            request.cwd,
            request.max_runs,
        )
        .await
        .map_err(err)
}

pub async fn scheduler_cancel(state: &AppState, id: String) -> Result<(), String> {
    state.scheduler.cancel(&id).await.map_err(err)
}

pub async fn scheduler_pause(state: &AppState, id: String) -> Result<(), String> {
    state.scheduler.pause(&id).await.map_err(err)
}

pub async fn scheduler_resume(state: &AppState, id: String) -> Result<(), String> {
    state.scheduler.resume(&id).await.map_err(err)
}

// ── Phase 4: Diff, Export, Recovery ──────────────────────────────────────

pub async fn diff_current(cwd: String) -> Result<DiffSummary, String> {
    DiffEngine::current_summary(PathBuf::from(cwd).as_path())
        .await
        .map_err(err)
}

pub async fn diff_capture_before(cwd: String) -> Result<DiffCapture, String> {
    DiffEngine::capture_before(PathBuf::from(cwd).as_path())
        .await
        .map_err(err)
}

pub async fn diff_capture_after(capture: DiffCapture) -> Result<DiffCapture, String> {
    DiffEngine::capture_after(capture).await.map_err(err)
}

pub async fn export_session_markdown(state: &AppState, id: String) -> Result<String, String> {
    let id = Uuid::parse_str(&id).map_err(err)?;
    state.persistence.export_markdown(id).map_err(err)
}

pub async fn list_persisted_sessions(state: &AppState) -> Result<Vec<SessionRecord>, String> {
    state.persistence.list_sessions().map_err(err)
}

pub async fn persistence_checkpoint(state: &AppState) -> Result<(), String> {
    state.persistence.checkpoint().map_err(err)
}

pub async fn shutdown_all(state: &AppState) -> Result<(), String> {
    state.registry.shutdown_all().await;
    state.persistence.checkpoint().map_err(err)?;
    Ok(())
}

async fn persist_session(state: &AppState, id: Uuid) {
    if let Ok(snap) = state.registry.get_snapshot(id) {
        model_history::record(
            &state.persistence,
            id,
            snap.metadata.backend.key(),
            &snap.metadata.model,
        );
        let mode = match snap.metadata.mode {
            grok_control_core::AgentMode::Acp => "acp",
            grok_control_core::AgentMode::Headless => "headless",
        };
        let status = format!("{:?}", snap.metadata.status).to_lowercase();
        let metadata_json = serde_json::to_string(&snap).unwrap_or_else(|_| "{}".into());
        let rec = SessionRecord {
            id,
            cwd: snap.metadata.cwd.clone(),
            mode: mode.into(),
            model: snap.metadata.model.clone(),
            status,
            worktree: snap.metadata.worktree.clone(),
            acp_session_id: snap.metadata.acp_session_id.clone(),
            metadata_json,
            created_at: snap.metadata.created_at,
            updated_at: Utc::now(),
            message_count: 0,
        };
        let _ = state.persistence.upsert_session(&rec);
    }
}

fn build_thread_list(state: &AppState) -> Vec<ThreadDto> {
    let show_mock = std::env::var("BOMB_SMOKE").ok().as_deref() == Some("1");
    let mut out = build_thread_list_all(state);
    if !show_mock {
        out.retain(|t| !t.model.eq_ignore_ascii_case("mock"));
    }
    out
}

fn build_thread_list_all(state: &AppState) -> Vec<ThreadDto> {
    let live = state.registry.list_sessions();
    let mut live_ids = std::collections::HashSet::new();
    let mut out: Vec<ThreadDto> = Vec::new();

    for m in live {
        live_ids.insert(m.id);
        let mode = match m.mode {
            grok_control_core::AgentMode::Acp => "acp",
            grok_control_core::AgentMode::Headless => "headless",
        };
        let status = format!("{:?}", m.status).to_lowercase();
        let msg_count = state
            .persistence
            .get_session(m.id)
            .map(|r| r.message_count)
            .unwrap_or(0);
        out.push(ThreadDto {
            models_used: model_history::load(&state.persistence, m.id),
            id: m.id.to_string(),
            cwd: m.cwd,
            mode: mode.into(),
            model: m.model,
            backend: m.backend.key().to_string(),
            status,
            live: true,
            message_count: msg_count,
            created_at: m.created_at.to_rfc3339(),
            updated_at: m.last_activity.to_rfc3339(),
            worktree: m.worktree,
            mcp_servers: m.mcp_servers,
            label: m.label,
            approval_mode: Some(
                serde_json::to_value(m.approval_mode)
                    .ok()
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_else(|| "ask".into()),
            ),
            project_root: m.project_root,
            brain_mode: Some(m.brain_mode.as_str().into()),
        });
    }

    if let Ok(saved) = state.persistence.list_sessions() {
        for rec in saved {
            if live_ids.contains(&rec.id) {
                continue;
            }
            // After reboot ACP is gone — never show stale "running".
            let status = match rec.status.to_lowercase().as_str() {
                "running" | "starting" | "cancelling" | "waitingapproval" => "saved".into(),
                other => other.to_string(),
            };
            let mcp = extract_mcp_from_meta(&rec.metadata_json);
            let backend = extract_backend_from_meta(&rec.metadata_json);
            let label = extract_meta_string(&rec.metadata_json, "label");
            let project_root = extract_meta_string(&rec.metadata_json, "projectRoot")
                .or_else(|| extract_meta_string(&rec.metadata_json, "project_root"));
            out.push(ThreadDto {
                models_used: model_history::load(&state.persistence, rec.id),
                id: rec.id.to_string(),
                cwd: rec.cwd,
                mode: rec.mode,
                model: rec.model,
                backend: backend.key().to_string(),
                status,
                live: false,
                message_count: rec.message_count,
                created_at: rec.created_at.to_rfc3339(),
                updated_at: rec.updated_at.to_rfc3339(),
                worktree: rec.worktree,
                mcp_servers: mcp,
                label,
                approval_mode: extract_meta_string(&rec.metadata_json, "approvalMode")
                    .or_else(|| extract_meta_string(&rec.metadata_json, "approval_mode")),
                project_root,
                brain_mode: None,
            });
        }
    }

    out.retain(|t| !state.foundry.child(&t.id));
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out
}

fn extract_backend_from_meta(json: &str) -> grok_config::Backend {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| {
            v.pointer("/metadata/backend")
                .and_then(|b| b.as_str())
                .and_then(grok_config::Backend::from_key)
        })
        .unwrap_or_default()
}

fn extract_approved_mcp_from_meta(json: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| {
            v.pointer("/metadata/approvedHighRiskMcp")
                .or_else(|| v.pointer("/metadata/approved_high_risk_mcp"))
                .cloned()
        })
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn extract_meta_string(json: &str, key: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| {
            v.pointer(&format!("/metadata/{key}"))
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from)
        })
}

fn extract_mcp_from_meta(json: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| {
            v.pointer("/metadata/mcpServers")
                .or_else(|| v.pointer("/metadata/mcp_servers"))
                .cloned()
        })
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

/// Called from the event-bus persistence task (best-effort, never panics).
pub fn persist_control_event(db: &grok_persistence::Persistence, ev: &ControlEvent) {
    use ControlEvent::*;
    let res: Result<(), grok_persistence::PersistenceError> = (|| match ev {
        AgentMessage {
            session_id,
            text,
            at,
        } => {
            if text.trim().is_empty() {
                return Ok(());
            }
            let lower = text.to_lowercase();
            if lower.starts_with("prompt sent") || lower == "turn complete" {
                return Ok(());
            }
            // Keep the chunk's own spacing — these are streaming deltas and
            // append_message_merged concatenates them into one row.
            let (kind, body) = match text.strip_prefix('💭') {
                Some(rest) => ("thought", rest),
                None => ("agent", text.as_str()),
            };
            db.append_message_merged(*session_id, kind, body, *at, 10)
                .map(|_| ())
        }
        ToolCall { session_id, event } => {
            // Plan-presenting tool calls persist their plan as a plan_doc
            // row — the raw JSON dump would just duplicate it, hugely.
            let is_plan_tool = event.tool.to_lowercase().contains("plan")
                || event.args_summary.contains("\"plan\":");
            if is_plan_tool {
                return Ok(());
            }
            let payload = serde_json::json!({
                "id": event.id,
                "tool": event.tool,
                "status": event.status,
                "args": event.args_summary,
                "result": event.result_summary,
            })
            .to_string();
            db.append_message(*session_id, "tool", payload, event.at)
                .map(|_| ())?;
            // Tool output may arrive after the turn ends. Only lifecycle events change status.
            Ok(())
        }
        PlanUpdate { session_id, event } => {
            let payload = serde_json::to_string(event).unwrap_or_else(|_| "{}".into());
            db.append_message(*session_id, "plan", payload, event.at)
                .map(|_| ())
        }
        SessionStatusChanged {
            session_id, status, ..
        } => {
            let s = format!("{status:?}").to_lowercase();
            db.update_session_status(*session_id, &s)
        }
        SessionCancelled { session_id, at } => {
            let _ = db.update_session_status(*session_id, "cancelled");
            db.append_message(*session_id, "system", "session cancelled", *at)
                .map(|_| ())
        }
        SessionCompleted { session_id, at } => {
            let _ = db.update_session_status(*session_id, "completed");
            db.append_message(*session_id, "system", "session completed", *at)
                .map(|_| ())
        }
        ApprovalRequired {
            session_id,
            tool,
            summary,
            auto_approved,
            at,
            ..
        } => {
            if *auto_approved {
                db.append_message(
                    *session_id,
                    "system",
                    format!("auto-approved (yolo): {tool}"),
                    *at,
                )
                .map(|_| ())
            } else {
                let _ = db.update_session_status(*session_id, "waitingapproval");
                // Durable as an approval row so it renders as a card (inert
                // after restart — the live request died with the process).
                db.append_message(*session_id, "approval", format!("{tool} — {summary}"), *at)
                    .map(|_| ())
            }
        }
        ApprovalResolved {
            session_id,
            option_id,
            cancelled,
            at,
            ..
        } => {
            let _ = db.update_session_status(*session_id, "running");
            let body = if *cancelled {
                "approval cancelled".to_string()
            } else {
                format!(
                    "approval granted: {}",
                    option_id.as_deref().unwrap_or("selected")
                )
            };
            db.append_message(*session_id, "system", body, *at)
                .map(|_| ())
        }
        Raw { session_id:Some(session_id), payload } if payload["channel"]=="policy_blocked" => {
            db.append_message(*session_id,"system",payload["message"].as_str().unwrap_or("Access was blocked by read-only policy."),Utc::now()).map(|_|())
        }
        // Plan documents lifted out of plan-presenting tool calls
        // (ExitPlanMode etc.) — durable as real plan rows.
        Raw {
            session_id: Some(session_id),
            payload,
        } if payload.get("channel").and_then(|v| v.as_str()) == Some("plan_doc") => {
            let Some(text) = payload.get("text").and_then(|v| v.as_str()) else {
                return Ok(());
            };
            db.append_message(*session_id, "plan", text, Utc::now())
                .map(|_| ())
        }
        // Images produced by tools: bytes go to disk next to the database,
        // the row keeps the path so a restored thread shows them again.
        Raw {
            session_id: Some(session_id),
            payload,
        } if payload.get("channel").and_then(|v| v.as_str()) == Some("image") => {
            let Some(data) = payload.get("data").and_then(|v| v.as_str()) else {
                return Ok(());
            };
            let mime = payload
                .get("mimeType")
                .and_then(|v| v.as_str())
                .unwrap_or("image/png");
            let ext = match mime {
                "image/jpeg" => "jpg",
                "image/webp" => "webp",
                "image/gif" => "gif",
                "image/svg+xml" => "svg",
                _ => "png",
            };
            use base64::Engine;
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else {
                return Ok(());
            };
            let dir = db
                .path()
                .parent()
                .map(|p| p.join("images").join(session_id.to_string()))
                .unwrap_or_else(|| PathBuf::from("/tmp/bomb-images"));
            let _ = std::fs::create_dir_all(&dir);
            let name = format!("{}.{ext}", Utc::now().format("%Y%m%d-%H%M%S%.3f"));
            let path = dir.join(name);
            if std::fs::write(&path, &bytes).is_err() {
                return Ok(());
            }
            let row = serde_json::json!({ "path": path.display().to_string(), "mimeType": mime })
                .to_string();
            db.append_message(*session_id, "image", row, Utc::now())
                .map(|_| ())
        }
        // Raw ACP protocol lines: persist (merged into bounded multiline
        // rows) so the View toggle can reveal history across restarts.
        // Skip our own side channels (explain/usage/thread label events).
        Raw {
            session_id: Some(session_id),
            payload,
        } if payload.get("channel").and_then(|v| v.as_str()) == Some("term") => {
            let Some(line) = payload.get("line").and_then(|v| v.as_str()) else {
                return Ok(());
            };
            if line.trim().is_empty() {
                return Ok(());
            }
            db.append_message_merged(*session_id, "term", &format!("{line}\n"), Utc::now(), 10)
                .map(|_| ())
        }
        Error {
            session_id: Some(session_id),
            message,
            at,
        } => {
            // Errors are rows, not verdicts — terminal failures arrive as
            // SessionStatusChanged(Failed). Flipping the record here made a
            // recovered thread show a permanent failed badge after reboot.
            db.append_message(*session_id, "error", message.clone(), *at)
                .map(|_| ())
        }
        _ => Ok(()),
    })();
    if let Err(e) = res {
        tracing::debug!(error = %e, "persist_control_event skipped/failed");
    }
}

// ── Dev server / live preview ────────────────────────────────────────────

fn resolve_preview_cwd(
    state: &AppState,
    cwd: Option<String>,
    session_id: Option<String>,
) -> Result<PathBuf, String> {
    // A thread's own directory wins (it may be a worktree). Live threads carry
    // it in the registry; a saved thread has no live snapshot but its cwd is
    // still on disk — falling straight through to "session not found" made
    // preview unusable for any thread whose agent was not currently running.
    if let Some(id) = session_id.as_deref() {
        if let Ok(uuid) = Uuid::parse_str(id) {
            let from_thread = state
                .registry
                .get_snapshot(uuid)
                .map(|snap| snap.metadata.cwd)
                .ok()
                .or_else(|| state.persistence.get_session(uuid).map(|rec| rec.cwd).ok());
            if let Some(dir) = from_thread {
                let p = PathBuf::from(dir);
                if p.is_dir() {
                    return Ok(p);
                }
                // A worktree that has since been pruned: fall back to the
                // project cwd rather than dead-ending.
                tracing::warn!(path = %p.display(), "thread cwd is gone; falling back to project cwd");
            }
        }
    }
    if let Some(c) = cwd {
        let p = PathBuf::from(c);
        if p.is_dir() {
            return Ok(p);
        }
        return Err(format!("cwd is not a directory: {}", p.display()));
    }
    if let Ok(Some(last)) = state.persistence.get_kv("last_cwd") {
        let p = PathBuf::from(last);
        if p.is_dir() {
            return Ok(p);
        }
    }
    Err("No project path — select a session or set cwd".into())
}

pub async fn detect_dev_server(
    state: &AppState,
    cwd: Option<String>,
    session_id: Option<String>,
) -> Result<crate::devserver::DetectedProject, String> {
    let path = resolve_preview_cwd(state, cwd, session_id)?;
    crate::devserver::DevServerManager::detect(&path)
}

pub async fn start_dev_server(
    state: &AppState,
    cwd: Option<String>,
    session_id: Option<String>,
    open_browser: Option<bool>,
) -> Result<crate::devserver::DevServerStatus, String> {
    let path = resolve_preview_cwd(state, cwd, session_id)?;
    let _ = state
        .persistence
        .set_kv("last_cwd", &path.display().to_string());
    state
        .dev_server
        .start(&path, open_browser.unwrap_or(true))
        .await
}

pub async fn stop_dev_server(
    state: &AppState,
) -> Result<crate::devserver::DevServerStatus, String> {
    Ok(state.dev_server.stop().await)
}

pub async fn dev_server_status(
    state: &AppState,
) -> Result<crate::devserver::DevServerStatus, String> {
    Ok(state.dev_server.status().await)
}

pub async fn open_dev_server(state: &AppState) -> Result<String, String> {
    state.dev_server.open_in_browser().await
}

pub async fn reveal_project(
    state: &AppState,
    cwd: Option<String>,
    session_id: Option<String>,
) -> Result<(), String> {
    let path = resolve_preview_cwd(state, cwd, session_id)?;
    crate::devserver::DevServerManager::reveal_project(&path).await
}

fn enforce_inline(opts: &mut SpawnOptions) {
    opts.read_only = true;
    opts.approval_mode = Some(grok_control_core::ApprovalMode::Plan);
    opts.plan_mode = true;
    opts.always_approve = false;
    opts.sandbox_profile = Some("read-only".into());
    opts.mcp_servers.clear();
    opts.mcp_server_names.clear();
    opts.include_auto_mcp = false;
    opts.permission_deny.push("*".into());
}

#[cfg(test)]
mod image_restore_tests {
    use super::*;

    #[test]
    fn generated_image_survives_database_reopen_and_transcript_hydration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        let id = Uuid::new_v4();
        let data = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=";
        {
            let db = grok_persistence::Persistence::open(&path).unwrap();
            persist_control_event(
                &db,
                &ControlEvent::Raw {
                    session_id: Some(id),
                    payload: serde_json::json!({"channel":"image", "mimeType":"image/png", "data":data}),
                },
            );
        }
        let db = grok_persistence::Persistence::open(path).unwrap();
        let rows = db.transcript_entries(id).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].role, "image");
        let mut thread = crate::transcript::Thread::new();
        thread.hydrate(&rows);
        assert_eq!(thread.entries.len(), 1);
        assert!(matches!(
            thread.entries[0].role,
            crate::transcript::Role::Agent
        ));
        let images = &thread.entries[0].images;
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].mime_type, "image/png");
        assert_eq!(images[0].data, data);
        assert!(std::path::Path::new(images[0].name.as_deref().unwrap()).is_file());
    }
}

pub mod scratch;

pub mod model_history;

fn speed_value(values: &[String], enabled: bool) -> Option<String> {
    let preferred: &[&str] = if enabled {
        &["on", "true", "enabled", "fast", "priority"]
    } else {
        &["off", "false", "disabled", "standard", "normal", "default"]
    };
    preferred.iter().find_map(|choice| {
        values
            .iter()
            .find(|value| value.eq_ignore_ascii_case(choice))
            .cloned()
    })
}

#[cfg(test)]
mod speed_selection_tests {
    #[test]
    fn speed_uses_the_agents_exact_values_and_rejects_unknown_choices() {
        let values = vec!["off".into(), "on".into()];
        assert_eq!(super::speed_value(&values, true).as_deref(), Some("on"));
        assert_eq!(super::speed_value(&values, false).as_deref(), Some("off"));
        assert!(super::speed_value(&["high".into()], true).is_none());
    }
}

pub mod model_catalog;

pub mod model_suggestions;
