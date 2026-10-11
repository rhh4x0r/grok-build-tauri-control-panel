//! A Codex thread's goal: the standing objective Codex keeps working toward, turn after turn.
//! Read on the machine (see `grok_acp::goal`), carried on `goal` events, and shown the same way
//! by the desktop app and the phone.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
}

impl GoalStatus {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "active" => Self::Active,
            "paused" => Self::Paused,
            "blocked" => Self::Blocked,
            "usage_limited" => Self::UsageLimited,
            "budget_limited" => Self::BudgetLimited,
            "complete" => Self::Complete,
            _ => return None,
        })
    }

    /// Codex may still start turns for it on its own.
    pub fn is_live(self) -> bool {
        self == Self::Active
    }

    /// Pausable now.
    pub fn can_pause(self) -> bool {
        self == Self::Active
    }

    /// Stopped short of done: Codex can pick it back up.
    pub fn can_resume(self) -> bool {
        !matches!(self, Self::Active | Self::Complete)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "Working on it",
            Self::Paused => "Paused",
            Self::Blocked => "Blocked — Codex needs you",
            Self::UsageLimited => "Stopped at your usage limit",
            Self::BudgetLimited => "Used its token budget",
            Self::Complete => "Done",
        }
    }
}

/// A thread's goal as Codex last saved it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goal {
    pub objective: String,
    pub status: GoalStatus,
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// The goal in one line. A long goal saved to a file shows the file's first line, read on
    /// the machine that has the file.
    #[serde(default)]
    pub summary: String,
}

impl Goal {
    /// "Goal · Working on it · 12m · 41.2K tokens of 100.0K".
    pub fn headline(&self) -> String {
        let mut line = format!("Goal · {}", self.status.label());
        if self.time_used_seconds >= 60 {
            let (h, m) = (self.time_used_seconds / 3600, self.time_used_seconds / 60 % 60);
            line.push_str(&if h > 0 { format!(" · {h}h {m}m") } else { format!(" · {m}m") });
        }
        if self.tokens_used > 0 {
            line.push_str(&format!(" · {} tokens", compact_count(self.tokens_used)));
            if let Some(budget) = self.token_budget {
                line.push_str(&format!(" of {}", compact_count(budget)));
            }
        }
        line
    }

    /// The summary, or the objective's first line when there is none.
    pub fn title(&self) -> String {
        if !self.summary.is_empty() {
            return self.summary.clone();
        }
        self.objective.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default().to_string()
    }
}

/// 1234 → "1.2K", 2300000 → "2.3M".
fn compact_count(n: i64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.1}K", n as f64 / 1e3),
        n => n.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headline_says_status_time_and_tokens() {
        let mut goal = Goal { objective: "\n Ship it\nwith tests".into(), status: GoalStatus::Active, token_budget: None, tokens_used: 0, time_used_seconds: 30, created_at_ms: 0, updated_at_ms: 0, summary: String::new() };
        assert_eq!(goal.headline(), "Goal · Working on it");
        assert_eq!(goal.title(), "Ship it");
        goal.time_used_seconds = 3 * 3600 + 7 * 60;
        goal.tokens_used = 41_234;
        goal.token_budget = Some(2_000_000);
        assert_eq!(goal.headline(), "Goal · Working on it · 3h 7m · 41.2K tokens of 2.0M");
    }
}
