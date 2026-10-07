//! Bring Claude Code CLI conversations into Bomb Code as saved threads.
//!
//! Each session file under `~/.claude/projects/<folder>/<session id>.jsonl`
//! becomes a thread in its original folder whose ACP session id is the Claude
//! session id. The transcript is copied here so the thread reads like any
//! other; the first send reopens the real session (`session/load`), so the
//! agent continues with its full context. Source files are only read.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use grok_control_core::{AgentMode, BrainMode, SessionMetadata};
use grok_events::SessionStatus;
use grok_persistence::{Persistence, SessionRecord, WorkspaceRecord};
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::state::AppState;

/// Tool output kept per call; the full output stays in the Claude session.
pub(super) const MAX_TOOL_RESULT: usize = 4000;
pub(super) const MAX_TOOL_ARGS: usize = 4000;

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub imported: usize,
    /// Already a Bomb Code thread (imported before, or started here).
    pub already_here: usize,
    /// Project folder no longer exists, or no conversation in the file.
    pub skipped: usize,
    pub failed: usize,
}

/// Import every Claude Code session that isn't a Bomb Code thread yet.
pub async fn import_claude_code_sessions(state: &AppState) -> Result<ImportReport, String> {
    let persistence = state.persistence.clone();
    tokio::task::spawn_blocking(move || {
        let root = claude_projects_dir().ok_or("no home folder")?;
        import_from(&root, &persistence)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Import every session file under `root` (a Claude `projects` folder).
pub fn import_from(root: &Path, persistence: &Persistence) -> Result<ImportReport, String> {
    let known: HashSet<String> = persistence
        .list_sessions()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter_map(|r| r.acp_session_id)
        .collect();
    let mut workspaces = persistence.list_workspaces().map_err(|e| e.to_string())?;
    let mut report = ImportReport::default();
    for file in session_files(root) {
        let Some(sid) = file.file_stem().and_then(|s| s.to_str()).map(String::from) else {
            continue;
        };
        if known.contains(&sid) {
            report.already_here += 1;
            continue;
        }
        let parsed = match parse_session(&file) {
            Ok(Some(p)) => p,
            Ok(None) => {
                report.skipped += 1;
                continue;
            }
            Err(e) => {
                tracing::warn!(file = %file.display(), error = %e, "claude import: unreadable session");
                report.failed += 1;
                continue;
            }
        };
        if !importable_folder(&parsed.cwd) {
            report.skipped += 1;
            continue;
        }
        let root = project_root_for(&parsed.cwd);
        let (rec, rows) = to_thread(&sid, parsed, grok_config::Backend::Claude, &root);
        let saved = persistence
            .import_session(&rec, &rows)
            .and_then(|()| attach_folder_workspace(persistence, &mut workspaces, &rec, &root));
        match saved {
            Ok(()) => report.imported += 1,
            Err(e) => {
                tracing::warn!(%sid, error = %e, "claude import: save failed");
                report.failed += 1;
            }
        }
    }
    Ok(report)
}

/// Imported threads work in their project folder, as they did in the CLI. Without a
/// workspace, the startup migration would file them as read-only questions.
pub(super) fn attach_folder_workspace(
    persistence: &Persistence,
    workspaces: &mut Vec<WorkspaceRecord>,
    rec: &SessionRecord,
    project_root: &str,
) -> grok_persistence::Result<()> {
    let existing = workspaces
        .iter()
        .find(|w| {
            w.path == rec.cwd && w.shared_checkout && !w.inline && !w.read_only && w.archived_at.is_none()
        })
        .map(|w| w.id.clone());
    let id = match existing {
        Some(id) => id,
        None => {
            let branch = current_branch(Path::new(&rec.cwd)).unwrap_or_default();
            let w = WorkspaceRecord {
                id: Uuid::new_v4().to_string(),
                project_root: project_root.to_string(),
                name: Path::new(&rec.cwd)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| rec.cwd.clone()),
                base_ref: if branch.is_empty() { "HEAD".into() } else { branch.clone() },
                branch,
                path: rec.cwd.clone(),
                created_at: rec.created_at.to_rfc3339(),
                archived_at: None,
                inline: false,
                shared_checkout: true,
                read_only: false,
                threads: Vec::new(),
            };
            persistence.save_workspace(&w)?;
            let id = w.id.clone();
            workspaces.push(w);
            id
        }
    };
    persistence.attach_workspace(rec.id, &id)
}

/// The checked-out branch, or `None` outside a Git repository.
pub(super) fn current_branch(dir: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|b| !b.is_empty() && b != "HEAD")
}

fn claude_projects_dir() -> Option<PathBuf> {
    let base = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")))?;
    Some(base.join("projects"))
}

/// Top-level session files only; subfolders hold subagent runs.
fn session_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(dirs) = std::fs::read_dir(root) else {
        return out;
    };
    for dir in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(dir.path()) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.is_file() && p.extension().is_some_and(|e| e == "jsonl") {
                out.push(p);
            }
        }
    }
    out
}

