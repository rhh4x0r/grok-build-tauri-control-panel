use super::*;
use execution::{next_job, Job};
use features::{Assignment, CheckCommand, Verification};
use grok_worktree::run_git;

async fn fixture() -> (tempfile::TempDir, Arc<AppState>, String) {
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
    (
        temp,
        Arc::new(state),
        repo.canonicalize().unwrap().to_string_lossy().into_owned(),
    )
}
fn assignment() -> Assignment {
    Assignment {
        backend: "grok".into(),
        model: "mock".into(),
        effort: "medium".into(),
    }
}
fn draft(id: &str) -> FeatureDraft {
    FeatureDraft {
        id: id.into(),
        title: "Scores".into(),
        brief: "Show sorted scores, loading and failure states.".into(),
        depends_on: vec![],
        tasks: [
            ("T-ui", "Frontend"),
            ("T-api", "Backend"),
            ("T-wire", "Build"),
        ]
        .into_iter()
        .map(|(id, role)| TaskDraft {
            id: id.into(),
            role: role.into(),
            title: role.into(),
            brief: format!("Build {role} and verify its behavior."),
            assignment: assignment(),
            waits_for: if role == "Build" {
                vec!["T-ui".into(), "T-api".into()]
            } else {
                vec![]
            },
        })
        .collect(),
        verification: Verification {
            criteria: vec!["Scores are sorted".into()],
            checks: vec![CheckCommand {
                program: "git".into(),
                args: vec!["diff".into(), "--check".into()],
            }],
            test_steps: "Open scores and test loading, ordering and errors.".into(),
            reviewer: assignment(),
        },
    }
}
fn work(d: FeatureDraft, w: Workspace) -> FeatureWork {
    let f = d.feature();
    FeatureWork {
        id: f.id.clone(),
        document: features::encode(&f).unwrap(),
        seed: w,
        seed_head: "test-seed".into(),
        state: FeatureState::Queued,
        tasks: f
            .tasks
            .iter()
            .map(|t| (t.id.clone(), TaskWork::default()))
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
        pending_repair: None,
        reviewer_thread: None,
        blocker: None,
        checked_head: None,
        verification_notes: vec![],
        review_history: vec![],
        assignments: Default::default(),
    }
}
fn fake_workspace() -> Workspace {
    Workspace {
        id: "W-test".into(),
        path: "/tmp/not-used".into(),
        branch: "task-test".into(),
        session: None,
    }
}
fn proposal() -> Proposal {
    Proposal {
        message: "Build scores.".into(),
        features: vec![draft("F-scores")],
        ..Default::default()
    }
}

fn planning_question() -> PlanningQuestion {
    PlanningQuestion {
        prompt: "Who should be able to see the scores?".into(),
        options: vec!["Only me".into(), "My team".into()],
    }
}

#[test]
fn legacy_plans_and_free_text_planning_questions_remain_readable() {
    let old = Proposal::parse(
        r#"<project-plan>{"message":"Existing plan","questions":[],"features":[],"updates":[]}</project-plan>"#,
    )
    .unwrap();
    old.validate(&[]).unwrap();
    assert!(old.planning_question.is_none());
    assert!(old.decisions.is_empty());
    assert!(old.assumptions.is_empty());

    let open = Proposal::parse(
        r#"<project-plan>{"planning_question":{"prompt":"What should a successful result let you do?"}}</project-plan>"#,
    )
    .unwrap();
    open.validate(&[]).unwrap();
    assert!(open.planning_question.unwrap().options.is_empty());
}

#[test]
fn planning_question_rejects_empty_ambiguous_and_excessive_choices() {
    let mut p = proposal();
    p.planning_question = Some(planning_question());
    p.validate(&[]).unwrap();
    for options in [
        vec!["One".into()],
        vec!["One".into(), "  ".into()],
        vec!["One".into(), " one ".into()],
        vec!["One".into(), "Two".into(), "Three".into(), "Four".into()],
        vec!["One".into(), "x".repeat(501)],
    ] {
        p.planning_question.as_mut().unwrap().options = options;
        assert!(p.validate(&[]).is_err());
    }
    p.planning_question = Some(PlanningQuestion {
        prompt: " ".into(),
        options: vec![],
    });
    assert!(p.validate(&[]).is_err());
}

#[tokio::test]
async fn guided_planning_survives_restart_and_keeps_scope_until_revised() {
    let (_temp, state, root) = fixture().await;
    let mut draft = proposal();
    draft.decisions = vec!["Keep the scores private.".into()];
    draft.assumptions = vec!["Use the existing visual style.".into()];
    let scope = serde_json::to_string(&draft.features).unwrap();
    change(&state, &root, |p| {
        p.enabled = true;
        planning::record_proposal(p, draft, Default::default())?;
        planning::record_proposal(
            p,
            Proposal {
                planning_question: Some(planning_question()),
                decisions: vec!["Keep the scores private.".into()],
                assumptions: vec!["Use the existing visual style.".into()],
                ..Default::default()
            },
            Default::default(),
        )?;
        planning::record_proposal(
            p,
            Proposal {
                message: "A team includes people you invite.".into(),
                ..Default::default()
            },
            Default::default(),
        )
    })
    .unwrap();
    let restarted = ProjectWorkService::new(state.persistence.clone());
    let restored = restarted.load(&root).unwrap();
    let restored = restored.draft.unwrap();
    assert_eq!(serde_json::to_string(&restored.features).unwrap(), scope);
    assert_eq!(restored.planning_question, Some(planning_question()));
    assert_eq!(restored.decisions, ["Keep the scores private."]);
    assert_eq!(restored.assumptions, ["Use the existing visual style."]);

    let mut updated = restored;
    updated.planning_question = None;
    updated.decisions = vec!["Share scores with invited teammates.".into()];
    change(&state, &root, |p| {
        planning::record_proposal(p, updated, Default::default())
    })
    .unwrap();
    let updated = load(&state, &root).unwrap().draft.unwrap();
    assert!(updated.planning_question.is_none());
    assert_eq!(updated.decisions, ["Share scores with invited teammates."]);
}

