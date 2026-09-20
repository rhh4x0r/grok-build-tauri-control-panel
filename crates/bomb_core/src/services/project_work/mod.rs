//! Conversational project coordination over the existing ACP/workspace services.
//! Portable briefs own intent; the local store owns execution and exact evidence.
mod execution;
mod integration;
mod planning;
mod recovery;
pub use recovery::*;
mod types;
pub use planning::{accept_plan, accept_selected_plan, describe};
pub use types::*;

use super::{err, features};
use crate::AppState;
use grok_persistence::Persistence;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub struct ProjectWorkService {
    db: Arc<Persistence>,
    projects: Mutex<HashMap<String, ProjectWork>>,
    driving: Mutex<HashSet<String>>,
    checks: Mutex<HashMap<String, Vec<Arc<grok_acp::TerminalRegistry>>>>,
}
impl ProjectWorkService {
    pub fn new(db: Arc<Persistence>) -> Self {
        Self {
            db,
            projects: Mutex::new(HashMap::new()),
            driving: Mutex::new(HashSet::new()),
            checks: Mutex::new(HashMap::new()),
        }
    }
    fn key(project: &str) -> String {
        format!("project-work/v1/{project}")
    }
    pub fn load(&self, project: &str) -> Result<ProjectWork, String> {
        let mut map = self.projects.lock().map_err(err)?;
        if let Some(work) = map.get(project) {
            return Ok(work.clone());
        }
        let mut work: ProjectWork = self
            .db
            .get_kv(&Self::key(project))
            .map_err(err)?
            .map(|s| serde_json::from_str(&s).map_err(err))
            .transpose()?
            .unwrap_or_default();
        if work.planning
            || matches!(work.state, RunState::Running)
            || work.features.iter().any(|f| {
                matches!(
                    f.state,
                    FeatureState::Working
                        | FeatureState::Checking
                        | FeatureState::Reviewing
                        | FeatureState::Landing
                ) || f.tasks.values().any(|t| t.state == TaskState::Running)
            })
        {
            work.planning = false;
            work.state = RunState::Interrupted;
            work.note="Work was interrupted. Open Needs you to continue unfinished stages; Resume only restarts ready work.".into();
            for f in &mut work.features {
                for t in f
                    .tasks
                    .values_mut()
                    .filter(|t| t.state == TaskState::Running)
                {
                    t.state = TaskState::Interrupted;
                    t.note = "Interrupted turn; inspect the thread before retrying.".into();
                }
                if matches!(
                    f.state,
                    FeatureState::Working
                        | FeatureState::Checking
                        | FeatureState::Reviewing
                        | FeatureState::Landing
                ) {
                    let stage = match f.state {
                        _ if f.pending_repair.is_some() => WorkStage::Repair,
                        FeatureState::Landing => WorkStage::Merge,
                        FeatureState::Reviewing => WorkStage::Review,
                        FeatureState::Checking => WorkStage::Checks,
                        _ => WorkStage::Build,
                    };
                    f.blocker = Some(Blocker::new(
                        stage,
                        "Execution was interrupted. Completed checkpoints are saved.",
                        None,
                    ));
                    f.state = FeatureState::NeedsInput;
                    f.note = "Execution was interrupted; inspect and continue this feature.".into();
                }
            }
        }
        map.insert(project.into(), work.clone());
        Ok(work)
    }
    pub fn snapshot(&self, project: &str) -> Option<ProjectWork> {
        self.projects.lock().ok()?.get(project).cloned()
    }
    fn update<T>(
        &self,
        project: &str,
        f: impl FnOnce(&mut ProjectWork) -> Result<T, String>,
    ) -> Result<T, String> {
        self.load(project)?;
        let mut map = self.projects.lock().map_err(err)?;
        let mut work = map.get(project).cloned().ok_or("Project is not loaded")?;
        let value = f(&mut work)?;
        work.revision += 1;
        self.db
            .set_kv(
                &Self::key(project),
                &serde_json::to_string(&work).map_err(err)?,
            )
            .map_err(err)?;
        map.insert(project.into(), work);
        Ok(value)
    }
    /// Project-owned candidates cannot be changed through unrelated thread/Git
    /// actions while their revision is being checked or queued for landing.
    pub fn owns_cwd(&self, cwd: &str) -> bool {
        self.projects.lock().is_ok_and(|map| {
            map.values().any(|p| {
                p.enabled
                    && (p.final_workspace.as_ref().is_some_and(|w| w.path == cwd)
                        || p.features
                            .iter()
                            .filter(|f| f.state != FeatureState::Done)
                            .any(|f| {
                                f.seed.path == cwd
                                    || f.candidate.as_ref().is_some_and(|w| w.path == cwd)
                                    || f.tasks.values().any(|t| {
                                        t.workspace.as_ref().is_some_and(|w| w.path == cwd)
                                    })
                            }))
            })
        })
    }
}

