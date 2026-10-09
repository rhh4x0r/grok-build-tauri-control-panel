//! The `bomb` MCP server a thread's agent runs (`bomb_app --bomb-mcp`, `bombd --bomb-mcp`): MCP
//! over stdio, each tool call forwarded to the helpers socket of the app that started the agent
//! (`BOMB_HELPERS_SOCKET`), tied to that thread by `BOMB_HELPERS_TOKEN`. See `helpers`.
//!
//! Std only, and nothing but protocol on stdout.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

const INSTRUCTIONS: &str = "Bomb Code helpers: run parts of a task on another agent and model, in parallel. \
Use them when the person asks for subagents or helpers on specific agents or models (for example \"one on Codex with Astra, one on Opus\"), \
or when a task splits into independent parts that suit different models. Start each with start_helper (it returns at once), \
then call wait_helpers with their ids to collect their reports; call it again while any is still running. \
Helpers run in this thread's mode; when this thread can edit, each helper works on its own branch, named in its report.";

fn tools() -> Value {
    let ids = json!({ "type": "array", "items": { "type": "string" }, "description": "Helper ids from start_helper. Leave out for all of this thread's helpers." });
    json!([
        {
            "name": "list_agents",
            "description": "The agents (claude, codex, grok) set up on this machine, each with its models' ids and display names. Check here before naming a model.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "start_helper",
            "description": "Start a helper: a separate agent session on the agent and model you choose, working on one task in this project. Returns its id immediately; collect its report with wait_helpers.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent": { "type": "string", "description": "claude, codex or grok. Leave out to use this thread's agent." },
                    "model": { "type": "string", "description": "A model id or display name from list_agents, such as \"Astra\" or \"Opus 5.5\". Leave out for the agent's default." },
                    "task": { "type": "string", "description": "Everything the helper needs to know: it doesn't see this conversation." },
                    "name": { "type": "string", "description": "A short name to show for it, like \"Repo tour\"." },
                },
                "required": ["task"],
            },
        },
        {
            "name": "wait_helpers",
            "description": "Wait for helpers to finish (up to timeout_secs, at most 50) and return each one's status and, once finished, its report. Call again while any is still running.",
            "inputSchema": { "type": "object", "properties": { "ids": ids.clone(), "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 50 } } },
        },
        {
            "name": "helper_status",
            "description": "How helpers are doing, without waiting: status, steps taken, and the latest line.",
            "inputSchema": { "type": "object", "properties": { "ids": ids } },
        },
        {
            "name": "message_helper",
            "description": "Send a finished helper a follow-up, the way you'd reply in a thread. Wait for it again with wait_helpers.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "string" }, "text": { "type": "string" } }, "required": ["id", "text"] },
        },
        {
            "name": "stop_helper",
            "description": "Stop a helper that's still working.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] },
        },
    ])
}

/// Forward one tool call to the app; its answer, or why it failed.
fn forward(tool: &str, arguments: &Value) -> Result<Value, String> {
    let socket = std::env::var("BOMB_HELPERS_SOCKET").map_err(|_| "Helpers aren't connected (no socket).".to_string())?;
    let token = std::env::var("BOMB_HELPERS_TOKEN").unwrap_or_default();
    let mut stream = UnixStream::connect(&socket).map_err(|e| format!("Bomb Code isn't reachable: {e}"))?;
    let request = json!({ "id": 1, "token": token, "method": tool, "params": arguments });
    stream.write_all(format!("{request}\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).map_err(|e| e.to_string())?;
    let reply: Value = serde_json::from_str(&line).map_err(|_| "Bomb Code gave no answer.".to_string())?;
    match reply.get("error").and_then(Value::as_str) {
        Some(error) => Err(error.to_string()),
        None => Ok(reply.get("ok").cloned().unwrap_or(Value::Null)),
    }
}

fn answer(request: &Value) -> Option<Value> {
    let id = request.get("id")?.clone();
    let method = request["method"].as_str().unwrap_or_default();
    let result = match method {
        "initialize" => json!({
            "protocolVersion": request.pointer("/params/protocolVersion").cloned().unwrap_or_else(|| "2025-06-18".into()),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "bomb", "version": env!("CARGO_PKG_VERSION") },
            "instructions": INSTRUCTIONS,
        }),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => {
            let tool = request.pointer("/params/name").and_then(Value::as_str).unwrap_or_default();
            let arguments = request.pointer("/params/arguments").cloned().unwrap_or_else(|| json!({}));
            match forward(tool, &arguments) {
                Ok(value) => json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default() }], "isError": false }),
                Err(error) => json!({ "content": [{ "type": "text", "text": error }], "isError": true }),
            }
        }
        "ping" => json!({}),
        _ => return Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("Unknown method {method}") } })),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// Serve MCP on stdin/stdout until stdin closes. Calls run side by side, so a long wait never
/// holds up a start.
pub fn run_stdio() {
    let out = Arc::new(Mutex::new(std::io::stdout()));
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        let Ok(request) = serde_json::from_str::<Value>(&line) else { continue };
        let out = out.clone();
        std::thread::spawn(move || {
            if let Some(reply) = answer(&request) {
                let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
                let _ = writeln!(out, "{reply}");
                let _ = out.flush();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_introduces_itself_and_lists_its_tools() {
        let init = answer(&json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } })).unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "bomb");
        let list = answer(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["list_agents", "start_helper", "wait_helpers", "helper_status", "message_helper", "stop_helper"]);
        assert!(answer(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).is_none(), "notifications get no reply");
    }
}