#[test]
fn planning_can_begin_with_a_question_before_features_exist() {
    let mut p = ProjectWork::default();
    planning::record_proposal(
        &mut p,
        Proposal {
            planning_question: Some(planning_question()),
            ..Default::default()
        },
        Default::default(),
    )
    .unwrap();
    let draft = p.draft.unwrap();
    assert!(draft.features.is_empty());
    assert_eq!(draft.planning_question, Some(planning_question()));
}

#[test]
fn next_planning_question_can_clear_rejected_decisions_and_assumptions() {
    let mut draft = proposal();
    draft.decisions = vec!["Create public profiles.".into()];
    draft.assumptions = vec!["Everyone can see the scores.".into()];
    let scope = serde_json::to_string(&draft.features).unwrap();
    let mut p = ProjectWork {
        draft: Some(draft),
        ..Default::default()
    };
    planning::record_proposal(
        &mut p,
        Proposal {
            message: "Public profiles are removed from the plan. Let's settle access.".into(),
            planning_question: Some(planning_question()),
            decisions: vec![],
            assumptions: vec![],
            ..Default::default()
        },
        Default::default(),
    )
    .unwrap();
    let updated = p.draft.unwrap();
    assert!(updated.decisions.is_empty());
    assert!(updated.assumptions.is_empty());
    assert_eq!(updated.planning_question, Some(planning_question()));
    assert_eq!(serde_json::to_string(&updated.features).unwrap(), scope);
}

#[tokio::test]
async fn pending_planning_choice_blocks_start_and_save_without_side_effects() {
    let (_temp, state, root) = fixture().await;
    enable(&state, &root, true).await.unwrap();
    let mut draft = proposal();
    draft.planning_question = Some(planning_question());
    update_draft(&state, &root, draft.clone()).unwrap();
    let expected = serde_json::to_string(&draft).unwrap();
    for start in [false, true] {
        let error = accept_plan(state.clone(), root.clone(), expected.clone(), start)
            .await
            .unwrap_err();
        assert!(error.contains("Answer the planning question"));
        let p = load(&state, &root).unwrap();
        assert!(p.features.is_empty());
        assert!(p.draft.is_some());
        assert!(state.project_work.driving.lock().unwrap().is_empty());
        assert_eq!(
            run_git(Path::new(&root), &["worktree", "list", "--porcelain"])
                .await
                .unwrap()
                .matches("worktree ")
                .count(),
            1
        );
    }
}

#[test]
fn blocked_project_reports_attention_and_named_dependency() {
    let mut foundation = work(draft("F-foundation"), fake_workspace());
    foundation.state = FeatureState::NeedsInput;
    foundation.blocker = Some(Blocker::new(
        WorkStage::Review,
        "Read-only policy blocked permission",
        None,
    ));
    let mut child = draft("F-child");
    child.depends_on = vec![foundation.id.clone()];
    let child = work(child, fake_workspace());
    let p = ProjectWork {
        state: RunState::Running,
        features: vec![foundation, child],
        ..Default::default()
    };
    assert!(p.activity_label().contains("Waiting for you"));
    assert!(p.activity_label().contains("no jobs running"));
    assert_eq!(p.features[0].state.column(), "Needs you");
    assert_eq!(
        p.waiting_reason(&p.features[1]),
        "Waiting for Scores to merge"
    );
    assert_eq!(p.unblocks("F-foundation"), 1);
    assert!(next_job(&p, &HashSet::new()).is_none());
}

#[tokio::test]
async fn resume_never_dispatches_saved_backlog() {
    let (_temp, state, root) = fixture().await;
    change(&state, &root, |p| {
        p.enabled = true;
        let mut f = work(draft("F-later"), fake_workspace());
        f.state = FeatureState::Idea;
        p.features.push(f);
        Ok(())
    })
    .unwrap();
    command(state.clone(), root.clone(), "resume")
        .await
        .unwrap();
    let p = load(&state, &root).unwrap();
    assert_eq!(p.features[0].state, FeatureState::Idea);
    assert!(next_job(&p, &HashSet::new()).is_none());
}

#[tokio::test]
async fn stale_recovery_cannot_repeat_a_stage() {
    let (_temp, state, root) = fixture().await;
    change(&state, &root, |p| {
        p.enabled = true;
        let mut f = work(draft("F-work"), fake_workspace());
        f.state = FeatureState::NeedsInput;
        f.blocker = Some(Blocker::new(WorkStage::Build, "Interrupted", None));
        p.features.push(f);
        Ok(())
    })
    .unwrap();
    assert!(retry_stage(
        state.clone(),
        root.clone(),
        "F-work".into(),
        "stale-id".into(),
        None
    )
    .await
    .is_err());
    assert_eq!(
        load(&state, &root).unwrap().features[0].state,
        FeatureState::NeedsInput
    );
}

