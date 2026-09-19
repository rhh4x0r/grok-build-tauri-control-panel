//! Grok Build configuration discovery, parsing, and safe mutation.
//!
//! Resolves `~/.grok/config.toml`, project `.grok/` overlays, and the `grok` binary.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, info, warn};

pub mod backends;
pub mod env_bootstrap;
pub mod paths;
pub mod sandbox;

pub use backends::{
    Backend, BackendConfig, BackendDescriptor, LaunchVia, ResolvedBackend, descriptor,
    resolve_backend,
};
pub use env_bootstrap::{bootstrap_process_env, child_path_env, preferred_grok_candidates};
pub use paths::{GrokPaths, discover_grok_binary};
pub use sandbox::SandboxProfile;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("toml serialize error: {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("toml edit error: {0}")]
    TomlEdit(String),
    #[error("grok binary not found")]
    BinaryNotFound,
    #[error("{0} backend not found (no binary on PATH and npx unavailable)")]
    BackendNotFound(&'static str),
    #[error("invalid config: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, ConfigError>;

/// Top-level Grok control-panel configuration (app-level + grok mirror).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GrokConfig {
    pub model_suggestions: ModelSuggestionsConfig,
    pub default_model: String,
    pub default_effort: String,
    pub default_backend: Backend,
    /// Per-backend overrides keyed by "grok" | "claude" | "codex".
    pub backends: HashMap<String, BackendConfig>,
    pub grok_binary: Option<PathBuf>,
    pub max_concurrent_sessions: usize,
    pub session_timeout_secs: u64,
    pub always_approve_default: bool,
    pub plan_mode_default: bool,
    /// Default approval stance for new threads: ask | plan | auto | yolo.
    /// Supersedes the two booleans above when set.
    #[serde(default)]
    pub approval_mode_default: Option<String>,
    pub sandbox_profile: SandboxProfile,
    pub worktrees_root: Option<PathBuf>,
    /// Give each new thread in a git project its own worktree by default.
    pub worktree_isolation_default: bool,
    /// Right-panel ELI12 narrator (side LLM calls on the selected thread).
    pub explainer_enabled: bool,
    /// Backend for narrator calls (grok | claude | codex); None → grok.
    pub explainer_backend: Option<String>,
    /// Model for narrator calls; None → cheapest known fast model.
    pub explainer_model: Option<String>,
    pub mcp_servers: HashMap<String, McpServerConfig>,
    pub skills: HashMap<String, SkillConfig>,
    pub plugins: HashMap<String, PluginConfig>,
    pub permissions: PermissionDefaults,
    pub env: HashMap<String, String>,
}

/// Optional advisory routing; secrets are stored separately in the OS keychain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModelSuggestionsConfig {
    pub enabled: bool,
    pub connection: String,
    pub guidelines: String,
}
impl Default for ModelSuggestionsConfig {
    fn default() -> Self {
        Self { enabled: false, connection: "typesafe".into(), guidelines: "Prefer Astra for backend logic, Rust, databases, and debugging when available. Prefer Fable 5.1 for frontend work and visual polish when available. Otherwise use provider model descriptions. Keep small follow-ups with the current model only when no task-specific model preference applies. Suggest a switch when another model better matches the requested work.".into() }
    }
}

