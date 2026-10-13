//! `elicitation/create` forms as questions, and answers back into the form's content.
//!
//! Two agents ask this way:
//! - Codex: one string field per question, its choices as `oneOf`; a question that also takes a
//!   typed answer has a "None of the above" choice plus a note field (`_meta.codex.role == "user_note"`).
//! - Claude (AskUserQuestion): `question_<n>` titled with a short header, its text in the field's
//!   description or, for a lone question, the form's message; choices as `oneOf`, or for a
//!   multi-select an array of `anyOf` items; and an optional `question_<n>_custom` "Other" box.
//!
//! MCP servers' simple forms fit the same shape.

use grok_events::{QuestionChoice, QuestionInfo};
use serde_json::{json, Map, Value};

/// Codex's "my own answer" choice; the typed text goes in the note field.
const OTHER_CHOICE: &str = "None of the above";

fn choices_of(list: Option<&Vec<Value>>, plain: Option<&Vec<Value>>) -> Vec<QuestionChoice> {
    match (list, plain) {
        (Some(options), _) => options.iter().filter_map(|c| {
            let value = c.get("const").and_then(Value::as_str)?.to_string();
            let label = c.get("title").and_then(Value::as_str).unwrap_or(&value).to_string();
            Some(QuestionChoice { value, label, description: c.get("description").and_then(Value::as_str).map(str::to_string) })
        }).collect(),
        (None, Some(values)) => values.iter().filter_map(Value::as_str).map(|v| QuestionChoice { value: v.into(), label: v.into(), description: None }).collect(),
        _ => Vec::new(),
    }
}

/// The questions in a form request, or `None` for forms we can't show (URLs, numbers, nested
/// objects): those are declined so the agent carries on.
pub fn parse(params: &Value) -> Option<(String, Vec<QuestionInfo>)> {
    if params.get("mode").and_then(Value::as_str).is_some_and(|m| m != "form") {
        return None;
    }
    let schema = params.get("requestedSchema")?;
    let properties = schema.get("properties")?.as_object()?;
    let message = params.get("message").and_then(Value::as_str).unwrap_or("The agent needs your input.").to_string();
    // Ask in the form's own order: `required` first, then the rest.
    let required: Vec<&str> = schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let mut order = required.clone();
    for key in properties.keys() {
        if !order.contains(&key.as_str()) {
            order.push(key);
        }
    }
    // A note field belongs to a question: Codex marks it, Claude names it `<question>_custom`.
    let note_for = |key: &str, field: &Value| -> Option<String> {
        if field.pointer("/_meta/codex/role").and_then(Value::as_str) == Some("user_note") {
            return field.pointer("/_meta/codex/questionId").and_then(Value::as_str).map(str::to_string);
        }
        key.strip_suffix("_custom").filter(|q| properties.contains_key(*q)).map(str::to_string)
    };
    let asked: Vec<&str> = order.iter().copied().filter(|k| properties.get(*k).is_some_and(|f| note_for(k, f).is_none())).collect();
    let mut questions = Vec::new();
    for id in &asked {
        let field = &properties[*id];
        let (choices, multi) = match field.get("type").and_then(Value::as_str) {
            Some("array") => {
                let items = field.get("items")?;
                (choices_of(items.get("anyOf").or(items.get("oneOf")).and_then(Value::as_array), items.get("enum").and_then(Value::as_array)), true)
            }
            None | Some("string") => (choices_of(field.get("oneOf").and_then(Value::as_array), field.get("enum").and_then(Value::as_array)), false),
            Some(_) => return None,
        };
        if multi && choices.is_empty() {
            return None;
        }
        let note_id = properties.iter().find(|(k, f)| note_for(k, f).as_deref() == Some(*id)).map(|(k, _)| k.clone());
        let other_choice = note_id.as_ref().and_then(|_| choices.iter().find(|c| c.value == OTHER_CHOICE)).map(|c| c.value.clone());
        // "None of the above" is what typing an answer means; it isn't a button of its own.
        let choices = choices.into_iter().filter(|c| Some(&c.value) != other_choice.as_ref()).collect();
        // The question itself: the field's description, or the form's message for a lone question;
        // the field's title is then just its short header.
        let header = field.get("title").and_then(Value::as_str).map(str::to_string);
        let text = field.get("description").and_then(Value::as_str).map(str::to_string)
            .or_else(|| (asked.len() == 1 && header.as_deref() != Some(message.as_str())).then(|| message.clone()));
        let (title, description) = match (text, header) {
            (Some(text), header) => (text.clone(), header.filter(|h| *h != text)),
            (None, Some(header)) => (header, None),
            (None, None) => (id.to_string(), None),
        };
        questions.push(QuestionInfo {
            id: id.to_string(),
            title,
            description,
            choices,
            note_id,
            other_choice,
            secret: field.pointer("/_meta/codex/isSecret").and_then(Value::as_bool).unwrap_or(false),
            required: required.contains(id),
            multi,
        });
    }
    if questions.is_empty() {
        return None;
    }
    Some((message, questions))
}

