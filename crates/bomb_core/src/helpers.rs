//! Helpers: subagents on any agent and model, started by a thread's own agent.
//!
//! Every top-level thread's agent gets an MCP server, `bomb` (this app run with `--bomb-mcp`), whose tools
//! reach this module over a local socket. A helper is a thread of its own on the agent and model
//! asked for, linked to the thread that started it (`parentThread`) and listed with its subagents.
//! See docs/plan/cross_agent_subagents_plan.md.
//!
//! Rules: a helper runs in its parent's mode, never a more permissive one; in a parent that can
//! edit, each helper gets its own branch; helpers can't start helpers; at most `MAX_RUNNING`
//! run at once and `MAX_TOTAL` per thread; stopping the parent stops its helpers.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use grok_config::Backend;
use grok_control_core::{ApprovalMode, SpawnOptions};
use grok_events::{ControlEvent, SessionStatus, ToolCallEvent, ToolCallStatus};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use uuid::Uuid;

use crate::services;
use crate::state::AppState;

/// Helpers one thread may have running at once, and in all.
pub const MAX_RUNNING: usize = 4;
pub const MAX_TOTAL: usize = 8;
/// `wait_helpers` returns by then, so a call never outlasts an agent's tool timeout; call again to keep waiting.
const WAIT_MAX_SECS: u64 = 50;

#[derive(Default)]
pub struct Helpers {
    /// The token each top-level thread's `bomb` server carries → that thread.
    tokens: Mutex<HashMap<String, Uuid>>,
    by_thread: Mutex<HashMap<Uuid, String>>,
    /// Where helper calls are served, once they are.
    socket: Mutex<Option<PathBuf>>,
    /// Helpers whose turn has ended since they last got a message.
    finished: Mutex<HashSet<Uuid>>,
    /// Tokens each helper has used, as its agent reports them.
    tokens_used: Mutex<HashMap<Uuid, u64>>,
    /// The shared state, for background work (set once helpers are served).
    shared: Mutex<Option<std::sync::Weak<AppState>>>,
}

impl Helpers {
    fn token_for(&self, thread: Uuid) -> String {
        let mut by_thread = self.by_thread.lock().unwrap_or_else(|e| e.into_inner());
        by_thread
            .entry(thread)
            .or_insert_with(|| {
                let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
                self.tokens.lock().unwrap_or_else(|e| e.into_inner()).insert(token.clone(), thread);
                token
            })
            .clone()
    }

    fn thread_of(&self, token: &str) -> Option<Uuid> {
        self.tokens.lock().unwrap_or_else(|e| e.into_inner()).get(token).copied()
    }

    fn shared(&self) -> Option<Arc<AppState>> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|w| w.upgrade())
    }

    pub fn tokens_used(&self, helper: Uuid) -> Option<u64> {
        self.tokens_used.lock().unwrap_or_else(|e| e.into_inner()).get(&helper).copied()
    }
}

/// The `bomb` MCP server entry for a top-level thread's agent, once helpers are served.
pub fn mcp_server(state: &AppState, thread: Uuid) -> Option<Value> {
    let socket = state.helpers.socket.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    // The app (or bombd) is its own MCP server when started with --bomb-mcp (see `helpers_mcp`).
    let command = std::env::current_exe().ok()?;
    let token = state.helpers.token_for(thread);
    Some(json!({
        "name": "bomb",
        "command": command,
        "args": ["--bomb-mcp"],
        "env": [
            { "name": "BOMB_HELPERS_SOCKET", "value": socket },
            { "name": "BOMB_HELPERS_TOKEN", "value": token },
        ],
    }))
}

/// The `bomb` tools run without an approval card: they're Bomb Code's own, and a helper never
/// gets more freedom than its thread. Only these six names; deny rules still win.
pub fn allow_rules() -> Vec<String> {
    ["list_agents", "start_helper", "wait_helpers", "helper_status", "message_helper", "stop_helper"]
        .iter()
        .map(|tool| format!("*bomb*{tool}*"))
        .collect()
}

