use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkillGraph {
    pub schema_version: String,
    pub id: String,
    pub name: String,
    pub description: String,
    pub when_to_use: String,
    pub nodes: Vec<SkillNode>,
    pub edges: Vec<SkillEdge>,
    pub entry_node_id: String,
    #[serde(default)]
    pub limitations: Vec<String>,
    #[serde(default)]
    pub becomes_wrong_if: Vec<String>,
    #[serde(default)]
    pub confidence: Option<String>,
    pub authority_text: Value,
    pub generation_metadata: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SkillNode {
    pub id: String,
    pub kind: String,
    pub role: String,
    pub title: String,
    pub prompt: String,
    pub purpose: String,
    pub exit_criteria: Vec<String>,
    pub sequence_index: usize,
    pub position: Position,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Position {
    pub x: f32,
    pub y: f32,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SkillEdge {
    pub id: String,
    pub kind: String,
    pub source: String,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
}
impl SkillGraph {
    pub fn ordered(&self) -> Vec<&SkillNode> {
        let mut nodes: Vec<_> = self.nodes.iter().collect();
        nodes.sort_by_key(|n| n.sequence_index);
        nodes
    }
    pub fn validate(&self, runnable: bool) -> Result<(), String> {
        if self.schema_version != "0.1.0" {
            return Err("Unsupported SkillGraph version".into());
        }
        if self.nodes.is_empty() || self.nodes.len() > 80 || self.edges.len() > 200 {
            return Err("A graph needs 1–80 stages and at most 200 links".into());
        }
        if self.name.trim().is_empty() {
            return Err("Name the skill".into());
        }
        let mut ids = HashSet::new();
        let mut ranks = HashSet::new();
        for n in &self.nodes {
            if n.id.is_empty() || !ids.insert(n.id.as_str()) || !ranks.insert(n.sequence_index) {
                return Err("Duplicate or missing stage ID/order".into());
            }
            if !["stage", "review", "gate"].contains(&n.kind.as_str())
                || ![
                    "intake",
                    "research",
                    "draft",
                    "revise",
                    "independent-review",
                    "verify",
                    "other",
                ]
                .contains(&n.role.as_str())
            {
                return Err("Unknown stage kind or role".into());
            }
            if !n.position.x.is_finite() || !n.position.y.is_finite() {
                return Err("Invalid stage position".into());
            }
            if n.title.trim().is_empty()
                || n.purpose.trim().is_empty()
                || (runnable && n.prompt.trim().is_empty())
            {
                return Err(format!(
                    "Stage {} needs a title, purpose and prompt",
                    n.title
                ));
            }
        }
        if !ids.contains(self.entry_node_id.as_str()) {
            return Err("Entry stage is missing".into());
        }
        let ranks: HashMap<_, _> = self
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n.sequence_index))
            .collect();
        let mut edge_ids = HashSet::new();
        let mut links = HashSet::new();
        for e in &self.edges {
            if !edge_ids.insert(&e.id)
                || !links.insert((&e.kind, &e.source, &e.target))
                || e.source == e.target
                || !ids.contains(e.source.as_str())
                || !ids.contains(e.target.as_str())
            {
                return Err("Duplicate or dangling linkage".into());
            }
            let from = ranks[e.source.as_str()];
            let to = ranks[e.target.as_str()];
            match e.kind.as_str() {
                "sequence" if from<to=>{},
                "depends-on" if !runnable || from>to=>{},
                "loop" if (!runnable || from>to) && e.purpose.as_deref().is_some_and(|p|["review","revise","independent-review","other"].contains(&p))=>{},
                _=>return Err("Sequence must go forward; runnable dependencies and loops must point to earlier stages".into()),
            }
        }
        if runnable {
            let ordered = self.ordered();
            if ordered[0].id != self.entry_node_id {
                return Err("Run must enter at the first stage".into());
            }
            let actual: HashSet<_> = self
                .edges
                .iter()
                .filter(|e| e.kind == "sequence")
                .map(|e| (&e.source, &e.target))
                .collect();
            let expected: HashSet<_> = ordered.windows(2).map(|n| (&n[0].id, &n[1].id)).collect();
            if actual != expected {
                return Err(
                    "Running requires one connected ordered sequence; use Rebuild sequence".into(),
                );
            }
        }
        Ok(())
    }
    pub fn rebuild_sequence(&mut self) {
        self.nodes.sort_by_key(|n| n.sequence_index);
        for (i, n) in self.nodes.iter_mut().enumerate() {
            n.sequence_index = i;
        }
        self.entry_node_id = self.nodes.first().map(|n| n.id.clone()).unwrap_or_default();
        self.edges.retain(|e| e.kind != "sequence");
        for ns in self.nodes.windows(2) {
            self.edges.push(SkillEdge {
                id: crate::id(),
                kind: "sequence".into(),
                source: ns[0].id.clone(),
                target: ns[1].id.clone(),
                label: None,
                purpose: None,
            });
        }
    }
    pub fn add_stage(&mut self) {
        let i = self.nodes.len();
        self.nodes.push(SkillNode {
            id: crate::id(),
            kind: "stage".into(),
            role: "other".into(),
            title: "New stage".into(),
            purpose: "Describe the outcome".into(),
            prompt: "Outcome:\nCentral question:\nPreserve:\nDo not:\nVerification:\nStop:".into(),
            exit_criteria: vec![],
            sequence_index: i,
            position: Position {
                x: 40.,
                y: 40. + i as f32 * 150.,
            },
        });
        self.rebuild_sequence();
    }
    pub fn remove_stage(&mut self, id: &str) {
        self.nodes.retain(|n| n.id != id);
        self.edges.retain(|e| e.source != id && e.target != id);
        self.rebuild_sequence();
    }
    pub fn move_stage(&mut self, id: &str, delta: isize) {
        self.nodes.sort_by_key(|n| n.sequence_index);
        if let Some(i) = self.nodes.iter().position(|n| n.id == id) {
            let j = i.saturating_add_signed(delta).min(self.nodes.len() - 1);
            self.nodes.swap(i, j);
            for (i, n) in self.nodes.iter_mut().enumerate() {
                n.sequence_index = i;
            }
        }
        self.rebuild_sequence();
    }
    pub fn markdown(&self) -> Result<String, String> {
        self.validate(false)?;
        if self.nodes.iter().any(|n| n.prompt.trim().is_empty()) {
            return Err("Cannot compile an empty stage prompt".into());
        }
        let name = self
            .name
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>();
        let mut out=format!("---\nname: {}\ndescription: {}\n---\n\n# {}\n\n{}\n\n## When to use\n{}\n\n## Procedure\n",name.trim_matches('-'),serde_json::to_string(&self.description).unwrap(),self.name,self.description,self.when_to_use);
        for n in self.ordered() {
            out.push_str(&format!(
                "\n### {}. {}\n\n{}\n\n{}\n\nExit criteria:\n{}\n",
                n.sequence_index + 1,
                n.title,
                n.purpose,
                n.prompt,
                n.exit_criteria
                    .iter()
                    .map(|s| format!("- {s}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        out.push_str("\n## Loops\n");
        for e in self.edges.iter().filter(|e| e.kind == "loop") {
            out.push_str(&format!(
                "- {} → {} ({})\n",
                e.source,
                e.target,
                e.purpose.as_deref().unwrap_or("other")
            ));
        }
        out.push_str("\n## Dependencies\n");
        for e in self.edges.iter().filter(|e| e.kind == "depends-on") {
            out.push_str(&format!("- {} depends on {}\n", e.source, e.target));
        }
        out.push_str(&format!("\n## Authority\nThis skill describes what to propose. It does not enlarge what you may execute.\n\n{}\n\n## Limitations\n{}\n\n## Provenance\n{}\n\nThis Markdown is a lossy projection. skill-graph.json is canonical. Export does not install or run this skill.\n",self.authority_text,self.limitations.join("\n"),self.generation_metadata));
        Ok(out)
    }
}
pub const TEMPLATES: &[&str] = &[
    "plan-build-review",
    "research-review-revise",
    "inspect-lock-slice",
    "source-truth-classify",
    "slice-evidence-release",
    "governed-authority",
];
pub fn template(name: &str) -> SkillGraph {
    let (titles, roles, loops): (&[&str], &[&str], &[(usize, usize)]) = match name {
        "plan-build-review" => (
            &["Plan", "Build and verify", "Independent review", "Approve result"],
            &["research", "draft", "independent-review", "other"],
            &[(2, 1)],
        ),
        "inspect-lock-slice" => (
            &[
                "Recapture baseline",
                "Lock decisions",
                "Implement slice",
                "Quality gate",
                "Independent review",
                "Rehearsal",
                "Human gate",
            ],
            &[
                "research",
                "intake",
                "draft",
                "verify",
                "independent-review",
                "verify",
                "other",
            ],
            &[(3, 0), (4, 2), (5, 3)],
        ),
        "source-truth-classify" => (
            &[
                "Name the claim",
                "Inspect live artifacts",
                "Classify claims",
                "Preserve conflicts",
                "Independent rederive",
                "Human gate",
            ],
            &[
                "intake",
                "research",
                "review",
                "revise",
                "independent-review",
                "other",
            ],
            &[(2, 1), (4, 3)],
        ),
        "slice-evidence-release" => (
            &[
                "Baseline contract",
                "Do owning slice",
                "Collect exit evidence",
                "Independent acceptance",
                "Repair or defer",
                "Release gate",
            ],
            &[
                "intake",
                "draft",
                "verify",
                "independent-review",
                "revise",
                "other",
            ],
            &[(2, 1), (3, 0), (4, 2)],
        ),
        "governed-authority" => (
            &[
                "Observe",
                "Separate capability from permission",
                "Propose",
                "Independent authority review",
                "Record receipt",
                "Human gate",
            ],
            &[
                "research",
                "intake",
                "draft",
                "independent-review",
                "verify",
                "other",
            ],
            &[(1, 0), (3, 2), (4, 1)],
        ),
        _ => (
            &[
                "Research",
                "Review",
                "Revise",
                "Independent review",
                "Revise-check",
                "Human gate",
            ],
            &[
                "research",
                "other",
                "revise",
                "independent-review",
                "verify",
                "other",
            ],
            &[(1, 0), (3, 2), (4, 3)],
        ),
    };
    let nodes=titles.iter().enumerate().map(|(i,title)|{
        let role=if roles[i]=="review"{"other"}else{roles[i]};
        let gate=i==titles.len()-1 || (name=="inspect-lock-slice" && i==1);
        let review=title.to_lowercase().contains("review") || role=="independent-review";
        SkillNode{id:format!("stage-{i}"),title:(*title).into(),kind:if gate{"gate"}else if review{"review"}else{"stage"}.into(),role:role.into(),purpose:format!("{title} against the accepted contract, with evidence."),prompt:format!("Outcome: {title} for the current contract.\nCentral question: does the work meet the contract and this stage's exit criteria?\nPreserve: accepted constraints, source attribution, unresolved disagreements, and prior evidence.\nDo not: invent evidence, weaken criteria, or treat assumptions as permission.{}\nVerification: cite artifacts and observed checks; distinguish supported, contradicted, unknown and NOT_RUN.\nStop: report passed, needs_revision with concrete findings, or blocked.{}",if role=="independent-review"{" Independently rederive the verdict; do not rubber-stamp the author."}else{""},if gate{" Human attestation is required; an agent cannot approve this gate."}else{""}),exit_criteria:vec![format!("{title} outcome is documented with evidence and remaining limitations.")],sequence_index:i,position:Position{x:40.,y:40.+i as f32*150.}}
    }).collect();
    let mut graph = SkillGraph {
        schema_version: "0.1.0".into(),
        id: crate::id(),
        name: name.into(),
        description: "A reusable, evidence-driven workflow adapted from Prompt Foundry.".into(),
        when_to_use: "Use for a bounded task with review and explicit completion criteria.".into(),
        nodes,
        edges: vec![],
        entry_node_id: "stage-0".into(),
        limitations: vec![
            "Agent-reported evidence is not independently verified by the application.".into(),
        ],
        becomes_wrong_if: vec![
            "The contract, source artifacts or acceptance criteria change.".into(),
        ],
        confidence: Some("medium".into()),
        authority_text: json!({"mayDoUnattended":["Work within the run's existing permissions"],"mustProposeAndWait":["Human gates and additional permissions"]}),
        generation_metadata: json!({"createdAt":crate::now(),"source":"fixture","baselineSha":"6feda89e5aea6ce256dff58a95f102f7ce4f6692"}),
    };
    graph.rebuild_sequence();
    for (from, to) in loops {
        graph.edges.push(SkillEdge {
            id: crate::id(),
            kind: "loop".into(),
            source: format!("stage-{from}"),
            target: format!("stage-{to}"),
            label: Some("Revise if criteria fail".into()),
            purpose: Some(
                if roles[*to] == "independent-review" {
                    "independent-review"
                } else if ["research", "intake"].contains(&roles[*to]) {
                    "review"
                } else {
                    "revise"
                }
                .into(),
            ),
        });
    }
    let dependencies: &[(usize, usize)] = match name {
        "inspect-lock-slice" => &[(2, 1), (3, 0)],
        "slice-evidence-release" => &[(1, 0), (5, 2)],
        "governed-authority" => &[(4, 0)],
        _ => &[],
    };
    for (from, to) in dependencies {
        graph.edges.push(SkillEdge {
            id: crate::id(),
            kind: "depends-on".into(),
            source: format!("stage-{from}"),
            target: format!("stage-{to}"),
            label: None,
            purpose: None,
        });
    }
    graph
}
