//! Opt-in project briefs. Markdown owns intent; SQLite owns local thread links.
//! Writes happen only on explicit actions. Merely opening a project is read-only.
use super::{err, model_suggestions::Candidate, workspaces};
use crate::AppState;
use grok_control_core::{ApprovalMode, SpawnOptions};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[path = "feature_documents.rs"]
pub(crate) mod documents;
pub use documents::{decode, encode};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Assignment {
    pub backend: String,
    pub model: String,
    pub effort: String,
}
impl Assignment {
    pub fn from_candidate(c: &Candidate) -> Self {
        Self {
            backend: c.backend.clone(),
            model: c.model.clone(),
            effort: "medium".into(),
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectSettings {
    pub enabled: bool,
    pub guidelines: String,
    pub roles: BTreeMap<String, Assignment>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FeatureTask {
    pub id: String,
    pub title: String,
    pub role: String,
    #[serde(skip)]
    pub brief: String,
    pub assignment: Option<Assignment>,
    #[serde(default)]
    pub waits_for: Vec<String>,
    #[serde(default)]
    pub review_of: Option<String>,
}
impl FeatureTask {
    pub fn new(role: &str) -> Self {
        Self {
            id: format!("T-{}", &Uuid::new_v4().simple().to_string()[..8]),
            title: role.into(),
            role: role.into(),
            brief: String::new(),
            assignment: None,
            waits_for: vec![],
            review_of: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Feature {
    pub id: String,
    pub title: String,
    #[serde(skip)]
    pub brief: String,
    pub tasks: Vec<FeatureTask>,
    /// Version 2 metadata for approved project runs. Dependencies name features;
    /// task-local checkpoint dependencies retain the version 1 semantics.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<Verification>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Verification {
    pub criteria: Vec<String>,
    pub checks: Vec<CheckCommand>,
    pub test_steps: String,
    pub reviewer: Assignment,
}

/// Exact argv approved with the plan; never interpolated into a shell command.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CheckCommand {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
}
impl Feature {
    pub fn new() -> Self {
        Self {
            id: format!("F-{}", &Uuid::new_v4().simple().to_string()[..8]),
            title: String::new(),
            brief: String::new(),
            tasks: vec![],
            depends_on: vec![],
            verification: None,
        }
    }
}
impl Default for Feature {
    fn default() -> Self {
        Self::new()
    }
}
#[derive(Clone, Debug)]
pub struct Record {
    pub feature: Feature,
    pub revision: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskRun {
    pub thread: Option<Uuid>,
    pub assignment: Option<Assignment>,
    pub applied: Option<Assignment>,
    pub ready_head: Option<String>,
    pub ready_revision: Option<String>,
    pub error: Option<String>,
    pub prompt: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct ProjectBoard {
    pub settings: ProjectSettings,
    pub records: Vec<Record>,
    pub runs: BTreeMap<String, TaskRun>,
    pub dispositions: BTreeMap<String, String>,
}
fn key(root: &Path, suffix: &str) -> String {
    format!("features/v1/{}/{suffix}", root.display())
}
fn run_key(feature: &str, task: &str) -> String {
    format!("{feature}/{task}")
}
pub fn task_key(feature: &str, task: &str) -> String {
    run_key(feature, task)
}
fn root(path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path);
    if !p.is_absolute() || !p.is_dir() {
        return Err("Choose an existing project folder.".into());
    }
    p.canonicalize().map_err(err)
}
fn settings(state: &AppState, root: &Path) -> Result<ProjectSettings, String> {
    state
        .persistence
        .get_kv(&key(root, "settings"))
        .map_err(err)?
        .map(|s| serde_json::from_str(&s).map_err(err))
        .transpose()
        .map(Option::unwrap_or_default)
}
fn run(state: &AppState, root: &Path, feature: &str, task: &str) -> Result<TaskRun, String> {
    state
        .persistence
        .get_kv(&key(root, &run_key(feature, task)))
        .map_err(err)?
        .map(|s| serde_json::from_str(&s).map_err(err))
        .transpose()
        .map(Option::unwrap_or_default)
}
fn save_run(
    state: &AppState,
    root: &Path,
    feature: &str,
    task: &str,
    run: &TaskRun,
) -> Result<(), String> {
    state
        .persistence
        .set_kv(
            &key(root, &run_key(feature, task)),
            &serde_json::to_string(run).map_err(err)?,
        )
        .map_err(err)
}
fn enabled(state: &AppState, root: &Path) -> Result<(), String> {
    if settings(state, root)?.enabled {
        Ok(())
    } else {
        Err(
            "Feature tracking is off for this project. Existing threads still work normally."
                .into(),
        )
    }
}
fn ensure_record_writer_idle(state: &AppState, root: &Path) -> Result<(), String> {
    if state.foundry.owns_cwd(&root.to_string_lossy()) {
        return Err(
            "A review loop is using this folder. Wait before saving project records.".into(),
        );
    }
    for w in state.persistence.list_workspaces().map_err(err)? {
        if Path::new(&w.path).canonicalize().ok().as_deref() == Some(root) {
            workspaces::ensure_idle(state, &w)?;
        }
    }
    Ok(())
}
pub async fn load(state: &AppState, project: &str) -> Result<ProjectBoard, String> {
    let root = root(project)?;
    let _gate = state.feature_gate.lock().await;
    let records = documents::load(&root)?;
    let mut runs = BTreeMap::new();
    let mut dispositions = BTreeMap::new();
    for record in &records {
        dispositions.insert(
            record.feature.id.clone(),
            state
                .persistence
                .get_kv(&key(&root, &format!("{}/disposition", record.feature.id)))
                .map_err(err)?
                .unwrap_or_else(|| "Open".into()),
        );
        for task in &record.feature.tasks {
            runs.insert(
                run_key(&record.feature.id, &task.id),
                run(state, &root, &record.feature.id, &task.id)?,
            );
        }
    }
    Ok(ProjectBoard {
        settings: settings(state, &root)?,
        records,
        runs,
        dispositions,
    })
}
pub async fn save_settings(
    state: &AppState,
    project: &str,
    value: ProjectSettings,
) -> Result<(), String> {
    let root = root(project)?;
    let _gate = state.feature_gate.lock().await;
    if value.guidelines.len() > 12000 {
        return Err("Keep project routing guidelines under 12,000 characters.".into());
    }
    // No scaffolding, Git changes, or model calls on enable/disable.
    state
        .persistence
        .set_kv(
            &key(&root, "settings"),
            &serde_json::to_string(&value).map_err(err)?,
        )
        .map_err(err)
}
pub async fn save(
    state: &AppState,
    project: &str,
    feature: Feature,
    expected: Option<String>,
) -> Result<Record, String> {
    let root = root(project)?;
    let _gate = state.feature_gate.lock().await;
    let _workspace = state.workspace_gate.lock().await;
    enabled(state, &root)?;
    ensure_record_writer_idle(state, &root)?;
    validate(&feature)?;
    let path = documents::feature_path(&root, &feature.id)?;
    documents::check_revision(&path, expected.as_deref())?;
    let revision = encode(&feature)?;
    documents::write_atomic(&path, &revision)?;
    Ok(Record { feature, revision })
}
pub fn validate(feature: &Feature) -> Result<(), String> {
    documents::valid_id(&feature.id)?;
    if feature.title.trim().is_empty()
        || feature.title.contains(['\n', '\r'])
        || feature.title.len() > 200
        || feature.brief.trim().is_empty()
    {
        return Err(
            "Give the feature a title (up to 200 characters) and describe the outcome.".into(),
        );
    }
    if feature.tasks.len() > 24 || feature.brief.len() > 64000 {
        return Err("Keep each feature to 24 tasks and a brief under 64 KB.".into());
    }
    let mut ids = HashSet::new();
    for task in &feature.tasks {
        documents::valid_id(&task.id)?;
        if !ids.insert(task.id.as_str()) {
            return Err("Task IDs must be unique.".into());
        }
        if task.title.trim().is_empty()
            || task.title.contains(['\n', '\r'])
            || task.title.len() > 200
            || task.brief.len() > 32000
        {
            return Err("Each task needs a title and a brief under 32 KB.".into());
        }
        if !["Build", "Frontend", "Backend", "Review", "Other"].contains(&task.role.as_str()) {
            return Err("Unknown task role.".into());
        }
        if let Some(a) = &task.assignment {
            validate_assignment(a)?;
        }
    }
    for task in &feature.tasks {
        for dep in task.waits_for.iter().chain(task.review_of.iter()) {
            if dep == &task.id || !ids.contains(dep.as_str()) {
                return Err("Dependencies must refer to a different task in this feature.".into());
            }
        }
        if task.role != "Review" && task.review_of.is_some() {
            return Err("Only a review task can have a review target.".into());
        }
        let mut stack: Vec<&str> = task
            .waits_for
            .iter()
            .chain(task.review_of.iter())
            .map(String::as_str)
            .collect();
        let mut seen = HashSet::new();
        while let Some(id) = stack.pop() {
            if id == task.id {
                return Err("Tasks cannot have circular dependencies.".into());
            }
            if seen.insert(id) {
                if let Some(t) = feature.tasks.iter().find(|t| t.id == id) {
                    stack.extend(
                        t.waits_for
                            .iter()
                            .chain(t.review_of.iter())
                            .map(String::as_str),
                    );
                }
            }
        }
    }
    Ok(())
}
pub(crate) fn validate_assignment(a: &Assignment) -> Result<(), String> {
    let efforts: &[&str] = match a.backend.as_str() {
        "grok" => &["low", "medium", "high"],
        "codex" => &["minimal", "low", "medium", "high"],
        "claude" => &["low", "medium", "high", "max"],
        _ => return Err("Select a connected provider.".into()),
    };
    if a.model.trim().is_empty() || !efforts.contains(&a.effort.as_str()) {
        return Err("Select a model and supported reasoning effort.".into());
    }
    Ok(())
}
fn saved_task(root: &Path, feature: &str, task: &str) -> Result<(Record, FeatureTask), String> {
    let record = documents::read(&documents::feature_path(root, feature)?)?;
    let task = record
        .feature
        .tasks
        .iter()
        .find(|t| t.id == task)
        .cloned()
        .ok_or("Task no longer exists. Reload the feature.")?;
    Ok((record, task))
}
fn task_workspace(
    state: &AppState,
    run: &TaskRun,
) -> Result<grok_persistence::WorkspaceRecord, String> {
    let id = run
        .thread
        .ok_or("Start or attach a thread first.")?
        .to_string();
    state
        .persistence
        .list_workspaces()
        .map_err(err)?
        .into_iter()
        .find(|w| w.threads.contains(&id))
        .ok_or_else(|| "The task's workspace is unavailable. Open its thread to inspect it.".into())
}
async fn ready_workspace(
    state: &AppState,
    run: &TaskRun,
    revision: &str,
) -> Result<grok_persistence::WorkspaceRecord, String> {
    if run.ready_revision.as_deref() != Some(revision) {
        return Err(
            "A dependency needs a fresh Ready for review checkpoint for this brief.".into(),
        );
    }
    let w = task_workspace(state, run)?;
    workspaces::ensure_idle(state, &w)?;
    let head = grok_worktree::run_git(Path::new(&w.path), &["rev-parse", "HEAD"])
        .await
        .map_err(err)?;
    if run.ready_head.as_deref() != Some(head.trim())
        || !state
            .worktrees
            .is_clean(Path::new(&w.path))
            .await
            .map_err(err)?
    {
        return Err("A dependency changed since its Ready for review checkpoint. Check it again before continuing.".into());
    }
    Ok(w)
}
/// Creates a thread and records its link BEFORE the slow handshake/prompt.
/// The UI may start another task immediately after this returns.
pub async fn start_task(
    state: &AppState,
    project: &str,
    feature_id: &str,
    task_id: &str,
) -> Result<Uuid, String> {
    let root = root(project)?;
    let _gate = state.feature_gate.lock().await;
    enabled(state, &root)?;
    let (record, task) = saved_task(&root, feature_id, task_id)?;
    validate(&record.feature)?;
    if run(state, &root, feature_id, task_id)?.thread.is_some() {
        return Err("This task already has a thread. Open it to continue or retry.".into());
    }
    let assignment = task
        .assignment
        .clone()
        .ok_or("Choose a model before starting this task.")?;
    validate_assignment(&assignment)?;
    let mut base_ref = None;
    let mut dependencies = BTreeMap::new();
    {
        let _workspace = state.workspace_gate.lock().await;
        for id in task.waits_for.iter().chain(task.review_of.iter()) {
            let dep = run(state, &root, feature_id, id)?;
            let w = ready_workspace(state, &dep, &record.revision).await?;
            if task.review_of.as_ref() == Some(id) {
                base_ref = Some(w.branch.clone());
            }
            dependencies.insert(
                id.clone(),
                (
                    w.branch,
                    dep.ready_head
                        .clone()
                        .ok_or("Dependency checkpoint is missing")?,
                ),
            );
        }
    }
    if task.role == "Review" && task.review_of.is_none() {
        return Err("Choose the task to review. A review uses that task's checkpoint in a read-only worktree.".into());
    }
    let opts = SpawnOptions {
        backend: grok_config::Backend::from_key(&assignment.backend)
            .ok_or("Unsupported provider")?,
        model: Some(assignment.model.clone()),
        effort: Some(assignment.effort.clone()),
        approval_mode: Some(ApprovalMode::Ask),
        isolate_worktree: true,
        read_only: task.role == "Review",
        base_ref,
        prompt: Some(format!("{} — {}", record.feature.title, task.title)),
        project_root: Some(root.to_string_lossy().into_owned()),
        ..Default::default()
    };
    let started = super::start_session(state, root.to_string_lossy().into_owned(), opts).await?;
    let id = Uuid::parse_str(&started.id).map_err(err)?;
    let mut launch = TaskRun {
        thread: Some(id),
        assignment: Some(assignment),
        prompt: Some(task_prompt(&record.feature, &task)),
        ..Default::default()
    };
    // Persist before preparation: conflicts/IO failures retain a recoverable thread.
    save_run(state, &root, feature_id, task_id, &launch)?;
    let prepared: Result<(), String> = async {
        let _workspace = state.workspace_gate.lock().await;
        let w = task_workspace(state, &launch)?;
        let cwd = Path::new(&w.path);
        if let Some(target) = &task.review_of {
            let expected = &dependencies.get(target).ok_or("Review target missing")?.1;
            let actual = grok_worktree::run_git(cwd, &["rev-parse", "HEAD"]).await.map_err(err)?;
            if actual.trim() != expected { return Err("The review target moved during launch. Open the new review thread to inspect the snapshot before continuing.".into()); }
            launch.prompt = Some(format!("{}\n\nReview target checkpoint: {}.\n", launch.prompt.as_deref().unwrap_or_default(), expected));
        } else {
            // Dependencies are real inputs, not merely green boxes. Merge their
            // pinned checkpoints only into this newly created isolated branch.
            for (dep, (branch, head)) in &dependencies {
                grok_worktree::run_git(cwd, &["merge", "--no-edit", head]).await.map_err(|e|format!("Could not combine dependency {dep} ({branch}): {e}. Open the task thread to resolve it; no prompt was sent."))?;
            }
            let path = documents::safe_path(cwd, &["plan", "tasks", &format!("{feature_id}-{task_id}.md")])?;
            if path.exists() { return Err("The new task document already exists. It was preserved; open the task thread to inspect it.".into()); }
            let doc = format!("# {} — {}\n\n{}\n\n## Handoff\n\nRecord the tested checkpoint, checks, assumptions and remaining integration work here.\n", feature_id, task_id, launch.prompt.as_deref().unwrap_or_default());
            documents::write_atomic(&path, &doc)?;
            launch.prompt = Some(format!("{}\n\nUpdate your task record at plan/tasks/{feature_id}-{task_id}.md with the handoff. Dependency checkpoints already combined into this worktree: {:?}.\n",launch.prompt.as_deref().unwrap_or_default(),dependencies));
        }
        Ok(())
    }.await;
    launch.error = prepared.err();
    save_run(state, &root, feature_id, task_id, &launch)?;
    Ok(id)
}
/// Uses the accepted launch assignment, even if the draft/role defaults change.
pub async fn send_task(
    state: &AppState,
    project: &str,
    feature_id: &str,
    task_id: &str,
    id: Uuid,
) -> Result<(), String> {
    let root = root(project)?;
    let r = run(state, &root, feature_id, task_id)?;
    if r.thread != Some(id) {
        return Err("Task thread changed. Open the current thread.".into());
    }
    let a = r
        .assignment
        .clone()
        .ok_or("Task has no accepted assignment")?;
    let prompt = r
        .prompt
        .clone()
        .ok_or("The launch brief is unavailable. Open the thread to continue.")?;
    let result = async {
        if let Some(error) = &r.error {
            return Err(error.clone());
        }
        super::wait_until_idle(state, &id.to_string(), std::time::Duration::from_secs(90)).await?;
        super::send_prompt(
            state,
            id.to_string(),
            prompt.clone(),
            Some(a.backend),
            Some(a.model),
            Some("ask".into()),
            None,
            None,
            None,
            Some(false),
            Some(a.effort),
        )
        .await
    }
    .await;
    if let Err(error) = &result {
        // A failed handshake/preparation must still leave a retryable draft.
        state
            .persistence
            .append_message(id, "prompt", &prompt, chrono::Utc::now())
            .map_err(err)?;
        state
            .event_bus
            .emit_error(Some(id), format!("Feature task could not start: {error}"));
    }
    let _gate = state.feature_gate.lock().await;
    let mut latest = run(state, &root, feature_id, task_id)?;
    if latest.thread == Some(id) {
        latest.error = result.as_ref().err().cloned();
        if result.is_ok() {
            if let Ok(snapshot) = state.registry.get_snapshot(id) {
                latest.applied = Some(Assignment {
                    backend: snapshot.metadata.backend.key().into(),
                    model: snapshot.metadata.model,
                    effort: state.registry.current_effort(id).await.unwrap_or_default(),
                });
            }
        }
        save_run(state, &root, feature_id, task_id, &latest)?;
    }
    result
}
pub fn task_prompt(feature: &Feature, task: &FeatureTask) -> String {
    let review = if task.role == "Review" {
        "Review the checkpoint in this read-only worktree. Identify concrete findings against the feature criteria; do not edit files. This is a review of ONE task, not proof of integrated feature behavior."
    } else {
        "Work only on this task in its isolated worktree. Use provisional fixtures if the task allows it. Do not merge, push, or deploy. Read existing repository instructions. Record focused checks, assumptions, and remaining integration work in your final handoff. Do not rewrite shared project/feature records."
    };
    format!("Feature: {} ({})\n\n{}\n\nTask: {} ({}, {})\n\n{}\n\n{}\n\nOther work (coordinate interfaces; do not implement these tasks):\n{}",
        feature.title, feature.id, feature.brief, task.title, task.id, task.role, task.brief, review,
        feature.tasks.iter().filter(|t| t.id != task.id).map(|t| format!("- {}: {} — {}", t.id, t.title, t.brief)).collect::<Vec<_>>().join("\n"))
}
pub async fn mark_ready(
    state: &AppState,
    project: &str,
    feature: &str,
    task: &str,
) -> Result<(), String> {
    let root = root(project)?;
    let _gate = state.feature_gate.lock().await;
    let _workspace = state.workspace_gate.lock().await;
    enabled(state, &root)?;
    let (record, _) = saved_task(&root, feature, task)?;
    let mut r = run(state, &root, feature, task)?;
    let w = task_workspace(state, &r)?;
    workspaces::ensure_idle(state, &w)?;
    if !state
        .worktrees
        .is_clean(Path::new(&w.path))
        .await
        .map_err(err)?
    {
        return Err("Save a checkpoint in the thread before marking it Ready for review.".into());
    }
    r.ready_head = Some(
        grok_worktree::run_git(Path::new(&w.path), &["rev-parse", "HEAD"])
            .await
            .map_err(err)?
            .trim()
            .into(),
    );
    r.ready_revision = Some(record.revision);
    r.error = None;
    save_run(state, &root, feature, task, &r)
}
pub async fn attach(
    state: &AppState,
    project: &str,
    feature: &str,
    task: &str,
    thread: Uuid,
) -> Result<(), String> {
    let root = root(project)?;
    let _gate = state.feature_gate.lock().await;
    enabled(state, &root)?;
    saved_task(&root, feature, task)?;
    if run(state, &root, feature, task)?.thread.is_some() {
        return Err("This task already has a thread.".into());
    }
    let s = state.persistence.get_session(thread).map_err(err)?;
    let w = state
        .persistence
        .list_workspaces()
        .map_err(err)?
        .into_iter()
        .find(|w| w.threads.contains(&thread.to_string()))
        .ok_or("Thread workspace not found")?;
    if Path::new(&w.project_root).canonicalize().map_err(err)? != root {
        return Err("Choose a thread from this project.".into());
    }
    save_run(
        state,
        &root,
        feature,
        task,
        &TaskRun {
            thread: Some(thread),
            assignment: Some(Assignment {
                backend: super::extract_backend_from_meta(&s.metadata_json)
                    .key()
                    .into(),
                model: s.model,
                effort: String::new(),
            }),
            ..Default::default()
        },
    )
}
/// Explicit, portable snapshot. Does not mark any code as merged or approved.
pub async fn snapshot(state: &AppState, project: &str) -> Result<(), String> {
    let root = root(project)?;
    let _gate = state.feature_gate.lock().await;
    let _workspace = state.workspace_gate.lock().await;
    enabled(state, &root)?;
    ensure_record_writer_idle(state, &root)?;
    let mut body = format!("<!-- bomb-status/1 -->\n# Project feature checkpoint\n\nCaptured {}. Local task checkpoints only; not integration or deployment status.\n\n", chrono::Utc::now().to_rfc3339());
    for record in documents::load(&root)? {
        let disposition = state
            .persistence
            .get_kv(&key(&root, &format!("{}/disposition", record.feature.id)))
            .map_err(err)?
            .unwrap_or_else(|| "Open".into());
        body.push_str(&format!(
            "## {} — {}\n\nUser tracking status: {} (not a merge or deployment approval).\n\n",
            record.feature.id, record.feature.title, disposition
        ));
        for task in &record.feature.tasks {
            let r = run(state, &root, &record.feature.id, &task.id)?;
            let status = if r.ready_head.is_some()
                && ready_workspace(state, &r, &record.revision).await.is_ok()
            {
                format!(
                    "Ready for review at `{}` (local branch; not merged)",
                    r.ready_head.as_deref().unwrap_or_default()
                )
            } else if r.thread.is_some() {
                "Thread linked; progress must be checked in that workspace".into()
            } else {
                "Planned".into()
            };
            body.push_str(&format!(
                "- {}: {} — {}\n",
                task.id,
                task.title.replace('\n', " "),
                status
            ));
        }
        body.push('\n');
    }
    let path = documents::safe_path(&root, &["plan", "STATUS.md"])?;
    if path.exists()
        && !std::fs::read_to_string(&path)
            .map_err(err)?
            .starts_with("<!-- bomb-status/1 -->")
    {
        return Err(
            "plan/STATUS.md already exists and is not a Bomb Code snapshot. It was left unchanged."
                .into(),
        );
    }
    documents::write_atomic(&path, &body)
}

/// A user's tracking label never changes code, permissions or merge approval.
pub async fn set_disposition(
    state: &AppState,
    project: &str,
    feature: &str,
    disposition: &str,
) -> Result<(), String> {
    if !["Open", "Done", "Archived"].contains(&disposition) {
        return Err("Unknown feature status".into());
    }
    let root = root(project)?;
    let _gate = state.feature_gate.lock().await;
    enabled(state, &root)?;
    documents::read(&documents::feature_path(&root, feature)?)?;
    state
        .persistence
        .set_kv(&key(&root, &format!("{feature}/disposition")), disposition)
        .map_err(err)
}

/// Optional starter guide; never overwrites an existing PROJECT.md or AGENTS.md.
pub async fn create_guide(state: &AppState, project: &str) -> Result<(), String> {
    let root = root(project)?;
    let _gate = state.feature_gate.lock().await;
    let _workspace = state.workspace_gate.lock().await;
    enabled(state, &root)?;
    ensure_record_writer_idle(state, &root)?;
    let path = documents::safe_path(&root, &["plan", "PROJECT.md"])?;
    documents::check_revision(&path, None)?;
    documents::write_atomic(&path, "# Project guide\n\n## Purpose\nDescribe the product and who it is for.\n\n## Source map & existing instructions\nLink the existing README, AGENTS.md and architecture documentation here.\n\n## Run & check commands\nRecord the development command, focused checks and release checks.\n\n## Conventions & boundaries\nRecord the target branch, shared interfaces and decisions that apply across features.\n\n## Feature workflow\nFeature briefs live in plan/features/. Task handoffs live in plan/tasks/ on their task branches. STATUS.md is an explicit checkpoint snapshot, not live execution state or proof of integration.\n\nIndependent tasks use separate worktrees. Provisional UI fixtures can be built before an API is ready. Dependencies are explicit. Review combined behavior before landing code. Ordinary threads remain available; this workflow is optional.\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use grok_worktree::run_git;

    async fn fixture() -> (tempfile::TempDir, AppState, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.com"],
            vec!["commit", "--allow-empty", "-m", "initial"],
        ] {
            run_git(&repo, &args).await.unwrap();
        }
        let home = temp.path().to_path_buf();
        let grok = home.join("grok");
        let panel = grok.join("panel");
        let state = AppState::initialize_with_paths(grok_config::GrokPaths {
            home_dir: home.clone(),
            grok_dir: grok.clone(),
            config_file: panel.join("config.toml"),
            grok_cli_config_file: grok.join("config.toml"),
            worktrees_dir: home.join("worktrees"),
            memory_dir: panel.join("memory"),
            sessions_dir: panel.join("sessions"),
            panel_dir: panel,
            project_config_file: None,
            project_root: None,
        })
        .await
        .unwrap();
        (temp, state, repo.canonicalize().unwrap())
    }
    fn feature() -> Feature {
        let mut f = Feature::new();
        f.title = "Scores".into();
        f.brief="Build a leaderboard. UI can use provisional rows; real wiring needs both parts. Check the combined behavior before shipping.".into();
        for role in ["Frontend", "Backend", "Build", "Review"] {
            let mut t = FeatureTask::new(role);
            t.brief = format!("Implement the {role} part.");
            t.assignment = Some(Assignment {
                backend: "grok".into(),
                model: "mock".into(),
                effort: "medium".into(),
            });
            f.tasks.push(t);
        }
        f.tasks[2].waits_for = vec![f.tasks[0].id.clone(), f.tasks[1].id.clone()];
        f.tasks[3].review_of = Some(f.tasks[1].id.clone());
        f
    }
    async fn idle(state: &AppState, id: Uuid) {
        super::super::wait_until_idle(state, &id.to_string(), std::time::Duration::from_secs(5))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn optional_records_roundtrip_preserves_existing_project_and_runtime() {
        let (_tmp, state, root) = fixture().await;
        let project = root.to_str().unwrap();
        let f = feature();
        assert!(!load(&state, project).await.unwrap().settings.enabled);
        assert!(!root.join("plan").exists());
        assert!(save(&state, project, f.clone(), None).await.is_err());
        save_settings(
            &state,
            project,
            ProjectSettings {
                enabled: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(!root.join("plan").exists());
        std::fs::write(root.join("AGENTS.md"), "Keep these project instructions.").unwrap();
        create_guide(&state, project).await.unwrap();
        assert!(create_guide(&state, project).await.is_err());
        let record = save(&state, project, f.clone(), None).await.unwrap();
        assert_eq!(load(&state, project).await.unwrap().records[0].feature, f);
        let path = documents::feature_path(&root, &f.id).unwrap();
        let external = record
            .revision
            .replace("Build a leaderboard.", "Build a team leaderboard.");
        std::fs::write(&path, &external).unwrap();
        assert!(save(&state, project, f.clone(), Some(record.revision))
            .await
            .is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), external);
        save_settings(&state, project, ProjectSettings::default())
            .await
            .unwrap();
        assert!(start_task(&state, project, &f.id, &f.tasks[0].id)
            .await
            .is_err());
        let board = load(&state, project).await.unwrap();
        assert_eq!(board.records.len(), 1);
        assert!(!board.settings.enabled);
        assert_eq!(
            std::fs::read_to_string(root.join("AGENTS.md")).unwrap(),
            "Keep these project instructions."
        );
    }

    #[tokio::test]
    async fn task_launches_isolate_writers_and_consume_pinned_dependency_code() {
        let (_tmp, state, root) = fixture().await;
        let project = root.to_str().unwrap();
        let f = feature();
        save_settings(
            &state,
            project,
            ProjectSettings {
                enabled: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let saved = save(&state, project, f.clone(), None).await.unwrap();
        assert!(start_task(&state, project, &f.id, &f.tasks[2].id)
            .await
            .is_err());
        let (a, b) = tokio::join!(
            start_task(&state, project, &f.id, &f.tasks[0].id),
            start_task(&state, project, &f.id, &f.tasks[1].id)
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        idle(&state, a).await;
        idle(&state, b).await;
        assert!(start_task(&state, project, &f.id, &f.tasks[0].id)
            .await
            .is_err());
        let ar = run(&state, &root, &f.id, &f.tasks[0].id).unwrap();
        let br = run(&state, &root, &f.id, &f.tasks[1].id).unwrap();
        let aw = task_workspace(&state, &ar).unwrap();
        let bw = task_workspace(&state, &br).unwrap();
        assert_ne!(aw.path, bw.path);
        assert_ne!(aw.path, project);
        assert_ne!(bw.path, project);
        assert_eq!(
            state
                .registry
                .get_snapshot(a)
                .unwrap()
                .metadata
                .approval_mode,
            ApprovalMode::Ask
        );
        assert!(mark_ready(&state, project, &f.id, &f.tasks[0].id)
            .await
            .is_err()); // task document is still uncheckpointed
        std::fs::write(Path::new(&aw.path).join("ui.txt"), "mock UI").unwrap();
        std::fs::write(Path::new(&bw.path).join("api.txt"), "real API").unwrap();
        for (task, w) in [(&f.tasks[0], &aw), (&f.tasks[1], &bw)] {
            state
                .worktrees
                .commit_all(Path::new(&w.path), "task checkpoint")
                .await
                .unwrap();
            mark_ready(&state, project, &f.id, &task.id).await.unwrap();
        }
        // A later code change invalidates readiness, even if the app still has a prior checkpoint.
        std::fs::write(Path::new(&aw.path).join("ui.txt"), "changed UI").unwrap();
        assert!(start_task(&state, project, &f.id, &f.tasks[2].id)
            .await
            .is_err());
        state
            .worktrees
            .commit_all(Path::new(&aw.path), "new UI checkpoint")
            .await
            .unwrap();
        assert!(start_task(&state, project, &f.id, &f.tasks[2].id)
            .await
            .is_err());
        mark_ready(&state, project, &f.id, &f.tasks[0].id)
            .await
            .unwrap();
        let wiring = start_task(&state, project, &f.id, &f.tasks[2].id)
            .await
            .unwrap();
        idle(&state, wiring).await;
        let wr = run(&state, &root, &f.id, &f.tasks[2].id).unwrap();
        assert!(wr.error.is_none(), "{:?}", wr.error);
        let ww = task_workspace(&state, &wr).unwrap();
        assert_eq!(
            std::fs::read_to_string(Path::new(&ww.path).join("ui.txt")).unwrap(),
            "changed UI"
        );
        assert_eq!(
            std::fs::read_to_string(Path::new(&ww.path).join("api.txt")).unwrap(),
            "real API"
        );
        let review = start_task(&state, project, &f.id, &f.tasks[3].id)
            .await
            .unwrap();
        idle(&state, review).await;
        let rr = run(&state, &root, &f.id, &f.tasks[3].id).unwrap();
        let rw = task_workspace(&state, &rr).unwrap();
        assert!(rw.read_only);
        assert!(rr.error.is_none());
        assert_eq!(
            run_git(Path::new(&rw.path), &["rev-parse", "HEAD"])
                .await
                .unwrap(),
            run_git(Path::new(&bw.path), &["rev-parse", "HEAD"])
                .await
                .unwrap()
        );
        assert!(!root.join("ui.txt").exists());
        assert!(!root.join("api.txt").exists());
        // Changing the brief invalidates old readiness rather than silently certifying new scope.
        let mut changed = f.clone();
        changed.brief.push_str(" Also support teams.");
        save(&state, project, changed, Some(saved.revision))
            .await
            .unwrap();
        let old = run(&state, &root, &f.id, &f.tasks[0].id).unwrap();
        let updated = load(&state, project).await.unwrap();
        assert!(ready_workspace(&state, &old, &updated.records[0].revision)
            .await
            .is_err());
        snapshot(&state, project).await.unwrap();
        assert!(!std::fs::read_to_string(root.join("plan/STATUS.md"))
            .unwrap()
            .contains("Ready for review at"));
        state.registry.shutdown_all().await;
    }
    #[tokio::test]
    async fn conflicting_dependencies_keep_a_recoverable_thread_without_sending() {
        let (_tmp, state, root) = fixture().await;
        let project = root.to_str().unwrap();
        let f = feature();
        save_settings(
            &state,
            project,
            ProjectSettings {
                enabled: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        save(&state, project, f.clone(), None).await.unwrap();
        for (index, text) in [(0, "frontend version"), (1, "backend version")] {
            let t = &f.tasks[index];
            let id = start_task(&state, project, &f.id, &t.id).await.unwrap();
            idle(&state, id).await;
            let r = run(&state, &root, &f.id, &t.id).unwrap();
            let w = task_workspace(&state, &r).unwrap();
            std::fs::write(Path::new(&w.path).join("shared.txt"), text).unwrap();
            state
                .worktrees
                .commit_all(Path::new(&w.path), "overlap checkpoint")
                .await
                .unwrap();
            mark_ready(&state, project, &f.id, &t.id).await.unwrap();
        }
        let id = start_task(&state, project, &f.id, &f.tasks[2].id)
            .await
            .unwrap();
        idle(&state, id).await;
        let r = run(&state, &root, &f.id, &f.tasks[2].id).unwrap();
        assert!(r
            .error
            .as_ref()
            .is_some_and(|e| e.contains("no prompt was sent")));
        assert!(send_task(&state, project, &f.id, &f.tasks[2].id, id)
            .await
            .is_err());
        assert_eq!(
            state.registry.get_snapshot(id).unwrap().metadata.status,
            grok_events::SessionStatus::Idle
        );
        assert!(state
            .persistence
            .transcript_entries(id)
            .unwrap()
            .iter()
            .any(|entry| entry.role == "user" && entry.body.contains("Scores")));
        let w = task_workspace(&state, &r).unwrap();
        assert!(!run_git(
            Path::new(&w.path),
            &["diff", "--name-only", "--diff-filter=U"]
        )
        .await
        .unwrap()
        .is_empty());
        assert!(!root.join("shared.txt").exists());
        set_disposition(&state, project, &f.id, "Done")
            .await
            .unwrap();
        assert_eq!(
            load(&state, project).await.unwrap().dispositions[&f.id],
            "Done"
        );
        assert!(set_disposition(&state, project, &f.id, "deployed")
            .await
            .is_err());
        state.registry.shutdown_all().await;
    }
}
