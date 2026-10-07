//! Bring Codex conversations (desktop app and CLI) into Bomb Code as saved threads.
//!
//! Codex lists its threads in `~/.codex/state_<n>.sqlite` (`threads`: id, cwd, title, the
//! rollout file) and keeps each conversation in a JSONL "rollout". A thread here gets the
//! Codex thread id as its ACP session id, so the first send reopens the real session
//! (`session/load`, checked against codex-acp 2.1) with its full context. The transcript
//! is copied once so the thread reads like any other. Codex's files are only read.
//!
//! Left out: guardian checks and spawned sub-agents (Codex's own workers), archived
//! threads, threads Bomb Code itself started, and conversations already here.

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use grok_persistence::{Persistence, WorkspaceRecord};
use serde_json::Value;

use super::claude_import::{
    attach_folder_workspace, importable_folder, push_agent_text, to_thread, tool_payload,
    truncate, ImportReport, ParsedSession, MAX_TOOL_ARGS, MAX_TOOL_RESULT,
};
use crate::state::AppState;

/// Import every Codex conversation that isn't a Bomb Code thread yet.
pub async fn import_codex_sessions(state: &AppState) -> Result<ImportReport, String> {
    let persistence = state.persistence.clone();
    tokio::task::spawn_blocking(move || {
        let home = codex_home().ok_or("no home folder")?;
        import_from(&home, &persistence)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn codex_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")))
}

/// Codex's thread index: the newest `state_<n>.sqlite` (the number is a schema version).
fn state_db(home: &Path) -> Option<PathBuf> {
    std::fs::read_dir(home)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let n = name.strip_prefix("state_")?.strip_suffix(".sqlite")?.parse::<u32>().ok()?;
            Some((n, e.path()))
        })
        .max_by_key(|(n, _)| *n)
        .map(|(_, p)| p)
}

struct CodexThread {
    id: String,
    rollout: PathBuf,
    cwd: String,
    title: String,
    created: DateTime<Utc>,
}

