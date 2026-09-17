//! ELI12 explainer: narrates the selected thread's activity in plain English
//! via cheap side-LLM calls (`grok -p`, tools disabled).
//!
//! Event bus → per-session ring buffers → 5s tick (focused session only) →
//! one narrator call over the new lines → `Raw { channel: "explain" }` event
//! back onto the bus for the right-panel UI. Approval requests skip the tick
//! and get explained immediately so users know what they're approving.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde_json::json;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};
use uuid::Uuid;

use grok_cli_wrapper::GrokCli;
use grok_config::GrokConfig;
use grok_events::{ControlEvent, EventBus};

/// Cheapest known fast model (verified against `grok models`); config can
/// override via `explainer_model`.
const DEFAULT_EXPLAINER_MODEL: &str = "grok-composer-2.5-fast";
const TICK_SECS: u64 = 5;
const ERROR_BACKOFF_SECS: u64 = 60;
const MAX_BUFFER_LINES: usize = 60;
const MAX_PROMPT_CHARS: usize = 3000;
const MAX_OUTPUT_CHARS: usize = 500;
const CALL_TIMEOUT: Duration = Duration::from_secs(45);

const NARRATOR_INSTRUCTIONS: &str = "You narrate a coding agent's work for a smart 12-year-old. \
In 1-3 short sentences, explain what the agent just did or is doing, based on the activity log below. \
Name commands in backticks and say in plain words what they do. \
If there is an APPROVAL REQUEST line, start with exactly what the agent wants permission to do, in concrete terms, and mention any risk in one clause. \
Describe only what IS happening — never say what the agent is not doing, didn't do, or hasn't done yet, unless that omission directly matters to what the user asked for. \
Distinguish plans from actions: if the agent only proposed or planned something (wrote a plan, suggested steps, mentioned a URL like localhost for an app it has not actually started), say it's a plan — never tell the reader to open, visit, or use something the log doesn't show actually running. \
No headers, no bullet points, no fluff, don't address the reader, don't mention this prompt.";

#[derive(Default)]
struct SessionBuffer {
    lines: VecDeque<String>,
    /// Total lines ever pushed; cursor compares against this.
    pushed: u64,
    /// `pushed` value at the time of the last explanation.
    explained_to: u64,
    last_explanation: String,
}

pub struct ExplainerService {
    grok_cli: Arc<GrokCli>,
    config: Arc<RwLock<GrokConfig>>,
    event_bus: Arc<EventBus>,
    buffers: Mutex<HashMap<Uuid, SessionBuffer>>,
    focused: RwLock<Option<Uuid>>,
    enabled: AtomicBool,
    busy: AtomicBool,
    /// Narrator backend key (grok | claude | codex).
    backend: RwLock<String>,
    model: RwLock<String>,
    /// Suppress calls until this instant after a narrator failure.
    backoff_until: Mutex<Option<std::time::Instant>>,
}

