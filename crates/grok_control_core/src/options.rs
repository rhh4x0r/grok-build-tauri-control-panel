use grok_config::Backend;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    #[default]
    Acp,
    Headless,
}

// Note: AgentMode accepts "acp" / "headless" (snake). Frontend may send same.

pub use grok_acp::ApprovalMode;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SpawnOptions {
    /// Which agent backend to spawn (grok | claude | codex).
    pub backend: Backend,
    pub model: Option<String>,
    pub worktree: Option<String>,
    pub mode: AgentMode,
    pub prompt: Option<String>,
    pub rules: Vec<String>,
    /// Approval stance: plan | ask | auto | yolo. The two booleans below are
    /// legacy inputs (older payloads / persisted metadata) — `resolved_mode()`
    /// reconciles them.
    pub approval_mode: Option<ApprovalMode>,
    pub always_approve: bool,
    pub plan_mode: bool,
    pub sandbox_profile: Option<String>,
    /// Raw ACP mcpServers payloads (advanced). Prefer `mcp_server_names`.
    pub mcp_servers: Vec<Value>,
    /// Names of configured MCP servers to attach (resolved by McpManager).
    pub mcp_server_names: Vec<String>,
    /// High-risk MCP servers explicitly approved for this spawn.
    pub approved_high_risk_mcp: Vec<String>,
    /// Include servers with `auto_attach = true`.
    pub include_auto_mcp: bool,
    pub permission_allow: Vec<String>,
    pub permission_deny: Vec<String>,
    pub trust_repo: bool,
    /// Give this thread its own git worktree when the project is a repo.
    pub isolate_worktree: bool,
    /// The real project folder (thread cwd may be a worktree derived from it).
    pub project_root: Option<String>,
    /// Reasoning effort (low | medium | high) for backends that take one;
    /// Grok passes it as `--reasoning-effort` to `grok agent stdio`.
    pub effort: Option<String>,
}

impl Default for SpawnOptions {
    fn default() -> Self {
        Self {
            backend: Backend::Grok,
            model: None,
            worktree: None,
            mode: AgentMode::Acp,
            prompt: None,
            rules: Vec::new(),
            approval_mode: None,
            always_approve: false,
            // No stance by default — every request is confirmed until the user
            // picks plan/auto/yolo.
            plan_mode: false,
            sandbox_profile: Some("workspace".into()),
            mcp_servers: Vec::new(),
            mcp_server_names: Vec::new(),
            approved_high_risk_mcp: Vec::new(),
            include_auto_mcp: true,
            permission_allow: Vec::new(),
            permission_deny: Vec::new(),
            trust_repo: false,
            isolate_worktree: true,
            project_root: None,
            effort: None,
        }
    }
}

impl SpawnOptions {
    pub fn validate(&self) -> Result<(), String> {
        if matches!(self.mode, AgentMode::Headless) && self.prompt.as_ref().map(|p| p.trim().is_empty()).unwrap_or(true)
        {
            return Err("headless mode requires a non-empty prompt".into());
        }
        if matches!(self.mode, AgentMode::Headless) && self.backend != Backend::Grok {
            return Err(format!(
                "headless mode is grok-only (got backend '{}')",
                self.backend
            ));
        }
        if self.always_approve && self.plan_mode {
            // Explicit always_approve wins; plan_mode soft-disabled
        }
        Ok(())
    }

    /// The stance to run with: an explicit `approval_mode` wins; otherwise
    /// derive it from the legacy booleans (old payloads / restored metadata).
    pub fn resolved_mode(&self) -> ApprovalMode {
        if let Some(m) = self.approval_mode {
            return m;
        }
        if self.always_approve {
            ApprovalMode::Yolo
        } else if self.plan_mode {
            ApprovalMode::Plan
        } else {
            ApprovalMode::Ask
        }
    }
}
