//! Concurrent session registry with ACP-first spawn path.

use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use dashmap::DashMap;
use serde_json::json;
use tracing::{info, warn};
use uuid::Uuid;

use grok_acp::{AcpClient, AcpClientConfig, AcpSpawnOptions, ApprovalMode, BrainMode, ConnectOpts};
use grok_cli_wrapper::{GrokCli, HeadlessSpawnOptions};
use grok_config::{Backend, GrokConfig, ResolvedBackend, descriptor, resolve_backend};
use grok_events::{EventBus, SessionStatus};

use crate::error::{CoreError, Result};
use crate::handle::{AgentHandle, AgentHandleSnapshot, SessionMetadata};
use crate::options::{AgentMode, SpawnOptions};

pub struct SessionRegistry {
    sessions: Arc<DashMap<Uuid, AgentHandle>>,
    event_bus: Arc<EventBus>,
    config: Arc<tokio::sync::RwLock<GrokConfig>>,
    grok_cli: Arc<GrokCli>,
}

/// Everything the deferred ACP connect needs, detached from `&self` so it can
/// run in a background task while the UI already shows the thread.
struct PendingConnect {
    client_cfg: AcpClientConfig,
    acp_opts: AcpSpawnOptions,
    connect_opts: ConnectOpts,
    /// Reasoning effort to apply through the agent's `effort` config option
    /// (the Claude adapter advertises one; Grok takes a CLI flag instead).
    effort: Option<String>,
}

/// Run the ACP handshake and fill in (or fail) the placeholder session entry.
async fn connect_and_fill(
    sessions: Arc<DashMap<Uuid, AgentHandle>>,
    event_bus: Arc<EventBus>,
    id: Uuid,
    pending: PendingConnect,
) -> Result<()> {
    let pending_effort = pending.effort.clone();
    match AcpClient::connect_with(
        pending.client_cfg,
        &pending.acp_opts,
        Some(event_bus.clone()),
        id,
        pending.connect_opts,
    )
    .await
    {
        Ok(client) => {
            if let Some(effort) = pending_effort.as_deref() {
                match client.set_effort(effort).await {
                    Ok(true) => {}
                    Ok(false) => tracing::debug!("agent has no effort config option"),
                    Err(e) => tracing::warn!(error = %e, "setting effort failed"),
                }
            }
            let acp_session_id = client.session_id().await;
            let brain_mode = client.brain_mode().await;
            // Never hold a DashMap guard across an await.
            if let Some(mut entry) = sessions.get_mut(&id) {
                entry.metadata.acp_session_id = acp_session_id;
                entry.metadata.brain_mode = brain_mode;
                entry.metadata.status = SessionStatus::Idle;
                entry.acp_client = Some(client);
                entry.touch();
            } else {
                // Session was removed while starting — kill the orphan.
                let _ = client.shutdown().await;
            }
            Ok(())
        }
        Err(e) => {
            if let Some(mut entry) = sessions.get_mut(&id) {
                entry.metadata.status = SessionStatus::Failed;
                entry.touch();
            }
            event_bus.emit_error(Some(id), format!("session start failed: {e}"));
            event_bus.emit_status(id, SessionStatus::Failed).await;
            Err(e.into())
        }
    }
}

