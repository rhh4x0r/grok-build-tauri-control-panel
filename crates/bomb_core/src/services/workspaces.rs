//! Project/workspace navigation and explicit Git operations.
use super::*;
use grok_persistence::WorkspaceRecord;
use grok_worktree::run_git;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceReview {
    pub branch: String,
    pub base: String,
    pub ahead: usize,
    pub behind: usize,
    pub files: Vec<String>,
    pub checkpoints: Vec<(String, String)>,
    pub diff: String,
    pub remote: bool,
    pub pr: Option<String>,
    pub conflicts: Vec<String>,
}

pub async fn default_branch(root: &Path) -> Result<String, String> {
    if let Ok(r) = run_git(
        root,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .await
    {
        return Ok(r.trim().trim_start_matches("origin/").to_string());
    }
    for name in ["main", "master"] {
        if run_git(
            root,
            &["show-ref", "--verify", &format!("refs/heads/{name}")],
        )
        .await
        .is_ok()
        {
            return Ok(name.into());
        }
    }
    let branch = run_git(root, &["symbolic-ref", "--short", "HEAD"])
        .await
        .map_err(err)?;
    Ok(branch.trim().into())
}

pub async fn workspace_base(root: &Path) -> Result<String, String> {
    let branch = default_branch(root).await?;
    let remote = format!("origin/{branch}");
    if run_git(root, &["rev-parse", "--verify", &remote])
        .await
        .is_ok()
    {
        Ok(remote)
    } else {
        Ok(branch)
    }
}

/// Idempotently migrate existing conversations without changing their files.
pub async fn list_workspaces(state: &AppState) -> Result<Vec<WorkspaceRecord>, String> {
    let _gate = state.workspace_gate.lock().await;
    let mut workspaces = state.persistence.list_workspaces().map_err(err)?;
    for rec in state.persistence.list_sessions().map_err(err)? {
        if rec.cwd.is_empty()
            || workspaces
                .iter()
                .any(|w| w.threads.contains(&rec.id.to_string()))
        {
            continue;
        }
        let root = extract_meta_string(&rec.metadata_json, "projectRoot")
            .or_else(|| extract_meta_string(&rec.metadata_json, "project_root"))
            .unwrap_or_else(|| rec.cwd.clone());
        let inline = rec.worktree.is_none() || root == rec.cwd;
        let existing = workspaces
            .iter()
            .find(|w| w.path == rec.cwd)
            .map(|w| w.id.clone());
        let wid = if let Some(id) = existing {
            id
        } else {
            let branch = state
                .worktrees
                .current_branch(Path::new(&rec.cwd))
                .await
                .unwrap_or_default();
            let w = WorkspaceRecord {
                id: Uuid::new_v4().to_string(),
                project_root: root.clone(),
                name: if inline {
                    "Inline (read-only)".into()
                } else {
                    extract_meta_string(&rec.metadata_json, "label")
                        .unwrap_or_else(|| branch.clone())
                },
                branch,
                path: rec.cwd.clone(),
                base_ref: workspace_base(Path::new(&root))
                    .await
                    .unwrap_or_else(|_| "HEAD".into()),
                created_at: rec.created_at.to_rfc3339(),
                archived_at: None,
                inline,
                threads: vec![],
            };
            state.persistence.save_workspace(&w).map_err(err)?;
            let id = w.id.clone();
            workspaces.push(w);
            id
        };
        state
            .persistence
            .attach_workspace(rec.id, &wid)
            .map_err(err)?;
        if let Some(w) = workspaces.iter_mut().find(|w| w.id == wid) {
            w.threads.push(rec.id.to_string());
        }
    }
    Ok(workspaces)
}

pub fn workspace(state: &AppState, id: &str) -> Result<WorkspaceRecord, String> {
    state
        .persistence
        .list_workspaces()
        .map_err(err)?
        .into_iter()
        .find(|w| w.id == id)
        .ok_or_else(|| "Workspace not found".into())
}

pub fn ensure_idle(state: &AppState, w: &WorkspaceRecord) -> Result<(), String> {
    if state
        .workspace_turns
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&w.id)
    {
        return Err("This workspace is finishing a turn or checkpoint. Please wait.".into());
    }
    for t in &w.threads {
        if let Ok(id) = Uuid::parse_str(t) {
            if let Ok(s) = state.registry.get_snapshot(id) {
                if !matches!(
                    s.metadata.status,
                    grok_events::SessionStatus::Idle
                        | grok_events::SessionStatus::Failed
                        | grok_events::SessionStatus::Completed
                        | grok_events::SessionStatus::Cancelled
                ) {
                    return Err(
                        "This workspace is busy. Wait for its current conversation to finish."
                            .into(),
                    );
                }
            }
        }
    }
    Ok(())
}

