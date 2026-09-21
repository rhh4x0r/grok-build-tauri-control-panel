//! The core's single event-bus subscriber.
//!
//! The journal saves each event to history, stamps it with a sequence number,
//! keeps a short backlog, and fans it out to attached clients (the desktop UI,
//! or remote devices through `bombd`). Persisting and numbering happen under
//! one lock, so a [`Journal::snapshot`] is always consistent with its `as_of`.
//!
//! `transcripts.seq` cannot serve as the cursor: streamed text is merged into
//! the last row in place, and most events create no row at all.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use grok_events::{ControlEvent, EventBus};
use grok_persistence::{Persistence, TranscriptEntry};
use tokio::sync::mpsc;
use tracing::{debug, warn};
use uuid::Uuid;

/// Events kept for reconnecting clients.
const BACKLOG: usize = 10_000;
/// Queue depth for a remote client before it is told to resync.
const CLIENT_QUEUE: usize = 16_384;

pub type Sequenced = (u64, ControlEvent);

enum Client {
    /// The in-process UI: never dropped.
    Local(mpsc::UnboundedSender<Sequenced>),
    /// A remote device: dropped when it falls behind, so the journal never blocks or loses events silently.
    Remote(mpsc::Sender<Sequenced>),
}

pub enum EventReceiver {
    Local(mpsc::UnboundedReceiver<Sequenced>),
    Remote(mpsc::Receiver<Sequenced>),
}

impl EventReceiver {
    /// `None` when the journal dropped this client (it fell behind) or shut down.
    pub async fn recv(&mut self) -> Option<Sequenced> {
        match self {
            Self::Local(rx) => rx.recv().await,
            Self::Remote(rx) => rx.recv().await,
        }
    }
}

pub struct Attached {
    /// Sequence at the moment of attaching.
    pub seq: u64,
    /// The client's cursor is older than the backlog; it must rebuild from snapshots.
    pub resync: bool,
    /// Events after the client's cursor, in order.
    pub backlog: Vec<Sequenced>,
    pub events: EventReceiver,
}

/// Saved history for one thread plus what is still waiting on the user.
pub struct Snapshot {
    /// Apply only events with a sequence above this.
    pub as_of: u64,
    pub rows: Vec<TranscriptEntry>,
    /// Open approval requests, with the ids and options history rows do not keep.
    pub pending_approvals: Vec<ControlEvent>,
}

struct Inner {
    seq: u64,
    backlog: VecDeque<Sequenced>,
    clients: Vec<Client>,
    pending_approvals: Vec<ControlEvent>,
}

pub struct Journal {
    inner: Mutex<Inner>,
    db: Arc<Persistence>,
    foundry: Arc<crate::foundry::FoundryService>,
}

