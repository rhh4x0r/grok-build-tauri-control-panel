//! Type-ahead: messages sent while the agent is mid-turn (or the thread is still
//! starting) wait here, per thread, and go out together as one prompt when the
//! turn ends. An ACP prompt runs as one turn and nothing can be added to it, so
//! this is the closest thing to typing into a running turn.
//!
//! Pure data, no clocks or I/O. The shell owns one [`PromptQueue`] per thread,
//! feeds it how turns end ([`TurnEnd::of`]) and asks [`PromptQueue::next`] what
//! to send whenever the agent might be free.

use std::path::PathBuf;

use grok_events::{ControlEvent, SessionStatus};

use crate::transcript::ImageAttachment;

/// One message waiting to be sent.
#[derive(Debug, Clone, PartialEq)]
pub struct Queued {
    pub id: u64,
    pub text: String,
    pub images: Vec<ImageAttachment>,
    /// Other attached files, by path and size. They go as a note in the prompt.
    pub files: Vec<(PathBuf, u64)>,
}

impl Queued {
    pub fn has_attachments(&self) -> bool {
        !self.images.is_empty() || !self.files.is_empty()
    }
}

/// How a turn ended, as far as the queue is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnEnd {
    Finished,
    Failed,
    Stopped,
}

impl TurnEnd {
    /// The turn end an event reports, if any. A bare `Idle` status is not one: a
    /// session reports it when it connects or reconnects, before any turn ran. A
    /// real turn ends with its prompt's answer (`turn_complete`), a failure or a stop.
    pub fn of(ev: &ControlEvent) -> Option<TurnEnd> {
        match ev {
            ControlEvent::Raw { payload, .. } => payload
                .get("turn_complete")
                .and_then(|v| v.as_bool())
                .map(|done| if done { TurnEnd::Finished } else { TurnEnd::Stopped }),
            ControlEvent::SessionStatusChanged { status, .. } => match status {
                SessionStatus::Completed => Some(TurnEnd::Finished),
                SessionStatus::Failed => Some(TurnEnd::Failed),
                SessionStatus::Cancelled | SessionStatus::Cancelling => Some(TurnEnd::Stopped),
                _ => None,
            },
            ControlEvent::SessionCompleted { .. } => Some(TurnEnd::Finished),
            ControlEvent::SessionCancelled { .. } => Some(TurnEnd::Stopped),
            ControlEvent::Error { session_id: Some(_), .. } => Some(TurnEnd::Failed),
            _ => None,
        }
    }
}

/// Why a queue is holding its messages instead of sending them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pause {
    /// The turn failed; sending into a broken session would only fail again.
    Failed,
    /// The user stopped the agent.
    Stopped,
}

/// Messages that go out as one turn. Each keeps its own bubble in the
/// transcript; the agent gets them as one prompt, like Claude Code does.
#[derive(Debug, Clone, PartialEq)]
pub struct Batch {
    pub messages: Vec<Queued>,
}

impl Batch {
    /// The prompt the agent receives: every message's text, in order.
    pub fn prompt(&self) -> String {
        self.messages
            .iter()
            .map(|m| m.text.trim())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    pub fn images(&self) -> Vec<ImageAttachment> {
        self.messages.iter().flat_map(|m| m.images.iter().cloned()).collect()
    }

    pub fn files(&self) -> Vec<(PathBuf, u64)> {
        self.messages.iter().flat_map(|m| m.files.iter().cloned()).collect()
    }
}

/// "Send now": one message waiting for the turn it stopped to wind down.
#[derive(Debug, Clone)]
struct Interrupt {
    message: Queued,
    /// The agent confirmed the stopped turn is over (its prompt came back).
    settled: bool,
    /// The stop has reached the thread, so its status no longer shows the old turn.
    stopped: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PromptQueue {
    items: Vec<Queued>,
    paused: Option<Pause>,
    /// Open in the composer: it stays in line but is held back until saved.
    editing: Option<u64>,
    interrupt: Option<Interrupt>,
    /// The last turn ended and nothing has been sent since.
    due: bool,
    next_id: u64,
}

impl PromptQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn items(&self) -> &[Queued] {
        &self.items
    }

    /// Nothing waiting, including a "Send now" message.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.interrupt.is_none()
    }

    pub fn paused(&self) -> Option<Pause> {
        self.paused
    }

    pub fn editing(&self) -> Option<u64> {
        self.editing
    }

    /// The "Send now" message still waiting for the stopped turn to end.
    pub fn sending_now(&self) -> Option<&Queued> {
        self.interrupt.as_ref().map(|i| &i.message)
    }