/// Serve helper calls on a socket only this user can open. Runs until the app quits.
pub async fn serve(state: Arc<AppState>) -> Result<(), String> {
    let path = state.paths.bomb_dir.join("helpers.sock");
    let _ = std::fs::remove_file(&path);
    let listener = tokio::net::UnixListener::bind(&path).map_err(|e| format!("helpers socket {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    *state.helpers.socket.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
    *state.helpers.shared.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::downgrade(&state));
    tracing::info!("helpers served");
    loop {
        let (stream, _) = listener.accept().await.map_err(|e| e.to_string())?;
        let state = state.clone();
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let request: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
                let token = request["token"].as_str().unwrap_or_default();
                let result = match state.helpers.thread_of(token) {
                    Some(parent) => call(&state, parent, request["method"].as_str().unwrap_or_default(), &request["params"]).await,
                    None => Err("This helper connection isn't tied to a thread.".to_string()),
                };
                let reply = match result {
                    Ok(value) => json!({ "id": request["id"], "ok": value }),
                    Err(error) => json!({ "id": request["id"], "error": error }),
                };
                if write.write_all(format!("{reply}\n").as_bytes()).await.is_err() {
                    return;
                }
            }
        });
    }
}

/// One tool call from a thread's `bomb` server.
pub async fn call(state: &AppState, parent: Uuid, method: &str, params: &Value) -> Result<Value, String> {
    let text = |key: &str| params[key].as_str().unwrap_or_default().trim().to_string();
    let ids = || -> Vec<Uuid> {
        params["ids"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).filter_map(|s| Uuid::parse_str(s).ok()).collect()).unwrap_or_default()
    };
    match method {
        "list_agents" => list_agents(state).await,
        "start_helper" => start(state, parent, &text("agent"), &text("model"), &text("task"), &text("name")).await,
        "wait_helpers" => {
            let secs = params["timeout_secs"].as_u64().unwrap_or(WAIT_MAX_SECS).min(WAIT_MAX_SECS);
            wait(state, parent, ids(), Duration::from_secs(secs)).await
        }
        "helper_status" => report(state, parent, &ids(), false),
        "message_helper" => {
            let helper = owned(state, parent, &text("id"))?;
            let message = text("text");
            if message.is_empty() {
                return Err("Say what to tell the helper.".into());
            }
            state.helpers.finished.lock().unwrap_or_else(|e| e.into_inner()).remove(&helper);
            let events = state.event_bus.subscribe();
            services::send_prompt(state, helper.to_string(), message, None, None, None, None, None, None, None, None).await?;
            watch(state, parent, helper, events);
            Ok(json!({ "id": helper, "status": "running" }))
        }
        "stop_helper" => {
            let helper = owned(state, parent, &text("id"))?;
            services::cancel_session(state, helper.to_string()).await?;
            Ok(json!({ "id": helper, "status": "stopped" }))
        }
        other => Err(format!("No helper tool called {other}.")),
    }
}

/// Agents this machine can run now (signed in), each with its models' ids and names.
async fn list_agents(state: &AppState) -> Result<Value, String> {
    let backends = services::list_backends(state).await?;
    let auth = services::backend_auth_status(state).await.unwrap_or_default();
    let agents: Vec<Value> = backends
        .into_iter()
        .filter(|b| b.available && auth.iter().any(|a| a.backend == b.id && a.logged_in && a.runnable))
        .map(|b| {
            let models: Vec<Value> = b.models.iter().map(|id| json!({ "id": id, "name": b.model_names.get(id).cloned().unwrap_or_else(|| id.clone()) })).collect();
            json!({ "agent": b.id, "name": b.display_name, "defaultModel": b.default_model, "models": models })
        })
        .collect();
    Ok(json!({ "agents": agents }))
}

/// "codex", "Codex", "openai", "chatgpt" → Codex; and so on.
fn backend_named(agent: &str) -> Option<Backend> {
    let a = agent.trim().to_lowercase();
    Backend::from_key(&a).or(match a.as_str() {
        "openai" | "chatgpt" | "gpt" | "codex cli" => Some(Backend::Codex),
        "anthropic" | "claude code" => Some(Backend::Claude),
        "xai" | "grok build" => Some(Backend::Grok),
        _ => None,
    })
}

/// A model by id, alias or display name ("Astra", "Opus 5.5"), among the agent's models.
fn model_named(wanted: &str, ids: &[String], names: &HashMap<String, String>) -> Option<String> {
    let norm = |s: &str| s.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>();
    let w = norm(wanted);
    if w.is_empty() {
        return None;
    }
    let name_of = |id: &String| names.get(id).cloned().unwrap_or_else(|| id.clone());
    ids.iter().find(|id| norm(id) == w || norm(&name_of(id)) == w)
        .or_else(|| ids.iter().find(|id| norm(&name_of(id)).contains(&w) || norm(id).contains(&w)))
        .cloned()
}

