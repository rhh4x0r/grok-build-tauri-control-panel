//! Broadcast event bus for the control panel backend.
//!
//! Fans out session lifecycle, tool calls, plan updates, and system events
//! to UI subscribers and internal services.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::sync::broadcast;
use tracing::debug;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum EventError {
    #[error("broadcast lag: {0}")]
    Lagged(u64),
    #[error("channel closed")]
    Closed,
}

pub type Result<T> = std::result::Result<T, EventError>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Starting,
    Idle,
    Running,
    WaitingApproval,
    Cancelling,
    Cancelled,
    Completed,
    Failed,
    Recovering,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlEvent {
    SessionCreated {
        session_id: Uuid,
        cwd: String,
        mode: String,
        at: DateTime<Utc>,
    },
    SessionStatusChanged {
        session_id: Uuid,
        status: SessionStatus,
        at: DateTime<Utc>,
    },
    SessionCancelled {
        session_id: Uuid,
        at: DateTime<Utc>,
    },
    SessionCompleted {
        session_id: Uuid,
        at: DateTime<Utc>,
    },
    ToolCall {
        session_id: Uuid,
        event: ToolCallEvent,
    },
    PlanUpdate {
        session_id: Uuid,
        event: PlanUpdateEvent,
    },
    AgentMessage {
        session_id: Uuid,
        text: String,
        at: DateTime<Utc>,
    },
    ApprovalRequired {
        session_id: Uuid,
        request_id: String,
        tool: String,
        summary: String,
        options: Vec<PermissionOptionInfo>,
        auto_approved: bool,
        selected_option: Option<String>,
        /// True when this approval presents a plan (enables the
        /// "code with a different model" handoff in the UI).
        #[serde(default)]
        plan_approval: bool,
        at: DateTime<Utc>,
    },
    ApprovalResolved {
        session_id: Uuid,
        request_id: String,
        option_id: Option<String>,
        cancelled: bool,
        at: DateTime<Utc>,
    },
    /// A prompt sent by a connected device, so other devices on the same thread see it.
    /// `origin` names the sending client; that client already shows the prompt and skips it.
    UserMessage {
        session_id: Uuid,
        text: String,
        origin: String,
        at: DateTime<Utc>,
    },
    Error {
        session_id: Option<Uuid>,
        message: String,
        at: DateTime<Utc>,
    },
    SchedulerJob {
        job_id: String,
        message: String,
        at: DateTime<Utc>,
    },
    McpChanged {
        name: String,
        enabled: bool,
        at: DateTime<Utc>,
    },
    MemoryUpdated {
        scope: String,
        at: DateTime<Utc>,
    },
    Raw {
        session_id: Option<Uuid>,
        payload: Value,
    },
}

/// One option offered by the agent in a `session/request_permission` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionOptionInfo {
    pub id: String,
    pub kind: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallEvent {
    pub id: String,
    pub tool: String,
    pub args_summary: String,
    pub status: ToolCallStatus,
    pub result_summary: Option<String>,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanUpdateEvent {
    pub plan_id: Option<String>,
    pub title: Option<String>,
    pub steps: Vec<PlanStep>,
    pub status: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub description: String,
    pub status: String,
}

// Heavy token streaming can burst thousands of events; a lagged receiver
// permanently loses transcript rows, so keep generous headroom.
const DEFAULT_CAPACITY: usize = 8192;

#[derive(Debug)]
pub struct EventBus {
    tx: broadcast::Sender<ControlEvent>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ControlEvent> {
        self.tx.subscribe()
    }

    pub fn sender(&self) -> broadcast::Sender<ControlEvent> {
        self.tx.clone()
    }

    pub fn emit(&self, event: ControlEvent) {
        debug!(?event, "emit");
        let _ = self.tx.send(event);
    }

    pub async fn emit_session_created(&self, session_id: Uuid, cwd: &str, mode: &str) {
        self.emit(ControlEvent::SessionCreated {
            session_id,
            cwd: cwd.to_string(),
            mode: mode.to_string(),
            at: Utc::now(),
        });
    }

    pub async fn emit_session_cancelled(&self, session_id: Uuid) {
        self.emit(ControlEvent::SessionCancelled {
            session_id,
            at: Utc::now(),
        });
    }

    pub async fn emit_status(&self, session_id: Uuid, status: SessionStatus) {
        self.emit(ControlEvent::SessionStatusChanged {
            session_id,
            status,
            at: Utc::now(),
        });
    }

    pub fn emit_error(&self, session_id: Option<Uuid>, message: impl Into<String>) {
        self.emit(ControlEvent::Error {
            session_id,
            message: message.into(),
            at: Utc::now(),
        });
    }

    pub fn emit_tool_call(&self, session_id: Uuid, event: ToolCallEvent) {
        self.emit(ControlEvent::ToolCall { session_id, event });
    }

    pub fn emit_plan_update(&self, session_id: Uuid, event: PlanUpdateEvent) {
        self.emit(ControlEvent::PlanUpdate { session_id, event });
    }

    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared handle used across crates.
pub type SharedEventBus = Arc<EventBus>;

pub fn shared_bus() -> SharedEventBus {
    Arc::new(EventBus::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn broadcast_reaches_subscriber() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let id = Uuid::new_v4();
        bus.emit_session_created(id, "/tmp", "acp").await;
        let ev = rx.recv().await.unwrap();
        match ev {
            ControlEvent::SessionCreated { session_id, cwd, .. } => {
                assert_eq!(session_id, id);
                assert_eq!(cwd, "/tmp");
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
