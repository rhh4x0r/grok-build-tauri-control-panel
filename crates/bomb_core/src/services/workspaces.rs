//! Project/workspace navigation and explicit Git operations.
use super::*;
use grok_persistence::WorkspaceRecord;
use grok_worktree::run_git;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceReview {
    pub branch: String,
    pub default_branch: String,
    pub comparison: String,
    pub dirty: Vec<super::git_ui::FileChange>,
    pub branch_files: Vec<super::git_ui::FileChange>,
    pub upstream: Option<String>,
    pub unpushed: usize,
    pub remote_behind: usize,
    pub main_unpushed: usize,
    pub base: String,
    pub ahead: usize,
    pub behind: usize,
    pub files: Vec<String>,
    pub checkpoints: Vec<(String, String)>,
    pub diff: String,
    pub remote: bool,
    pub pr: Option<String>,
    pub pr_url: Option<String>,
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

/// The local branch a thread's work merges back into: the branch it started from.
/// `base_ref` may be a local name, `origin/<name>` (migrated records) or `HEAD`; anything unusable falls back to the default branch.
pub async fn merge_target(w: &WorkspaceRecord) -> Result<String, String> {
    let root = Path::new(&w.project_root);
    let base = w.base_ref.trim();
    let local = if run_git(root, &["show-ref", "--verify", &format!("refs/remotes/{base}")]).await.is_ok() {
        base.split_once('/').map(|(_, name)| name).unwrap_or(base)
    } else {
        base
    };
    if !local.is_empty() && local != "HEAD" && local != w.branch
        && run_git(root, &["show-ref", "--verify", &format!("refs/heads/{local}")]).await.is_ok()
    {
        return Ok(local.to_string());
    }
    default_branch(root).await
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
                shared_checkout: inline,
                read_only: inline,
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
        .ok_or_else(|| "Thread not found".into())
}

pub fn ensure_idle(state: &AppState, w: &WorkspaceRecord) -> Result<(), String> {
    if state.foundry.owns_cwd(&w.path) {return Err("A Foundry run owns this thread folder. Stop it before changing the workspace.".into());}
    if state
        .workspace_turns
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&w.path)
    {
        return Err("This thread is finishing a turn or checkpoint. Please wait.".into());
    }
    if state.registry.list_sessions().iter().any(|s|s.cwd==w.path && matches!(s.status,grok_events::SessionStatus::Running|grok_events::SessionStatus::WaitingApproval|grok_events::SessionStatus::Starting)) {return Err("Another conversation is using this working copy. Wait for it to finish.".into());}
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
                        "This thread is busy. Wait for its current conversation to finish."
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
    let default_branch=default_branch(Path::new(&w.project_root)).await?;
    // Standing is measured against the branch this thread started from.
    let base = merge_target(&w).await?;
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
    let pr_url=if pr.is_some(){
        tokio::time::timeout(std::time::Duration::from_secs(5),tokio::process::Command::new("gh").args(["pr","view","--json","url","--jq",".url"]).current_dir(path).output()).await.ok().and_then(Result::ok).filter(|o|o.status.success()).map(|o|String::from_utf8_lossy(&o.stdout).trim().to_string())
    }else{None};
    let dirty=super::git_ui::changes(path,"HEAD").await?;
    let branch_files=super::git_ui::changes(path,comparison).await?;
    let upstream=run_git(path,&["rev-parse","--abbrev-ref","--symbolic-full-name","@{upstream}"]).await.ok().map(|s|s.trim().to_string());
    let sync=if let Some(upstream)=&upstream {run_git(path,&["rev-list","--left-right","--count",&format!("{upstream}...HEAD")]).await.unwrap_or_default()}else{String::new()};
    let sync:Vec<usize>=sync.split_whitespace().filter_map(|n|n.parse().ok()).collect();
    let main_unpushed=run_git(Path::new(&w.project_root),&["rev-list","--count",&format!("origin/{default_branch}..{default_branch}")]).await.ok().and_then(|s|s.trim().parse().ok()).unwrap_or(0);
    Ok(WorkspaceReview {
        branch: state.worktrees.current_branch(path).await.map_err(err)?,
        default_branch,
        comparison: comparison.into(),
        dirty, branch_files, upstream,
        unpushed: *sync.get(1).unwrap_or(&0),remote_behind:*sync.first().unwrap_or(&0),main_unpushed,
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
        pr_url,
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
        return Err("Thread name cannot be empty".into());
    }
    let mut w = workspace(state, &id)?;
    w.name = name.trim().into();
    state.persistence.save_workspace(&w).map_err(err)
}

pub async fn archive_workspace(state: &AppState, id: String) -> Result<(), String> {
    let _gate = state.workspace_gate.lock().await;
    let mut w = workspace(state, &id)?;
    ensure_idle(state, &w)?;
    for other in state.persistence.list_workspaces().map_err(err)?.iter().filter(|other|other.path==w.path && other.id!=w.id) {ensure_idle(state,other)?;}
    if w.inline || w.shared_checkout {
        return Err("Shared checkouts cannot be archived or removed".into());
    }
    if !state
        .worktrees
        .is_clean(Path::new(&w.path))
        .await
        .map_err(err)?
    {
        return Err(
            "Save a checkpoint before archiving; this thread has unsaved changes".into(),
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

/// Threads build on each other when one started from another's branch. A chain, never a tree:
/// each thread has at most one parent, and children wait for the parent to land before they merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackLink {
    pub child: String,
    pub parent: String,
    pub parent_name: String,
    /// The parent has saved work the child has not picked up yet.
    pub behind_parent: usize,
}

/// The parent of every thread that builds on another open thread in this project.
pub async fn stack_links(state: &AppState, root: &str) -> Result<Vec<StackLink>, String> {
    let open: Vec<WorkspaceRecord> = state.persistence.list_workspaces().map_err(err)?.into_iter()
        .filter(|w| w.project_root == root && !w.inline && !w.shared_checkout && w.archived_at.is_none()).collect();
    let mut links = Vec::new();
    for w in &open {
        let base = w.base_ref.trim().trim_start_matches("origin/");
        let Some(parent) = open.iter().find(|p| p.id != w.id && p.branch == base) else { continue };
        let counts = run_git(Path::new(root), &["rev-list", "--left-right", "--count", &format!("refs/heads/{}...refs/heads/{}", w.branch, parent.branch), "--"]).await.unwrap_or_default();
        let behind_parent = counts.split_whitespace().nth(1).and_then(|n| n.parse().ok()).unwrap_or(0);
        links.push(StackLink { child: w.id.clone(), parent: parent.id.clone(), parent_name: parent.name.clone(), behind_parent });
    }
    Ok(links)
}

/// The open thread this one builds on, if any.
pub async fn parent_of(state: &AppState, w: &WorkspaceRecord) -> Result<Option<WorkspaceRecord>, String> {
    let base = w.base_ref.trim().trim_start_matches("origin/").to_string();
    Ok(state.persistence.list_workspaces().map_err(err)?.into_iter()
        .find(|p| p.id != w.id && p.project_root == w.project_root && !p.inline && !p.shared_checkout && p.archived_at.is_none() && p.branch == base))
}

/// What happened to one child when its parent landed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Restack {
    /// Brought up to date with `main` and now builds on it directly.
    Updated,
    /// Busy or unsaved: left alone, still pointing at the old parent branch.
    Skipped,
    /// The child and the new base changed the same lines; the merge was left for the child's agent.
    NeedsHand(Vec<String>),
}

/// A parent's work is in `target` now: bring each child up to date with it and re-point the child at it,
/// so the child's next merge goes there. Merges, never rebases: nothing is rewritten or force-pushed.
pub async fn restack_children(state: &AppState, parent: &WorkspaceRecord, target: &str) -> Result<Vec<(WorkspaceRecord, Restack)>, String> {
    let _gate = state.workspace_gate.lock().await;
    let children: Vec<WorkspaceRecord> = state.persistence.list_workspaces().map_err(err)?.into_iter()
        .filter(|c| c.project_root == parent.project_root && c.id != parent.id && !c.inline && !c.shared_checkout && c.archived_at.is_none()
            && c.base_ref.trim().trim_start_matches("origin/") == parent.branch)
        .collect();
    let mut results = Vec::new();
    for mut child in children {
        let path = Path::new(&child.path);
        let idle = ensure_idle(state, &child).is_ok();
        let clean = path.exists() && state.worktrees.is_clean(path).await.unwrap_or(false);
        if !idle || !clean { results.push((child, Restack::Skipped)); continue; }
        let outcome = match state.worktrees.merge(path, target, &format!("Update with the latest {target} after {} was merged", parent.name)).await.map_err(err)? {
            grok_worktree::MergeOutcome::Merged => Restack::Updated,
            // Leave the conflicted merge in place: that is exactly what the child's agent is asked to finish.
            grok_worktree::MergeOutcome::Conflicts { files } => Restack::NeedsHand(files),
        };
        child.base_ref = target.to_string();
        state.persistence.save_workspace(&child).map_err(err)?;
        results.push((child, outcome));
    }
    Ok(results)
}

/// One child's outcome, for the app: which thread, what happened, and the files left for its agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Restacked {
    pub workspace: String,
    pub name: String,
    pub outcome: Restack,
}

/// A thread's work has landed but the thread stays open: bring the threads built on it up to date.
/// Does nothing unless the target really contains the branch.
pub async fn restack(state: &AppState, id: String) -> Result<Vec<Restacked>, String> {
    let w = workspace(state, &id)?;
    let base = merge_target(&w).await?;
    let root = Path::new(&w.project_root);
    let contained = run_git(root, &["merge-base", "--is-ancestor", &format!("refs/heads/{}", w.branch), &format!("refs/heads/{base}")]).await.is_ok();
    if !contained { return Ok(Vec::new()); }
    Ok(restack_children(state, &w, &base).await?.into_iter()
        .map(|(c, outcome)| Restacked { workspace: c.id, name: c.name, outcome })
        .collect())
}

/// The request sent to a thread's own agent so it performs the merge in its chat.
pub async fn merge_request(state: &AppState, id: String) -> Result<String, String> {
    let w = workspace(state, &id)?;
    if w.inline || w.read_only || w.shared_checkout || w.archived_at.is_some() {
        return Err("Only an active thread with its own branch can be merged".into());
    }
    if let Some(parent) = parent_of(state, &w).await? {
        return Err(format!("Merge “{}” first: this thread builds on it. Once that lands, this thread is brought up to date and can merge.", parent.name));
    }
    let base = merge_target(&w).await?;
    if w.branch == base { return Err(format!("This thread already works directly on {base}")); }
    // Where does the target branch live? The project folder, another thread's folder, or nowhere.
    let checkouts = state.worktrees.list(Path::new(&w.project_root)).await.map_err(err)?;
    let holder = checkouts.iter().find(|c| c.branch.as_deref().map(|b| b.trim_start_matches("refs/heads/")) == Some(base.as_str()));
    let landing = match holder {
        Some(c) => {
            let folder = c.path.display().to_string();
            for other in state.persistence.list_workspaces().map_err(err)?.iter().filter(|o| o.id != w.id && o.path == folder) {
                ensure_idle(state, other).map_err(|_| format!("Another thread is working on {base} right now. Let it finish, then merge again."))?;
            }
            format!(
"3. When this branch is healthy, go to the folder at {folder}, which should have {base} checked out, and run `git merge --no-ff {branch}` there.
4. If that folder is not on {base} or has uncommitted edits to tracked files, stop and tell me instead of stashing, resetting or discarding anything. Untracked files there are fine to leave alone.", branch = w.branch)
        }
        None => format!(
"3. {base} is not checked out in any folder, so once step 2 is done this branch already contains it. From this folder run `git fetch . {branch}:{base}` to move {base} forward to this branch.
4. If that command refuses because it is not a fast-forward, stop and tell me instead of forcing it.", branch = w.branch),
    };
    Ok(format!(
"Merge this thread's work into {base}, the branch it started from.

1. Look at what this branch ({branch}) changed and commit any unsaved work here.
2. Merge the latest {base} into this branch. If anything conflicts, resolve it so both this feature and the newer {base} changes keep working, then run the project's checks.
{landing}
5. Do not push and do not delete this branch or folder.

Finish with a short plain-English summary: what went into {base}, any conflicts you resolved and how, and whether the checks passed.",
        branch = w.branch))
}

/// True when the thread's target branch already contains everything on its branch and nothing is left unsaved.
pub async fn is_merged(state: &AppState, id: &str) -> Result<bool, String> {
    let w = workspace(state, id)?;
    let root = Path::new(&w.project_root);
    let base = merge_target(&w).await?;
    if Path::new(&w.path).exists() && !state.worktrees.is_clean(Path::new(&w.path)).await.map_err(err)? { return Ok(false); }
    Ok(run_git(root, &["merge-base", "--is-ancestor", &format!("refs/heads/{}", w.branch), &format!("refs/heads/{base}")]).await.is_ok())
}

/// Save any loose work, archive the thread, and delete its branch only when its target branch already contains it.
pub async fn close_feature(state: &AppState, id: String) -> Result<String, String> {
    let w = workspace(state, &id)?;
    {
        let _gate = state.workspace_gate.lock().await;
        ensure_idle(state, &w)?;
        if w.inline || w.shared_checkout { return Err("This conversation works in the project folder and has no feature branch to close".into()); }
        if w.archived_at.is_none() && Path::new(&w.path).exists() {
            state.worktrees.commit_all(Path::new(&w.path), &format!("Save work before closing {}", w.name)).await.map_err(err)?;
        }
    }
    if w.archived_at.is_none() { archive_workspace(state, id).await?; }
    let root = Path::new(&w.project_root);
    let base = merge_target(&w).await?;
    // Delete only after Git confirms the target contains the branch, so unmerged work is never lost.
    // `-D` because `-d` compares with whatever the project folder has checked out, not with the target.
    let contained = run_git(root, &["merge-base", "--is-ancestor", &format!("refs/heads/{}", w.branch), &format!("refs/heads/{base}")]).await.is_ok();
    // Threads built on this one now build on what it merged into.
    let restacked = if contained { restack_children(state, &w, &base).await.unwrap_or_default() } else { Vec::new() };
    let removed = w.branch != base && contained && run_git(root, &["branch", "-D", &w.branch]).await.is_ok();
    let mut note = if removed { format!("Closed “{}” · its work is in {base}, so the branch was removed", w.name) }
       else { format!("Closed “{}” · branch {} kept because it has work that is not in {base}", w.name, w.branch) };
    let updated = restacked.iter().filter(|(_, r)| *r == Restack::Updated).count();
    let hands: Vec<&str> = restacked.iter().filter_map(|(c, r)| matches!(r, Restack::NeedsHand(_)).then_some(c.name.as_str())).collect();
    if updated > 0 { note.push_str(&format!(" · {updated} thread{} built on it brought up to date", if updated == 1 { "" } else { "s" })); }
    if !hands.is_empty() { note.push_str(&format!(" · needs a hand: {}", hands.join(", "))); }
    Ok(note)
}

/// Tidy a project: close threads and delete branches whose work is already in the default branch.
/// Nothing unmerged, busy or checked out is touched.
pub async fn cleanup_merged(state: &AppState, root: String) -> Result<String, String> {
    let path = Path::new(&root);
    let default = default_branch(path).await?;
    let current = state.worktrees.current_branch(path).await.unwrap_or_default();
    let workspaces: Vec<WorkspaceRecord> = state.persistence.list_workspaces().map_err(err)?.into_iter().filter(|w| w.project_root == root && !w.inline && !w.shared_checkout).collect();
    let branches = run_git(path, &["for-each-ref", "--format=%(refname:short)", "refs/heads/"]).await.map_err(err)?;
    let (mut removed, mut kept) = (0usize, 0usize);
    for branch in branches.lines().map(str::trim).filter(|b| !b.is_empty() && *b != default && *b != current) {
        let workspace = workspaces.iter().find(|w| w.branch == branch);
        let target = default.clone();
        let merged = run_git(path, &["merge-base", "--is-ancestor", &format!("refs/heads/{branch}"), &format!("refs/heads/{target}")]).await.is_ok();
        if !merged { continue; }
        let done = match workspace.filter(|w| w.archived_at.is_none()) {
            // An open thread goes through the same close as the button, which refuses while it is busy.
            Some(w) => close_feature(state, w.id.clone()).await.is_ok_and(|note| note.contains("branch was removed")),
            None => run_git(path, &["branch", "-D", branch]).await.is_ok(),
        };
        if done { removed += 1 } else { kept += 1 }
    }
    Ok(match (removed, kept) {
        (0, 0) => "Nothing to clean up.".into(),
        (r, 0) => format!("Removed {r} finished branch{}.", if r == 1 { "" } else { "es" }),
        (r, k) => format!("Removed {r} finished branch{} · kept {k} still in use.", if r == 1 { "" } else { "es" }),
    })
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
    if w.inline || w.read_only || w.archived_at.is_some() {
        return Err("Create an active thread to make changes".into());
    }
    if w.shared_checkout && matches!(action.as_str(),"archive"|"squash"|"restore"|"revert-file") {return Err("This action is unavailable in a shared checkout".into());}
    let path = Path::new(&w.path);
    if matches!(action.as_str(), "push" | "pr")
        && !state.worktrees.is_clean(path).await.map_err(err)?
    {
        return Err("Save a checkpoint before shipping so all changes are included".into());
    }
    let result = match action.as_str() {
        "commit-selected" => super::git_ui::commit_selected(path,&value).await?,
        "push-main" => {
            let root=Path::new(&w.project_root);
            let base=default_branch(root).await?;
            for other in state.persistence.list_workspaces().map_err(err)?.iter().filter(|other|other.path==w.project_root){ensure_idle(state,other)?;}
            run_git(root,&["push","origin",&format!("refs/heads/{base}:refs/heads/{base}")]).await.map_err(err)?;
            format!("Pushed {base} to origin")
        }
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
            // Bring in the branch this thread started from; prefer its freshly fetched remote copy.
            let target = merge_target(&w).await?;
            let remote = format!("origin/{target}");
            let base = if run_git(path, &["rev-parse", "--verify", &remote]).await.is_ok() { remote } else { target };
            match state
                .worktrees
                .merge(path, &base, &format!("Update from {base}"))
                .await
                .map_err(err)?
            {
                grok_worktree::MergeOutcome::Merged => format!("Updated from {base}"),
                grok_worktree::MergeOutcome::Conflicts { files } => format!(
                    "Resolve these conflicts in this thread: {}",
                    files.join(", ")
                ),
            }
        }
        "push" => {
            run_git(path, &["push", "-u", "origin", &state.worktrees.current_branch(path).await.map_err(err)?])
                .await
                .map_err(err)?;
            "Branch pushed".into()
        }
        "pr" => {
            let payload:serde_json::Value=serde_json::from_str(&value).map_err(err)?;
            let title=payload["title"].as_str().filter(|s|!s.trim().is_empty()).ok_or("Enter a PR title")?;
            let body=payload["body"].as_str().unwrap_or("");
            let target=payload["base"].as_str().ok_or("Choose a target branch")?;
            run_git(Path::new(&w.project_root),&["show-ref","--verify",&format!("refs/heads/{target}")]).await.map_err(err)?;
            let branch=state.worktrees.current_branch(path).await.map_err(err)?;
            if branch==target {return Err("Choose a different target branch for the pull request".into());}
            run_git(path, &["push", "-u", "origin", &state.worktrees.current_branch(path).await.map_err(err)?])
                .await
                .map_err(err)?;
            let out = tokio::process::Command::new("gh")
                .args([
                    "pr",
                    "create",
                    "--head",
                    &branch,
                    "--base",
                    target,
                    "--title",
                    title,
                    "--body",
                    body,
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
            let root = Path::new(&w.project_root);
            for other in state.persistence.list_workspaces().map_err(err)?.iter().filter(|other|other.path==w.project_root) {ensure_idle(state,other)?;}
            if !state.worktrees.is_clean(root).await.map_err(err)?
                || !state.worktrees.is_clean(path).await.map_err(err)?
            {
                return Err(
                    "Both the project checkout and the thread’s working copy must be clean before merging".into(),
                );
            }
            let base = merge_target(&w).await?;
            if state.worktrees.current_branch(root).await.map_err(err)? != base {
                return Err(format!("Check out {base} in the project before merging"));
            }
            let current=state.worktrees.current_branch(path).await.map_err(err)?;
            if current == base {return Err("This thread is already on the target branch".into());}
            match state
                .worktrees
                .merge(root, &current, &format!("Merge {}", w.name))
                .await
                .map_err(err)?
            {
                grok_worktree::MergeOutcome::Merged => format!("Merged into local {base} · Not pushed"),
                grok_worktree::MergeOutcome::Conflicts { .. } => {
                    state.worktrees.merge_abort(root).await;
                    return Err(
                        "Merge conflicts: Update the thread and resolve conflicts there first"
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
            let base = merge_target(&w).await?;
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
        _ => return Err("Unknown thread action".into()),
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
            return Err("Thread is busy".into());
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    async fn merge_is_requested_in_chat_detected_when_done_and_close_removes_merged_branch() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo"); std::fs::create_dir(&root).unwrap();
        for args in [vec!["init", "-b", "main"], vec!["config", "user.name", "Test"], vec!["config", "user.email", "test@example.com"]] { run_git(&root, &args).await.unwrap(); }
        std::fs::write(root.join("app.txt"), "one\n").unwrap();
        run_git(&root, &["add", "-A"]).await.unwrap(); run_git(&root, &["commit", "-m", "initial"]).await.unwrap();
        let home = temp.path().to_path_buf(); let grok = home.join("grok"); let panel = grok.join("panel");
        let state = AppState::initialize_with_paths(grok_config::GrokPaths {
            home_dir: home.clone(), grok_dir: grok.clone(), config_file: panel.join("config.toml"),
            grok_cli_config_file: grok.join("config.toml"), worktrees_dir: home.join("worktrees"),
            memory_dir: panel.join("memory"), sessions_dir: panel.join("sessions"), panel_dir: panel,
            project_config_file: None, project_root: None,
        }).await.unwrap();
        let options = SpawnOptions { model: Some("mock".into()), prompt: Some("Change the app".into()), ..Default::default() };
        let session = super::super::start_session(&state, root.display().to_string(), options).await.unwrap();
        super::super::wait_until_idle(&state, &session.id, std::time::Duration::from_secs(5)).await.unwrap();
        let w = list_workspaces(&state).await.unwrap()[0].clone();
        let copy = Path::new(&w.path);

        let request = merge_request(&state, w.id.clone()).await.unwrap();
        assert!(request.contains(&w.branch) && request.contains(&root.display().to_string()) && request.contains("Do not push"));

        // Unsaved or unmerged work is never reported as merged.
        std::fs::write(copy.join("app.txt"), "feature\n").unwrap();
        assert!(!is_merged(&state, &w.id).await.unwrap());
        run_git(copy, &["commit", "-am", "feature"]).await.unwrap();
        assert!(!is_merged(&state, &w.id).await.unwrap());

        // What the agent does in the chat.
        run_git(&root, &["merge", "--no-ff", &w.branch, "-m", "Merge feature"]).await.unwrap();
        assert!(is_merged(&state, &w.id).await.unwrap());

        let note = close_feature(&state, w.id.clone()).await.unwrap();
        assert!(note.contains("branch was removed"), "{note}");
        assert!(workspace(&state, &w.id).unwrap().archived_at.is_some());
        assert!(!copy.exists());
        assert!(run_git(&root, &["rev-parse", "--verify", &format!("refs/heads/{}", w.branch)]).await.is_err());
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
        super::super::send_prompt(&state, first.id.clone(), "Add dark mode".into(), None, None, None, None, None, None, None, None).await.unwrap();
        assert!(workspace_action(&state, w.id.clone(), "archive".into(), String::new()).await.is_err());
        let overlapping = SpawnOptions { workspace_id: Some(w.id.clone()), ..options.clone() };
        assert!(super::super::start_session(&state, root.display().to_string(), overlapping.clone()).await.is_err());
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                if state.worktrees.is_clean(Path::new(&w.path)).await.unwrap() && !state.workspace_turns.lock().unwrap().contains(&w.path) { break; }
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
        super::super::wait_until_idle(&state, &inline.id, std::time::Duration::from_secs(5)).await.unwrap();
        let direct=super::super::start_session(&state,root.display().to_string(),SpawnOptions{model:Some("mock".into()),isolate_worktree:false,edit_checkout:true,..Default::default()}).await.unwrap();
        super::super::wait_until_idle(&state,&direct.id,std::time::Duration::from_secs(5)).await.unwrap();
        let direct_id=Uuid::parse_str(&direct.id).unwrap();
        let direct_w=state.persistence.workspace_for_session(direct_id).unwrap().unwrap();
        assert!(direct_w.shared_checkout);assert!(!direct_w.inline);assert!(!direct_w.read_only);
        assert!(!state.registry.get_snapshot(direct_id).unwrap().metadata.read_only);
        assert!(state.persistence.workspace_for_session(inline_id).unwrap().unwrap().inline);
        assert!(archive_workspace(&state,direct_w.id.clone()).await.is_err());assert!(root.exists());
        std::fs::write(root.join("local.txt"),"direct checkout").unwrap();
        workspace_action(&state,direct_w.id.clone(),"commit-selected".into(),serde_json::json!({"message":"Local edit","files":["local.txt"]}).to_string()).await.unwrap();
        assert!(state.worktrees.is_clean(&root).await.unwrap());
        // Existing branch is opened without switching or modifying main.
        run_git(&root,&["branch","feature-existing"]).await.unwrap();
        let existing=super::super::start_session(&state,root.display().to_string(),SpawnOptions{model:Some("mock".into()),checkout_branch:Some("feature-existing".into()),..Default::default()}).await.unwrap();
        super::super::wait_until_idle(&state,&existing.id,std::time::Duration::from_secs(5)).await.unwrap();
        let existing_w=state.persistence.workspace_for_session(Uuid::parse_str(&existing.id).unwrap()).unwrap().unwrap();
        assert_eq!(state.worktrees.current_branch(&root).await.unwrap(),"main");
        assert_ne!(existing_w.path,root.display().to_string());
        std::fs::write(Path::new(&existing_w.path).join("feature.txt"),"feature").unwrap();
        workspace_action(&state,existing_w.id.clone(),"commit-selected".into(),serde_json::json!({"message":"Feature","files":["feature.txt"]}).to_string()).await.unwrap();
        let remote=temp.path().join("remote.git");run_git(temp.path(),&["init","--bare",remote.to_str().unwrap()]).await.unwrap();
        run_git(&root,&["remote","add","origin",remote.to_str().unwrap()]).await.unwrap();
        workspace_action(&state,existing_w.id.clone(),"merge".into(),String::new()).await.unwrap();assert!(root.join("feature.txt").exists());
        assert!(run_git(&remote,&["show-ref","--verify","refs/heads/main"]).await.is_err());
        workspace_action(&state,existing_w.id,"push-main".into(),String::new()).await.unwrap();
        assert!(run_git(&remote,&["show-ref","--verify","refs/heads/main"]).await.is_ok());
        super::super::shutdown_all(&state).await.unwrap();
    }

    #[tokio::test]
    async fn cleanup_removes_only_branches_whose_work_is_already_merged() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo"); std::fs::create_dir(&root).unwrap();
        for args in [vec!["init", "-b", "main"], vec!["config", "user.name", "Test"], vec!["config", "user.email", "test@example.com"]] { run_git(&root, &args).await.unwrap(); }
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        run_git(&root, &["add", "-A"]).await.unwrap(); run_git(&root, &["commit", "-m", "initial"]).await.unwrap();
        // merged-a and merged-b are in main; unmerged has its own work; checked-out is where the folder is.
        for b in ["merged-a", "merged-b", "unmerged"] { run_git(&root, &["branch", b]).await.unwrap(); }
        run_git(&root, &["checkout", "-q", "unmerged"]).await.unwrap();
        std::fs::write(root.join("b.txt"), "work\n").unwrap();
        run_git(&root, &["add", "-A"]).await.unwrap(); run_git(&root, &["commit", "-m", "work"]).await.unwrap();
        run_git(&root, &["checkout", "-q", "-b", "checked-out", "main"]).await.unwrap();
        let home = temp.path().to_path_buf(); let grok = home.join("grok"); let panel = grok.join("panel");
        let state = AppState::initialize_with_paths(grok_config::GrokPaths {
            home_dir: home.clone(), grok_dir: grok.clone(), config_file: panel.join("config.toml"),
            grok_cli_config_file: grok.join("config.toml"), worktrees_dir: home.join("worktrees"),
            memory_dir: panel.join("memory"), sessions_dir: panel.join("sessions"), panel_dir: panel,
            project_config_file: None, project_root: None,
        }).await.unwrap();
        assert_eq!(cleanup_merged(&state, root.display().to_string()).await.unwrap(), "Removed 2 finished branches.");
        let left = run_git(&root, &["for-each-ref", "--format=%(refname:short)", "refs/heads/"]).await.unwrap();
        let mut left: Vec<&str> = left.lines().collect(); left.sort();
        assert_eq!(left, ["checked-out", "main", "unmerged"]);
        assert_eq!(cleanup_merged(&state, root.display().to_string()).await.unwrap(), "Nothing to clean up.");
    }

    #[tokio::test]
    async fn merge_targets_the_branch_a_thread_started_from() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo"); std::fs::create_dir(&root).unwrap();
        for args in [vec!["init", "-b", "main"], vec!["config", "user.name", "Test"], vec!["config", "user.email", "test@example.com"], vec!["commit", "--allow-empty", "-m", "initial"], vec!["branch", "develop"], vec!["branch", "thread"]] { run_git(&root, &args).await.unwrap(); }
        let record = |base: &str| WorkspaceRecord {
            id: "w".into(), project_root: root.display().to_string(), name: "Thread".into(), branch: "thread".into(),
            path: root.display().to_string(), base_ref: base.into(), created_at: String::new(), archived_at: None,
            inline: false, shared_checkout: false, read_only: false, threads: vec![],
        };
        assert_eq!(merge_target(&record("develop")).await.unwrap(), "develop");
        // Unusable bases fall back to the default branch rather than failing.
        for base in ["HEAD", "", "deleted-branch", "thread"] {
            assert_eq!(merge_target(&record(base)).await.unwrap(), "main", "{base}");
        }
        // Migrated records store the remote form.
        run_git(&root, &["update-ref", "refs/remotes/origin/develop", "HEAD"]).await.unwrap();
        assert_eq!(merge_target(&record("origin/develop")).await.unwrap(), "develop");
    }

    #[tokio::test]
    async fn merge_request_lands_where_the_target_branch_lives_and_close_respects_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo"); std::fs::create_dir(&root).unwrap();
        for args in [vec!["init", "-b", "main"], vec!["config", "user.name", "Test"], vec!["config", "user.email", "test@example.com"]] { run_git(&root, &args).await.unwrap(); }
        std::fs::write(root.join("app.txt"), "one\n").unwrap();
        run_git(&root, &["add", "-A"]).await.unwrap(); run_git(&root, &["commit", "-m", "initial"]).await.unwrap();
        run_git(&root, &["branch", "develop"]).await.unwrap();
        let home = temp.path().to_path_buf(); let grok = home.join("grok"); let panel = grok.join("panel");
        let state = AppState::initialize_with_paths(grok_config::GrokPaths {
            home_dir: home.clone(), grok_dir: grok.clone(), config_file: panel.join("config.toml"),
            grok_cli_config_file: grok.join("config.toml"), worktrees_dir: home.join("worktrees"),
            memory_dir: panel.join("memory"), sessions_dir: panel.join("sessions"), panel_dir: panel,
            project_config_file: None, project_root: None,
        }).await.unwrap();
        let options = SpawnOptions { model: Some("mock".into()), prompt: Some("Work from develop".into()), base_ref: Some("develop".into()), ..Default::default() };
        let session = super::super::start_session(&state, root.display().to_string(), options).await.unwrap();
        super::super::wait_until_idle(&state, &session.id, std::time::Duration::from_secs(5)).await.unwrap();
        let w = list_workspaces(&state).await.unwrap()[0].clone();
        assert_eq!(w.base_ref, "develop");
        assert_eq!(review_workspace(&state, w.id.clone()).await.unwrap().base, "develop");

        // develop is not checked out anywhere: the agent is told to fast-forward it from the thread's folder.
        let request = merge_request(&state, w.id.clone()).await.unwrap();
        assert!(request.contains("into develop") && request.contains(&format!("git fetch . {}:develop", w.branch)), "{request}");
        assert!(!request.contains("git merge --no-ff"));

        // develop checked out in the project folder: the agent is sent there instead.
        run_git(&root, &["checkout", "develop"]).await.unwrap();
        let request = merge_request(&state, w.id.clone()).await.unwrap();
        assert!(request.contains(&format!("git merge --no-ff {}", w.branch)), "{request}");
        assert!(request.contains(root.canonicalize().unwrap().to_str().unwrap()) || request.contains(root.to_str().unwrap()), "{request}");

        // Merged into main only is not "merged" for a thread that started from develop.
        let copy = Path::new(&w.path);
        std::fs::write(copy.join("app.txt"), "feature\n").unwrap();
        run_git(copy, &["commit", "-am", "feature"]).await.unwrap();
        run_git(&root, &["checkout", "main"]).await.unwrap();
        run_git(&root, &["merge", "--no-ff", &w.branch, "-m", "wrong target"]).await.unwrap();
        assert!(!is_merged(&state, &w.id).await.unwrap());
        run_git(&root, &["checkout", "develop"]).await.unwrap();
        run_git(&root, &["merge", "--no-ff", &w.branch, "-m", "right target"]).await.unwrap();
        assert!(is_merged(&state, &w.id).await.unwrap());

        // Close removes the branch even though the project folder is back on main, because develop contains it.
        run_git(&root, &["checkout", "main"]).await.unwrap();
        run_git(&root, &["reset", "--hard", "HEAD~1"]).await.unwrap();
        let note = close_feature(&state, w.id.clone()).await.unwrap();
        assert!(note.contains("its work is in develop") && note.contains("branch was removed"), "{note}");
    }

    #[tokio::test]
    async fn a_thread_built_on_another_waits_for_it_then_is_brought_up_to_date() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo"); std::fs::create_dir(&root).unwrap();
        for args in [vec!["init", "-b", "main"], vec!["config", "user.name", "Test"], vec!["config", "user.email", "test@example.com"]] { run_git(&root, &args).await.unwrap(); }
        std::fs::write(root.join("app.txt"), "one\n").unwrap();
        run_git(&root, &["add", "-A"]).await.unwrap(); run_git(&root, &["commit", "-m", "initial"]).await.unwrap();
        let home = temp.path().to_path_buf(); let grok = home.join("grok"); let panel = grok.join("panel");
        let state = AppState::initialize_with_paths(grok_config::GrokPaths {
            home_dir: home.clone(), grok_dir: grok.clone(), config_file: panel.join("config.toml"),
            grok_cli_config_file: grok.join("config.toml"), worktrees_dir: home.join("worktrees"),
            memory_dir: panel.join("memory"), sessions_dir: panel.join("sessions"), panel_dir: panel,
            project_config_file: None, project_root: None,
        }).await.unwrap();
        let start = |prompt: &str, base: Option<String>| SpawnOptions { model: Some("mock".into()), prompt: Some(prompt.into()), base_ref: base, ..Default::default() };
        // The backend thread commits some work but is not merged.
        let backend = super::super::start_session(&state, root.display().to_string(), start("Backend API", None)).await.unwrap();
        super::super::wait_until_idle(&state, &backend.id, std::time::Duration::from_secs(5)).await.unwrap();
        let parent = list_workspaces(&state).await.unwrap()[0].clone();
        std::fs::write(Path::new(&parent.path).join("api.txt"), "api\n").unwrap();
        run_git(Path::new(&parent.path), &["add", "-A"]).await.unwrap(); run_git(Path::new(&parent.path), &["commit", "-m", "api"]).await.unwrap();
        // The frontend thread starts from the backend's branch.
        let frontend = super::super::start_session(&state, root.display().to_string(), start("Frontend", Some(parent.branch.clone()))).await.unwrap();
        super::super::wait_until_idle(&state, &frontend.id, std::time::Duration::from_secs(5)).await.unwrap();
        let child = list_workspaces(&state).await.unwrap().into_iter().find(|w| w.name != parent.name).unwrap();
        assert!(Path::new(&child.path).join("api.txt").exists(), "the child sees the parent's unmerged work");
        let links = stack_links(&state, &root.display().to_string()).await.unwrap();
        assert_eq!(links, vec![StackLink { child: child.id.clone(), parent: parent.id.clone(), parent_name: parent.name.clone(), behind_parent: 0 }]);
        // The parent gains a commit: the child is behind it.
        std::fs::write(Path::new(&parent.path).join("api.txt"), "api v2\n").unwrap();
        run_git(Path::new(&parent.path), &["commit", "-am", "api v2"]).await.unwrap();
        assert_eq!(stack_links(&state, &root.display().to_string()).await.unwrap()[0].behind_parent, 1);
        // Bottom-up only: the child cannot merge while the parent is open.
        let refused = merge_request(&state, child.id.clone()).await.unwrap_err();
        assert!(refused.contains("first") && refused.contains(&parent.name), "{refused}");
        // The parent lands in main and is closed: the child is updated and now builds on main.
        run_git(&root, &["merge", "--no-ff", &parent.branch, "-m", "merge backend"]).await.unwrap();
        let note = close_feature(&state, parent.id.clone()).await.unwrap();
        assert!(note.contains("1 thread built on it brought up to date"), "{note}");
        let child = workspace(&state, &child.id).unwrap();
        assert_eq!(child.base_ref, "main");
        assert_eq!(std::fs::read_to_string(Path::new(&child.path).join("api.txt")).unwrap(), "api v2\n");
        assert!(stack_links(&state, &root.display().to_string()).await.unwrap().is_empty());
        assert!(merge_request(&state, child.id.clone()).await.unwrap().contains("into main"));
        assert_eq!(merge_target(&child).await.unwrap(), "main");
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