pub async fn review_workspace(state: &AppState, id: String) -> Result<WorkspaceReview, String> {
    let w = workspace(state, &id)?;
    let path = Path::new(&w.path);
    let base = workspace_base(Path::new(&w.project_root)).await?;
    let range = format!("{base}...HEAD");
    let counts = run_git(path, &["rev-list", "--left-right", "--count", &range])
        .await
        .map_err(err)?;
    let counts: Vec<usize> = counts
        .split_whitespace()
        .filter_map(|n| n.parse().ok())
        .collect();
    let comparison = run_git(path, &["merge-base", &base, "HEAD"])
        .await
        .map_err(err)?;
    let comparison = comparison.trim();
    let files = run_git(path, &["diff", "--name-only", comparison, "--"])
        .await
        .map_err(err)?;
    let mut files: Vec<String> = files.lines().map(String::from).collect();
    let untracked = run_git(path, &["ls-files", "--others", "--exclude-standard"])
        .await
        .map_err(err)?;
    files.extend(untracked.lines().map(String::from));
    files.sort();
    files.dedup();
    let diff = run_git(
        path,
        &["diff", "--no-ext-diff", "--no-color", comparison, "--"],
    )
    .await
    .map_err(err)?;
    let log = run_git(
        path,
        &["log", "--format=%H%x09%s", &format!("{base}..HEAD")],
    )
    .await
    .map_err(err)?;
    let remote = run_git(path, &["remote", "get-url", "origin"])
        .await
        .is_ok();
    let conflicts = run_git(path, &["diff", "--name-only", "--diff-filter=U"])
        .await
        .map_err(err)?
        .lines()
        .map(String::from)
        .collect();
    let pr = if remote {
        pr_status(&w.path).await
    } else {
        None
    };
    Ok(WorkspaceReview {
        branch: w.branch,
        base,
        ahead: *counts.get(1).unwrap_or(&0),
        behind: *counts.first().unwrap_or(&0),
        files,
        checkpoints: log
            .lines()
            .filter_map(|l| l.split_once('\t').map(|(a, b)| (a.into(), b.into())))
            .collect(),
        diff,
        remote,
        pr,
        conflicts,
    })
}

pub async fn fetch_project(root: String) -> Result<(), String> {
    let path = Path::new(&root);
    if !path.is_absolute() {
        return Err("Project path must be absolute".into());
    }
    run_git(path, &["fetch", "--prune", "origin"])
        .await
        .map_err(err)?;
    Ok(())
}

pub async fn rename_workspace(state: &AppState, id: String, name: String) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Workspace name cannot be empty".into());
    }
    let mut w = workspace(state, &id)?;
    w.name = name.trim().into();
    state.persistence.save_workspace(&w).map_err(err)
}

pub async fn archive_workspace(state: &AppState, id: String) -> Result<(), String> {
    let _gate = state.workspace_gate.lock().await;
    let mut w = workspace(state, &id)?;
    ensure_idle(state, &w)?;
    if w.inline {
        return Err("The main checkout cannot be archived".into());
    }
    if !state
        .worktrees
        .is_clean(Path::new(&w.path))
        .await
        .map_err(err)?
    {
        return Err(
            "Save a checkpoint before archiving; this workspace has unsaved changes".into(),
        );
    }
    for t in &w.threads {
        if let Ok(id) = Uuid::parse_str(t) {
            if state.registry.is_live(id) {
                state.registry.remove_session(id).await.map_err(err)?;
            }
        }
    }
    state
        .worktrees
        .remove(Path::new(&w.project_root), &w.path, false)
        .await
        .map_err(err)?;
    w.archived_at = Some(Utc::now().to_rfc3339());
    state.persistence.save_workspace(&w).map_err(err)
}

