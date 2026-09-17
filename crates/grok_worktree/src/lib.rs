//! Worktree manager: create, list, remove, and land isolated agent workspaces.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::process::Command;
use tracing::{info, warn};
use uuid::Uuid;

use grok_cli_wrapper::GrokCli;

#[derive(Debug, Error)]
pub enum WorktreeError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("git failed: {0}")]
    Git(String),
    #[error("cli error: {0}")]
    Cli(#[from] grok_cli_wrapper::CliError),
    #[error("invalid name: {0}")]
    InvalidName(String),
    #[error("worktree not found: {0}")]
    NotFound(String),
    #[error("not a git repository: {0}")]
    NotGitRepo(String),
}

pub type Result<T> = std::result::Result<T, WorktreeError>;

/// Provider-neutral branch prefix for thread worktrees.
pub const THREAD_BRANCH_PREFIX: &str = "bomb/";

/// Outcome of a merge attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum MergeOutcome {
    Merged,
    /// Conflicted paths (merge left in progress unless the caller aborts).
    Conflicts { files: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeInfo {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub created_at: DateTime<Utc>,
    pub locked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorktreeRequest {
    pub name: String,
    pub base_ref: Option<String>,
    /// If true, try `grok worktree create` first, else pure git.
    pub prefer_grok_cli: bool,
}

pub struct WorktreeManager {
    grok_cli: Arc<GrokCli>,
    /// Root under which managed worktrees are placed (e.g. ~/.grok/worktrees).
    worktrees_root: PathBuf,
}

impl WorktreeManager {
    pub fn new(grok_cli: Arc<GrokCli>, worktrees_root: PathBuf) -> Self {
        Self {
            grok_cli,
            worktrees_root,
        }
    }

    pub fn worktrees_root(&self) -> &Path {
        &self.worktrees_root
    }

    pub async fn ensure_root(&self) -> Result<()> {
        tokio::fs::create_dir_all(&self.worktrees_root).await?;
        Ok(())
    }

    pub async fn create(
        &self,
        repo: &Path,
        req: CreateWorktreeRequest,
    ) -> Result<WorktreeInfo> {
        validate_name(&req.name)?;
        ensure_git_repo(repo).await?;
        self.ensure_root().await?;

        if req.prefer_grok_cli {
            match self
                .grok_cli
                .worktree_create(&req.name, Some(repo), req.base_ref.as_deref())
                .await
            {
                Ok(out) => {
                    info!(name = %req.name, %out, "created worktree via grok cli");
                    // Discover path after CLI create
                    if let Ok(list) = self.list_git(repo).await {
                        if let Some(found) = list.into_iter().find(|w| w.name == req.name || w.path.ends_with(&req.name))
                        {
                            return Ok(found);
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "grok worktree create failed; falling back to git");
                }
            }
        }

        let path = self
            .worktrees_root
            .join(format!("{}-{}", req.name, &Uuid::new_v4().to_string()[..8]));
        let branch = format!("{THREAD_BRANCH_PREFIX}{}", req.name);
        let base = req.base_ref.as_deref().unwrap_or("HEAD");

        run_git(
            repo,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                path.to_str().unwrap_or_default(),
                base,
            ],
        )
        .await?;

        let path = std::fs::canonicalize(&path)?;
        let head = run_git(&path, &["rev-parse", "HEAD"])
            .await
            .ok()
            .map(|s| s.trim().to_string());

        let info = WorktreeInfo {
            id: Uuid::new_v4().to_string(),
            name: req.name,
            path,
            branch: Some(branch),
            head,
            created_at: Utc::now(),
            locked: false,
        };
        info!(path = %info.path.display(), "worktree created");
        Ok(info)
    }

    pub async fn list(&self, repo: &Path) -> Result<Vec<WorktreeInfo>> {
        ensure_git_repo(repo).await?;
        self.list_git(repo).await
    }

    async fn list_git(&self, repo: &Path) -> Result<Vec<WorktreeInfo>> {
        let out = run_git(repo, &["worktree", "list", "--porcelain"]).await?;
        Ok(parse_porcelain(&out))
    }

    pub async fn remove(&self, repo: &Path, path_or_name: &str, force: bool) -> Result<()> {
        ensure_git_repo(repo).await?;

        // Try grok CLI first
        if validate_name(path_or_name).is_ok() {
            if let Err(e) = self.grok_cli.worktree_remove(path_or_name, Some(repo)).await {
                warn!(error = %e, "grok worktree rm failed; using git");
            } else {
                return Ok(());
            }
        }

        let list = self.list_git(repo).await?;
        let canonical = std::fs::canonicalize(path_or_name).ok();
        let target = list
            .iter()
            .find(|w| w.name == path_or_name || w.path.to_string_lossy() == path_or_name
                || canonical.as_ref().is_some_and(|p| std::fs::canonicalize(&w.path).ok().as_ref() == Some(p)))
            .ok_or_else(|| WorktreeError::NotFound(path_or_name.to_string()))?;

        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        let path_str = target.path.to_string_lossy();
        args.push(path_str.as_ref());
        run_git(repo, &args).await?;
        info!(path = %target.path.display(), "worktree removed");
        Ok(())
    }

    pub async fn prune(&self, repo: &Path) -> Result<String> {
        ensure_git_repo(repo).await?;
        run_git(repo, &["worktree", "prune", "-v"]).await
    }

    /// Stage and commit everything in `path`. Returns false when there was
    /// nothing to commit.
    pub async fn commit_all(&self, path: &Path, message: &str) -> Result<bool> {
        if !run_git(path, &["diff", "--name-only", "--diff-filter=U"]).await?.trim().is_empty() {
            return Err(WorktreeError::Git("Resolve and stage merge conflicts before saving a checkpoint".into()));
        }
        run_git(path, &["add", "-A"]).await?;
        if self.is_clean(path).await? {
            return Ok(false);
        }
        run_git(path, &["commit", "-m", message]).await?;
        Ok(true)
    }

    /// True when the working tree has no staged or unstaged changes.
    pub async fn is_clean(&self, path: &Path) -> Result<bool> {
        let out = run_git(path, &["status", "--porcelain"]).await?;
        Ok(out.trim().is_empty())
    }

    pub async fn current_branch(&self, path: &Path) -> Result<String> {
        Ok(run_git(path, &["rev-parse", "--abbrev-ref", "HEAD"])
            .await?
            .trim()
            .to_string())
    }

    /// Merge `reference` into the branch checked out at `path`.
    /// On conflict the merge is left IN PROGRESS (caller decides whether to
    /// abort — land aborts, sync leaves it for the agent to resolve).
    pub async fn merge(&self, path: &Path, reference: &str, message: &str) -> Result<MergeOutcome> {
        match run_git(path, &["merge", "--no-ff", reference, "-m", message]).await {
            Ok(_) => Ok(MergeOutcome::Merged),
            Err(WorktreeError::Git(err)) => {
                let files = run_git(path, &["diff", "--name-only", "--diff-filter=U"])
                    .await
                    .unwrap_or_default()
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>();
                if files.is_empty() {
                    // Not a conflict — a real merge failure (bad ref, etc.).
                    return Err(WorktreeError::Git(err));
                }
                Ok(MergeOutcome::Conflicts { files })
            }
            Err(e) => Err(e),
        }
    }

    /// Abort an in-progress merge (best-effort).
    pub async fn merge_abort(&self, path: &Path) {
        let _ = run_git(path, &["merge", "--abort"]).await;
    }

    /// Capture a summary of uncommitted changes for landing review.
    pub async fn status_summary(&self, worktree_path: &Path) -> Result<String> {
        run_git(worktree_path, &["status", "--short"]).await
    }

    /// Produce a full diff for the worktree.
    pub async fn diff(&self, worktree_path: &Path) -> Result<String> {
        let staged = run_git(worktree_path, &["diff", "--cached"]).await.unwrap_or_default();
        let unstaged = run_git(worktree_path, &["diff"]).await.unwrap_or_default();
        Ok(format!("{staged}\n{unstaged}"))
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        return Err(WorktreeError::InvalidName(name.to_string()));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '/')
    {
        return Err(WorktreeError::InvalidName(name.to_string()));
    }
    Ok(())
}

async fn ensure_git_repo(path: &Path) -> Result<()> {
    if is_git_repo(path).await {
        Ok(())
    } else {
        Err(WorktreeError::NotGitRepo(path.display().to_string()))
    }
}

/// Public check used by the session-start isolation decision.
pub async fn is_git_repo(path: &Path) -> bool {
    matches!(
        run_git(path, &["rev-parse", "--is-inside-work-tree"]).await,
        Ok(s) if s.trim() == "true"
    )
}

pub async fn run_git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = tokio::time::timeout(std::time::Duration::from_secs(60), Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true)
        .output())
        .await.map_err(|_| WorktreeError::Git("Git operation timed out".into()))??;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(WorktreeError::Git(stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn parse_porcelain(out: &str) -> Vec<WorktreeInfo> {
    let mut result = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_head: Option<String> = None;
    let mut current_branch: Option<String> = None;
    let mut locked = false;

    let flush = |result: &mut Vec<WorktreeInfo>,
                 path: &mut Option<PathBuf>,
                 head: &mut Option<String>,
                 branch: &mut Option<String>,
                 locked: &mut bool| {
        if let Some(p) = path.take() {
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string());
            result.push(WorktreeInfo {
                id: Uuid::new_v4().to_string(),
                name,
                path: p,
                branch: branch.take(),
                head: head.take(),
                created_at: Utc::now(),
                locked: *locked,
            });
            *locked = false;
        }
    };

    for line in out.lines() {
        if line.is_empty() {
            flush(
                &mut result,
                &mut current_path,
                &mut current_head,
                &mut current_branch,
                &mut locked,
            );
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            flush(
                &mut result,
                &mut current_path,
                &mut current_head,
                &mut current_branch,
                &mut locked,
            );
            current_path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            current_head = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("branch ") {
            current_branch = Some(rest.trim_start_matches("refs/heads/").to_string());
        } else if line.starts_with("locked") {
            locked = true;
        }
    }
    flush(
        &mut result,
        &mut current_path,
        &mut current_head,
        &mut current_branch,
        &mut locked,
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_worktree_list() {
        let sample = "\
worktree /repo
HEAD abcdef
branch refs/heads/main

worktree /repo/.grok/wt-feature
HEAD 123456
branch refs/heads/grok/feature
locked
";
        let list = parse_porcelain(sample);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "repo");
        assert_eq!(list[1].branch.as_deref(), Some("grok/feature"));
        assert!(list[1].locked);
    }

    #[test]
    fn name_validation() {
        assert!(validate_name("feat-1").is_ok());
        assert!(validate_name("bad name").is_err());
    }

    async fn temp_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.email", "t@t.local"],
            vec!["config", "user.name", "t"],
        ] {
            run_git(&repo, &args).await.unwrap();
        }
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        run_git(&repo, &["add", "-A"]).await.unwrap();
        run_git(&repo, &["commit", "-m", "init"]).await.unwrap();
        (dir, repo)
    }

    fn test_manager(root: &Path) -> WorktreeManager {
        WorktreeManager::new(
            Arc::new(GrokCli::new("/bin/true")),
            root.join("worktrees"),
        )
    }

    #[tokio::test]
    async fn commit_all_and_is_clean() {
        let (dir, repo) = temp_repo().await;
        let mgr = test_manager(dir.path());
        assert!(mgr.is_clean(&repo).await.unwrap());
        assert!(!mgr.commit_all(&repo, "noop").await.unwrap());
        std::fs::write(repo.join("b.txt"), "two\n").unwrap();
        assert!(!mgr.is_clean(&repo).await.unwrap());
        assert!(mgr.commit_all(&repo, "add b").await.unwrap());
        assert!(mgr.is_clean(&repo).await.unwrap());
    }

    #[tokio::test]
    async fn thread_worktree_land_and_conflict_flow() {
        let (dir, repo) = temp_repo().await;
        let mgr = test_manager(dir.path());
        let wt = mgr
            .create(
                &repo,
                CreateWorktreeRequest {
                    name: "t-abc".into(),
                    base_ref: None,
                    prefer_grok_cli: false,
                },
            )
            .await
            .unwrap();
        assert!(wt.branch.as_deref().unwrap().starts_with(THREAD_BRANCH_PREFIX));

        // Thread edits its copy; main is untouched.
        std::fs::write(wt.path.join("a.txt"), "thread version\n").unwrap();
        assert!(mgr.commit_all(&wt.path, "thread work").await.unwrap());
        assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "one\n");

        // Land: clean merge into main.
        let branch = mgr.current_branch(&wt.path).await.unwrap();
        match mgr.merge(&repo, &branch, "land").await.unwrap() {
            MergeOutcome::Merged => {}
            other => panic!("expected clean merge, got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "thread version\n"
        );

        // Diverge both sides on the same line → conflict on next land.
        std::fs::write(repo.join("a.txt"), "main again\n").unwrap();
        mgr.commit_all(&repo, "main change").await.unwrap();
        std::fs::write(wt.path.join("a.txt"), "thread again\n").unwrap();
        mgr.commit_all(&wt.path, "thread change").await.unwrap();
        match mgr.merge(&repo, &branch, "land 2").await.unwrap() {
            MergeOutcome::Conflicts { files } => {
                assert_eq!(files, vec!["a.txt".to_string()]);
            }
            other => panic!("expected conflicts, got {other:?}"),
        }
        mgr.merge_abort(&repo).await;
        assert!(mgr.is_clean(&repo).await.unwrap());

        // Sync: merge main INTO the worktree; conflict stays there.
        match mgr.merge(&wt.path, "main", "sync").await.unwrap() {
            MergeOutcome::Conflicts { files } => assert_eq!(files.len(), 1),
            other => panic!("expected sync conflicts, got {other:?}"),
        }
        assert!(!mgr.is_clean(&wt.path).await.unwrap());
        assert!(mgr.commit_all(&wt.path, "must not stage conflict markers").await.is_err());
        let conflicts = run_git(&wt.path, &["diff", "--name-only", "--diff-filter=U"]).await.unwrap();
        assert_eq!(conflicts.trim(), "a.txt");
    }

    #[tokio::test]
    async fn is_git_repo_detection() {
        let (dir, repo) = temp_repo().await;
        assert!(is_git_repo(&repo).await);
        assert!(!is_git_repo(dir.path()).await);
    }
}