/// A folder the thread can run in again. Bomb Code's own working folders
/// (`~/.grok/…`) hold its internal sessions, and the home folder can't be a project.
pub(super) fn importable_folder(cwd: &str) -> bool {
    let path = Path::new(cwd);
    if !path.is_absolute() || !path.is_dir() {
        return false;
    }
    match std::env::var_os("HOME").map(PathBuf::from) {
        Some(home) => path != home && !path.starts_with(home.join(".grok")),
        None => true,
    }
}

pub(super) struct ParsedSession {
    pub(super) cwd: String,
    pub(super) title: Option<String>,
    pub(super) first_prompt: Option<String>,
    pub(super) started: DateTime<Utc>,
    pub(super) last: DateTime<Utc>,
    pub(super) rows: Vec<(String, String, DateTime<Utc>)>,
}

/// Tool call waiting for its result (results arrive in a later user line).
struct OpenTool {
    row: usize,
    id: String,
    name: String,
    args: String,
}

fn parse_session(file: &Path) -> std::io::Result<Option<ParsedSession>> {
    let reader = BufReader::new(std::fs::File::open(file)?);
    let mut cwd: Option<String> = None;
    let mut title: Option<String> = None;
    let mut first_prompt: Option<String> = None;
    let mut started: Option<DateTime<Utc>> = None;
    let mut last: Option<DateTime<Utc>> = None;
    let mut rows: Vec<(String, String, DateTime<Utc>)> = Vec::new();
    let mut open: HashMap<String, OpenTool> = HashMap::new();
    let mut spoke = false;

    for line in reader.lines() {
        let line = line?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "ai-title" => {
                if let Some(t) = v.get("aiTitle").and_then(Value::as_str) {
                    title = Some(t.to_string());
                }
                continue;
            }
            // A name the person gave the session beats the generated one.
            "custom-title" => {
                if let Some(t) = v.get("customTitle").and_then(Value::as_str) {
                    title = Some(t.to_string());
                }
                continue;
            }
            "summary" => {
                if title.is_none() {
                    title = v.get("summary").and_then(Value::as_str).map(String::from);
                }
                continue;
            }
            "user" | "assistant" => {}
            _ => continue,
        }
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if cwd.is_none() {
            cwd = v.get("cwd").and_then(Value::as_str).map(String::from);
        }
        let at = v
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|t| t.parse::<DateTime<Utc>>().ok())
            .or(last)
            .unwrap_or_else(Utc::now);
        started.get_or_insert(at);
        last = Some(at);
        let content = v
            .pointer("/message/content")
            .cloned()
            .unwrap_or(Value::Null);

        if kind == "assistant" {
            for block in blocks(&content) {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                        push_agent_text(&mut rows, text, at);
                    }
                    Some("tool_use") => {
                        let id = block
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let name = block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string();
                        let args = truncate(
                            &block.get("input").map(Value::to_string).unwrap_or_default(),
                            MAX_TOOL_ARGS,
                        );
                        rows.push((
                            "tool".into(),
                            tool_payload(&id, &name, "completed", &args, None),
                            at,
                        ));
                        if !id.is_empty() {
                            open.insert(
                                id.clone(),
                                OpenTool {
                                    row: rows.len() - 1,
                                    id,
                                    name,
                                    args,
                                },
                            );
                        }
                    }
                    _ => {}
                }
            }
            continue;
        }

        // A user line: the person's words, or tool results going back to the model.
        if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if v.get("isCompactSummary").and_then(Value::as_bool) == Some(true) {
            rows.push((
                "system".into(),
                "Conversation compacted — earlier context was summarized for the agent.".into(),
                at,
            ));
            continue;
        }
        let mut words: Vec<String> = Vec::new();
        for block in blocks(&content) {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(t) = block
                        .get("text")
                        .and_then(Value::as_str)
                        .and_then(user_words)
                    {
                        words.push(t);
                    }
                }
                Some("image") => words.push("[image]".into()),
                Some("tool_result") => {
                    let id = block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if let Some(tool) = open.remove(id) {
                        let failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
                        let result =
                            truncate(&tool_result_text(block.get("content")), MAX_TOOL_RESULT);
                        rows[tool.row].1 = tool_payload(
                            &tool.id,
                            &tool.name,
                            if failed { "failed" } else { "completed" },
                            &tool.args,
                            Some(&result),
                        );
                    }
                }
                _ => {}
            }
        }
        let prompt = words.join("\n\n");
        if !prompt.trim().is_empty() {
            first_prompt.get_or_insert_with(|| prompt.clone());
            rows.push(("prompt".into(), prompt, at));
            spoke = true;
        }
    }

    let (Some(cwd), Some(started), Some(last)) = (cwd, started, last) else {
        return Ok(None);
    };
    if !spoke {
        return Ok(None);
    }
    Ok(Some(ParsedSession {
        cwd,
        title,
        first_prompt,
        started,
        last,
        rows,
    }))
}

