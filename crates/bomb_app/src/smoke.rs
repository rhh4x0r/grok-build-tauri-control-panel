//! `BOMB_SMOKE=1`: drive one mock turn end-to-end and print the resulting
//! transcript to the log, then quit. Lets CI (and a headless developer)
//! verify the bridge → reducer → model path without a screen.

use std::time::Duration;

use gpui_kit::*;

use crate::models::app::AppModel;

pub fn maybe_run(model: Entity<AppModel>, cx: &mut App) {
    if std::env::var("BOMB_SMOKE").ok().as_deref() != Some("1") {
        return;
    }
    tracing::info!("smoke: starting mock session");
    let project = crate::runtime::services(cx)
        .paths
        .home_dir
        .join("workflow-fixture")
        .to_string_lossy()
        .into_owned();
    cx.spawn(async move |cx| {
        model.update(cx, |m, cx| {m.active_project=Some(project.clone());m.new_mock_session(cx);});
        cx.background_executor().timer(Duration::from_millis(1500)).await;
        model.update(cx, |m, cx| {
            tracing::info!(selected = ?m.selected, threads = m.threads.len(), "smoke: sending prompt");
            m.send_prompt("hello from the smoke test".into(), Vec::new(), cx);
        });
        cx.background_executor().timer(Duration::from_millis(6000)).await;
        model.update(cx, |m, cx| {
            match m.selected_thread() {
                Some(t) => {
                    let t = t.read(cx);
                    tracing::info!(
                        phase = ?t.thread.presence.phase,
                        entries = t.thread.entries.len(),
                        label = ?t.thread.label,
                        "smoke: transcript"
                    );
                    for e in &t.thread.entries {
                        let body = match &e.body {
                            bomb_core::transcript::Body::Text(s) => s.chars().take(80).collect::<String>(),
                            other => format!("{other:?}").chars().take(80).collect(),
                        };
                        tracing::info!(role = ?e.role, streaming = e.streaming, %body, "smoke: entry");
                    }
                    tracing::info!(protocol = t.thread.protocol_log.len(), markdown_states = t.markdown.len(), "smoke: side state");
                }
                None => tracing::error!("smoke: no selected thread"),
            }
        });
        // Settings screen: open it in the main window and report what loaded.
        cx.update(|cx| cx.dispatch_action(&crate::actions::OpenSettings));
        cx.background_executor().timer(Duration::from_millis(2500)).await;
        cx.update(|cx| {
            match cx.try_global::<crate::views::settings::SettingsHandle>() {
                Some(h) => {
                    let m = h.0.read(cx);
                    tracing::info!(
                        config = m.config.is_some(),
                        mcp = m.mcp.len(),
                        catalog = m.catalog.len(),
                        credentials = m.credentials.len(),
                        memory = m.memory.len(),
                        worktree_repos = m.worktrees.len(),
                        runtime = m.runtime.is_some(),
                        presets = m.presets.len(),
                        "smoke: settings loaded"
                    );
                }
                None => tracing::error!("smoke: settings model missing"),
            }
        });
        model.update(cx,|m,cx|m.set_active_project(project.clone(),cx));
        cx.update(|cx|cx.dispatch_action(&crate::actions::OpenFeatures));
        cx.background_executor().timer(Duration::from_millis(1000)).await;
        for id in ["F-foundation","F-ready"] {
            model.update(cx,|m,cx|{m.project_feature_request=Some(id.into());cx.notify();});
            cx.background_executor().timer(Duration::from_millis(1000)).await;
            tracing::info!(feature=id,"smoke: project decision rendered");
        }
        cx.update(|cx|cx.dispatch_action(&crate::actions::NewFeature));
        cx.background_executor().timer(Duration::from_millis(1000)).await;
        tracing::info!("smoke: project board, blocked/ready decisions and proposed plan rendered");
        cx.update(|cx| cx.quit());
    })
    .detach();
}

