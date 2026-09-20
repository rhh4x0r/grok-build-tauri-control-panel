use super::*;
use features::Assignment;
use grok_control_core::SpawnOptions;
use grok_events::{ControlEvent, SessionStatus};
use std::time::Duration;
use uuid::Uuid;

pub(super) async fn spawn(
    state: &Arc<AppState>,
    project: &str,
    w: &Workspace,
    sid: Uuid,
    a: &Assignment,
    read_only: bool,
) -> Result<(), String> {
    features::validate_assignment(a)?;
    if load(state, project)?.state == RunState::Stopped {
        return Err("Project stopped before agent launch.".into());
    }
    let _gate = state.workspace_gate.lock().await;
    // The caller durably reserves this session ID before spawning it. The
    // runner owns the workspace; unrelated sends/Git actions are excluded.
    let opts = SpawnOptions {
        backend: grok_config::Backend::from_key(&a.backend).ok_or("Unknown provider")?,
        model: Some(a.model.clone()),
        effort: Some(a.effort.clone()),
        read_only,
        plan_mode: read_only,
        approval_mode: Some(if read_only {
            grok_control_core::ApprovalMode::Plan
        } else {
            grok_control_core::ApprovalMode::Ask
        }),
        isolate_worktree: false,
        include_auto_mcp: false,
        project_root: Some(project.into()),
        worktree: Some(w.branch.clone()),
        ..Default::default()
    };
    state
        .registry
        .spawn_agent_preallocated(sid, &w.path, opts, grok_acp::ConnectOpts::default())
        .await
        .map_err(err)?;
    super::super::persist_session(state, sid).await;
    state
        .persistence
        .attach_workspace(sid, &w.id)
        .map_err(err)?;
    Ok(())
}

/// Collect one correlated turn. Idle/quiet is not a completion signal.
pub(super) async fn turn(
    state: &Arc<AppState>,
    project: &str,
    sid: Uuid,
    a: &Assignment,
    prompt: &str,
) -> Result<(String, Vec<String>), String> {
    super::super::wait_until_idle(state, &sid.to_string(), Duration::from_secs(90)).await?;
    if load(state, project)?.state == RunState::Stopped {
        return Err("Project stopped before sending the task.".into());
    }
    state
        .registry
        .set_effort(sid, &a.effort)
        .await
        .map_err(err)?;
    let token = id("A");
    let mut events = state.event_bus.subscribe();
    state
        .persistence
        .append_message(sid, "prompt", prompt, chrono::Utc::now())
        .map_err(err)?;
    state
        .registry
        .send_foundry_prompt(sid, prompt, token.clone())
        .await
        .map_err(err)?;
    let mut output = String::new();
    let mut observed = Vec::new();
    let mut policy_block: Option<String> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1800);
    loop {
        if load(state, project)?.state == RunState::Stopped {
            let _ = state.registry.cancel_session(sid).await;
            return Err("Project stopped.".into());
        }
        if tokio::time::Instant::now() > deadline {
            let _ = state.registry.cancel_session(sid).await;
            return Err(
                "This task exceeded 30 minutes. Inspect its thread before continuing.".into(),
            );
        }
        let event = match tokio::time::timeout(Duration::from_millis(250), events.recv()).await {
            Err(_) => continue,
            Ok(Err(_)) => {
                return Err("Lost provider events. Inspect the thread before retrying.".into())
            }
            Ok(Ok(e)) => e,
        };
        let value = serde_json::to_value(&event).map_err(err)?;
        if value["session_id"] != sid.to_string() {
            continue;
        }
        match event {
            ControlEvent::AgentMessage { text, .. } => output.push_str(&text),
            ControlEvent::ToolCall { event, .. } => {
                if matches!(event.status, grok_events::ToolCallStatus::Completed)
                    && observed.len() < 2000
                {
                    observed.push(format!(
                        "{}: {:?} — {}",
                        event.tool,
                        event.status,
                        event.result_summary.unwrap_or_default()
                    ));
                }
            }
            ControlEvent::Raw { payload, .. } if payload["channel"] == "policy_blocked" => {
                policy_block = payload["message"].as_str().map(str::to_owned);
            }
            ControlEvent::Error { message, .. } => return Err(policy_block.unwrap_or(message)),
            ControlEvent::SessionStatusChanged {
                status: SessionStatus::Failed | SessionStatus::Cancelled,
                ..
            } => {
                return Err(policy_block.unwrap_or_else(|| {
                    "The task failed or was cancelled. Completed checkpoints are saved.".into()
                }))
            }
            ControlEvent::Raw { payload, .. } if payload["correlation"] == token => {
                if payload["turn_complete"] == true {
                    super::super::wait_until_idle(state, &sid.to_string(), Duration::from_secs(15))
                        .await?;
                    return Ok((output, observed));
                }
                return Err(policy_block.unwrap_or_else(||"The agent stopped without completing the task. Inspect the saved attempt before continuing.".into()));
            }
            _ => {}
        }
        if output.len() > 2_000_000 {
            let _ = state.registry.cancel_session(sid).await;
            return Err("Task output exceeded 2 MB. Inspect the thread before continuing.".into());
        }
    }
}

