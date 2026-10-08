//! What Swift renders: plain records converted from the shared reducer's model.

use std::time::Instant;

use bomb_transcript::presence::{Phase, Presence};
use bomb_transcript::transcript::{Body, Change, Entry, Role, Thread};
use serde::Deserialize;

/// Who wrote an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EntryRole {
    You,
    Agent,
    Thought,
    Tool,
    Plan,
    Approval,
    System,
    Error,
}

impl From<Role> for EntryRole {
    fn from(role: Role) -> Self {
        match role {
            Role::You => Self::You,
            Role::Agent => Self::Agent,
            Role::Thought => Self::Thought,
            Role::Tool => Self::Tool,
            Role::Plan => Self::Plan,
            Role::Approval => Self::Approval,
            Role::System => Self::System,
            Role::Error => Self::Error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ApprovalChoice {
    pub id: String,
    /// allow_once | allow_always | reject_once | reject_always
    pub kind: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum EntryBody {
    /// Markdown for agent replies and plain text otherwise.
    Text { text: String },
    Tool { tool_id: String, name: String, status: String, args: String, result: Option<String> },
    Plan { title: Option<String>, markdown: String },
    Approval {
        request_id: String,
        tool: String,
        summary: String,
        explanation: Option<String>,
        options: Vec<ApprovalChoice>,
        plan_approval: bool,
        /// The rule an "always allow" choice installs, shown under the buttons.
        allow_pattern: Option<String>,
        /// The chosen option id, "cancelled" or "restored"; `None` while it is waiting for an answer.
        resolution: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct EntryImage {
    pub mime_type: String,
    /// Base64.
    pub data: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct EntryView {
    /// Stable within one load of a thread; use it as the row identity.
    pub id: u64,
    /// Milliseconds since 1970.
    pub at_ms: i64,
    pub role: EntryRole,
    pub body: EntryBody,
    /// Still receiving text.
    pub streaming: bool,
    pub images: Vec<EntryImage>,
}

impl From<&Entry> for EntryView {
    fn from(entry: &Entry) -> Self {
        let body = match &entry.body {
            Body::Text(text) => EntryBody::Text { text: text.clone() },
            Body::Tool(tool) => EntryBody::Tool { tool_id: tool.tool_id.clone(), name: tool.name.clone(), status: tool.status.clone(), args: tool.args.clone(), result: tool.result.clone() },
            Body::Plan(plan) => EntryBody::Plan { title: plan.title.clone(), markdown: plan.markdown.clone() },
            Body::Approval(card) => EntryBody::Approval {
                request_id: card.request_id.clone(),
                tool: card.tool.clone(),
                summary: card.summary.clone(),
                explanation: card.explanation.clone(),
                options: card.options.iter().map(|o| ApprovalChoice { id: o.id.clone(), kind: o.kind.clone(), label: o.label.clone() }).collect(),
                plan_approval: card.plan_approval,
                allow_pattern: card.allow_pattern.clone(),
                resolution: card.resolution.clone(),
            },
        };
        Self {
            id: entry.id,
            at_ms: entry.at.timestamp_millis(),
            role: entry.role.into(),
            body,
            streaming: entry.streaming,
            images: entry.images.iter().map(|i| EntryImage { mime_type: i.mime_type.clone(), data: i.data.clone(), name: i.name.clone() }).collect(),
        }
    }
}

/// What the current turn is doing, for the status line under the transcript.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PresenceView {
    /// idle | send | think | tools | reply | wait | done | error
    pub phase: String,
    /// "Thinking · 12s", "Running cargo test · 4s", …
    pub label: String,
    pub turn_active: bool,
    /// 0…1, for the thin progress bar.
    pub progress: f32,
    pub last_tool: Option<String>,
    /// Seconds since the turn started, while one is active.
    pub elapsed_secs: Option<u64>,
}

impl PresenceView {
    pub fn of(presence: &Presence, now: Instant) -> Self {
        let phase = match presence.phase {
            Phase::Idle => "idle",
            Phase::Send => "send",
            Phase::Think => "think",
            Phase::Tools => "tools",
            Phase::Reply => "reply",
            Phase::Wait => "wait",
            Phase::Done => "done",
            Phase::Error => "error",
        };
        Self {
            phase: phase.into(),
            label: presence.label(now),
            turn_active: presence.turn_active(),
            progress: presence.progress(),
            last_tool: presence.last_tool_summary(),
            elapsed_secs: presence.turn_active().then(|| presence.elapsed(now)).flatten().map(|d| d.as_secs()),
        }
    }
}

/// How to bring Swift's copy of an open thread up to date.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum ThreadPatch {
    /// Replace everything (first load, a reload, or a rebuild after reconnecting).
    Reset { entries: Vec<EntryView>, has_earlier: bool },
    /// Put `entry` at `index`: append when `index` is the current count, replace otherwise.
    Upsert { index: u64, entry: EntryView },
    /// Append streamed text to the entry at `index`.
    Stream { index: u64, delta: String },
    /// Drop `count` entries from the front.
    Trim { count: u64 },
}

/// Turn the reducer's changes into patches, merging what a view needs to redraw.
pub fn patches(thread: &Thread, changes: &[Change]) -> Vec<ThreadPatch> {
    let mut out = Vec::new();
    for change in changes {
        match change {
            Change::Appended(index) | Change::Updated(index) => {
                if let Some(entry) = thread.entries.get(*index) {
                    out.push(ThreadPatch::Upsert { index: *index as u64, entry: entry.into() });
                }
            }
            Change::Streamed { index, delta } => out.push(ThreadPatch::Stream { index: *index as u64, delta: delta.clone() }),
            Change::Trimmed { count } => out.push(ThreadPatch::Trim { count: *count as u64 }),
            Change::Label(_) | Change::Status(_) | Change::Explain | Change::Protocol | Change::Boom | Change::Presence => {}
        }
    }
    out
}

/// One row of the thread list.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ThreadSummary {
    pub id: String,
    pub machine_id: String,
    pub label: Option<String>,
    /// The project the thread belongs to (its worktree's original folder when it has one).
    pub project_root: String,
    pub cwd: String,
    pub backend: String,
    pub model: String,
    /// starting | idle | running | waiting_approval | completed | failed | cancelled | …
    pub status: String,
    pub approval_mode: Option<String>,
    pub needs_approval: bool,
    pub running: bool,
    pub message_count: u64,
    /// RFC 3339.
    pub updated_at: String,
    /// RFC 3339.
    pub created_at: String,
}

/// The fields of a core's `ThreadDto` the phone uses.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadRow {
    pub id: String,
    pub cwd: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub backend: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub message_count: u64,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub created_at: String,
    pub label: Option<String>,
    pub approval_mode: Option<String>,
    pub project_root: Option<String>,
}

impl ThreadSummary {
    pub(crate) fn from_row(machine_id: &str, row: ThreadRow) -> Self {
        let mut summary = Self {
            project_root: row.project_root.clone().unwrap_or_else(|| row.cwd.clone()),
            id: row.id,
            machine_id: machine_id.to_string(),
            label: row.label,
            cwd: row.cwd,
            backend: row.backend,
            model: row.model,
            status: String::new(),
            approval_mode: row.approval_mode,
            needs_approval: false,
            running: false,
            message_count: row.message_count,
            updated_at: row.updated_at,
            created_at: row.created_at,
        };
        summary.set_status(&row.status);
        summary
    }

    pub(crate) fn set_status(&mut self, status: &str) {
        self.status = status.to_string();
        self.needs_approval = status.contains("wait");
        self.running = matches!(status, "starting" | "running" | "cancelling" | "recovering") || self.needs_approval;
    }
}

/// A backend and the models it offers on that machine.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct BackendChoice {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub reason: Option<String>,
    pub default_model: String,
    pub models: Vec<ModelChoice>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ModelChoice {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BackendRow {
    pub id: String,
    pub display_name: String,
    pub available: bool,
    pub reason: Option<String>,
    #[serde(default)]
    pub default_model: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub model_names: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub model_descriptions: std::collections::HashMap<String, String>,
}

impl From<BackendRow> for BackendChoice {
    fn from(row: BackendRow) -> Self {
        let models = row.models.iter().map(|id| ModelChoice {
            id: id.clone(),
            name: row.model_names.get(id).cloned().unwrap_or_else(|| id.clone()),
            description: row.model_descriptions.get(id).cloned(),
        }).collect();
        Self { id: row.id, name: row.display_name, available: row.available, reason: row.reason, default_model: row.default_model, models }
    }
}

/// How the machine's desktop sidebar is organised: threads archived out of view, and pinned projects.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SidebarPrefs {
    pub archived: Vec<String>,
    pub pinned_projects: Vec<String>,
}

/// One provider's usage limits, for the bars under the thread list.
#[derive(Debug, Clone, PartialEq, uniffi::Record, Deserialize)]
pub struct UsageView {
    pub backend: String,
    pub plan: Option<String>,
    pub windows: Vec<UsageWindowView>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record, Deserialize)]
pub struct UsageWindowView {
    pub label: String,
    pub used_pct: f32,
    /// RFC 3339.
    pub resets_at: Option<String>,
}

/// The parts of a tool row its label needs.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ToolStep {
    pub name: String,
    pub args: String,
}

fn tool_row(step: &ToolStep) -> bomb_transcript::transcript::ToolRow {
    bomb_transcript::transcript::ToolRow { tool_id: String::new(), name: step.name.clone(), status: String::new(), args: step.args.clone(), result: None }
}

/// "Thought · Ran 4 commands · Edited 1 file", the same line the Mac shows for a folded run of steps.
#[uniffi::export]
pub fn activity_label(thoughts: u32, steps: Vec<ToolStep>) -> String {
    let rows: Vec<_> = steps.iter().map(tool_row).collect();
    bomb_transcript::summary::activity_label(thoughts as usize, rows.iter())
}

/// "Running `npm test`", for the step in progress.
#[uniffi::export]
pub fn running_label(step: ToolStep) -> String {
    bomb_transcript::summary::running_label(&tool_row(&step))
}
