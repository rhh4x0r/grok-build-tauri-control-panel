use crate::Document;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RunPolicy {
    pub max_edge_returns: u32,
    pub max_attempts: usize,
}
impl Default for RunPolicy {
    fn default() -> Self {
        Self {
            max_edge_returns: 2,
            max_attempts: 0,
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StageBinding {
    pub backend: Option<String>,
    pub model: Option<String>,
    pub return_edge: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Ready,
    Running,
    Paused,
    WaitingGate,
    Blocked,
    Completed,
    Stopped,
    Interrupted,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CriterionResult {
    pub criterion: usize,
    pub status: String,
    pub evidence: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageResult {
    pub outcome: String,
    pub summary: String,
    pub criteria: Vec<CriterionResult>,
    #[serde(default)]
    pub artifacts: Vec<String>,
}
impl StageResult {
    pub fn parse(text: &str, count: usize) -> Result<Self, String> {
        let raw = text
            .rsplit_once("<foundry-result>")
            .map(|(_, s)| s.split("</foundry-result>").next().unwrap_or(s))
            .unwrap_or(text)
            .trim();
        let r: Self = serde_json::from_str(raw).map_err(|_| {
            "Missing or malformed <foundry-result> JSON; review output before retrying".to_string()
        })?;
        if !["passed", "needs_revision", "blocked"].contains(&r.outcome.as_str())
            || r.summary.trim().is_empty()
        {
            return Err("Invalid stage outcome".into());
        }
        if r.outcome == "passed" {
            if r.criteria.len() != count {
                return Err("Every exit criterion needs evidence".into());
            }
            let mut seen = std::collections::HashSet::new();
            for c in &r.criteria {
                if c.criterion >= count
                    || !seen.insert(c.criterion)
                    || c.status != "passed"
                    || c.evidence.is_empty()
                    || c.evidence.iter().any(|s| s.trim().is_empty())
                {
                    return Err("Passed outcome lacks criterion evidence".into());
                }
            }
        }
        Ok(r)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub id: String,
    pub node_id: String,
    pub session_id: Option<String>,
    pub backend: String,
    pub model: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub output: String,
    pub observed_evidence: Vec<String>,
    pub result: Option<StageResult>,
    pub error: Option<String>,
    pub invalidated: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    #[serde(default = "crate::now")]
    pub created_at: String,
    pub id: String,
    pub document: Document,
    pub cwd: String,
    pub backend: String,
    pub model: String,
    pub approval_mode: String,
    pub status: RunStatus,
    pub cursor: usize,
    pub attempts: Vec<Attempt>,
    pub returns: HashMap<String, u32>,
    pub accepted: HashMap<String, String>,
    pub gates: Vec<serde_json::Value>,
    pub note: String,
    pub parent_thread: Option<String>,
}
impl Run {
    pub fn new(
        document: Document,
        cwd: String,
        backend: String,
        model: String,
        approval_mode: String,
    ) -> Result<Self, String> {
        document.contract.validate()?;
        document.graph.validate(true)?;
        for binding in document.bindings.values() {
            if binding.backend.as_ref().is_some_and(|b| b != &backend)
                && binding.model.as_ref().is_none_or(|m| m.trim().is_empty())
            {
                return Err("Choose a model for each provider override".into());
            }
        }
        for n in &document.graph.nodes {
            let loops: Vec<_> = document
                .graph
                .edges
                .iter()
                .filter(|e| e.kind == "loop" && e.source == n.id)
                .collect();
            if loops.len() > 1
                && !document
                    .bindings
                    .get(&n.id)
                    .and_then(|b| b.return_edge.as_ref())
                    .is_some_and(|id| loops.iter().any(|e| &e.id == id))
            {
                return Err(format!("Choose the failure return link for {}", n.title));
            }
        }
        Ok(Self {
            created_at: crate::now(),
            id: crate::id(),
            document,
            cwd,
            backend,
            model,
            approval_mode,
            status: RunStatus::Ready,
            cursor: 0,
            attempts: vec![],
            returns: HashMap::new(),
            accepted: HashMap::new(),
            gates: vec![],
            note: String::new(),
            parent_thread: None,
        })
    }
    pub fn current(&self) -> Option<&crate::SkillNode> {
        self.document.graph.ordered().get(self.cursor).copied()
    }
    pub fn limit(&self) -> usize {
        if self.document.policy.max_attempts == 0 {
            self.document.graph.nodes.len() * 3
        } else {
            self.document.policy.max_attempts
        }
    }
    pub fn prepare(&mut self) -> Result<Option<Attempt>, String> {
        if !matches!(self.status, RunStatus::Ready | RunStatus::Running) {
            return Ok(None);
        }
        let Some(n) = self.current().cloned() else {
            self.status = RunStatus::Completed;
            self.note.clear();
            return Ok(None);
        };
        if n.kind == "gate" {
            self.status = RunStatus::WaitingGate;
            self.note = format!("Human approval required: {}", n.title);
            return Ok(None);
        }
        if self.attempts.len() >= self.limit() {
            self.status = RunStatus::Paused;
            self.note = "Attempt limit reached; explicitly extend limits to continue".into();
            return Ok(None);
        }
        for e in self
            .document
            .graph
            .edges
            .iter()
            .filter(|e| e.kind == "depends-on" && e.source == n.id)
        {
            if !self.accepted.contains_key(&e.target) {
                self.status = RunStatus::Blocked;
                return Err(format!("Dependency {} has no current acceptance", e.target));
            }
        }
        let b = self
            .document
            .bindings
            .get(&n.id)
            .cloned()
            .unwrap_or_default();
        let a = Attempt {
            id: crate::id(),
            node_id: n.id,
            session_id: None,
            backend: b.backend.unwrap_or_else(|| self.backend.clone()),
            model: b.model.unwrap_or_else(|| self.model.clone()),
            started_at: crate::now(),
            finished_at: None,
            output: String::new(),
            observed_evidence: vec![],
            result: None,
            error: None,
            invalidated: false,
        };
        self.status = RunStatus::Running;
        self.note.clear();
        self.attempts.push(a.clone());
        Ok(Some(a))
    }
    pub fn finish(&mut self, id: &str, result: Result<StageResult, String>) -> Result<(), String> {
        let Some(a) = self
            .attempts
            .last_mut()
            .filter(|a| a.id == id && a.finished_at.is_none())
        else {
            return Err("Stale or duplicate attempt completion".into());
        };
        a.finished_at = Some(crate::now());
        if self.status == RunStatus::Stopped {
            return Ok(());
        }
        let r = match result {
            Ok(r) => r,
            Err(e) => {
                a.error = Some(e.clone());
                self.status = RunStatus::Blocked;
                self.note = e;
                return Ok(());
            }
        };
        a.result = Some(r.clone());
        let node_id = a.node_id.clone();
        match r.outcome.as_str() {
            "passed" => {
                self.accepted.insert(node_id, id.into());
                self.cursor += 1;
            }
            "needs_revision" => {
                let binding = self
                    .document
                    .bindings
                    .get(&node_id)
                    .and_then(|b| b.return_edge.as_ref());
                let edge = self
                    .document
                    .graph
                    .edges
                    .iter()
                    .find(|e| {
                        e.kind == "loop"
                            && e.source == node_id
                            && binding.is_none_or(|id| id == &e.id)
                    })
                    .cloned();
                let Some(edge) = edge else {
                    self.status = RunStatus::Blocked;
                    self.note = "Revision requested but no return link is configured".into();
                    return Ok(());
                };
                let count = self.returns.entry(edge.id.clone()).or_default();
                if *count >= self.document.policy.max_edge_returns {
                    self.status = RunStatus::Paused;
                    self.note =
                        "Return limit reached; explicitly extend limits before retrying".into();
                    return Ok(());
                }
                *count += 1;
                self.cursor = self
                    .document
                    .graph
                    .ordered()
                    .iter()
                    .position(|n| n.id == edge.target)
                    .ok_or("Return target is missing")?;
                let invalid: Vec<_> = self
                    .document
                    .graph
                    .ordered()
                    .iter()
                    .skip(self.cursor)
                    .map(|n| n.id.clone())
                    .collect();
                self.accepted.retain(|id, _| !invalid.contains(id));
                for a in &mut self.attempts {
                    if invalid.contains(&a.node_id) {
                        a.invalidated = true;
                    }
                }
                self.gates
                    .retain(|g| !invalid.iter().any(|id| g["nodeId"] == *id));
            }
            _ => {
                self.status = RunStatus::Blocked;
                self.note = r.summary;
                return Ok(());
            }
        }
        if self.cursor >= self.document.graph.nodes.len() {
            self.status = RunStatus::Completed;
        } else if self.status != RunStatus::Paused {
            self.status = RunStatus::Ready;
        }
        Ok(())
    }
    pub fn gate_token(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.id,
            self.document.revision,
            self.cursor,
            self.attempts
                .last()
                .map(|a| a.id.as_str())
                .unwrap_or("initial")
        )
    }
    pub fn request_changes(&mut self, token: &str, feedback: &str) -> Result<(), String> {
        if self.status != RunStatus::WaitingGate || token != self.gate_token() {
            return Err("This review changed. Refresh before requesting changes.".into());
        }
        let feedback = feedback.trim();
        if feedback.is_empty() || feedback.len() > 20_000 {
            return Err("Describe the requested changes (up to 20,000 bytes).".into());
        }
        let ordered = self.document.graph.ordered();
        let target = ordered.iter().take(self.cursor).rev().find(|n| n.role == "draft" || n.role == "revise")
            .map(|n| n.id.clone()).ok_or("This workflow has no build stage to revise")?;
        let cursor = ordered.iter().position(|n| n.id == target).unwrap();
        let invalid: Vec<_> = ordered.iter().skip(cursor).map(|n| n.id.clone()).collect();
        self.accepted.retain(|id, _| !invalid.contains(id));
        for a in &mut self.attempts { if invalid.contains(&a.node_id) { a.invalidated = true; } }
        self.gates.retain(|g| !invalid.iter().any(|id| g["nodeId"] == *id));
        for node in self.document.graph.nodes.iter_mut().filter(|n| invalid.contains(&n.id) && n.kind != "gate") {
            node.prompt.push_str(&format!("\n\nUser requested changes after review:\n{feedback}\nAddress or independently verify this feedback within your stage role. Existing permission boundaries still apply."));
        }
        self.cursor = cursor;
        self.status = RunStatus::Ready;
        self.note = "Changes requested; returning to build and review".into();
        Ok(())
    }

    pub fn approve_gate(&mut self, token: &str) -> Result<(), String> {
        if token != self.gate_token() {
            return Err("This gate changed. Review the current evidence before approving.".into());
        }
        self.approve(self.document.revision)
    }
    pub fn approve(&mut self, revision: u64) -> Result<(), String> {
        if self.status != RunStatus::WaitingGate || revision != self.document.revision {
            return Err("Gate no longer matches this run revision".into());
        }
        let n = self.current().ok_or("Missing gate")?.id.clone();
        let receipt = crate::id();
        self.gates.push(serde_json::json!({"id":receipt,"nodeId":n,"revision":revision,"at":crate::now(),"attemptIds":self.accepted}));
        self.accepted.insert(n, receipt);
        self.note.clear();
        self.cursor += 1;
        self.status = if self.cursor == self.document.graph.nodes.len() {
            RunStatus::Completed
        } else {
            RunStatus::Ready
        };
        Ok(())
    }
    pub fn prompt(&self) -> String {
        let Some(n) = self.current() else {
            return String::new();
        };
        let independent = n.role == "independent-review";
        let handoff: Vec<_> = self
            .attempts
            .iter()
            .filter(|a| !a.invalidated)
            .filter_map(|a| a.result.as_ref().map(|r| (a, r)))
            .map(|(a, r)| {
                if independent {
                    serde_json::json!({"artifacts":r.artifacts,"appObservedToolEvents":a.observed_evidence})
                } else {
                    serde_json::to_value(r).unwrap()
                }
            })
            .collect();
        format!("You are executing one bounded Foundry stage. Do not execute other stages or approve human gates. Existing permissions remain in force.\n\n{}\n\nSTAGE: {}\n{}\n\nExit criteria (zero-based indices): {:?}\n\nExplicit handoff (agent-reported, verify before relying on it): {}\n\nIndependently inspect referenced artifacts. Never invent evidence. Finish with exactly one <foundry-result> JSON object </foundry-result> with fields outcome (passed, needs_revision, blocked), summary, artifacts (path strings), criteria (objects with criterion index, status passed/failed/unknown/NOT_RUN, and evidence string array). passed requires evidence for every criterion. Do not claim tests were run without tool evidence.",self.document.contract.markdown(),n.title,n.prompt,n.exit_criteria,serde_json::to_string(&handoff).unwrap())
    }
}
