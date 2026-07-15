//! JSON-RPC 2.0 and ACP method payloads.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC id may be string or number on the wire.
pub fn id_key(id: &Value) -> String {
    match id {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    /// Wire id (string or number).
    pub id: Value,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Wire message discrimination.
///
/// IMPORTANT: Do NOT use `#[serde(untagged)]` with Response-first ordering.
/// `JsonRpcResponse` only requires `jsonrpc` (id/result/error optional), so serde
/// happily parses `session/update` notifications and agent→client requests as
/// empty Responses — speech never reaches the UI and terminal/fs calls hang.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    Response(JsonRpcResponse),
    Request(JsonRpcRequest),
    Notification(JsonRpcNotification),
}

impl<'de> Deserialize<'de> for JsonRpcMessage {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = Value::deserialize(deserializer)?;
        let obj = v.as_object().ok_or_else(|| {
            serde::de::Error::custom("JSON-RPC message must be an object")
        })?;

        let has_method = obj.contains_key("method");
        let has_id = obj.contains_key("id");
        let has_result = obj.contains_key("result");
        let has_error = obj.contains_key("error");

        if has_method && has_id {
            // Agent → client request (or client→agent request echoed).
            let req: JsonRpcRequest = serde_json::from_value(v).map_err(serde::de::Error::custom)?;
            return Ok(JsonRpcMessage::Request(req));
        }
        if has_method && !has_id {
            let n: JsonRpcNotification =
                serde_json::from_value(v).map_err(serde::de::Error::custom)?;
            return Ok(JsonRpcMessage::Notification(n));
        }
        if has_result || has_error || has_id {
            let r: JsonRpcResponse = serde_json::from_value(v).map_err(serde::de::Error::custom)?;
            return Ok(JsonRpcMessage::Response(r));
        }

        Err(serde::de::Error::custom(
            "unrecognized JSON-RPC message shape",
        ))
    }
}

/// Agent → client JSON-RPC request that needs a response.
#[derive(Debug, Clone)]
pub struct IncomingAgentRequest {
    pub id: Value,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FsCapabilities {
    #[serde(rename = "readTextFile", default)]
    pub read_text_file: bool,
    #[serde(rename = "writeTextFile", default)]
    pub write_text_file: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClientCapabilities {
    #[serde(default)]
    pub fs: FsCapabilities,
    #[serde(default)]
    pub terminal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeParams {
    /// Required by Grok Build ACP (missing → JSON-RPC -32602 Invalid params).
    #[serde(rename = "protocolVersion")]
    pub protocol_version: u32,
    #[serde(rename = "clientInfo")]
    pub client_info: ClientInfo,
    #[serde(rename = "clientCapabilities")]
    pub client_capabilities: ClientCapabilities,
}

impl InitializeParams {
    pub fn new(client_info: ClientInfo, client_capabilities: ClientCapabilities) -> Self {
        Self {
            protocol_version: 1,
            client_info,
            client_capabilities,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion", default)]
    pub protocol_version: Option<u32>,
    #[serde(rename = "agentCapabilities", default)]
    pub agent_capabilities: Option<Value>,
    #[serde(rename = "authMethods", default)]
    pub auth_methods: Option<Value>,
    #[serde(rename = "serverInfo", default)]
    pub server_info: Option<Value>,
    #[serde(flatten)]
    pub extra: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticateParams {
    #[serde(rename = "methodId")]
    pub method_id: String,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionNewParams {
    pub cwd: String,
    #[serde(rename = "mcpServers", default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

/// One ACP prompt content block. The wire shape is tagged by `type`
/// ("text" | "image"), matching the Agent Client Protocol ContentBlock.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptBlock {
    Text {
        text: String,
    },
    Image {
        /// e.g. "image/png", "image/jpeg"
        #[serde(rename = "mimeType")]
        mime_type: String,
        /// base64-encoded bytes, no `data:` prefix
        data: String,
    },
}

impl PromptBlock {
    pub fn text(s: impl Into<String>) -> Self {
        PromptBlock::Text { text: s.into() }
    }
}

/// An image the user attached to a prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptImage {
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    /// base64-encoded bytes, no `data:` prefix.
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionPromptParams {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub prompt: Vec<PromptBlock>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prompt_blocks_serialize_to_acp_content_shapes() {
        let params = SessionPromptParams {
            session_id: "s1".into(),
            prompt: vec![
                PromptBlock::text("look at this"),
                PromptBlock::Image {
                    mime_type: "image/png".into(),
                    data: "AAAA".into(),
                },
            ],
        };
        let v = serde_json::to_value(&params).unwrap();
        assert_eq!(v["prompt"][0], json!({"type": "text", "text": "look at this"}));
        assert_eq!(
            v["prompt"][1],
            json!({"type": "image", "mimeType": "image/png", "data": "AAAA"})
        );
    }

    #[test]
    fn parses_session_update_as_notification_not_response() {
        let line = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"}}}}"#;
        let msg: JsonRpcMessage = serde_json::from_str(line).expect("parse");
        match msg {
            JsonRpcMessage::Notification(n) => {
                assert_eq!(n.method, "session/update");
                let text = n
                    .params
                    .as_ref()
                    .and_then(|p| p.pointer("/update/content/text"))
                    .and_then(|v| v.as_str());
                assert_eq!(text, Some("hello"));
            }
            other => panic!("expected Notification, got {other:?}"),
        }
    }

    #[test]
    fn parses_agent_request_as_request_not_response() {
        let line = r#"{"jsonrpc":"2.0","id":7,"method":"terminal/create","params":{"command":"ls","sessionId":"s1"}}"#;
        let msg: JsonRpcMessage = serde_json::from_str(line).expect("parse");
        match msg {
            JsonRpcMessage::Request(r) => {
                assert_eq!(r.method, "terminal/create");
                assert_eq!(r.id, json!(7));
            }
            other => panic!("expected Request, got {other:?}"),
        }
    }

    #[test]
    fn parses_response_with_result() {
        let line = r#"{"jsonrpc":"2.0","id":"abc","result":{"sessionId":"s1"}}"#;
        let msg: JsonRpcMessage = serde_json::from_str(line).expect("parse");
        match msg {
            JsonRpcMessage::Response(r) => {
                assert!(r.result.is_some());
                assert_eq!(r.id.as_ref().and_then(|v| v.as_str()), Some("abc"));
            }
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn parses_error_response() {
        let line = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"nope"}}"#;
        let msg: JsonRpcMessage = serde_json::from_str(line).expect("parse");
        match msg {
            JsonRpcMessage::Response(r) => {
                assert_eq!(r.error.as_ref().map(|e| e.code), Some(-32601));
            }
            other => panic!("expected Response, got {other:?}"),
        }
    }
}
