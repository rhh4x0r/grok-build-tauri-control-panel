//! Shipping a thread: one clear status, and the steps that move it along. Open a pull request
//! (regular, or a draft), push new work to it, merge it on GitHub (squashed), and bring the local
//! target branch up to date. Merging into the local branch stays the agent's job (`merge_request`).
//!
//! The PR and the merge are remembered on the workspace: a squash leaves no trace Git can follow
//! back to the branch, so "merged" can't be worked out from history later.

use super::workspaces::{ensure_idle, merge_target, parent_of, workspace};
use super::*;
use grok_persistence::{MergedRecord, PrRecord, WorkspaceRecord};
use grok_worktree::run_git;
use std::path::Path;

/// Where a thread stands, from first edit to merged.
#[derive(Debug, Clone, Default, PartialEq, Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Ship {
    /// Nothing changed yet.
    #[default]
    Nothing,
    /// Work to ship: commits on the branch and/or unsaved edits.
    Ready { commits: usize, unsaved: usize },
    /// A pull request is open. `unpushed`/`unsaved` are what it doesn't have yet.
    PrOpen { number: u64, url: String, draft: bool, checks: String, unpushed: usize, unsaved: usize },
    /// The pull request was closed without merging.
    PrClosed { number: u64, url: String },
    /// The work is in `into`, through PR `pr` or a local merge.
    Merged { into: String, pr: Option<u64>, url: Option<String> },
}

/// The thread's status. Refreshes an open PR from GitHub first, and records it if it merged there.
pub async fn status(state: &AppState, w: &mut WorkspaceRecord, commits: usize, unsaved: usize, unpushed: usize) -> Ship {
    if w.pr.as_ref().is_some_and(|pr| pr.state == "OPEN") {
        let _ = refresh_pr(state, w).await;
    }
    if let Some(m) = &w.merged {
        let url = w.pr.as_ref().filter(|pr| Some(pr.number) == m.pr).map(|pr| pr.url.clone());
        return Ship::Merged { into: m.into.clone(), pr: m.pr, url };
    }
    match &w.pr {
        Some(pr) if pr.state == "OPEN" => Ship::PrOpen { number: pr.number, url: pr.url.clone(), draft: pr.draft, checks: pr.checks.clone(), unpushed, unsaved },
        // A closed PR stays the headline until there's new work; then it's ready to ship again.
        Some(pr) if pr.state == "CLOSED" && commits + unsaved == 0 => Ship::PrClosed { number: pr.number, url: pr.url.clone() },
        _ if commits + unsaved == 0 => Ship::Nothing,
        _ => Ship::Ready { commits, unsaved },
    }
}

/// `gh` in `dir`, bounded so a hung network never hangs the app.
async fn gh(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = tokio::time::timeout(std::time::Duration::from_secs(60), tokio::process::Command::new("gh").args(args).current_dir(dir).output())
        .await
        .map_err(|_| "GitHub took too long to answer".to_string())?
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "The GitHub CLI (gh) isn't installed. Install it and run `gh auth login`.".to_string() } else { e.to_string() })?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if msg.is_empty() { "GitHub refused the request".into() } else { msg });
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// passing | failing | pending | none, from `statusCheckRollup`.
pub fn checks_of(rollup: &serde_json::Value) -> &'static str {
    let Some(checks) = rollup.as_array().filter(|c| !c.is_empty()) else { return "none" };
    let failed = |c: &serde_json::Value| matches!(c["conclusion"].as_str().or(c["state"].as_str()), Some("FAILURE" | "TIMED_OUT" | "CANCELLED" | "ERROR" | "ACTION_REQUIRED"));
    let running = |c: &serde_json::Value| c["status"].as_str().is_some_and(|s| s != "COMPLETED") || matches!(c["state"].as_str(), Some("PENDING" | "EXPECTED"));
    if checks.iter().any(failed) { "failing" } else if checks.iter().any(running) { "pending" } else { "passing" }
}

