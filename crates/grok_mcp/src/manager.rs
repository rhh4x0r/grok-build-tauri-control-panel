//! McpManager: CRUD, doctor, tools listing, session payload building.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::{info, warn};

use grok_cli_wrapper::GrokCli;
use grok_config::{GrokConfig, GrokPaths};
use grok_events::{ControlEvent, EventBus};

use crate::catalog::{builtin_catalog, catalog_entry, McpCatalogEntry};
use crate::credentials::CredentialStore;
use crate::injection::{build_session_mcp_payload, resolve_attachments, SkippedMcp};
use crate::security::{validate_server, SecurityVerdict, validate_custom_server};
use crate::types::{
    AddMcpRequest, McpScope, McpServerConfigExt, McpTransport, UpdateMcpRequest,
};

#[derive(Debug, Error)]
pub enum McpError {
    #[error("config error: {0}")]
    Config(#[from] grok_config::ConfigError),
    #[error("cli error: {0}")]
    Cli(#[from] grok_cli_wrapper::CliError),
    #[error("credential error: {0}")]
    Cred(#[from] crate::credentials::CredentialError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("security: {0}")]
    Security(String),
}

pub type Result<T> = std::result::Result<T, McpError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    Ok,
    Warn,
    Error,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub name: String,
    pub status: DoctorStatus,
    pub messages: Vec<String>,
    pub checked_at: DateTime<Utc>,
    pub cli_output: Option<String>,
}

/// Outcome of pre-spawn MCP resolution for a session.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionMcpResolution {
    /// ACP `mcpServers` payload for the servers that passed all checks.
    pub payload: Vec<serde_json::Value>,
    pub attached_names: Vec<String>,
    pub skipped: Vec<SkippedMcp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub server: String,
    pub name: String,
    pub description: Option<String>,
    pub namespace: String,
}

pub struct McpManager {
    config: Arc<RwLock<GrokConfig>>,
    paths: GrokPaths,
    grok_cli: Arc<GrokCli>,
    event_bus: Arc<EventBus>,
    credentials: CredentialStore,
    prefer_cli: bool,
}

impl McpManager {
    pub fn new(
        config: Arc<RwLock<GrokConfig>>,
        paths: GrokPaths,
        grok_cli: Arc<GrokCli>,
        event_bus: Arc<EventBus>,
    ) -> Result<Arc<Self>> {
        let cred_path = CredentialStore::default_path(&paths.bomb_dir);
        let credentials = CredentialStore::open(cred_path)?;
        Ok(Arc::new(Self {
            config,
            paths,
            grok_cli,
            event_bus,
            credentials,
            prefer_cli: true,
        }))
    }

    pub fn set_prefer_cli(&mut self, prefer: bool) {
        self.prefer_cli = prefer;
    }

    pub fn catalog(&self) -> Vec<McpCatalogEntry> {
        builtin_catalog()
    }

    pub async fn list(&self) -> Vec<McpServerConfigExt> {
        let cfg = self.config.read().await;
        cfg.mcp_servers
            .iter()
            .map(|(name, entry)| McpServerConfigExt::from_config_entry(name, entry))
            .collect()
    }

    pub async fn get(&self, name: &str) -> Result<McpServerConfigExt> {
        let cfg = self.config.read().await;
        let entry = cfg
            .mcp_servers
            .get(name)
            .ok_or_else(|| McpError::NotFound(name.to_string()))?;
        Ok(McpServerConfigExt::from_config_entry(name, entry))
    }

