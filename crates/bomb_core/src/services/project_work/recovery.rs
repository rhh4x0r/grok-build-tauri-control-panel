//! Explicit recovery actions retain valid evidence and never broaden approved scope.
use super::*;
use features::Assignment;

pub fn draft_text(state: &AppState, project: &str, context: &str) -> String {
    state
        .persistence
        .get_kv(&format!("project-draft/v1/{project}/{context}"))
        .ok()
        .flatten()
        .unwrap_or_default()
}
pub fn save_draft_text(
    state: &AppState,
    project: &str,
    context: &str,
    text: &str,
) -> Result<(), String> {
    state
        .persistence
        .set_kv(&format!("project-draft/v1/{project}/{context}"), text)
        .map_err(err)
}

pub fn discard_proposal(state: &AppState, project: &str) -> Result<(), String> {
    change(state, &project_root(project)?, |p| {
        if p.planning {
            return Err("Wait for planning to finish before discarding its proposal.".into());
        }
        p.draft = None;
        p.questions.clear();
        Ok(())
    })
}

pub fn set_routing(state: &AppState, project: &str, enabled: Option<bool>) -> Result<(), String> {
    change(state, &project_root(project)?, |p| {
        p.routing = enabled;
        Ok(())
    })
}

pub async fn start_features(
    state: Arc<AppState>,
    project: String,
    ids: Vec<String>,
) -> Result<(), String> {
    let project = project_root(&project)?;
    change(&state, &project, |p| {
        if !p.enabled || ids.is_empty() {
            return Err("Select saved features to start.".into());
        }
        for id in &ids {
            let f = feature(p, id)?;
            if f.state != FeatureState::Idea {
                return Err("A selected feature is already started. Refresh the board.".into());
            }
            for dep in &f.feature()?.depends_on {
                if feature(p, dep)?.state == FeatureState::Idea && !ids.contains(dep) {
                    return Err(format!(
                        "Start {} first, or include it in this selection.",
                        feature(p, dep)?.title()
                    ));
                }
            }
        }
        for id in &ids {
            feature_mut(p, id)?.state = FeatureState::Queued;
        }
        p.state = RunState::Running;
        p.completed_head = None;
        p.final_blocker = None;
        Ok(())
    })?;
    execution::drive(state, project);
    Ok(())
}

/// Updating an unfinished assignment is an explicit user action. The approved
/// brief stays immutable; the override and actual applied model remain separate.
pub fn replace_assignment(
    state: &AppState,
    project: &str,
    fid: &str,
    task: &str,
    assignment: Assignment,
) -> Result<(), String> {
    features::validate_assignment(&assignment)?;
    change(state, &project_root(project)?, |p| {
        if p.active_jobs
            .iter()
            .any(|k| k == fid || k.starts_with(&format!("{fid}/")))
        {
            return Err("Wait for this feature's active work before changing its model.".into());
        }
        let f = feature_mut(p, fid)?;
        if f.state == FeatureState::Done {
            return Err("This feature is already merged.".into());
        }
        if task != "reviewer" {
            let t = f.tasks.get(task).ok_or("Task not found")?;
            if matches!(t.state, TaskState::Running | TaskState::Checkpointed) {
                return Err("Only unfinished tasks can change model.".into());
            }
        }
        f.assignments.insert(task.into(), assignment);
        Ok(())
    })
}

pub async fn retry_stage(
    state: Arc<AppState>,
    project: String,
    fid: String,
    expected: String,
    evidence: Option<String>,
) -> Result<(), String> {
    let project = project_root(&project)?;
    change(&state, &project, |p| {
        if !p.enabled {
            return Err("Enable this project's workflow to continue.".into());
        }
        if p.active_jobs
            .iter()
            .any(|k| k == &fid || k.starts_with(&format!("{fid}/")))
        {
            return Err(
                "This feature still has active work. Stop it or wait for it to finish.".into(),
            );
        }
        let f = feature_mut(p, &fid)?;
        if f.state != FeatureState::NeedsInput || f.decision_key() != expected {
            return Err(
                "This decision changed. Inspect the current result before retrying.".into(),
            );
        }
        let stage = f
            .blocker
            .as_ref()
            .map(|b| b.stage.clone())
            .unwrap_or_else(|| {
                if f.review.is_some() {
                    WorkStage::Merge
                } else if f.tasks.values().all(|t| t.state == TaskState::Checkpointed) {
                    WorkStage::Review
                } else {
                    WorkStage::Build
                }
            });
        if let Some(note) = evidence {
            if note.trim().is_empty() || note.len() > 32000 {
                return Err("Provide verification evidence in 1–32,000 characters.".into());
            }
            if !matches!(stage, WorkStage::Review | WorkStage::Checks) {
                return Err("Evidence can be attached to verification, not an implementation or merge retry.".into());
            }
            f.verification_notes.push(note);
        }
        match stage {
            WorkStage::Merge if f.review.is_some() => f.state = FeatureState::Review,
            WorkStage::Build => {
                let target = f.blocker.as_ref().and_then(|b| b.task.clone());
                for (id, t) in &mut f.tasks {
                    if target.as_ref().is_none_or(|target| target == id)
                        && matches!(t.state, TaskState::NeedsInput | TaskState::Interrupted)
                    {
                        t.state = TaskState::Queued;
                    }
                }
                f.state = FeatureState::Queued;
            }
            _ => {
                f.state = FeatureState::Checking;
                f.review = None;
                f.approved_head = None;
            }
        }
        f.blocker = None;
        f.note = "Retry queued; completed work is preserved.".into();
        // This explicit action resumes the selected stage, including after restart.
        p.state = RunState::Running;
        p.completed_head = None;
        Ok(())
    })?;
    execution::drive(state, project);
    Ok(())
}

pub async fn retry_final(
    state: Arc<AppState>,
    project: String,
    expected: String,
) -> Result<(), String> {
    let project = project_root(&project)?;
    change(&state, &project, |p| {
        if p.has_activity() || p.final_blocker.as_ref().is_none_or(|b| b.id != expected) {
            return Err("The final-check decision changed. Inspect its current result.".into());
        }
        p.final_blocker = None;
        p.completed_head = None;
        p.state = RunState::Running;
        Ok(())
    })?;
    execution::drive(state, project);
    Ok(())
}