/// Seed representative UX states in the smoke-only temporary database. No agents
/// are dispatched and no real project is imported or changed.
pub async fn prepare_workflow(state: &bomb_core::AppState) -> Result<(), String> {
    use bomb_core::services::{
        features::{self, Assignment, CheckCommand, Verification},
        project_work::*,
    };
    let root = state.paths.home_dir.join("workflow-fixture");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    for args in [
        vec!["init", "-b", "main"],
        vec!["config", "user.name", "Smoke"],
        vec!["config", "user.email", "smoke@localhost"],
        vec!["commit", "--allow-empty", "-m", "Smoke fixture"],
    ] {
        grok_worktree::run_git(&root, &args)
            .await
            .map_err(|e| e.to_string())?;
    }
    let root = root
        .canonicalize()
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .into_owned();
    let head = grok_worktree::run_git(std::path::Path::new(&root), &["rev-parse", "HEAD"])
        .await
        .map_err(|e| e.to_string())?
        .trim()
        .to_string();
    let assignment = Assignment {
        backend: "grok".into(),
        model: "mock".into(),
        effort: "medium".into(),
    };
    let draft = FeatureDraft {
        id: "F-foundation".into(),
        title: "Multiplayer foundation".into(),
        brief: "Connect a real backend and preserve solo gameplay.".into(),
        depends_on: vec![],
        tasks: vec![TaskDraft {
            id: "T-build".into(),
            title: "Foundation build".into(),
            brief: "Build and verify the foundation.".into(),
            role: "Backend".into(),
            assignment: assignment.clone(),
            waits_for: vec![],
        }],
        verification: Verification {
            criteria: vec!["The backend belongs to the intended project.".into()],
            checks: vec![CheckCommand {
                program: "true".into(),
                args: vec![],
            }],
            test_steps: "Open the game and check the solo controls; confirm backend ownership."
                .into(),
            reviewer: assignment.clone(),
        },
    };
    let workspace = Workspace {
        id: "W-smoke".into(),
        path: root.clone(),
        branch: "main".into(),
        session: None,
    };
    let check = CheckEvidence {
        command: draft.verification.checks[0].clone(),
        exit_code: Some(0),
        output: "All checks passed in the smoke fixture.".into(),
        at: now(),
    };
    let mut f=FeatureWork {id:draft.id.clone(),document:features::encode(&draft.feature())?,seed:workspace.clone(),seed_head:head.clone(),state:FeatureState::NeedsInput,tasks:std::collections::BTreeMap::from([("T-build".into(),TaskWork {state:TaskState::Checkpointed,head:Some(head.clone()),summary:"Build finished; verification still needs access.".into(),..Default::default()})]),candidate:Some(workspace),candidate_base:Some(head.clone()),review:None,checks:vec![check.clone()],approved_head:None,integrated_head:None,note:"Review policy blocked external verification. Your completed build and checks are saved.".into(),feedback:vec![],repairs:0,pending_repair:None,reviewer_thread:None,blocker:Some(Blocker::new(WorkStage::Review,"Review policy blocked external verification.",None)),checked_head:Some(head.clone()),verification_notes:vec![],review_history:vec![],assignments:Default::default()};
    let mut ready = f.clone();
    ready.id = "F-ready".into();
    let mut ready_draft = draft.clone();
    ready_draft.id = ready.id.clone();
    ready_draft.title = "HUD visual polish".into();
    ready.document = features::encode(&ready_draft.feature())?;
    ready.state = FeatureState::Review;
    ready.blocker = None;
    ready.note = "Ready to test and approve.".into();
    ready.review = Some(ReviewEvidence {
        candidate: head.clone(),
        base: head.clone(),
        document: ready.document.clone(),
        summary: "The HUD result is ready to test.".into(),
        criteria: vec![],
        checks: vec![check],
        reviewer: assignment,
        thread: uuid::Uuid::new_v4(),
        at: now(),
    });
    let mut queued = f.clone();
    queued.id = "F-lobby".into();
    let mut child = draft.clone();
    child.id = queued.id.clone();
    child.title = "Lobby".into();
    child.depends_on = vec![draft.id.clone()];
    queued.document = features::encode(&child.feature())?;
    queued.state = FeatureState::Queued;
    queued.blocker = None;
    queued.note.clear();
    queued.candidate = None;
    queued.checks.clear();
    queued
        .tasks
        .values_mut()
        .for_each(|t| *t = TaskWork::default());
    let mut later = draft.clone();
    later.id = "F-later".into();
    later.title = "Leaderboard".into();
    f.note.push_str(&format!(
        " Technical detail: {}",
        "long-tool-output ".repeat(50)
    ));
    let p = ProjectWork {
        enabled: true,
        state: RunState::Paused,
        features: vec![f, ready, queued],
        draft: Some(Proposal {
            message: "A separate proposed feature.".into(),
            features: vec![later],
            ..Default::default()
        }),
        messages: vec![
            Message::new("user", "Build multiplayer and improve the HUD."),
            Message::new(
                "assistant",
                "Foundation needs verification; the HUD is ready to test.",
            ),
        ],
        ..Default::default()
    };
    state
        .persistence
        .set_kv(
            &format!("project-work/v1/{root}"),
            &serde_json::to_string(&p).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    load(state, &root)?;
    Ok(())
}
