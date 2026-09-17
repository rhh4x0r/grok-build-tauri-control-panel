//! Event → thread reducer. Folds the [`ControlEvent`] stream (and persisted
//! transcript rows) into a renderable [`Thread`]: an ordered list of
//! [`Entry`]s plus turn [`Presence`]. Ported from `handleControlEvent` /
//! `appendTranscript` in `frontend/app.js`, minus everything that was only
//! there to feed the removed right rail.
//!
//! Pure: no clocks, no I/O. Every mutation returns [`Change`]s so a view can
//! patch incrementally (push a streamed delta into a markdown state instead
//! of re-parsing the whole entry).

use std::collections::{HashSet, VecDeque};
use std::time::Instant;

use chrono::{DateTime, Utc};
use grok_events::{ControlEvent, PermissionOptionInfo, PlanUpdateEvent, SessionStatus, ToolCallEvent};
use grok_persistence::TranscriptEntry;
use serde_json::Value;

use crate::presence::{Patch, Phase, Presence};

/// Streaming blocks longer than this are closed and a fresh one started, so a
/// single multi-megabyte entry never has to be re-laid-out on every chunk.
const ROTATE_STREAM_AT: usize = 64_000;
/// How far back a streamed chunk looks for its still-open block, skipping
/// tool/plan rows that landed mid-response.
const STREAM_LOOKBACK: usize = 8;
/// Entries kept per thread (older ones are dropped from the live model; the
/// SQLite transcript keeps everything).
const MAX_ENTRIES: usize = 2000;
const MAX_PROTOCOL_LINES: usize = 500;
const MAX_EXPLANATIONS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    You,
    Agent,
    Thought,
    Tool,
    Plan,
    Approval,
    System,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolRow {
    pub tool_id: String,
    pub name: String,
    /// running | completed | failed | denied | pending | cancelled
    pub status: String,
    pub args: String,
    pub result: Option<String>,
}