/// A short name from the task when none was given: its first few words.
fn name_from(task: &str) -> String {
    let words: Vec<&str> = task.split_whitespace().take(5).collect();
    let name = words.join(" ");
    if name.is_empty() { "Helper".into() } else { name.trim_end_matches(['.', ',', ':', ';']).to_string() }
}

/// This thread's helpers: (id, still working).
fn helpers_of(state: &AppState, parent: Uuid) -> Vec<(Uuid, bool)> {
    let parent = parent.to_string();
    let mut out: Vec<(Uuid, bool)> = state
        .registry
        .list_sessions()
        .into_iter()
        .filter(|m| m.parent_thread.as_deref() == Some(parent.as_str()))
        .map(|m| (m.id, matches!(m.status, SessionStatus::Running | SessionStatus::Starting | SessionStatus::WaitingApproval)))
        .collect();
    if let Ok(saved) = state.persistence.list_sessions() {
        for rec in saved {
            let helper = services::subagent_parent(&rec.metadata_json).is_some_and(|p| p.to_string() == parent)
                && serde_json::from_str::<Value>(&rec.metadata_json).ok().and_then(|v| v.pointer("/metadata/subagent/kind").and_then(|k| k.as_str()).map(|k| k == "helper")).unwrap_or(false);
            if helper && !out.iter().any(|(id, _)| *id == rec.id) {
                out.push((rec.id, false));
            }
        }
    }
    out
}

/// A helper id this thread started.
fn owned(state: &AppState, parent: Uuid, id: &str) -> Result<Uuid, String> {
    let helper = Uuid::parse_str(id).map_err(|_| "That isn't a helper id.".to_string())?;
    if helpers_of(state, parent).iter().any(|(h, _)| *h == helper) { Ok(helper) } else { Err("That helper isn't one this thread started.".into()) }
}

async fn start(state: &AppState, parent: Uuid, agent: &str, model: &str, task: &str, name: &str) -> Result<Value, String> {
    if task.is_empty() {
        return Err("Give the helper a task.".into());
    }
    let parent_meta = state.registry.get_snapshot(parent).map_err(|_| "The thread asking for a helper isn't running.".to_string())?.metadata;
    if parent_meta.parent_thread.is_some() {
        return Err("A helper can't start helpers.".into());
    }
    let mine = helpers_of(state, parent);
    if mine.len() >= MAX_TOTAL {
        return Err(format!("This thread already started {MAX_TOTAL} helpers, the most it can."));
    }
    let running = mine.iter().filter(|(_, r)| *r).count();
    if running >= MAX_RUNNING {
        return Err(format!("{running} helpers are already working, the most at once. Wait for one with wait_helpers, then start the next."));
    }
    let backend = if agent.is_empty() { parent_meta.backend } else {
        backend_named(agent).ok_or_else(|| format!("No agent called {agent}. Use claude, codex or grok (list_agents shows which are set up)."))?
    };
    let mock = model.eq_ignore_ascii_case("mock");
    let signed_in = mock || services::backend_auth_status(state).await.unwrap_or_default().into_iter().any(|a| a.backend == backend.key() && a.logged_in && a.runnable);
    if !signed_in {
        return Err(format!("{} isn't set up on this machine. Sign in to it in Bomb Code's Services first.", backend.key()));
    }
    let (model_id, model_label) = if model.is_empty() {
        (None, None)
    } else if model.eq_ignore_ascii_case("mock") {
        (Some("mock".to_string()), Some("mock".to_string()))
    } else {
        let info = services::list_backends(state).await?.into_iter().find(|b| b.id == backend.key()).ok_or("That agent isn't available.")?;
        let id = model_named(model, &info.models, &info.model_names).ok_or_else(|| {
            let names: Vec<String> = info.models.iter().map(|id| info.model_names.get(id).cloned().unwrap_or_else(|| id.clone())).collect();
            format!("{} has no model called {model}. It offers: {}.", info.display_name, names.join(", "))
        })?;
        let label = info.model_names.get(&id).cloned().unwrap_or_else(|| id.clone());
        (Some(id), Some(label))
    };
    // Same mode as the parent, never more permissive; a parent that can edit gives its helpers branches.
    let mode = parent_meta.approval_mode;
    let editing = mode != ApprovalMode::Plan;
    let root = parent_meta.project_root.clone().unwrap_or_else(|| parent_meta.cwd.clone());
    let name = if name.is_empty() { name_from(task) } else { name.to_string() };
    let opts = SpawnOptions {
        backend,
        model: model_id.clone(),
        approval_mode: Some(mode),
        plan_mode: mode == ApprovalMode::Plan,
        always_approve: mode == ApprovalMode::Yolo,
        isolate_worktree: editing,
        project_root: Some(root.clone()),
        prompt: Some(task.to_string()),
        parent_thread: Some(parent.to_string()),
        subagent: Some(json!({
            "kind": "helper", "name": name, "task": task, "agent": backend.key(),
            "model": model_label, "startedAt": Utc::now().to_rfc3339(),
        })),
        ..SpawnOptions::default()
    };
    let helper = Uuid::parse_str(&services::start_session(state, root, opts).await?.id).map_err(|e| e.to_string())?;
    // The parent shows it like any subagent: a step that runs until the helper is done.
    state.event_bus.emit(ControlEvent::Raw {
        session_id: Some(parent),
        payload: json!({ "channel": "subagent", "kind": "spawned", "child": helper, "name": name, "task": task, "model": model_label, "agent": backend.key() }),
    });
    state.event_bus.emit_tool_call(parent, ToolCallEvent {
        id: helper.to_string(),
        tool: format!("Subagent · {name}"),
        args_summary: task.to_string(),
        status: ToolCallStatus::Running,
        result_summary: None,
        at: Utc::now(),
    });
    // Send the task once its agent is up, then watch for the end of its turn.
    let shared = state.helpers.shared().ok_or("Helpers aren't available here.")?;
    let task = task.to_string();
    tokio::spawn(async move {
        let state = shared;
        if let Err(e) = services::wait_until_idle(&state, &helper.to_string(), Duration::from_secs(120)).await {
            finish(&state, parent, helper, "failed", Some(e)).await;
            return;
        }
        let events = state.event_bus.subscribe();
        if let Err(e) = services::send_prompt(&state, helper.to_string(), task, None, None, None, None, None, None, None, None).await {
            finish(&state, parent, helper, "failed", Some(e)).await;
            return;
        }
        watch(&state, parent, helper, events);
    });
    Ok(json!({
        "id": helper, "name": name, "agent": backend.key(), "model": model_label,
        "status": "running", "edits": if editing { "on its own branch" } else { "read-only" },
        "next": "Call wait_helpers with this id (or several) to get its report.",
    }))
}

