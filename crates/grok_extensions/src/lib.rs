//! Extensions browser: MCP servers, plugins, and skills CRUD.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::info;

use grok_cli_wrapper::GrokCli;
use grok_config::{GrokConfig, GrokPaths, McpServerConfig, PluginConfig, SkillConfig};
use grok_events::{ControlEvent, EventBus};

#[derive(Debug, Error)]
pub enum ExtensionsError {
    #[error("config error: {0}")]
    Config(#[from] grok_config::ConfigError),
    #[error("cli error: {0}")]
    Cli(#[from] grok_cli_wrapper::CliError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, ExtensionsError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionKind {
    Mcp,
    Plugin,
    Skill,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionEntry {
    pub kind: ExtensionKind,
    pub name: String,
    pub enabled: bool,
    pub detail: serde_json::Value,
}

pub struct ExtensionsService {
    config: Arc<RwLock<GrokConfig>>,
    paths: GrokPaths,
    grok_cli: Arc<GrokCli>,
    event_bus: Arc<EventBus>,
    prefer_cli: bool,
}

impl ExtensionsService {
    pub fn new(
        config: Arc<RwLock<GrokConfig>>,
        paths: GrokPaths,
        grok_cli: Arc<GrokCli>,
        event_bus: Arc<EventBus>,
    ) -> Self {
        Self {
            config,
            paths,
            grok_cli,
            event_bus,
            prefer_cli: true,
        }
    }

    pub async fn list_all(&self) -> Vec<ExtensionEntry> {
        let cfg = self.config.read().await;
        let mut out = Vec::new();
        for (name, s) in &cfg.mcp_servers {
            out.push(ExtensionEntry {
                kind: ExtensionKind::Mcp,
                name: name.clone(),
                enabled: s.enabled,
                detail: serde_json::to_value(s).unwrap_or_default(),
            });
        }
        for (name, p) in &cfg.plugins {
            out.push(ExtensionEntry {
                kind: ExtensionKind::Plugin,
                name: name.clone(),
                enabled: p.enabled,
                detail: serde_json::to_value(p).unwrap_or_default(),
            });
        }
        for (name, s) in &cfg.skills {
            out.push(ExtensionEntry {
                kind: ExtensionKind::Skill,
                name: name.clone(),
                enabled: s.enabled,
                detail: serde_json::to_value(s).unwrap_or_default(),
            });
        }
        out
    }

    pub async fn add_mcp(
        &self,
        name: String,
        command: String,
        args: Vec<String>,
        enabled: bool,
    ) -> Result<()> {
        validate_ext_name(&name)?;
        if command.trim().is_empty() {
            return Err(ExtensionsError::Invalid("command required".into()));
        }

        if self.prefer_cli {
            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            if let Err(e) = self.grok_cli.mcp_add(&name, &command, &arg_refs, &[]).await {
                tracing::warn!(error = %e, "grok mcp add failed; updating config only");
            }
        }

        {
            let mut cfg = self.config.write().await;
            cfg.set_mcp_server(
                name.clone(),
                McpServerConfig {
                    command: Some(command),
                    args,
                    enabled,
                    ..Default::default()
                },
            );
            cfg.save(&self.paths.config_file)?;
        }

        self.event_bus.emit(ControlEvent::McpChanged {
            name,
            enabled,
            at: Utc::now(),
        });
        Ok(())
    }

    pub async fn remove_mcp(&self, name: &str) -> Result<()> {
        if self.prefer_cli {
            let _ = self.grok_cli.mcp_remove(name).await;
        }
        {
            let mut cfg = self.config.write().await;
            cfg.remove_mcp_server(name)
                .ok_or_else(|| ExtensionsError::NotFound(name.to_string()))?;
            cfg.save(&self.paths.config_file)?;
        }
        self.event_bus.emit(ControlEvent::McpChanged {
            name: name.to_string(),
            enabled: false,
            at: Utc::now(),
        });
        Ok(())
    }

    pub async fn toggle_mcp(&self, name: &str, enabled: bool) -> Result<()> {
        let entry = {
            let mut cfg = self.config.write().await;
            let server = cfg
                .mcp_servers
                .get_mut(name)
                .ok_or_else(|| ExtensionsError::NotFound(name.to_string()))?;
            server.enabled = enabled;
            let entry = server.clone();
            cfg.save(&self.paths.config_file)?;
            entry
        };
        // Mirror into the grok CLI registry — otherwise a disabled server keeps
        // loading in every terminal grok session. CLI has no disable: remove on
        // disable, re-add on enable.
        if self.prefer_cli {
            let res = if enabled {
                let args: Vec<&str> = entry.args.iter().map(String::as_str).collect();
                match entry.command.as_deref() {
                    Some(cmd) => {
                        let env_pairs: Vec<(String, String)> = entry
                            .env
                            .iter()
                            .filter(|(k, _)| !k.starts_with("_panel_"))
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect();
                        self.grok_cli
                            .mcp_add(name, cmd, &args, &env_pairs)
                            .await
                            .map(|_| ())
                    }
                    None => Ok(()),
                }
            } else {
                self.grok_cli.mcp_remove(name).await.map(|_| ())
            };
            if let Err(e) = res {
                tracing::warn!(error = %e, name, enabled, "grok CLI mirror sync failed");
            }
        }
        self.event_bus.emit(ControlEvent::McpChanged {
            name: name.to_string(),
            enabled,
            at: Utc::now(),
        });
        info!(name, enabled, "mcp toggled");
        Ok(())
    }

    pub async fn add_skill(
        &self,
        name: String,
        path: Option<PathBuf>,
        description: Option<String>,
        enabled: bool,
    ) -> Result<()> {
        validate_ext_name(&name)?;
        let mut cfg = self.config.write().await;
        cfg.set_skill(
            name,
            SkillConfig {
                path,
                enabled,
                description,
            },
        );
        cfg.save(&self.paths.config_file)?;
        Ok(())
    }

    pub async fn remove_skill(&self, name: &str) -> Result<()> {
        let mut cfg = self.config.write().await;
        cfg.remove_skill(name)
            .ok_or_else(|| ExtensionsError::NotFound(name.to_string()))?;
        cfg.save(&self.paths.config_file)?;
        Ok(())
    }

    pub async fn add_plugin(
        &self,
        name: String,
        path: Option<PathBuf>,
        version: Option<String>,
        enabled: bool,
    ) -> Result<()> {
        validate_ext_name(&name)?;
        let mut cfg = self.config.write().await;
        cfg.set_plugin(
            name,
            PluginConfig {
                path,
                enabled,
                version,
            },
        );
        cfg.save(&self.paths.config_file)?;
        Ok(())
    }

    pub async fn remove_plugin(&self, name: &str) -> Result<()> {
        let mut cfg = self.config.write().await;
        cfg.remove_plugin(name)
            .ok_or_else(|| ExtensionsError::NotFound(name.to_string()))?;
        cfg.save(&self.paths.config_file)?;
        Ok(())
    }

    pub async fn doctor(&self) -> Result<String> {
        match self.grok_cli.doctor().await {
            Ok(s) => Ok(s),
            Err(e) => Ok(format!("doctor unavailable: {e}")),
        }
    }

    pub async fn cli_mcp_list(&self) -> Result<String> {
        Ok(self.grok_cli.mcp_list().await.unwrap_or_else(|e| e.to_string()))
    }
}

fn validate_ext_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(ExtensionsError::Invalid(format!("bad name: {name}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use grok_events::shared_bus;
    use tempfile::tempdir;

    #[tokio::test]
    async fn add_and_list_mcp() {
        let dir = tempdir().unwrap();
        let paths = GrokPaths {
            home_dir: dir.path().to_path_buf(),
            grok_dir: dir.path().to_path_buf(), bomb_dir: dir.path().to_path_buf(),
            config_file: dir.path().join("config.toml"),
            grok_cli_config_file: dir.path().join("cli-config.toml"),
            worktrees_dir: dir.path().join("worktrees"),
            memory_dir: dir.path().join("memory"),
            sessions_dir: dir.path().join("sessions"),
            panel_dir: dir.path().to_path_buf(),
            project_config_file: None,
            project_root: None,
        };
        let cfg = Arc::new(RwLock::new(GrokConfig::default()));
        let cli = Arc::new(GrokCli::new("/bin/true"));
        let bus = shared_bus();
        let svc = ExtensionsService::new(cfg, paths, cli, bus);
        // prefer_cli will fail harmlessly on /bin/true
        let mut svc = svc;
        svc.prefer_cli = false;
        svc.add_mcp(
            "test".into(),
            "npx".into(),
            vec!["-y".into(), "foo".into()],
            true,
        )
        .await
        .unwrap();
        let list = svc.list_all().await;
        assert!(list.iter().any(|e| e.name == "test"));
    }
}
