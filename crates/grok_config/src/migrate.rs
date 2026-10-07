//! One-time move of Bomb Code's data out of the Grok CLI's folder.
//!
//! Early builds kept their data in `~/.grok/control-panel` and their thread worktrees in
//! `~/.grok/worktrees`. Bomb Code's home is now `~/.bombcode`. On the first launch after
//! the change, the items this app owns are moved there (renames, so nothing is copied),
//! moved worktrees are re-linked with `git worktree repair`, and a note is left behind.
//! Files this app doesn't own stay put: the Grok CLI's own files, and `haven.toml`, which
//! an older build of the app reads from the old place.
//!
//! Saved paths inside the databases are rewritten afterwards by the caller
//! (`Persistence::rewrite_path_prefixes`) using [`path_rewrites`].

use std::path::{Path, PathBuf};

use crate::paths::GrokPaths;

/// Things in the old panel folder that belong to this app.
const OWNED: &[&str] = &[
    "sessions", "config.toml", "memory", "cache", "foundry-drafts", "model-discovery",
    "perspective", "servers", "thumbnails", "transfer",
];

#[derive(Debug, Default)]
pub struct MigrationReport {
    pub moved: Vec<String>,
    pub worktrees: usize,
    /// Items left where they were because something already exists at the destination.
    pub conflicts: Vec<String>,
}

fn legacy_panel(paths: &GrokPaths) -> PathBuf {
    paths.grok_dir.join("control-panel")
}

/// Old → new path prefixes, for rewriting paths saved in databases and settings.
pub fn path_rewrites(paths: &GrokPaths) -> Vec<(String, String)> {
    let s = |p: &Path| format!("{}/", p.display());
    vec![
        (s(&legacy_panel(paths)), s(&paths.bomb_dir)),
        (s(&paths.grok_dir.join("worktrees")), s(&paths.worktrees_dir)),
    ]
}

/// Move the data if this Mac still has it in the old place and the new home is unused.
/// Safe to call on every launch: once moved (or on a fresh install) it does nothing.
pub fn migrate_legacy_data(paths: &GrokPaths) -> std::io::Result<Option<MigrationReport>> {
    let old = legacy_panel(paths);
    let has_old_data = old.join("sessions").join("control_panel.db").is_file();
    let has_new_data = paths.sessions_dir.join("control_panel.db").is_file();
    if !has_old_data || has_new_data {
        return Ok(None);
    }
    std::fs::create_dir_all(&paths.bomb_dir)?;
    let mut report = MigrationReport::default();
    for name in OWNED {
        let from = old.join(name);
        if from.exists() {
            match move_into(&from, &paths.bomb_dir.join(name))? {
                true => report.moved.push((*name).to_string()),
                false => report.conflicts.push((*name).to_string()),
            }
        }
    }

    // Thread worktrees: move each, then point its repository at the new location.
    let old_worktrees = paths.grok_dir.join("worktrees");
    if let Ok(entries) = std::fs::read_dir(&old_worktrees) {
        std::fs::create_dir_all(&paths.worktrees_dir)?;
        for entry in entries.flatten() {
            let from = entry.path();
            if !from.is_dir() {
                continue;
            }
            let to = paths.worktrees_dir.join(entry.file_name());
            if move_into(&from, &to)? {
                report.worktrees += 1;
                let _ = std::process::Command::new("git").arg("-C").arg(&to).args(["worktree", "repair"]).output();
            } else {
                report.conflicts.push(format!("worktrees/{}", entry.file_name().to_string_lossy()));
            }
        }
        let _ = std::fs::remove_file(old_worktrees.join(".DS_Store"));
        let _ = std::fs::remove_dir(&old_worktrees);
    }

    // MCP credentials saved by this app.
    let creds = paths.grok_dir.join("mcp_credentials.json");
    if creds.is_file() && move_into(&creds, &paths.bomb_dir.join("mcp_credentials.json"))? {
        report.moved.push("mcp_credentials.json".into());
    }

    let note = format!(
        "# Bomb Code moved\n\nBomb Code's data now lives in {} (settings, threads, memory, worktrees).\n\
         Files still here belong to other tools or older builds.\n",
        paths.bomb_dir.display()
    );
    let _ = std::fs::write(old.join("MOVED.md"), note);
    Ok(Some(report))
}

