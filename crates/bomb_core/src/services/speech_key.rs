//! Read-aloud settings: the Fish Audio API key, the chosen voice, and the speed. They live in this
//! core's settings table and are shared by every device of the person who owns it (a paired phone
//! reads and changes them over its connection), so the key is entered once. The key sits with the
//! other API keys (owner-only database), goes nowhere but Fish Audio and a paired device of the
//! owner, and is never logged; the change notice carries no settings, only that they changed.

use std::sync::Arc;

use grok_persistence::Persistence;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::AppState;

const SLOT: &str = "settings/credentials/fish_audio";
/// The voice and speed, as JSON (`voice`, `voice_name`, `rate`); the Mac's player keeps more here.
const CHOICES: &str = "read_aloud";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechSettings {
    pub api_key: Option<String>,
    pub voice: Option<String>,
    pub voice_name: Option<String>,
    pub rate: Option<f32>,
}

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

/// Everything a device needs to read aloud like this one.
pub async fn settings(state: &AppState) -> SpeechSettings {
    let choices: Value = state.persistence.get_kv(CHOICES).ok().flatten()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or(Value::Null);
    let text = |name: &str| choices.get(name).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
    SpeechSettings {
        api_key: load(state.persistence.clone()).await,
        // An Apple voice id from before Fish Audio ("com.apple…") isn't a Fish voice.
        voice: text("voice").filter(|v| !v.contains('.')),
        voice_name: text("voice_name"),
        rate: choices.get("rate").and_then(Value::as_f64).map(|r| r as f32),
    }
}

/// Change some of the settings (`None` leaves one as it is; an empty key removes the key), then
/// tell every device. A key is expected to have been checked with Fish Audio by the caller.
pub async fn update(state: &AppState, api_key: Option<String>, voice: Option<(String, String)>, rate: Option<f32>) -> Result<(), String> {
    match api_key {
        Some(key) if key.trim().is_empty() => remove(state.persistence.clone()).await?,
        Some(key) => save(state.persistence.clone(), key).await?,
        None => {}
    }
    if voice.is_some() || rate.is_some() {
        let mut choices: Value = state.persistence.get_kv(CHOICES).ok().flatten()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({ "rate": 1.0 }));
        if let Some((id, name)) = voice {
            choices["voice"] = json!(id);
            choices["voice_name"] = json!(name);
        }
        if let Some(rate) = rate.filter(|r| (0.5..=3.0).contains(r)) {
            choices["rate"] = json!(rate);
        }
        state.persistence.set_kv(CHOICES, &choices.to_string()).map_err(|e| e.to_string())?;
    }
    state.event_bus.emit(grok_events::ControlEvent::Raw {
        session_id: None,
        payload: json!({ "channel": "settings", "kind": "speech" }),
    });
    Ok(())
}