/// All destructive operations require a concrete confirmation in the view.
pub async fn workspace_action(
    state: &AppState,
    id: String,
    action: String,
    value: String,
) -> Result<String, String> {
    let _gate = state.workspace_gate.lock().await;
    let w = workspace(state, &id)?;
    ensure_idle(state, &w)?;
    if w.inline || w.archived_at.is_some() {
        return Err("Create an active workspace to make changes".into());
    }
    let path = Path::new(&w.path);
    if matches!(action.as_str(), "push" | "pr")
        && !state.worktrees.is_clean(path).await.map_err(err)?
    {
        return Err("Save a checkpoint before shipping so all changes are included".into());
    }
    let result = match action.as_str() {
        "checkpoint" => {
            state
                .worktrees
                .commit_all(path, &value)
                .await
                .map_err(err)?;
            "Checkpoint saved".into()
        }
        "update" => {
            if run_git(path, &["remote", "get-url", "origin"])
                .await
                .is_ok()
            {
                fetch_project(w.project_root.clone()).await?;
            }
            if !state.worktrees.is_clean(path).await.map_err(err)? {
                return Err("Save a checkpoint before updating".into());
            }
            let base = workspace_base(Path::new(&w.project_root)).await?;
            match state
                .worktrees
                .merge(path, &base, &format!("Update from {base}"))
                .await
                .map_err(err)?
            {
                grok_worktree::MergeOutcome::Merged => format!("Updated from {base}"),
                grok_worktree::MergeOutcome::Conflicts { files } => format!(
                    "Resolve these conflicts in this workspace: {}",
                    files.join(", ")
                ),
            }
        }
        "push" => {
            run_git(path, &["push", "-u", "origin", &w.branch])
                .await
                .map_err(err)?;
            "Workspace pushed".into()
        }
        "pr" => {
            run_git(path, &["push", "-u", "origin", &w.branch])
                .await
                .map_err(err)?;
            let out = tokio::process::Command::new("gh")
                .args([
                    "pr",
                    "create",
                    "--head",
                    &w.branch,
                    "--base",
                    &default_branch(Path::new(&w.project_root)).await?,
                    "--title",
                    &w.name,
                    "--body",
                    &value,
                ])
                .current_dir(path)
                .output()
                .await
                .map_err(err)?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).into());
            }
            String::from_utf8_lossy(&out.stdout).trim().into()
        }
        "merge" => {
            if run_git(path, &["remote", "get-url", "origin"])
                .await
                .is_ok()
            {
                return Err("This project has origin; use Push or Open pull request".into());
            }
            let root = Path::new(&w.project_root);
            if !state.worktrees.is_clean(root).await.map_err(err)?
                || !state.worktrees.is_clean(path).await.map_err(err)?
            {
                return Err(
                    "Both the project checkout and workspace must be clean before merging".into(),
                );
            }
            let base = default_branch(root).await?;
            if state.worktrees.current_branch(root).await.map_err(err)? != base {
                return Err(format!("Check out {base} in the project before merging"));
            }
            match state
                .worktrees
                .merge(root, &w.branch, &format!("Merge {}", w.name))
                .await
                .map_err(err)?
            {
                grok_worktree::MergeOutcome::Merged => format!("Merged into {base}"),
                grok_worktree::MergeOutcome::Conflicts { .. } => {
                    state.worktrees.merge_abort(root).await;
                    return Err(
                        "Merge conflicts: Update the workspace and resolve conflicts there first"
                            .into(),
                    );
                }
            }
        }
        "revert-file" => {
            let review = review_workspace(state, id.clone()).await?;
            if !review.files.contains(&value) {
                return Err("Choose a changed file".into());
            }
            if !state.worktrees.is_clean(path).await.map_err(err)? {
                return Err("Save a checkpoint before reverting a file".into());
            }
            let base = run_git(path, &["merge-base", &review.base, "HEAD"])
                .await
                .map_err(err)?;
            run_git(
                path,
                &[
                    "restore",
                    "--source",
                    base.trim(),
                    "--staged",
                    "--worktree",
                    "--",
                    &value,
                ],
            )
            .await
            .map_err(err)?;
            state
                .worktrees
                .commit_all(path, &format!("Revert {value}"))
                .await
                .map_err(err)?;
            format!("Reverted {value} in a new checkpoint")
        }
        "squash" => {
            if !state.worktrees.is_clean(path).await.map_err(err)? {
                return Err("Save a checkpoint before squashing".into());
            }
            if run_git(path, &["rev-parse", "--verify", "@{upstream}"])
                .await
                .is_ok()
            {
                return Err("This branch has been published. Squashing is only available before the first push.".into());
            }
            let base = workspace_base(Path::new(&w.project_root)).await?;
            let base = run_git(path, &["merge-base", &base, "HEAD"])
                .await
                .map_err(err)?;
            let head = run_git(path, &["rev-parse", "HEAD"]).await.map_err(err)?;
            if head == base {
                return Err("No checkpoints to squash".into());
            }
            let backup = format!("bomb-backup/{}", Uuid::new_v4());
            run_git(path, &["branch", &backup, head.trim()])
                .await
                .map_err(err)?;
            run_git(path, &["reset", "--soft", base.trim()])
                .await
                .map_err(err)?;
            if let Err(e) = run_git(path, &["commit", "-m", &value]).await {
                run_git(path, &["reset", "--soft", head.trim()])
                    .await
                    .map_err(err)?;
                return Err(e.to_string());
            }
            format!("Checkpoints squashed; original history kept on {backup}")
        }
        "restore" => {
            if !state.worktrees.is_clean(path).await.map_err(err)? {
                return Err("Save a checkpoint before restoring".into());
            }
            let review = review_workspace(state, id.clone()).await?;
            if !review.checkpoints.iter().any(|(sha, _)| sha == &value) {
                return Err("Unknown checkpoint".into());
            }
            run_git(
                path,
                &["restore", "--source", &value, "--staged", "--worktree", "."],
            )
            .await
            .map_err(err)?;
            state
                .worktrees
                .commit_all(path, &format!("Restore checkpoint {}", &value[..8]))
                .await
                .map_err(err)?;
            "Checkpoint restored as a new commit".into()
        }
        _ => return Err("Unknown workspace action".into()),
    };
    for t in &w.threads {
        if let Ok(id) = Uuid::parse_str(t) {
            let _ = state
                .persistence
                .append_message(id, "system", &result, Utc::now());
            state.event_bus.emit(ControlEvent::Raw {
                session_id: Some(id),
                payload: serde_json::json!({"channel":"term", "line":result}),
            });
        }
    }
    Ok(result)
}