impl SessionRegistry {
    pub fn new(
        event_bus: Arc<EventBus>,
        config: Arc<tokio::sync::RwLock<GrokConfig>>,
        grok_cli: Arc<GrokCli>,
    ) -> Arc<Self> {
        let registry = Arc::new(Self {
            sessions: Arc::new(DashMap::new()),
            event_bus: event_bus.clone(),
            config,
            grok_cli,
        });
        // Mirror status events into live metadata. The ACP client reports
        // turn completion (Idle) only on the event bus; without this the
        // thread list — which reads metadata — shows "running" forever
        // after the first prompt.
        if tokio::runtime::Handle::try_current().is_ok() {
            let sessions = registry.sessions.clone();
            let mut rx = event_bus.subscribe();
            tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(grok_events::ControlEvent::SessionStatusChanged {
                            session_id,
                            status,
                            ..
                        }) => {
                            if let Some(mut entry) = sessions.get_mut(&session_id) {
                                entry.metadata.status = status;
                                entry.metadata.last_activity = Utc::now();
                            }
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }
        registry
    }

    /// Start a session. The thread appears (status `Starting`) immediately;
    /// the ACP handshake completes in the background and flips it to
    /// `Idle`/`Failed` via status events.
    pub async fn spawn_agent(&self, cwd: &str, opts: SpawnOptions) -> Result<Uuid> {
        let id = Uuid::new_v4();
        self.spawn_agent_preallocated(id, cwd, opts, ConnectOpts::default())
            .await?;
        Ok(id)
    }

    /// Spawn with a caller-chosen id — used when the caller needs the id
    /// before spawning (e.g. to name the thread's worktree after it).
    /// `connect_opts` carries first-prompt injections (memory context).
    pub async fn spawn_agent_preallocated(
        &self,
        id: Uuid,
        cwd: &str,
        opts: SpawnOptions,
        connect_opts: ConnectOpts,
    ) -> Result<()> {
        self.spawn_agent_with_id(id, cwd, opts, None, connect_opts, true)
            .await
    }

    /// Set the display label (smart thread name).
    pub fn set_label(&self, id: Uuid, label: &str) -> Result<()> {
        let mut entry = self
            .sessions
            .get_mut(&id)
            .ok_or(CoreError::SessionNotFound(id))?;
        entry.metadata.label = Some(label.to_string());
        Ok(())
    }

    /// Re-attach a live ACP process to an existing thread id (after reboot / update).
    /// Tries session/load for full brain; else history-only + transcript inject.
    pub async fn resume_session(
        &self,
        id: Uuid,
        cwd: &str,
        opts: SpawnOptions,
        created_at: Option<chrono::DateTime<Utc>>,
        connect_opts: ConnectOpts,
    ) -> Result<BrainMode> {
        if self.sessions.contains_key(&id) {
            let mode = if let Some(c) = self.sessions.get(&id).and_then(|h| h.acp_client.clone()) {
                c.brain_mode().await
            } else {
                BrainMode::Fresh
            };
            return Ok(mode);
        }
        // Resume is blocking: the caller sends a prompt right after, so the
        // client must be live before we return.
        self.spawn_agent_with_id(id, cwd, opts, created_at, connect_opts, false)
            .await?;
        let mode = self
            .sessions
            .get(&id)
            .and_then(|h| h.acp_client.clone())
            .map(|c| async move { c.brain_mode().await });
        let brain = if let Some(f) = mode {
            f.await
        } else {
            BrainMode::HistoryOnly
        };
        info!(%id, cwd, ?brain, "session resumed from disk");
        Ok(brain)
    }

    async fn spawn_agent_with_id(
        &self,
        id: Uuid,
        cwd: &str,
        opts: SpawnOptions,
        created_at: Option<chrono::DateTime<Utc>>,
        connect_opts: ConnectOpts,
        background: bool,
    ) -> Result<()> {
        opts.validate().map_err(CoreError::InvalidOptions)?;

        let cwd_path = Path::new(cwd);
        if !cwd_path.is_absolute() {
            return Err(CoreError::InvalidOptions(format!(
                "cwd must be absolute: {cwd}"
            )));
        }
        if !cwd_path.exists() {
            return Err(CoreError::InvalidOptions(format!(
                "cwd does not exist: {cwd}"
            )));
        }

        let cfg = self.config.read().await;
        let max = cfg.max_concurrent_sessions;
        if self.sessions.len() >= max {
            return Err(CoreError::MaxSessions(max));
        }

        if opts.always_approve {
            warn!("spawning with always_approve=true — elevated trust mode");
        }

        let backend = opts.backend;
        let model = opts
            .model
            .clone()
            .unwrap_or_else(|| cfg.model_for(backend));
        // Mock threads need no binary; headless resolves via grok_cli.
        let needs_binary =
            matches!(opts.mode, AgentMode::Acp) && !model.eq_ignore_ascii_case("mock");
        let resolved: Option<ResolvedBackend> = if needs_binary {
            Some(resolve_backend(backend, &cfg)?)
        } else {
            None
        };
        let backend_env_cfg = cfg
            .backend_config(backend)
            .map(|c| c.env.clone())
            .unwrap_or_default();
        // Deny rules: global config deny list + per-spawn deny list, enforced
        // by the ACP client ahead of any approval.
        let mut deny_patterns = cfg.permissions.deny.clone();
        deny_patterns.extend(opts.permission_deny.iter().cloned());
        // Allow rules: auto-approve matching requests in any mode (deny wins).
        // These were previously built in config and silently dropped here.
        let mut allow_patterns = cfg.permissions.allow.clone();
        allow_patterns.extend(opts.permission_allow.iter().cloned());
        drop(cfg);
        let approval_mode = opts.resolved_mode();

        let now = Utc::now();
        let mut metadata = SessionMetadata {
            id,
            acp_session_id: None,
            cwd: cwd.to_string(),
            worktree: opts.worktree.clone(),
            read_only: opts.read_only,
            project_root: opts.project_root.clone(),
            model: model.clone(),
            backend,
            mode: opts.mode,
            status: SessionStatus::Starting,
            approval_mode,
            plan_mode: approval_mode == ApprovalMode::Plan,
            always_approve: approval_mode == ApprovalMode::Yolo,
            sandbox_profile: opts.sandbox_profile.clone(),
            mcp_servers: opts.mcp_server_names.clone(),
            approved_high_risk_mcp: opts.approved_high_risk_mcp.clone(),
            created_at: created_at.unwrap_or(now),
            last_activity: now,
            label: None,
            brain_mode: BrainMode::Fresh,
        };

        let handle = match opts.mode {
            AgentMode::Acp => {
                // Offline / mock threads from memory
                if model.eq_ignore_ascii_case("mock") {
                    let client =
                        AcpClient::mock_for_session(&format!("mock-{id}"), Some(self.event_bus.clone()), id);
                    metadata.acp_session_id = Some(format!("mock-{id}"));
                    metadata.status = SessionStatus::Idle;
                    metadata.label = Some("mock".into());
                    metadata.brain_mode = if connect_opts.transcript_context.is_some() {
                        BrainMode::HistoryOnly
                    } else {
                        BrainMode::Fresh
                    };
                    AgentHandle {
                        metadata,
                        child: None,
                        acp_client: Some(client),
                    }
                } else {
                    let acp_opts = AcpSpawnOptions {
                        model: Some(model),
                        rules: if opts.rules.is_empty() {
                            None
                        } else {
                            Some(json!(opts.rules))
                        },
                        mcp_servers: opts.mcp_servers.clone(),
                        plan_mode: metadata.plan_mode,
                        always_approve: metadata.always_approve,
                        approval_mode,
                        sandbox_profile: opts.sandbox_profile.clone(),
                        extra_env: Vec::new(),
                        deny_patterns,
                        allow_patterns,
                    };
                    let resolved = resolved.expect("resolved backend for live ACP spawn");
                    let desc = descriptor(backend);

                    // Forward backend API-key/env vars from the panel process,
                    // then apply per-backend config env on top.
                    let mut env: Vec<(String, String)> = Vec::new();
                    for key in desc.env_passthrough {
                        if let Ok(v) = std::env::var(key) {
                            if !v.is_empty() {
                                env.push(((*key).to_string(), v));
                            }
                        }
                    }
                    for (k, v) in backend_env_cfg {
                        env.retain(|(ek, _)| ek != &k);
                        env.push((k, v));
                    }
                    // Some claude adapter versions require the var to be defined;
                    // empty means "use the CLI login".
                    if backend == Backend::Claude
                        && !env.iter().any(|(k, _)| k == "ANTHROPIC_API_KEY")
                    {
                        env.push(("ANTHROPIC_API_KEY".into(), String::new()));
                    }

                    let mut client_cfg = AcpClientConfig::new(&resolved.program, cwd_path);
                    client_cfg.args = resolved.args.clone();
                    client_cfg.read_only = opts.read_only;
                    // `grok agent --reasoning-effort <e> stdio`: the flag belongs
                    // to `agent`, so it goes before the `stdio` subcommand.
                    if backend == Backend::Grok {
                        if let Some(effort) = opts.effort.as_deref().filter(|e| !e.is_empty()) {
                            let at = client_cfg
                                .args
                                .iter()
                                .position(|a| a == "stdio")
                                .unwrap_or(client_cfg.args.len());
                            client_cfg
                                .args
                                .splice(at..at, ["--reasoning-effort".to_string(), effort.to_string()]);
                        }
                    }
                    client_cfg.env = env;
                    client_cfg.auth_preference = desc
                        .auth_preference
                        .iter()
                        .map(|s| (*s).to_string())
                        .collect();
                    client_cfg.skip_auth_when_unadvertised = desc.skip_auth_when_unadvertised;
                    client_cfg.backend_label = backend.key().to_string();

                    // Insert a placeholder and announce the thread NOW — the
                    // handshake (spawn + initialize + auth + session/new) can
                    // take many seconds and the UI must not sit blank. The
                    // entry API also makes concurrent resume/start of the same
                    // id spawn exactly one process.
                    match self.sessions.entry(id) {
                        dashmap::mapref::entry::Entry::Occupied(_) => {
                            return Err(CoreError::InvalidOptions(format!(
                                "session {id} is already starting"
                            )));
                        }
                        dashmap::mapref::entry::Entry::Vacant(v) => {
                            v.insert(AgentHandle {
                                metadata: metadata.clone(),
                                child: None,
                                acp_client: None,
                            });
                        }
                    }
                    self.event_bus.emit_session_created(id, cwd, "acp").await;
                    self.event_bus.emit_status(id, SessionStatus::Starting).await;

                    let pending = PendingConnect {
                        client_cfg,
                        acp_opts,
                        connect_opts,
                        effort: opts.effort.clone(),
                    };
                    if background {
                        let sessions = self.sessions.clone();
                        let bus = self.event_bus.clone();
                        tokio::spawn(async move {
                            let _ = connect_and_fill(sessions, bus, id, pending).await;
                        });
                    } else {
                        // Blocking (resume): propagate failure and drop the
                        // placeholder so callers see a clean error.
                        if let Err(e) = connect_and_fill(
                            self.sessions.clone(),
                            self.event_bus.clone(),
                            id,
                            pending,
                        )
                        .await
                        {
                            self.sessions.remove(&id);
                            return Err(e);
                        }
                    }
                    info!(%id, cwd, background, "ACP session spawn initiated");
                    return Ok(());
                }
            }
            AgentMode::Headless => {
                let prompt = opts.prompt.clone().unwrap_or_default();
                let headless = HeadlessSpawnOptions {
                    model: Some(model),
                    worktree: opts.worktree.clone(),
                    always_approve: opts.always_approve,
                    plan_mode: metadata.plan_mode,
                    rules: opts.rules.clone(),
                    sandbox_profile: opts.sandbox_profile.clone(),
                    timeout_secs: None,
                };
                let child = self
                    .grok_cli
                    .spawn_headless(cwd_path, &prompt, &headless)
                    .await?;
                metadata.status = SessionStatus::Running;
                AgentHandle {
                    metadata,
                    child: Some(tokio::sync::Mutex::new(child)),
                    acp_client: None,
                }
            }
        };

        let mode_str = match opts.mode {
            AgentMode::Acp => "acp",
            AgentMode::Headless => "headless",
        };
        self.event_bus
            .emit_session_created(id, cwd, mode_str)
            .await;
        self.event_bus
            .emit_status(id, handle.metadata.status)
            .await;

        info!(%id, mode = mode_str, cwd, "session spawned");
        self.sessions.insert(id, handle);
        Ok(())
    }

    /// Spawn a mock ACP session for tests / offline UI development.
    pub async fn spawn_mock(&self, cwd: &str) -> Result<Uuid> {
        let id = Uuid::new_v4();
        let opts = SpawnOptions {
            model: Some("mock".into()),
            mode: AgentMode::Acp,
            ..SpawnOptions::default()
        };
        self.spawn_agent_with_id(id, cwd, opts, None, ConnectOpts::default(), false)
            .await?;
        Ok(id)
    }

    /// Status events may arrive before connect_and_fill installs the ACP handle.
    pub fn is_ready(&self, id: Uuid) -> bool {
        self.sessions.get(&id).is_some_and(|entry| entry.metadata.mode != AgentMode::Acp || entry.acp_client.is_some())
    }

    pub fn is_live(&self, id: Uuid) -> bool {
        self.sessions.contains_key(&id)
    }

    pub async fn brain_mode(&self, id: Uuid) -> Option<BrainMode> {
        let c = self.sessions.get(&id)?.acp_client.clone()?;
        Some(c.brain_mode().await)
    }

    /// Whether the live agent accepts image prompt blocks. `None` when the
    /// thread is not live (no client to ask).
    pub async fn image_prompts_supported(&self, id: Uuid) -> Option<bool> {
        let c = self.sessions.get(&id)?.acp_client.clone()?;
        Some(c.image_prompts_supported().await)
    }

    pub fn list_sessions(&self) -> Vec<SessionMetadata> {
        self.sessions
            .iter()
            .map(|e| e.value().metadata.clone())
            .collect()
    }

    pub fn get_snapshot(&self, id: Uuid) -> Result<AgentHandleSnapshot> {
        self.sessions
            .get(&id)
            .map(|h| h.snapshot())
            .ok_or(CoreError::SessionNotFound(id))
    }

    pub async fn send_prompt(&self, id: Uuid, prompt: &str) -> Result<()> {
        self.send_prompt_with_images(id, prompt, &[]).await
    }

    pub async fn send_prompt_with_images(
        &self,
        id: Uuid,
        prompt: &str,
        images: &[grok_acp::PromptImage],
    ) -> Result<()> {
        let client = {
            let mut entry = self
                .sessions
                .get_mut(&id)
                .ok_or(CoreError::SessionNotFound(id))?;
            if entry.acp_client.is_none()
                && matches!(entry.metadata.status, SessionStatus::Starting)
            {
                return Err(CoreError::InvalidOptions(
                    "session is still starting — wait for it to become idle".into(),
                ));
            }
            entry.touch();
            entry.metadata.status = SessionStatus::Running;
            entry
                .acp_client
                .clone()
                .ok_or(CoreError::NotAcp)?
        };
        self.event_bus
            .emit_status(id, SessionStatus::Running)
            .await;
        client.send_prompt_with_images(prompt, images).await?;
        Ok(())
    }

    pub async fn cancel_session(&self, id: Uuid) -> Result<()> {
        let (acp, has_child) = {
            let mut entry = self
                .sessions
                .get_mut(&id)
                .ok_or(CoreError::SessionNotFound(id))?;
            entry.metadata.status = SessionStatus::Cancelling;
            (entry.acp_client.clone(), entry.child.is_some())
        };

        if let Some(client) = acp {
            client.cancel().await?;
        }
        if has_child {
            if let Some(mut entry) = self.sessions.get_mut(&id) {
                if let Some(child) = entry.child.take() {
                    let mut c = child.lock().await;
                    let _ = c.kill().await;
                }
            }
        }

        if let Some(mut entry) = self.sessions.get_mut(&id) {
            entry.metadata.status = SessionStatus::Cancelled;
            entry.touch();
        }
        self.event_bus.emit_session_cancelled(id).await;
        Ok(())
    }

    pub async fn set_plan_mode(&self, id: Uuid, enabled: bool) -> Result<()> {
        let client = {
            let mut entry = self
                .sessions
                .get_mut(&id)
                .ok_or(CoreError::SessionNotFound(id))?;
            entry.metadata.plan_mode = enabled;
            if enabled {
                entry.metadata.always_approve = false;
            }
            entry.touch();
            entry.acp_client.clone()
        };
        if let Some(client) = client {
            let mode = if enabled { "plan" } else { "default" };
            client.set_mode(mode).await?;
        }
        Ok(())
    }

    /// Switch a live session's approval stance (composer pills).
    /// Change reasoning effort on a live session, where the agent exposes an
    /// `effort` config option. Ok(false) when it does not.
    pub async fn backend_model_catalog(&self, backend: Backend) -> Option<grok_acp::ModelCatalog> {
        let clients: Vec<_> = self.sessions.iter().filter(|entry| entry.metadata.backend == backend).filter_map(|entry| entry.acp_client.clone()).collect();
        for client in clients { let catalog = client.model_catalog().await; if !catalog.models.is_empty() { return Some(catalog); } }
        None
    }

    pub async fn speed_option(&self, id: Uuid) -> Result<Option<(String, Vec<String>, Option<String>)>> {
        let client = self.sessions.get(&id).and_then(|e| e.acp_client.clone()).ok_or(CoreError::SessionNotFound(id))?;
        Ok(client.speed_option().await)
    }

    pub async fn set_speed_option(&self, id: Uuid, option: &str, value: &str) -> Result<bool> {
        let client = self.sessions.get(&id).and_then(|e| e.acp_client.clone()).ok_or(CoreError::SessionNotFound(id))?;
        let Some((advertised, values, _)) = client.speed_option().await else { return Ok(false); };
        if advertised != option || !values.iter().any(|v| v == value) { return Ok(false); }
        client.set_config_option(option, value).await.map_err(Into::into)
    }

    pub async fn set_effort(&self, id: Uuid, effort: &str) -> Result<bool> {
        let client = self
            .sessions
            .get(&id)
            .and_then(|e| e.acp_client.clone())
            .ok_or(CoreError::SessionNotFound(id))?;
        client.set_effort(effort).await.map_err(Into::into)
    }

    pub async fn set_approval_mode(&self, id: Uuid, mode: ApprovalMode) -> Result<()> {
        let client = self.sessions.get(&id).ok_or(CoreError::SessionNotFound(id))?.acp_client.clone();
        if let Some(client) = client {
            let wanted = match mode {
                ApprovalMode::Plan => "plan",
                ApprovalMode::Auto => "auto",
                ApprovalMode::Yolo => "always_approve",
                ApprovalMode::Ask => "default",
            };
            // Do not claim a mode changed when the provider rejected it.
            client.set_mode(wanted).await?;
            client.set_approval_mode(mode).await;
        }
        let mut entry = self.sessions.get_mut(&id).ok_or(CoreError::SessionNotFound(id))?;
        entry.metadata.approval_mode = mode;
        entry.metadata.plan_mode = mode == ApprovalMode::Plan;
        entry.metadata.always_approve = mode == ApprovalMode::Yolo;
        entry.touch();
        Ok(())
    }

    /// Record an "always allow this" rule for the rest of the session.
    pub async fn add_session_allow_rule(&self, id: Uuid, pattern: String) -> Result<()> {
        let client = self
            .sessions
            .get(&id)
            .ok_or(CoreError::SessionNotFound(id))?
            .acp_client
            .clone()
            .ok_or(CoreError::NotAcp)?;
        client.add_session_allow_rule(pattern).await;
        Ok(())
    }

    pub async fn set_always_approve(&self, id: Uuid, enabled: bool) -> Result<()> {
        let client = {
            let mut entry = self
                .sessions
                .get_mut(&id)
                .ok_or(CoreError::SessionNotFound(id))?;
            entry.metadata.always_approve = enabled;
            if enabled {
                entry.metadata.plan_mode = false;
            }
            entry.touch();
            entry.acp_client.clone()
        };
        if let Some(client) = client {
            // Client-side gating is the real mechanism; agent-side mode is
            // best-effort (not every agent advertises a bypass mode).
            client.set_always_approve(enabled);
            let mode = if enabled { "always_approve" } else { "default" };
            if let Err(e) = client.set_mode(mode).await {
                tracing::warn!(error = %e, enabled, "agent-side always-approve mode not applied");
            }
        }
        Ok(())
    }

    pub async fn respond_approval(
        &self,
        id: Uuid,
        request_id: &str,
        option_id: Option<&str>,
    ) -> Result<()> {
        let client = self
            .sessions
            .get(&id)
            .ok_or(CoreError::SessionNotFound(id))?
            .acp_client
            .clone()
            .ok_or(CoreError::NotAcp)?;
        client.respond_approval(request_id, option_id).await?;
        if let Some(mut entry) = self.sessions.get_mut(&id) {
            entry.metadata.status = SessionStatus::Running;
            entry.touch();
        }
        Ok(())
    }

    /// Retire an idle provider during a model switch without cancelling the new UI turn.
    pub async fn retire_session(&self, id: Uuid) -> Result<()> {
        if let Some((_, handle)) = self.sessions.remove(&id) {
            if let Some(client) = handle.acp_client { client.shutdown().await?; }
        }
        Ok(())
    }

    pub async fn remove_session(&self, id: Uuid) -> Result<()> {
        let _ = self.cancel_session(id).await;
        // cancel() only sends session/cancel — the grok child (and any MCP
        // servers it spawned) keeps running unless we kill it.
        if let Some((_, handle)) = self.sessions.remove(&id) {
            if let Some(client) = handle.acp_client {
                let _ = client.shutdown().await;
            }
        }
        Ok(())
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    pub async fn shutdown_all(&self) {
        let ids: Vec<Uuid> = self.sessions.iter().map(|e| *e.key()).collect();
        for id in ids {
            if let Err(e) = self.cancel_session(id).await {
                warn!(%id, error = %e, "error cancelling session during shutdown");
            }
            if let Some((_, handle)) = self.sessions.remove(&id) {
                if let Some(client) = handle.acp_client {
                    let _ = client.shutdown().await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grok_cli_wrapper::GrokCli;
    use grok_events::shared_bus;
    use std::path::PathBuf;

    fn test_registry() -> Arc<SessionRegistry> {
        let bus = shared_bus();
        let cfg = Arc::new(tokio::sync::RwLock::new(GrokConfig::default()));
        let cli = Arc::new(GrokCli::new(PathBuf::from("/bin/true")));
        SessionRegistry::new(bus, cfg, cli)
    }

    #[tokio::test]
    async fn readiness_requires_installed_client_and_retirement_does_not_cancel() {
        let reg = test_registry();
        let id = reg.spawn_mock("/tmp").await.unwrap();
        assert!(reg.is_ready(id));
        let client = reg.sessions.get_mut(&id).unwrap().acp_client.take();
        reg.sessions.get_mut(&id).unwrap().metadata.status = SessionStatus::Idle;
        assert!(!reg.is_ready(id));
        reg.sessions.get_mut(&id).unwrap().acp_client = client;
        let mut events = reg.event_bus.subscribe();
        reg.retire_session(id).await.unwrap();
        assert!(!reg.is_live(id));
        while let Ok(event) = events.try_recv() {
            assert!(!matches!(event, grok_events::ControlEvent::SessionCancelled { .. } | grok_events::ControlEvent::SessionStatusChanged { status: SessionStatus::Cancelled, .. }));
        }
    }

    #[tokio::test]
    async fn mock_session_lifecycle() {
        let reg = test_registry();
        let id = reg.spawn_mock("/tmp").await.unwrap();
        assert_eq!(reg.session_count(), 1);
        let snap = reg.get_snapshot(id).unwrap();
        assert_eq!(snap.metadata.cwd, "/tmp");
        assert_eq!(snap.metadata.mode, AgentMode::Acp);
        reg.cancel_session(id).await.unwrap();
        assert_eq!(
            reg.get_snapshot(id).unwrap().metadata.status,
            SessionStatus::Cancelled
        );
        reg.remove_session(id).await.unwrap();
        assert_eq!(reg.session_count(), 0);
    }

    #[tokio::test]
    async fn list_sessions() {
        let reg = test_registry();
        let _a = reg.spawn_mock("/tmp").await.unwrap();
        let _b = reg.spawn_mock("/tmp").await.unwrap();
        assert_eq!(reg.list_sessions().len(), 2);
    }

    #[test]
    fn spawn_options_validate_headless() {
        let mut opts = SpawnOptions {
            mode: AgentMode::Headless,
            prompt: None,
            ..Default::default()
        };
        assert!(opts.validate().is_err());
        opts.prompt = Some("do thing".into());
        assert!(opts.validate().is_ok());
    }
}
