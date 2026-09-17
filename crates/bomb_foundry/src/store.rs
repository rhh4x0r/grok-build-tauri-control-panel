use crate::{Document, Run, RunStatus};
use rusqlite::{params, Connection};
use std::{path::Path, sync::Mutex};
pub struct Store(Mutex<Connection>);
impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let db = Connection::open(path).map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS foundry_documents(id TEXT NOT NULL, revision INTEGER NOT NULL, body TEXT NOT NULL, PRIMARY KEY(id,revision)); CREATE TABLE IF NOT EXISTS foundry_runs(id TEXT PRIMARY KEY, body TEXT NOT NULL);").map_err(|e|e.to_string())?;
        let store = Self(Mutex::new(db));
        for mut run in store.runs()? {
            if matches!(run.status, RunStatus::Running | RunStatus::Ready) {
                run.status = RunStatus::Interrupted;
                run.note="App restarted; inspect the last attempt before explicitly resuming. Commands will not be replayed automatically.".into();
                store.save_run(&run)?;
            }
        }
        Ok(store)
    }
    pub fn save(&self, document: &mut Document) -> Result<(), String> {
        document.contract.validate()?;
        document.graph.validate(false)?;
        let mut db = self.0.lock().map_err(|e| e.to_string())?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let revision: u64 = tx
            .query_row(
                "SELECT COALESCE(MAX(revision),0) FROM foundry_documents WHERE id=?",
                [&document.id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if revision != document.revision {
            return Err("A newer document revision exists; reload before saving".into());
        }
        if revision > 0 {
            let previous: String = tx
                .query_row(
                    "SELECT body FROM foundry_documents WHERE id=? AND revision=?",
                    params![document.id, revision],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if previous == serde_json::to_string(document).map_err(|e| e.to_string())? {
                return Ok(());
            }
        }
        let mut next = document.clone();
        next.revision += 1;
        tx.execute(
            "INSERT INTO foundry_documents VALUES(?,?,?)",
            params![
                next.id,
                next.revision,
                serde_json::to_string(&next).map_err(|e| e.to_string())?
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        *document = next;
        Ok(())
    }
    pub fn documents(&self) -> Result<Vec<Document>, String> {
        let db = self.0.lock().map_err(|e| e.to_string())?;
        let mut st=db.prepare("SELECT body FROM foundry_documents d WHERE revision=(SELECT MAX(revision) FROM foundry_documents WHERE id=d.id) ORDER BY rowid DESC").map_err(|e|e.to_string())?;
        let rows = st
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|r| {
            serde_json::from_str(&r.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
        })
        .collect()
    }
    pub fn revisions(&self, id: &str) -> Result<Vec<Document>, String> {
        let db = self.0.lock().map_err(|e| e.to_string())?;
        let mut st = db
            .prepare("SELECT body FROM foundry_documents WHERE id=? ORDER BY revision DESC")
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([id], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|r| {
            serde_json::from_str(&r.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
        })
        .collect()
    }
    pub fn save_run(&self, run: &Run) -> Result<(), String> {
        self.0.lock().map_err(|e|e.to_string())?.execute("INSERT INTO foundry_runs VALUES(?,?) ON CONFLICT(id) DO UPDATE SET body=excluded.body",params![run.id,serde_json::to_string(run).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub fn runs(&self) -> Result<Vec<Run>, String> {
        let db = self.0.lock().map_err(|e| e.to_string())?;
        let mut st = db
            .prepare("SELECT body FROM foundry_runs ORDER BY rowid DESC")
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|r| {
            serde_json::from_str(&r.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
        })
        .collect()
    }
}
pub fn export_package(document: &Document, dir: &Path) -> Result<(), String> {
    document.contract.validate()?;
    document.graph.validate(false)?;
    // The UI chooses a parent folder. Export always creates a unique child, never overwrites files.
    std::fs::create_dir(dir).map_err(|e| e.to_string())?;
    let write =
        |name: &str, body: String| std::fs::write(dir.join(name), body).map_err(|e| e.to_string());
    write(
        "foundry.json",
        serde_json::to_string_pretty(document).map_err(|e| e.to_string())?,
    )?;
    write(
        "project-contract.json",
        serde_json::to_string_pretty(&document.contract).unwrap(),
    )?;
    write(
        "skill-graph.json",
        serde_json::to_string_pretty(&document.graph).unwrap(),
    )?;
    write("contract.md", document.contract.markdown())?;
    write("SKILL.md", document.graph.markdown()?)?;
    write("LIMITATIONS.md",format!("# Limitations\n{}\n\nMarkdown is a lossy projection. Export does not install or execute.\n",document.graph.limitations.join("\n")))?;
    write("PROVENANCE.md",format!("# Provenance\nBomb Code revision {}.\nUpstream Prompt Foundry: https://github.com/jedisherpa/prompt-foundry\n{}\nAttribution: {}\n",document.revision,document.graph.generation_metadata,serde_json::to_string_pretty(&document.attribution).unwrap()))?;
    let refs = dir.join("references");
    std::fs::create_dir(&refs).map_err(|e| e.to_string())?;
    for (i, n) in document.graph.ordered().iter().enumerate() {
        if n.prompt.len() > 400 {
            std::fs::write(
                refs.join(format!("stage-{i}.md")),
                format!("# {}\n\n{}", n.title, n.prompt),
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