    pub async fn add(&self, req: AddMcpRequest) -> Result<McpServerConfigExt> {
        let mut server = if let Some(ref catalog_id) = req.from_catalog {
            let entry = catalog_entry(catalog_id)
                .ok_or_else(|| McpError::Invalid(format!("unknown catalog id: {catalog_id}")))?;
            let paths = req
                .allowed_paths
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(PathBuf::from)
                .collect();
            entry.instantiate(Some(req.name.clone()), paths, None)
        } else {
            self.request_to_config(&req)?
        };

        // Apply remaining request overrides
        if let Some(t) = req.transport.as_deref() {
            server.transport = McpTransport::from_str_lossy(t);
        }
        if let Some(c) = req.command {
            server.command = Some(c);
        }
        if let Some(a) = req.args {
            // For filesystem from catalog, paths already in args; merge carefully
            if server.kind != "filesystem" || server.args.is_empty() {
                server.args = a;
            }
        }
        if let Some(u) = req.url {
            server.url = Some(u);
        }
        if let Some(e) = req.env {
            server.env.extend(e);
        }
        if let Some(h) = req.headers {
            server.headers.extend(h);
        }
        if let Some(en) = req.enabled {
            server.enabled = en;
        }
        if let Some(d) = req.description {
            server.description = Some(d);
        }
        if let Some(ro) = req.read_only {
            server.read_only = ro;
        }
        if let Some(aa) = req.auto_attach {
            server.auto_attach = aa;
        }
        if let Some(ra) = req.requires_approval {
            server.requires_approval = ra;
        }
        if let Some(st) = req.startup_timeout_sec {
            server.startup_timeout_sec = st;
        }
        if let Some(tt) = req.tool_timeout_sec {
            server.tool_timeout_sec = tt;
        }
        if let Some(rl) = req.rate_limit_per_min {
            server.rate_limit_per_min = Some(rl);
        }
        if let Some(ck) = req.credential_keys {
            server.credential_keys = ck;
        }
        if let Some(paths) = req.allowed_paths {
            if server.kind == "filesystem" {
                server.allowed_paths = paths.iter().map(PathBuf::from).collect();
                // Ensure args include package + paths
                if !server.args.iter().any(|a| a.contains("server-filesystem")) {
                    server.args = vec![
                        "-y".into(),
                        "@modelcontextprotocol/server-filesystem".into(),
                    ];
                }
                for p in &server.allowed_paths {
                    let s = p.display().to_string();
                    if !server.args.iter().any(|a| a == &s) {
                        server.args.push(s);
                    }
                }
            }
        }
        if let Some(scope) = req.scope.as_deref() {
            server.scope = match scope {
                "project" => McpScope::Project,
                "session" => McpScope::Session,
                _ => McpScope::Global,
            };
        }

        validate_server(&server).map_err(McpError::Security)?;

        // Save the panel config first — the CLI mirror is secondary, and a
        // failed save must not leave the CLI with a server the panel can't see.
        {
            let mut cfg = self.config.write().await;
            cfg.set_mcp_server(server.name.clone(), server.to_config_entry());
            cfg.save(&self.paths.config_file)?;
        }

        // Optional CLI mirror (only when enabled — a disabled server must not
        // load in terminal `grok` sessions).
        if self.prefer_cli && server.enabled {
            if let Err(e) = self.cli_add(&server).await {
                warn!(error = %e, name = %server.name, "grok mcp add failed; config-only");
            }
        }

        self.event_bus.emit(ControlEvent::McpChanged {
            name: server.name.clone(),
            enabled: server.enabled,
            at: Utc::now(),
        });
        info!(name = %server.name, kind = %server.kind, "MCP server added");
        Ok(server)
    }

    fn request_to_config(&self, req: &AddMcpRequest) -> Result<McpServerConfigExt> {
        let kind = req
            .kind
            .clone()
            .unwrap_or_else(|| "custom".into());
        let transport = req
            .transport
            .as_deref()
            .map(McpTransport::from_str_lossy)
            .unwrap_or(McpTransport::Stdio);
        Ok(McpServerConfigExt {
            name: req.name.clone(),
            transport,
            command: req.command.clone(),
            args: req.args.clone().unwrap_or_default(),
            url: req.url.clone(),
            env: req.env.clone().unwrap_or_default(),
            enabled: req.enabled.unwrap_or(true),
            scope: McpScope::Global,
            kind,
            description: req.description.clone(),
            startup_timeout_sec: req.startup_timeout_sec.unwrap_or(60),
            tool_timeout_sec: req.tool_timeout_sec.unwrap_or(120),
            allowed_paths: req
                .allowed_paths
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            read_only: req.read_only.unwrap_or(false),
            requires_approval: req.requires_approval.unwrap_or(false),
            high_risk: false,
            credential_keys: req.credential_keys.clone().unwrap_or_default(),
            auto_attach: req.auto_attach.unwrap_or(false),
            headers: req.headers.clone().unwrap_or_default(),
            rate_limit_per_min: req.rate_limit_per_min,
        })
    }