/// Conversations people had, read-only from Codex's own index.
fn list_threads(db: &Path) -> Result<(Vec<CodexThread>, Vec<String>), String> {
    let conn = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("can't read Codex's thread list: {e}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT id, rollout_path, cwd, COALESCE(NULLIF(name, ''), title), created_at
             FROM threads
             WHERE archived = 0 AND source NOT LIKE '{%' AND COALESCE(originator, '') != 'BombCode'",
        )
        .map_err(|e| e.to_string())?;
    let threads = stmt
        .query_map([], |r| {
            Ok(CodexThread {
                id: r.get(0)?,
                rollout: PathBuf::from(r.get::<_, String>(1)?),
                cwd: r.get(2)?,
                title: r.get(3)?,
                created: Utc.timestamp_opt(r.get(4)?, 0).single().unwrap_or_default(),
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    // Codex's own projects: folders that group threads in the Codex app.
    let roots = conn
        .prepare("SELECT path FROM project_roots")
        .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>())
        .unwrap_or_default();
    Ok((threads, roots))
}

/// Import every Codex conversation under `home` (a `~/.codex` folder).
pub fn import_from(home: &Path, persistence: &Persistence) -> Result<ImportReport, String> {
    let db = state_db(home).ok_or("Codex hasn't been used on this Mac")?;
    let (threads, codex_roots) = list_threads(&db)?;
    let sessions = persistence.list_sessions().map_err(|e| e.to_string())?;
    let known_ids: HashSet<String> = sessions.iter().filter_map(|r| r.acp_session_id.clone()).collect();
    let registered: Vec<String> = persistence
        .get_kv("projects")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let mut matcher = ProjectMatcher::new(
        sessions.iter().map(|r| project_root_of(&r.metadata_json).unwrap_or_else(|| r.cwd.clone())).chain(registered.clone()),
        registered,
        codex_roots,
    );
    let mut workspaces: Vec<WorkspaceRecord> = persistence.list_workspaces().map_err(|e| e.to_string())?;
    let mut report = ImportReport::default();
    for t in threads {
        if known_ids.contains(&t.id) {
            report.already_here += 1;
            continue;
        }
        if !importable_folder(&t.cwd) || !t.rollout.is_file() {
            report.skipped += 1;
            continue;
        }
        let parsed = match parse_rollout(&t) {
            Ok(Some(p)) => p,
            Ok(None) => {
                report.skipped += 1;
                continue;
            }
            Err(e) => {
                tracing::warn!(file = %t.rollout.display(), error = %e, "codex import: unreadable rollout");
                report.failed += 1;
                continue;
            }
        };
        let root = matcher.root_for(&t.cwd);
        let (rec, rows) = to_thread(&t.id, parsed, grok_config::Backend::Codex, &root);
        let saved = persistence
            .import_session(&rec, &rows)
            .and_then(|()| attach_folder_workspace(persistence, &mut workspaces, &rec, &root));
        match saved {
            Ok(()) => report.imported += 1,
            Err(e) => {
                tracing::warn!(id = %t.id, error = %e, "codex import: save failed");
                report.failed += 1;
            }
        }
    }
    Ok(report)
}

fn project_root_of(metadata_json: &str) -> Option<String> {
    serde_json::from_str::<Value>(metadata_json)
        .ok()?
        .pointer("/metadata/projectRoot")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(String::from)
}

/// Files a conversation under the project it belongs to, so a Codex thread lands next
/// to the Claude and Bomb Code threads of the same project.
struct ProjectMatcher {
    /// Project roots already in the sidebar, plus ones chosen during this import.
    known: HashSet<String>,
    /// Folders added as projects by hand: their subfolders belong to them.
    registered: Vec<String>,
    codex_roots: Vec<String>,
    home: Option<String>,
}

impl ProjectMatcher {
    fn new(known: impl Iterator<Item = String>, registered: Vec<String>, codex_roots: Vec<String>) -> Self {
        let clean = |s: String| s.trim_end_matches('/').to_string();
        Self {
            known: known.map(clean).filter(|s| !s.is_empty()).collect(),
            registered: registered.into_iter().map(clean).collect(),
            codex_roots: codex_roots.into_iter().map(clean).collect(),
            home: std::env::var("HOME").ok(),
        }
    }

    /// In order: a project that is this folder; the repository a worktree belongs to; the
    /// folder's Git repository when that is a project (in the sidebar, added by hand, or a
    /// Codex project); else the folder itself. Grouping is by repository, never by parent
    /// folder: `~/src` being a project must not collect every repository under it.
    fn root_for(&mut self, cwd: &str) -> String {
        let cwd = cwd.trim_end_matches('/');
        let root = self.choose(cwd);
        self.known.insert(root.clone());
        root
    }

    fn choose(&self, cwd: &str) -> String {
        if self.known.contains(cwd) {
            return cwd.to_string();
        }
        if let Some(main) = worktree_main_repo(Path::new(cwd)) {
            return main;
        }
        if let Some(top) = git_toplevel(Path::new(cwd)).filter(|t| self.is_project(t)) {
            return top;
        }
        cwd.to_string()
    }

    fn is_project(&self, root: &str) -> bool {
        Some(root) != self.home.as_deref()
            && (self.known.contains(root) || self.registered.iter().chain(&self.codex_roots).any(|r| r == root))
    }
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn git_toplevel(dir: &Path) -> Option<String> {
    git(dir, &["rev-parse", "--show-toplevel"])
}

/// For a linked worktree, the main checkout it was made from.
fn worktree_main_repo(dir: &Path) -> Option<String> {
    let common = git(dir, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let main = Path::new(&common).parent()?.to_string_lossy().into_owned();
    let top = git_toplevel(dir)?;
    (main != top && Path::new(&main).is_dir()).then_some(main)
}

/// Read one rollout into prompts, replies and tool calls.
fn parse_rollout(t: &CodexThread) -> std::io::Result<Option<ParsedSession>> {
    let reader = BufReader::new(std::fs::File::open(&t.rollout)?);
    let mut rows: Vec<(String, String, DateTime<Utc>)> = Vec::new();
    let mut open: std::collections::HashMap<String, (usize, String, String)> = Default::default();
    let mut first_prompt: Option<String> = None;
    // Time of the latest line, for lines without a timestamp of their own.
    let mut last = t.created;
    for line in reader.lines() {
        let line = line?;
        // Cheap skip for the bulky lines that never become rows.
        if line.contains("\"type\":\"token_count\"") || line.contains("\"type\":\"reasoning\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let at = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<DateTime<Utc>>().ok())
            .unwrap_or(last);
        last = last.max(at);
        let payload = v.get("payload").cloned().unwrap_or(Value::Null);
        let kind = payload.get("type").and_then(Value::as_str).unwrap_or("");
        match (v.get("type").and_then(Value::as_str).unwrap_or(""), kind) {
            ("response_item", "message") => match payload.get("role").and_then(Value::as_str) {
                Some("user") => {
                    if let Some(prompt) = user_prompt(payload.get("content")) {
                        first_prompt.get_or_insert_with(|| prompt.clone());
                        rows.push(("prompt".into(), prompt, at));
                    }
                }
                Some("assistant") => {
                    for item in payload.get("content").and_then(Value::as_array).into_iter().flatten() {
                        if let Some(text) = item.get("text").and_then(Value::as_str) {
                            push_agent_text(&mut rows, text, at);
                        }
                    }
                }
                _ => {}
            },
            ("response_item", "function_call" | "custom_tool_call" | "local_shell_call") => {
                let call = payload.get("call_id").and_then(Value::as_str).unwrap_or("").to_string();
                let name = payload.get("name").and_then(Value::as_str).unwrap_or(if kind == "local_shell_call" { "shell" } else { "tool" }).to_string();
                let args = payload
                    .get("arguments")
                    .or_else(|| payload.get("input"))
                    .or_else(|| payload.get("action"))
                    .map(|a| a.as_str().map(String::from).unwrap_or_else(|| a.to_string()))
                    .unwrap_or_default();
                let args = truncate(&args, MAX_TOOL_ARGS);
                rows.push(("tool".into(), tool_payload(&call, &name, "completed", &args, None), at));
                if !call.is_empty() {
                    open.insert(call, (rows.len() - 1, name, args));
                }
            }
            ("response_item", "function_call_output" | "custom_tool_call_output") => {
                let call = payload.get("call_id").and_then(Value::as_str).unwrap_or("");
                if let Some((row, name, args)) = open.remove(call) {
                    let result = truncate(&output_text(payload.get("output")), MAX_TOOL_RESULT);
                    rows[row].1 = tool_payload(call, &name, "completed", &args, Some(&result));
                }
            }
            ("response_item", "web_search_call") => {
                let query = payload.pointer("/action/query").and_then(Value::as_str).unwrap_or("");
                rows.push(("tool".into(), tool_payload("", "web_search", "completed", query, None), at));
            }
            ("response_item", "image_generation_call") => {
                // The image itself is base64 in the rollout; the prompt is what reads well.
                let prompt = payload.get("revised_prompt").and_then(Value::as_str).unwrap_or("");
                rows.push(("tool".into(), tool_payload("", "image_gen", "completed", &truncate(prompt, MAX_TOOL_ARGS), None), at));
            }
            ("compacted", _) => {
                rows.push(("system".into(), "Conversation compacted — earlier context was summarized for the agent.".into(), at));
            }
            _ => {}
        }
    }
    if !rows.iter().any(|r| r.0 == "prompt") {
        return Ok(None);
    }
    // Last activity is the last message or tool call. Codex also appends bookkeeping
    // (settings, token counts) and bumps its own `updated_at` when a thread is merely
    // reopened, which would make a September thread look minutes old.
    let last_message = rows.iter().map(|r| r.2).max().unwrap_or(t.created);
    Ok(Some(ParsedSession {
        cwd: t.cwd.clone(),
        title: Some(t.title.clone()).filter(|s| !s.trim().is_empty()),
        first_prompt,
        started: t.created.min(last_message),
        last: last_message,
        rows,
    }))
}

/// Tool output is a string, or a list of `{type: input_text, text}` blocks.
fn output_text(output: Option<&Value>) -> String {
    match output {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Object(o)) => o.get("content").and_then(Value::as_str).map(String::from).unwrap_or_default(),
        _ => String::new(),
    }
}

/// What the person typed. Codex adds context of its own as user messages (environment,
/// AGENTS.md, plugins, skills, interruptions, goal-mode nudges) and prefixes prompts with
/// files or the in-app browser before "## My request for Codex:" / "## My request:".
/// Answers to the agent's questions arrive wrapped; they read as "question → answer".
fn user_prompt(content: Option<&Value>) -> Option<String> {
    const CONTEXT: &[&str] = &[
        "<environment_context>", "# AGENTS.md instructions", "<recommended_plugins>", "<skill>",
        "<turn_aborted>", "<in-app-browser-context", "<permissions instructions>", "<user_instructions>",
        "<user_shell_command>", "<codex_internal_context", "<goal_context>",
    ];
    let mut parts: Vec<String> = Vec::new();
    for item in content?.as_array()? {
        match item.get("type").and_then(Value::as_str) {
            Some("input_image") => parts.push("[image]".into()),
            Some("input_text") => {
                let text = item.get("text").and_then(Value::as_str).unwrap_or("").trim();
                if text.is_empty() || CONTEXT.iter().any(|c| text.starts_with(c)) {
                    continue;
                }
                // Tags around an attached image.
                if text == "</image>" || text.starts_with("<image") {
                    continue;
                }
                if let Some(answers) = question_answers(text) {
                    parts.push(answers);
                    continue;
                }
                let text = match text.split_once("\n## My request") {
                    // "## My request for Codex:" or "## My request:", then the typed text.
                    Some((_, rest)) => rest.split_once(':').map(|(_, r)| r).unwrap_or(rest).trim(),
                    None => text,
                };
                if !text.is_empty() {
                    parts.push(text.to_string());
                }
            }
            _ => {}
        }
    }
    let prompt = parts.join("\n\n");
    (!prompt.trim().is_empty() && prompt != "[image]").then_some(prompt).or_else(|| {
        // An image on its own is still something the person sent.
        parts.iter().any(|p| p == "[image]").then(|| "[image]".to_string())
    })
}

fn question_answers(text: &str) -> Option<String> {
    let inner = text
        .strip_prefix("<send_user_message_question_reply>")?
        .trim()
        .strip_suffix("</send_user_message_question_reply>")
        .unwrap_or(text)
        .trim();
    let items: Vec<Value> = serde_json::from_str(inner).ok()?;
    let lines: Vec<String> = items
        .iter()
        .filter_map(|i| {
            let answer = i.get("answer").and_then(Value::as_str)?;
            Some(match i.get("question").and_then(Value::as_str) {
                Some(q) => format!("{q} → {answer}"),
                None => answer.to_string(),
            })
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_what_was_typed() {
        let wrapped = json!([{"type":"input_text","text":"# In app browser (IAB):\n- Current URL: x\n\n## My request for Codex:\nremove the button"}]);
        assert_eq!(user_prompt(Some(&wrapped)).as_deref(), Some("remove the button"));
        let context = json!([{"type":"input_text","text":"<environment_context>\n<cwd>/x</cwd>\n</environment_context>"}]);
        assert_eq!(user_prompt(Some(&context)), None);
        let image = json!([{"type":"input_text","text":"<image name=[Image #1]>"},{"type":"input_image","image_url":"data:"},{"type":"input_text","text":"</image>"},{"type":"input_text","text":"make it blue"}]);
        assert_eq!(user_prompt(Some(&image)).as_deref(), Some("[image]\n\nmake it blue"));
        let files = json!([{"type":"input_text","text":"# Files mentioned by the user:\n\n## a.png: /tmp/a.png\n\n## My request:\ncenter it"}]);
        assert_eq!(user_prompt(Some(&files)).as_deref(), Some("center it"));
        let goal = json!([{"type":"input_text","text":"<codex_internal_context source=\"goal\">\nContinue working"}]);
        assert_eq!(user_prompt(Some(&goal)), None);
        let reply = json!([{"type":"input_text","text":"<send_user_message_question_reply>\n[{\"question\":\"Which?\",\"answer\":\"The first\"}]\n</send_user_message_question_reply>"}]);
        assert_eq!(user_prompt(Some(&reply)).as_deref(), Some("Which? → The first"));
    }

    #[test]
    fn reads_a_rollout() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("rollout.jsonl");
        let lines = [
            json!({"timestamp":"2026-09-01T10:00:00Z","type":"session_meta","payload":{"id":"t1","cwd":"/x"}}),
            json!({"timestamp":"2026-09-01T10:00:00Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"rules"}]}}),
            json!({"timestamp":"2026-09-01T10:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"fix it"}]}}),
            json!({"timestamp":"2026-09-01T10:00:02Z","type":"response_item","payload":{"type":"reasoning","summary":[]}}),
            json!({"timestamp":"2026-09-01T10:00:03Z","type":"response_item","payload":{"type":"custom_tool_call","call_id":"c1","name":"exec","input":"ls"}}),
            json!({"timestamp":"2026-09-01T10:00:04Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"c1","output":[{"type":"input_text","text":"a.rs"}]}}),
            json!({"timestamp":"2026-09-01T10:00:05Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done."}]}}),
            json!({"timestamp":"2026-09-01T10:00:06Z","type":"compacted","payload":{"message":""}}),
        ];
        std::fs::write(&file, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        let t = CodexThread {
            id: "t1".into(),
            rollout: file,
            cwd: "/x".into(),
            title: "Fix".into(),
            created: "2026-09-01T10:00:00Z".parse().unwrap(),
        };
        let p = parse_rollout(&t).unwrap().unwrap();
        let kinds: Vec<&str> = p.rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(kinds, ["prompt", "tool", "agent", "system"]);
        let tool: Value = serde_json::from_str(&p.rows[1].1).unwrap();
        assert_eq!((tool["tool"].as_str(), tool["result"].as_str()), (Some("exec"), Some("a.rs")));
        assert_eq!(p.last.to_rfc3339(), "2026-09-01T10:00:06+00:00");
    }

    #[test]
    fn groups_by_repository_not_parent_folder() {
        let git = |dir: &Path, args: &[&str]| {
            assert!(std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap().status.success());
        };
        let base = tempfile::tempdir().unwrap();
        let base = base.path().canonicalize().unwrap();
        let repo = base.join("repo");
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        git(&repo, &["init", "-q"]);
        let other = base.join("other");
        std::fs::create_dir_all(&other).unwrap();
        git(&other, &["init", "-q"]);
        let s = |p: &Path| p.to_string_lossy().into_owned();
        // The parent folder is a project (added by hand and in Codex) and the repo is known.
        let m = ProjectMatcher::new([s(&base), s(&repo)].into_iter(), vec![s(&base)], vec![s(&base)]);
        assert_eq!(m.choose(&s(&repo)), s(&repo));
        assert_eq!(m.choose(&s(&repo.join("sub"))), s(&repo), "a subfolder joins its repository");
        assert_eq!(m.choose(&s(&other)), s(&other), "a parent folder doesn't collect other repositories");
        assert_eq!(m.choose(&s(&base)), s(&base));
    }
}