impl ToolRow {
    pub fn is_terminal(&self) -> bool {
        tool_status_terminal(&self.status)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanDoc {
    pub title: Option<String>,
    /// Markdown body: either a lifted plan document or a rendered step list.
    pub markdown: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalCard {
    pub request_id: String,
    pub tool: String,
    pub summary: String,
    /// Plain-English explanation from the narrator, when it arrives.
    pub explanation: Option<String>,
    pub options: Vec<PermissionOptionInfo>,
    pub plan_approval: bool,
    /// Rule an "always allow" button would install (`Bash(cargo test *)`).
    pub allow_pattern: Option<String>,
    /// Option id chosen, "cancelled", or "restored" for inert cards loaded
    /// from disk after the live request died with its process.
    pub resolution: Option<String>,
}

impl ApprovalCard {
    pub fn is_open(&self) -> bool {
        self.resolution.is_none()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageAttachment {
    pub mime_type: String,
    /// Base64 payload (what was sent to the agent).
    pub data: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Text(String),
    Tool(ToolRow),
    Plan(PlanDoc),
    Approval(ApprovalCard),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: u64,
    pub at: DateTime<Utc>,
    pub role: Role,
    pub body: Body,
    /// Still receiving chunks.
    pub streaming: bool,
    pub images: Vec<ImageAttachment>,
}

impl Entry {
    pub fn text(&self) -> Option<&str> {
        match &self.body {
            Body::Text(t) => Some(t),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Explanation {
    pub text: String,
    /// tick | approval | error
    pub kind: String,
    pub request_id: Option<String>,
    pub at: DateTime<Utc>,
}

/// What a reducer step changed, for incremental view updates.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Appended(usize),
    Updated(usize),
    /// A streaming chunk was appended to `entries[index]`.
    Streamed { index: usize, delta: String },
    /// Older entries were dropped; `count` entries removed from the front.
    Trimmed { count: usize },
    Label(String),
    Status(SessionStatus),
    Explain,
    Protocol,
    /// Turn finished: the UI should hold the boom for `BOOM_HOLD` and then
    /// call [`Thread::settle`].
    Boom,
    Presence,
}

#[derive(Debug, Default)]
pub struct Thread {
    pub entries: Vec<Entry>,
    pub presence: Presence,
    pub label: Option<String>,
    pub status: Option<SessionStatus>,
    pub context_tokens: Option<u64>,
    /// Raw ACP / stderr lines. Not shown in the transcript; available in the
    /// diagnostics protocol log.
    pub protocol_log: VecDeque<String>,
    pub explanations: Vec<Explanation>,
    pub explain_pending: bool,
    open_tools: HashSet<String>,
    next_id: u64,
}

impl Thread {
    pub fn new() -> Self {
        Self::default()
    }

    /// Unresolved approval cards, oldest first.
    pub fn open_approvals(&self) -> impl Iterator<Item = (usize, &ApprovalCard)> {
        self.entries.iter().enumerate().filter_map(|(i, e)| match &e.body {
            Body::Approval(a) if a.is_open() => Some((i, a)),
            _ => None,
        })
    }

    /// Rebuild from persisted rows (roles written by `persist_control_event`
    /// and `send_prompt`). Approval cards come back inert.
    pub fn hydrate(&mut self, rows: &[TranscriptEntry]) {
        self.hydrate_rows(rows);
        // Nothing can still be running in a saved copy: the agent that ran
        // those tools is gone, and any live turn will re-open its own rows.
        let mut ch = Vec::new();
        self.sweep_tools("completed", &mut ch);
    }

    fn hydrate_rows(&mut self, rows: &[TranscriptEntry]) {
        self.entries.clear();
        self.protocol_log.clear();
        self.open_tools.clear();
        self.presence = Presence::default();
        for row in rows {
            let at = row
                .at
                .parse::<DateTime<Utc>>()
                .unwrap_or_else(|_| Utc::now());
            match row.role.as_str() {
                "prompt" | "user" => {
                    let (text, images) = split_prompt_row(&row.body);
                    let e = self.push(Role::You, Body::Text(text), at);
                    e.images = images;
                }
                "agent" => {
                    if starts_with_status_glyph(&row.body) || is_noise_agent_text(&row.body) {
                        for line in row.body.lines() {
                            self.protocol(line);
                        }
                    } else {
                        self.push(Role::Agent, Body::Text(row.body.clone()), at);
                    }
                }
                "thought" => {
                    self.push(Role::Thought, Body::Text(row.body.clone()), at);
                }
                "tool" => {
                    let v: Value = serde_json::from_str(&row.body).unwrap_or(Value::Null);
                    let row_ = ToolRow {
                        tool_id: str_at(&v, "id").unwrap_or_default(),
                        name: str_at(&v, "tool").unwrap_or_else(|| "tool".into()),
                        status: str_at(&v, "status").unwrap_or_else(|| "completed".into()),
                        args: str_at(&v, "args").unwrap_or_default(),
                        result: str_at(&v, "result"),
                    };
                    if v.is_null() {
                        self.push(Role::Tool, Body::Text(row.body.clone()), at);
                    } else if let Some(i) = self.find_tool(&row_.tool_id).filter(|_| !row_.tool_id.is_empty()) {
                        // Later status of the same call: update, don't stack.
                        self.entries[i].body = Body::Tool(row_);
                        self.entries[i].at = at;
                    } else {
                        self.push(Role::Tool, Body::Tool(row_), at);
                    }
                }
                "plan" => {
                    let doc = match serde_json::from_str::<PlanUpdateEvent>(&row.body) {
                        Ok(ev) => plan_from_update(&ev),
                        Err(_) => PlanDoc {
                            title: None,
                            markdown: row.body.clone(),
                        },
                    };
                    self.push(Role::Plan, Body::Plan(doc), at);
                }
                "approval" => {
                    let (tool, summary) = match row.body.split_once(" — ") {
                        Some((t, s)) => (t.to_string(), s.to_string()),
                        None => ("tool".into(), row.body.clone()),
                    };
                    self.push(
                        Role::Approval,
                        Body::Approval(ApprovalCard {
                            request_id: String::new(),
                            tool,
                            summary,
                            explanation: None,
                            options: Vec::new(),
                            plan_approval: false,
                            allow_pattern: None,
                            resolution: Some("restored".into()),
                        }),
                        at,
                    );
                }
                "term" => {
                    for line in row.body.lines() {
                        self.protocol(line);
                    }
                }
                "error" => {
                    self.push(Role::Error, Body::Text(row.body.clone()), at);
                }
                "image" => {
                    let v: Value = serde_json::from_str(&row.body).unwrap_or(Value::Null);
                    let (Some(path), mime) = (str_at(&v, "path"), str_at(&v, "mimeType").unwrap_or_else(|| "image/png".into())) else {
                        continue;
                    };
                    if let Ok(bytes) = std::fs::read(&path) {
                        use base64::Engine;
                        let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                        let e = self.push(Role::Agent, Body::Text(String::new()), at);
                        e.images.push(ImageAttachment { mime_type: mime, data, name: Some(path) });
                    }
                }
                _ => {
                    // Breadcrumbs the old UI needed so the column never looked
                    // idle; the status line covers that now.
                    if starts_with_status_glyph(&row.body) || row.body.starts_with("→ prompt accepted") {
                        self.protocol(&row.body);
                    } else {
                        self.push(Role::System, Body::Text(row.body.clone()), at);
                    }
                }
            }
        }
    }

    /// Record the user's own prompt (optimistically, before the backend acks).
    pub fn note_prompt(
        &mut self,
        text: &str,
        images: Vec<ImageAttachment>,
        now: Instant,
    ) -> Vec<Change> {
        self.close_streams();
        let e = self.push(Role::You, Body::Text(text.to_string()), Utc::now());
        e.images = images;
        let idx = self.entries.len() - 1;
        self.presence = Presence::default();
        self.presence.signal(
            Phase::Send,
            Patch {
                prompt_chars: Some(text.chars().count()),
                ..Default::default()
            },
            now,
        );
        vec![Change::Appended(idx), Change::Presence]
    }

    /// Local system line (dev server started, MCP skipped, …).
    pub fn note_failure(&mut self, message: &str, now: Instant) -> Vec<Change> {
        let mut ch = self.close_streams();
        self.sweep_tools("failed", &mut ch);
        self.push(Role::Error, Body::Text(message.into()), Utc::now());
        ch.push(Change::Appended(self.entries.len() - 1));
        self.end_turn(Phase::Error, message, now);
        ch.push(Change::Presence);
        ch
    }

    pub fn note_system(&mut self, text: &str) -> Vec<Change> {
        self.push(Role::System, Body::Text(text.to_string()), Utc::now());
        vec![Change::Appended(self.entries.len() - 1)]
    }

    /// Called after `BOOM_HOLD` elapses: a finished turn returns to idle.
    pub fn settle(&mut self, now: Instant) -> Vec<Change> {
        if self.presence.phase == Phase::Done {
            self.presence.signal(Phase::Idle, Patch::default(), now);
            return vec![Change::Presence];
        }
        Vec::new()
    }

    /// Periodic tick (1s) while a turn is active so stall detection and the
    /// elapsed clock refresh. Returns whether anything visible changed.
    pub fn tick(&mut self, _now: Instant) -> bool {
        self.presence.turn_active()
    }

    /// Fold one event for this thread. The caller has already routed by
    /// session id; host-level events (no session) never reach here.
    pub fn apply(&mut self, ev: &ControlEvent, now: Instant) -> Vec<Change> {
        match ev {
            ControlEvent::AgentMessage { text, .. } => self.on_agent_message(text, now),
            ControlEvent::ToolCall { event, .. } => self.on_tool_call(event, now),
            ControlEvent::PlanUpdate { event, .. } => {
                let mut ch = self.close_streams();
                self.push(Role::Plan, Body::Plan(plan_from_update(event)), event.at);
                ch.push(Change::Appended(self.entries.len() - 1));
                // Agents emit an initial plan right after session start; only
                // nudge presence during a real turn.
                if self.presence.turn_active() {
                    self.presence.signal(
                        Phase::Think,
                        Patch {
                            note: Some(event.title.clone().unwrap_or_else(|| "plan update".into())),
                            ..Default::default()
                        },
                        now,
                    );
                    ch.push(Change::Presence);
                }
                ch
            }
            ControlEvent::SessionCreated { session_id, .. } => {
                self.protocol(&format!("session ready · {}", short_id(&session_id.to_string())));
                vec![Change::Protocol]
            }
            ControlEvent::SessionStatusChanged { status, .. } => self.on_status(*status, now),
            ControlEvent::SessionCancelled { .. } => {
                let mut ch = self.close_streams();
                self.sweep_tools("cancelled", &mut ch);
                self.protocol("session cancelled");
                self.end_turn(Phase::Error, "Cancelled", now);
                ch.push(Change::Presence);
                ch
            }
            ControlEvent::SessionCompleted { .. } => self.on_status(SessionStatus::Completed, now),
            ControlEvent::Error { message, .. } => self.note_failure(message, now),
            ControlEvent::ApprovalRequired {
                request_id,
                tool,
                summary,
                options,
                auto_approved,
                plan_approval,
                at,
                ..
            } => {
                if *auto_approved {
                    self.protocol(&format!("auto-approved (yolo) · {tool}"));
                    return vec![Change::Protocol];
                }
                let mut ch = self.close_streams();
                let card = ApprovalCard {
                    request_id: request_id.clone(),
                    tool: tool.clone(),
                    summary: summary.clone(),
                    explanation: None,
                    options: options.clone(),
                    plan_approval: *plan_approval,
                    // Plans are one-offs — never offer "always allow" for them.
                    allow_pattern: if *plan_approval {
                        None
                    } else {
                        allow_pattern_for(tool, summary)
                    },
                    resolution: None,
                };
                self.push(Role::Approval, Body::Approval(card), *at);
                ch.push(Change::Appended(self.entries.len() - 1));
                self.presence.signal(
                    Phase::Wait,
                    Patch {
                        last_tool: Some(tool.clone()),
                        note: Some(summary.clone()),
                        ..Default::default()
                    },
                    now,
                );
                ch.push(Change::Presence);
                ch
            }
            ControlEvent::ApprovalResolved {
                request_id,
                option_id,
                cancelled,
                ..
            } => {
                let resolution = if *cancelled {
                    "cancelled".to_string()
                } else {
                    option_id.clone().unwrap_or_else(|| "allowed".into())
                };
                let mut ch = Vec::new();
                if let Some(i) = self.find_approval(request_id) {
                    if let Body::Approval(a) = &mut self.entries[i].body {
                        a.resolution = Some(resolution);
                    }
                    ch.push(Change::Updated(i));
                }
                // Back to work: wait accepts a lower-ranked phase.
                self.presence.signal(
                    Phase::Think,
                    Patch {
                        note: Some("Approval resolved".into()),
                        ..Default::default()
                    },
                    now,
                );
                ch.push(Change::Presence);
                ch
            }
            ControlEvent::Raw { payload, .. } => self.on_raw(payload, now),
            ControlEvent::SchedulerJob { .. }
            | ControlEvent::McpChanged { .. }
            | ControlEvent::MemoryUpdated { .. } => Vec::new(),
        }
    }

    // ── event handlers ──────────────────────────────────────────────────

    fn on_agent_message(&mut self, raw: &str, now: Instant) -> Vec<Change> {
        if raw.is_empty() {
            return Vec::new();
        }
        if is_noise_agent_text(raw)
            || raw.starts_with('🧠')
            || raw.starts_with('📜')
            || raw.starts_with('⚙')
            || raw.starts_with("wrote ")
        {
            self.protocol(raw);
            let mut ch = vec![Change::Protocol];
            if self.presence.turn_active() {
                let phase = self.presence.phase;
                self.presence.signal(
                    phase,
                    Patch {
                        note: Some(clip(raw, 80)),
                        ..Default::default()
                    },
                    now,
                );
                ch.push(Change::Presence);
            }
            return ch;
        }
        // Strip ONLY the marker — trimming whitespace after it welded
        // streamed thoughts into "Ihaveagoodpicture…".
        let (role, text) = match raw.strip_prefix('💭') {
            Some(rest) => (Role::Thought, rest),
            None => (Role::Agent, raw),
        };
        let mut ch = self.stream(role, text);
        let n = text.chars().count();
        match role {
            Role::Thought => self.presence.signal(
                Phase::Think,
                Patch {
                    add_thought_chars: n,
                    ..Default::default()
                },
                now,
            ),
            _ => self.presence.signal(
                Phase::Reply,
                Patch {
                    add_reply_chars: n,
                    ..Default::default()
                },
                now,
            ),
        }
        ch.push(Change::Presence);
        ch
    }

    fn on_tool_call(&mut self, te: &ToolCallEvent, now: Instant) -> Vec<Change> {
        let mut ch = self.close_streams();
        let status = format!("{:?}", te.status).to_lowercase();
        let terminal = tool_status_terminal(&status);
        // Plan-presenting tools already rendered their plan via plan_doc —
        // don't also dump the raw JSON.
        let is_plan_tool =
            te.tool.to_lowercase().contains("plan") || te.args_summary.contains("\"plan\":");
        if !is_plan_tool {
            let mut row = ToolRow {
                tool_id: te.id.clone(),
                name: te.tool.clone(),
                status: status.clone(),
                args: clip_chars(&te.args_summary, 2000),
                result: te.result_summary.as_deref().map(|r| clip_chars(r, 800)),
            };
            // One row per tool call: later events update it in place.
            match self.find_tool(&te.id) {
                Some(i) => {
                    // ACP status-only updates omit the original name/input.
                    if let Body::Tool(previous) = &self.entries[i].body {
                        if row.name == "tool" || row.name.is_empty() { row.name.clone_from(&previous.name); }
                        if row.args.is_empty() { row.args.clone_from(&previous.args); }
                    }
                    self.entries[i].body = Body::Tool(row);
                    self.entries[i].at = te.at;
                    ch.push(Change::Updated(i));
                }
                None => {
                    self.push(Role::Tool, Body::Tool(row), te.at);
                    ch.push(Change::Appended(self.entries.len() - 1));
                }
            }
        }

        if terminal {
            if self.open_tools.remove(&te.id) {
                self.presence.tool_finished(&te.tool, &status, now);
            } else {
                // Late terminal update without a prior start — status only.
                let phase = if self.presence.phase == Phase::Idle {
                    Phase::Tools
                } else {
                    self.presence.phase
                };
                self.presence.signal(
                    phase,
                    Patch {
                        last_tool: Some(te.tool.clone()),
                        last_tool_status: Some(status.clone()),
                        tools_active: Some(self.open_tools.len()),
                        ..Default::default()
                    },
                    now,
                );
            }
            self.presence.tools_active = self.open_tools.len();
        } else if !self.open_tools.contains(&te.id) {
            self.open_tools.insert(te.id.clone());
            self.presence.tool_started(&te.tool, now);
            self.presence.note = clip(&te.args_summary, 60);
            self.presence.tools_active = self.open_tools.len();
        } else {
            self.presence.signal(
                Phase::Tools,
                Patch {
                    last_tool: Some(te.tool.clone()),
                    last_tool_status: Some(status.clone()),
                    note: Some(clip(&te.args_summary, 60)),
                    tools_active: Some(self.open_tools.len()),
                    ..Default::default()
                },
                now,
            );
        }
        ch.push(Change::Presence);
        ch
    }

    fn on_status(&mut self, status: SessionStatus, now: Instant) -> Vec<Change> {
        self.status = Some(status);
        let mut ch = vec![Change::Status(status)];
        self.protocol(&format!("status → {status:?}"));
        match status {
            SessionStatus::WaitingApproval => {
                self.presence.signal(
                    Phase::Wait,
                    Patch {
                        note: Some("Waiting for approval".into()),
                        ..Default::default()
                    },
                    now,
                );
            }
            SessionStatus::Failed => {
                ch.extend(self.close_streams());
                self.sweep_tools("failed", &mut ch);
                self.end_turn(Phase::Error, "failed", now);
            }
            SessionStatus::Cancelled | SessionStatus::Cancelling => {
                ch.extend(self.close_streams());
                self.sweep_tools("cancelled", &mut ch);
                self.end_turn(Phase::Error, "Cancelled", now);
            }
            SessionStatus::Idle | SessionStatus::Completed => {
                ch.extend(self.close_streams());
                // The turn is over: anything still "running" finished without
                // a terminal update from the agent.
                self.sweep_tools("completed", &mut ch);
                let p = &self.presence;
                if p.phase == Phase::Error { return ch; }
                if p.turn_active() || p.reply_chars > 0 || p.tool_count > 0 {
                    self.open_tools.clear();
                    self.end_turn(Phase::Done, "Turn finished", now);
                    ch.push(Change::Boom);
                } else {
                    self.presence.signal(Phase::Idle, Patch::default(), now);
                }
            }
            SessionStatus::Running => {
                if !self.presence.turn_active() {
                    self.presence.signal(
                        Phase::Think,
                        Patch {
                            note: Some("Session running".into()),
                            ..Default::default()
                        },
                        now,
                    );
                }
            }
            SessionStatus::Starting | SessionStatus::Recovering => {}
        }
        ch.push(Change::Presence);
        ch
    }

    fn on_raw(&mut self, payload: &Value, now: Instant) -> Vec<Change> {
        let channel = str_at(payload, "channel").unwrap_or_default();
        match channel.as_str() {
            "explain" => self.on_explain(payload),
            "plan_doc" => {
                let Some(text) = str_at(payload, "text") else {
                    return Vec::new();
                };
                let mut ch = self.close_streams();
                self.push(
                    Role::Plan,
                    Body::Plan(PlanDoc {
                        title: None,
                        markdown: text,
                    }),
                    Utc::now(),
                );
                ch.push(Change::Appended(self.entries.len() - 1));
                ch
            }
            "thread" if str_at(payload, "kind").as_deref() == Some("model_switch") => {
                str_at(payload, "line").map(|line| self.note_system(&line)).unwrap_or_default()
            }
            "thread" if str_at(payload, "kind").as_deref() == Some("label") => {
                match str_at(payload, "label") {
                    Some(l) if !l.is_empty() => {
                        self.label = Some(l.clone());
                        vec![Change::Label(l)]
                    }
                    _ => Vec::new(),
                }
            }
            "image" => {
                let Some(data) = str_at(payload, "data") else { return Vec::new() };
                let mime = str_at(payload, "mimeType").unwrap_or_else(|| "image/png".into());
                let mut ch = self.close_streams();
                let e = self.push(Role::Agent, Body::Text(String::new()), Utc::now());
                e.images.push(ImageAttachment { mime_type: mime, data, name: None });
                ch.push(Change::Appended(self.entries.len() - 1));
                ch
            }
            "usage" => {
                let n = payload
                    .get("totalTokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if n == 0 {
                    return Vec::new();
                }
                self.context_tokens = Some(n);
                if self.presence.turn_active() || self.presence.phase == Phase::Idle {
                    let phase = if self.presence.phase == Phase::Idle {
                        Phase::Think
                    } else {
                        self.presence.phase
                    };
                    self.presence.signal(
                        phase,
                        Patch {
                            context_tokens: Some(n),
                            ..Default::default()
                        },
                        now,
                    );
                }
                vec![Change::Presence]
            }
            "term" => {
                let Some(line) = str_at(payload, "line") else {
                    return Vec::new();
                };
                self.protocol(&line);
                let mut ch = vec![Change::Protocol];
                if self.presence.turn_active() {
                    let phase = self.presence.phase;
                    self.presence.signal(
                        phase,
                        Patch {
                            note: Some(clip(&line, 80)),
                            ..Default::default()
                        },
                        now,
                    );
                    ch.push(Change::Presence);
                }
                ch
            }
            _ => {
                // Generic agent text buried in an ACP notification.
                let maybe = payload
                    .pointer("/update/content/text")
                    .or_else(|| payload.pointer("/content/text"))
                    .or_else(|| payload.get("text"))
                    .or_else(|| payload.get("message"))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                match maybe {
                    Some(t) if !t.trim().is_empty() && !is_noise_agent_text(&t) => {
                        let mut ch = self.stream(Role::Agent, &t);
                        self.presence.signal(
                            Phase::Reply,
                            Patch {
                                add_reply_chars: t.chars().count(),
                                ..Default::default()
                            },
                            now,
                        );
                        ch.push(Change::Presence);
                        ch
                    }
                    _ => {
                        let dump = payload.to_string();
                        if dump != "{}" && dump != "null" {
                            self.protocol(&clip_chars(&dump, 400));
                            return vec![Change::Protocol];
                        }
                        Vec::new()
                    }
                }
            }
        }
    }

    fn on_explain(&mut self, payload: &Value) -> Vec<Change> {
        let kind = str_at(payload, "kind").unwrap_or_else(|| "tick".into());
        if kind == "pending" {
            self.explain_pending = true;
            return vec![Change::Explain];
        }
        self.explain_pending = false;
        let text = str_at(payload, "text").unwrap_or_default();
        let text = text.trim().to_string();
        if text.is_empty() {
            return Vec::new();
        }
        let request_id = str_at(payload, "requestId");
        let at = str_at(payload, "at")
            .and_then(|s| s.parse::<DateTime<Utc>>().ok())
            .unwrap_or_else(Utc::now);
        self.explanations.push(Explanation {
            text: text.clone(),
            kind: kind.clone(),
            request_id: request_id.clone(),
            at,
        });
        if self.explanations.len() > MAX_EXPLANATIONS {
            let extra = self.explanations.len() - MAX_EXPLANATIONS;
            self.explanations.drain(..extra);
        }
        let mut ch = vec![Change::Explain];
        // Approval explanations also land under the matching card.
        if kind == "approval" {
            if let Some(rid) = request_id {
                if let Some(i) = self.find_approval(&rid) {
                    if let Body::Approval(a) = &mut self.entries[i].body {
                        a.explanation = Some(text);
                    }
                    ch.push(Change::Updated(i));
                }
            }
        }
        ch
    }

    // ── primitives ──────────────────────────────────────────────────────

    fn push(&mut self, role: Role, body: Body, at: DateTime<Utc>) -> &mut Entry {
        self.next_id += 1;
        self.entries.push(Entry {
            id: self.next_id,
            at,
            role,
            body,
            streaming: false,
            images: Vec::new(),
        });
        self.entries.last_mut().expect("just pushed")
    }

    /// Append a streaming chunk, coalescing into the open block of the same
    /// role (looking back past tool/plan rows that landed mid-response).
    fn stream(&mut self, role: Role, text: &str) -> Vec<Change> {
        let len = self.entries.len();
        for hops in 0..STREAM_LOOKBACK.min(len) {
            let i = len - 1 - hops;
            let entry = &mut self.entries[i];
            if entry.role == role && entry.streaming {
                if let Body::Text(body) = &mut entry.body {
                    if body.len() > ROTATE_STREAM_AT {
                        entry.streaming = false;
                        break;
                    }
                    body.push_str(text);
                    entry.at = Utc::now();
                    return vec![Change::Streamed {
                        index: i,
                        delta: text.to_string(),
                    }];
                }
            }
            if matches!(entry.role, Role::Tool | Role::Plan) {
                continue;
            }
            break;
        }
        let e = self.push(role, Body::Text(text.to_string()), Utc::now());
        e.streaming = true;
        let mut ch = vec![Change::Streamed {
            index: self.entries.len() - 1,
            delta: text.to_string(),
        }];
        ch[0] = Change::Appended(self.entries.len() - 1);
        ch.extend(self.trim());
        ch
    }

    /// Non-stream content after a stream closes the open block(s).
    fn close_streams(&mut self) -> Vec<Change> {
        let len = self.entries.len();
        let start = len.saturating_sub(STREAM_LOOKBACK);
        let mut ch = Vec::new();
        for i in start..len {
            if self.entries[i].streaming {
                self.entries[i].streaming = false;
                ch.push(Change::Updated(i));
            }
        }
        ch
    }

    fn trim(&mut self) -> Vec<Change> {
        if self.entries.len() > MAX_ENTRIES {
            let count = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..count);
            return vec![Change::Trimmed { count }];
        }
        Vec::new()
    }

    fn protocol(&mut self, line: &str) {
        self.protocol_log.push_back(line.to_string());
        while self.protocol_log.len() > MAX_PROTOCOL_LINES {
            self.protocol_log.pop_front();
        }
    }

    fn find_tool(&self, tool_id: &str) -> Option<usize> {
        self.entries
            .iter()
            .enumerate()
            .rev()
            .take(12)
            .find(|(_, e)| matches!(&e.body, Body::Tool(t) if t.tool_id == tool_id))
            .map(|(i, _)| i)
    }

    fn find_approval(&self, request_id: &str) -> Option<usize> {
        if request_id.is_empty() {
            return None;
        }
        self.entries
            .iter()
            .enumerate()
            .rev()
            .find(|(_, e)| matches!(&e.body, Body::Approval(a) if a.request_id == request_id))
            .map(|(i, _)| i)
    }

    /// Mark every non-terminal tool row with `final_status`.
    fn sweep_tools(&mut self, final_status: &str, ch: &mut Vec<Change>) {
        for (i, e) in self.entries.iter_mut().enumerate() {
            if let Body::Tool(t) = &mut e.body {
                if !t.is_terminal() {
                    t.status = final_status.to_string();
                    ch.push(Change::Updated(i));
                }
            }
        }
        self.open_tools.clear();
    }

    fn end_turn(&mut self, phase: Phase, note: &str, now: Instant) {
        self.presence.signal(
            phase,
            Patch {
                note: Some(note.to_string()),
                tools_active: Some(0),
                last_tool_status: Some(if phase == Phase::Error {
                    "failed".into()
                } else {
                    "completed".into()
                }),
                ..Default::default()
            },
            now,
        );
        self.open_tools.clear();
    }
}

// ── helpers ─────────────────────────────────────────────────────────────

pub fn tool_status_terminal(status: &str) -> bool {
    let st = status.to_ascii_lowercase();
    ["complete", "done", "success", "fail", "error", "denied", "reject", "cancel"]
        .iter()
        .any(|k| st.contains(k))
}

/// Chatter the ACP layer emits that is not agent speech.
pub fn is_noise_agent_text(text: &str) -> bool {
    let t = text.trim().to_ascii_lowercase();
    t.starts_with("prompt sent")
        || t == "turn complete"
        || t.starts_with("still generating after")
        || t.starts_with("[local/mock]")
        || starts_with_status_glyph(text)
}

/// Backend status breadcrumbs are prefixed with a pictograph (🌱 worktree,
/// 📜 history, 🧠 brain, ◈ memory, ⚙ …). They belong in the protocol log,
/// not the conversation.
pub fn starts_with_status_glyph(text: &str) -> bool {
    match text.trim_start().chars().next() {
        // 💭 marks a thought chunk, which is agent content.
        Some('💭') => false,
        Some(c) => {
            let u = c as u32;
            (0x1F300..=0x1FAFF).contains(&u)
                || (0x2600..=0x27BF).contains(&u)
                || (0x2B00..=0x2BFF).contains(&u)
                || matches!(c, '◈' | '→' | '⟳' | '⎇' | '⚠')
        }
        None => false,
    }
}

/// Rule an "always allow" button installs: narrow enough to be safe, broad
/// enough to stop the repeat asks (`Bash(cargo test *)`).
pub fn allow_pattern_for(tool: &str, summary: &str) -> Option<String> {
    let name = tool.trim();
    if name.is_empty() {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    let commandish = ["bash", "shell", "terminal", "exec", "run"]
        .iter()
        .any(|k| lower.contains(k));
    if commandish {
        if let Some((_, rest)) = summary.split_once(':') {
            let line = rest.lines().next().unwrap_or("").trim();
            let words: Vec<&str> = line.split_whitespace().collect();
            if !words.is_empty() {
                let take = if matches!(words[0], "git" | "cargo" | "npm") { 2 } else { 1 };
                let head: Vec<&str> = words.iter().take(take).copied().collect();
                return Some(format!("{name}({} *)", head.join(" ")));
            }
        }
    }
    Some(format!("{name}(*)"))
}

fn plan_from_update(ev: &PlanUpdateEvent) -> PlanDoc {
    let mut md = String::new();
    for s in &ev.steps {
        let mark = match s.status.as_str() {
            "completed" | "done" => "x",
            _ => " ",
        };
        md.push_str(&format!("- [{mark}] {}\n", s.description));
    }
    PlanDoc {
        title: ev.title.clone(),
        markdown: md,
    }
}

/// Persisted prompt rows may carry an image marker appended by `send_prompt`;
/// today they are plain text, so this only splits if a marker is present.
fn split_prompt_row(body: &str) -> (String, Vec<ImageAttachment>) {
    (body.to_string(), Vec::new())
}

fn str_at(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(str::to_string)
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn clip(text: &str, n: usize) -> String {
    let t: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    clip_chars(&t, n)
}

fn clip_chars(text: &str, n: usize) -> String {
    if text.chars().count() <= n {
        text.to_string()
    } else {
        text.chars().take(n).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use grok_events::ToolCallStatus;
    use uuid::Uuid;

    fn sid() -> Uuid {
        Uuid::nil()
    }

    fn msg(text: &str) -> ControlEvent {
        ControlEvent::AgentMessage {
            session_id: sid(),
            text: text.into(),
            at: Utc::now(),
        }
    }

    fn tool(id: &str, name: &str, status: ToolCallStatus, args: &str) -> ControlEvent {
        ControlEvent::ToolCall {
            session_id: sid(),
            event: ToolCallEvent {
                id: id.into(),
                tool: name.into(),
                args_summary: args.into(),
                status,
                result_summary: None,
                at: Utc::now(),
            },
        }
    }

    fn status(s: SessionStatus) -> ControlEvent {
        ControlEvent::SessionStatusChanged {
            session_id: sid(),
            status: s,
            at: Utc::now(),
        }
    }

    #[test]
    fn failed_cancelled_and_completed_turns_freeze_elapsed_and_retry_resets_it() {
        for event in [status(SessionStatus::Failed), status(SessionStatus::Cancelled), ControlEvent::SessionCompleted { session_id: sid(), at: Utc::now() }] {
            let start = Instant::now(); let mut t = Thread::new();
            t.note_prompt("hello", vec![], start);
            t.apply(&event, start + Duration::from_secs(4));
            assert_eq!(t.presence.elapsed(start + Duration::from_secs(90)), Some(Duration::from_secs(4)));
            t.note_prompt("retry", vec![], start + Duration::from_secs(100));
            assert_eq!(t.presence.elapsed(start + Duration::from_secs(102)), Some(Duration::from_secs(2)));
            t.note_failure("connection failed", start + Duration::from_secs(103));
            t.apply(&status(SessionStatus::Idle), start + Duration::from_secs(104));
            assert_eq!(t.presence.phase, Phase::Error);
            assert_eq!(t.presence.elapsed(start + Duration::from_secs(200)), Some(Duration::from_secs(3)));
        }
    }

    #[test]
    fn model_switch_notice_is_visible_in_the_live_transcript() {
        let mut t = Thread::new();
        t.apply(&ControlEvent::Raw { session_id: Some(sid()), payload: serde_json::json!({
            "channel":"thread", "kind":"model_switch", "line":"Switched model: Grok → Astra"
        }) }, Instant::now());
        assert_eq!(t.entries.len(), 1);
        assert_eq!(t.entries[0].role, Role::System);
        assert_eq!(t.entries[0].text(), Some("Switched model: Grok → Astra"));
    }

    #[test]
    fn sparse_image_tool_updates_keep_identity_and_arguments() {
        let mut t = Thread::new(); let now = Instant::now();
        t.apply(&tool("image-1", "image_gen", ToolCallStatus::Running, r#"{"prompt":"Lake"}"#), now);
        for status in [ToolCallStatus::Running, ToolCallStatus::Completed] {
            t.apply(&tool("image-1", "tool", status, ""), now);
            let Body::Tool(row) = &t.entries[0].body else { panic!() };
            assert_eq!(row.name, "image_gen");
            assert_eq!(row.args, r#"{"prompt":"Lake"}"#);
        }
        let Body::Tool(row) = &t.entries[0].body else { panic!() };
        assert!(row.is_terminal());
    }

    #[test]
    fn coalesces_agent_chunks_and_splits_thoughts() {
        let now = Instant::now();
        let mut t = Thread::new();
        t.note_prompt("hi", vec![], now);
        let ch = t.apply(&msg("💭I have"), now);
        assert!(matches!(ch[0], Change::Appended(1)));
        let ch = t.apply(&msg("💭 a plan"), now);
        assert_eq!(
            ch[0],
            Change::Streamed {
                index: 1,
                delta: " a plan".into()
            }
        );
        assert_eq!(t.entries[1].text(), Some("I have a plan"));
        assert_eq!(t.entries[1].role, Role::Thought);
        t.apply(&msg("Hello"), now);
        t.apply(&msg(" world"), now);
        assert_eq!(t.entries.len(), 3);
        assert_eq!(t.entries[2].text(), Some("Hello world"));
        assert_eq!(t.entries[2].role, Role::Agent);
        assert_eq!(t.presence.phase, Phase::Reply);
        assert_eq!(t.presence.reply_chars, 11);
        assert_eq!(t.presence.thought_chars, 13);
    }

    #[test]
    fn tool_rows_update_in_place_and_drive_presence() {
        let now = Instant::now();
        let mut t = Thread::new();
        t.note_prompt("run tests", vec![], now);
        t.apply(&msg("Sure."), now);
        t.apply(&tool("t1", "Bash", ToolCallStatus::Running, "cmd: cargo test"), now);
        assert_eq!(t.entries.len(), 3);
        assert!(!t.entries[1].streaming, "tool call closes the stream");
        assert_eq!(t.presence.phase, Phase::Tools);
        assert_eq!(t.presence.tools_active, 1);
        let ch = t.apply(&tool("t1", "Bash", ToolCallStatus::Completed, "cmd: cargo test"), now);
        assert!(ch.contains(&Change::Updated(2)));
        assert_eq!(t.entries.len(), 3, "same tool id updates, never stacks");
        match &t.entries[2].body {
            Body::Tool(r) => assert_eq!(r.status, "completed"),
            _ => panic!(),
        }
        assert_eq!(t.presence.tools_active, 0);
        assert_eq!(t.presence.phase, Phase::Reply, "reply resumes after tool");
        // a tool call closes the open block: text after it is a new paragraph
        t.apply(&msg("Done."), now);
        assert_eq!(t.entries.len(), 4);
        assert_eq!(t.entries[3].text(), Some("Done."));
        assert!(t.entries[3].streaming);
    }

    #[test]
    fn plan_tools_are_not_dumped() {
        let now = Instant::now();
        let mut t = Thread::new();
        t.apply(&tool("p", "ExitPlanMode", ToolCallStatus::Running, "{\"plan\": \"x\"}"), now);
        assert!(t.entries.is_empty());
    }

    #[test]
    fn approval_lifecycle() {
        let now = Instant::now();
        let mut t = Thread::new();
        t.note_prompt("do it", vec![], now);
        t.apply(
            &ControlEvent::ApprovalRequired {
                session_id: sid(),
                request_id: "r1".into(),
                tool: "Bash".into(),
                summary: "Run: cargo test --all".into(),
                options: vec![],
                auto_approved: false,
                selected_option: None,
                plan_approval: false,
                at: Utc::now(),
            },
            now,
        );
        assert_eq!(t.presence.phase, Phase::Wait);
        let (i, card) = t.open_approvals().next().unwrap();
        assert_eq!(i, 1);
        assert_eq!(card.allow_pattern.as_deref(), Some("Bash(cargo test *)"));
        // narrator explains it
        t.apply(
            &ControlEvent::Raw {
                session_id: Some(sid()),
                payload: serde_json::json!({"channel":"explain","kind":"approval","requestId":"r1","text":"It wants to run the tests."}),
            },
            now,
        );
        match &t.entries[1].body {
            Body::Approval(a) => assert_eq!(a.explanation.as_deref(), Some("It wants to run the tests.")),
            _ => panic!(),
        }
        t.apply(
            &ControlEvent::ApprovalResolved {
                session_id: sid(),
                request_id: "r1".into(),
                option_id: Some("allow_once".into()),
                cancelled: false,
                at: Utc::now(),
            },
            now,
        );
        assert_eq!(t.open_approvals().count(), 0);
        assert_eq!(t.presence.phase, Phase::Think);
    }

    #[test]
    fn auto_approved_goes_to_protocol_log_only() {
        let now = Instant::now();
        let mut t = Thread::new();
        t.apply(
            &ControlEvent::ApprovalRequired {
                session_id: sid(),
                request_id: "r".into(),
                tool: "Read".into(),
                summary: "".into(),
                options: vec![],
                auto_approved: true,
                selected_option: None,
                plan_approval: false,
                at: Utc::now(),
            },
            now,
        );
        assert!(t.entries.is_empty());
        assert_eq!(t.protocol_log.len(), 1);
    }

    #[test]
    fn turn_end_booms_then_settles() {
        let now = Instant::now();
        let mut t = Thread::new();
        t.note_prompt("x", vec![], now);
        t.apply(&msg("y"), now);
        let ch = t.apply(&status(SessionStatus::Idle), now);
        assert!(ch.contains(&Change::Boom));
        assert_eq!(t.presence.phase, Phase::Done);
        t.settle(now);
        assert_eq!(t.presence.phase, Phase::Idle);
        // an idle status with no turn activity is silent
        let ch = t.apply(&status(SessionStatus::Idle), now);
        assert!(!ch.contains(&Change::Boom));
    }

    #[test]
    fn failure_sweeps_open_tools() {
        let now = Instant::now();
        let mut t = Thread::new();
        t.note_prompt("x", vec![], now);
        t.apply(&tool("t1", "Bash", ToolCallStatus::Running, "cmd: sleep"), now);
        t.apply(&status(SessionStatus::Failed), now);
        match &t.entries[1].body {
            Body::Tool(r) => assert_eq!(r.status, "failed"),
            _ => panic!(),
        }
        assert_eq!(t.presence.phase, Phase::Error);
        assert_eq!(t.presence.tools_active, 0);
    }

    #[test]
    fn raw_channels() {
        let now = Instant::now();
        let mut t = Thread::new();
        t.note_prompt("x", vec![], now);
        let ch = t.apply(
            &ControlEvent::Raw {
                session_id: Some(sid()),
                payload: serde_json::json!({"channel":"thread","kind":"label","label":"Fix auth"}),
            },
            now,
        );
        assert_eq!(ch, vec![Change::Label("Fix auth".into())]);
        t.apply(
            &ControlEvent::Raw {
                session_id: Some(sid()),
                payload: serde_json::json!({"channel":"usage","totalTokens": 1234}),
            },
            now,
        );
        assert_eq!(t.context_tokens, Some(1234));
        t.apply(
            &ControlEvent::Raw {
                session_id: Some(sid()),
                payload: serde_json::json!({"channel":"term","line":"[acp] session/update"}),
            },
            now,
        );
        assert_eq!(t.entries.len(), 1, "protocol lines never enter the transcript");
        assert_eq!(t.protocol_log.back().unwrap(), "[acp] session/update");
        t.apply(
            &ControlEvent::Raw {
                session_id: Some(sid()),
                payload: serde_json::json!({"channel":"plan_doc","text":"# Plan\n1. do"}),
            },
            now,
        );
        assert_eq!(t.entries[1].role, Role::Plan);
        t.apply(
            &ControlEvent::Raw {
                session_id: Some(sid()),
                payload: serde_json::json!({"update":{"content":{"text":"buried text"}}}),
            },
            now,
        );
        assert_eq!(t.entries[2].text(), Some("buried text"));
    }

    #[test]
    fn noise_never_enters_transcript() {
        let now = Instant::now();
        let mut t = Thread::new();
        t.apply(&msg("Prompt sent (1 chars)"), now);
        t.apply(&msg("turn complete"), now);
        t.apply(&msg("🧠 loaded brain"), now);
        assert!(t.entries.is_empty());
        assert_eq!(t.protocol_log.len(), 3);
    }

    #[test]
    fn hydrate_settles_tools_left_running() {
        // A saved copy whose tool rows never got a terminal status (the agent
        // replayed them as one-shot `tool_call`s) must not show "working".
        let rows = vec![
            TranscriptEntry { role: "tool".into(), body: r#"{"id":"a","tool":"Bash","status":"running","args":"ls"}"#.into(), at: "".into(), seq: 1 },
            TranscriptEntry { role: "tool".into(), body: r#"{"id":"b","tool":"Read","status":"pending","args":"f"}"#.into(), at: "".into(), seq: 2 },
        ];
        let mut t = Thread::new();
        t.hydrate(&rows);
        for e in &t.entries {
            match &e.body {
                Body::Tool(r) => assert_eq!(r.status, "completed"),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(t.open_tools.is_empty());
    }

    #[test]
    fn hydrates_persisted_rows() {
        let rows = vec![
            TranscriptEntry { role: "prompt".into(), body: "hello".into(), at: "2026-01-01T00:00:00Z".into(), seq: 1 },
            TranscriptEntry { role: "agent".into(), body: "hi".into(), at: "2026-01-01T00:00:01Z".into(), seq: 2 },
            TranscriptEntry { role: "tool".into(), body: r#"{"id":"t","tool":"Read","status":"completed","args":"f.rs","result":"ok"}"#.into(), at: "bad".into(), seq: 3 },
            TranscriptEntry { role: "approval".into(), body: "Bash — Run: rm -rf".into(), at: "".into(), seq: 4 },
            TranscriptEntry { role: "term".into(), body: "a\nb\n".into(), at: "".into(), seq: 5 },
            TranscriptEntry { role: "system".into(), body: "session completed".into(), at: "".into(), seq: 6 },
        ];
        let mut t = Thread::new();
        t.hydrate(&rows);
        assert_eq!(t.entries.len(), 5);
        assert_eq!(t.entries[0].role, Role::You);
        match &t.entries[2].body {
            Body::Tool(r) => {
                assert_eq!(r.name, "Read");
                assert_eq!(r.result.as_deref(), Some("ok"));
            }
            _ => panic!(),
        }
        match &t.entries[3].body {
            Body::Approval(a) => {
                assert_eq!(a.tool, "Bash");
                assert!(!a.is_open());
            }
            _ => panic!(),
        }
        assert_eq!(t.protocol_log.len(), 2);
        assert_eq!(t.entries[4].role, Role::System);
    }

    #[test]
    fn allow_patterns() {
        assert_eq!(allow_pattern_for("Bash", "Run: git status -s").as_deref(), Some("Bash(git status *)"));
        assert_eq!(allow_pattern_for("Bash", "Run: ls -la").as_deref(), Some("Bash(ls *)"));
        assert_eq!(allow_pattern_for("Read", "path: x").as_deref(), Some("Read(*)"));
        assert_eq!(allow_pattern_for("", "").as_deref(), None);
    }
}