/// Held until the turn's checkpoint finishes, including error paths.
pub struct WorkspaceTurn {
    active: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    id: String,
}
impl WorkspaceTurn {
    pub fn new(
        active: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
        id: &str,
    ) -> Result<Self, String> {
        if !active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.into())
        {
            return Err("Workspace is busy".into());
        }
        Ok(Self {
            active,
            id: id.into(),
        })
    }
}
impl Drop for WorkspaceTurn {
    fn drop(&mut self) {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProjectStatus {
    pub branch: String,
    pub dirty: bool,
    pub ahead: usize,
    pub behind: usize,
    pub remote: bool,
    pub error: Option<String>,
}

pub async fn project_status(root: &str) -> Result<ProjectStatus, String> {
    let path = Path::new(root);
    let branch = default_branch(path).await?;
    let dirty = !run_git(path, &["status", "--porcelain"])
        .await
        .map_err(err)?
        .is_empty();
    let remote = run_git(path, &["remote", "get-url", "origin"])
        .await
        .is_ok();
    let counts = if remote {
        run_git(
            path,
            &[
                "rev-list",
                "--left-right",
                "--count",
                &format!("origin/{branch}...{branch}"),
            ],
        )
        .await
        .unwrap_or_default()
    } else {
        String::new()
    };
    let counts: Vec<usize> = counts
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    Ok(ProjectStatus {
        branch,
        dirty,
        ahead: *counts.get(1).unwrap_or(&0),
        behind: *counts.first().unwrap_or(&0),
        remote,
        error: None,
    })
}

pub async fn pull_project(root: String) -> Result<String, String> {
    let path = Path::new(&root);
    let status = project_status(&root).await?;
    if status.dirty {
        return Err("The main checkout has uncommitted changes; Pull is blocked".into());
    }
    let current = run_git(path, &["branch", "--show-current"])
        .await
        .map_err(err)?;
    if current.trim() != status.branch {
        return Err(format!("Check out {} before pulling", status.branch));
    }
    run_git(path, &["pull", "--ff-only", "origin", &status.branch])
        .await
        .map_err(err)
}

pub async fn refresh_projects(
    state: &AppState,
    fetch: bool,
) -> Result<Vec<(String, ProjectStatus)>, String> {
    let mut roots = super::list_projects(state).await?;
    for w in state.persistence.list_workspaces().map_err(err)? {
        if !roots.contains(&w.project_root) {
            roots.push(w.project_root);
        }
    }
    let mut result = Vec::new();
    for root in roots {
        if fetch {
            let _ = fetch_project(root.clone()).await;
        }
        let status = project_status(&root)
            .await
            .unwrap_or_else(|e| ProjectStatus {
                error: Some(e),
                ..Default::default()
            });
        result.push((root, status));
    }
    Ok(result)
}

pub async fn pr_status(path: &str) -> Option<String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::process::Command::new("gh")
            .args([
                "pr",
                "view",
                "--json",
                "number,state,reviewDecision,statusCheckRollup",
            ])
            .current_dir(path)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let checks = v["statusCheckRollup"]
        .as_array()
        .map(|c| {
            if c.iter().any(|x| {
                matches!(
                    x["conclusion"].as_str(),
                    Some("FAILURE" | "TIMED_OUT" | "CANCELLED")
                )
            }) {
                "checks failing"
            } else if c
                .iter()
                .any(|x| x["status"].as_str().is_some_and(|s| s != "COMPLETED"))
            {
                "checks pending"
            } else if c.is_empty() {
                "no checks"
            } else {
                "checks complete"
            }
        })
        .unwrap_or("no checks");
    Some(format!(
        "PR #{} · {} · {}",
        v["number"],
        v["state"].as_str().unwrap_or("unknown"),
        checks
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn turn_guard_blocks_overlapping_turns_and_releases_on_error() {
        let active = std::sync::Arc::new(std::sync::Mutex::new(Default::default()));
        let guard = WorkspaceTurn::new(active.clone(), "a").unwrap();
        assert!(WorkspaceTurn::new(active.clone(), "a").is_err());
        assert!(WorkspaceTurn::new(active.clone(), "b").is_ok());
        drop(guard);
        assert!(WorkspaceTurn::new(active, "a").is_ok());
    }
    #[tokio::test]
    async fn workspace_lifecycle_checkpoints_sharing_inline_and_archive() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo"); std::fs::create_dir(&root).unwrap();
        for args in [vec!["init", "-b", "main"], vec!["config", "user.name", "Test"], vec!["config", "user.email", "test@example.com"], vec!["commit", "--allow-empty", "-m", "initial"]] { run_git(&root, &args).await.unwrap(); }
        let home = temp.path().to_path_buf(); let grok = home.join("grok"); let panel = grok.join("panel");
        let state = AppState::initialize_with_paths(grok_config::GrokPaths {
            home_dir: home.clone(), grok_dir: grok.clone(), config_file: panel.join("config.toml"),
            grok_cli_config_file: grok.join("config.toml"), worktrees_dir: home.join("worktrees"),
            memory_dir: panel.join("memory"), sessions_dir: panel.join("sessions"), panel_dir: panel,
            project_config_file: None, project_root: None,
        }).await.unwrap();
        let options = SpawnOptions { model: Some("mock".into()), prompt: Some("Build dark mode".into()), ..Default::default() };
        let first = super::super::start_session(&state, root.display().to_string(), options.clone()).await.unwrap();
        super::super::wait_until_idle(&state, &first.id, std::time::Duration::from_secs(5)).await.unwrap();
        let rows = list_workspaces(&state).await.unwrap(); let w = rows[0].clone();
        assert!(!w.inline); assert_ne!(w.path, root.display().to_string()); assert!(w.branch.starts_with("bomb/"));
        std::fs::write(Path::new(&w.path).join("dark.txt"), "dark mode").unwrap();
        super::super::send_prompt(&state, first.id.clone(), "Add dark mode".into(), None, None, None, None, None, None).await.unwrap();
        assert!(workspace_action(&state, w.id.clone(), "archive".into(), String::new()).await.is_err());
        let overlapping = SpawnOptions { workspace_id: Some(w.id.clone()), ..options.clone() };
        assert!(super::super::start_session(&state, root.display().to_string(), overlapping.clone()).await.is_err());
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                if state.worktrees.is_clean(Path::new(&w.path)).await.unwrap() && !state.workspace_turns.lock().unwrap().contains(&w.id) { break; }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }).await.unwrap();
        let log = run_git(Path::new(&w.path), &["log", "-1", "--format=%B"]).await.unwrap();
        assert!(log.contains("Bomb-Thread:")); assert!(!root.join("dark.txt").exists());
        let second = super::super::start_session(&state, root.display().to_string(), overlapping).await.unwrap();
        super::super::wait_until_idle(&state, &second.id, std::time::Duration::from_secs(5)).await.unwrap();
        assert_eq!(workspace(&state, &w.id).unwrap().threads.len(), 2);
        let second_id = Uuid::parse_str(&second.id).unwrap();
        super::super::set_approval_mode(&state, second.id.clone(), "ask".into()).await.unwrap();
        assert_eq!(state.registry.get_snapshot(second_id).unwrap().metadata.approval_mode, grok_control_core::ApprovalMode::Ask);
        state.registry.remove_session(second_id).await.unwrap();
        super::super::set_approval_mode(&state, second.id.clone(), "auto".into()).await.unwrap();
        let saved = state.persistence.get_session(second_id).unwrap();
        let metadata: serde_json::Value = serde_json::from_str(&saved.metadata_json).unwrap();
        assert_eq!(metadata["metadata"]["approvalMode"], "auto");
        assert_eq!(metadata["metadata"]["planMode"], false);
        assert_eq!(metadata["metadata"]["alwaysApprove"], false);

        super::super::remove_session(&state, first.id, Some(true)).await.unwrap();
        assert!(Path::new(&w.path).exists());
        let inline = super::super::start_session(&state, root.display().to_string(), SpawnOptions { isolate_worktree: false, ..options }).await.unwrap();
        let inline_id = Uuid::parse_str(&inline.id).unwrap();
        assert!(state.registry.get_snapshot(inline_id).unwrap().metadata.read_only);
        assert!(super::super::set_approval_mode(&state, inline.id.clone(), "yolo".into()).await.is_err());
        let review = review_workspace(&state, w.id.clone()).await.unwrap(); assert_eq!(review.files, vec!["dark.txt"]);
        archive_workspace(&state, w.id.clone()).await.unwrap();
        assert!(!Path::new(&w.path).exists());
        assert!(workspace(&state, &w.id).unwrap().archived_at.is_some());
        assert!(run_git(&root, &["show-ref", "--verify", &format!("refs/heads/{}", w.branch)]).await.is_ok());
        super::super::shutdown_all(&state).await.unwrap();
    }

    #[tokio::test]
    async fn base_is_default_branch_even_when_checkout_is_on_feature() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.com"],
            vec!["commit", "--allow-empty", "-m", "initial"],
            vec!["checkout", "-b", "feature"],
        ] {
            run_git(root, &args).await.unwrap();
        }
        assert_eq!(workspace_base(root).await.unwrap(), "main");
        run_git(root, &["update-ref", "refs/remotes/origin/main", "HEAD"])
            .await
            .unwrap();
        assert_eq!(workspace_base(root).await.unwrap(), "origin/main");
        let status = project_status(root.to_str().unwrap()).await.unwrap();
        assert_eq!(status.branch, "main");
        assert!(!status.dirty);
    }
}
