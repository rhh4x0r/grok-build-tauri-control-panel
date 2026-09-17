//! Per-thread UI model: the pure [`bomb_core::transcript::Thread`] plus the
//! metadata row from the backend and per-entry render state (markdown
//! entities that receive streamed deltas, expand/collapse state, timers).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bomb_core::presence::BOOM_HOLD;
use bomb_core::transcript::{Body, Change, Role, Thread};
use bomb_core::ControlEvent;
use gpui_kit::component::text::TextViewState;
use gpui_kit::*;
use grok_persistence::ThreadDto;

pub struct ThreadModel {
    pub meta: ThreadDto,
    pub thread: Thread,
    /// Transcript rows have been loaded from SQLite at least once.
    pub hydrated: bool,
    pub provider_modes: serde_json::Value,
    pub provider_commands: serde_json::Value,
    pub loading: bool,
    /// Markdown render state per entry id; the streaming tail gets `push_str`.
    pub markdown: HashMap<u64, Entity<TextViewState>>,
    /// Decoded attachment thumbnails per entry id.
    pub images: HashMap<u64, Vec<Arc<Image>>>,
    /// Entry ids the user expanded (thoughts, tool chips) or collapsed
    /// (tool groups are open by default while running).
    pub expanded: HashSet<u64>,
    pub collapsed_groups: HashSet<u64>,
    pub explain_open: bool,
    /// Bumped whenever the transcript should re-pin to its tail.
    pub tail_version: u64,
    ticking: bool,
    tick_task: Option<Task<()>>,
    boom_task: Option<Task<()>>,
}

impl ThreadModel {
    pub fn new(meta: ThreadDto) -> Self {
        Self {
            meta,
            thread: Thread::new(),
            hydrated: false,
            provider_modes: serde_json::Value::Null,
            provider_commands: serde_json::Value::Null,
            loading: false,
            markdown: HashMap::new(),
            images: HashMap::new(),
            expanded: HashSet::new(),
            collapsed_groups: HashSet::new(),
            explain_open: false,
            tail_version: 0,
            ticking: false,
            tick_task: None,
            boom_task: None,
        }
    }

    pub fn id(&self) -> String {
        self.meta.id.clone()
    }

    pub fn title(&self) -> String {
        self.thread
            .label
            .clone()
            .or_else(|| self.meta.label.clone())
            .unwrap_or_else(|| short_id(&self.meta.id))
    }