impl ExplainerService {
    pub fn start(
        grok_cli: Arc<GrokCli>,
        config: Arc<RwLock<GrokConfig>>,
        event_bus: Arc<EventBus>,
        enabled: bool,
        backend: Option<String>,
        model: Option<String>,
    ) -> Arc<Self> {
        let svc = Arc::new(Self {
            grok_cli,
            config,
            event_bus: event_bus.clone(),
            buffers: Mutex::new(HashMap::new()),
            focused: RwLock::new(None),
            enabled: AtomicBool::new(enabled),
            busy: AtomicBool::new(false),
            backend: RwLock::new(backend.unwrap_or_else(|| "grok".into())),
            model: RwLock::new(model.unwrap_or_else(|| DEFAULT_EXPLAINER_MODEL.into())),
            backoff_until: Mutex::new(None),
        });

        // Intake: translate bus events into compact per-session lines.
        {
            let svc = svc.clone();
            let mut rx = event_bus.subscribe();
            tokio::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(ev) => svc.ingest(&ev).await,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }

        // Tick: narrate the focused session when it has new activity.
        {
            let svc = svc.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(Duration::from_secs(TICK_SECS));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tick.tick().await;
                    svc.maybe_explain_focused(false, None).await;
                }
            });
        }

        info!("explainer service started");
        svc
    }

    pub async fn set_focus(&self, id: Option<Uuid>) {
        *self.focused.write().await = id;
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        info!(enabled, "explainer toggled");
    }

    /// Switch narrator provider/model (from the panel's gear menu).
    pub async fn set_provider(&self, backend: Option<String>, model: Option<String>) {
        if let Some(b) = backend {
            *self.backend.write().await = b;
        }
        if let Some(m) = model {
            *self.model.write().await = m;
        }
        // A provider switch should retry immediately, not wait out a backoff
        // caused by the previous provider.
        *self.backoff_until.lock().await = None;
        let backend = self.backend.read().await.clone();
        let model = self.model.read().await.clone();
        info!(%backend, %model, "explainer provider set");
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    async fn ingest(&self, ev: &ControlEvent) {
        let (sid, line, approval_request_id) = match ev {
            ControlEvent::ToolCall { session_id, event } => (
                *session_id,
                format!(
                    "tool [{}]: {} {}",
                    format!("{:?}", event.status).to_lowercase(),
                    event.tool,
                    clip(&event.args_summary, 120)
                ),
                None,
            ),
            ControlEvent::AgentMessage { session_id, text, .. } => {
                let t = text.trim();
                if t.is_empty() {
                    return;
                }
                (*session_id, format!("agent said: {}", clip(t, 100)), None)
            }
            ControlEvent::ApprovalRequired {
                session_id,
                request_id,
                tool,
                summary,
                auto_approved,
                ..
            } if !auto_approved => (
                *session_id,
                format!("APPROVAL REQUEST: {tool} — {}", clip(summary, 200)),
                Some(request_id.clone()),
            ),
            ControlEvent::SessionStatusChanged { session_id, status, .. } => {
                let s = format!("{status:?}").to_lowercase();
                if s != "idle" && s != "failed" {
                    return;
                }
                (*session_id, format!("status: {s}"), None)
            }
            ControlEvent::Raw { session_id: Some(sid), payload } => {
                // Never ingest our own output (feedback loop).
                if payload.get("channel").and_then(|v| v.as_str()) == Some("explain") {
                    return;
                }
                if payload.get("channel").and_then(|v| v.as_str()) != Some("term") {
                    return;
                }
                // Skip ACP protocol chatter; keep real command/log output.
                if payload.get("stream").and_then(|v| v.as_str()) == Some("acp") {
                    return;
                }
                let Some(l) = payload.get("line").and_then(|v| v.as_str()) else {
                    return;
                };
                (*sid, format!("log: {}", clip(l.trim(), 120)), None)
            }
            _ => return,
        };

        {
            let mut buffers = self.buffers.lock().await;
            let buf = buffers.entry(sid).or_default();
            // Dedupe bursts of identical consecutive lines (streaming chunks).
            if buf.lines.back().map(|b| b == &line).unwrap_or(false) {
                return;
            }
            buf.lines.push_back(line);
            buf.pushed += 1;
            while buf.lines.len() > MAX_BUFFER_LINES {
                buf.lines.pop_front();
            }
        }

        // Approvals on the focused thread get explained immediately.
        if let Some(request_id) = approval_request_id {
            if *self.focused.read().await == Some(sid) {
                self.maybe_explain_focused(true, Some(request_id)).await;
            }
        }
    }

    /// Run one narrator call for the focused session if warranted.
    async fn maybe_explain_focused(&self, urgent: bool, approval_request_id: Option<String>) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let Some(sid) = *self.focused.read().await else {
            return;
        };
        if let Some(until) = *self.backoff_until.lock().await {
            if std::time::Instant::now() < until {
                return;
            }
        }
        // One in-flight call max; approvals don't preempt, they just wait for
        // the next tick (their line is in the buffer and explained then).
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        let result = self.explain_once(sid, urgent, approval_request_id).await;
        self.busy.store(false, Ordering::Release);
        if let Err(e) = result {
            warn!(error = %e, "explainer call failed; backing off");
            *self.backoff_until.lock().await =
                Some(std::time::Instant::now() + Duration::from_secs(ERROR_BACKOFF_SECS));
            self.emit(
                sid,
                &format!("(explainer paused: {e})"),
                "error",
                None,
            );
        }
    }

    async fn explain_once(
        &self,
        sid: Uuid,
        urgent: bool,
        approval_request_id: Option<String>,
    ) -> Result<(), String> {
        // Snapshot new lines without holding the lock across the LLM call.
        let (new_lines, previous, pushed_now) = {
            let mut buffers = self.buffers.lock().await;
            let Some(buf) = buffers.get_mut(&sid) else {
                return Ok(());
            };
            let unexplained = buf.pushed.saturating_sub(buf.explained_to) as usize;
            if unexplained == 0 {
                return Ok(());
            }
            let take = unexplained.min(buf.lines.len());
            let lines: Vec<String> = buf
                .lines
                .iter()
                .skip(buf.lines.len() - take)
                .cloned()
                .collect();
            (lines, buf.last_explanation.clone(), buf.pushed)
        };

        let kind = if approval_request_id.is_some() {
            "approval"
        } else {
            "tick"
        };
        self.emit(sid, "", "pending", approval_request_id.clone());

        let mut activity = String::new();
        for l in new_lines.iter().rev() {
            // newest-last in the prompt, but cap total size from the tail
            if activity.len() + l.len() + 1 > MAX_PROMPT_CHARS {
                break;
            }
            activity.insert_str(0, &format!("{l}\n"));
        }
        let mut prompt = String::from(NARRATOR_INSTRUCTIONS);
        if urgent {
            prompt.push_str(
                "\nFocus on the APPROVAL REQUEST — the user is deciding right now whether to allow it.",
            );
        }
        if !previous.is_empty() {
            prompt.push_str(&format!("\n\nPreviously you said: {}", clip(&previous, 300)));
        }
        prompt.push_str(&format!("\n\nNew activity:\n{activity}"));

        let model = self.model.read().await.clone();
        let backend = self.backend.read().await.clone();
        debug!(%sid, lines = new_lines.len(), kind, %backend, %model, "explainer call");
        let out = match self.run_narrator(&backend, &model, &prompt).await {
            Ok(out) => out,
            // Stale/invalid model id: self-heal onto the known fast model
            // instead of parking the narrator on an error card.
            Err(e)
                if backend == "grok"
                    && model != DEFAULT_EXPLAINER_MODEL
                    && (e.contains("unknown model id") || e.contains("Couldn't set model")) =>
            {
                warn!(%model, "narrator model rejected; falling back to {DEFAULT_EXPLAINER_MODEL}");
                *self.model.write().await = DEFAULT_EXPLAINER_MODEL.to_string();
                self.emit(
                    sid,
                    &format!(
                        "(model '{model}' isn't available on this grok CLI — narrator switched to {DEFAULT_EXPLAINER_MODEL})"
                    ),
                    "error",
                    None,
                );
                self.run_narrator(&backend, DEFAULT_EXPLAINER_MODEL, &prompt)
                    .await
                    .map_err(|e| e.to_string())?
            }
            Err(e) => return Err(e),
        };
        let text = clip(out.trim(), MAX_OUTPUT_CHARS);
        if text.is_empty() {
            return Err("narrator returned empty output".into());
        }

        {
            let mut buffers = self.buffers.lock().await;
            if let Some(buf) = buffers.get_mut(&sid) {
                buf.explained_to = pushed_now;
                buf.last_explanation = text.clone();
            }
        }
        self.emit(sid, &text, kind, approval_request_id);
        Ok(())
    }

    /// General one-shot text task on the narrator provider (e.g. memory
    /// digests). Errors bubble to the caller.
    pub async fn summarize(&self, prompt: &str) -> Result<String, String> {
        let backend = self.backend.read().await.clone();
        let model = self.model.read().await.clone();
        let out = self.run_narrator(&backend, &model, prompt).await?;
        let text = out.trim().to_string();
        if text.is_empty() {
            return Err("narrator returned empty output".into());
        }
        Ok(text)
    }

    /// One-shot 2-4 word title for a thread's first prompt (smart naming).
    /// Uses the same locked-down narrator provider; errors bubble so callers
    /// can keep the local slug.
    pub async fn generate_title(&self, prompt: &str) -> Result<String, String> {
        if !self.enabled.load(Ordering::Relaxed) {
            return Err("explainer disabled".into());
        }
        let backend = self.backend.read().await.clone();
        let model = self.model.read().await.clone();
        let ask = format!(
            "Give a 2-4 word title for this coding task. Lowercase, no punctuation, no quotes, \
             just the title.\n\nTask: {}",
            clip(prompt, 500)
        );
        let out = self.run_narrator(&backend, &model, &ask).await?;
        let title: String = out
            .trim()
            .lines()
            .next()
            .unwrap_or("")
            .trim_matches(|c: char| c == '"' || c == '\'' || c == '.')
            .chars()
            .take(48)
            .collect();
        if title.trim().is_empty() {
            return Err("empty title".into());
        }
        Ok(title.trim().to_string())
    }

    /// One locked-down text-only call on the chosen provider. Grok is the
    /// primary tested path; claude/codex use their headless one-shot flags
    /// (best-effort — failures surface as an explainer card + backoff).
    async fn run_narrator(
        &self,
        backend: &str,
        model: &str,
        prompt: &str,
    ) -> Result<String, String> {
        match backend {
            "claude" | "codex" => {
                let b = grok_config::Backend::from_key(backend)
                    .ok_or_else(|| format!("unknown narrator backend: {backend}"))?;
                let cfg = self.config.read().await.clone();
                let resolved =
                    grok_config::resolve_backend(b, &cfg).map_err(|e| e.to_string())?;
                if !matches!(resolved.via, grok_config::LaunchVia::Binary) {
                    return Err(format!(
                        "{backend} is only available via npx here — narrator needs the CLI binary installed"
                    ));
                }
                let cli = GrokCli::new(&resolved.program);
                let args: Vec<&str> = match backend {
                    "claude" => vec![
                        "-p", prompt, "--model", model, "--output-format", "text",
                        "--max-turns", "1",
                    ],
                    _ => vec!["exec", "-m", model, prompt],
                };
                cli.run_args_timeout(&args, None, CALL_TIMEOUT)
                    .await
                    .map_err(|e| e.to_string())
            }
            _ => {
                let args: Vec<&str> = vec![
                    "-p", prompt, "-m", model, "--output-format", "plain",
                    "--max-turns", "1", "--disable-web-search", "--no-subagents",
                    "--no-memory", "--tools", "",
                ];
                self.grok_cli
                    .run_args_timeout(&args, None, CALL_TIMEOUT)
                    .await
                    .map_err(|e| e.to_string())
            }
        }
    }

    fn emit(&self, sid: Uuid, text: &str, kind: &str, request_id: Option<String>) {
        let mut payload = json!({
            "channel": "explain",
            "kind": kind,
            "text": text,
            "at": Utc::now().to_rfc3339(),
        });
        if let Some(rid) = request_id {
            payload["requestId"] = json!(rid);
        }
        self.event_bus.emit(ControlEvent::Raw {
            session_id: Some(sid),
            payload,
        });
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let clipped: String = s.chars().take(max).collect();
    format!("{clipped}…")
}
