//! `elicitation/create` forms as questions, and answers back into the form's content.
//!
//! Codex's clarifying questions arrive this way: one string field per question, its choices as
//! `oneOf`, and for questions that also take a typed answer a "None of the above" choice plus a
//! note field (`_meta.codex.role == "user_note"`). MCP servers' simple forms fit the same shape.

use grok_events::{QuestionChoice, QuestionInfo};
use serde_json::{json, Map, Value};

/// Codex's "my own answer" choice; the typed text goes in the note field.
const OTHER_CHOICE: &str = "None of the above";

/// The questions in a form request, or `None` for forms we can't show (URLs, numbers, nested
/// objects): those are declined so the agent carries on.
pub fn parse(params: &Value) -> Option<(String, Vec<QuestionInfo>)> {
    if params.get("mode").and_then(Value::as_str).is_some_and(|m| m != "form") {
        return None;
    }
    let schema = params.get("requestedSchema")?;
    let properties = schema.get("properties")?.as_object()?;
    // Ask in the form's own order: `required` lists every question, then any optional fields.
    let mut order: Vec<&str> = schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    for key in properties.keys() {
        if !order.contains(&key.as_str()) {
            order.push(key);
        }
    }
    let note_of = |field: &Value| field.pointer("/_meta/codex/role").and_then(Value::as_str) == Some("user_note");
    let mut questions = Vec::new();
    for id in order {
        let Some(field) = properties.get(id) else { continue };
        if note_of(field) {
            continue;
        }
        if field.get("type").and_then(Value::as_str).is_some_and(|t| t != "string") {
            return None;
        }
        let choices: Vec<QuestionChoice> = match (field.get("oneOf").and_then(Value::as_array), field.get("enum").and_then(Value::as_array)) {
            (Some(one_of), _) => one_of.iter().filter_map(|c| {
                let value = c.get("const").and_then(Value::as_str)?.to_string();
                let label = c.get("title").and_then(Value::as_str).unwrap_or(&value).to_string();
                Some(QuestionChoice { value, label, description: c.get("description").and_then(Value::as_str).map(str::to_string) })
            }).collect(),
            (None, Some(values)) => values.iter().filter_map(Value::as_str).map(|v| QuestionChoice { value: v.into(), label: v.into(), description: None }).collect(),
            _ => Vec::new(),
        };
        let note_id = properties.iter()
            .find(|(_, f)| note_of(f) && f.pointer("/_meta/codex/questionId").and_then(Value::as_str) == Some(id))
            .map(|(k, _)| k.clone());
        let other_choice = note_id.as_ref().and_then(|_| choices.iter().find(|c| c.value == OTHER_CHOICE)).map(|c| c.value.clone());
        // "None of the above" is what typing an answer means; it isn't a button of its own.
        let choices = choices.into_iter().filter(|c| Some(&c.value) != other_choice.as_ref()).collect();
        questions.push(QuestionInfo {
            id: id.to_string(),
            title: field.get("title").and_then(Value::as_str).unwrap_or(id).to_string(),
            description: field.get("description").and_then(Value::as_str).map(str::to_string),
            choices,
            note_id,
            other_choice,
            secret: field.pointer("/_meta/codex/isSecret").and_then(Value::as_bool).unwrap_or(false),
        });
    }
    if questions.is_empty() {
        return None;
    }
    let message = params.get("message").and_then(Value::as_str).unwrap_or("The agent needs your input.").to_string();
    Some((message, questions))
}

/// The form content for `answers` (question id → a choice's label or value, or typed text).
pub fn content(questions: &[QuestionInfo], answers: &[(String, String)]) -> Value {
    let mut out = Map::new();
    for q in questions {
        let Some((_, answer)) = answers.iter().find(|(id, _)| *id == q.id) else { continue };
        let answer = answer.trim();
        if answer.is_empty() {
            continue;
        }
        let picked = q.choices.iter().find(|c| c.value == answer || c.label == answer);
        match (picked, &q.note_id) {
            (Some(choice), _) => { out.insert(q.id.clone(), json!(choice.value)); }
            // Typed next to choices: "None of the above", and the words in the note.
            (None, Some(note)) => {
                out.insert(q.id.clone(), json!(q.other_choice.clone().unwrap_or_else(|| OTHER_CHOICE.into())));
                out.insert(note.clone(), json!(answer));
            }
            (None, None) => { out.insert(q.id.clone(), json!(answer)); }
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codex_form() -> Value {
        json!({
            "sessionId": "s", "toolCallId": "item", "mode": "form",
            "message": "Codex needs your input to continue.",
            "requestedSchema": {"type": "object", "required": ["storage", "name"], "properties": {
                "storage": {"type": "string", "title": "Where should sessions live?", "description": "Storage",
                    "_meta": {"codex": {"isOther": true, "isSecret": false}},
                    "oneOf": [
                        {"const": "SQLite", "title": "SQLite", "description": "Local file"},
                        {"const": "Postgres", "title": "Postgres"},
                        {"const": "None of the above", "title": "None of the above", "description": "Provide a different answer in the note field."}
                    ]},
                "storage_note": {"type": "string", "title": "Additional answer or note",
                    "_meta": {"codex": {"questionId": "storage", "role": "user_note"}}},
                "name": {"type": "string", "title": "What should the table be called?"}
            }}
        })
    }

    #[test]
    fn codex_questions_keep_their_order_choices_and_typed_answers() {
        let (message, qs) = parse(&codex_form()).unwrap();
        assert_eq!(message, "Codex needs your input to continue.");
        assert_eq!(qs.iter().map(|q| q.id.as_str()).collect::<Vec<_>>(), ["storage", "name"]);
        assert_eq!(qs[0].choices.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["SQLite", "Postgres"]);
        assert_eq!(qs[0].note_id.as_deref(), Some("storage_note"));
        assert!(qs[1].choices.is_empty() && qs[1].note_id.is_none());

        let picked = content(&qs, &[("storage".into(), "Postgres".into()), ("name".into(), "sessions".into())]);
        assert_eq!(picked, json!({"storage": "Postgres", "name": "sessions"}));
        let typed = content(&qs, &[("storage".into(), "Redis".into())]);
        assert_eq!(typed, json!({"storage": "None of the above", "storage_note": "Redis"}));
    }

    #[test]
    fn forms_we_cannot_show_are_left_alone() {
        assert!(parse(&json!({"mode": "url", "url": "https://x"})).is_none());
        assert!(parse(&json!({"mode": "form", "requestedSchema": {"type": "object", "properties": {"n": {"type": "number"}}}})).is_none());
        assert!(parse(&json!({"mode": "form", "requestedSchema": {"type": "object", "properties": {}}})).is_none());
    }
}
