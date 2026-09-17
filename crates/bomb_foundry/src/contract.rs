use crate::{Position, SkillGraph, SkillNode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const SECTIONS: &[&str] = &[
    "goal",
    "operatingMode",
    "authorityOrder",
    "projectContext",
    "knownState",
    "assumptions",
    "constraints",
    "initialDiscovery",
    "agentRoles",
    "phases",
    "technicalRequirements",
    "researchRules",
    "verification",
    "approvalBoundaries",
    "deliverables",
    "doneWhen",
    "handoff",
    "prohibitions",
];
pub const MODES: &[&str] = &[
    "research-only",
    "repository-audit",
    "planning-docs",
    "implementation",
    "implementation-plus-verification",
    "production-readiness-audit",
    "refactor-migration",
    "content-creation",
    "mixed-workflow",
];
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectContract(pub Value);
impl ProjectContract {
    pub fn validate(&self) -> Result<(), String> {
        let v = &self.0;
        if v["schemaVersion"] != "1.0.0" {
            return Err("Unsupported ProjectContract version".into());
        }
        for key in [
            "title",
            "rawRequest",
            "goal",
            "targetAgent",
            "operatingMode",
        ] {
            if v[key].as_str().is_none_or(|s| s.trim().is_empty()) {
                return Err(format!("Contract needs {key}"));
            }
        }
        if !MODES.contains(&v["operatingMode"].as_str().unwrap_or("")) {
            return Err("Unknown operating mode".into());
        }
        if ![
            "grok-build",
            "openai-codex",
            "claude-code",
            "research-agent",
            "general-assistant",
        ]
        .contains(&v["targetAgent"].as_str().unwrap_or(""))
        {
            return Err("Unknown target agent".into());
        }
        if v["rawRequest"].as_str().unwrap_or("").len() > 100_000 {
            return Err("Request exceeds 100,000 bytes".into());
        }
        for key in [
            "sources",
            "authorityOrder",
            "knownState",
            "assumptions",
            "constraints",
            "initialDiscovery",
            "agentRoles",
            "phases",
            "technicalRequirements",
            "researchRules",
            "verification",
            "approvalBoundaries",
            "deliverables",
            "doneWhen",
            "handoff",
            "prohibitions",
        ] {
            if !v[key].is_array() {
                return Err(format!("{key} must be a list"));
            }
        }
        Ok(())
    }
    pub fn markdown(&self) -> String {
        let mut out = format!("# {}\n", self.0["title"].as_str().unwrap_or("Contract"));
        if let Some(sources) = self.0["sources"].as_array() {
            if !sources.is_empty() {
                out.push_str(&format!(
                    "\n## Sources\n{}\n",
                    serde_json::to_string_pretty(sources).unwrap()
                ));
            }
        }
        for key in SECTIONS {
            let v = &self.0[*key];
            if v.is_null() || v.as_array().is_some_and(Vec::is_empty) {
                continue;
            }
            let body = if let Some(text) = v.as_str() {
                text.into()
            } else {
                serde_json::to_string_pretty(v).unwrap()
            };
            out.push_str(&format!("\n## {key}\n{body}\n"));
        }
        out
    }
    pub fn explain(&self) -> String {
        format!("What are we trying to do?\n{}\n\nHow will the AI try to do it?\nFollow the {} stages and constraints, checking the supplied sources before making claims.\n\nHow will we know it worked?\n{}",self.0["goal"].as_str().unwrap_or(""),self.0["operatingMode"].as_str().unwrap_or("planned"),self.0["doneWhen"].as_array().map(|a|a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")).unwrap_or_default())
    }
    pub fn graph(&self) -> SkillGraph {
        let mut g = crate::template("research-review-revise");
        g.name = self.0["title"].as_str().unwrap_or("Contract loop").into();
        g.description = self.0["goal"]
            .as_str()
            .unwrap_or("Contract workflow")
            .into();
        g.nodes.clear();
        g.edges.clear();
        for (i, p) in self.0["phases"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            g.nodes.push(SkillNode {
                id: p["id"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(crate::id),
                kind: "stage".into(),
                role: "other".into(),
                title: p["title"].as_str().unwrap_or("Phase").into(),
                purpose: p["objective"].as_str().unwrap_or("Complete phase").into(),
                prompt: p["activities"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default(),
                exit_criteria: p["exitCriteria"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                sequence_index: i,
                position: Position {
                    x: 40.,
                    y: 40. + 150. * i as f32,
                },
            });
        }
        g.generation_metadata =
            json!({"createdAt":crate::now(),"source":"contract-seed","baselineSha":"bomb-code"});
        g.rebuild_sequence();
        g
    }
}
pub fn draft(request: &str, depth: &str, mode: &str, target: &str) -> ProjectContract {
    let mode = if MODES.contains(&mode) {
        mode
    } else {
        "planning-docs"
    };
    let implementing = [
        "implementation",
        "implementation-plus-verification",
        "mixed-workflow",
        "refactor-migration",
    ]
    .contains(&mode);
    let titles = if depth == "fast-draft" {
        vec![
            "Inspect and produce the bounded artifact",
            "Verify and hand off evidence",
        ]
    } else if implementing {
        vec![
            "Inspect current state",
            "Implement bounded change",
            "Verify behavior",
            "Deliver evidence",
        ]
    } else {
        vec![
            "Inspect sources and constraints",
            "Produce the requested artifact",
            "Review evidence and limitations",
        ]
    };
    let phases:Vec<_>=titles.iter().enumerate().map(|(i,t)|json!({"id":format!("phase-{i}"),"title":t,"objective":format!("{t} for the stated goal"),"activities":[format!("{t}. Use actual artifacts and preserve constraints.")],"exitCriteria":["Outcome has evidence; unverified claims remain explicitly unverified"]})).collect();
    ProjectContract(
        json!({"schemaVersion":"1.0.0","title":request.lines().next().unwrap_or("New contract").chars().take(72).collect::<String>(),"rawRequest":request,"targetAgent":target,"operatingMode":mode,"goal":request,"projectContext":"","sources":[],"authorityOrder":["Current user direction and accepted project instructions","Live artifacts and reproduced evidence","Supporting documents","Historical notes and assumptions"],"knownState":[],"assumptions":[{"id":crate::id(),"kind":"technical-verify","text":"Repository state and source claims still need inspection."}],"constraints":["Keep the task bounded to the stated goal"],"initialDiscovery":["Inspect the relevant sources; label gaps and conflicts"],"agentRoles":[],"phases":phases,"technicalRequirements":[],"researchRules":["Cite primary evidence and label inference"],"verification":["Check behavior against the stated completion criteria; compilation alone is insufficient"],"approvalBoundaries":["Existing permissions and explicit human gates remain in force"],"deliverables":["Requested artifact","Evidence and remaining limitations"],"doneWhen":["The goal is met with observable evidence","Remaining limitations are explicit"],"handoff":["Summarize outcomes, evidence, and how to continue"],"prohibitions":["Do not invent evidence or treat assumptions as permission"],"generationMetadata":{"generatedAt":crate::now(),"provider":"local-compiler","depth":depth,"demoMode":false}}),
    )
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub version: u32,
    pub id: String,
    pub revision: u64,
    pub name: String,
    pub contract: ProjectContract,
    pub graph: SkillGraph,
    pub bindings: std::collections::HashMap<String, crate::StageBinding>,
    pub policy: crate::RunPolicy,
    #[serde(default)]
    pub attribution: Vec<Value>,
}
impl Document {
    pub fn new(request: &str) -> Self {
        let contract = draft(request, "fast-draft", "planning-docs", "general-assistant");
        Self {
            version: 1,
            id: crate::id(),
            revision: 0,
            name: contract.0["title"].as_str().unwrap_or("Untitled").into(),
            contract,
            graph: crate::template("research-review-revise"),
            bindings: Default::default(),
            policy: Default::default(),
            attribution: vec![],
        }
    }
    pub fn import(text: &str) -> Result<Self, String> {
        if text.len() > 4_000_000 {
            return Err("Import exceeds 4 MB".into());
        }
        let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let mut d = Self::new("Imported contract");
        if v["version"] == 1 && v.get("contract").is_some() {
            d = serde_json::from_value(v).map_err(|e| e.to_string())?;
        } else if v["schemaVersion"] == "0.1.0" {
            d.graph = serde_json::from_value(v).map_err(|e| e.to_string())?;
            d.name = d.graph.name.clone();
        } else {
            d.contract = ProjectContract(v);
            d.graph = d.contract.graph();
            d.name = d.contract.0["title"].as_str().unwrap_or("Imported").into();
        }
        d.contract.validate()?;
        d.graph.validate(false)?;
        d.id = crate::id();
        d.revision = 0;
        Ok(d)
    }
}