#[test]
fn infrastructure_failures_are_not_code_findings() {
    assert_eq!(
        Blocker::classify("getaddrinfo ENOTFOUND api.convex.dev"),
        BlockerKind::Environment
    );
    assert_eq!(
        Blocker::classify("Review policy blocked permission"),
        BlockerKind::Access
    );
    assert_eq!(
        Blocker::classify("provider model unavailable"),
        BlockerKind::Provider
    );
    assert_eq!(
        Blocker::classify("session cancelled"),
        BlockerKind::Interrupted
    );
}

#[test]
fn asking_about_a_plan_preserves_its_scope_and_assignments() {
    let mut p = ProjectWork {
        draft: Some(proposal()),
        ..Default::default()
    };
    let before = serde_json::to_string(p.draft.as_ref().unwrap()).unwrap();
    planning::record_proposal(
        &mut p,
        Proposal {
            message: "UI can start with fixtures.".into(),
            ..Default::default()
        },
        Default::default(),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_string(p.draft.as_ref().unwrap()).unwrap(),
        before
    );
    planning::record_proposal(
        &mut p,
        Proposal {
            questions: vec!["Which Convex project?".into()],
            ..Default::default()
        },
        Default::default(),
    )
    .unwrap();
    assert_eq!(p.needs_attention(), 1);
    assert!(p.draft.is_some());
}

#[tokio::test]
async fn selected_plan_cannot_omit_a_required_backlog_prerequisite() {
    let (_temp, state, root) = fixture().await;
    enable(&state, &root, true).await.unwrap();
    let mut child = draft("F-child");
    child.depends_on = vec!["F-parent".into()];
    let proposal = Proposal {
        features: vec![draft("F-parent"), child],
        ..Default::default()
    };
    update_draft(&state, &root, proposal.clone()).unwrap();
    let error = accept_selected_plan(
        state.clone(),
        root.clone(),
        serde_json::to_string(&proposal).unwrap(),
        true,
        vec!["F-child".into()],
    )
    .await
    .unwrap_err();
    assert!(error.contains("prerequisite"));
    assert!(load(&state, &root).unwrap().features.is_empty());
}

#[tokio::test]
async fn saved_feedback_is_isolated_by_project_and_feature() {
    let (_temp, state, root) = fixture().await;
    save_draft_text(&state, &root, "project", "new idea").unwrap();
    save_draft_text(&state, &root, "F-one", "fix loading state").unwrap();
    assert_eq!(draft_text(&state, &root, "project"), "new idea");
    assert_eq!(draft_text(&state, &root, "F-one"), "fix loading state");
    assert!(draft_text(&state, &root, "F-two").is_empty());
    assert!(draft_text(&state, "another-project", "F-one").is_empty());
}

#[tokio::test]
async fn explicit_model_replacement_cannot_rewrite_finished_work() {
    let (_temp, state, root) = fixture().await;
    change(&state, &root, |p| {
        let mut f = work(draft("F-one"), fake_workspace());
        f.tasks.get_mut("T-ui").unwrap().state = TaskState::Checkpointed;
        p.features.push(f);
        Ok(())
    })
    .unwrap();
    assert!(replace_assignment(&state, &root, "F-one", "T-ui", assignment()).is_err());
    replace_assignment(&state, &root, "F-one", "T-api", assignment()).unwrap();
    let p = load(&state, &root).unwrap();
    assert_eq!(p.features[0].assignments.len(), 1);
    assert_eq!(p.features[0].tasks["T-ui"].state, TaskState::Checkpointed);
}

