//! The Fish Audio API key that read-aloud uses on this machine. Kept in the settings table
//! (owner-only, like the other API keys), never sent anywhere but Fish Audio, never logged.

use std::sync::Arc;

use grok_persistence::Persistence;

const SLOT: &str = "settings/credentials/fish_audio";

pub async fn save(db: Arc<Persistence>, key: String) -> Result<(), String> {
    let key = key.trim().to_string();
    if key.is_empty() || key.contains(['\r', '\n']) {
        return Err("Enter a valid API key.".into());
    }
    tokio::task::spawn_blocking(move || {
        super::model_suggestions::keys::protect_database(&db)?;
        db.set_kv(SLOT, &key).map_err(|_| "Could not save the key.".to_string())
    })
    .await
    .map_err(|_| "Could not save the key.".to_string())?
}

pub async fn remove(db: Arc<Persistence>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || db.set_kv(SLOT, "").map_err(|_| "Could not remove the key.".to_string()))
        .await
        .map_err(|_| "Could not remove the key.".to_string())?
}

/// The saved key, if there is one.
pub async fn load(db: Arc<Persistence>) -> Option<String> {
    tokio::task::spawn_blocking(move || db.get_kv(SLOT).ok().flatten().filter(|k| !k.trim().is_empty()))
        .await
        .ok()
        .flatten()
}
