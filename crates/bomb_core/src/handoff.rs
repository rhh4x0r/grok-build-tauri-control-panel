//! Handoffs between threads, in any project: one thread's agent (or the person, with "Send to
//! thread…") passes something to another thread, which starts working on it.
//!
//! Agents do it with the `bomb` tools `find_threads`, `read_thread` and `send_to_thread`. A thread
//! may send only to threads the person @-mentioned in it (`note_mentions`, from the message's
//! "thread id" note), so an agent can't wander off messaging threads on its own.
//!
//! Both sides get a card: "Sent to …" in the sender, "From …" in the receiver, before the message
//! itself. A thread this core doesn't hold (it lives on a server, or on the Mac when this is a
//! server) is handed to the app as a `relay`, which passes it to the core that does.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use grok_events::ControlEvent;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::services;
use crate::state::AppState;

/// How long a delivery waits for a busy thread's turn to end before going anyway.
const WAIT_FOR_TURN: Duration = Duration::from_secs(30 * 60);
/// A received message's preview on its card.
const PREVIEW_CHARS: usize = 220;

/// Threads each thread's person has @-mentioned: the only ones its agent may send to.
#[derive(Default)]
pub struct Mentions(Mutex<HashMap<Uuid, HashSet<Uuid>>>);

impl Mentions {
    /// Remember the threads a message mentions (its "thread id <uuid>" note).
    pub fn note(&self, thread: Uuid, message: &str) {
        let found = mentioned_ids(message);
        if !found.is_empty() {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).entry(thread).or_default().extend(found);
        }
    }

    fn allows(&self, from: Uuid, to: Uuid) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).get(&from).is_some_and(|s| s.contains(&to))
    }
}

/// The thread ids a message's mention note names.
pub fn mentioned_ids(message: &str) -> Vec<Uuid> {
    message.match_indices("thread id ").filter_map(|(i, m)| message.get(i + m.len()..i + m.len() + 36)).filter_map(|s| Uuid::parse_str(s).ok()).collect()
}

fn preview(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > PREVIEW_CHARS { format!("{}…", flat.chars().take(PREVIEW_CHARS).collect::<String>()) } else { flat }
}

/// A thread's name as the thread list shows it.
pub fn title_of(state: &AppState, id: Uuid) -> Option<String> {
    let rec = state.persistence.get_session(id).ok()?;
    Some(services::extract_meta_string(&rec.metadata_json, "label").unwrap_or_else(|| "Untitled thread".into()))
}

/// Threads in any project matching `query` (title, project or branch words; empty = most recent).
pub async fn find_threads(state: &AppState, query: &str, except: Option<Uuid>) -> Result<Value, String> {
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_string).collect();
    let threads = services::list_threads(state).await?;
    let found: Vec<Value> = threads.into_iter()
        .filter(|t| except.is_none_or(|e| e.to_string() != t.id))
        .filter(|t| {
            let hay = format!("{} {} {}", t.label.clone().unwrap_or_default(), t.project_root.clone().unwrap_or_else(|| t.cwd.clone()), t.worktree.clone().unwrap_or_default()).to_lowercase();
            words.iter().all(|w| hay.contains(w))
        })
        .take(12)
        .map(|t| json!({
            "id": t.id,
            "title": t.label.clone().unwrap_or_else(|| "Untitled thread".into()),
            "project": t.project_root.clone().unwrap_or_else(|| t.cwd.clone()),
            "status": t.status,
            "updated": t.updated_at,
        }))
        .collect();
    Ok(json!({ "threads": found }))
}

/// A thread's recent conversation, oldest first: what was asked and answered.
pub fn read_thread(state: &AppState, id: Uuid, limit: usize) -> Result<Value, String> {
    let title = title_of(state, id).ok_or("No thread with that id here.")?;
    let rows = state.persistence.transcript_entries(id).map_err(|e| e.to_string())?;
    let said: Vec<Value> = rows.iter()
        .filter(|r| matches!(r.role.as_str(), "user" | "prompt" | "agent") && !r.body.trim().is_empty())
        .rev()
        .take(limit.clamp(1, 60))
        .map(|r| json!({ "who": if r.role == "agent" { "agent" } else { "person" }, "text": r.body, "at": r.at }))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Ok(json!({ "id": id, "title": title, "messages": said }))
}

/// The person (or `from`'s agent) sends `message` from one thread to another. Records the sender's
/// card, then delivers here, or relays through the app when the thread lives in another core.
pub fn send(state: &Arc<AppState>, from: Uuid, to: Uuid, to_title: Option<String>, message: String) -> Result<Value, String> {
    if message.trim().is_empty() {
        return Err("Say what to send.".into());
    }
    if from == to {
        return Err("That's this thread.".into());
    }
    let from_title = title_of(state, from).unwrap_or_else(|| "Another thread".into());
    let local = title_of(state, to);
    let to_title = local.clone().or(to_title).unwrap_or_else(|| "another thread".into());
    state.event_bus.emit(ControlEvent::Raw {
        session_id: Some(from),
        payload: json!({ "channel": "handoff", "kind": "sent", "thread": to, "title": to_title, "preview": preview(&message) }),
    });
    if local.is_some() {
        deliver(state.clone(), to, from, from_title, message);
        Ok(json!({ "sent_to": to_title, "status": "delivered; it starts once that thread is free" }))
    } else {
        state.event_bus.emit(ControlEvent::Raw {
            session_id: Some(from),
            payload: json!({ "channel": "handoff", "kind": "relay", "to": to, "from": from, "from_title": from_title, "message": message }),
        });
        Ok(json!({ "sent_to": to_title, "status": "passed to the app for the machine that thread runs on" }))
    }
}

