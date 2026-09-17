//! SQLite-backed session + transcript memory for Bomb Code.
//!
//! Survives app quit, reboot, and updates under:
//! `~/.grok/control-panel/sessions/control_panel.db`

pub mod workspaces;
pub use workspaces::WorkspaceRecord;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::info;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, PersistenceError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub id: Uuid,
    pub cwd: String,
    pub mode: String,
    pub model: String,
    pub status: String,
    pub worktree: Option<String>,
    pub acp_session_id: Option<String>,
    pub metadata_json: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Populated by list queries (not stored as column — computed).
    #[serde(default)]
    pub message_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptChunk {
    pub session_id: Uuid,
    pub seq: u64,
    pub kind: String,
    pub payload: String,
    pub at: DateTime<Utc>,
}

/// Frontend-friendly transcript row.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptEntry {
    pub role: String,
    pub body: String,
    pub at: String,
    pub seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelUsage {
    pub backend: String,
    pub model: String,
}

/// Thread list row (live or restored from disk).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadDto {
    #[serde(default)]
    pub models_used: Vec<ModelUsage>,
    pub id: String,
    pub cwd: String,
    pub mode: String,
    pub model: String,
    /// Agent backend (grok | claude | codex); old records default to grok.
    #[serde(default = "default_backend_key")]
    pub backend: String,
    pub status: String,
    /// True when an ACP/headless process is currently attached in this process.
    pub live: bool,
    pub message_count: u64,
    pub created_at: String,
    pub updated_at: String,
    pub worktree: Option<String>,
    pub mcp_servers: Vec<String>,
    pub label: Option<String>,
    /// Approval stance this thread runs with (plan | ask | auto | yolo).
    #[serde(default)]
    pub approval_mode: Option<String>,
    /// Original project folder when cwd is an isolated thread worktree.
    #[serde(default)]
    pub project_root: Option<String>,
    /// full_brain | history_only | fresh | null when not live
    pub brain_mode: Option<String>,
}

fn default_backend_key() -> String {
    "grok".into()
}

pub struct Persistence {
    path: PathBuf,
    /// Serialize writes — multiple event-loop tasks may append concurrently.
    write_lock: Mutex<()>,
}