#[test]
fn approval_and_final_blockers_share_the_attention_count() {
    let mut f = work(draft("F-one"), fake_workspace());
    f.state = FeatureState::NeedsInput;
    f.approved_head = Some("approved-before-merge-blocked".into());
    let p = ProjectWork {
        features: vec![f],
        final_blocker: Some(Blocker::new(WorkStage::Final, "integration failed", None)),
        questions: vec!["Which environment?".into()],
        ..Default::default()
    };
    assert_eq!(p.needs_attention(), 3);
}
#[test]
fn planner_contract_preserves_prose_and_rejects_dependency_cycles() {
    let p = proposal();
    let parsed = Proposal::parse(&format!(
        "<project-plan>{}</project-plan>",
        serde_json::to_string(&p).unwrap()
    ))
    .unwrap();
    parsed.validate(&[]).unwrap();
    let content = features::encode(&parsed.features[0].feature()).unwrap();
    assert!(content.starts_with("<!-- bomb-feature/2"));
    let round = features::decode(&content).unwrap();
    assert_eq!(round.brief, p.features[0].brief);
    assert_eq!(round.tasks[0].brief, p.features[0].tasks[0].brief);
    assert!(round.verification.is_some());
    let mut p = p;
    p.features.push(draft("F-followup"));
    p.features[0].depends_on = vec!["F-followup".into()];
    p.features[1].depends_on = vec!["F-scores".into()];
    assert!(p.validate(&[]).unwrap_err().contains("circular"));
    p.features[1].depends_on = vec!["missing".into()];
    assert!(p.validate(&[]).is_err());
}
#[test]
fn scheduler_parallelizes_independent_tasks_and_waits_for_exact_dependencies() {
    let mut p = ProjectWork {
        state: RunState::Running,
        features: vec![work(draft("F-scores"), fake_workspace())],
        ..Default::default()
    };
    let first = next_job(&p, &HashSet::new()).unwrap();
    assert_eq!(first, Job::Task("F-scores".into(), "T-ui".into()));
    p.features[0].tasks.get_mut("T-ui").unwrap().state = TaskState::Running;
    assert_eq!(
        next_job(&p, &HashSet::from(["F-scores/T-ui".into()])),
        Some(Job::Task("F-scores".into(), "T-api".into()))
    );
    p.features[0].tasks.get_mut("T-api").unwrap().state = TaskState::Checkpointed;
    assert!(next_job(&p, &HashSet::new()).is_none());
    p.features[0].tasks.get_mut("T-ui").unwrap().state = TaskState::Checkpointed;
    assert_eq!(
        next_job(&p, &HashSet::new()),
        Some(Job::Task("F-scores".into(), "T-wire".into()))
    );
    p.features[0].tasks.get_mut("T-wire").unwrap().state = TaskState::Checkpointed;
    assert_eq!(
        next_job(&p, &HashSet::new()),
        Some(Job::Candidate("F-scores".into()))
    );
    let mut next = draft("F-next");
    next.depends_on = vec!["F-scores".into()];
    p.features.push(work(next, fake_workspace()));
    p.features[0].state = FeatureState::Review;
    assert!(next_job(&p, &HashSet::new()).is_none());
    p.features[0].state = FeatureState::Done;
    assert_eq!(
        next_job(&p, &HashSet::new()),
        Some(Job::Task("F-next".into(), "T-ui".into()))
    );
    p.features[1].state = FeatureState::Idea;
    assert_eq!(next_job(&p, &HashSet::new()), Some(Job::Final)); // Saved ideas do not block finishing the current run.
    p.state = RunState::Paused;
    assert!(next_job(&p, &HashSet::new()).is_none());
}
#[tokio::test]
async fn saving_plan_is_optional_isolated_and_does_not_dispatch() {
    let (_tmp, state, root) = fixture().await;
    assert!(!load(&state, &root).unwrap().enabled);
    assert!(!Path::new(&root).join("plan").exists());
    enable(&state, &root, true).await.unwrap();
    let mut draft = proposal();
    draft.decisions = vec!["Keep scores private.".into()];
    draft.assumptions = vec!["Use the existing visual style.".into()];
    update_draft(&state, &root, draft.clone()).unwrap();
    let expected = serde_json::to_string(&draft).unwrap();
    accept_plan(state.clone(), root.clone(), expected, false)
        .await
        .unwrap();
    let p = load(&state, &root).unwrap();
    assert_eq!(p.features[0].state, FeatureState::Idea);
    assert!(p.features[0].tasks.values().all(|t| t.workspace.is_none()));
    assert!(!Path::new(&root).join("plan").exists());
    assert!(Path::new(&p.features[0].seed.path)
        .join("plan/features/F-scores.md")
        .exists());
    let approved = std::fs::read_to_string(
        Path::new(&p.features[0].seed.path)
            .join("plan/runs")
            .join(format!("{}.md", p.features[0].seed.id)),
    )
    .unwrap();
    assert!(approved.contains("## Decisions\n\n- Keep scores private."));
    assert!(approved.contains("## Assumptions\n\n- Use the existing visual style."));
    assert!(state.worktrees.is_clean(Path::new(&root)).await.unwrap());
    assert!(state.project_work.driving.lock().unwrap().is_empty());
    let restarted = ProjectWorkService::new(state.persistence.clone());
    assert_eq!(restarted.load(&root).unwrap().features.len(), 1);
}
async fn reviewed(state: &Arc<AppState>, root: &str) -> FeatureWork {
    let w = integration::create_workspace(state, root, "main", "candidate-test")
        .await
        .unwrap();
    let base = integration::head(&w.path).await.unwrap();
    std::fs::write(Path::new(&w.path).join("scores.txt"), "1. Player").unwrap();
    let head = integration::checkpoint(state, &w, "Scores").await.unwrap();
    let mut f = work(draft("F-scores"), w.clone());
    f.candidate = Some(w);
    f.candidate_base = Some(base.clone());
    f.state = FeatureState::Review;
    f.approved_head = Some(head.clone());
    f.review = Some(ReviewEvidence {
        candidate: head,
        base,
        document: f.document.clone(),
        summary: "Sorted scores reviewed.".into(),
        criteria: vec![bomb_foundry::CriterionResult {
            criterion: 0,
            status: "passed".into(),
            evidence: vec!["Inspected score sorting.".into()],
        }],
        checks: vec![],
        reviewer: assignment(),
        thread: uuid::Uuid::new_v4(),
        at: now(),
    });
    change(state, root, |p| {
        p.enabled = true;
        p.state = RunState::Running;
        p.features = vec![f.clone()];
        Ok(())
    })
    .unwrap();
    f
}
#[tokio::test]
async fn landing_preserves_dirty_checkout_and_rejects_changed_candidate() {
    let (_tmp, state, root) = fixture().await;
    let local = Path::new(&root).join("my-work.txt");
    std::fs::write(&local, "saved work").unwrap();
    state
        .worktrees
        .commit_all(Path::new(&root), "Save existing work")
        .await
        .unwrap();
    let f = reviewed(&state, &root).await;
    std::fs::write(Path::new(&root).join("my-work.txt"), "keep this").unwrap();
    assert!(integration::land(&state, &root, &f.id)
        .await
        .unwrap_err()
        .contains("local edits"));
    assert_eq!(
        std::fs::read_to_string(Path::new(&root).join("my-work.txt")).unwrap(),
        "keep this"
    );
    assert!(!Path::new(&root).join("scores.txt").exists());
    std::fs::write(
        Path::new(&f.candidate.unwrap().path).join("scores.txt"),
        "changed after review",
    )
    .unwrap();
    assert!(integration::land(&state, &root, &f.id)
        .await
        .unwrap_err()
        .contains("changed after review"));
}
#[tokio::test]
async fn landing_preserves_unrelated_untracked_plans_without_extra_commits() {
    let (_tmp, state, root) = fixture().await;
    let f = reviewed(&state, &root).await;
    let plan = Path::new(&root).join("plan/features/older-plan.md");
    std::fs::create_dir_all(plan.parent().unwrap()).unwrap();
    std::fs::write(&plan, "An earlier planning draft").unwrap();
    integration::land(&state, &root, &f.id).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(&plan).unwrap(),
        "An earlier planning draft"
    );
    assert_eq!(
        integration::head(&root).await.unwrap(),
        f.approved_head.unwrap()
    );
    assert_eq!(
        load(&state, &root).unwrap().features[0].state,
        FeatureState::Done
    );
    assert_eq!(
        run_git(
            Path::new(&root),
            &["ls-files", "--others", "--exclude-standard"]
        )
        .await
        .unwrap()
        .trim(),
        "plan/features/older-plan.md"
    );
}

