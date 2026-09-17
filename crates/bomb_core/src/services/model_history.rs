//! Durable model identities, independent of ACP process lifetime.
use grok_persistence::{ModelUsage, Persistence};
use uuid::Uuid;

pub fn load(db: &Persistence, id: Uuid) -> Vec<ModelUsage> {
    db.get_kv(&format!("thread-models/{id}")).ok().flatten()
        .and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default()
}

pub fn record(db: &Persistence, id: Uuid, backend: &str, model: &str) {
    if model.is_empty() { return; }
    let mut models = load(db, id);
    let item = ModelUsage { backend: backend.into(), model: model.into() };
    if models.contains(&item) { return; }
    models.push(item);
    if let Ok(json) = serde_json::to_string(&models) {
        if let Err(error) = db.set_kv(&format!("thread-models/{id}"), &json) { tracing::warn!(%error, "Could not save model history"); }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn model_history_survives_reopen_and_deduplicates_models_not_providers() {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("state.db");
        let id = uuid::Uuid::new_v4();
        {
            let db = grok_persistence::Persistence::open(&path).unwrap();
            for (backend, model) in [("codex", "astra"), ("codex", "sol"), ("grok", "grok-4.6"), ("codex", "astra")] { super::record(&db, id, backend, model); }
        }
        let db = grok_persistence::Persistence::open(path).unwrap();
        let models = super::load(&db, id);
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].model, "astra");
        assert_eq!(models[1].model, "sol");
        db.delete_session(id).unwrap();
        assert!(super::load(&db, id).is_empty());
    }
}
