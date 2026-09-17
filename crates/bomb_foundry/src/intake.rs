//! Explicit composer intake, using Prompt Foundry's public contract vocabulary.
use crate::{Document, MODES};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const TARGETS: &[(&str, &str)] = &[
    ("grok-build", "Grok Build"),
    ("openai-codex", "OpenAI Codex"),
    ("claude-code", "Claude Code"),
    ("research-agent", "General research agent"),
    ("general-assistant", "General-purpose AI assistant"),
];
pub const WORK_TYPES: &[(&str, &str)] = &[
    ("research-only", "Research only"),
    ("repository-audit", "Repository audit only"),
    ("planning-docs", "Planning and documentation only"),
    ("implementation", "Implementation"),
    (
        "implementation-plus-verification",
        "Implementation plus verification",
    ),
    ("production-readiness-audit", "Production-readiness audit"),
    ("refactor-migration", "Refactor or migration"),
    ("content-creation", "Content or document creation"),
    ("mixed-workflow", "Mixed workflow"),
];
pub const SOURCE_ROLES: &[(&str, &str)] = &[
    ("canonical-authority", "Source of truth"),
    ("supporting-context", "Supporting context"),
    ("historical-planning", "Historical plan"),
    ("reference-implementation", "Reference implementation"),
    ("unverified-assumption", "Unverified assumption"),
];
pub fn label<'a>(options: &'a [(&str, &str)], key: &'a str) -> &'a str {
    options
        .iter()
        .find(|(id, _)| *id == key)
        .map(|(_, name)| *name)
        .unwrap_or(key)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PromptSource {
    pub value: String,
    pub role: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PromptOptions {
    pub depth: String,
    pub target: String,
    pub work_type: String,
    pub autonomy: String,
    pub sources: Vec<PromptSource>,
}
impl PromptOptions {
    pub fn document(&self, request: &str) -> Result<Document, String> {
        if request.trim().is_empty() || request.len() > 100_000 {
            return Err("Enter a request of up to 100,000 bytes".into());
        }
        if !["fast-draft", "full-project"].contains(&self.depth.as_str()) {
            return Err("Choose Fast Draft or Full Project".into());
        }
        if !TARGETS.iter().any(|(id, _)| *id == self.target) {
            return Err("Choose a target agent".into());
        }
        if !MODES.contains(&self.work_type.as_str()) {
            return Err("Choose the work type before generating".into());
        }
        if self.autonomy.len() > 4_000 || self.sources.len() > 20 {
            return Err("Use up to 4,000 characters of approval notes and 20 sources".into());
        }
        if self
            .sources
            .iter()
            .any(|s| s.value.len() > 20_000 || !SOURCE_ROLES.iter().any(|(id, _)| *id == s.role))
        {
            return Err("Each source needs a valid role and at most 20,000 characters".into());
        }
        let mut d = Document::new(request);
        d.contract = crate::draft(request, &self.depth, &self.work_type, &self.target);
        let sources: Vec<_> = self.sources.iter().filter(|s|!s.value.trim().is_empty()).enumerate().map(|(i,s)|json!({"id":crate::id(),"kind":if s.value.starts_with("https://") || s.value.starts_with("http://") {"website"} else {"document"},"role":s.role,"label":format!("Source {}", i+1),"value":s.value})).collect();
        d.contract.0["sources"] = json!(sources);
        d.contract.0["projectContext"] = json!(format!("Compiled from the user's request. Target: {}. Depth: {}. Work type: {}. Only the request and listed sources are known; source content has not been inspected.",label(TARGETS,&self.target),self.depth,label(WORK_TYPES,&self.work_type)));
        if !self.autonomy.trim().is_empty() {
            d.contract.0["approvalBoundaries"]
                .as_array_mut()
                .unwrap()
                .push(json!(format!(
                    "User approval note: {}",
                    self.autonomy.trim()
                )));
        }
        for source in &sources {
            d.contract.0["authorityOrder"]
                .as_array_mut()
                .unwrap()
                .push(json!(format!(
                    "{} ({}): {}",
                    source["label"].as_str().unwrap(),
                    source["role"].as_str().unwrap(),
                    source["value"].as_str().unwrap()
                )));
        }
        d.contract.validate()?;
        Ok(d)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn options() -> PromptOptions {
        PromptOptions {
            depth: "full-project".into(),
            target: "claude-code".into(),
            work_type: "implementation-plus-verification".into(),
            autonomy: "Do not deploy".into(),
            sources: vec![PromptSource {
                value: "https://example.com/spec".into(),
                role: "canonical-authority".into(),
            }],
        }
    }
    #[test]
    fn tetris_choices_and_authority_survive_compilation() {
        let d = options()
            .document("Ok I want to create a 2d tetris game")
            .unwrap();
        assert_eq!(
            d.contract.0["operatingMode"],
            "implementation-plus-verification"
        );
        assert_eq!(d.contract.0["targetAgent"], "claude-code");
        assert_eq!(d.contract.0["generationMetadata"]["depth"], "full-project");
        assert_eq!(d.contract.0["sources"][0]["role"], "canonical-authority");
        assert!(d.contract.markdown().contains("Do not deploy"));
    }
    #[test]
    fn missing_work_type_does_not_silently_guess() {
        let mut o = options();
        o.work_type.clear();
        assert!(o.document("Make a game").is_err());
    }
}