pub(super) async fn combine_inputs(
    state: &AppState,
    w: &Workspace,
    inputs: &[String],
    allow_repair: bool,
) -> Result<Option<String>, String> {
    let _gate = state.workspace_gate.lock().await;
    for input in inputs {
        if let Err(error) =
            grok_worktree::run_git(Path::new(&w.path), &["merge", "--no-edit", input]).await
        {
            let error=format!("Dependency combination needs attention: {error}. The workspace and conflict were preserved.");
            return if allow_repair {
                Ok(Some(error))
            } else {
                Err(error)
            };
        }
    }
    Ok(None)
}

async fn task(
    state: Arc<AppState>,
    project: String,
    fid: String,
    tid: String,
) -> Result<(), String> {
    let p = load(&state, &project)?;
    let f = feature(&p, &fid)?;
    let spec = f.feature()?;
    let task = spec
        .tasks
        .iter()
        .find(|t| t.id == tid)
        .ok_or("Task no longer exists")?
        .clone();
    let assignment = f
        .assignments
        .get(&tid)
        .cloned()
        .or(task.assignment.clone())
        .ok_or("Choose a model for this task")?;
    let w = if let Some(w) = f.tasks.get(&tid).and_then(|t| t.workspace.clone()) {
        w
    } else {
        let w = integration::create_workspace(
            &state,
            &project,
            &p.policy.target,
            &format!("task-{}", task.role.to_lowercase()),
        )
        .await?;
        change(&state, &project, |p| {
            feature_mut(p, &fid)?
                .tasks
                .get_mut(&tid)
                .ok_or("Task missing")?
                .workspace = Some(w.clone());
            Ok(())
        })?;
        w
    };
    let mut inputs = vec![f.seed_head.clone()];
    for dep in &task.waits_for {
        inputs.push(
            f.tasks
                .get(dep)
                .and_then(|t| t.head.clone())
                .ok_or("A dependency has no checkpoint")?,
        );
    }
    let conflict = combine_inputs(&state, &w, &inputs, !f.feedback.is_empty()).await?;
    {
        let _gate = state.workspace_gate.lock().await;
        let record = features::documents::safe_path(
            Path::new(&w.path),
            &["plan", "tasks", &format!("{fid}-{tid}.md")],
        )?;
        if !record.exists() {
            features::documents::write_atomic(
                &record,
                &format!(
                    "# {}\n\n{}\n\n## Handoff\nRecord checks, assumptions, and remaining work.\n",
                    task.title, task.brief
                ),
            )?;
        }
    }
    let sid = Uuid::new_v4();
    change(&state, &project, |p| {
        let t = feature_mut(p, &fid)?
            .tasks
            .get_mut(&tid)
            .ok_or("Task missing")?;
        t.workspace.as_mut().ok_or("Workspace missing")?.session = Some(sid);
        t.attempts += 1;
        Ok(())
    })?;
    spawn(&state, &project, &w, sid, &assignment, false).await?;
    super::super::rename_thread(
        &state,
        sid.to_string(),
        format!("{} · {}", spec.title, task.title),
    )
    .await?;
    let feedback = feature(&load(&state, &project)?, &fid)?
        .feedback
        .join("\n\n");
    let prompt=format!("{}\n\nUpdate plan/tasks/{fid}-{tid}.md with your actual checks, assumptions and remaining work. If blocked, explain the specific user decision needed instead of claiming success. Finish with <task-result>{{\"outcome\":\"completed\" or \"blocked\",\"summary\":\"concrete handoff or question\"}}</task-result>. The app checkpoints your result; do not merge, push, or deploy.\n\nUser follow-up:\n{}",features::task_prompt(&spec,&task),feedback);
    let prompt = if let Some(conflict) = conflict {
        format!("{prompt}\n\nExplicit dependency-recovery scope for this retry: resolve the preserved input conflict and stage the resolved files, then combine ONLY the following already approved commits into THIS task branch before implementing. This is the sole exception to the no-merge instruction above; never merge into the project target or push. Required input commits: {}.\nPreparation error: {conflict}",inputs.join(", "))
    } else {
        prompt
    };
    let result = turn(&state, &project, sid, &assignment, &prompt).await;
    let applied = state.registry.get_snapshot(sid).ok().map(|s| Assignment {
        backend: s.metadata.backend.key().into(),
        model: s.metadata.model,
        effort: String::new(),
    });
    let effort = state.registry.current_effort(sid).await.unwrap_or_default();
    super::super::persist_session(&state, sid).await;
    let _ = state.registry.retire_session(sid).await;
    let (output, _) = result?;
    let raw=output.rsplit_once("<task-result>").and_then(|(_,s)|s.split_once("</task-result>")).map(|(s,_)|s).ok_or("The agent did not provide a completion handoff. Inspect its thread and continue with feedback.")?;
    let reported: serde_json::Value = serde_json::from_str(raw).map_err(err)?;
    let summary = reported["summary"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or("Task handoff is empty")?
        .to_string();
    if reported["outcome"] != "completed" {
        change(&state, &project, |p| {
            let mut blocker = Blocker::new(WorkStage::Build, summary.clone(), Some(tid.clone()));
            if reported["outcome"] == "blocked" && blocker.kind == BlockerKind::Unknown {
                blocker.kind = BlockerKind::Input;
            }
            feature_mut(p, &fid)?.blocker = Some(blocker);
            Ok(())
        })?;
        return Err(summary);
    }
    let head = integration::checkpoint(&state, &w, &format!("Complete {}", task.title)).await?;
    for input in &inputs {
        grok_worktree::run_git(Path::new(&w.path),&["merge-base","--is-ancestor",input,&head]).await.map_err(|_|format!("The task did not include required checkpoint {input}. Keep working to combine all approved inputs."))?;
    }
    change(&state, &project, |p| {
        let f = feature_mut(p, &fid)?;
        let t = f.tasks.get_mut(&tid).ok_or("Task missing")?;
        t.head = Some(head);
        t.summary = summary;
        t.note.clear();
        t.state = TaskState::Checkpointed;
        t.applied = applied.map(|mut a| {
            a.effort = effort;
            a
        });
        Ok(())
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Job {
    Task(String, String),
    Candidate(String),
    Land(String),
    Final,
}
impl Job {
    fn key(&self) -> String {
        match self {
            Self::Task(f, t) => format!("{f}/{t}"),
            Self::Candidate(f) | Self::Land(f) => f.clone(),
            Self::Final => "final".into(),
        }
    }
    fn feature(&self) -> Option<&str> {
        match self {
            Self::Task(f, _) | Self::Candidate(f) | Self::Land(f) => Some(f),
            Self::Final => None,
        }
    }
}
pub(super) fn next_job(p: &ProjectWork, active: &HashSet<String>) -> Option<Job> {
    if p.state != RunState::Running {
        return None;
    }
    for f in &p.features {
        if active.contains(&f.id) {
            continue;
        }
        if matches!(f.state, FeatureState::Landing)
            || (f.state == FeatureState::Review
                && (p.policy.auto_merge || f.approved_head.is_some()))
        {
            return Some(Job::Land(f.id.clone()));
        }
        if !matches!(
            f.state,
            FeatureState::Queued | FeatureState::Working | FeatureState::Checking
        ) {
            continue;
        }
        let Ok(spec) = f.feature() else { continue };
        if !spec.depends_on.iter().all(|id| {
            p.features
                .iter()
                .any(|d| &d.id == id && d.state == FeatureState::Done)
        }) {
            continue;
        }
        for task in &spec.tasks {
            if f.tasks
                .get(&task.id)
                .is_some_and(|t| t.state == TaskState::Queued)
                && !active.contains(&format!("{}/{}", f.id, task.id))
                && task.waits_for.iter().all(|dep| {
                    f.tasks
                        .get(dep)
                        .is_some_and(|t| t.state == TaskState::Checkpointed)
                })
            {
                return Some(Job::Task(f.id.clone(), task.id.clone()));
            }
        }
        if !f.tasks.is_empty() && f.tasks.values().all(|t| t.state == TaskState::Checkpointed) {
            return Some(Job::Candidate(f.id.clone()));
        }
    }
    if p.features.iter().any(|f| f.state == FeatureState::Done)
        && p.features
            .iter()
            .all(|f| matches!(f.state, FeatureState::Done | FeatureState::Idea))
        && p.final_blocker.is_none()
        && !active.contains("final")
    {
        return Some(Job::Final);
    }
    None
}

pub(super) fn drive(state: Arc<AppState>, project: String) {
    if !state
        .project_work
        .driving
        .lock()
        .unwrap()
        .insert(project.clone())
    {
        return;
    }
    tokio::spawn(async move {
        let mut jobs = tokio::task::JoinSet::new();
        let mut active = HashSet::new();
        while let Ok(p) = load(&state, &project) {
            while jobs.len() < p.policy.concurrency {
                let current = match load(&state, &project) {
                    Ok(p) => p,
                    Err(_) => break,
                };
                let Some(job) = next_job(&current, &active) else {
                    break;
                };
                active.insert(job.key());
                let reserved = change(&state, &project, |p| {
                    p.active_jobs.insert(job.key());
                    match &job {
                        Job::Task(f, t) => {
                            let f = feature_mut(p, f)?;
                            f.state = FeatureState::Working;
                            f.tasks.get_mut(t).ok_or("Task missing")?.state = TaskState::Running;
                        }
                        Job::Candidate(f) => {
                            feature_mut(p, f)?.state = FeatureState::Checking;
                        }
                        Job::Land(f) => {
                            feature_mut(p, f)?.state = FeatureState::Landing;
                        }
                        Job::Final => {}
                    }
                    Ok(())
                });
                if reserved.is_err() {
                    break;
                }
                jobs.spawn({
                    let state = state.clone();
                    let project = project.clone();
                    async move {
                        let result = match &job {
                            Job::Task(f, t) => {
                                task(state.clone(), project.clone(), f.clone(), t.clone()).await
                            }
                            Job::Candidate(f) => {
                                integration::candidate(state.clone(), project.clone(), f.clone())
                                    .await
                            }
                            Job::Land(f) => integration::land(&state, &project, f).await,
                            Job::Final => final_check(&state, &project).await,
                        };
                        (job, result)
                    }
                });
            }
            if jobs.is_empty() {
                break;
            }
            if let Some(result) = jobs.join_next().await {
                match result {
                    Ok((job, result)) => {
                        active.remove(&job.key());
                        let _ = change(&state, &project, |p| {
                            p.active_jobs.remove(&job.key());
                            Ok(())
                        });
                        if let Err(error) = result {
                            let _ = change(&state, &project, |p| {
                                if let Some(fid) = job.feature() {
                                    let f = feature_mut(p, fid)?;
                                    let stage = match &job {
                                        Job::Task(_, _) => WorkStage::Build,
                                        Job::Land(_) => WorkStage::Merge,
                                        _ if f.pending_repair.is_some() => WorkStage::Repair,
                                        _ if error.contains("combine feature inputs")
                                            || error.contains("conflict resolution") =>
                                        {
                                            WorkStage::Combine
                                        }
                                        _ if f.state == FeatureState::Reviewing => {
                                            WorkStage::Review
                                        }
                                        _ => WorkStage::Checks,
                                    };
                                    if f.blocker.as_ref().is_none_or(|b| b.message != error) {
                                        f.blocker = Some(Blocker::new(
                                            stage,
                                            error.clone(),
                                            match &job {
                                                Job::Task(_, tid) => Some(tid.clone()),
                                                _ => None,
                                            },
                                        ));
                                    }
                                    f.state = FeatureState::NeedsInput;
                                    f.note = error.clone();
                                    if let Job::Task(_, tid) = &job {
                                        let t = f.tasks.get_mut(tid).ok_or("Task missing")?;
                                        t.state = TaskState::NeedsInput;
                                        t.note = error.clone();
                                    }
                                    let mut m = Message::new(
                                        "assistant",
                                        format!("{} needs you: {error}", f.title()),
                                    );
                                    m.features.push(fid.into());
                                    p.messages.push(m);
                                } else {
                                    p.final_blocker =
                                        Some(Blocker::new(WorkStage::Final, error.clone(), None));
                                    p.state = RunState::Paused;
                                    p.note =
                                        format!("Final project checks need attention: {error}");
                                }
                                Ok(())
                            });
                            announce_feature(
                                &state,
                                &project,
                                job.feature(),
                                format!("Project work needs you: {error}"),
                            );
                        }
                    }
                    Err(error) => {
                        let _ = change(&state, &project, |p| {
                            p.active_jobs.clear();
                            p.state = RunState::Interrupted;
                            p.final_blocker = Some(Blocker::new(
                                WorkStage::Final,
                                format!("A worker was interrupted: {error}"),
                                None,
                            ));
                            p.note=format!("A worker was interrupted: {error}. Inspect its thread before resuming.");
                            Ok(())
                        });
                    }
                }
            }
        }
        let _ = change(&state, &project, |p| {
            p.active_jobs.clear();
            Ok(())
        });
        state.project_work.driving.lock().unwrap().remove(&project);
        // A user may enqueue work while the old driver is exiting. Avoid both
        // duplicate drivers and losing that wakeup.
        if load(&state, &project).is_ok_and(|p| next_job(&p, &HashSet::new()).is_some()) {
            drive(state, project);
        }
    });
}

pub(super) async fn final_check(state: &Arc<AppState>, project: &str) -> Result<(), String> {
    let p = load(state, project)?;
    let w = integration::create_workspace(state, project, &p.policy.target, "project-final-check")
        .await?;
    let head = integration::head(&w.path).await?;
    let approved_features = p
        .features
        .iter()
        .filter(|f| f.state != FeatureState::Idea)
        .map(|f| f.id.clone())
        .collect::<Vec<_>>();
    change(state, project, |p| {
        p.final_workspace = Some(w.clone());
        p.note = "Checking the assembled project.".into();
        p.final_checks.clear();
        Ok(())
    })?;
    let mut seen = HashSet::new();
    let mut evidence = Vec::new();
    for f in p.features.iter().filter(|f| f.state != FeatureState::Idea) {
        for command in f
            .feature()?
            .verification
            .ok_or("Missing verification plan")?
            .checks
        {
            if seen.insert(serde_json::to_string(&command).map_err(err)?) {
                let result = integration::check(state, project, &w.path, &command).await?;
                let passed = result.exit_code == Some(0);
                evidence.push(result);
                change(state, project, |p| {
                    p.final_checks = evidence.clone();
                    Ok(())
                })?;
                if !passed {
                    return Err(
                        "An assembled-project check failed. Open the check output before resuming."
                            .into(),
                    );
                }
            }
        }
    }
    if !state
        .worktrees
        .is_clean(Path::new(&w.path))
        .await
        .map_err(err)?
        || integration::head(&w.path).await? != head
    {
        return Err("Final checks changed the verified candidate.".into());
    }
    let actual = grok_worktree::run_git(
        Path::new(project),
        &["rev-parse", &format!("refs/heads/{}", p.policy.target)],
    )
    .await
    .map_err(err)?;
    if actual.trim() != head {
        return Err(
            "The project target changed during final checks. Resume to verify its new revision."
                .into(),
        );
    }
    change(state, project, |p| {
        if p.state == RunState::Running
            && p.features
                .iter()
                .all(|f| matches!(f.state, FeatureState::Done | FeatureState::Idea))
            && p.features
                .iter()
                .filter(|f| f.state != FeatureState::Idea)
                .map(|f| &f.id)
                .collect::<Vec<_>>()
                == approved_features.iter().collect::<Vec<_>>()
        {
            p.final_blocker = None;
            p.state = RunState::Complete;
            p.completed_head = Some(head);
            p.note = "All started features are integrated and final checks passed.".into();
            p.messages.push(Message::new("assistant", p.note.clone()));
            announce(state, project, p.note.clone());
        }
        Ok(())
    })
}