impl Journal {
    /// Subscribe before returning so no event emitted afterwards is missed.
    pub fn start(bus: &EventBus, db: Arc<Persistence>, foundry: Arc<crate::foundry::FoundryService>) -> Arc<Self> {
        let journal = Arc::new(Self {
            inner: Mutex::new(Inner { seq: 0, backlog: VecDeque::new(), clients: Vec::new(), pending_approvals: Vec::new() }),
            db,
            foundry,
        });
        let mut sub = bus.subscribe();
        let this = Arc::downgrade(&journal);
        tokio::spawn(async move {
            loop {
                let event = match sub.recv().await {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(n, "event journal lagged behind the bus");
                        ControlEvent::Error { session_id: None, message: format!("{n} events dropped (the app fell behind)"), at: chrono::Utc::now() }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                let Some(journal) = this.upgrade() else { break };
                journal.record(event);
            }
            debug!("event journal closed");
        });
        journal
    }

    /// Save, number and deliver one event.
    pub fn record(&self, event: ControlEvent) {
        let session = session_of(&event).map(|id| id.to_string());
        // Foundry's helper sessions: transient ones leave no history, and none of them reach clients.
        let transient = session.as_deref().is_some_and(|id| self.foundry.transient_child(id));
        let internal = session.as_deref().is_some_and(|id| self.foundry.child(id));
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !transient {
            crate::services::persist_control_event(&self.db, &event);
        }
        if internal {
            return;
        }
        match &event {
            ControlEvent::ApprovalRequired { auto_approved: false, .. } => inner.pending_approvals.push(event.clone()),
            ControlEvent::ApprovalResolved { session_id, request_id, .. } => {
                inner.pending_approvals.retain(|p| !matches!(p, ControlEvent::ApprovalRequired { session_id: s, request_id: r, .. } if s == session_id && r == request_id));
            }
            // A finished or cancelled turn cannot still be waiting on an answer.
            ControlEvent::SessionCompleted { session_id, .. } | ControlEvent::SessionCancelled { session_id, .. } => {
                inner.pending_approvals.retain(|p| session_of(p) != Some(*session_id));
            }
            _ => {}
        }
        inner.seq += 1;
        let item = (inner.seq, event);
        inner.backlog.push_back(item.clone());
        if inner.backlog.len() > BACKLOG {
            inner.backlog.pop_front();
        }
        inner.clients.retain(|client| match client {
            Client::Local(tx) => tx.send(item.clone()).is_ok(),
            Client::Remote(tx) => tx.try_send(item.clone()).is_ok(),
        });
    }

    /// Attach the in-process UI.
    pub fn attach_local(&self) -> Attached {
        let (tx, rx) = mpsc::unbounded_channel();
        self.attach(None, Client::Local(tx), EventReceiver::Local(rx))
    }

    /// Attach a remote device that last applied `since`.
    pub fn attach_remote(&self, since: Option<u64>) -> Attached {
        let (tx, rx) = mpsc::channel(CLIENT_QUEUE);
        self.attach(since, Client::Remote(tx), EventReceiver::Remote(rx))
    }

    fn attach(&self, since: Option<u64>, client: Client, events: EventReceiver) -> Attached {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let seq = inner.seq;
        let oldest = inner.backlog.front().map(|(s, _)| *s).unwrap_or(seq + 1);
        let (resync, backlog) = match since {
            None => (false, Vec::new()),
            // A cursor from another core's lifetime, or older than the backlog, cannot be continued.
            Some(n) if n > seq || n + 1 < oldest => (true, Vec::new()),
            Some(n) => (false, inner.backlog.iter().filter(|(s, _)| *s > n).cloned().collect()),
        };
        inner.clients.push(client);
        Attached { seq, resync, backlog, events }
    }

    pub fn seq(&self) -> u64 {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).seq
    }

    /// History and open approvals for one thread, consistent with `as_of`.
    pub fn snapshot(&self, session: Uuid) -> Result<Snapshot, String> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let rows = self.db.transcript_entries(session).map_err(|e| e.to_string())?;
        let pending_approvals = inner.pending_approvals.iter().filter(|p| session_of(p) == Some(session)).cloned().collect();
        Ok(Snapshot { as_of: inner.seq, rows, pending_approvals })
    }
}

/// The session an event belongs to, if any.
pub fn session_of(event: &ControlEvent) -> Option<Uuid> {
    match event {
        ControlEvent::SessionCreated { session_id, .. }
        | ControlEvent::SessionStatusChanged { session_id, .. }
        | ControlEvent::SessionCancelled { session_id, .. }
        | ControlEvent::SessionCompleted { session_id, .. }
        | ControlEvent::ToolCall { session_id, .. }
        | ControlEvent::PlanUpdate { session_id, .. }
        | ControlEvent::AgentMessage { session_id, .. }
        | ControlEvent::ApprovalRequired { session_id, .. }
        | ControlEvent::ApprovalResolved { session_id, .. }
        | ControlEvent::UserMessage { session_id, .. } => Some(*session_id),
        ControlEvent::Error { session_id, .. } | ControlEvent::Raw { session_id, .. } => *session_id,
        ControlEvent::SchedulerJob { .. } | ControlEvent::McpChanged { .. } | ControlEvent::MemoryUpdated { .. } => None,
    }
}
