//! Extended MCP server configuration types.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use grok_config::McpServerConfig;

/// Transport for MCP servers.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    #[default]
    Stdio,
    Http,
    Sse,
}

impl McpTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
            Self::Sse => "sse",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "http" => Self::Http,
            "sse" => Self::Sse,
            _ => Self::Stdio,
        }
    }
}

/// Scope of an MCP server registration.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpScope {
    #[default]
    Global,
    Project,
    Session,
}

/// Extended MCP config used by the control panel (superset of `grok_config::McpServerConfig`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct McpServerConfigExt {
    pub name: String,
    pub transport: McpTransport,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub env: HashMap<String, String>,
    pub enabled: bool,
    pub scope: McpScope,
    pub kind: String,
    pub description: Option<String>,
    pub startup_timeout_sec: u64,
    pub tool_timeout_sec: u64,
    /// Paths attached for filesystem MCP.
    pub allowed_paths: Vec<PathBuf>,
    pub read_only: bool,
    /// Requires explicit user approval before attaching (browser, custom, etc.).
    pub requires_approval: bool,
    /// High-risk flag for UI warning banners.
    pub high_risk: bool,
    /// Credential keys referenced (never store secrets here).
    pub credential_keys: Vec<String>,
    /// Auto-attach when spawning sessions in matching projects.
    pub auto_attach: bool,
    /// Extra headers for HTTP transport (values may be env var refs).
    pub headers: HashMap<String, String>,
    /// Rate limit for recursive/self-referential servers (calls/min).
    pub rate_limit_per_min: Option<u32>,
}

impl Default for McpServerConfigExt {
    fn default() -> Self {
        Self {
            name: String::new(),
            transport: McpTransport::Stdio,
            command: None,
            args: Vec::new(),
            url: None,
            env: HashMap::new(),
            enabled: true,
            scope: McpScope::Global,
            kind: "custom".into(),
            description: None,
            startup_timeout_sec: 60,
            tool_timeout_sec: 120,
            allowed_paths: Vec::new(),
            read_only: false,
            requires_approval: false,
            high_risk: false,
            credential_keys: Vec::new(),
            auto_attach: false,
            headers: HashMap::new(),
            rate_limit_per_min: None,
        }
    }
}

impl McpServerConfigExt {
    /// Convert to the slim config stored in `GrokConfig.mcp_servers`.
    pub fn to_config_entry(&self) -> McpServerConfig {
        let mut env = self.env.clone();
        // Persist panel metadata under reserved keys for round-trip.
        env.insert("_panel_kind".into(), self.kind.clone());
        env.insert("_panel_transport".into(), self.transport.as_str().into());
        env.insert(
            "_panel_scope".into(),
            match self.scope {
                McpScope::Global => "global",
                McpScope::Project => "project",
                McpScope::Session => "session",
            }
            .into(),
        );
        env.insert(
            "_panel_startup_timeout".into(),
            self.startup_timeout_sec.to_string(),
        );
        env.insert(
            "_panel_tool_timeout".into(),
            self.tool_timeout_sec.to_string(),
        );
        env.insert(
            "_panel_read_only".into(),
            self.read_only.to_string(),
        );
        env.insert(
            "_panel_requires_approval".into(),
            self.requires_approval.to_string(),
        );
        env.insert(
            "_panel_high_risk".into(),
            self.high_risk.to_string(),
        );
        env.insert(
            "_panel_auto_attach".into(),
            self.auto_attach.to_string(),
        );
        if let Some(ref d) = self.description {
            env.insert("_panel_description".into(), d.clone());
        }
        if !self.allowed_paths.is_empty() {
            let joined = self
                .allowed_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("|");
            env.insert("_panel_allowed_paths".into(), joined);
        }
        if let Some(r) = self.rate_limit_per_min {
            env.insert("_panel_rate_limit".into(), r.to_string());
        }
        for (k, v) in &self.headers {
            env.insert(format!("_panel_header_{k}"), v.clone());
        }
        for key in &self.credential_keys {
            env.insert(format!("_panel_cred_{key}"), "1".into());
        }

        McpServerConfig {
            command: self.command.clone(),
            args: self.args.clone(),
            url: self.url.clone(),
            enabled: self.enabled,
            env,
        }
    }

