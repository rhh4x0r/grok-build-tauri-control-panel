//! Foundry's portable authoring model and deterministic, framework-free run reducer.
pub mod contract;
pub mod intake;
pub use intake::*;
pub mod graph;
pub mod run;
pub mod store;
pub use contract::*;
pub use graph::*;
pub use run::*;
pub use store::*;
pub fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_templates_validate_and_export_loops() {
        for name in TEMPLATES {
            let g = template(name);
            g.validate(true).unwrap();
            let text = g.markdown().unwrap();
            assert!(text.contains("## Loops"));
            for e in g.edges.iter().filter(|e| e.kind == "loop") {
                assert!(text.contains(&format!("{} → {}", e.source, e.target)));
            }
            let json = serde_json::to_string(&g).unwrap();
            assert_eq!(serde_json::from_str::<SkillGraph>(&json).unwrap(), g);
        }
    }
    #[test]
    fn position_is_not_execution_order() {
        let mut g = template(TEMPLATES[0]);
        let first = g.entry_node_id.clone();
        g.nodes[0].position.y = 999.;
        assert_eq!(g.ordered()[0].id, first);
        g.move_stage(&first, 1);
        assert_ne!(g.entry_node_id, first);
        g.edges.retain(|e| e.kind != "loop");
        g.validate(true).unwrap();
    }
    fn run() -> Run {
        Run::new(
            Document::new("Research a topic"),
            "/tmp".into(),
            "grok".into(),
            "actual-model".into(),
            "ask".into(),
        )
        .unwrap()
    }
    fn passed() -> StageResult {
        StageResult {
            outcome: "passed".into(),
            summary: "Evidence recorded".into(),
            criteria: vec![CriterionResult {
                criterion: 0,
                status: "passed".into(),
                evidence: vec!["artifact.md".into()],
            }],
            artifacts: vec!["artifact.md".into()],
        }
    }
    #[test]
    fn gates_are_real_and_duplicate_results_rejected() {
        let mut r = run();
        while r.current().is_some_and(|n| n.kind != "gate") {
            let a = r.prepare().unwrap().unwrap();
            r.finish(&a.id, Ok(passed())).unwrap();
            assert!(r.finish(&a.id, Ok(passed())).is_err());
        }
        assert!(r.prepare().unwrap().is_none());
        assert_eq!(r.status, RunStatus::WaitingGate);
        assert!(r.approve(99).is_err());
        r.approve(0).unwrap();
        assert_eq!(r.status, RunStatus::Completed);
    }
    #[test]
    fn return_invalidates_acceptance_and_is_bounded() {
        let mut r = run();
        let a = r.prepare().unwrap().unwrap();
        r.finish(&a.id, Ok(passed())).unwrap();
        let a = r.prepare().unwrap().unwrap();
        let mut result = passed();
        result.outcome = "needs_revision".into();
        r.finish(&a.id, Ok(result)).unwrap();
        assert_eq!(r.cursor, 0);
        assert!(r.accepted.is_empty());
        assert!(r.attempts.iter().all(|a| a.invalidated));
        r.document.policy.max_attempts = 2;
        assert!(r.prepare().unwrap().is_none());
        assert_eq!(r.status, RunStatus::Paused);
    }
    #[test]
    fn claimed_success_requires_each_criterion() {
        assert!(
            StageResult::parse(r#"{"outcome":"passed","summary":"done","criteria":[]}"#, 1)
                .is_err()
        );
        assert!(StageResult::parse("done", 0).is_err());
    }
    #[test]
    fn contract_seed_does_not_invent_loops() {
        let c = draft(
            "Build thing",
            "full-project",
            "implementation",
            "openai-codex",
        );
        c.validate().unwrap();
        let g = c.graph();
        g.validate(true).unwrap();
        assert!(g.edges.iter().all(|e| e.kind == "sequence"));
    }
    #[test]
    fn store_revisions_and_crash_recovery() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("foundry.db");
        let store = Store::open(&p).unwrap();
        let mut d = Document::new("Task");
        store.save(&mut d).unwrap();
        let mut stale = d.clone();
        d.name = "Updated".into();
        store.save(&mut d).unwrap();
        assert!(store.save(&mut stale).is_err());
        assert_eq!(store.revisions(&d.id).unwrap().len(), 2);
        let mut r = run();
        r.prepare().unwrap();
        store.save_run(&r).unwrap();
        drop(store);
        let s = Store::open(&p).unwrap();
        assert_eq!(s.runs().unwrap()[0].status, RunStatus::Interrupted);
    }
}

