//! Read-only project map. Branch comparisons are against the local default branch.
use super::workspaces::default_branch;
use grok_worktree::run_git;
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    /// The branch this one is compared with: a thread's starting branch, otherwise the default branch.
    pub base: String,
    pub current: bool,
    pub ahead: usize,
    pub behind: usize,
    pub files: Vec<String>,
    pub commits: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectOverview {
    pub git_detected: bool,
    pub default_branch: String,
    pub branches: Vec<Branch>,
    pub prs: Vec<PullRequest>,
    pub pr_error: Option<String>,
}

/// Overview where each thread's branch is compared with the branch it started from.
pub async fn load_for_project(state: &crate::state::AppState, root: &str) -> Result<ProjectOverview, String> {
    let mut bases = std::collections::HashMap::new();
    for w in state.persistence.list_workspaces().map_err(|e| e.to_string())? {
        if w.project_root == root && !w.inline && !w.shared_checkout && w.archived_at.is_none() {
            if let Ok(base) = super::workspaces::merge_target(&w).await { bases.insert(w.branch.clone(), base); }
        }
    }
    load(root, &bases).await
}

/// `bases` maps a branch to the branch it should be compared with; others use the default branch.
pub async fn load(root: &str, bases: &std::collections::HashMap<String, String>) -> Result<ProjectOverview, String> {
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
        let default = base.clone();
        let base = bases.get(name).filter(|b| b.as_str() != name).cloned().unwrap_or(default);
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
            base: base.clone(),
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

/// Plain-language relation of a branch to the default branch.
/// `ahead` counts commits only the branch has; `behind` counts commits only the default branch has.
pub fn describe_relation(ahead: usize, behind: usize, base: &str) -> String {
    let changes = |n: usize| if n == 1 { "1 saved change".to_string() } else { format!("{n} saved changes") };
    let updates = |n: usize| if n == 1 { "1 newer update".to_string() } else { format!("{n} newer updates") };
    match (ahead, behind) {
        (0, 0) => format!("Same as {base} · no new work yet"),
        (a, 0) => format!("{} not in {base} yet · ready to merge", changes(a)),
        (0, _) => format!("Nothing to merge · everything here is already in {base}"),
        (a, b) => format!("{} not in {base} yet · {base} has {} this hasn’t picked up", changes(a), updates(b)),
    }
}

/// The same relation in a few words, for toolbars. `unsaved` counts edited files not saved to the branch yet.
pub fn describe_relation_short(unsaved: usize, ahead: usize, behind: usize, base: &str) -> String {
    let mut parts = Vec::new();
    if unsaved > 0 { parts.push(format!("{unsaved} unsaved {}", if unsaved == 1 { "edit" } else { "edits" })); }
    if ahead > 0 { parts.push(format!("{ahead} {} to merge", if ahead == 1 { "change" } else { "changes" })); }
    if behind > 0 && (ahead > 0 || unsaved > 0) { parts.push(format!("{base} has {behind} newer")); }
    if parts.is_empty() { parts.push(if behind > 0 { format!("Already in {base}") } else { format!("Same as {base}") }); }
    parts.join(" · ")
}

/// Distinguish an ordinary folder from Git execution/access failures.
pub(super) async fn repository_detected(path: &Path) -> Result<bool, String> {
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
    #[test]
    fn relation_is_described_without_git_jargon() {
        use super::describe_relation as d;
        use super::describe_relation_short as short;
        assert_eq!(short(8, 1, 0, "main"), "8 unsaved edits · 1 change to merge");
        assert_eq!(short(0, 2, 3, "main"), "2 changes to merge · main has 3 newer");
        assert_eq!(short(0, 0, 4, "main"), "Already in main");
        assert_eq!(short(0, 0, 0, "main"), "Same as main");
        assert_eq!(d(0, 0, "main"), "Same as main · no new work yet");
        assert_eq!(d(1, 0, "main"), "1 saved change not in main yet · ready to merge");
        assert_eq!(d(0, 7, "main"), "Nothing to merge · everything here is already in main");
        assert_eq!(d(2, 1, "main"), "2 saved changes not in main yet · main has 1 newer update this hasn’t picked up");
    }
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
        let map = load(p.to_str().unwrap(), &Default::default()).await.unwrap();
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
        assert!(!load(root, &Default::default()).await.unwrap().git_detected);
        initialize_repository(root).await.unwrap();
        let map = load(root, &Default::default()).await.unwrap();
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