/// Rename `from` to `to`. An empty folder at `to` (e.g. one an earlier launch created) is
/// replaced; anything else there is a conflict and `from` stays put (returns false).
fn move_into(from: &Path, to: &Path) -> std::io::Result<bool> {
    if to.exists() {
        let empty_dir = to.is_dir() && std::fs::read_dir(to)?.next().is_none();
        if !empty_dir {
            return Ok(false);
        }
        std::fs::remove_dir(to)?;
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(from, to)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths_in(home: &Path) -> GrokPaths {
        let grok = home.join(".grok");
        let bomb = home.join(".bombcode");
        GrokPaths {
            home_dir: home.to_path_buf(),
            grok_dir: grok.clone(),
            bomb_dir: bomb.clone(),
            panel_dir: bomb.clone(),
            config_file: bomb.join("config.toml"),
            grok_cli_config_file: grok.join("config.toml"),
            worktrees_dir: bomb.join("worktrees"),
            memory_dir: bomb.join("memory"),
            sessions_dir: bomb.join("sessions"),
            project_config_file: None,
            project_root: None,
        }
    }

    #[test]
    fn moves_owned_data_and_leaves_the_rest() {
        let home = tempfile::tempdir().unwrap();
        let p = paths_in(home.path());
        let old = home.path().join(".grok/control-panel");
        std::fs::create_dir_all(old.join("sessions")).unwrap();
        std::fs::write(old.join("sessions/control_panel.db"), b"db").unwrap();
        std::fs::write(old.join("config.toml"), b"cfg").unwrap();
        std::fs::write(old.join("haven.toml"), b"other app").unwrap();
        std::fs::write(home.path().join(".grok/auth.json"), b"grok login").unwrap();
        std::fs::create_dir_all(home.path().join(".grok/worktrees/thread-a")).unwrap();
        // An earlier launch of the new build already made an empty folder.
        std::fs::create_dir_all(p.bomb_dir.join("memory")).unwrap();
        std::fs::create_dir_all(old.join("memory")).unwrap();
        std::fs::write(old.join("memory/notes.md"), b"m").unwrap();

        let report = migrate_legacy_data(&p).unwrap().expect("migrated");
        assert!(report.conflicts.is_empty(), "{:?}", report.conflicts);
        assert_eq!(std::fs::read(p.bomb_dir.join("sessions/control_panel.db")).unwrap(), b"db");
        assert_eq!(std::fs::read(p.bomb_dir.join("config.toml")).unwrap(), b"cfg");
        assert!(p.bomb_dir.join("memory/notes.md").is_file());
        assert!(p.worktrees_dir.join("thread-a").is_dir());
        // Not ours: stays.
        assert!(old.join("haven.toml").is_file() && home.path().join(".grok/auth.json").is_file());
        assert!(old.join("MOVED.md").is_file());
        // Second launch: nothing to do.
        assert!(migrate_legacy_data(&p).unwrap().is_none());
    }

    #[test]
    fn a_fresh_install_does_nothing() {
        let home = tempfile::tempdir().unwrap();
        assert!(migrate_legacy_data(&paths_in(home.path())).unwrap().is_none());
    }

    #[test]
    fn rewrites_point_old_prefixes_at_the_new_home() {
        let home = Path::new("/Users/me");
        let p = paths_in(home);
        assert_eq!(
            path_rewrites(&p),
            [
                ("/Users/me/.grok/control-panel/".to_string(), "/Users/me/.bombcode/".to_string()),
                ("/Users/me/.grok/worktrees/".to_string(), "/Users/me/.bombcode/worktrees/".to_string()),
            ]
        );
    }
}