    async fn cli_add(&self, server: &McpServerConfigExt) -> Result<()> {
        match server.transport {
            McpTransport::Stdio => {
                let cmd = server
                    .command
                    .as_deref()
                    .ok_or_else(|| McpError::Invalid("missing command".into()))?;
                let arg_refs: Vec<&str> = server.args.iter().map(String::as_str).collect();
                // Resolve credentials so the CLI-side copy can authenticate;
                // unresolved placeholders must not be mirrored (they'd fail
                // opaquely in every terminal grok session).
                let env = self.credentials.resolve_env(&server.env)?;
                let mut env_pairs: Vec<(String, String)> =
                    env.into_iter().filter(|(k, _)| !k.starts_with("_panel_")).collect();
                env_pairs.sort();
                self.grok_cli
                    .mcp_add(&server.name, cmd, &arg_refs, &env_pairs)
                    .await?;
            }
            McpTransport::Http | McpTransport::Sse => {
                let url = server
                    .url
                    .as_deref()
                    .ok_or_else(|| McpError::Invalid("missing url".into()))?;
                let headers = self.credentials.resolve_env(&server.headers)?;
                let mut header_pairs: Vec<(String, String)> = headers.into_iter().collect();
                header_pairs.sort();
                self.grok_cli
                    .mcp_add_http(&server.name, url, server.transport.as_str(), &header_pairs)
                    .await?;
            }
        }
        Ok(())
    }

    /// Mirror the enabled/disabled state into the grok CLI's global registry.
    /// The CLI has no disable flag, so disable = remove, enable = re-add.
    async fn cli_sync_enabled(&self, server: &McpServerConfigExt) {
        if !self.prefer_cli {
            return;
        }
        let res = if server.enabled {
            self.cli_add(server).await
        } else {
            self.grok_cli
                .mcp_remove(&server.name)
                .await
                .map(|_| ())
                .map_err(McpError::Cli)
        };
        if let Err(e) = res {
            warn!(error = %e, name = %server.name, enabled = server.enabled,
                "grok CLI mirror sync failed; panel config still updated");
        }
    }

    pub async fn update(&self, req: UpdateMcpRequest) -> Result<McpServerConfigExt> {
        let mut server = self.get(&req.name).await?;
        if let Some(en) = req.enabled {
            server.enabled = en;
        }
        if let Some(a) = req.args {
            server.args = a;
        }
        if let Some(u) = req.url {
            server.url = Some(u);
        }
        if let Some(e) = req.env {
            server.env.extend(e);
        }
        if let Some(h) = req.headers {
            server.headers.extend(h);
        }
        if let Some(paths) = req.allowed_paths {
            server.allowed_paths = paths.into_iter().map(PathBuf::from).collect();
        }
        if let Some(ro) = req.read_only {
            server.read_only = ro;
        }
        if let Some(aa) = req.auto_attach {
            server.auto_attach = aa;
        }
        if let Some(d) = req.description {
            server.description = Some(d);
        }
        if let Some(st) = req.startup_timeout_sec {
            server.startup_timeout_sec = st;
        }
        if let Some(tt) = req.tool_timeout_sec {
            server.tool_timeout_sec = tt;
        }
        if let Some(rl) = req.rate_limit_per_min {
            server.rate_limit_per_min = Some(rl);
        }
        validate_server(&server).map_err(McpError::Security)?;
        {
            let mut cfg = self.config.write().await;
            cfg.set_mcp_server(server.name.clone(), server.to_config_entry());
            cfg.save(&self.paths.config_file)?;
        }
        // Keep the grok CLI's global registry in sync — without this, a
        // panel-disabled server keeps loading in every terminal session.
        self.cli_sync_enabled(&server).await;
        self.event_bus.emit(ControlEvent::McpChanged {
            name: server.name.clone(),
            enabled: server.enabled,
            at: Utc::now(),
        });
        Ok(server)
    }