/// Post `message` into `to` and start it working: its "From …" card, then the message as a prompt
/// in the thread's own mode. Waits for a busy thread's turn to end first (in the background).
pub fn deliver(state: Arc<AppState>, to: Uuid, from: Uuid, from_title: String, message: String) {
    tokio::spawn(async move {
        let _ = services::wait_turn_settled(&state, &to.to_string(), WAIT_FOR_TURN).await;
        state.event_bus.emit(ControlEvent::Raw {
            session_id: Some(to),
            payload: json!({ "channel": "handoff", "kind": "received", "thread": from, "title": from_title, "preview": preview(&message) }),
        });
        let text = format!("From the thread “{from_title}”:\n\n{message}");
        state.event_bus.emit(ControlEvent::UserMessage { session_id: to, text: text.clone(), origin: "handoff".into(), at: Utc::now() });
        if let Err(e) = services::send_prompt(&state, to.to_string(), text, None, None, None, None, None, None, None, None).await {
            tracing::warn!(%to, error = %e, "handoff could not start the thread");
            state.event_bus.emit_error(Some(to), format!("A handoff from “{from_title}” arrived but couldn't start: {e}"));
        }
    });
}

/// The `bomb` tool: an agent sends to a thread its person mentioned.
pub fn send_from_agent(state: &Arc<AppState>, from: Uuid, to: Uuid, message: String) -> Result<Value, String> {
    if !state.helpers.mentions.allows(from, to) {
        return Err("You can send only to threads the person @-mentioned in this conversation. Ask them to mention it with @ first.".into());
    }
    send(state, from, to, None, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mentions_come_from_the_note() {
        let a = Uuid::new_v4();
        let note = format!("send this over\n\n[Threads mentioned]\n- @Fix login crash: thread id {a} · project x");
        assert_eq!(mentioned_ids(&note), vec![a]);
        assert!(mentioned_ids("thread id not-a-uuid").is_empty());
        let m = Mentions::default();
        let me = Uuid::new_v4();
        m.note(me, &note);
        assert!(m.allows(me, a));
        assert!(!m.allows(me, Uuid::new_v4()));
        assert!(!m.allows(a, me), "only the thread that mentioned it may send");
    }

    #[tokio::test]
    async fn a_handoff_lands_in_the_other_thread_and_starts_it() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_path_buf();
        let (grok, panel) = (home.join("grok"), home.join("grok").join("panel"));
        let state = Arc::new(AppState::initialize_with_paths(grok_config::GrokPaths {
            home_dir: home.clone(), grok_dir: grok.clone(), bomb_dir: panel.clone(), config_file: panel.join("config.toml"),
            grok_cli_config_file: grok.join("config.toml"), worktrees_dir: home.join("worktrees"),
            memory_dir: panel.join("memory"), sessions_dir: panel.join("sessions"), panel_dir: panel,
            project_config_file: None, project_root: None,
        }).await.unwrap());
        let project = home.join("p");
        std::fs::create_dir_all(&project).unwrap();
        let start = || services::start_mock_session(&state, project.display().to_string());
        let a = Uuid::parse_str(&start().await.unwrap().id).unwrap();
        let b = Uuid::parse_str(&start().await.unwrap().id).unwrap();
        for id in [a, b] { services::wait_until_idle(&state, &id.to_string(), Duration::from_secs(5)).await.unwrap(); }

        // Not mentioned: the agent is told to ask for a mention.
        assert!(send_from_agent(&state, a, b, "the algorithm".into()).is_err());
        state.helpers.mentions.note(a, &format!("send it over\n\n[Threads mentioned — Bomb Code]\n- @B: thread id {b} · p"));
        send_from_agent(&state, a, b, "Use this algorithm: binary search.".into()).unwrap();

        // B gets the message as a prompt (and runs it); both sides keep a card.
        let landed = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let rows = state.persistence.transcript_entries(b).unwrap();
                if rows.iter().any(|r| r.body.contains("From the thread") && r.body.contains("binary search")) { return rows; }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }).await.expect("the handoff reached the other thread");
        assert!(landed.iter().any(|r| r.role == "handoff" && r.body.contains(&a.to_string())), "B's card points back at A");
        let sent = state.persistence.transcript_entries(a).unwrap();
        assert!(sent.iter().any(|r| r.role == "handoff" && r.body.contains(&b.to_string()) && r.body.contains("\"outgoing\":true")));
    }
}