#[cfg(test)]
mod acceptance_tests {
    use super::*;
    fn pass() -> StageResult {
        StageResult {
            outcome: "passed".into(),
            summary: "Observed artifact supports the criterion".into(),
            criteria: vec![CriterionResult {
                criterion: 0,
                status: "passed".into(),
                evidence: vec!["report.md:1".into()],
            }],
            artifacts: vec!["report.md".into()],
        }
    }
    fn new_run() -> Run {
        Run::new(
            Document::new("Compare implementation options"),
            "/tmp".into(),
            "grok".into(),
            "advertised-model".into(),
            "ask".into(),
        )
        .unwrap()
    }
    #[test]
    fn mock_research_revision_and_independent_review_complete_at_human_gate() {
        let mut r = new_run();
        r.document.bindings.insert(
            "stage-3".into(),
            StageBinding {
                backend: Some("claude".into()),
                model: Some("sonnet".into()),
                return_edge: None,
            },
        );
        let mut returned = false;
        while r.current().is_some_and(|n| n.kind != "gate") {
            let a = r.prepare().unwrap().unwrap();
            let mut result = pass();
            if a.node_id == "stage-1" && !returned {
                result.outcome = "needs_revision".into();
                returned = true;
            }
            if a.node_id == "stage-3" {
                assert_eq!(a.backend, "claude");
                assert_eq!(a.model, "sonnet");
                assert!(!r
                    .prompt()
                    .contains("Observed artifact supports the criterion"));
                assert!(r.prompt().contains("report.md"));
            }
            r.finish(&a.id, Ok(result)).unwrap();
        }
        r.prepare().unwrap();
        assert_eq!(r.status, RunStatus::WaitingGate);
        assert_eq!(r.attempts.len(), 7);
        let token = r.gate_token();
        assert!(r.approve_gate("stale").is_err());
        r.approve_gate(&token).unwrap();
        assert_eq!(r.status, RunStatus::Completed);
    }
    #[test]
    fn pause_does_not_dispatch_and_stop_discards_completion() {
        let mut r = new_run();
        let a = r.prepare().unwrap().unwrap();
        r.status = RunStatus::Paused;
        r.finish(&a.id, Ok(pass())).unwrap();
        assert!(r.prepare().unwrap().is_none());
        assert_eq!(r.cursor, 1);
        r.status = RunStatus::Ready;
        let a = r.prepare().unwrap().unwrap();
        r.status = RunStatus::Stopped;
        r.finish(&a.id, Ok(pass())).unwrap();
        assert_eq!(r.cursor, 1);
        assert_eq!(r.status, RunStatus::Stopped);
    }
    #[test]
    fn adjacent_human_gates_require_distinct_receipts() {
        let mut r = new_run();
        for n in &mut r.document.graph.nodes {
            n.kind = "gate".into();
        }
        r.prepare().unwrap();
        let token = r.gate_token();
        r.approve_gate(&token).unwrap();
        r.prepare().unwrap();
        assert!(r.approve_gate(&token).is_err());
    }
    #[test]
    fn imports_reject_unknown_versions_and_export_does_not_overwrite() {
        let d = Document::new("Task");
        let json = serde_json::to_string(&d).unwrap();
        let restored = Document::import(&json).unwrap();
        assert_eq!(restored.graph, d.graph);
        assert_ne!(restored.id, d.id);
        assert!(Document::import(r#"{"schemaVersion":"99"}"#).is_err());
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("export");
        export_package(&d, &target).unwrap();
        assert!(target.join("SKILL.md").is_file());
        assert!(target.join("skill-graph.json").is_file());
        assert!(export_package(&d, &target).is_err());
    }
    #[test]
    fn invalid_dependencies_and_ambiguous_returns_fail_preflight() {
        let mut d = Document::new("Task");
        d.graph.edges.push(SkillEdge {
            id: crate::id(),
            kind: "depends-on".into(),
            source: "stage-0".into(),
            target: "stage-5".into(),
            label: None,
            purpose: None,
        });
        assert!(Run::new(
            d,
            "/tmp".into(),
            "grok".into(),
            "model".into(),
            "ask".into()
        )
        .is_err());
        let mut d = Document::new("Task");
        d.graph.edges.push(SkillEdge {
            id: crate::id(),
            kind: "loop".into(),
            source: "stage-3".into(),
            target: "stage-0".into(),
            label: None,
            purpose: Some("review".into()),
        });
        assert!(Run::new(
            d,
            "/tmp".into(),
            "grok".into(),
            "model".into(),
            "ask".into()
        )
        .is_err());
    }
}