impl Persistence {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let db = Self {
            path,
            write_lock: Mutex::new(()),
        };
        db.migrate()?;
        info!(path = %db.path.display(), "session memory database open");
        Ok(db)
    }

    fn conn(&self) -> Result<Connection> {
        let conn = Connection::open(&self.path)?;
        // Reasonable durability for a desktop app.
        let _ = conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;",
        );
        Ok(conn)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn()?;
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                cwd TEXT NOT NULL,
                mode TEXT NOT NULL,
                model TEXT NOT NULL,
                status TEXT NOT NULL,
                worktree TEXT,
                acp_session_id TEXT,
                metadata_json TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS transcripts (
                session_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                kind TEXT NOT NULL,
                payload TEXT NOT NULL,
                at TEXT NOT NULL,
                PRIMARY KEY (session_id, seq),
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_transcripts_session_seq
                ON transcripts(session_id, seq);
            CREATE INDEX IF NOT EXISTS idx_sessions_updated
                ON sessions(updated_at DESC);
            CREATE TABLE IF NOT EXISTS workspaces (
                id TEXT PRIMARY KEY,
                path TEXT NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS session_workspaces (
                session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                workspace_id TEXT NOT NULL REFERENCES workspaces(id)
            );
            CREATE TABLE IF NOT EXISTS kv (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            "#,
        )?;
        // Read-only and writable conversations may share a checkout without
        // inheriting each other's permissions. Preserve ids and associations.
        let schema:String=conn.query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name='workspaces'",[],|row|row.get(0))?;
        if schema.contains("path TEXT NOT NULL UNIQUE") {
            conn.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE;
                CREATE TABLE workspaces_v2 (id TEXT PRIMARY KEY, path TEXT NOT NULL, data TEXT NOT NULL);
                INSERT INTO workspaces_v2 SELECT id,path,data FROM workspaces;
                DROP TABLE workspaces;
                ALTER TABLE workspaces_v2 RENAME TO workspaces;
                COMMIT; PRAGMA foreign_keys=ON;")?;
        }
        conn.execute_batch("CREATE INDEX IF NOT EXISTS idx_workspaces_path ON workspaces(path);")?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn upsert_session(&self, rec: &SessionRecord) -> Result<()> {
        let _g = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.conn()?;
        conn.execute(
            r#"
            INSERT INTO sessions (id, cwd, mode, model, status, worktree, acp_session_id, metadata_json, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(id) DO UPDATE SET
                cwd=excluded.cwd,
                mode=excluded.mode,
                model=excluded.model,
                status=excluded.status,
                worktree=excluded.worktree,
                acp_session_id=excluded.acp_session_id,
                metadata_json=excluded.metadata_json,
                updated_at=excluded.updated_at
            "#,
            params![
                rec.id.to_string(),
                rec.cwd,
                rec.mode,
                rec.model,
                rec.status,
                rec.worktree,
                rec.acp_session_id,
                rec.metadata_json,
                rec.created_at.to_rfc3339(),
                rec.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Update status + touch updated_at without rewriting full metadata.
    pub fn update_session_status(&self, id: Uuid, status: &str) -> Result<()> {
        let _g = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.conn()?;
        let n = conn.execute(
            "UPDATE sessions SET status=?1, updated_at=?2 WHERE id=?3",
            params![status, Utc::now().to_rfc3339(), id.to_string()],
        )?;
        if n == 0 {
            return Err(PersistenceError::NotFound(id.to_string()));
        }
        Ok(())
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT s.id, s.cwd, s.mode, s.model, s.status, s.worktree, s.acp_session_id,
                   s.metadata_json, s.created_at, s.updated_at,
                   (SELECT COUNT(*) FROM transcripts t WHERE t.session_id = s.id) AS msg_count
            FROM sessions s
            ORDER BY s.updated_at DESC
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SessionRecord {
                id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap_or_else(|_| Uuid::nil()),
                cwd: row.get(1)?,
                mode: row.get(2)?,
                model: row.get(3)?,
                status: row.get(4)?,
                worktree: row.get(5)?,
                acp_session_id: row.get(6)?,
                metadata_json: row.get(7)?,
                created_at: parse_dt(&row.get::<_, String>(8)?),
                updated_at: parse_dt(&row.get::<_, String>(9)?),
                message_count: row.get::<_, i64>(10)? as u64,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get_session(&self, id: Uuid) -> Result<SessionRecord> {
        let conn = self.conn()?;
        conn.query_row(
            r#"
            SELECT id, cwd, mode, model, status, worktree, acp_session_id, metadata_json,
                   created_at, updated_at,
                   (SELECT COUNT(*) FROM transcripts t WHERE t.session_id = sessions.id)
            FROM sessions WHERE id=?1
            "#,
            params![id.to_string()],
            |row| {
                Ok(SessionRecord {
                    id: Uuid::parse_str(&row.get::<_, String>(0)?).unwrap_or_else(|_| Uuid::nil()),
                    cwd: row.get(1)?,
                    mode: row.get(2)?,
                    model: row.get(3)?,
                    status: row.get(4)?,
                    worktree: row.get(5)?,
                    acp_session_id: row.get(6)?,
                    metadata_json: row.get(7)?,
                    created_at: parse_dt(&row.get::<_, String>(8)?),
                    updated_at: parse_dt(&row.get::<_, String>(9)?),
                    message_count: row.get::<_, i64>(10)? as u64,
                })
            },
        )
        .map_err(|_| PersistenceError::NotFound(id.to_string()))
    }

    pub fn delete_session(&self, id: Uuid) -> Result<()> {
        let _g = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM transcripts WHERE session_id=?1",
            params![id.to_string()],
        )?;
        conn.execute("DELETE FROM sessions WHERE id=?1", params![id.to_string()])?;
        conn.execute("DELETE FROM kv WHERE key=?1", params![format!("thread-models/{id}")])?;
        // Tombstone: late events for this id must not resurrect a ghost row.
        conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![format!("tombstone_{id}"), Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Idempotent parent-row creation (caller must hold write_lock).
    fn ensure_session_row(&self, conn: &Connection, session_id: Uuid) -> Result<bool> {
        // Deleted sessions stay deleted.
        let tombstoned: i64 = conn.query_row(
            "SELECT COUNT(1) FROM kv WHERE key=?1",
            params![format!("tombstone_{session_id}")],
            |r| r.get(0),
        )?;
        if tombstoned > 0 {
            return Ok(false);
        }
        let now = Utc::now().to_rfc3339();
        conn.execute(
            r#"
            INSERT OR IGNORE INTO sessions
              (id, cwd, mode, model, status, worktree, acp_session_id, metadata_json, created_at, updated_at)
            VALUES (?1, '', 'acp', '', 'unknown', NULL, NULL, '{}', ?2, ?2)
            "#,
            params![session_id.to_string(), now],
        )?;
        Ok(true)
    }

    /// Append a transcript line with auto-incrementing seq.
    ///
    /// MAX(seq) and the INSERT happen under one write lock/connection — two
    /// concurrent appenders previously computed the same seq and the second
    /// silently replaced the first row.
    pub fn append_message(
        &self,
        session_id: Uuid,
        kind: &str,
        payload: impl Into<String>,
        at: DateTime<Utc>,
    ) -> Result<u64> {
        let _g = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.conn()?;
        if !self.ensure_session_row(&conn, session_id)? {
            return Err(PersistenceError::NotFound(format!(
                "session {session_id} was deleted"
            )));
        }
        let max: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM transcripts WHERE session_id=?1",
            params![session_id.to_string()],
            |r| r.get(0),
        )?;
        let seq = (max as u64).saturating_add(1);
        conn.execute(
            "INSERT INTO transcripts (session_id, seq, kind, payload, at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session_id.to_string(),
                seq as i64,
                kind,
                payload.into(),
                at.to_rfc3339(),
            ],
        )?;
        let _ = conn.execute(
            "UPDATE sessions SET updated_at=?1 WHERE id=?2",
            params![Utc::now().to_rfc3339(), session_id.to_string()],
        );
        Ok(seq)
    }

    /// Append a streamed chunk, concatenating onto the previous row when it
    /// has the same kind and arrived within `window_secs`. Keeps token-level
    /// streaming deltas from becoming one DB row (and one UI line) each.
    pub fn append_message_merged(
        &self,
        session_id: Uuid,
        kind: &str,
        payload: &str,
        at: DateTime<Utc>,
        window_secs: i64,
    ) -> Result<u64> {
        let merged = {
            let _g = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
            let conn = self.conn()?;
            let last: Option<(i64, String, String)> = conn
                .query_row(
                    "SELECT seq, kind, at FROM transcripts WHERE session_id=?1 ORDER BY seq DESC LIMIT 1",
                    params![session_id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            match last {
                Some((seq, k, at_s))
                    if k == kind
                        && (at - parse_dt(&at_s)).num_seconds().abs() <= window_secs =>
                {
                    conn.execute(
                        "UPDATE transcripts SET payload = payload || ?1, at = ?2 WHERE session_id=?3 AND seq=?4",
                        params![payload, at.to_rfc3339(), session_id.to_string(), seq],
                    )?;
                    Some(seq as u64)
                }
                _ => None,
            }
        };
        match merged {
            Some(seq) => Ok(seq),
            // New row starts the message; drop leading stream whitespace.
            None => self.append_message(session_id, kind, payload.trim_start(), at),
        }
    }

    pub fn transcripts(&self, session_id: Uuid) -> Result<Vec<TranscriptChunk>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT session_id, seq, kind, payload, at FROM transcripts WHERE session_id=?1 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(params![session_id.to_string()], |row| {
            Ok(TranscriptChunk {
                session_id: Uuid::parse_str(&row.get::<_, String>(0)?)
                    .unwrap_or_else(|_| Uuid::nil()),
                seq: row.get::<_, i64>(1)? as u64,
                kind: row.get(2)?,
                payload: row.get(3)?,
                at: parse_dt(&row.get::<_, String>(4)?),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn transcript_entries(&self, session_id: Uuid) -> Result<Vec<TranscriptEntry>> {
        let chunks = self.transcripts(session_id)?;
        // Repair sessions recorded before write-side merging: token-level
        // streaming deltas were stored one row each. Fold consecutive
        // same-role agent/thought rows within a short window back together.
        let mut out: Vec<TranscriptEntry> = Vec::new();
        let mut last_at: Option<DateTime<Utc>> = None;
        for c in chunks {
            let role = kind_to_role(&c.kind);
            let mergeable = matches!(role.as_str(), "agent" | "thought");
            if mergeable {
                if let Some(prev) = out.last_mut() {
                    let close = last_at
                        .map(|t| (c.at - t).num_seconds().abs() <= 10)
                        .unwrap_or(false);
                    if prev.role == role && close {
                        // Legacy rows lost their leading spaces to trim; add a
                        // space unless punctuation continues the previous word.
                        let needs_space = !prev.body.ends_with(char::is_whitespace)
                            && !c
                                .payload
                                .chars()
                                .next()
                                .map(|ch| ".,!?;:)]}%'\"".contains(ch) || ch.is_whitespace())
                                .unwrap_or(true);
                        if needs_space {
                            prev.body.push(' ');
                        }
                        prev.body.push_str(&c.payload);
                        prev.at = c.at.to_rfc3339();
                        last_at = Some(c.at);
                        continue;
                    }
                }
            }
            last_at = Some(c.at);
            out.push(TranscriptEntry {
                role,
                body: c.payload,
                at: c.at.to_rfc3339(),
                seq: c.seq,
            });
        }
        Ok(out)
    }

    pub fn export_markdown(&self, session_id: Uuid) -> Result<String> {
        let session = self.get_session(session_id)?;
        let chunks = self.transcripts(session_id)?;
        let mut md = format!(
            "# Session {}\n\n- cwd: `{}`\n- mode: {}\n- model: {}\n- status: {}\n\n## Transcript\n\n",
            session.id, session.cwd, session.mode, session.model, session.status
        );
        for c in chunks {
            md.push_str(&format!(
                "### [{}] {}\n\n```\n{}\n```\n\n",
                c.at, c.kind, c.payload
            ));
        }
        Ok(md)
    }

    pub fn set_kv(&self, key: &str, value: &str) -> Result<()> {
        let _g = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_kv(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT value FROM kv WHERE key=?1")?;
        let mut rows = stmt.query(params![key])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn checkpoint(&self) -> Result<()> {
        info!(path = %self.path.display(), "persistence checkpoint");
        self.set_kv("last_checkpoint", &Utc::now().to_rfc3339())?;
        // Truncate WAL for a clean snapshot on disk.
        if let Ok(conn) = self.conn() {
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        }
        Ok(())
    }

}

fn kind_to_role(kind: &str) -> String {
    match kind {
        "prompt" | "user" => "user".into(),
        "agent" | "message" | "assistant" => "agent".into(),
        "thought" => "thought".into(),
        "tool" | "tool_call" => "tool".into(),
        "plan" => "plan".into(),
        "error" => "error".into(),
        // Raw ACP protocol lines — hidden by default in the UI, revealed by
        // the View toggle.
        "term" => "term".into(),
        "image" => "image".into(),
        // Permission requests render as (inert, post-restart) approval cards.
        "approval" => "approval".into(),
        _ => "system".into(),
    }
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    // Epoch, not now(): a garbage stored timestamp must not float a stale
    // thread to the top of the recency-sorted list.
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn streamed_chunks_merge_into_one_row() {
        let dir = tempdir().unwrap();
        let db = Persistence::open(dir.path().join("m.db")).unwrap();
        let id = Uuid::new_v4();
        let t = Utc::now();
        db.append_message(id, "prompt", "hi", t).unwrap();
        for chunk in ["Sup", " —", " what", " are", " we", " building", "?"] {
            db.append_message_merged(id, "agent", chunk, t, 10).unwrap();
        }
        let entries = db.transcript_entries(id).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].role, "agent");
        assert_eq!(entries[1].body, "Sup — what are we building?");
        // Different kind starts a new row.
        db.append_message_merged(id, "thought", "pondering", t, 10).unwrap();
        assert_eq!(db.transcript_entries(id).unwrap().len(), 3);
    }

    #[test]
    fn legacy_fragmented_rows_are_merged_on_read() {
        let dir = tempdir().unwrap();
        let db = Persistence::open(dir.path().join("l.db")).unwrap();
        let id = Uuid::new_v4();
        let t = Utc::now();
        // Simulate pre-fix rows: one token per row, leading spaces lost.
        for tok in ["Sup", "—", "what", "are", "we", "building", "?"] {
            db.append_message(id, "agent", tok, t).unwrap();
        }
        db.append_message(id, "prompt", "next", t).unwrap();
        let entries = db.transcript_entries(id).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].body, "Sup — what are we building?");
        assert_eq!(entries[1].role, "user");
    }

    #[test]
    fn session_and_transcript_roundtrip() {
        let dir = tempdir().unwrap();
        let db = Persistence::open(dir.path().join("t.db")).unwrap();
        let id = Uuid::new_v4();
        let rec = SessionRecord {
            id,
            cwd: "/tmp/proj".into(),
            mode: "acp".into(),
            model: "grok-4".into(),
            status: "idle".into(),
            worktree: None,
            acp_session_id: Some("s1".into()),
            metadata_json: "{}".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            message_count: 0,
        };
        db.upsert_session(&rec).unwrap();
        db.append_message(id, "prompt", "hello world", Utc::now())
            .unwrap();
        db.append_message(id, "agent", "hi back", Utc::now()).unwrap();
        let loaded = db.get_session(id).unwrap();
        assert_eq!(loaded.cwd, "/tmp/proj");
        assert_eq!(loaded.message_count, 2);
        let entries = db.transcript_entries(id).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].role, "user");
        assert_eq!(entries[1].role, "agent");
        let list = db.list_sessions().unwrap();
        assert_eq!(list[0].message_count, 2);
    }

    #[test]
    fn survives_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mem.db");
        let id = Uuid::new_v4();
        {
            let db = Persistence::open(&path).unwrap();
            db.upsert_session(&SessionRecord {
                id,
                cwd: "/Users/me/app".into(),
                mode: "acp".into(),
                model: "grok".into(),
                status: "idle".into(),
                worktree: None,
                acp_session_id: None,
                metadata_json: "{}".into(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
                message_count: 0,
            })
            .unwrap();
            db.append_message(id, "prompt", "build a game", Utc::now())
                .unwrap();
            db.append_message(id, "agent", "sure, starting…", Utc::now())
                .unwrap();
        }
        let db2 = Persistence::open(&path).unwrap();
        let entries = db2.transcript_entries(id).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[1].body.contains("starting"));
    }
}