#[tokio::test]
async fn landing_preserves_colliding_untracked_and_ignored_files() {
    for ignored in [false, true] {
        let (_tmp, state, root) = fixture().await;
        let f = reviewed(&state, &root).await;
        if ignored {
            std::fs::write(Path::new(&root).join(".git/info/exclude"), "scores.txt\n").unwrap();
        }
        let local = Path::new(&root).join("scores.txt");
        std::fs::write(&local, "Unsaved local scores").unwrap();
        let before = integration::head(&root).await.unwrap();
        assert!(integration::land(&state, &root, &f.id).await.is_err());
        assert_eq!(integration::head(&root).await.unwrap(), before);
        assert_eq!(
            std::fs::read_to_string(local).unwrap(),
            "Unsaved local scores"
        );
        assert_ne!(
            load(&state, &root).unwrap().features[0].state,
            FeatureState::Done
        );
    }
}

#[tokio::test]
async fn landing_preserves_untracked_directory_in_the_way_of_a_file() {
    let (_tmp, state, root) = fixture().await;
    let f = reviewed(&state, &root).await;
    let local = Path::new(&root).join("scores.txt/draft.txt");
    std::fs::create_dir_all(local.parent().unwrap()).unwrap();
    std::fs::write(&local, "Local draft").unwrap();
    let before = integration::head(&root).await.unwrap();
    assert!(integration::land(&state, &root, &f.id).await.is_err());
    assert_eq!(integration::head(&root).await.unwrap(), before);
    assert_eq!(std::fs::read_to_string(local).unwrap(), "Local draft");
}

#[tokio::test]
async fn landing_preserves_staged_changes() {
    let (_tmp, state, root) = fixture().await;
    let f = reviewed(&state, &root).await;
    let local = Path::new(&root).join("new-file.txt");
    std::fs::write(&local, "Staged work").unwrap();
    run_git(Path::new(&root), &["add", "new-file.txt"])
        .await
        .unwrap();
    let before = integration::head(&root).await.unwrap();
    assert!(integration::land(&state, &root, &f.id)
        .await
        .unwrap_err()
        .contains("local edits"));
    assert_eq!(integration::head(&root).await.unwrap(), before);
    assert_eq!(std::fs::read_to_string(local).unwrap(), "Staged work");
    assert_eq!(
        run_git(Path::new(&root), &["diff", "--cached", "--name-only"])
            .await
            .unwrap()
            .trim(),
        "new-file.txt"
    );
}

