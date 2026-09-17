//! Durable workspaces are independent of conversation lifetimes.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceRecord {
    pub id: String,
    pub project_root: String,
    pub name: String,
    pub branch: String,
    pub path: String,
    pub base_ref: String,
    pub created_at: String,
    pub archived_at: Option<String>,
    pub inline: bool,
    #[serde(default)]
    pub threads: Vec<String>,
}

impl Persistence {
    pub fn save_workspace(&self, workspace: &WorkspaceRecord) -> Result<()> {
        let _guard = self.write_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.conn()?.execute(
            "INSERT INTO workspaces(id, path, data) VALUES (?1, ?2, ?3) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
            params![workspace.id, workspace.path, serde_json::to_string(workspace)?],
        )?;
        Ok(())
    }

    pub fn attach_workspace(&self, session: Uuid, workspace: &str) -> Result<()> {
        self.conn()?.execute(
            "INSERT INTO session_workspaces(session_id, workspace_id) VALUES (?1, ?2) ON CONFLICT(session_id) DO UPDATE SET workspace_id=excluded.workspace_id",
            params![session.to_string(), workspace],
        )?;
        Ok(())
    }

    pub fn workspace_for_session(&self, session: Uuid) -> Result<Option<WorkspaceRecord>> {
        Ok(self
            .list_workspaces()?
            .into_iter()
            .find(|w| w.threads.contains(&session.to_string())))
    }

    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT data FROM workspaces ORDER BY rowid DESC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut result = Vec::new();
        for row in rows {
            let mut w: WorkspaceRecord = serde_json::from_str(&row?)?;
            let mut threads = conn.prepare("SELECT sw.session_id FROM session_workspaces sw JOIN sessions s ON s.id=sw.session_id WHERE workspace_id=?1 ORDER BY s.updated_at DESC")?;
            w.threads = threads
                .query_map([&w.id], |r| r.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            result.push(w);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_survives_conversation_deletion_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.db");
        let db = Persistence::open(&path).unwrap();
        let w = WorkspaceRecord {
            id: "w".into(),
            project_root: "/project".into(),
            name: "Feature".into(),
            branch: "bomb/feature".into(),
            path: "/worktree".into(),
            base_ref: "main".into(),
            created_at: Utc::now().to_rfc3339(),
            archived_at: None,
            inline: false,
            threads: vec![],
        };
        db.save_workspace(&w).unwrap();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        for id in [a, b] {
            db.append_message(id, "prompt", "hello", Utc::now())
                .unwrap();
            db.attach_workspace(id, "w").unwrap();
        }
        assert_eq!(db.list_workspaces().unwrap()[0].threads.len(), 2);
        db.delete_session(a).unwrap();
        assert_eq!(
            db.workspace_for_session(b).unwrap().unwrap().name,
            "Feature"
        );
        drop(db);
        let db = Persistence::open(path).unwrap();
        let mut loaded = db.list_workspaces().unwrap().remove(0);
        assert_eq!(loaded.threads, vec![b.to_string()]);
        loaded.archived_at = Some(Utc::now().to_rfc3339());
        db.save_workspace(&loaded).unwrap();
        assert!(
            db.workspace_for_session(b)
                .unwrap()
                .unwrap()
                .archived_at
                .is_some()
        );
        db.delete_session(b).unwrap();
        assert_eq!(db.list_workspaces().unwrap().len(), 1);
    }
}