/// Message content as a list of blocks; a plain string is one text block.
pub(super) fn blocks(content: &Value) -> Vec<Value> {
    match content {
        Value::String(s) => vec![json!({ "type": "text", "text": s })],
        Value::Array(a) => a.clone(),
        _ => Vec::new(),
    }
}

/// Consecutive text from one turn reads as one message.
pub(super) fn push_agent_text(rows: &mut Vec<(String, String, DateTime<Utc>)>, text: &str, at: DateTime<Utc>) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    if let Some((kind, body, _)) = rows.last_mut() {
        if kind == "agent" {
            body.push_str("\n\n");
            body.push_str(text);
            return;
        }
    }
    rows.push(("agent".into(), text.to_string(), at));
}

pub(super) fn tool_payload(id: &str, name: &str, status: &str, args: &str, result: Option<&str>) -> String {
    json!({ "id": id, "tool": name, "status": status, "args": args, "result": result }).to_string()
}

fn tool_result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| match i.get("type").and_then(Value::as_str) {
                Some("text") => i.get("text").and_then(Value::as_str).map(String::from),
                Some("image") => Some("[image]".into()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// What the person typed, without the wrappers Claude Code stores around it.
/// `None` when the text is only machinery (command output, reminders, caveats).
fn user_words(text: &str) -> Option<String> {
    let text = text.trim();
    if text.starts_with("[Request interrupted by user")
        || text.starts_with("Caveat: The messages below")
    {
        return None;
    }
    // `/command args` typed in the CLI.
    if let Some(name) = tag_body(text, "command-name") {
        let args = tag_body(text, "command-args").unwrap_or_default();
        let line = format!("{} {}", name.trim(), args.trim());
        return Some(line.trim().to_string());
    }
    // `! command` shell input.
    if let Some(cmd) = tag_body(text, "bash-input") {
        return Some(format!("! {}", cmd.trim()));
    }
    let mut out = text.to_string();
    for tag in [
        "system-reminder",
        "local-command-stdout",
        "local-command-stderr",
        "local-command-caveat",
        "bash-stdout",
        "bash-stderr",
        "task-notification",
        "command-message",
    ] {
        out = strip_tag(&out, tag);
    }
    let out = out.trim();
    (!out.is_empty()).then(|| out.to_string())
}

fn tag_body(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(text[start..end].to_string())
}

fn strip_tag(text: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(&open) {
        out.push_str(&rest[..i]);
        match rest[i..].find(&close) {
            Some(j) => rest = &rest[i + j + close.len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

pub(super) fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Claude Code keeps its own worktrees inside the project; list those threads under it.
fn project_root_for(cwd: &str) -> String {
    match cwd.find("/.claude/worktrees/") {
        Some(i) => cwd[..i].to_string(),
        None => cwd.to_string(),
    }
}

/// A saved thread for an imported conversation: `sid` is the agent's own session id,
/// which the first send reopens with `session/load`.
pub(super) fn to_thread(
    sid: &str,
    p: ParsedSession,
    backend: grok_config::Backend,
    project_root: &str,
) -> (SessionRecord, Vec<(String, String, DateTime<Utc>)>) {
    let id = Uuid::new_v4();
    let label = p
        .title
        .or(p
            .first_prompt
            .map(|f| f.lines().next().unwrap_or("").to_string()))
        .map(|t| truncate(t.trim(), 80))
        .filter(|t| !t.is_empty());
    let meta = SessionMetadata {
        id,
        acp_session_id: Some(sid.to_string()),
        cwd: p.cwd.clone(),
        worktree: None,
        read_only: false,
        project_root: Some(project_root.to_string()),
        // Empty: reopening the session restores the agent's own model.
        model: String::new(),
        backend,
        mode: AgentMode::Acp,
        status: SessionStatus::Idle,
        approval_mode: Default::default(),
        plan_mode: false,
        always_approve: false,
        sandbox_profile: None,
        mcp_servers: Vec::new(),
        approved_high_risk_mcp: Vec::new(),
        created_at: p.started,
        last_activity: p.last,
        label,
        brain_mode: BrainMode::default(),
    };
    let rec = SessionRecord {
        id,
        cwd: p.cwd,
        mode: "acp".into(),
        model: String::new(),
        status: "idle".into(),
        worktree: None,
        acp_session_id: Some(sid.to_string()),
        metadata_json: json!({ "metadata": meta }).to_string(),
        created_at: p.started,
        updated_at: p.last,
        message_count: 0,
    };
    (rec, p.rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(lines: &[Value]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        f
    }

    #[test]
    fn reads_prompts_replies_and_tool_results() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().display().to_string();
        let f = write(&[
            json!({"type":"user","cwd":cwd,"timestamp":"2026-09-01T10:00:00Z","message":{"role":"user","content":"fix the build"}}),
            json!({"type":"user","isMeta":true,"cwd":cwd,"timestamp":"2026-09-01T10:00:00Z","message":{"role":"user","content":"meta"}}),
            json!({"type":"assistant","cwd":cwd,"timestamp":"2026-09-01T10:00:01Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"Looking."}]}}),
            json!({"type":"assistant","cwd":cwd,"timestamp":"2026-09-01T10:00:02Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo build"}}]}}),
            json!({"type":"user","cwd":cwd,"timestamp":"2026-09-01T10:00:03Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"error[E0425]","is_error":true}]}}),
            json!({"type":"user","isSidechain":true,"cwd":cwd,"timestamp":"2026-09-01T10:00:03Z","message":{"role":"user","content":"subagent"}}),
            json!({"type":"assistant","cwd":cwd,"timestamp":"2026-09-01T10:00:04Z","message":{"role":"assistant","content":[{"type":"text","text":"Fixed."}]}}),
            json!({"type":"user","cwd":cwd,"timestamp":"2026-09-01T10:01:00Z","message":{"role":"user","content":"<command-name>/model</command-name>\n<command-message>model</command-message>\n<command-args>opus</command-args>"}}),
            json!({"type":"user","cwd":cwd,"timestamp":"2026-09-01T10:01:00Z","message":{"role":"user","content":"<local-command-stdout>Set model</local-command-stdout>"}}),
            json!({"type":"ai-title","aiTitle":"Build fix","sessionId":"s"}),
        ]);
        let p = parse_session(f.path()).unwrap().unwrap();
        assert_eq!(p.title.as_deref(), Some("Build fix"));
        assert_eq!(p.cwd, cwd);
        let kinds: Vec<&str> = p.rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(kinds, ["prompt", "agent", "tool", "agent", "prompt"]);
        assert_eq!(p.rows[0].1, "fix the build");
        let tool: Value = serde_json::from_str(&p.rows[2].1).unwrap();
        assert_eq!(tool["tool"], "Bash");
        assert_eq!(tool["status"], "failed");
        assert_eq!(tool["result"], "error[E0425]");
        assert_eq!(p.rows[4].1, "/model opus");
        assert_eq!(p.last.to_rfc3339(), "2026-09-01T10:01:00+00:00");
    }

    #[test]
    fn imported_threads_can_make_changes_and_import_once() {
        let project = tempfile::tempdir().unwrap();
        let claude = tempfile::tempdir().unwrap();
        let folder = claude.path().join("-some-project");
        std::fs::create_dir(&folder).unwrap();
        let cwd = project.path().display().to_string();
        for sid in ["11111111-1111-1111-1111-111111111111", "22222222-2222-2222-2222-222222222222"] {
            let lines = [
                json!({"type":"user","cwd":cwd,"timestamp":"2026-09-01T10:00:00Z","message":{"role":"user","content":"hi"}}),
                json!({"type":"assistant","cwd":cwd,"timestamp":"2026-09-01T10:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}),
            ];
            let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
            std::fs::write(folder.join(format!("{sid}.jsonl")), body).unwrap();
        }
        let db_dir = tempfile::tempdir().unwrap();
        let db = Persistence::open(db_dir.path().join("t.db")).unwrap();

        let first = import_from(claude.path(), &db).unwrap();
        assert_eq!(first.imported, 2);
        let again = import_from(claude.path(), &db).unwrap();
        assert_eq!((again.imported, again.already_here), (0, 2));

        let workspaces = db.list_workspaces().unwrap();
        assert_eq!(workspaces.len(), 1, "threads in one folder share its workspace");
        let w = &workspaces[0];
        assert!(w.shared_checkout && !w.inline && !w.read_only);
        assert_eq!(w.path, cwd);
        assert_eq!(w.threads.len(), 2);
        let sessions = db.list_sessions().unwrap();
        assert!(sessions.iter().all(|s| s.updated_at.to_rfc3339() == "2026-09-01T10:00:01+00:00"));
    }

    #[test]
    fn a_file_without_a_prompt_is_skipped() {
        let f = write(&[json!({"type":"ai-title","aiTitle":"x","sessionId":"s"})]);
        assert!(parse_session(f.path()).unwrap().is_none());
    }

    #[test]
    fn reminders_are_stripped_from_prompts() {
        assert_eq!(
            user_words("hello<system-reminder>be nice</system-reminder> world").as_deref(),
            Some("hello world")
        );
        assert_eq!(user_words("<system-reminder>only</system-reminder>"), None);
        assert_eq!(
            user_words("<bash-input>ls</bash-input>").as_deref(),
            Some("! ls")
        );
    }

    #[test]
    fn claude_worktrees_list_under_their_project() {
        assert_eq!(project_root_for("/a/b/.claude/worktrees/x"), "/a/b");
        assert_eq!(project_root_for("/a/b"), "/a/b");
    }
}