async fn view_pr(dir: &Path, which: &str) -> Result<PrRecord, String> {
    let raw = gh(dir, &["pr", "view", which, "--json", "number,url,isDraft,state,statusCheckRollup"]).await?;
    let v: serde_json::Value = serde_json::from_str(&raw).map_err(err)?;
    Ok(PrRecord {
        number: v["number"].as_u64().ok_or("GitHub didn't say which PR this is")?,
        url: v["url"].as_str().unwrap_or_default().to_string(),
        draft: v["isDraft"].as_bool().unwrap_or(false),
        state: v["state"].as_str().unwrap_or("OPEN").to_string(),
        checks: checks_of(&v["statusCheckRollup"]).to_string(),
    })
}

/// The folder to run `gh` in: the thread's own while it exists, else the project's.
fn gh_dir(w: &WorkspaceRecord) -> &Path {
    if Path::new(&w.path).exists() { Path::new(&w.path) } else { Path::new(&w.project_root) }
}

/// Bring the stored PR up to date with GitHub; a PR merged there marks the thread merged.
pub async fn refresh_pr(state: &AppState, w: &mut WorkspaceRecord) -> Result<(), String> {
    let Some(number) = w.pr.as_ref().map(|pr| pr.number) else { return Ok(()) };
    let fresh = view_pr(gh_dir(w), &number.to_string()).await?;
    let changed = w.pr.as_ref() != Some(&fresh);
    if fresh.state == "MERGED" && w.merged.is_none() {
        w.merged = Some(MergedRecord { at: Utc::now().to_rfc3339(), pr: Some(number), into: merge_target(w).await.unwrap_or_default() });
    }
    w.pr = Some(fresh);
    if changed || w.merged.is_some() {
        state.persistence.save_workspace(w).map_err(err)?;
    }
    Ok(())
}

/// Refresh every open PR in a project (the sidebar's badges); quiet about failures.
pub async fn refresh_project_prs(state: &AppState, root: &str) -> Result<usize, String> {
    let open: Vec<WorkspaceRecord> = state.persistence.list_workspaces().map_err(err)?.into_iter()
        .filter(|w| w.project_root == root && w.pr.as_ref().is_some_and(|pr| pr.state == "OPEN"))
        .collect();
    let mut changed = 0;
    for mut w in open {
        let before = (w.pr.clone(), w.merged.is_some());
        if refresh_pr(state, &mut w).await.is_ok() && (w.pr.clone(), w.merged.is_some()) != before {
            changed += 1;
        }
    }
    Ok(changed)
}

fn shippable(w: &WorkspaceRecord) -> Result<(), String> {
    if w.inline || w.read_only || w.shared_checkout || w.archived_at.is_some() {
        return Err("Only an active thread with its own branch can be shipped".into());
    }
    Ok(())
}

/// Commit loose edits so what ships is everything on screen.
async fn save_loose_work(state: &AppState, w: &WorkspaceRecord, why: &str) -> Result<(), String> {
    let path = Path::new(&w.path);
    if !state.worktrees.is_clean(path).await.map_err(err)? {
        state.worktrees.commit_all(path, &format!("{why}: {}", w.name)).await.map_err(err)?;
    }
    Ok(())
}

/// Push the thread's branch to origin (setting it as the upstream).
async fn push_branch(state: &AppState, w: &WorkspaceRecord) -> Result<String, String> {
    let path = Path::new(&w.path);
    let branch = state.worktrees.current_branch(path).await.map_err(err)?;
    run_git(path, &["push", "-u", "origin", &branch]).await.map_err(err)?;
    Ok(branch)
}