    pub async fn remove(&self, name: &str) -> Result<()> {
        if self.prefer_cli {
            let _ = self.grok_cli.mcp_remove(name).await;
        }
        {
            let mut cfg = self.config.write().await;
            cfg.remove_mcp_server(name)
                .ok_or_else(|| McpError::NotFound(name.to_string()))?;
            cfg.save(&self.paths.config_file)?;
        }
        self.event_bus.emit(ControlEvent::McpChanged {
            name: name.to_string(),
            enabled: false,
            at: Utc::now(),
        });
        Ok(())
    }

    pub async fn set_enabled(&self, name: &str, enabled: bool) -> Result<()> {
        self.update(UpdateMcpRequest {
            name: name.to_string(),
            enabled: Some(enabled),
            args: None,
            url: None,
            env: None,
            allowed_paths: None,
            read_only: None,
            auto_attach: None,
            description: None,
            headers: None,
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            rate_limit_per_min: None,
        })
        .await?;
        Ok(())
    }

    pub async fn doctor(&self, name: Option<&str>) -> Result<Vec<DoctorReport>> {
        let servers = self.list().await;
        let targets: Vec<_> = match name {
            Some(n) => servers.into_iter().filter(|s| s.name == n).collect(),
            None => servers,
        };
        if targets.is_empty() {
            if let Some(n) = name {
                return Err(McpError::NotFound(n.to_string()));
            }
        }

        let mut reports = Vec::new();
        for s in targets {
            let mut messages = Vec::new();
            let mut status = DoctorStatus::Ok;

            match validate_custom_server(&s) {
                SecurityVerdict::Deny { reason } => {
                    status = DoctorStatus::Error;
                    messages.push(format!("security: {reason}"));
                }
                SecurityVerdict::Warn { reason } => {
                    status = DoctorStatus::Warn;
                    messages.push(format!("security: {reason}"));
                }
                SecurityVerdict::Allow => messages.push("security validation passed".into()),
            }

            if s.kind == "filesystem" {
                for p in &s.allowed_paths {
                    if !p.exists() {
                        status = DoctorStatus::Error;
                        messages.push(format!("path missing: {}", p.display()));
                    }
                }
            }

            for key in &s.credential_keys {
                match self.credentials.get(key)? {
                    Some(_) => messages.push(format!("credential `{key}` present")),
                    None => {
                        // Also check process env
                        if std::env::var(key).is_ok() {
                            messages.push(format!("credential `{key}` present in env"));
                        } else {
                            if status == DoctorStatus::Ok {
                                status = DoctorStatus::Warn;
                            }
                            messages.push(format!("credential `{key}` missing"));
                        }
                    }
                }
            }

            if let Some(ref cmd) = s.command {
                if s.transport == McpTransport::Stdio {
                    let found = which::which(cmd).is_ok();
                    if !found {
                        if status != DoctorStatus::Error {
                            status = DoctorStatus::Warn;
                        }
                        messages.push(format!("command `{cmd}` not found on PATH"));
                    } else {
                        messages.push(format!("command `{cmd}` resolvable"));
                    }
                }
            }

            let cli_output = match self.grok_cli.mcp_list().await {
                Ok(out) => {
                    if out.contains(&s.name) {
                        messages.push("visible in `grok mcp list`".into());
                    } else {
                        messages.push("not listed by `grok mcp list` (config-local ok)".into());
                    }
                    Some(out)
                }
                Err(e) => {
                    messages.push(format!("cli list unavailable: {e}"));
                    None
                }
            };

            reports.push(DoctorReport {
                name: s.name,
                status,
                messages,
                checked_at: Utc::now(),
                cli_output,
            });
        }
        Ok(reports)
    }

