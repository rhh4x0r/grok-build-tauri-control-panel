//! Per-thread UI model: the pure [`bomb_core::transcript::Thread`] plus the
//! metadata row from the backend and per-entry render state.

use std::time::Instant;

use bomb_core::transcript::{Change, Thread};
use bomb_core::ControlEvent;
use gpui_kit::*;
use grok_persistence::ThreadDto;

pub struct ThreadModel {
    pub meta: ThreadDto,
    pub thread: Thread,
    /// Transcript rows have been loaded from SQLite at least once.
    pub hydrated: bool,
    pub loading: bool,
}

impl ThreadModel {
    pub fn new(meta: ThreadDto) -> Self {
        Self {
            meta,
            thread: Thread::new(),
            hydrated: false,
            loading: false,
        }
    }

    pub fn title(&self) -> String {
        self.thread
            .label
            .clone()
            .or_else(|| self.meta.label.clone())
            .unwrap_or_else(|| short_id(&self.meta.id))
    }

    pub fn apply(&mut self, ev: &ControlEvent, cx: &mut Context<Self>) -> Vec<Change> {
        let changes = self.thread.apply(ev, Instant::now());
        if !changes.is_empty() {
            cx.notify();
        }
        changes
    }
}

pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}