pub fn project_root(project: &str) -> Result<String, String> {
    let path = Path::new(project);
    if !path.is_absolute() || !path.is_dir() {
        return Err("Choose an existing absolute project folder.".into());
    }
    Ok(path
        .canonicalize()
        .map_err(err)?
        .to_string_lossy()
        .into_owned())
}
pub fn load(state: &AppState, project: &str) -> Result<ProjectWork, String> {
    state.project_work.load(&project_root(project)?)
}
fn notify(state: &AppState, project: &str) {
    state.event_bus.emit(grok_events::ControlEvent::Raw {
        session_id: None,
        payload: serde_json::json!({"channel":"project-work","project":project}),
    });
}
fn announce(state: &AppState, project: &str, message: String) {
    announce_feature(state, project, None, message);
}
fn announce_feature(state: &AppState, project: &str, feature: Option<&str>, message: String) {
    state.event_bus.emit(grok_events::ControlEvent::Raw {
        session_id: None,
        payload: serde_json::json!({"channel":"project-work","project":project,"feature":feature,"notice":message}),
    });
}
fn change<T>(
    state: &AppState,
    project: &str,
    f: impl FnOnce(&mut ProjectWork) -> Result<T, String>,
) -> Result<T, String> {
    let out = state.project_work.update(project, f)?;
    notify(state, project);
    Ok(out)
}
pub async fn enable(state: &AppState, project: &str, enabled: bool) -> Result<(), String> {
    let project = project_root(project)?;
    let current = load(state, &project)?;
    if !enabled
        && state
            .project_work
            .driving
            .lock()
            .map_err(err)?
            .contains(&project)
    {
        return Err("Active project work is finishing. Wait for it to settle before turning the workflow off.".into());
    }
    if !enabled
        && (current.planning
            || current.state == RunState::Running
            || current.features.iter().any(|f| {
                matches!(
                    f.state,
                    FeatureState::Working
                        | FeatureState::Checking
                        | FeatureState::Reviewing
                        | FeatureState::Landing
                ) || f.tasks.values().any(|t| t.state == TaskState::Running)
            }))
    {
        return Err("Pause or stop project work before turning it off.".into());
    }
    let mut settings = features::load(state, &project).await?.settings;
    settings.enabled = enabled;
    features::save_settings(state, &project, settings).await?;
    let target = super::workspaces::default_branch(Path::new(&project))
        .await
        .unwrap_or_else(|_| "main".into());
    change(state, &project, |p| {
        p.enabled = enabled;
        if p.features.is_empty() {
            p.policy.target = target;
        }
        Ok(())
    })
}
pub fn set_policy(state: &AppState, project: &str, policy: RunPolicy) -> Result<(), String> {
    let project = project_root(project)?;
    policy.validate()?;
    change(state, &project, |p| {
        if p.state == RunState::Running {
            return Err("Pause the project before changing run settings.".into());
        }
        if p.features.iter().any(|f| f.state != FeatureState::Done)
            && p.policy.target != policy.target
        {
            return Err(
                "Finish the current feature candidates before changing the target branch.".into(),
            );
        }
        p.policy = policy;
        Ok(())
    })
}
pub fn update_draft(state: &AppState, project: &str, draft: Proposal) -> Result<(), String> {
    let project = project_root(project)?;
    change(state, &project, |p| {
        if p.planning {
            return Err("Wait for planning to finish.".into());
        }
        draft.validate(&p.features)?;
        p.routing_suggestions.retain(|key, _| {
            draft
                .features
                .iter()
                .any(|f| f.tasks.iter().any(|t| key == &format!("{}/{}", f.id, t.id)))
        });
        p.draft = Some(draft);
        Ok(())
    })
}

pub async fn command(state: Arc<AppState>, project: String, command: &str) -> Result<(), String> {
    let project = project_root(&project)?;
    change(&state, &project, |p| {
        if !p.enabled {
            return Err("Enable project work first.".into());
        }
        if command == "status" {
            p.messages
                .push(Message::new("assistant", p.activity_label()));
            return Ok(());
        }
        p.state = match command {
            "go" | "resume" => RunState::Running,
            "pause" => RunState::Paused,
            "stop" => RunState::Stopped,
            _ => return Err("Unknown project command.".into()),
        };
        if ["go", "resume"].contains(&command) {
            p.completed_head = None;
            p.final_checks.clear();
        }
        p.note = match command {
            "pause" => "Paused. Active tasks may finish; no new tasks or merges will start.",
            "stop" => "Stopped. Worktrees and history are preserved.",
            _ => "",
        }
        .into();
        Ok(())
    })?;
    if command == "stop" {
        let p = load(&state, &project)?;
        let mut ids = HashSet::new();
        if let Some(id) = p.planner_thread.filter(|_| p.planning) {
            ids.insert(id);
        }
        for f in &p.features {
            for t in f.tasks.values().filter(|t| t.state == TaskState::Running) {
                if let Some(id) = t.workspace.as_ref().and_then(|w| w.session) {
                    ids.insert(id);
                }
            }
            if let Some(id) = f
                .reviewer_thread
                .filter(|_| f.state == FeatureState::Reviewing)
            {
                ids.insert(id);
            }
            if let Some(id) = f.candidate.as_ref().and_then(|w| w.session) {
                ids.insert(id);
            }
        }
        for id in ids {
            let _ = state.registry.cancel_session(id).await;
        }
        let checks = state
            .project_work
            .checks
            .lock()
            .map_err(err)?
            .get(&project)
            .cloned()
            .unwrap_or_default();
        for check in checks {
            check.kill_all().await;
        }
    }
    if ["go", "resume"].contains(&command) {
        execution::drive(state, project);
    }
    Ok(())
}