/// Open a pull request for the thread: save loose work, push, and open it against the branch the
/// thread started from, titled with the thread's name. Returns its URL.
pub async fn open_pr(state: &AppState, id: String, draft: bool) -> Result<String, String> {
    let mut w = workspace(state, &id)?;
    shippable(&w)?;
    if let Some(pr) = w.pr.as_ref().filter(|pr| pr.state == "OPEN") {
        return Err(format!("This thread already has PR #{} open", pr.number));
    }
    if let Some(parent) = parent_of(state, &w).await? {
        return Err(format!("Ship “{}” first: this thread builds on it.", parent.name));
    }
    let path = Path::new(&w.path);
    if run_git(path, &["remote", "get-url", "origin"]).await.is_err() {
        return Err("This project isn't on GitHub (no origin remote). Merge to main instead.".into());
    }
    let base = merge_target(&w).await?;
    if run_git(path, &["ls-remote", "--exit-code", "--heads", "origin", &base]).await.is_err() {
        return Err(format!("{base} isn't on GitHub yet. Push {base} first, then open the pull request."));
    }
    {
        let _gate = state.workspace_gate.lock().await;
        ensure_idle(state, &w)?;
        save_loose_work(state, &w, "Save work before opening a pull request").await?;
    }
    let branch = push_branch(state, &w).await?;
    if branch == base {
        return Err(format!("This thread works directly on {base}; there's nothing to open a pull request from"));
    }
    let commits = run_git(path, &["log", "--reverse", "--format=- %s", &format!("origin/{base}..HEAD")]).await.unwrap_or_default();
    let body = format!("{}\n\nOpened from Bomb Code.", commits.trim());
    let mut args = vec!["pr", "create", "--head", branch.as_str(), "--base", base.as_str(), "--title", w.name.as_str(), "--body", body.as_str()];
    if draft {
        args.push("--draft");
    }
    let out = gh(path, &args).await?;
    let url = out.lines().rev().find(|l| l.starts_with("https://")).unwrap_or(out.as_str()).trim().to_string();
    w.pr = Some(view_pr(path, &url).await.unwrap_or(PrRecord { number: url.rsplit('/').next().and_then(|n| n.parse().ok()).unwrap_or(0), url: url.clone(), draft, state: "OPEN".into(), checks: "none".into() }));
    w.merged = None;
    state.persistence.save_workspace(&w).map_err(err)?;
    Ok(url)
}

/// Push the thread's newer work to its open PR.
pub async fn push_pr(state: &AppState, id: String) -> Result<String, String> {
    let mut w = workspace(state, &id)?;
    shippable(&w)?;
    let number = w.pr.as_ref().filter(|pr| pr.state == "OPEN").map(|pr| pr.number).ok_or("This thread has no open pull request")?;
    {
        let _gate = state.workspace_gate.lock().await;
        ensure_idle(state, &w)?;
        save_loose_work(state, &w, "Save work for the pull request").await?;
    }
    push_branch(state, &w).await?;
    let _ = refresh_pr(state, &mut w).await;
    Ok(format!("Pushed the latest work to PR #{number}"))
}

/// Mark a draft PR ready for review.
pub async fn ready_pr(state: &AppState, id: String) -> Result<String, String> {
    let mut w = workspace(state, &id)?;
    let number = w.pr.as_ref().filter(|pr| pr.state == "OPEN").map(|pr| pr.number).ok_or("This thread has no open pull request")?;
    gh(gh_dir(&w), &["pr", "ready", &number.to_string()]).await?;
    let _ = refresh_pr(state, &mut w).await;
    Ok(format!("PR #{number} is ready for review"))
}

/// Merge the thread's PR on GitHub as one squashed commit, then bring the local target branch up
/// to date. A draft is marked ready first: pressing Merge says it is. New local work is pushed first.
pub async fn merge_pr(state: &AppState, id: String) -> Result<String, String> {
    let mut w = workspace(state, &id)?;
    shippable(&w)?;
    refresh_pr(state, &mut w).await?;
    let pr = w.pr.clone().filter(|pr| pr.state == "OPEN").ok_or("This thread has no open pull request")?;
    {
        let _gate = state.workspace_gate.lock().await;
        ensure_idle(state, &w)?;
        save_loose_work(state, &w, "Save work for the pull request").await?;
    }
    push_branch(state, &w).await?;
    let number = pr.number.to_string();
    if pr.draft {
        gh(Path::new(&w.path), &["pr", "ready", &number]).await?;
    }
    gh(Path::new(&w.path), &["pr", "merge", &number, "--squash"]).await?;
    let base = merge_target(&w).await?;
    w.merged = Some(MergedRecord { at: Utc::now().to_rfc3339(), pr: Some(pr.number), into: base.clone() });
    let _ = refresh_pr(state, &mut w).await;
    state.persistence.save_workspace(&w).map_err(err)?;
    let local = update_local_base(state, &w, &base).await;
    Ok(format!("Merged PR #{} into {base} (squashed){local}", pr.number))
}