impl Default for GrokConfig {
    fn default() -> Self {
        Self {
            model_suggestions: ModelSuggestionsConfig::default(),
            default_model: "grok-4".to_string(),
            default_effort: "high".to_string(),
            default_backend: Backend::Grok,
            backends: HashMap::new(),
            grok_binary: None,
            max_concurrent_sessions: 10,
            session_timeout_secs: 3600,
            always_approve_default: false,
            plan_mode_default: false,
            approval_mode_default: Some("ask".into()),
            sandbox_profile: SandboxProfile::Workspace,
            worktrees_root: None,
            worktree_isolation_default: true,
            explainer_enabled: true,
            explainer_backend: None,
            explainer_model: None,
            mcp_servers: HashMap::new(),
            skills: HashMap::new(),
            plugins: HashMap::new(),
            permissions: PermissionDefaults::default(),
            env: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct McpServerConfig {
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub enabled: bool,
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct SkillConfig {
    pub path: Option<PathBuf>,
    pub enabled: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct PluginConfig {
    pub path: Option<PathBuf>,
    pub enabled: bool,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PermissionDefaults {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub trust_repo: bool,
}

impl Default for PermissionDefaults {
    fn default() -> Self {
        Self {
            allow: vec![
                "Read(**)".to_string(),
                "Glob(**)".to_string(),
                "Grep(**)".to_string(),
            ],
            deny: vec![
                "Bash(rm -rf *)".to_string(),
                "Bash(sudo *)".to_string(),
            ],
            trust_repo: false,
        }
    }
}

impl GrokConfig {
    /// Load ONLY the user-global config (no project overlay). Use this when
    /// the result will be saved back to the global path — saving the merged
    /// view would permanently promote project-scoped settings to global.
    pub fn load_base(paths: &GrokPaths) -> Result<Self> {
        let mut cfg = Self::default();
        if paths.config_file.exists() {
            let raw = fs::read_to_string(&paths.config_file)?;
            cfg = toml::from_str(&raw)?;
        }
        Ok(cfg)
    }

    /// Load config from paths, falling back to defaults.
    pub fn load(paths: &GrokPaths) -> Result<Self> {
        let mut cfg = Self::default();
        if paths.config_file.exists() {
            let raw = fs::read_to_string(&paths.config_file)?;
            cfg = toml::from_str(&raw)?;
            debug!(path = %paths.config_file.display(), "loaded user config");
        } else {
            info!("no user config found; using defaults");
        }

        // Project overlay if present
        if let Some(project_cfg) = &paths.project_config_file {
            if project_cfg.exists() {
                let raw = fs::read_to_string(project_cfg)?;
                let overlay: GrokConfig = toml::from_str(&raw)?;
                cfg = cfg.merge_overlay(overlay);
                debug!(path = %project_cfg.display(), "merged project config");
            }
        }

        if cfg.worktrees_root.is_none() {
            cfg.worktrees_root = Some(paths.worktrees_dir.clone());
        }

        if cfg.grok_binary.is_none() {
            cfg.grok_binary = discover_grok_binary().ok();
        }

        Ok(cfg)
    }

    /// Save to the user config path (atomic write).
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, content)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Shallow merge: overlay wins only for fields that differ from the
    /// defaults; maps extend. An overlay file that omits a key must never
    /// reset the user's global value (scalars used to be copied blindly).
    pub fn merge_overlay(mut self, overlay: GrokConfig) -> Self {
        // Routing opt-in and preferences are global-only: a repository cannot enable external evaluation.
        let defaults = Self::default();
        if overlay.default_model != defaults.default_model {
            self.default_model = overlay.default_model;
        }
        if overlay.default_effort != defaults.default_effort {
            self.default_effort = overlay.default_effort;
        }
        if overlay.grok_binary.is_some() {
            self.grok_binary = overlay.grok_binary;
        }
        if overlay.default_backend != Backend::default() {
            self.default_backend = overlay.default_backend;
        }
        self.backends.extend(overlay.backends);
        if overlay.max_concurrent_sessions != defaults.max_concurrent_sessions {
            self.max_concurrent_sessions = overlay.max_concurrent_sessions;
        }
        if overlay.session_timeout_secs != defaults.session_timeout_secs {
            self.session_timeout_secs = overlay.session_timeout_secs;
        }
        if overlay.always_approve_default != defaults.always_approve_default {
            self.always_approve_default = overlay.always_approve_default;
        }
        if overlay.plan_mode_default != defaults.plan_mode_default {
            self.plan_mode_default = overlay.plan_mode_default;
        }
        if overlay.approval_mode_default.is_some() {
            self.approval_mode_default = overlay.approval_mode_default.clone();
        }
        if overlay.sandbox_profile != defaults.sandbox_profile {
            self.sandbox_profile = overlay.sandbox_profile;
        }
        if overlay.worktrees_root.is_some() {
            self.worktrees_root = overlay.worktrees_root;
        }
        if overlay.worktree_isolation_default != defaults.worktree_isolation_default {
            self.worktree_isolation_default = overlay.worktree_isolation_default;
        }
        if overlay.explainer_enabled != defaults.explainer_enabled {
            self.explainer_enabled = overlay.explainer_enabled;
        }
        if overlay.explainer_backend.is_some() {
            self.explainer_backend = overlay.explainer_backend.clone();
        }
        if overlay.explainer_model.is_some() {
            self.explainer_model = overlay.explainer_model.clone();
        }
        self.mcp_servers.extend(overlay.mcp_servers);
        self.skills.extend(overlay.skills);
        self.plugins.extend(overlay.plugins);
        if !overlay.permissions.allow.is_empty() {
            self.permissions.allow = overlay.permissions.allow;
        }
        if !overlay.permissions.deny.is_empty() {
            // Deny rules are additive — an overlay can tighten, never loosen.
            let mut deny = self.permissions.deny.clone();
            deny.extend(overlay.permissions.deny);
            deny.dedup();
            self.permissions.deny = deny;
        }
        if overlay.permissions.trust_repo != defaults.permissions.trust_repo {
            self.permissions.trust_repo = overlay.permissions.trust_repo;
        }
        self.env.extend(overlay.env);
        self
    }

    /// Per-backend user overrides, if configured.
    pub fn backend_config(&self, backend: Backend) -> Option<&BackendConfig> {
        self.backends.get(backend.key())
    }

    /// Default model for a backend: per-backend config → legacy top-level
    /// default_model (Grok only) → built-in descriptor default.
    pub fn model_for(&self, backend: Backend) -> String {
        if let Some(m) = self
            .backend_config(backend)
            .and_then(|c| c.default_model.clone())
        {
            return m;
        }
        if backend == Backend::Grok {
            return self.default_model.clone();
        }
        backends::descriptor(backend).default_model.to_string()
    }

    /// Model catalog for a backend: config override when non-empty, else built-in.
    pub fn models_for(&self, backend: Backend) -> Vec<String> {
        match self.backend_config(backend) {
            Some(c) if !c.models.is_empty() => c.models.clone(),
            _ => backends::descriptor(backend)
                .model_catalog
                .iter()
                .map(|m| m.to_string())
                .collect(),
        }
    }

    /// Resolve the grok binary path or error.
    pub fn resolve_grok_binary(&self) -> Result<PathBuf> {
        if let Some(ref p) = self.grok_binary {
            if p.exists() {
                return Ok(p.clone());
            }
            warn!(path = %p.display(), "configured grok binary missing");
        }
        discover_grok_binary().map_err(|_| ConfigError::BinaryNotFound)
    }

    pub fn set_mcp_server(&mut self, name: String, server: McpServerConfig) {
        self.mcp_servers.insert(name, server);
    }

    pub fn remove_mcp_server(&mut self, name: &str) -> Option<McpServerConfig> {
        self.mcp_servers.remove(name)
    }

    pub fn set_skill(&mut self, name: String, skill: SkillConfig) {
        self.skills.insert(name, skill);
    }

    pub fn remove_skill(&mut self, name: &str) -> Option<SkillConfig> {
        self.skills.remove(name)
    }

    pub fn set_plugin(&mut self, name: String, plugin: PluginConfig) {
        self.plugins.insert(name, plugin);
    }

    pub fn remove_plugin(&mut self, name: &str) -> Option<PluginConfig> {
        self.plugins.remove(name)
    }
}

/// Capture baseline environment info for Phase 0 discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryReport {
    pub grok_binary: Option<PathBuf>,
    pub config_path: PathBuf,
    pub worktrees_dir: PathBuf,
    pub config_exists: bool,
    pub home_dir: PathBuf,
    pub platform: String,
}

pub fn discover_environment() -> Result<DiscoveryReport> {
    let paths = GrokPaths::discover(None)?;
    let binary = discover_grok_binary().ok();
    Ok(DiscoveryReport {
        grok_binary: binary,
        config_path: paths.config_file.clone(),
        worktrees_dir: paths.worktrees_dir.clone(),
        config_exists: paths.config_file.exists(),
        home_dir: paths.home_dir.clone(),
        platform: std::env::consts::OS.to_string(),
    })
}

/// Path to the official Grok CLI config (do not overwrite from the panel).
pub fn grok_cli_config_path() -> Result<PathBuf> {
    Ok(GrokPaths::discover(None)?.grok_cli_config_file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn model_suggestions_require_global_opt_in_and_roundtrip() {
        let mut project = GrokConfig::default();
        project.model_suggestions.enabled = true;
        assert!(!GrokConfig::default().merge_overlay(project).model_suggestions.enabled);
        let mut global = GrokConfig::default();
        global.model_suggestions.enabled = true;
        global.model_suggestions.connection = "vercel".into();
        let encoded = toml::to_string(&global).unwrap();
        let decoded: GrokConfig = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.model_suggestions, global.model_suggestions);
        assert!(decoded.merge_overlay(GrokConfig::default()).model_suggestions.enabled);
        assert!(!toml::from_str::<GrokConfig>("").unwrap().model_suggestions.enabled);
    }

    #[test]
    fn default_config_roundtrip() {
        let cfg = GrokConfig::default();
        let s = toml::to_string_pretty(&cfg).unwrap();
        let back: GrokConfig = toml::from_str(&s).unwrap();
        assert_eq!(cfg.default_model, back.default_model);
        assert_eq!(cfg.max_concurrent_sessions, back.max_concurrent_sessions);
        // No stance is pre-selected: new threads confirm every request until
        // the user picks plan/auto/yolo.
        assert!(!cfg.always_approve_default);
        assert!(!cfg.plan_mode_default);
        assert_eq!(cfg.approval_mode_default.as_deref(), Some("ask"));
        assert_eq!(back.approval_mode_default.as_deref(), Some("ask"));
    }

    #[test]
    fn save_and_load() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut cfg = GrokConfig {
            default_model: "grok-test".into(),
            ..Default::default()
        };
        cfg.set_mcp_server(
            "github".into(),
            McpServerConfig {
                command: Some("npx".into()),
                args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
                enabled: true,
                ..Default::default()
            },
        );
        cfg.save(&path).unwrap();
        let paths = GrokPaths {
            home_dir: dir.path().to_path_buf(),
            grok_dir: dir.path().to_path_buf(),
            config_file: path.clone(),
            grok_cli_config_file: dir.path().join("cli-config.toml"),
            worktrees_dir: dir.path().join("worktrees"),
            memory_dir: dir.path().join("memory"),
            sessions_dir: dir.path().join("sessions"),
            panel_dir: dir.path().to_path_buf(),
            project_config_file: None,
            project_root: None,
        };
        let loaded = GrokConfig::load(&paths).unwrap();
        assert_eq!(loaded.default_model, "grok-test");
        assert!(loaded.mcp_servers.contains_key("github"));
    }

    #[test]
    fn old_config_without_backends_table_loads() {
        // Pre-multi-backend config.toml: no default_backend, no [backends].
        let raw = r#"
default_model = "grok-4"
default_effort = "high"
max_concurrent_sessions = 10
"#;
        let cfg: GrokConfig = toml::from_str(raw).unwrap();
        assert_eq!(cfg.default_backend, Backend::Grok);
        assert!(cfg.backends.is_empty());
        // Round-trips with the new fields serialized.
        let s = toml::to_string_pretty(&cfg).unwrap();
        let back: GrokConfig = toml::from_str(&s).unwrap();
        assert_eq!(back.default_backend, Backend::Grok);
    }

    #[test]
    fn model_for_fallback_chain() {
        let mut cfg = GrokConfig::default();
        // Built-in descriptor defaults when nothing configured.
        assert_eq!(cfg.model_for(Backend::Claude), "claude-fable-5-1");
        assert_eq!(cfg.model_for(Backend::Codex), "gpt-5.6-terra");
        // Grok falls back to legacy top-level default_model.
        cfg.default_model = "grok-code-fast-1".into();
        assert_eq!(cfg.model_for(Backend::Grok), "grok-code-fast-1");
        // Per-backend config wins.
        cfg.backends.insert(
            "claude".into(),
            BackendConfig {
                default_model: Some("claude-opus-4-8".into()),
                ..Default::default()
            },
        );
        assert_eq!(cfg.model_for(Backend::Claude), "claude-opus-4-8");
        // Catalog override.
        assert_eq!(
            cfg.models_for(Backend::Codex),
            backends::descriptor(Backend::Codex)
                .model_catalog
                .iter()
                .map(|m| m.to_string())
                .collect::<Vec<_>>()
        );
        cfg.backends.insert(
            "codex".into(),
            BackendConfig {
                models: vec!["gpt-5-codex".into()],
                ..Default::default()
            },
        );
        assert_eq!(cfg.models_for(Backend::Codex), vec!["gpt-5-codex".to_string()]);
    }

    #[test]
    fn merge_overlay_extends_maps() {
        let mut base = GrokConfig::default();
        base.set_skill(
            "a".into(),
            SkillConfig {
                enabled: true,
                ..Default::default()
            },
        );
        let mut overlay = GrokConfig {
            default_model: "custom".into(),
            ..Default::default()
        };
        overlay.set_skill(
            "b".into(),
            SkillConfig {
                enabled: false,
                ..Default::default()
            },
        );
        let merged = base.merge_overlay(overlay);
        assert_eq!(merged.default_model, "custom");
        assert!(merged.skills.contains_key("a"));
        assert!(merged.skills.contains_key("b"));
    }
}