pub async fn approve(
    state: Arc<AppState>,
    project: String,
    feature: String,
    head: String,
) -> Result<(), String> {
    let project = project_root(&project)?;
    change(&state, &project, |p| {
        let f = p
            .features
            .iter_mut()
            .find(|f| f.id == feature)
            .ok_or("Feature not found")?;
        if f.state != FeatureState::Review
            || f.review
                .as_ref()
                .is_none_or(|r| r.candidate != head || r.document != f.document)
        {
            return Err("The result changed. Inspect the current result before approving.".into());
        }
        f.approved_head = Some(head);
        f.note = "Approved for local integration.".into();
        Ok(())
    })?;
    if load(&state, &project)?.state == RunState::Running {
        execution::drive(state, project);
    }
    Ok(())
}

pub async fn keep_working(
    state: Arc<AppState>,
    project: String,
    feature: String,
    feedback: String,
) -> Result<(), String> {
    let project = project_root(&project)?;
    if feedback.trim().is_empty() {
        return Err("Describe what to change or which failure to retry.".into());
    }
    change(&state, &project, |p| {
        let f = p
            .features
            .iter_mut()
            .find(|f| f.id == feature)
            .ok_or("Feature not found")?;
        if matches!(
            f.state,
            FeatureState::Working
                | FeatureState::Checking
                | FeatureState::Reviewing
                | FeatureState::Landing
        ) || f.tasks.values().any(|t| t.state == TaskState::Running)
        {
            return Err("Wait for this feature's active tasks, or stop the project first.".into());
        }
        if f.state == FeatureState::Done {
            return Err("This feature is already merged. Describe the follow-up in the project conversation to plan a new change.".into());
        }
        f.blocker = None;
        f.checked_head = None;
        f.feedback.push(feedback.clone());
        f.review = None;
        f.approved_head = None;
        f.note = "Feedback received; queued to continue.".into();
        for t in f
            .tasks
            .values_mut()
            .filter(|t| matches!(t.state, TaskState::NeedsInput | TaskState::Interrupted))
        {
            t.state = TaskState::Queued;
        }
        f.state = if f.tasks.values().all(|t| t.state == TaskState::Checkpointed) {
            FeatureState::Checking
        } else {
            FeatureState::Queued
        };
        let mut message = Message::new("user", feedback);
        message.features.push(feature);
        p.messages.push(message);
        Ok(())
    })?;
    if load(&state, &project)?.state == RunState::Running {
        execution::drive(state, project);
    }
    Ok(())
}

fn feature<'a>(p: &'a ProjectWork, id: &str) -> Result<&'a FeatureWork, String> {
    p.features
        .iter()
        .find(|f| f.id == id)
        .ok_or("Feature not found".into())
}
fn feature_mut<'a>(p: &'a mut ProjectWork, id: &str) -> Result<&'a mut FeatureWork, String> {
    p.features
        .iter_mut()
        .find(|f| f.id == id)
        .ok_or("Feature not found".into())
}

pub async fn diff(state: &AppState, project: &str, id: &str) -> Result<String, String> {
    let project = project_root(project)?;
    let p = load(state, &project)?;
    let f = feature(&p, id)?;
    let w = f
        .candidate
        .as_ref()
        .ok_or("The combined candidate is not ready yet.")?;
    let base = f
        .candidate_base
        .as_deref()
        .ok_or("Candidate base is missing")?;
    grok_worktree::run_git(
        Path::new(&w.path),
        &["diff", "--no-ext-diff", "--no-color", base, "HEAD"],
    )
    .await
    .map_err(err)
}

pub fn candidate_path(state: &AppState, project: &str, id: &str) -> Result<PathBuf, String> {
    let p = load(state, project)?;
    let f = feature(&p, id)?;
    f.candidate
        .as_ref()
        .map(|w| PathBuf::from(&w.path))
        .ok_or("The combined candidate is not ready for preview yet.".into())
}

#[cfg(test)]
mod tests;