#[tokio::test]
async fn target_advance_invalidates_review_and_approval() {
    let (_tmp, state, root) = fixture().await;
    let f = reviewed(&state, &root).await;
    std::fs::write(Path::new(&root).join("other.txt"), "another feature").unwrap();
    state
        .worktrees
        .commit_all(Path::new(&root), "Another feature")
        .await
        .unwrap();
    integration::land(&state, &root, &f.id).await.unwrap();
    let p = load(&state, &root).unwrap();
    assert_eq!(p.features[0].state, FeatureState::Queued);
    assert!(p.features[0].review.is_none());
    assert!(p.features[0].approved_head.is_none());
    assert!(!Path::new(&root).join("scores.txt").exists());
}
#[tokio::test]
async fn exact_reviewed_commit_lands_and_crash_reconciliation_is_idempotent() {
    let (_tmp, state, root) = fixture().await;
    let f = reviewed(&state, &root).await;
    let head = f.review.as_ref().unwrap().candidate.clone();
    change(&state, &root, |p| {
        p.state = RunState::Paused;
        Ok(())
    })
    .unwrap();
    integration::land(&state, &root, &f.id).await.unwrap();
    assert!(!Path::new(&root).join("scores.txt").exists());
    change(&state, &root, |p| {
        p.state = RunState::Running;
        Ok(())
    })
    .unwrap();
    integration::land(&state, &root, &f.id).await.unwrap();
    assert_eq!(integration::head(&root).await.unwrap(), head);
    change(&state, &root, |p| {
        p.features[0].state = FeatureState::Landing;
        Ok(())
    })
    .unwrap();
    integration::land(&state, &root, &f.id).await.unwrap();
    assert_eq!(integration::head(&root).await.unwrap(), head);
    assert_eq!(
        load(&state, &root).unwrap().features[0].state,
        FeatureState::Done
    );
}
#[tokio::test]
async fn restart_paused_active_work_requires_inspection_without_resending() {
    let (_tmp, state, root) = fixture().await;
    let mut f = work(draft("F-scores"), fake_workspace());
    f.state = FeatureState::Working;
    f.tasks.get_mut("T-ui").unwrap().state = TaskState::Running;
    change(&state, &root, |p| {
        p.enabled = true;
        p.state = RunState::Paused;
        p.features = vec![f];
        Ok(())
    })
    .unwrap();
    let recovered = ProjectWorkService::new(state.persistence.clone())
        .load(&root)
        .unwrap();
    assert_eq!(recovered.state, RunState::Interrupted);
    assert_eq!(recovered.features[0].state, FeatureState::NeedsInput);
    assert_eq!(
        recovered.features[0].tasks["T-ui"].state,
        TaskState::Interrupted
    );
    assert!(next_job(&recovered, &HashSet::new()).is_none());
}
#[tokio::test]
async fn actual_check_exit_output_and_stop_are_observed() {
    let (_tmp, state, root) = fixture().await;
    change(&state, &root, |p| {
        p.enabled = true;
        p.state = RunState::Running;
        Ok(())
    })
    .unwrap();
    let result = integration::check(
        &state,
        &root,
        &root,
        &CheckCommand {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo failed-check; exit 7".into()],
        },
    )
    .await
    .unwrap();
    assert_eq!(result.exit_code, Some(7));
    assert!(result.output.contains("failed-check"));
    let check = tokio::spawn({
        let state = state.clone();
        let root = root.clone();
        async move {
            integration::check(
                &state,
                &root,
                &root,
                &CheckCommand {
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), "sleep 30".into()],
                },
            )
            .await
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    command(state.clone(), root.clone(), "pause").await.unwrap();
    assert_eq!(load(&state, &root).unwrap().state, RunState::Paused);
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        command(state.clone(), root.clone(), "stop"),
    )
    .await
    .unwrap()
    .unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(3), check)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_err() || result.unwrap().exit_code != Some(0));
    assert!(state
        .project_work
        .checks
        .lock()
        .unwrap()
        .get(&root)
        .unwrap()
        .is_empty());
}
#[tokio::test]
async fn feedback_cannot_race_checking_and_stale_approval_is_rejected() {
    let (_tmp, state, root) = fixture().await;
    let f = reviewed(&state, &root).await;
    assert!(
        approve(state.clone(), root.clone(), f.id.clone(), "stale".into())
            .await
            .is_err()
    );
    change(&state, &root, |p| {
        p.features[0].state = FeatureState::Checking;
        Ok(())
    })
    .unwrap();
    assert!(
        keep_working(state.clone(), root.clone(), f.id, "change scores".into())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn correlated_turn_finishes_without_treating_silence_as_completion() {
    let (_tmp, state, root) = fixture().await;
    change(&state, &root, |p| {
        p.enabled = true;
        p.state = RunState::Running;
        Ok(())
    })
    .unwrap();
    let w = integration::create_workspace(&state, &root, "main", "turn-test")
        .await
        .unwrap();
    let sid = uuid::Uuid::new_v4();
    execution::spawn(&state, &root, &w, sid, &assignment(), false)
        .await
        .unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        execution::turn(
            &state,
            &root,
            sid,
            &assignment(),
            "Exercise an offline turn.",
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(result.0.contains("mock"));
    assert!(!result.1.is_empty());
    assert_eq!(
        state
            .registry
            .get_snapshot(sid)
            .unwrap()
            .metadata
            .approval_mode,
        grok_control_core::ApprovalMode::Ask
    );
    state.registry.retire_session(sid).await.unwrap();
}

#[tokio::test]
async fn successive_saved_plans_share_their_document_history() {
    let (_tmp, state, root) = fixture().await;
    enable(&state, &root, true).await.unwrap();
    for id in ["F-first", "F-second"] {
        let mut p = proposal();
        p.features[0].id = id.into();
        let expected = serde_json::to_string(&p).unwrap();
        update_draft(&state, &root, p).unwrap();
        accept_plan(state.clone(), root.clone(), expected, false)
            .await
            .unwrap();
    }
    let p = load(&state, &root).unwrap();
    run_git(
        Path::new(&root),
        &[
            "merge-base",
            "--is-ancestor",
            &p.features[0].seed_head,
            &p.features[1].seed_head,
        ],
    )
    .await
    .unwrap();
    assert!(Path::new(&p.features[1].seed.path)
        .join("plan/features/F-first.md")
        .exists());
    assert!(!Path::new(&root).join("plan").exists());
}
#[tokio::test]
async fn combined_candidate_contains_both_workers_and_requires_valid_review_evidence() {
    let (_tmp, state, root) = fixture().await;
    let mut d = draft("F-combined");
    d.tasks.truncate(2);
    let seed = integration::create_workspace(&state, &root, "main", "plan")
        .await
        .unwrap();
    let document = features::encode(&d.feature()).unwrap();
    let record = features::documents::feature_path(Path::new(&seed.path), &d.id).unwrap();
    features::documents::write_atomic(&record, &document).unwrap();
    let seed_head = integration::checkpoint(&state, &seed, "Plan")
        .await
        .unwrap();
    let mut f = work(d, seed);
    f.seed_head = seed_head.clone();
    for (id, file) in [("T-ui", "ui.txt"), ("T-api", "api.txt")] {
        let w = integration::create_workspace(&state, &root, &seed_head, "worker")
            .await
            .unwrap();
        std::fs::write(Path::new(&w.path).join(file), file).unwrap();
        let head = integration::checkpoint(&state, &w, "Task").await.unwrap();
        let t = f.tasks.get_mut(id).unwrap();
        t.workspace = Some(w);
        t.head = Some(head);
        t.state = TaskState::Checkpointed;
    }
    change(&state, &root, |p| {
        p.enabled = true;
        p.state = RunState::Running;
        p.policy.max_repairs = 0;
        p.features = vec![f];
        Ok(())
    })
    .unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        integration::candidate(state.clone(), root.clone(), "F-combined".into()),
    )
    .await
    .unwrap();
    assert!(result.is_err()); // The offline mock is not a structured review.
    let p = load(&state, &root).unwrap();
    let f = &p.features[0];
    let w = f.candidate.as_ref().unwrap();
    assert!(Path::new(&w.path).join("ui.txt").exists());
    assert!(Path::new(&w.path).join("api.txt").exists());
    assert!(Path::new(&w.path)
        .join("plan/results/F-combined.md")
        .exists());
    assert_eq!(f.checks[0].exit_code, Some(0));
    assert!(f.review.is_none());
    assert!(f.approved_head.is_none());
    assert!(!Path::new(&root).join("ui.txt").exists());
    let checked = f.checked_head.clone();
    let check_time = f.checks[0].at.clone();
    let attempts = f.review_history.len();
    assert!(
        integration::candidate(state.clone(), root.clone(), "F-combined".into())
            .await
            .is_err()
    );
    let p = load(&state, &root).unwrap();
    let f = &p.features[0];
    assert_eq!(f.checked_head, checked);
    assert_eq!(
        f.checks[0].at, check_time,
        "retrying unchanged review must preserve passing checks"
    );
    assert_eq!(f.review_history.len(), attempts + 1);
    assert_eq!(
        f.repairs, 0,
        "a malformed review is not an implementation defect"
    );
}
#[tokio::test]
async fn final_project_checks_gate_completion_at_the_integrated_commit() {
    let (_tmp, state, root) = fixture().await;
    let f = reviewed(&state, &root).await;
    integration::land(&state, &root, &f.id).await.unwrap();
    execution::final_check(&state, &root).await.unwrap();
    let p = load(&state, &root).unwrap();
    assert_eq!(p.state, RunState::Complete);
    assert_eq!(
        p.completed_head,
        Some(integration::head(&root).await.unwrap())
    );
    assert_eq!(p.final_checks[0].exit_code, Some(0));
    change(&state, &root, |p| {
        p.state = RunState::Running;
        let mut spec = p.features[0].feature()?;
        spec.verification.as_mut().unwrap().checks = vec![CheckCommand {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo final-failed; exit 9".into()],
        }];
        p.features[0].document = features::encode(&spec)?;
        Ok(())
    })
    .unwrap();
    assert!(execution::final_check(&state, &root).await.is_err());
    let p = load(&state, &root).unwrap();
    assert_ne!(p.state, RunState::Complete);
    assert_eq!(p.final_checks[0].exit_code, Some(9));
    assert!(p.final_checks[0].output.contains("final-failed"));
}

#[tokio::test]
async fn dependency_conflict_is_preserved_and_feedback_allows_scoped_repair() {
    let (_tmp, state, root) = fixture().await;
    let mut inputs = Vec::new();
    for label in ["frontend", "backend"] {
        let w = integration::create_workspace(&state, &root, "main", label)
            .await
            .unwrap();
        std::fs::write(Path::new(&w.path).join("shared.txt"), label).unwrap();
        inputs.push(
            integration::checkpoint(&state, &w, "Shared interface")
                .await
                .unwrap(),
        );
    }
    let w = integration::create_workspace(&state, &root, "main", "wiring")
        .await
        .unwrap();
    assert!(execution::combine_inputs(&state, &w, &inputs, false)
        .await
        .is_err());
    run_git(Path::new(&w.path), &["rev-parse", "--verify", "MERGE_HEAD"])
        .await
        .unwrap();
    assert!(execution::combine_inputs(&state, &w, &inputs, true)
        .await
        .unwrap()
        .is_some());
    std::fs::write(
        Path::new(&w.path).join("shared.txt"),
        "Combined frontend and backend contract",
    )
    .unwrap();
    run_git(Path::new(&w.path), &["add", "shared.txt"])
        .await
        .unwrap();
    let head = integration::checkpoint(&state, &w, "Resolve approved inputs")
        .await
        .unwrap();
    for input in &inputs {
        run_git(
            Path::new(&w.path),
            &["merge-base", "--is-ancestor", input, &head],
        )
        .await
        .unwrap();
    }
    assert!(execution::combine_inputs(&state, &w, &inputs, false)
        .await
        .unwrap()
        .is_none());
    assert!(!Path::new(&root).join("shared.txt").exists());
}

#[test]
fn conversational_controls_do_not_consume_feature_requests() {
    assert_eq!(project_control("Please pause."), Some("pause"));
    assert_eq!(project_control("status?"), Some("status"));
    assert_eq!(project_control("stop"), Some("stop"));
    assert_eq!(project_control("add a pause button"), None);
    assert_eq!(
        project_control("resume leaderboard after fixing scores"),
        None
    );
}

#[tokio::test]
async fn merge_retry_retains_exact_approval_after_checkout_cleanup() {
    let (_tmp, state, root) = fixture().await;
    let scratch = Path::new(&root).join("local-draft.txt");
    std::fs::write(&scratch, "saved draft").unwrap();
    state
        .worktrees
        .commit_all(Path::new(&root), "Save existing draft")
        .await
        .unwrap();
    let f = reviewed(&state, &root).await;
    let reviewed_head = f.approved_head.clone().unwrap();
    std::fs::write(&scratch, "user draft").unwrap();
    let error = integration::land(&state, &root, &f.id).await.unwrap_err();
    change(&state, &root, |p| {
        p.features[0].state = FeatureState::NeedsInput;
        p.features[0].blocker = Some(Blocker::new(WorkStage::Merge, error, None));
        Ok(())
    })
    .unwrap();
    let blocked = load(&state, &root).unwrap();
    assert!(blocked
        .waiting_reason(&blocked.features[0])
        .contains("local edits"));
    let expected = blocked.features[0].decision_key();
    std::fs::write(scratch, "saved draft").unwrap();
    // Reserve the test's driver so retry can be inspected before dispatch.
    state
        .project_work
        .driving
        .lock()
        .unwrap()
        .insert(root.clone());
    retry_stage(
        state.clone(),
        root.clone(),
        f.id.clone(),
        expected.clone(),
        None,
    )
    .await
    .unwrap();
    let current = load(&state, &root).unwrap();
    assert_eq!(
        current.features[0].approved_head.as_deref(),
        Some(reviewed_head.as_str())
    );
    assert_eq!(
        current.features[0].review.as_ref().unwrap().candidate,
        reviewed_head
    );
    assert!(
        retry_stage(state.clone(), root.clone(), f.id.clone(), expected, None)
            .await
            .is_err()
    );
    integration::land(&state, &root, &f.id).await.unwrap();
    assert_eq!(integration::head(&root).await.unwrap(), reviewed_head);
    assert_eq!(
        load(&state, &root).unwrap().features[0].state,
        FeatureState::Done
    );
}

#[tokio::test]
async fn interrupted_repair_retains_scope_and_can_replace_its_writer() {
    let (_tmp, state, root) = fixture().await;
    let mut f = work(draft("F-scores"), fake_workspace());
    f.state = FeatureState::Working;
    f.pending_repair = Some("Fix the score tie ordering".into());
    f.tasks
        .values_mut()
        .for_each(|t| t.state = TaskState::Checkpointed);
    change(&state, &root, |p| {
        p.enabled = true;
        p.state = RunState::Running;
        p.features = vec![f];
        Ok(())
    })
    .unwrap();
    let restored = ProjectWorkService::new(state.persistence.clone())
        .load(&root)
        .unwrap();
    let f = &restored.features[0];
    assert_eq!(f.blocker.as_ref().unwrap().stage, WorkStage::Repair);
    assert_eq!(
        f.pending_repair.as_deref(),
        Some("Fix the score tie ordering")
    );
    assert!(f.tasks.values().all(|t| t.state == TaskState::Checkpointed));
    change(&state, &root, |p| {
        *p = restored;
        Ok(())
    })
    .unwrap();
    let replacement = Assignment {
        model: "replacement".into(),
        ..assignment()
    };
    replace_assignment(&state, &root, "F-scores", "repair", replacement.clone()).unwrap();
    let f = load(&state, &root).unwrap().features[0].clone();
    assert_eq!(f.repair_assignment().unwrap().model, replacement.model);
    state
        .project_work
        .driving
        .lock()
        .unwrap()
        .insert(root.clone());
    retry_stage(
        state.clone(),
        root.clone(),
        f.id.clone(),
        f.decision_key(),
        None,
    )
    .await
    .unwrap();
    let f = load(&state, &root).unwrap().features[0].clone();
    assert_eq!(
        f.pending_repair.as_deref(),
        Some("Fix the score tie ordering")
    );
    assert_eq!(f.state, FeatureState::Checking);
    assert!(f.tasks.values().all(|t| t.state == TaskState::Checkpointed));
}
