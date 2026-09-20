use super::super::features::{self, Assignment, Feature, FeatureTask, Verification};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use uuid::Uuid;

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}
pub fn id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub role: String,
    pub text: String,
    pub at: String,
    #[serde(default)]
    pub features: Vec<String>,
}
impl Message {
    pub fn new(role: &str, text: impl Into<String>) -> Self {
        Self {
            id: id("M"),
            role: role.into(),
            text: text.into(),
            at: now(),
            features: vec![],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum RunState {
    #[default]
    Idle,
    Running,
    Paused,
    Stopped,
    Interrupted,
    Complete,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RunPolicy {
    pub concurrency: usize,
    pub auto_merge: bool,
    pub max_repairs: usize,
    pub target: String,
}
impl Default for RunPolicy {
    fn default() -> Self {
        Self {
            concurrency: 2,
            auto_merge: false,
            max_repairs: 1,
            target: "main".into(),
        }
    }
}
impl RunPolicy {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=4).contains(&self.concurrency) || self.max_repairs > 3 {
            return Err(
                "Choose 1–4 parallel agents and at most 3 automatic repair attempts.".into(),
            );
        }
        if self.target.trim().is_empty()
            || self.target.starts_with('-')
            || self.target.contains(['\n', '\r', ' '])
        {
            return Err("Choose a local target branch.".into());
        }
        Ok(())
    }
}

/// Explicit planner DTO: feature document serialization intentionally skips
/// prose fields, so raw Feature JSON is not an adequate model output contract.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Proposal {
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub questions: Vec<String>,
    #[serde(default)]
    pub features: Vec<FeatureDraft>,
    #[serde(default)]
    pub updates: Vec<WorkUpdate>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkUpdate {
    pub feature_id: String,
    pub feedback: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FeatureDraft {
    pub id: String,
    pub title: String,
    pub brief: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub tasks: Vec<TaskDraft>,
    pub verification: Verification,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskDraft {
    pub id: String,
    pub title: String,
    pub brief: String,
    pub role: String,
    pub assignment: Assignment,
    #[serde(default)]
    pub waits_for: Vec<String>,
}
impl FeatureDraft {
    pub fn feature(&self) -> Feature {
        Feature {
            id: self.id.clone(),
            title: self.title.clone(),
            brief: self.brief.clone(),
            depends_on: self.depends_on.clone(),
            verification: Some(self.verification.clone()),
            tasks: self
                .tasks
                .iter()
                .map(|t| FeatureTask {
                    id: t.id.clone(),
                    title: t.title.clone(),
                    brief: t.brief.clone(),
                    role: t.role.clone(),
                    assignment: Some(t.assignment.clone()),
                    waits_for: t.waits_for.clone(),
                    review_of: None,
                })
                .collect(),
        }
    }
}
impl Proposal {
    pub fn parse(text: &str) -> Result<Self, String> {
        let (_, tail) = text.rsplit_once("<project-plan>").ok_or("The planner did not return a project plan. Its response is available in the planning thread.")?;
        let (json, _) = tail
            .split_once("</project-plan>")
            .ok_or("The planner's project plan was incomplete.")?;
        if json.len() > 512_000 {
            return Err("The proposed plan is too large. Split it into smaller requests.".into());
        }
        serde_json::from_str(json).map_err(|e| format!("The proposed plan could not be read: {e}"))
    }
    pub fn validate(&self, existing: &[FeatureWork]) -> Result<(), String> {
        if self.features.len() > 16 || self.questions.len() > 6 || self.updates.len() > 16 {
            return Err("Keep one plan update to 16 features and at most 6 questions.".into());
        }
        let mut ids: HashSet<_> = existing.iter().map(|f| f.id.clone()).collect();
        for f in &self.features {
            if !ids.insert(f.id.clone()) {
                return Err(format!("Feature ID {} is already in use.", f.id));
            }
            features::validate(&f.feature())?;
            if f.tasks.is_empty()
                || f.tasks
                    .iter()
                    .any(|t| t.role == "Review" || t.brief.trim().is_empty())
            {
                return Err("Each feature needs implementation tasks with briefs; choose its independent reviewer separately.".into());
            }
            let v = &f.verification;
            if v.criteria.is_empty()
                || v.criteria.len() > 16
                || v.criteria.iter().any(|c| c.trim().is_empty())
                || v.test_steps.trim().is_empty()
            {
                return Err(
                    "Each feature needs acceptance criteria and instructions for testing it."
                        .into(),
                );
            }
            features::validate_assignment(&v.reviewer)?;
            if v.checks.is_empty() || v.checks.len() > 8 {
                return Err("Each feature needs 1–8 explicit verification commands.".into());
            }
            for c in &v.checks {
                if c.program.trim().is_empty()
                    || c.program.starts_with('-')
                    || c.program.contains(['\n', '\r', '\0'])
                    || c.args.len() > 64
                    || c.args.iter().any(|a| a.len() > 16000 || a.contains('\0'))
                {
                    return Err(
                        "Verification commands must contain a program and valid argument list."
                            .into(),
                    );
                }
            }
        }
        let mut graph: BTreeMap<String, Vec<String>> = existing
            .iter()
            .map(|f| Ok((f.id.clone(), f.feature()?.depends_on)))
            .collect::<Result<_, String>>()?;
        for f in &self.features {
            graph.insert(f.id.clone(), f.depends_on.clone());
        }
        for (id, deps) in &graph {
            if deps.iter().any(|d| !ids.contains(d)) {
                return Err(format!("Unknown dependency for {id}."));
            }
            let mut stack = deps.clone();
            let mut seen = HashSet::new();
            while let Some(dep) = stack.pop() {
                if &dep == id {
                    return Err("Features cannot have circular dependencies.".into());
                }
                if seen.insert(dep.clone()) {
                    stack.extend(graph.get(&dep).cloned().unwrap_or_default());
                }
            }
        }
        for update in &self.updates {
            if !existing.iter().any(|f| f.id == update.feature_id)
                || update.feedback.trim().is_empty()
            {
                return Err(
                    "Feedback must name an existing feature and describe the requested change."
                        .into(),
                );
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskState {
    #[default]
    Queued,
    Running,
    Checkpointed,
    NeedsInput,
    Interrupted,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TaskWork {
    pub state: TaskState,
    pub workspace: Option<Workspace>,
    pub head: Option<String>,
    pub summary: String,
    pub note: String,
    pub attempts: usize,
    pub applied: Option<Assignment>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub path: String,
    pub branch: String,
    pub session: Option<Uuid>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum FeatureState {
    Idea,
    #[default]
    Queued,
    Working,
    Checking,
    Reviewing,
    Review,
    NeedsInput,
    Landing,
    Done,
}
impl FeatureState {
    pub fn column(&self) -> &'static str {
        match self {
            Self::Idea => "Ideas",
            Self::Queued => "Queued",
            Self::Review => "Review",
            Self::Done => "Done",
            _ => "Working",
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Self::Idea => "Ideas",
            Self::Queued => "Queued",
            Self::Working => "Building",
            Self::Checking => "Checking",
            Self::Reviewing => "AI review",
            Self::Review => "Ready for your review",
            Self::NeedsInput => "Needs you",
            Self::Landing => "Merging",
            Self::Done => "Merged",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckEvidence {
    pub command: features::CheckCommand,
    pub exit_code: Option<i64>,
    pub output: String,
    pub at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewEvidence {
    pub candidate: String,
    pub base: String,
    pub document: String,
    pub summary: String,
    pub criteria: Vec<bomb_foundry::CriterionResult>,
    pub checks: Vec<CheckEvidence>,
    pub reviewer: Assignment,
    pub thread: Uuid,
    pub at: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FeatureWork {
    pub id: String,
    /// The immutable, approved portable record (including prose).
    pub document: String,
    pub seed: Workspace,
    pub seed_head: String,
    pub state: FeatureState,
    pub tasks: BTreeMap<String, TaskWork>,
    pub candidate: Option<Workspace>,
    pub candidate_base: Option<String>,
    pub review: Option<ReviewEvidence>,
    #[serde(default)]
    pub checks: Vec<CheckEvidence>,
    pub approved_head: Option<String>,
    pub integrated_head: Option<String>,
    pub note: String,
    pub feedback: Vec<String>,
    pub repairs: usize,
    pub reviewer_thread: Option<Uuid>,
}
impl FeatureWork {
    pub fn feature(&self) -> Result<Feature, String> {
        features::decode(&self.document)
    }
    pub fn title(&self) -> String {
        self.feature()
            .map(|f| f.title)
            .unwrap_or_else(|_| self.id.clone())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectWork {
    pub enabled: bool,
    pub state: RunState,
    pub policy: RunPolicy,
    pub messages: Vec<Message>,
    pub draft: Option<Proposal>,
    pub features: Vec<FeatureWork>,
    pub planning: bool,
    pub planner_thread: Option<Uuid>,
    pub planner: Option<Assignment>,
    pub note: String,
    pub revision: u64,
    pub completed_head: Option<String>,
    pub final_checks: Vec<CheckEvidence>,
    pub final_workspace: Option<Workspace>,
}
