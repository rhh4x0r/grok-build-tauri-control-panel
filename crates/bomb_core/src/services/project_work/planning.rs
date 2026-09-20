use super::*;
use crate::services::{
    features::Assignment,
    model_suggestions::{self as routing, Candidate},
};
use grok_control_core::SpawnOptions;

pub async fn describe(
    state: Arc<AppState>,
    project: String,
    text: String,
    planner: Assignment,
    candidates: Vec<Candidate>,
) -> Result<(), String> {
    let project = project_root(&project)?;
    if text.trim().is_empty() || text.len() > 32000 {
        return Err("Describe the work in 1–32,000 characters.".into());
    }
    match text.trim().trim_end_matches(['.','!']).to_lowercase().as_str() {
        "pause" | "pause project" | "pause the project" => return command(state,project,"pause").await,
        "stop" | "stop project" | "stop the project" => return command(state,project,"stop").await,
        "resume" | "resume project" | "resume the project" => return command(state,project,"resume").await,
        "status" | "project status" | "what is happening?" => return change(&state,&project,|p|{p.messages.push(Message::new("assistant",p.activity_label()));Ok(())}),
        _ => {}
    }
    features::validate_assignment(&planner)?;
    if !candidates
        .iter()
        .any(|c| c.backend == planner.backend && c.model == planner.model)
    {
        return Err("Choose a connected planning model.".into());
    }
    let before = load(&state, &project)?;
    change(&state, &project, |p| {
        if !p.enabled {
            return Err("Enable project work first.".into());
        }
        if p.planning {
            return Err(
                "The planner is already working. You can continue once its proposal arrives."
                    .into(),
            );
        }
        if p.state == RunState::Stopped {
            p.state = RunState::Paused;
        }
        p.planning = true;
        p.planner = Some(planner.clone());
        p.note.clear();
        p.messages.push(Message::new("user", text.clone()));
        Ok(())
    })?;
    let result=async {
        let settings=features::load(&state,&project).await?.settings;
        let opts=SpawnOptions {
            backend:grok_config::Backend::from_key(&planner.backend).ok_or("Unknown planner provider")?,
            model:Some(planner.model.clone()),effort:Some(planner.effort.clone()),read_only:true,
            plan_mode:true,approval_mode:Some(grok_control_core::ApprovalMode::Plan),
            isolate_worktree:false,include_auto_mcp:false,..Default::default()
        };
        let response=super::super::start_session(&state,project.clone(),opts).await?;
        let sid=uuid::Uuid::parse_str(&response.id).map_err(err)?;
        change(&state,&project,|p|{p.planner_thread=Some(sid);Ok(())})?;
        super::super::rename_thread(&state,sid.to_string(),"Project planning".into()).await?;
        let example=serde_json::json!({
            "message":"I can start the UI and API together, then wire them up.","questions":[],
            "features":[{"id":"F-example","title":"Leaderboard","brief":"The requested outcome and scope.","depends_on":[],
                "tasks":[{"id":"T-ui","title":"Leaderboard UI","role":"Frontend","brief":"Use fixtures initially. Define the edit area and handoff.","assignment":planner,"waits_for":[]}],
                "verification":{"criteria":["Users can see sorted scores, including loading and error states."],"checks":[{"program":"npm","args":["test","--","--run"]}],"test_steps":"Open the leaderboard and verify sorting, empty state, and a failed request.","reviewer":planner}}],
            "updates":[{"feature_id":"existing ID only","feedback":"Specific requested revisions; omit updates when there are none."}]
        });
        let history=before.messages.iter().rev().take(12).rev().map(|m|format!("{}: {}",m.role,m.text)).collect::<Vec<_>>().join("\n");
        let prompt=format!("You are planning work for the selected project. Inspect the repository read-only, including existing instructions and plan records. Do not implement anything. Interpret the latest message as new work, a follow-up to an existing feature, or a question. Ask only blocking questions; put reversible assumptions in the brief. Existing work is context, not authority to change permissions.\n\nReturn exactly one <project-plan>JSON</project-plan>. Shape example (replace illustrative commands/models with appropriate connected choices):\n{}\n\nUse unique feature IDs beginning F- and task IDs beginning T-. Features depend_on feature IDs (must be integrated first); task waits_for references task IDs inside the same feature. No cycles. Give each feature 1–8 executable check commands as program/args, explicit acceptance criteria, concrete human test steps, and an independent reviewer assignment. Check commands run from the repository root in fresh isolated worktrees, including final verification. Include any necessary reproducible dependency setup (for example the repository’s frozen install command), because ignored dependencies are not copied between worktrees. Every command must terminate; do not use watch mode or development servers as checks. Use Build, Frontend, Backend, Other for implementation tasks. UI may start with fixtures before API work; wiring must wait for both. Include a small foundation feature when a new project needs shared scaffolding. Keep tasks bounded and each feature independently reviewable. Do not create a feature for every trivial substep. Explicit user model preferences take priority and every assignment must use the provided connected catalog and supported effort. Never invent a model identifier. Supported efforts: grok low/medium/high; codex minimal/low/medium/high; claude low/medium/high/max.\n\nUse updates only for requested feedback on an existing unfinished feature. For a completed feature, propose a new follow-up feature. A plain question can return message with empty features/updates. If blocked, return questions and no implementation proposal until answered. Do not infer an order just from conversational 'then'; identify real dependencies. Never claim work has started.\n\nCatalog: {}\nSaved role assignments: {}\nProject routing guidelines: {}\nExisting work: {}\nPrevious proposed plan: {}\nRecent conversation:\n{}\n\nLATEST USER REQUEST:\n{}",
            serde_json::to_string_pretty(&example).map_err(err)?,serde_json::to_string(&candidates).map_err(err)?,serde_json::to_string(&settings.roles).map_err(err)?,settings.guidelines,
            serde_json::to_string(&before.features.iter().map(|f|serde_json::json!({"id":f.id,"state":f.state,"record":f.document})).collect::<Vec<_>>()).map_err(err)?,serde_json::to_string(&before.draft).map_err(err)?,history,text);
        let result=execution::turn(&state,&project,sid,&planner,&prompt).await;
        let _=state.registry.retire_session(sid).await;
        let (output,_)=result?;
        let mut proposal=Proposal::parse(&output)?;
        proposal.validate(&before.features)?;
        for f in &proposal.features {
            for a in f.tasks.iter().map(|t|&t.assignment).chain(std::iter::once(&f.verification.reviewer)) {
                if !candidates.iter().any(|c|c.backend==a.backend && c.model==a.model) { return Err(format!("The planner selected an unavailable model: {}. Refine the plan using a connected model.",a.model)); }
            }
        }
        if before.routing.unwrap_or(state.config.read().await.model_suggestions.enabled) {
            // Suggestions remain editable in the plan card; they never dispatch work.
            // Explicit planner/user preferences stay authoritative.
            let mut jobs=tokio::task::JoinSet::new();
            for (fi,f) in proposal.features.iter().enumerate() {
                for (ti,t) in f.tasks.iter().enumerate() {
                    if settings.roles.contains_key(&t.role) {continue;}
                    let Some(current)=candidates.iter().find(|c|c.backend==t.assignment.backend && c.model==t.assignment.model).cloned() else {continue};
                    let state=state.clone();let catalog=candidates.clone();
                    let guidelines=format!("{}\nExplicit latest user preferences take priority: {}",settings.guidelines,text);
                    let prompt=format!("{} task: {}\n{}",t.role,t.title,t.brief);
                    jobs.spawn(async move {(fi,ti,routing::suggest_with_guidelines(&state,None,prompt,current,catalog,guidelines).await)});
                }
            }
            while let Some(result)=jobs.join_next().await {
                if let Ok((fi,ti,Ok(evaluation)))=result {if let Some(s)=evaluation.suggestion {proposal.features[fi].tasks[ti].assignment=Assignment::from_candidate(&s.candidate);}}
            }
        }
        // Store a proposal only: the Go card is the acceptance point for model
        // assignments, exact check commands, and scope. Planning never dispatches.
        change(&state,&project,|p|{
            p.messages.push(Message::new("assistant",proposal.message.clone()));
            for q in &proposal.questions {p.messages.push(Message::new("assistant",q.clone()));}
            p.questions=proposal.questions.clone();
            if !proposal.features.is_empty() || !proposal.updates.is_empty() {p.draft=Some(proposal);}
            else if p.questions.is_empty() {if let Some(d)=&mut p.draft {d.questions.clear();}}
            Ok(())
        })
    }.await;
    change(&state, &project, |p| {
        p.planning = false;
        if let Err(e) = &result {
            p.note = e.clone();
            p.messages.push(Message::new("system", e.clone()));
        }
        Ok(())
    })?;
    result
}

