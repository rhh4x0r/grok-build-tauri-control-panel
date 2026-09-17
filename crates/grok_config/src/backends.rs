//! Agent backend descriptors: Grok, Claude Code, and Codex over ACP stdio.
//!
//! NOTE: crates keep the grok_ prefix for churn reasons; multi-backend since v0.2.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use which::which;

use crate::{ConfigError, GrokConfig, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    #[default]
    Grok,
    Claude,
    Codex,
}

impl Backend {
    pub const ALL: [Backend; 3] = [Backend::Grok, Backend::Claude, Backend::Codex];

    pub fn key(&self) -> &'static str {
        match self {
            Backend::Grok => "grok",
            Backend::Claude => "claude",
            Backend::Codex => "codex",
        }
    }

    pub fn from_key(key: &str) -> Option<Backend> {
        match key {
            "grok" => Some(Backend::Grok),
            "claude" => Some(Backend::Claude),
            "codex" => Some(Backend::Codex),
            _ => None,
        }
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.key())
    }
}

/// Per-backend user configuration stored under `[backends.<key>]` in config.toml.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct BackendConfig {
    pub binary: Option<PathBuf>,
    pub default_model: Option<String>,
    /// Overrides the built-in model catalog when non-empty.
    pub models: Vec<String>,
    pub env: HashMap<String, String>,
}

/// Static description of how to run and authenticate one agent backend.
pub struct BackendDescriptor {
    pub id: Backend,
    pub display_name: &'static str,
    /// Binary names probed on PATH and well-known dirs, in order.
    pub binary_names: &'static [&'static str],
    /// npm packages tried via `npx --yes` when no binary is found, in order.
    pub npx_packages: &'static [&'static str],
    /// Env vars forwarded from the panel process when set.
    pub env_passthrough: &'static [&'static str],
    /// Ordered auth-method preference matched against initialize.authMethods.
    pub auth_preference: &'static [&'static str],
    /// Skip authenticate entirely when the agent advertises no auth methods.
    pub skip_auth_when_unadvertised: bool,
    pub default_model: &'static str,
    pub model_catalog: &'static [&'static str],
    pub supports_headless: bool,
}

impl BackendDescriptor {
    /// Arguments for the backend's ACP entrypoint. Adapter binaries speak ACP
    /// directly; the native Codex CLI is not an ACP entrypoint.
    pub fn args_for_binary(&self, _program: &std::path::Path) -> Vec<String> {
        match self.id {
            Backend::Grok => vec!["agent".into(), "stdio".into()],
            _ => Vec::new(),
        }
    }
}

const GROK: BackendDescriptor = BackendDescriptor {
    id: Backend::Grok,
    display_name: "Grok",
    binary_names: &["grok"],
    npx_packages: &[],
    env_passthrough: &["XAI_API_KEY"],
    auth_preference: &["cached_token", "grok.com", "xai.api_key"],
    skip_auth_when_unadvertised: false,
    default_model: "grok-4",
    model_catalog: &["grok-4", "grok-code-fast-1"],
    supports_headless: true,
};

const CLAUDE: BackendDescriptor = BackendDescriptor {
    id: Backend::Claude,
    display_name: "Claude Code",
    binary_names: &["claude-code-acp", "claude-agent-acp"],
    npx_packages: &[
        "@agentclientprotocol/claude-agent-acp",
        "@zed-industries/claude-code-acp",
    ],
    env_passthrough: &["ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL", "CLAUDE_CONFIG_DIR"],
    auth_preference: &["claude-login", "anthropic-api-key"],
    skip_auth_when_unadvertised: true,
    default_model: "claude-fable-5-1",
    model_catalog: &[
        "claude-fable-5-1",
        "claude-fable-5",
        "claude-opus-5",
        "claude-opus-4-8",
        "claude-sonnet-5",
        "claude-haiku-4-5",
    ],
    supports_headless: false,
};

const CODEX: BackendDescriptor = BackendDescriptor {
    id: Backend::Codex,
    display_name: "Codex",
    binary_names: &["codex-acp"],
    // Old @zed-industries package is deprecated and its bundled Codex core
    // rejects gpt-5.6 models ("requires a newer version of Codex").
    npx_packages: &["@agentclientprotocol/codex-acp", "@zed-industries/codex-acp"],
    env_passthrough: &["OPENAI_API_KEY", "CODEX_HOME"],
    auth_preference: &["chatgpt", "openai-api-key", "apikey"],
    skip_auth_when_unadvertised: true,
    default_model: "gpt-5.6-terra",
    model_catalog: &[
        "gpt-6-astra",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "gpt-5.5",
        "gpt-5.4",
        "gpt-5.4-mini",
        "gpt-5.3-codex-spark",
        "gpt-5-codex",
    ],
    supports_headless: false,
};

