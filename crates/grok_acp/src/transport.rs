//! Newline-delimited JSON transport over process stdio.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{Mutex, oneshot};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::error::{AcpError, Result};
use crate::messages::{
    id_key, IncomingAgentRequest, JsonRpcError, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse,
};

pub struct NdjsonTransport {
    stdin: Mutex<ChildStdin>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<JsonRpcResponse>>>>,
    notification_tx: tokio::sync::mpsc::UnboundedSender<JsonRpcNotification>,
    agent_request_tx: tokio::sync::mpsc::UnboundedSender<IncomingAgentRequest>,
}

impl NdjsonTransport {
    pub fn new(
        stdin: ChildStdin,
        stdout: ChildStdout,
        notification_tx: tokio::sync::mpsc::UnboundedSender<JsonRpcNotification>,
        agent_request_tx: tokio::sync::mpsc::UnboundedSender<IncomingAgentRequest>,
    ) -> Arc<Self> {
        let transport = Arc::new(Self {
            stdin: Mutex::new(stdin),
            pending: Arc::new(Mutex::new(HashMap::new())),
            notification_tx,
            agent_request_tx,
        });

        let reader_self = transport.clone();
        tokio::spawn(async move {
            if let Err(e) = reader_self.read_loop(stdout).await {
                warn!(error = %e, "ACP transport read loop ended");
            }
        });

        transport
    }

    async fn read_loop(self: Arc<Self>, stdout: ChildStdout) -> Result<()> {
        let result = self.read_loop_inner(stdout).await;
        // Fail every pending waiter immediately — otherwise callers block for
        // their full request timeout (up to minutes) after the process dies.
        let mut pending = self.pending.lock().await;
        for (_, tx) in pending.drain() {
            drop(tx); // closes the oneshot → callers get ChannelClosed now
        }
        result
    }

    async fn read_loop_inner(&self, stdout: ChildStdout) -> Result<()> {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;
            if n == 0 {
                return Err(AcpError::ProcessExited);
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            debug!(payload = %trimmed, "acp recv");
            match serde_json::from_str::<JsonRpcMessage>(trimmed) {
                Ok(JsonRpcMessage::Response(resp)) => {
                    let id_key = resp
                        .id
                        .as_ref()
                        .map(id_key)
                        .unwrap_or_default();
                    let mut pending = self.pending.lock().await;
                    if let Some(tx) = pending.remove(&id_key) {
                        let _ = tx.send(resp);
                    } else {
                        debug!(id = %id_key, "no pending request for response");
                    }
                }
                Ok(JsonRpcMessage::Notification(n)) => {
                    let _ = self.notification_tx.send(n);
                }
                Ok(JsonRpcMessage::Request(req)) => {
                    // Agent → client request (fs/*, session/request_permission, …).
                    // MUST be answered or the agent turn hangs forever.
                    let _ = self.agent_request_tx.send(IncomingAgentRequest {
                        id: req.id,
                        method: req.method,
                        params: req.params,
                    });
                }
                Err(e) => {
                    // Try looser parse: notification-shaped with extra fields.
                    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                        if v.get("method").is_some() && v.get("id").is_some() {
                            let _ = self.agent_request_tx.send(IncomingAgentRequest {
                                id: v.get("id").cloned().unwrap_or(Value::Null),
                                method: v
                                    .get("method")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                params: v.get("params").cloned(),
                            });
                            continue;
                        }
                        if v.get("method").is_some() && v.get("id").is_none() {
                            let _ = self.notification_tx.send(JsonRpcNotification {
                                jsonrpc: "2.0".into(),
                                method: v
                                    .get("method")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                                params: v.get("params").cloned(),
                            });
                            continue;
                        }
                    }
                    warn!(error = %e, line = %trimmed, "failed to parse ACP line");
                }
            }
        }
    }

    /// Queue a client-made notification behind everything the agent has sent
    /// so far. The event loop sees it only after those, which marks a point in
    /// the agent's stream (e.g. "history replay finished").
    pub fn push_local_notification(&self, method: &str) {
        let _ = self.notification_tx.send(JsonRpcNotification {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params: None,
        });
    }

    pub async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let rx = self.send_request(method, params).await?;
        let resp = rx.await.map_err(|_| AcpError::ChannelClosed)?;
        Self::unwrap_response(resp)
    }

    /// Send a request and return the oneshot receiver (caller applies timeout policy).
    pub async fn send_request(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Result<oneshot::Receiver<JsonRpcResponse>> {
        let id = Value::String(Uuid::new_v4().to_string());
        let id_str = id_key(&id);
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id,
            method: method.to_string(),
            params,
        };
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id_str.clone(), tx);
        }

        let line = serde_json::to_string(&req)? + "\n";
        debug!(method, %id_str, "acp send");
        let write_res: Result<()> = async {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await?;
            Ok(())
        }
        .await;
        if let Err(e) = write_res {
            // Failed write leaks the pending entry — remove it so a later
            // response for a reused id can't match, and callers fail fast.
            self.pending.lock().await.remove(&id_str);
            return Err(e);
        }
        Ok(rx)
    }

    pub async fn send_response(&self, id: Value, result: Value) -> Result<()> {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: Some(id),
            result: Some(result),
            error: None,
        };
        let line = serde_json::to_string(&resp)? + "\n";
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    pub async fn send_error_response(
        &self,
        id: Value,
        code: i64,
        message: impl Into<String>,
    ) -> Result<()> {
        let resp = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: Some(id),
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        };
        let line = serde_json::to_string(&resp)? + "\n";
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    pub fn unwrap_response(resp: JsonRpcResponse) -> Result<Value> {
        if let Some(err) = resp.error {
            return Err(AcpError::Rpc {
                code: err.code,
                message: err.message,
            });
        }
        Ok(resp.result.unwrap_or(Value::Null))
    }

    /// Wait for a pending response with an explicit timeout.
    pub async fn request_with_timeout(
        &self,
        method: &str,
        params: Option<Value>,
        timeout: std::time::Duration,
    ) -> Result<Value> {
        let rx = self.send_request(method, params).await?;
        let resp = tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| AcpError::Timeout(method.to_string()))?
            .map_err(|_| AcpError::ChannelClosed)?;
        Self::unwrap_response(resp)
    }

    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        let n = JsonRpcNotification {
            jsonrpc: "2.0".into(),
            method: method.to_string(),
            params,
        };
        let line = serde_json::to_string(&n)? + "\n";
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }
}
