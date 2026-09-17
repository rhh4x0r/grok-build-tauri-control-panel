//! Read-only project map. Branch comparisons are against the local default branch.
use super::workspaces::default_branch;
use grok_worktree::run_git;
use serde::Deserialize;
use std::{path::Path, time::Duration};

#[derive(Debug, Clone)]
pub struct Branch {
    pub name: String,
    pub current: bool,
    pub ahead: usize,
    pub behind: usize,
    pub files: Vec<String>,
    pub commits: Vec<(String, String)>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub head_ref_name: String,
    pub base_ref_name: String,
    pub is_draft: bool,
    pub review_decision: String,
    pub mergeable: String,
    #[serde(default)]
    pub status_check_rollup: Option<Vec<serde_json::Value>>,
}

impl PullRequest {
    pub fn checks(&self) -> &'static str {
        let checks = self.status_check_rollup.as_deref().unwrap_or_default();
        if checks.is_empty() {
            return "No checks";
        }
        if checks.iter().any(|c| {
            matches!(
                c["conclusion"].as_str().or(c["state"].as_str()),
                Some("FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED")
            )
        }) {
            "Checks failing"
        } else if checks.iter().any(|c| {
            !matches!(
                c["conclusion"].as_str().or(c["state"].as_str()),
                Some("SUCCESS" | "NEUTRAL" | "SKIPPED")
            )
        }) {
            "Checks pending"
        } else {
            "Checks passed"
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProjectOverview {
    pub git_detected: bool,
    pub default_branch: String,
    pub branches: Vec<Branch>,
    pub prs: Vec<PullRequest>,
    pub pr_error: Option<String>,
}

pub async fn load(root: &str) -> Result<ProjectOverview, String> {
    let path = Path::new(root);
    if !path.is_absolute() {
        return Err("Project path must be absolute".into());
    }
    if !repository_detected(path).await? {
        return Ok(ProjectOverview {
            git_detected: false,
            default_branch: String::new(),
            branches: vec![],
            prs: vec![],
            pr_error: None,
        });
    }
    let base = default_branch(path).await?;
    let raw = run_git(
        path,
        &[
            "for-each-ref",
            "--format=%(refname:short)%09%(HEAD)",
            "refs/heads/",
        ],
    )
    .await
    .map_err(|e| e.to_string())?;
    let mut branches = Vec::new();
    for line in raw.lines() {
        let Some((name, head)) = line.split_once('\t') else {
            continue;
        };
        let reference = format!("refs/heads/{name}");
        let counts = run_git(
            path,
            &[
                "rev-list",
                "--left-right",
                "--count",
                &format!("refs/heads/{base}...{reference}"),
                "--",
            ],
        )
        .await
        .map_err(|e| e.to_string())?;
        let counts: Vec<usize> = counts
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        let files = run_git(
            path,
            &[
                "diff",
                "--name-only",
                &format!("refs/heads/{base}...{reference}"),
                "--",
            ],
        )
        .await
        .map_err(|e| e.to_string())?
        .lines()
        .map(str::to_string)
        .collect();
        let commits = run_git(path, &["log", "-6", "--format=%h%x09%s", &reference, "--"])
            .await
            .map_err(|e| e.to_string())?
            .lines()
            .filter_map(|l| {
                l.split_once('\t')
                    .map(|(a, b)| (a.to_string(), b.to_string()))
            })
            .collect();
        branches.push(Branch {
            name: name.into(),
            current: head.trim() == "*",
            ahead: *counts.get(1).unwrap_or(&0),
            behind: *counts.first().unwrap_or(&0),
            files,
            commits,
        });
    }
    branches.sort_by_key(|b| (b.name != base, b.name.clone()));
    let remote = run_git(path, &["remote", "get-url", "origin"])
        .await
        .is_ok();
    let (prs, pr_error) = if remote {
        match load_prs(path).await {
            Ok(prs) => (prs, None),
            Err(e) => (vec![], Some(e)),
        }
    } else {
        (vec![], Some("No origin remote configured".into()))
    };
    Ok(ProjectOverview {
        git_detected: true,
        default_branch: base,
        branches,
        prs,
        pr_error,
    })
}

/// Distinguish an ordinary folder from Git execution/access failures.
async fn repository_detected(path: &Path) -> Result<bool, String> {
    if !path.is_absolute() || !path.is_dir() {
        return Err("Choose an existing project folder with an absolute path.".into());
    }
    let output = tokio::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .env("LC_ALL", "C")
        .current_dir(path)
        .output()
        .await
        .map_err(|e| format!("Could not run Git: {e}"))?;
    if output.status.success() {
        return Ok(true);
    }
    let error = String::from_utf8_lossy(&output.stderr);
    if error.contains("not a git repository") {
        Ok(false)
    } else {
        Err(format!("Could not inspect repository: {}", error.trim()))
    }
}

/// Initialize only on an explicit UI action; never stage files or create a commit.
pub async fn initialize_repository(root: &str) -> Result<(), String> {
    let path = Path::new(root);
    // Also recognizes parent repositories and linked worktrees, avoiding nested repos.
    if repository_detected(path).await? {
        return Ok(());
    }
    run_git(path, &["init", "--initial-branch=main"])
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn load_prs(path: &Path) -> Result<Vec<PullRequest>, String> {
    let mut command = tokio::process::Command::new("gh");
    command.args(["pr", "list", "--state", "open", "--limit", "100", "--json", "number,title,url,headRefName,baseRefName,isDraft,reviewDecision,mergeable,statusCheckRollup"])
        .current_dir(path).kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(12), command.output())
        .await
        .map_err(|_| "PR refresh timed out. Try Refresh again.".to_string())?
        .map_err(|_| "Install GitHub CLI and sign in to load pull requests.".to_string())?;
    if !output.status.success() {
        return Err("PRs unavailable. Check GitHub CLI sign-in and repository access.".into());
    }
    serde_json::from_slice(&output.stdout).map_err(|_| "Could not read GitHub PR data".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn branch_map_reports_divergence_and_changed_files() {
        let temp = tempfile::tempdir().unwrap();
        let p = temp.path();
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.com"],
            vec!["commit", "--allow-empty", "-m", "initial"],
            vec!["checkout", "-b", "feature"],
        ] {
            run_git(p, &args).await.unwrap();
        }
        std::fs::write(p.join("hello.txt"), "hello").unwrap();
        run_git(p, &["add", "hello.txt"]).await.unwrap();
        run_git(p, &["commit", "-m", "add hello"]).await.unwrap();
        run_git(p, &["checkout", "main"]).await.unwrap();
        run_git(p, &["commit", "--allow-empty", "-m", "main progresses"])
            .await
            .unwrap();
        let map = load(p.to_str().unwrap()).await.unwrap();
        assert_eq!(map.default_branch, "main");
        assert!(map.branches[0].current);
        let feature = map.branches.iter().find(|b| b.name == "feature").unwrap();
        assert_eq!((feature.ahead, feature.behind), (1, 1));
        assert_eq!(feature.files, ["hello.txt"]);
        assert_eq!(feature.commits[0].1, "add hello");
        assert!(map.pr_error.is_some());
    }
    #[tokio::test]
    async fn initializes_plain_folder_without_staging_or_committing_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_str().unwrap();
        std::fs::write(temp.path().join("notes.txt"), "keep me").unwrap();
        assert!(!load(root).await.unwrap().git_detected);
        initialize_repository(root).await.unwrap();
        let map = load(root).await.unwrap();
        assert!(map.git_detected);
        assert_eq!(map.default_branch, "main");
        assert!(map.branches.is_empty());
        assert!(run_git(temp.path(), &["ls-files"])
            .await
            .unwrap()
            .trim()
            .is_empty());
        assert!(run_git(temp.path(), &["rev-parse", "--verify", "HEAD"])
            .await
            .is_err());
        assert_eq!(
            std::fs::read_to_string(temp.path().join("notes.txt")).unwrap(),
            "keep me"
        );
        initialize_repository(root).await.unwrap();
        let nested = temp.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        initialize_repository(nested.to_str().unwrap())
            .await
            .unwrap();
        assert!(!nested.join(".git").exists());
        assert!(initialize_repository("relative/path").await.is_err());
    }

    #[test]
    fn pr_checks_handle_both_github_check_formats() {
        let mut pr: PullRequest = serde_json::from_value(serde_json::json!({"number":1,"title":"Test","url":"https://github.com/a/b/pull/1","headRefName":"feature","baseRefName":"main","isDraft":false,"reviewDecision":"","mergeable":"UNKNOWN","statusCheckRollup":[{"state":"SUCCESS"},{"conclusion":"SUCCESS"}]})).unwrap();
        assert_eq!(pr.checks(), "Checks passed");
        pr.status_check_rollup = Some(vec![serde_json::json!({"state":"PENDING"})]);
        assert_eq!(pr.checks(), "Checks pending");
        pr.status_check_rollup = Some(vec![serde_json::json!({"conclusion":"FAILURE"})]);
        assert_eq!(pr.checks(), "Checks failing");
    }
}
