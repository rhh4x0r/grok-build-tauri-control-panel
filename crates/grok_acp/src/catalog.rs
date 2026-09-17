//! Model IDs and display names reported by an ACP session, never a static catalog.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ModelCatalog {
    pub models: Vec<AvailableModel>,
    #[serde(default)]
    pub commands: Option<Value>,
    pub current: Option<String>,
    pub config_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AvailableModel {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}
impl ModelCatalog {
    pub fn from_response(value: &Value) -> Option<Self> {
        let config = value.get("configOptions").and_then(Value::as_array).and_then(|options| options.iter().find(|option| {
            option.get("id").and_then(Value::as_str) == Some("model") || option.get("category").and_then(Value::as_str) == Some("model")
        }));
        if let Some(option) = config {
            let mut models = Vec::new();
            for item in option.get("options").and_then(Value::as_array).into_iter().flatten() {
                for item in item.get("options").and_then(Value::as_array).map(|v| v.as_slice()).unwrap_or(std::slice::from_ref(item)) {
                    if let Some(id) = item.get("value").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                        if !models.iter().any(|model: &AvailableModel| model.id == id) { models.push(AvailableModel {
                            id: id.into(), name: item.get("name").and_then(Value::as_str).unwrap_or(id).into(),
                            description: item.get("description").and_then(Value::as_str).map(Into::into),
                        }); }
                    }
                }
            }
            return Some(Self { commands: None, models, current: option.get("currentValue").and_then(Value::as_str).map(Into::into), config_id: option.get("id").and_then(Value::as_str).map(Into::into) });
        }
        let state = value.get("models")?;
        let available = state.get("availableModels").and_then(Value::as_array)?;
        let models = available.iter().filter_map(|model| {
            let id = model.get("modelId").and_then(Value::as_str).filter(|s| !s.is_empty())?;
            Some(AvailableModel { id: id.into(), name: model.get("name").and_then(Value::as_str).unwrap_or(id).into(), description: model.get("description").and_then(Value::as_str).map(Into::into) })
        }).collect();
        Some(Self { commands: None, models, current: state.get("currentModelId").and_then(Value::as_str).map(Into::into), config_id: None })
    }
}
#[cfg(test)]
mod tests {
    use super::ModelCatalog;
    use serde_json::json;
    #[test]
    fn reads_exact_aliases_and_names_from_grouped_options_over_legacy_models() {
        let result = ModelCatalog::from_response(&json!({"models":{"availableModels":[{"modelId":"stale"}]},"configOptions":[{"id":"model","currentValue":"opus","options":[{"name":"Claude","options":[{"value":"opus","name":"Claude Opus (available)"},{"value":"sonnet","name":"Claude Sonnet"}]}]}]})).unwrap();
        assert_eq!(result.models.iter().map(|m|m.id.as_str()).collect::<Vec<_>>(), vec!["opus","sonnet"]);
        assert_eq!(result.models[0].name, "Claude Opus (available)");
        assert_eq!(result.current.as_deref(), Some("opus"));
    }
    #[test]
    fn legacy_and_missing_catalogs_do_not_invent_choices() {
        let result = ModelCatalog::from_response(&json!({"models":{"currentModelId":"real-id","availableModels":[{"modelId":"real-id","name":"Real Model"}]}})).unwrap();
        assert_eq!(result.models[0].id, "real-id");
        assert!(ModelCatalog::from_response(&json!({})).is_none());
        assert!(ModelCatalog::from_response(&json!({"configOptions":[{"id":"model","options":[]}]})).unwrap().models.is_empty());
    }
}