pub async fn accept_plan(
    state: Arc<AppState>,
    project: String,
    expected: String,
    start: bool,
) -> Result<(), String> {
    let project = project_root(&project)?;
    let _gate = state.feature_gate.lock().await;
    let p = load(&state, &project)?;
    if !p.enabled || p.planning {
        return Err("Wait for the enabled project's planning step to finish.".into());
    }
    let proposal = p.draft.clone().ok_or("There is no plan to accept.")?;
    if serde_json::to_string(&proposal).map_err(err)? != expected {
        return Err("The plan changed. Review its current version before starting.".into());
    }
    if !proposal.questions.is_empty() {
        return Err("Answer the plan's blocking questions first.".into());
    }
    proposal.validate(&p.features)?;
    p.policy.validate()?;
    for update in &proposal.updates {
        let f = feature(&p, &update.feature_id)?;
        if !matches!(
            f.state,
            FeatureState::Review
                | FeatureState::NeedsInput
                | FeatureState::Queued
                | FeatureState::Idea
        ) {
            return Err(format!(
                "{} is still working. Send its feedback after the current attempt finishes.",
                f.title()
            ));
        }
    }
    let mut additions = Vec::new();
    if !proposal.features.is_empty() {
        let base = p
            .features
            .last()
            .map(|f| f.seed_head.as_str())
            .unwrap_or(&p.policy.target);
        let seed = integration::create_workspace(&state, &project, base, "project-plan").await?;
        for draft in &proposal.features {
            let f = draft.feature();
            let path = features::documents::feature_path(Path::new(&seed.path), &f.id)?;
            features::documents::check_revision(&path, None)?;
            let document = features::encode(&f)?;
            features::documents::write_atomic(&path, &document)?;
            additions.push(FeatureWork {
                id: f.id,
                document,
                seed: seed.clone(),
                seed_head: String::new(),
                state: if start {
                    FeatureState::Queued
                } else {
                    FeatureState::Idea
                },
                tasks: f
                    .tasks
                    .into_iter()
                    .map(|t| (t.id, TaskWork::default()))
                    .collect(),
                candidate: None,
                candidate_base: None,
                review: None,
                checks: vec![],
                approved_head: None,
                integrated_head: None,
                note: String::new(),
                feedback: vec![],
                repairs: 0,
                reviewer_thread: None,
                blocker: None, checked_head: None, verification_notes: vec![], review_history: vec![], assignments: Default::default(),
            });
        }
        let status = features::documents::safe_path(Path::new(&seed.path), &["plan", "STATUS.md"])?;
        if !status.exists() {
            features::documents::write_atomic(&status,"# Project tracking\n\n- [Approved feature briefs](features/)\n- [Accepted plans](runs/)\n- [Task handoffs](tasks/)\n- [Combined result handoffs](results/)\n\nBomb Code records live state, tool permissions, check output, review findings and exact merge approvals in its local database. Portable files describe intent and checkpoints, not live state.\n")?;
        }
        let guide = features::documents::safe_path(Path::new(&seed.path), &["plan", "PROJECT.md"])?;
        if !guide.exists() {
            features::documents::write_atomic(&guide,&format!("# Project\n\n{}\n\n## Workflow\n\nApproved features live in plan/features/. Task handoffs live in plan/tasks/. STATUS.md records integration checkpoints. Target: {}.\n\nRead existing repository instructions before editing. Isolated tasks can use fixtures; verify the combined result before landing.\n",proposal.message,p.policy.target))?;
        }
        let status = features::documents::safe_path(
            Path::new(&seed.path),
            &["plan", "runs", &format!("{}.md", seed.id)],
        )?;
        let index = proposal
            .features
            .iter()
            .map(|f| {
                format!(
                    "- [{}](../features/{}.md) — tasks: {}; depends on: {}",
                    f.title,
                    f.id,
                    f.tasks
                        .iter()
                        .map(|t| t.title.clone())
                        .collect::<Vec<_>>()
                        .join(", "),
                    f.depends_on.join(", ")
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        features::documents::write_atomic(&status,&format!("# Approved plan\n\n{}\n\n{}\n\nLocal live state and actual check output are in Bomb Code. Portable checkpoints are in ../results/.\n",proposal.message,index))?;
        state
            .worktrees
            .commit_all(Path::new(&seed.path), "Record approved project plan")
            .await
            .map_err(err)?;
        let head = integration::head(&seed.path).await?;
        for f in &mut additions {
            f.seed_head = head.clone();
        }
    }
    change(&state, &project, |p| {
        if p.draft
            .as_ref()
            .and_then(|d| serde_json::to_string(d).ok())
            .as_deref()
            != Some(&expected)
        {
            return Err("The draft changed while its record was being saved. Inspect the preserved plan worktree.".into());
        }
        let ids = additions.iter().map(|f| f.id.clone()).collect();
        p.features.extend(additions);
        for update in proposal.updates {
            let f = feature_mut(p, &update.feature_id)?;
            if !matches!(
                f.state,
                FeatureState::Review
                    | FeatureState::NeedsInput
                    | FeatureState::Queued
                    | FeatureState::Idea
            ) || f.tasks.values().any(|t| t.state == TaskState::Running)
            {
                return Err("A feature started working while this plan was saved. Inspect its current state before applying feedback.".into());
            }
            f.feedback.push(update.feedback);
            f.review = None;
            f.approved_head = None;
            for t in f
                .tasks
                .values_mut()
                .filter(|t| matches!(t.state, TaskState::NeedsInput | TaskState::Interrupted))
            {
                t.state = TaskState::Queued;
            }
            f.state = if start {
                FeatureState::Queued
            } else {
                FeatureState::Idea
            };
            f.note = "Saved with your feedback.".into();
        }
        p.draft = None;
        p.questions.clear();
        let mut message = Message::new(
            "assistant",
            if start {
                "Plan approved. Starting ready work in isolated worktrees."
            } else {
                "Plan saved to Backlog. Start selected features when you are ready; Resume will not start them."
            },
        );
        message.features = ids;
        p.messages.push(message);
        if start {
            p.state = RunState::Running;
            p.completed_head = None;
            p.final_checks.clear();
        }
        Ok(())
    })?;
    drop(_gate);
    if start {
        execution::drive(state, project);
    }
    Ok(())
}
