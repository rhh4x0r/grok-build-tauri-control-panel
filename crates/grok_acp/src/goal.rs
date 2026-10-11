//! Codex goals: a standing objective Codex keeps working toward, turn after turn, until it is
//! complete, blocked, paused or out of budget.
//!
//! The codex-acp adapter starts a goal from a `/goal <objective>` prompt and takes pause, resume
//! and clear on its `_session/goal` request, but it only reports a goal's status to JetBrains
//! clients. Codex keeps every thread's goal in its own database, keyed by the thread id (which is
//! the ACP session id), so the status is read from there.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The adapter's goal control request: `{ sessionId, action, objective? }`.
pub const CONTROL_METHOD: &str = "_session/goal";

/// The longest objective the adapter accepts, in UTF-16 code units (it measures a JS string).
pub const OBJECTIVE_LIMIT: usize = 4000;

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
    fn parse(s: &str) -> Option<Self> {
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
}

/// A `/goal` message, as the adapter reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalCommand {
    Set(String),
    Pause,
    Resume,
    Clear,
    /// `/goal` alone: the adapter answers with its usage line.
    Usage,
}

impl GoalCommand {
    /// The action name on the control request, for commands that have one.
    pub fn action(&self) -> Option<&'static str> {
        match self {
            Self::Pause => Some("pause"),
            Self::Resume => Some("resume"),
            Self::Clear => Some("clear"),
            Self::Set(_) | Self::Usage => None,
        }
    }
}

/// Read a message as a `/goal` command, matching the adapter: a leading `/goal` (any case) as the
/// first word, then `pause`, `resume`, `clear` or the objective.
pub fn parse_command(text: &str) -> Option<GoalCommand> {
    let text = text.trim();
    let rest = text.strip_prefix('/')?;
    let name_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    if !rest[..name_end].eq_ignore_ascii_case("goal") {
        return None;
    }
    let argument = rest[name_end..].trim();
    Some(match argument.to_lowercase().as_str() {
        "" => GoalCommand::Usage,
        "pause" => GoalCommand::Pause,
        "resume" => GoalCommand::Resume,
        "clear" => GoalCommand::Clear,
        _ => GoalCommand::Set(argument.to_string()),
    })
}

/// True when the adapter would turn the objective down for its length.
pub fn objective_too_long(objective: &str) -> bool {
    objective.encode_utf16().count() > OBJECTIVE_LIMIT
}

/// An objective too long for the adapter is saved to a file in `dir`, and the goal points Codex at
/// it, worded the way Codex's own CLI words a pasted goal. Shorter objectives are kept as they are.
pub fn fit_objective(objective: &str, dir: &Path) -> std::io::Result<String> {
    let objective = objective.trim();
    if !objective_too_long(objective) {
        return Ok(objective.to_string());
    }
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("goal-{}.md", uuid::Uuid::new_v4()));
    std::fs::write(&path, objective)?;
    let pointer = format!("pasted text file: {}. Read this file before continuing.", path.display());
    if objective_too_long(&pointer) {
        return Err(std::io::Error::other("the goal file's path is too long to name in a goal"));
    }
    Ok(pointer)
}

/// Codex's home: the session's `CODEX_HOME`, else this process's, else `~/.codex`.
pub fn codex_home(env: &[(String, String)]) -> Option<PathBuf> {
    env.iter()
        .rev()
        .find(|(k, _)| k == "CODEX_HOME")
        .map(|(_, v)| PathBuf::from(v))
        .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")))
}

/// The goal Codex has saved for `thread_id`, if any. Missing database or table reads as no goal.
pub fn read(codex_home: &Path, thread_id: &str) -> Option<Goal> {
    use rusqlite::{OpenFlags, OptionalExtension};
    let path = codex_home.join("goals_1.sqlite");
    if !path.exists() {
        return None;
    }
    let db = rusqlite::Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
    let _ = db.busy_timeout(std::time::Duration::from_millis(250));
    db.query_row(
        "SELECT objective, status, token_budget, tokens_used, time_used_seconds, created_at_ms, updated_at_ms \
         FROM thread_goals WHERE thread_id = ?1",
        [thread_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        },
    )
    .optional()
    .ok()
    .flatten()
    .and_then(|(objective, status, token_budget, tokens_used, time_used_seconds, created_at_ms, updated_at_ms)| {
        Some(Goal {
            objective,
            status: GoalStatus::parse(&status)?,
            token_budget,
            tokens_used,
            time_used_seconds,
            created_at_ms,
            updated_at_ms,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_goal_commands_like_the_adapter() {
        assert_eq!(parse_command("/goal"), Some(GoalCommand::Usage));
        assert_eq!(parse_command("  /GOAL  Pause "), Some(GoalCommand::Pause));
        assert_eq!(parse_command("/goal resume"), Some(GoalCommand::Resume));
        assert_eq!(parse_command("/goal clear"), Some(GoalCommand::Clear));
        assert_eq!(parse_command("/goal ship it\nwith tests"), Some(GoalCommand::Set("ship it\nwith tests".into())));
        assert_eq!(parse_command("/goals x"), None);
        assert_eq!(parse_command("please /goal x"), None);
        assert_eq!(parse_command("/goal pause the music"), Some(GoalCommand::Set("pause the music".into())));
    }

    #[test]
    fn long_objectives_move_to_a_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(fit_objective("  short  ", dir.path()).unwrap(), "short");
        let long = "x".repeat(OBJECTIVE_LIMIT + 1);
        let pointer = fit_objective(&long, dir.path()).unwrap();
        assert!(pointer.starts_with("pasted text file: "));
        assert!(!objective_too_long(&pointer));
        let path = pointer.trim_start_matches("pasted text file: ").trim_end_matches(". Read this file before continuing.");
        assert_eq!(std::fs::read_to_string(path).unwrap(), long);
    }

    #[test]
    fn limit_counts_utf16_like_the_adapter() {
        // Each emoji is two UTF-16 units.
        assert!(!objective_too_long(&"😀".repeat(OBJECTIVE_LIMIT / 2)));
        assert!(objective_too_long(&"😀".repeat(OBJECTIVE_LIMIT / 2 + 1)));
    }

    #[test]
    fn reads_codex_goal_rows() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(dir.path(), "t1"), None);
        let db = rusqlite::Connection::open(dir.path().join("goals_1.sqlite")).unwrap();
        db.execute_batch(
            "CREATE TABLE thread_goals (thread_id TEXT PRIMARY KEY, goal_id TEXT, objective TEXT, status TEXT, \
             token_budget INTEGER, tokens_used INTEGER, time_used_seconds INTEGER, created_at_ms INTEGER, updated_at_ms INTEGER);
             INSERT INTO thread_goals VALUES ('t1', 'g', 'Ship it', 'budget_limited', 5000, 4200, 61, 1, 2);",
        )
        .unwrap();
        let goal = read(dir.path(), "t1").unwrap();
        assert_eq!(goal.objective, "Ship it");
        assert_eq!(goal.status, GoalStatus::BudgetLimited);
        assert_eq!(goal.token_budget, Some(5000));
        assert_eq!(read(dir.path(), "t2"), None);
    }
}
