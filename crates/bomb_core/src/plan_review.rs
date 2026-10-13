//! Answering a plan approval the same way on every agent: implement it in a chosen mode, or keep
//! planning. Each agent offers its own options for the same moment:
//!
//! - Claude (ExitPlanMode): `exit-plan-default` (ask before edits), `exit-plan-auto`,
//!   `exit-plan-accept-edits`, `exit-plan-bypass`, `exit-plan-clear-*`, and `reject` (keep planning).
//!   Choosing a mode switches the agent and it implements in the same turn.
//! - Codex (plan review): `implement_plan` and `revise_plan`. Implementing runs under the approval
//!   mode the plan was made in, so to pick the mode we revise, switch mode, and ask it to implement.

use grok_events::PermissionOptionInfo;

/// Sent as the next message when an agent must be asked to implement (Codex's own wording).
pub const IMPLEMENT_PROMPT: &str = "Implement the approved plan.";

/// How to implement an approved plan.
#[derive(Debug, Clone, PartialEq)]
pub enum Implement {
    /// Answer with this option; the agent switches mode and implements in the same turn.
    InTurn(String),
    /// Answer with this option (the agent stays put), switch mode, then send `IMPLEMENT_PROMPT`.
    NextTurn(String),
}

fn has(options: &[PermissionOptionInfo], id: &str) -> bool {
    options.iter().any(|o| o.id == id)
}

/// The answer that implements the plan in `mode` ("auto" or "ask").
pub fn implement(options: &[PermissionOptionInfo], mode: &str) -> Option<Implement> {
    if options.iter().any(|o| o.id.starts_with("exit-plan-")) {
        let wanted: &[&str] = if mode == "ask" { &["exit-plan-default"] } else { &["exit-plan-auto", "exit-plan-accept-edits"] };
        return wanted.iter().find(|id| has(options, id)).map(|id| Implement::InTurn(id.to_string()));
    }
    if has(options, "implement_plan") {
        return keep_planning(options).map(Implement::NextTurn);
    }
    // Another agent: its one-time allow, then the mode switch.
    options
        .iter()
        .find(|o| o.kind.contains("allow") && !o.kind.contains("always"))
        .map(|o| Implement::InTurn(o.id.clone()))
}

/// The answer that keeps the agent planning.
pub fn keep_planning(options: &[PermissionOptionInfo]) -> Option<String> {
    ["revise_plan", "reject"]
        .iter()
        .find(|id| has(options, id))
        .map(|id| id.to_string())
        .or_else(|| options.iter().find(|o| o.kind.contains("reject") || o.kind.contains("deny")).map(|o| o.id.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(list: &[(&str, &str)]) -> Vec<PermissionOptionInfo> {
        list.iter().map(|(id, kind)| PermissionOptionInfo { id: id.to_string(), kind: kind.to_string(), label: id.to_string() }).collect()
    }

    #[test]
    fn claude_switches_mode_in_the_same_turn() {
        let claude = opts(&[
            ("exit-plan-default", "allow_once"),
            ("exit-plan-clear-auto", "allow_always"),
            ("exit-plan-auto", "allow_always"),
            ("exit-plan-bypass", "allow_always"),
            ("reject", "reject_once"),
        ]);
        assert_eq!(implement(&claude, "auto"), Some(Implement::InTurn("exit-plan-auto".into())));
        assert_eq!(implement(&claude, "ask"), Some(Implement::InTurn("exit-plan-default".into())));
        assert_eq!(keep_planning(&claude).as_deref(), Some("reject"));
        // Without an auto mode, Claude offers accept-edits instead.
        let older = opts(&[("exit-plan-default", "allow_once"), ("exit-plan-accept-edits", "allow_always"), ("reject", "reject_once")]);
        assert_eq!(implement(&older, "auto"), Some(Implement::InTurn("exit-plan-accept-edits".into())));
    }

    #[test]
    fn codex_is_asked_to_implement_in_the_next_turn() {
        let codex = opts(&[("implement_plan", "allow_once"), ("revise_plan", "reject_once")]);
        assert_eq!(implement(&codex, "auto"), Some(Implement::NextTurn("revise_plan".into())));
        assert_eq!(keep_planning(&codex).as_deref(), Some("revise_plan"));
    }

    #[test]
    fn other_agents_use_their_one_time_allow() {
        let other = opts(&[("ok", "allow_once"), ("always", "allow_always"), ("no", "reject_once")]);
        assert_eq!(implement(&other, "auto"), Some(Implement::InTurn("ok".into())));
        assert_eq!(keep_planning(&other).as_deref(), Some("no"));
    }
}