/// Wait for the helper's turn to end, then mark it done in its parent (and stop the clock there).
/// `events` is subscribed before the prompt is sent, so a quick turn's end isn't missed.
fn watch(state: &AppState, parent: Uuid, helper: Uuid, mut events: tokio::sync::broadcast::Receiver<ControlEvent>) {
    let Some(state) = state.helpers.shared() else { return };
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(ControlEvent::SessionStatusChanged { session_id, status, .. }) if session_id == helper => {
                    let state_word = match status {
                        SessionStatus::Idle | SessionStatus::Completed => "completed",
                        SessionStatus::Failed => "failed",
                        SessionStatus::Cancelled => "cancelled",
                        _ => continue,
                    };
                    finish(&state, parent, helper, state_word, None).await;
                    return;
                }
                Ok(ControlEvent::Raw { session_id: Some(id), payload }) if id == helper && payload["channel"] == "usage" => {
                    if let Some(tokens) = payload["totalTokens"].as_u64() {
                        state.helpers.tokens_used.lock().unwrap_or_else(|e| e.into_inner()).insert(helper, tokens);
                        state.event_bus.emit(ControlEvent::Raw { session_id: Some(parent), payload: json!({ "channel": "subagent", "kind": "usage", "child": helper, "tokens": tokens }) });
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
                _ => {}
            }
        }
    });
}

async fn finish(state: &AppState, parent: Uuid, helper: Uuid, word: &str, error: Option<String>) {
    state.helpers.finished.lock().unwrap_or_else(|e| e.into_inner()).insert(helper);
    if let Some(error) = &error {
        let _ = state.persistence.append_message(helper, "error", error, Utc::now());
    }
    state.event_bus.emit(ControlEvent::Raw {
        session_id: Some(parent),
        payload: json!({ "channel": "subagent", "kind": "state", "child": helper, "state": word }),
    });
    state.event_bus.emit_tool_call(parent, ToolCallEvent {
        id: helper.to_string(),
        tool: "tool".into(),
        args_summary: String::new(),
        status: if word == "completed" { ToolCallStatus::Completed } else { ToolCallStatus::Failed },
        result_summary: Some(word.to_string()),
        at: Utc::now(),
    });
}