    /// Known tools from catalog + optional CLI probe.
    pub async fn list_tools(&self, name: Option<&str>) -> Result<Vec<McpToolInfo>> {
        let servers = self.list().await;
        let mut tools = Vec::new();
        for s in servers {
            if let Some(n) = name {
                if s.name != n {
                    continue;
                }
            }
            if let Some(entry) = catalog_entry(&s.kind) {
                for t in entry.example_tools {
                    tools.push(McpToolInfo {
                        namespace: format!("{}__{}", s.name, t),
                        server: s.name.clone(),
                        description: Some(format!("{t} tool via {}", s.kind)),
                        name: t,
                    });
                }
            } else {
                tools.push(McpToolInfo {
                    server: s.name.clone(),
                    name: "custom_tool".into(),
                    description: Some("custom server tools (discover at runtime)".into()),
                    namespace: format!("{}__*", s.name),
                });
            }
        }
        Ok(tools)
    }

    /// Build ACP mcpServers payload for spawn, applying attachment policy and
    /// pre-spawn health checks. Servers that would fail at load (missing
    /// credentials, missing binary, missing paths) are excluded and reported
    /// in `skipped` so the thread can say why instead of showing agent-side
    /// load errors.
    pub async fn session_mcp_payload(
        &self,
        requested_names: &[String],
        approved_high_risk: &[String],
        include_auto: bool,
    ) -> Result<SessionMcpResolution> {
        let available = self.list().await;
        let mut res = resolve_attachments(
            &available,
            requested_names,
            approved_high_risk,
            include_auto,
        );
        let mut healthy = Vec::new();
        for mut s in res.attached {
            // Missing binary → guaranteed spawn failure.
            if s.transport == McpTransport::Stdio {
                if let Some(ref cmd) = s.command {
                    if which::which(cmd).is_err() {
                        res.skipped.push(SkippedMcp {
                            name: s.name.clone(),
                            reason: format!("command `{cmd}` not found on PATH"),
                        });
                        continue;
                    }
                }
            }
            // Missing allowed paths → filesystem server errors at load.
            if s.kind == "filesystem" {
                if let Some(missing) = s.allowed_paths.iter().find(|p| !p.exists()) {
                    res.skipped.push(SkippedMcp {
                        name: s.name.clone(),
                        reason: format!("allowed path missing: {}", missing.display()),
                    });
                    continue;
                }
            }
            // Unresolvable credentials → guaranteed auth failure.
            match self.credentials.resolve_env(&s.env) {
                Ok(env) => s.env = env,
                Err(e) => {
                    res.skipped.push(SkippedMcp {
                        name: s.name.clone(),
                        reason: e.to_string(),
                    });
                    continue;
                }
            }
            match self.credentials.resolve_env(&s.headers) {
                Ok(headers) => s.headers = headers,
                Err(e) => {
                    res.skipped.push(SkippedMcp {
                        name: s.name.clone(),
                        reason: e.to_string(),
                    });
                    continue;
                }
            }
            healthy.push(s);
        }
        Ok(SessionMcpResolution {
            payload: build_session_mcp_payload(&healthy),
            attached_names: healthy.iter().map(|s| s.name.clone()).collect(),
            skipped: res.skipped,
        })
    }

    pub fn credentials(&self) -> &CredentialStore {
        &self.credentials
    }

    pub async fn set_credential(&self, key: &str, value: &str) -> Result<()> {
        self.credentials.set(key, value)?;
        Ok(())
    }

    pub async fn list_credentials_masked(&self) -> Result<Vec<crate::credentials::McpCredential>> {
        Ok(self.credentials.list_masked()?)
    }