    /// Markdown state for an entry, created on first use.
    pub fn markdown_state(
        &mut self,
        entry_id: u64,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Entity<TextViewState> {
        if let Some(s) = self.markdown.get(&entry_id) {
            return s.clone();
        }
        let state = cx.new(|cx| TextViewState::markdown(text, cx).selectable(true));
        self.markdown.insert(entry_id, state.clone());
        state
    }

    /// Thumbnails for an entry's attachments, decoded once.
    pub fn images_for(&mut self, entry_id: u64) -> Vec<Arc<Image>> {
        if let Some(v) = self.images.get(&entry_id) {
            return v.clone();
        }
        use base64::Engine;
        let decoded: Vec<Arc<Image>> = self
            .thread
            .entries
            .iter()
            .find(|e| e.id == entry_id)
            .map(|e| {
                e.images
                    .iter()
                    .filter_map(|a| {
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(&a.data)
                            .ok()?;
                        let format = match a.mime_type.as_str() {
                            "image/png" => ImageFormat::Png,
                            "image/jpeg" => ImageFormat::Jpeg,
                            "image/webp" => ImageFormat::Webp,
                            "image/gif" => ImageFormat::Gif,
                            "image/svg+xml" => ImageFormat::Svg,
                            "image/bmp" => ImageFormat::Bmp,
                            _ => return None,
                        };
                        Some(Arc::new(Image::from_bytes(format, bytes)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.images.insert(entry_id, decoded.clone());
        decoded
    }

    pub fn toggle_expanded(&mut self, entry_id: u64, cx: &mut Context<Self>) {
        if !self.expanded.remove(&entry_id) {
            self.expanded.insert(entry_id);
        }
        cx.notify();
    }

    pub fn after_hydrate(&mut self, cx: &mut Context<Self>) {
        self.markdown.clear();
        self.images.clear();
        self.tail_version += 1;
        cx.notify();
    }

    pub fn apply(&mut self, ev: &ControlEvent, cx: &mut Context<Self>) -> Vec<Change> {
        if let ControlEvent::Raw { payload, .. } = ev {
            if payload.get("channel").and_then(|v| v.as_str()) == Some("provider_commands") {
                self.provider_commands = payload.clone();
                cx.notify();
            }
            if payload.get("channel").and_then(|v| v.as_str()) == Some("provider_modes") {
                if payload.get("backend") != self.provider_modes.get("backend") {
                    self.provider_modes = payload.clone();
                } else if let (Some(target), Some(source)) =
                    (self.provider_modes.as_object_mut(), payload.as_object())
                {
                    for (key, value) in source {
                        target.insert(key.clone(), value.clone());
                    }
                }
                cx.notify();
            }
        }
        let changes = self.thread.apply(ev, Instant::now());
        self.absorb(&changes, cx);
        changes
    }

    /// Record a local change set (e.g. from `note_prompt`).
    pub fn absorb(&mut self, changes: &[Change], cx: &mut Context<Self>) {
        if changes.is_empty() {
            return;
        }
        for ch in changes {
            match ch {
                Change::Streamed { index, delta } => {
                    if let Some(e) = self.thread.entries.get(*index) {
                        if let Some(state) = self.markdown.get(&e.id) {
                            let delta = delta.clone();
                            state.update(cx, |s, cx| s.push_str(&delta, cx));
                        }
                    }
                    self.tail_version += 1;
                }
                Change::Appended(_) => self.tail_version += 1,
                Change::Updated(index) => {
                    // A rewritten text body must be re-parsed, not appended.
                    if let Some(e) = self.thread.entries.get(*index) {
                        if let (Body::Text(t), Some(state)) = (&e.body, self.markdown.get(&e.id)) {
                            if matches!(e.role, Role::Agent | Role::Thought | Role::Plan) {
                                let t = t.clone();
                                state.update(cx, |s, cx| s.set_text(&t, cx));
                            }
                        }
                    }
                }
                Change::Trimmed { .. } => {
                    let live: HashSet<u64> = self.thread.entries.iter().map(|e| e.id).collect();
                    self.markdown.retain(|id, _| live.contains(id));
                    self.images.retain(|id, _| live.contains(id));
                }
                Change::Boom => self.schedule_settle(cx),
                Change::Presence => self.ensure_ticking(cx),
                _ => {}
            }
        }
        cx.notify();
    }

    fn schedule_settle(&mut self, cx: &mut Context<Self>) {
        self.boom_task = Some(cx.spawn(async move |weak, cx| {
            cx.background_executor().timer(BOOM_HOLD).await;
            let _ = weak.update(cx, |t, cx| {
                t.thread.settle(Instant::now());
                cx.notify();
            });
        }));
    }

    /// A 1s heartbeat while a turn is active so elapsed time and stall
    /// detection repaint. Exits on its own once the turn ends.
    fn ensure_ticking(&mut self, cx: &mut Context<Self>) {
        if self.ticking || !self.thread.presence.turn_active() {
            return;
        }
        self.ticking = true;
        self.tick_task = Some(cx.spawn(async move |weak, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let keep = weak
                .update(cx, |t, cx| {
                    let active = t.thread.presence.turn_active();
                    if !active {
                        t.ticking = false;
                    }
                    cx.notify();
                    active
                })
                .unwrap_or(false);
            if !keep {
                break;
            }
        }));
    }
}

pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}