/// Is the helper done with what it was last asked?
fn done(state: &AppState, helper: Uuid) -> bool {
    if state.helpers.finished.lock().unwrap_or_else(|e| e.into_inner()).contains(&helper) {
        return true;
    }
    match state.registry.get_snapshot(helper) {
        Ok(snap) => matches!(snap.metadata.status, SessionStatus::Failed | SessionStatus::Cancelled | SessionStatus::Completed),
        Err(_) => true,
    }
}

async fn wait(state: &AppState, parent: Uuid, mut ids: Vec<Uuid>, timeout: Duration) -> Result<Value, String> {
    if ids.is_empty() {
        ids = helpers_of(state, parent).into_iter().map(|(id, _)| id).collect();
    }
    for id in &ids {
        owned(state, parent, &id.to_string())?;
    }
    let deadline = tokio::time::Instant::now() + timeout;
    while !ids.iter().all(|id| done(state, *id)) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    report(state, parent, &ids, true)
}

/// Each helper's status; with `reports`, a finished one's final message too.
fn report(state: &AppState, parent: Uuid, ids: &[Uuid], reports: bool) -> Result<Value, String> {
    let ids: Vec<Uuid> = if ids.is_empty() { helpers_of(state, parent).into_iter().map(|(id, _)| id).collect() } else { ids.to_vec() };
    let mut out = Vec::new();
    for id in ids {
        owned(state, parent, &id.to_string())?;
        let rec = state.persistence.get_session(id).ok();
        let meta: Value = rec.as_ref().and_then(|r| serde_json::from_str(&r.metadata_json).ok()).unwrap_or(Value::Null);
        let sub = meta.pointer("/metadata/subagent").cloned().unwrap_or(Value::Null);
        let finished = done(state, id);
        let status = if !finished { "running" } else {
            match state.registry.get_snapshot(id).map(|s| s.metadata.status) {
                Ok(SessionStatus::Failed) => "failed",
                Ok(SessionStatus::Cancelled) => "stopped",
                _ => "finished",
            }
        };
        let entries = state.persistence.transcript_entries(id).unwrap_or_default();
        let last_agent = entries.iter().rev().find(|e| e.role == "agent").map(|e| e.body.trim().to_string());
        let mut item = json!({
            "id": id, "name": sub["name"], "agent": sub["agent"], "model": sub["model"], "status": status,
            "steps": entries.iter().filter(|e| e.role == "tool").count(),
            "tokens": state.helpers.tokens_used(id),
        });
        if let Some(branch) = meta.pointer("/metadata/worktree").and_then(|w| w.as_str()) {
            item["branch"] = branch.into();
        }
        if reports && finished {
            item["report"] = last_agent.unwrap_or_default().into();
        } else if !finished {
            item["latest"] = entries.iter().rev().find(|e| e.role == "agent" || e.role == "tool").map(|e| e.body.chars().take(200).collect::<String>()).into();
        }
        out.push(item);
    }
    Ok(json!({ "helpers": out }))
}

/// Stop a thread's working helpers (its turn was stopped).
pub async fn stop_all(state: &AppState, parent: Uuid) {
    for (helper, running) in helpers_of(state, parent) {
        if running {
            let _ = state.registry.cancel_session(helper).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_resolve_by_id_alias_or_display_name() {
        let ids = vec!["gpt-6-astra".to_string(), "gpt-6-terra".to_string(), "opus".to_string(), "claude-opus-5-5".to_string()];
        let names: HashMap<String, String> = [("gpt-6-astra", "6 Astra"), ("claude-opus-5-5", "Opus 5.5"), ("opus", "Opus")]
            .into_iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        assert_eq!(model_named("astra", &ids, &names).as_deref(), Some("gpt-6-astra"));
        assert_eq!(model_named("Opus 5.5", &ids, &names).as_deref(), Some("claude-opus-5-5"));
        assert_eq!(model_named("opus", &ids, &names).as_deref(), Some("opus"));
        assert_eq!(model_named("haiku", &ids, &names), None);
        assert_eq!(backend_named("OpenAI"), Some(Backend::Codex));
        assert_eq!(backend_named("Claude"), Some(Backend::Claude));
        assert_eq!(name_from("Tell me what's in this repo, briefly."), "Tell me what's in this");
    }
}