/// After a merge on GitHub: fetch, and move the local target branch up to it when that's a plain
/// fast-forward. Never touches a folder with edits. Returns a note for the toast.
async fn update_local_base(state: &AppState, w: &WorkspaceRecord, base: &str) -> String {
    let root = Path::new(&w.project_root);
    if run_git(root, &["fetch", "--prune", "origin"]).await.is_err() {
        return format!(" · couldn't fetch, so local {base} wasn't updated");
    }
    let holder = state.worktrees.list(root).await.unwrap_or_default().into_iter()
        .find(|c| c.branch.as_deref().map(|b| b.trim_start_matches("refs/heads/")) == Some(base));
    let updated = match holder {
        Some(c) => state.worktrees.is_clean(&c.path).await.unwrap_or(false)
            && run_git(&c.path, &["merge", "--ff-only", &format!("origin/{base}")]).await.is_ok(),
        None => run_git(root, &["fetch", "origin", &format!("{base}:{base}")]).await.is_ok(),
    };
    if updated { format!(" · local {base} is up to date") } else { format!(" · local {base} has its own changes, so it wasn't moved") }
}

/// Remember that the thread's work landed in `into` without a PR (the local squash merge).
pub fn record_local_merge(state: &AppState, w: &mut WorkspaceRecord, into: &str) -> Result<(), String> {
    if w.merged.is_none() {
        w.merged = Some(MergedRecord { at: Utc::now().to_rfc3339(), pr: None, into: into.to_string() });
        state.persistence.save_workspace(w).map_err(err)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn checks_read_as_one_word() {
        assert_eq!(checks_of(&json!([])), "none");
        assert_eq!(checks_of(&json!(null)), "none");
        assert_eq!(checks_of(&json!([{"status": "COMPLETED", "conclusion": "SUCCESS"}])), "passing");
        assert_eq!(checks_of(&json!([{"status": "COMPLETED", "conclusion": "SUCCESS"}, {"status": "IN_PROGRESS"}])), "pending");
        assert_eq!(checks_of(&json!([{"status": "IN_PROGRESS"}, {"status": "COMPLETED", "conclusion": "FAILURE"}])), "failing");
        // Commit statuses (not check runs) report `state`.
        assert_eq!(checks_of(&json!([{"state": "PENDING"}])), "pending");
    }

    /// Seeds `$DEMO_HOME` with a project whose threads stand in every shipping state, for looking at
    /// the project map: `DEMO_HOME=… cargo test -p bomb_core seed_project_map_demo -- --ignored`,
    /// then run the app with `BOMBCODE_HOME=$DEMO_HOME/home BOMB_OPEN_PROJECT=$DEMO_HOME/map-demo`.
    /// Keep `DEMO_HOME` short (e.g. `/tmp/bmap`): the app's sockets live under it and macOS caps
    /// socket paths near 104 characters.
    #[tokio::test]
    #[ignore]
    async fn seed_project_map_demo() {
        let demo = std::path::PathBuf::from(std::env::var("DEMO_HOME").expect("set DEMO_HOME"));
        let root = demo.join("map-demo");
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| { let root = root.clone(); let args: Vec<String> = args.iter().map(|s| s.to_string()).collect(); async move { run_git(&root, &args.iter().map(String::as_str).collect::<Vec<_>>()).await.unwrap() } };
        for args in [vec!["init", "-b", "main"], vec!["config", "user.name", "Demo"], vec!["config", "user.email", "demo@example.com"]] { git(&args).await; }
        std::fs::write(root.join("README.md"), "# Map demo\n").unwrap();
        git(&["add", "-A"]).await; git(&["commit", "-m", "initial"]).await;
        let mut paths = grok_config::GrokPaths::discover(None).unwrap();
        let home = demo.join("home");
        paths.bomb_dir = home.clone(); paths.panel_dir = home.clone(); paths.config_file = home.join("config.toml");
        paths.worktrees_dir = home.join("worktrees"); paths.memory_dir = home.join("memory"); paths.sessions_dir = home.join("sessions");
        let state = AppState::initialize_with_paths(paths).await.unwrap();
        super::super::add_project(&state, root.display().to_string()).await.unwrap();
        let start = |prompt: &str, base: Option<String>| SpawnOptions { model: Some("mock".into()), prompt: Some(prompt.into()), base_ref: base, isolate_worktree: true, ..Default::default() };
        let mut made = Vec::new();
        for (prompt, files, base) in [
            ("Explore caching", 0, None),
            ("Onboarding copy", 1, None),
            ("Add dark mode", 3, None),
            ("Fix login crash", 2, None),
            ("Payment page", 4, None),
        ] {
            let s = super::super::start_session(&state, root.display().to_string(), start(prompt, base)).await.unwrap();
            super::super::wait_until_idle(&state, &s.id, std::time::Duration::from_secs(5)).await.unwrap();
            let w = state.persistence.workspace_for_session(Uuid::parse_str(&s.id).unwrap()).unwrap().unwrap();
            for i in 0..files {
                std::fs::write(Path::new(&w.path).join(format!("{}-{i}.txt", w.branch.replace('/', "-"))), prompt).unwrap();
                run_git(Path::new(&w.path), &["add", "-A"]).await.unwrap();
                run_git(Path::new(&w.path), &["commit", "-m", &format!("{prompt}: step {}", i + 1)]).await.unwrap();
            }
            made.push(w);
        }
        // A thread that builds on "Payment page".
        let s = super::super::start_session(&state, root.display().to_string(), start("Payment tests", Some(made[4].branch.clone()))).await.unwrap();
        super::super::wait_until_idle(&state, &s.id, std::time::Duration::from_secs(5)).await.unwrap();
        let child = state.persistence.workspace_for_session(Uuid::parse_str(&s.id).unwrap()).unwrap().unwrap();
        std::fs::write(Path::new(&child.path).join("payment-tests.txt"), "tests").unwrap();
        run_git(Path::new(&child.path), &["add", "-A"]).await.unwrap();
        run_git(Path::new(&child.path), &["commit", "-m", "Payment tests"]).await.unwrap();
        // Onboarding landed (squashed); login and payment have PRs open.
        git(&["merge", "--squash", &made[1].branch]).await; git(&["commit", "-m", "Onboarding copy"]).await;
        std::fs::write(root.join("CHANGELOG.md"), "main moved on\n").unwrap();
        git(&["add", "-A"]).await; git(&["commit", "-m", "Changelog"]).await;
        let mut w = made[1].clone(); record_local_merge(&state, &mut w, "main").unwrap();
        let pr = |number: u64, draft: bool, checks: &str| Some(PrRecord { number, url: format!("https://github.com/demo/map-demo/pull/{number}"), draft, state: "OPEN".into(), checks: checks.into() });
        let mut w = made[3].clone(); w.pr = pr(12, false, "passing"); state.persistence.save_workspace(&w).unwrap();
        let mut w = made[4].clone(); w.pr = pr(14, true, "failing"); state.persistence.save_workspace(&w).unwrap();
        // The thread list hides offline "mock" threads; label them as ordinary Claude threads.
        for mut rec in state.persistence.list_sessions().unwrap() {
            rec.model = "opus".into();
            rec.metadata_json = rec.metadata_json.replace(r#""model":"mock""#, r#""model":"opus""#).replace(r#""backend":"grok""#, r#""backend":"claude""#);
            state.persistence.upsert_session(&rec).unwrap();
        }
        println!("seeded {}", root.display());
    }
}