/// The form content for `answers`: each `(question id, answer)` is a choice's value or label, or
/// typed text. A multi-select takes several; typed text goes in the question's note box if it
/// has one (Codex also picks "None of the above" when nothing else was chosen).
pub fn content(questions: &[QuestionInfo], answers: &[(String, String)]) -> Value {
    let mut out = Map::new();
    for q in questions {
        let given: Vec<&str> = answers.iter().filter(|(id, _)| *id == q.id).map(|(_, a)| a.trim()).filter(|a| !a.is_empty()).collect();
        let chosen: Vec<String> = given.iter().filter_map(|a| q.choices.iter().find(|c| c.value == *a || c.label == *a)).map(|c| c.value.clone()).collect();
        let typed: Vec<&str> = given.iter().copied().filter(|a| !q.choices.iter().any(|c| c.value == *a || c.label == *a)).collect();
        match (q.multi, chosen.first()) {
            (true, Some(_)) => { out.insert(q.id.clone(), json!(chosen)); }
            (false, Some(first)) => { out.insert(q.id.clone(), json!(first)); }
            _ => {}
        }
        if typed.is_empty() {
            continue;
        }
        let typed = typed.join("\n");
        match &q.note_id {
            Some(note) => {
                if let (true, Some(other)) = (chosen.is_empty(), &q.other_choice) { out.insert(q.id.clone(), json!(other)); }
                out.insert(note.clone(), json!(typed));
            }
            None if chosen.is_empty() => { out.insert(q.id.clone(), if q.multi { json!([typed]) } else { json!(typed) }); }
            None => {}
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
        assert!(qs[0].required && qs[1].required);

        let picked = content(&qs, &[("storage".into(), "Postgres".into()), ("name".into(), "sessions".into())]);
        assert_eq!(picked, json!({"storage": "Postgres", "name": "sessions"}));
        let typed = content(&qs, &[("storage".into(), "Redis".into())]);
        assert_eq!(typed, json!({"storage": "None of the above", "storage_note": "Redis"}));
    }

    #[test]
    fn an_optional_free_text_field_is_not_required() {
        // Claude's AskUserQuestion form: a choice, then an optional "Other" note.
        let form = json!({"mode": "form", "message": "?", "requestedSchema": {"type": "object", "required": ["server.env"], "properties": {
            "server.env": {"type": "string", "title": "server.env", "enum": ["List names only (Recommended)", "Pass it blind"]},
            "Other": {"type": "string", "title": "Other", "description": "Type your own answer (optional)."}
        }}});
        let (_, qs) = parse(&form).unwrap();
        assert_eq!(qs.iter().map(|q| (q.id.as_str(), q.required)).collect::<Vec<_>>(), [("server.env", true), ("Other", false)]);
    }

    #[test]
    fn claude_questions_show_their_text_and_keep_other_with_the_question() {
        // A lone question: its text is the form's message, the field title is its header.
        let lone = json!({"mode": "form", "message": "How should the server see the admin checkout's env?", "requestedSchema": {"type": "object", "properties": {
            "question_0": {"type": "string", "title": "server.env", "oneOf": [{"const": "List names only (Recommended)", "title": "List names only (Recommended)"}, {"const": "Pass it blind", "title": "Pass it blind"}]},
            "question_0_custom": {"type": "string", "title": "Other", "description": "Type your own answer, or add a note to the option you chose above (optional)."}
        }}});
        let (_, qs) = parse(&lone).unwrap();
        assert_eq!(qs.len(), 1, "Other belongs to the question, it isn't one");
        assert_eq!(qs[0].title, "How should the server see the admin checkout's env?");
        assert_eq!(qs[0].description.as_deref(), Some("server.env"));
        assert_eq!(qs[0].note_id.as_deref(), Some("question_0_custom"));
        assert_eq!(content(&qs, &[("question_0".into(), "Pass it blind".into())]), json!({"question_0": "Pass it blind"}));
        assert_eq!(content(&qs, &[("question_0".into(), "Only in staging".into())]), json!({"question_0_custom": "Only in staging"}));
        assert_eq!(content(&qs, &[("question_0".into(), "Pass it blind".into()), ("question_0".into(), "but log it".into())]),
            json!({"question_0": "Pass it blind", "question_0_custom": "but log it"}));

        // Several questions, one multi-select: each text is its field's description.
        let many = json!({"mode": "form", "message": "Please answer the following questions.", "requestedSchema": {"type": "object", "properties": {
            "question_0": {"type": "array", "title": "Targets", "description": "Which platforms?", "items": {"anyOf": [{"const": "mac", "title": "Mac"}, {"const": "ios", "title": "iPhone"}]}},
            "question_0_custom": {"type": "string", "title": "Other"},
            "question_1": {"type": "string", "title": "Style", "description": "Which look?", "oneOf": [{"const": "dark", "title": "Dark"}]},
            "question_1_custom": {"type": "string", "title": "Other"}
        }}});
        let (_, qs) = parse(&many).unwrap();
        assert_eq!(qs.iter().map(|q| (q.title.as_str(), q.multi)).collect::<Vec<_>>(), [("Which platforms?", true), ("Which look?", false)]);
        assert_eq!(content(&qs, &[("question_0".into(), "Mac".into()), ("question_0".into(), "iPhone".into()), ("question_1".into(), "Dark".into())]),
            json!({"question_0": ["mac", "ios"], "question_1": "dark"}));
    }

    #[test]
    fn forms_we_cannot_show_are_left_alone() {
        assert!(parse(&json!({"mode": "url", "url": "https://x"})).is_none());
        assert!(parse(&json!({"mode": "form", "requestedSchema": {"type": "object", "properties": {"n": {"type": "number"}}}})).is_none());
        assert!(parse(&json!({"mode": "form", "requestedSchema": {"type": "object", "properties": {}}})).is_none());
    }
}