    /// Add a message to the back of the line; returns its id.
    pub fn push(&mut self, text: impl Into<String>, images: Vec<ImageAttachment>, files: Vec<(PathBuf, u64)>) -> u64 {
        self.next_id += 1;
        self.items.push(Queued { id: self.next_id, text: text.into(), images, files });
        self.next_id
    }

    /// Hold a message back while the user edits it.
    pub fn begin_edit(&mut self, id: u64) -> Option<&Queued> {
        let item = self.items.iter().find(|m| m.id == id)?;
        self.editing = Some(id);
        Some(item)
    }

    pub fn cancel_edit(&mut self) {
        self.editing = None;
    }

    /// Replace a message's text, keeping its place and attachments. Editing it
    /// down to nothing removes it. False when it is no longer queued.
    pub fn edit(&mut self, id: u64, text: impl Into<String>) -> bool {
        let text = text.into();
        if self.editing == Some(id) {
            self.editing = None;
        }
        let Some(item) = self.items.iter_mut().find(|m| m.id == id) else { return false };
        if text.trim().is_empty() && !item.has_attachments() {
            self.remove(id);
        } else {
            item.text = text;
        }
        true
    }

    pub fn remove(&mut self, id: u64) -> Option<Queued> {
        let ix = self.items.iter().position(|m| m.id == id)?;
        if self.editing == Some(id) {
            self.editing = None;
        }
        let item = self.items.remove(ix);
        // An empty queue has nothing to hold back.
        if self.items.is_empty() {
            self.paused = None;
        }
        Some(item)
    }

    /// Something was sent to the thread: a new turn is under way.
    pub fn turn_started(&mut self) {
        self.due = false;
    }

    /// A turn ended. A failure or a stop pauses whatever is waiting.
    pub fn on_turn_end(&mut self, end: TurnEnd) {
        if let Some(interrupt) = &mut self.interrupt {
            // Our own stop, on the way to sending a message now.
            interrupt.stopped = true;
            return;
        }
        match end {
            TurnEnd::Finished => self.due = true,
            TurnEnd::Failed | TurnEnd::Stopped => {
                self.due = false;
                if !self.items.is_empty() {
                    self.paused = Some(if end == TurnEnd::Failed { Pause::Failed } else { Pause::Stopped });
                }
            }
        }
    }

    /// Let a paused queue go: it sends as soon as the agent is free.
    pub fn resume(&mut self) {
        self.paused = None;
        self.due = true;
    }

    /// Pull a message out to send it now, stopping the current turn first. The
    /// stop that follows does not pause the rest of the queue. False if it is not
    /// queued or another "Send now" is already waiting.
    pub fn send_now(&mut self, id: u64) -> bool {
        if self.interrupt.is_some() {
            return false;
        }
        let Some(message) = self.remove(id) else { return false };
        self.interrupt = Some(Interrupt { message, settled: false, stopped: false });
        true
    }

    /// The agent confirmed the stopped turn is over.
    pub fn send_now_settled(&mut self) {
        if let Some(interrupt) = &mut self.interrupt {
            interrupt.settled = true;
        }
    }

    /// Stopping failed: put the message back at the front and hold the queue.
    pub fn send_now_failed(&mut self) {
        if let Some(interrupt) = self.interrupt.take() {
            self.items.insert(0, interrupt.message);
            self.paused = Some(Pause::Failed);
            self.due = false;
        }
    }

    /// A drained batch could not be sent (its files failed to upload): put it back
    /// at the front, in order, and hold the queue.
    pub fn requeue(&mut self, batch: Batch) {
        self.items.splice(0..0, batch.messages);
        self.paused = Some(Pause::Failed);
        self.due = false;
    }