    /// Suggest MCP servers based on project context.
    pub async fn suggest_for_project(&self, git_remote: Option<&str>, branch: Option<&str>) -> Vec<String> {
        let mut suggestions = Vec::new();
        if let Some(remote) = git_remote {
            if crate::injection::suggest_github_mcp(remote) {
                suggestions.push("github".into());
            }
        }
        if let Some(b) = branch {
            if !crate::injection::detect_linear_issue_ids(b).is_empty() {
                suggestions.push("linear".into());
            }
        }
        let existing = self.list().await;
        for s in existing {
            if s.auto_attach && s.enabled {
                suggestions.push(s.name);
            }
        }
        suggestions.sort();
        suggestions.dedup();
        suggestions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grok_events::shared_bus;
    use tempfile::tempdir;

    fn test_manager(dir: &std::path::Path) -> Arc<McpManager> {
        let paths = GrokPaths {
            home_dir: dir.to_path_buf(),
            grok_dir: dir.to_path_buf(), bomb_dir: dir.to_path_buf(),
            config_file: dir.join("config.toml"),
            grok_cli_config_file: dir.join("cli-config.toml"),
            worktrees_dir: dir.join("worktrees"),
            memory_dir: dir.join("memory"),
            sessions_dir: dir.join("sessions"),
            panel_dir: dir.to_path_buf(),
            project_config_file: None,
            project_root: None,
        };
        let cfg = Arc::new(RwLock::new(GrokConfig::default()));
        let cli = Arc::new(GrokCli::new("/bin/true"));
        let bus = shared_bus();
        let mut mgr = McpManager {
            config: cfg,
            paths,
            grok_cli: cli,
            event_bus: bus,
            credentials: CredentialStore::open(dir.join("creds.json")).unwrap(),
            prefer_cli: false,
        };
        let _ = &mut mgr;
        Arc::new(mgr)
    }

    #[tokio::test]
    async fn add_filesystem_from_catalog() {
        let dir = tempdir().unwrap();
        let allowed = dir.path().join("docs");
        std::fs::create_dir_all(&allowed).unwrap();
        let mgr = test_manager(dir.path());
        let srv = mgr
            .add(AddMcpRequest {
                name: "docs-fs".into(),
                from_catalog: Some("filesystem".into()),
                allowed_paths: Some(vec![allowed.display().to_string()]),
                kind: None,
                transport: None,
                command: None,
                args: None,
                url: None,
                env: None,
                enabled: Some(true),
                scope: Some("project".into()),
                description: None,
                read_only: Some(true),
                auto_attach: None,
                requires_approval: None,
                headers: None,
                startup_timeout_sec: None,
                tool_timeout_sec: None,
                rate_limit_per_min: None,
                credential_keys: None,
            })
            .await
            .unwrap();
        assert_eq!(srv.kind, "filesystem");
        assert!(srv.args.iter().any(|a| a.contains("server-filesystem")));
        let list = mgr.list().await;
        assert_eq!(list.len(), 1);
    }

    #[tokio::test]
    async fn add_linear_http() {
        let dir = tempdir().unwrap();
        let mgr = test_manager(dir.path());
        let srv = mgr
            .add(AddMcpRequest {
                name: "linear".into(),
                from_catalog: Some("linear".into()),
                allowed_paths: None,
                kind: None,
                transport: None,
                command: None,
                args: None,
                url: None,
                env: None,
                enabled: Some(true),
                scope: None,
                description: None,
                read_only: None,
                auto_attach: None,
                requires_approval: None,
                headers: None,
                startup_timeout_sec: None,
                tool_timeout_sec: None,
                rate_limit_per_min: None,
                credential_keys: None,
            })
            .await
            .unwrap();
        assert_eq!(srv.transport, McpTransport::Http);
        assert!(srv.url.unwrap().contains("linear.app"));
    }

    #[tokio::test]
    async fn rejects_bad_path() {
        let dir = tempdir().unwrap();
        let mgr = test_manager(dir.path());
        let err = mgr
            .add(AddMcpRequest {
                name: "bad".into(),
                from_catalog: Some("filesystem".into()),
                allowed_paths: Some(vec!["/".into()]),
                kind: None,
                transport: None,
                command: None,
                args: None,
                url: None,
                env: None,
                enabled: None,
                scope: None,
                description: None,
                read_only: None,
                auto_attach: None,
                requires_approval: None,
                headers: None,
                startup_timeout_sec: None,
                tool_timeout_sec: None,
                rate_limit_per_min: None,
                credential_keys: None,
            })
            .await;
        assert!(err.is_err());
    }
}