    /// Reconstruct extended config from a stored name + slim entry.
    pub fn from_config_entry(name: &str, cfg: &McpServerConfig) -> Self {
        let env = &cfg.env;
        let transport = env
            .get("_panel_transport")
            .map(|s| McpTransport::from_str_lossy(s))
            .unwrap_or_else(|| {
                if cfg.url.is_some() {
                    McpTransport::Http
                } else {
                    McpTransport::Stdio
                }
            });
        let scope = match env.get("_panel_scope").map(String::as_str) {
            Some("project") => McpScope::Project,
            Some("session") => McpScope::Session,
            _ => McpScope::Global,
        };
        let kind = env
            .get("_panel_kind")
            .cloned()
            .unwrap_or_else(|| "custom".into());
        let allowed_paths = env
            .get("_panel_allowed_paths")
            .map(|s| {
                s.split('|')
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from)
                    .collect()
            })
            .unwrap_or_default();
        let mut clean_env = HashMap::new();
        let mut headers = HashMap::new();
        let mut credential_keys = Vec::new();
        for (k, v) in env {
            if let Some(h) = k.strip_prefix("_panel_header_") {
                headers.insert(h.to_string(), v.clone());
            } else if let Some(c) = k.strip_prefix("_panel_cred_") {
                credential_keys.push(c.to_string());
            } else if !k.starts_with("_panel_") {
                clean_env.insert(k.clone(), v.clone());
            }
        }

        Self {
            name: name.to_string(),
            transport,
            command: cfg.command.clone(),
            args: cfg.args.clone(),
            url: cfg.url.clone(),
            env: clean_env,
            enabled: cfg.enabled,
            scope,
            kind,
            description: env.get("_panel_description").cloned(),
            startup_timeout_sec: env
                .get("_panel_startup_timeout")
                .and_then(|s| s.parse().ok())
                .unwrap_or(60),
            tool_timeout_sec: env
                .get("_panel_tool_timeout")
                .and_then(|s| s.parse().ok())
                .unwrap_or(120),
            allowed_paths,
            read_only: env
                .get("_panel_read_only")
                .map(|s| s == "true")
                .unwrap_or(false),
            requires_approval: env
                .get("_panel_requires_approval")
                .map(|s| s == "true")
                .unwrap_or(false),
            high_risk: env
                .get("_panel_high_risk")
                .map(|s| s == "true")
                .unwrap_or(false),
            credential_keys,
            auto_attach: env
                .get("_panel_auto_attach")
                .map(|s| s == "true")
                .unwrap_or(false),
            headers,
            rate_limit_per_min: env
                .get("_panel_rate_limit")
                .and_then(|s| s.parse().ok()),
        }
    }

    /// JSON-RPC / ACP payload fragment for session/new mcpServers.
    /// Carries real (resolved) env values — this goes to the agent process.
    pub fn to_acp_payload(&self) -> serde_json::Value {
        match self.transport {
            McpTransport::Stdio => {
                serde_json::json!({
                    "name": self.name,
                    "command": self.command,
                    "args": self.args,
                    "env": strip_panel_keys(&self.env),
                })
            }
            McpTransport::Http | McpTransport::Sse => {
                serde_json::json!({
                    "name": self.name,
                    "url": self.url,
                    "headers": self.headers,
                    "env": strip_panel_keys(&self.env),
                })
            }
        }
    }
}

/// Drop internal `_panel_*` bookkeeping keys before spawning a server.
fn strip_panel_keys(env: &HashMap<String, String>) -> HashMap<String, String> {
    env.iter()
        .filter(|(k, _)| !k.starts_with("_panel_"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Mask secret-looking values for anything shown to the webview/UI.
pub fn mask_payload_for_preview(payload: &serde_json::Value) -> serde_json::Value {
    use crate::security::{looks_like_secret_key, mask_secret};
    let mut out = payload.clone();
    for field in ["env", "headers"] {
        if let Some(map) = out.get_mut(field).and_then(|v| v.as_object_mut()) {
            for (k, v) in map.iter_mut() {
                if let Some(s) = v.as_str() {
                    if looks_like_secret_key(k) || field == "headers" && k.eq_ignore_ascii_case("authorization") {
                        *v = serde_json::Value::String(mask_secret(s));
                    }
                }
            }
        }
    }
    out
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddMcpRequest {
    pub name: String,
    pub kind: Option<String>,
    pub transport: Option<String>,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub url: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub enabled: Option<bool>,
    pub scope: Option<String>,
    pub description: Option<String>,
    pub allowed_paths: Option<Vec<String>>,
    pub read_only: Option<bool>,
    pub auto_attach: Option<bool>,
    pub requires_approval: Option<bool>,
    /// Use a built-in catalog template id (filesystem, github, ...).
    pub from_catalog: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub startup_timeout_sec: Option<u64>,
    pub tool_timeout_sec: Option<u64>,
    pub rate_limit_per_min: Option<u32>,
    pub credential_keys: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMcpRequest {
    pub name: String,
    pub enabled: Option<bool>,
    pub args: Option<Vec<String>>,
    pub url: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub allowed_paths: Option<Vec<String>>,
    pub read_only: Option<bool>,
    pub auto_attach: Option<bool>,
    pub description: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    pub startup_timeout_sec: Option<u64>,
    pub tool_timeout_sec: Option<u64>,
    pub rate_limit_per_min: Option<u32>,
}