    /// What to send now, given whether the agent is busy. A "Send now" message
    /// goes once its stopped turn is over; otherwise everything queued goes
    /// together after a finished turn (or a resume), except a message being edited.
    pub fn next(&mut self, busy: bool) -> Option<Batch> {
        if busy {
            return None;
        }
        if let Some(interrupt) = &self.interrupt {
            if !(interrupt.settled && interrupt.stopped) {
                return None;
            }
            let message = self.interrupt.take().map(|i| i.message)?;
            self.due = false;
            return Some(Batch { messages: vec![message] });
        }
        if self.paused.is_some() || !self.due {
            return None;
        }
        let editing = self.editing;
        let (held, messages): (Vec<_>, Vec<_>) = std::mem::take(&mut self.items).into_iter().partition(|m| Some(m.id) == editing);
        self.items = held;
        if messages.is_empty() {
            return None;
        }
        self.due = false;
        Some(Batch { messages })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn image(name: &str) -> ImageAttachment {
        ImageAttachment { mime_type: "image/png".into(), data: "AAAA".into(), name: Some(name.into()) }
    }

    fn texts(batch: &Batch) -> Vec<&str> {
        batch.messages.iter().map(|m| m.text.as_str()).collect()
    }

    #[test]
    fn enqueue_edit_and_remove() {
        let mut q = PromptQueue::new();
        let a = q.push("first", vec![], vec![]);
        let b = q.push("second", vec![], vec![]);
        let c = q.push("third", vec![], vec![]);
        assert_eq!(q.items().len(), 3);

        assert!(q.edit(b, "second, reworded"));
        assert_eq!(q.items()[1].text, "second, reworded", "an edit keeps its place");

        assert_eq!(q.remove(a).map(|m| m.text), Some("first".into()));
        assert!(q.remove(a).is_none());
        assert_eq!(q.items().iter().map(|m| m.id).collect::<Vec<_>>(), vec![b, c]);

        // Editing a text-only message down to nothing drops it.
        assert!(q.edit(c, "  "));
        assert_eq!(q.items().len(), 1);
        assert!(!q.edit(c, "gone"));
    }

    #[test]
    fn a_finished_turn_sends_everything_as_one_prompt() {
        let mut q = PromptQueue::new();
        q.push("look at the tests", vec![], vec![]);
        q.push("  and the docs  ", vec![], vec![]);
        assert!(q.next(false).is_none(), "nothing goes before a turn ends");

        q.on_turn_end(TurnEnd::Finished);
        assert!(q.next(true).is_none(), "nor while the agent is still busy");
        let batch = q.next(false).expect("drains after the turn");
        assert_eq!(texts(&batch), vec!["look at the tests", "  and the docs  "], "each message keeps its own bubble");
        assert_eq!(batch.prompt(), "look at the tests\n\nand the docs");
        assert!(q.is_empty());
        assert!(q.next(false).is_none());
    }

    #[test]
    fn a_new_turn_clears_a_stale_turn_end() {
        let mut q = PromptQueue::new();
        q.on_turn_end(TurnEnd::Finished);
        q.turn_started();
        q.push("queued during the new turn", vec![], vec![]);
        // A connect/reconnect can leave the agent looking free before the new turn ends.
        assert!(q.next(false).is_none());
        q.on_turn_end(TurnEnd::Finished);
        assert!(q.next(false).is_some());
    }

    #[test]
    fn failure_or_stop_pauses_until_resumed() {
        for (end, pause) in [(TurnEnd::Failed, Pause::Failed), (TurnEnd::Stopped, Pause::Stopped)] {
            let mut q = PromptQueue::new();
            q.push("next step", vec![], vec![]);
            q.on_turn_end(end);
            assert_eq!(q.paused(), Some(pause));
            // A later idle/finish does not let a paused queue go.
            q.on_turn_end(TurnEnd::Finished);
            assert!(q.next(false).is_none());

            q.resume();
            assert_eq!(q.paused(), None);
            assert!(q.next(true).is_none(), "resume still waits for a busy agent");
            assert_eq!(q.next(false).map(|b| b.prompt()), Some("next step".into()));
        }
    }

    #[test]
    fn an_empty_queue_never_pauses() {
        let mut q = PromptQueue::new();
        q.on_turn_end(TurnEnd::Failed);
        assert_eq!(q.paused(), None);
        let id = q.push("a", vec![], vec![]);
        q.on_turn_end(TurnEnd::Stopped);
        assert_eq!(q.paused(), Some(Pause::Stopped));
        q.remove(id);
        assert_eq!(q.paused(), None, "emptying the queue lifts the pause");
    }

    #[test]
    fn attachments_travel_with_their_message() {
        let mut q = PromptQueue::new();
        let a = q.push("see screenshot", vec![image("one.png")], vec![]);
        q.push("", vec![image("two.png")], vec![(PathBuf::from("/tmp/notes.txt"), 12)]);
        q.push("plain", vec![], vec![]);
        // Editing text leaves the images alone; an attachment-only message survives an empty edit.
        q.edit(a, "see this screenshot");
        let b = q.items()[1].id;
        assert!(q.edit(b, ""));
        assert_eq!(q.items().len(), 3);

        q.on_turn_end(TurnEnd::Finished);
        let batch = q.next(false).unwrap();
        assert_eq!(batch.prompt(), "see this screenshot\n\nplain");
        assert_eq!(batch.images().iter().filter_map(|i| i.name.as_deref()).collect::<Vec<_>>(), vec!["one.png", "two.png"]);
        assert_eq!(batch.files(), vec![(PathBuf::from("/tmp/notes.txt"), 12)]);
        assert_eq!(batch.messages[0].images.len(), 1);
    }

    #[test]
    fn a_message_being_edited_waits_for_the_next_turn() {
        let mut q = PromptQueue::new();
        let a = q.push("a", vec![], vec![]);
        q.push("b", vec![], vec![]);
        assert_eq!(q.begin_edit(a).map(|m| m.text.clone()), Some("a".into()));
        q.on_turn_end(TurnEnd::Finished);
        assert_eq!(q.next(false).map(|b| b.prompt()), Some("b".into()));
        assert_eq!(q.items().len(), 1);
        assert_eq!(q.editing(), Some(a));

        q.turn_started();
        q.edit(a, "a, finished");
        assert_eq!(q.editing(), None);
        q.on_turn_end(TurnEnd::Finished);
        assert_eq!(q.next(false).map(|b| b.prompt()), Some("a, finished".into()));
    }

    #[test]
    fn send_now_waits_for_the_stopped_turn_and_keeps_the_rest_queued() {
        let mut q = PromptQueue::new();
        q.push("later", vec![], vec![]);
        let now = q.push("urgent", vec![image("x.png")], vec![]);
        assert!(q.send_now(now));
        assert!(!q.send_now(now), "one at a time");
        assert_eq!(q.sending_now().map(|m| m.text.as_str()), Some("urgent"));

        // Our own stop arrives; it must not pause the rest.
        q.on_turn_end(TurnEnd::Stopped);
        assert_eq!(q.paused(), None);
        assert!(q.next(false).is_none(), "not until the agent confirms the turn ended");
        q.send_now_settled();
        assert!(q.next(true).is_none());
        let batch = q.next(false).unwrap();
        assert_eq!(texts(&batch), vec!["urgent"]);
        assert_eq!(batch.images().len(), 1);

        // The rest go when the urgent turn finishes.
        q.turn_started();
        assert!(q.next(false).is_none());
        q.on_turn_end(TurnEnd::Finished);
        assert_eq!(q.next(false).map(|b| b.prompt()), Some("later".into()));
    }

    #[test]
    fn send_now_also_waits_for_the_stop_to_reach_the_thread() {
        let mut q = PromptQueue::new();
        let id = q.push("now", vec![], vec![]);
        q.send_now(id);
        q.send_now_settled();
        assert!(q.next(false).is_none());
        q.on_turn_end(TurnEnd::Stopped);
        assert!(q.next(false).is_some());
    }

    #[test]
    fn a_failed_send_now_goes_back_to_the_front_paused() {
        let mut q = PromptQueue::new();
        q.push("b", vec![], vec![]);
        let a = q.push("a", vec![], vec![]);
        q.send_now(a);
        q.send_now_failed();
        assert_eq!(q.items()[0].text, "a");
        assert_eq!(q.paused(), Some(Pause::Failed));
        assert!(q.sending_now().is_none());
    }

    #[test]
    fn an_unsent_batch_goes_back_in_front_paused() {
        let mut q = PromptQueue::new();
        q.push("a", vec![], vec![]);
        q.push("b", vec![], vec![]);
        q.on_turn_end(TurnEnd::Finished);
        let batch = q.next(false).unwrap();
        q.push("c", vec![], vec![]);
        q.requeue(batch);
        assert_eq!(q.items().iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), vec!["a", "b", "c"]);
        assert_eq!(q.paused(), Some(Pause::Failed));
    }

    #[test]
    fn turn_ends_are_read_from_events() {
        let sid = Uuid::new_v4();
        let status = |status| ControlEvent::SessionStatusChanged { session_id: sid, status, at: chrono::Utc::now() };
        let raw = |payload| ControlEvent::Raw { session_id: Some(sid), payload };
        assert_eq!(TurnEnd::of(&raw(serde_json::json!({"turn_complete": true}))), Some(TurnEnd::Finished));
        assert_eq!(TurnEnd::of(&raw(serde_json::json!({"turn_complete": false}))), Some(TurnEnd::Stopped));
        assert_eq!(TurnEnd::of(&raw(serde_json::json!({"channel": "term"}))), None);
        assert_eq!(TurnEnd::of(&status(SessionStatus::Failed)), Some(TurnEnd::Failed));
        assert_eq!(TurnEnd::of(&status(SessionStatus::Cancelled)), Some(TurnEnd::Stopped));
        assert_eq!(TurnEnd::of(&status(SessionStatus::Idle)), None, "connecting is not a turn ending");
        assert_eq!(TurnEnd::of(&status(SessionStatus::Running)), None);
    }
}
