//! Path discovery: Bomb Code's own home (`~/.bombcode`), the Grok CLI's (`~/.grok`), and
//! the `grok` binary.
//!
//! Everything Bomb Code writes lives under `~/.bombcode` (or `$BOMBCODE_HOME`), like
//! `~/.claude` and `~/.codex`: settings, the thread database, memory, worktrees, caches.
//! `~/.grok` is only read, for the Grok CLI's own login, config and binary. Early builds
//! kept their data in `~/.grok/control-panel` and `~/.grok/worktrees`; `migrate` moves it.

use std::path::{Path, PathBuf};

use which::which;

use crate::{ConfigError, Result};

#[derive(Debug, Clone)]
pub struct GrokPaths {
    pub home_dir: PathBuf,
    /// The Grok CLI's home (`~/.grok`): its login, config and binary. Bomb Code doesn't write here.
    pub grok_dir: PathBuf,
    /// Bomb Code's home (`~/.bombcode`).
    pub bomb_dir: PathBuf,
    /// Panel-owned config — never overwrites Grok CLI `~/.grok/config.toml`.
    pub config_file: PathBuf,
    /// Read-only path to the official Grok CLI config (for display/doctor).
    pub grok_cli_config_file: PathBuf,
    pub worktrees_dir: PathBuf,
    pub memory_dir: PathBuf,
    pub sessions_dir: PathBuf,
    pub panel_dir: PathBuf,
    pub project_config_file: Option<PathBuf>,
    pub project_root: Option<PathBuf>,
}

impl GrokPaths {
    pub fn discover(project_root: Option<&Path>) -> Result<Self> {
        let home_dir = directories::UserDirs::new()
            .map(|u| u.home_dir().to_path_buf())
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .ok_or_else(|| ConfigError::Invalid("cannot resolve home directory".into()))?;

        let grok_dir = home_dir.join(".grok");
        let bomb_dir = std::env::var_os("BOMBCODE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir.join(".bombcode"));
        let panel_dir = bomb_dir.clone();
        let config_file = bomb_dir.join("config.toml");
        let grok_cli_config_file = grok_dir.join("config.toml");
        let worktrees_dir = bomb_dir.join("worktrees");
        let memory_dir = bomb_dir.join("memory");
        let sessions_dir = bomb_dir.join("sessions");

        let (project_root, project_config_file) = if let Some(root) = project_root {
            let cfg = root.join(".grok").join("control-panel.toml");
            (Some(root.to_path_buf()), Some(cfg))
        } else {
            (None, None)
        };

        Ok(Self {
            home_dir,
            grok_dir,
            bomb_dir,
            config_file,
            grok_cli_config_file,
            worktrees_dir,
            memory_dir,
            sessions_dir,
            panel_dir,
            project_config_file,
            project_root,
        })
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        std::fs::create_dir_all(&self.bomb_dir)?;
        std::fs::create_dir_all(&self.worktrees_dir)?;
        std::fs::create_dir_all(&self.memory_dir)?;
        std::fs::create_dir_all(&self.sessions_dir)?;
        Ok(())
    }
}

/// Bomb Code's home folder (`$BOMBCODE_HOME`, else `~/.bombcode`), for code without the
/// app's `GrokPaths` at hand.
pub fn bomb_home() -> Option<PathBuf> {
    std::env::var_os("BOMBCODE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".bombcode")))
}

/// Locate the `grok` binary via official install locations first, then PATH.
pub fn discover_grok_binary() -> Result<PathBuf> {
    // Prefer official ~/.grok/bin install (GUI apps often lack PATH entries).
    for c in crate::env_bootstrap::preferred_grok_candidates() {
        if c.is_file() {
            // Resolve symlinks when possible for stability.
            return Ok(std::fs::canonicalize(&c).unwrap_or(c));
        }
    }

    if let Ok(p) = which("grok") {
        return Ok(std::fs::canonicalize(&p).unwrap_or(p));
    }

    Err(ConfigError::BinaryNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_paths() {
        let paths = GrokPaths::discover(None).unwrap();
        assert!(paths.config_file.ends_with(".bombcode/config.toml") || std::env::var_os("BOMBCODE_HOME").is_some());
        assert!(paths.grok_cli_config_file.ends_with(".grok/config.toml"));
        assert!(paths.worktrees_dir.ends_with("worktrees"));
    }

    #[test]
    fn project_overlay_path() {
        let root = Path::new("/tmp/myproject");
        let paths = GrokPaths::discover(Some(root)).unwrap();
        assert_eq!(
            paths.project_config_file.unwrap(),
            PathBuf::from("/tmp/myproject/.grok/control-panel.toml")
        );
    }
}