pub fn descriptor(b: Backend) -> &'static BackendDescriptor {
    match b {
        Backend::Grok => &GROK,
        Backend::Claude => &CLAUDE,
        Backend::Codex => &CODEX,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchVia {
    Binary,
    Npx,
}

/// Resolved launch plan: what to actually exec for a backend.
#[derive(Debug, Clone)]
pub struct ResolvedBackend {
    pub backend: Backend,
    pub program: PathBuf,
    /// Args that precede the ACP stdio entrypoint (npx package, subcommands).
    pub args: Vec<String>,
    pub via: LaunchVia,
}

/// Locate a backend's launchable program: config override, then well-known
/// install dirs + PATH, then npx fallback for adapter-based backends.
pub fn resolve_backend(b: Backend, cfg: &GrokConfig) -> Result<ResolvedBackend> {
    let desc = descriptor(b);

    if let Some(over) = cfg.backend_config(b).and_then(|c| c.binary.clone()) {
        if over.exists() {
            let canonical = std::fs::canonicalize(&over).unwrap_or_else(|_| over.clone());
            if b == Backend::Codex
                && [over.as_path(), canonical.as_path()].iter().any(|path| {
                    path.file_name().is_some_and(|name| name == "codex" || name == "codex.exe")
                })
            {
                return Err(ConfigError::Invalid(
                    "Codex requires a codex-acp adapter, not the native codex CLI; remove backends.codex.binary to use the npx adapter fallback".into(),
                ));
            }
            let args = desc.args_for_binary(&over);
            return Ok(ResolvedBackend { backend: b, program: over, args, via: LaunchVia::Binary });
        }
        tracing::warn!(backend = %b, path = %over.display(), "configured backend binary missing");
    }

    // Grok keeps its legacy resolution (config grok_binary + official dirs).
    if b == Backend::Grok {
        let program = cfg.resolve_grok_binary()?;
        let args = desc.args_for_binary(&program);
        return Ok(ResolvedBackend { backend: b, program, args, via: LaunchVia::Binary });
    }

    for name in desc.binary_names {
        for candidate in candidate_paths(name) {
            if candidate.is_file() {
                let program = std::fs::canonicalize(&candidate).unwrap_or(candidate);
                let args = desc.args_for_binary(&program);
                return Ok(ResolvedBackend { backend: b, program, args, via: LaunchVia::Binary });
            }
        }
        if let Ok(p) = which(name) {
            let program = std::fs::canonicalize(&p).unwrap_or(p);
            let args = desc.args_for_binary(&program);
            return Ok(ResolvedBackend { backend: b, program, args, via: LaunchVia::Binary });
        }
    }

    if !desc.npx_packages.is_empty() {
        if let Ok(npx) = which("npx") {
            let pkg = desc.npx_packages[0];
            return Ok(ResolvedBackend {
                backend: b,
                program: npx,
                args: vec!["--yes".into(), pkg.into()],
                via: LaunchVia::Npx,
            });
        }
    }

    Err(ConfigError::BackendNotFound(desc.display_name))
}

fn candidate_paths(name: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = directories::UserDirs::new()
        .map(|u| u.home_dir().to_path_buf())
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
    {
        out.push(home.join(".local").join("bin").join(name));
        out.push(home.join(".cargo").join("bin").join(name));
        out.push(home.join(".npm-global").join("bin").join(name));
    }
    out.push(PathBuf::from("/opt/homebrew/bin").join(name));
    out.push(PathBuf::from("/usr/local/bin").join(name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_lookup() {
        assert_eq!(descriptor(Backend::Grok).display_name, "Grok");
        assert_eq!(descriptor(Backend::Claude).default_model, "claude-fable-5-1");
        assert_eq!(
            descriptor(Backend::Codex).npx_packages[0],
            "@agentclientprotocol/codex-acp"
        );
    }

    #[test]
    fn backend_serde_snake_case() {
        assert_eq!(serde_json::to_string(&Backend::Claude).unwrap(), "\"claude\"");
        let b: Backend = serde_json::from_str("\"codex\"").unwrap();
        assert_eq!(b, Backend::Codex);
        assert_eq!(Backend::from_key("grok"), Some(Backend::Grok));
    }

    #[test]
    fn grok_args_are_agent_stdio() {
        let args = descriptor(Backend::Grok).args_for_binary(std::path::Path::new("/usr/local/bin/grok"));
        assert_eq!(args, vec!["agent".to_string(), "stdio".to_string()]);
    }

    #[test]
    fn codex_discovery_only_uses_acp_adapters() {
        let d = descriptor(Backend::Codex);
        assert_eq!(d.binary_names, &["codex-acp"]);
        assert_eq!(d.npx_packages[0], "@agentclientprotocol/codex-acp");
        assert!(d.args_for_binary(std::path::Path::new("/usr/local/bin/codex-acp")).is_empty());
    }

    #[test]
    fn configured_native_codex_is_rejected_before_spawn() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("codex");
        std::fs::write(&binary, "").unwrap();
        let mut cfg = GrokConfig::default();
        cfg.backends.insert("codex".into(), BackendConfig {
            binary: Some(binary),
            ..Default::default()
        });
        let error = resolve_backend(Backend::Codex, &cfg).unwrap_err();
        assert!(error.to_string().contains("requires a codex-acp adapter"));
    }

    #[test]
    fn configured_codex_adapter_starts_without_subcommand() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("codex-acp");
        std::fs::write(&binary, "").unwrap();
        let mut cfg = GrokConfig::default();
        cfg.backends.insert("codex".into(), BackendConfig {
            binary: Some(binary.clone()),
            ..Default::default()
        });
        let resolved = resolve_backend(Backend::Codex, &cfg).unwrap();
        assert_eq!(resolved.program, binary);
        assert_eq!(resolved.via, LaunchVia::Binary);
        assert!(resolved.args.is_empty());
    }

}
