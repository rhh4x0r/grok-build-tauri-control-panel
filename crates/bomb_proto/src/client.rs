//! Client half of the protocol: request/response correlation plus the event stream.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};

use crate::{Chunk, Connection, Frame, Message, ProtoError, PROTOCOL_VERSION};

/// Things the core sends without being asked.
#[derive(Debug)]
pub enum Incoming {
    Event { seq: u64, event: Value },
    /// The core stopped streaming because this client fell behind; reconnect and rebuild.
    Resync { seq: u64 },
    Chunk(Chunk),
    StreamEnd { stream: u32, error: Option<String> },
}

/// `None` once the connection has ended, so a late request fails instead of waiting forever.
type Pending = Arc<Mutex<Option<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>>;

/// Cheap to clone; all clones share one connection.
#[derive(Clone)]
pub struct Client {
    out: mpsc::Sender<Frame>,
    pending: Pending,
    next: Arc<AtomicU64>,
}

pub struct Connected {
    pub client: Client,
    /// Core's sequence when we attached.
    pub seq: u64,
    /// Our cursor could not be continued: rebuild every open thread from a snapshot.
    pub resync: bool,
    /// Closes when the connection ends.
    pub incoming: mpsc::UnboundedReceiver<Incoming>,
}

pub async fn connect<S>(stream: S, name: &str, since: Option<u64>) -> Result<Connected, ProtoError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut connection = Connection::new(stream);
    connection.send(&Frame::Message(Message::Hello { version: PROTOCOL_VERSION, client: name.to_string(), since })).await?;
    let (seq, resync) = match connection.recv().await? {
        Some(Frame::Message(Message::Welcome { version, seq, resync })) if version == PROTOCOL_VERSION => (seq, resync),
        Some(Frame::Message(Message::Welcome { version, .. })) => {
            return Err(ProtoError::Malformed(format!("server speaks protocol {version}, this app speaks {PROTOCOL_VERSION}")))
        }
        Some(other) => return Err(ProtoError::Malformed(format!("expected a welcome, got {other:?}"))),
        None => return Err(ProtoError::Closed),
    };
    let (mut tx, mut rx) = connection.split();
    let (out, mut outbox) = mpsc::channel::<Frame>(1024);
    tokio::spawn(async move {
        while let Some(frame) = outbox.recv().await {
            if tx.send(&frame).await.is_err() { break; }
        }
    });
    let pending: Pending = Arc::new(Mutex::new(Some(HashMap::new())));
    let (incoming_tx, incoming) = mpsc::unbounded_channel();
    let reader_pending = pending.clone();
    tokio::spawn(async move {
        while let Ok(Some(frame)) = rx.recv().await {
            let delivered = match frame {
                Frame::Message(Message::Response { id, ok, error }) => {
                    if let Some(waiter) = reader_pending.lock().unwrap_or_else(|e| e.into_inner()).as_mut().and_then(|p| p.remove(&id)) {
                        let _ = waiter.send(match error { Some(e) => Err(e), None => Ok(ok.unwrap_or(Value::Null)) });
                    }
                    true
                }
                Frame::Message(Message::Event { seq, event }) => incoming_tx.send(Incoming::Event { seq, event }).is_ok(),
                Frame::Message(Message::Resync { seq }) => incoming_tx.send(Incoming::Resync { seq }).is_ok(),
                Frame::Message(Message::StreamEnd { stream, error }) => incoming_tx.send(Incoming::StreamEnd { stream, error }).is_ok(),
                Frame::Chunk(chunk) => incoming_tx.send(Incoming::Chunk(chunk)).is_ok(),
                Frame::Message(_) => true,
            };
            if !delivered { break; }
        }
        // Connection gone: fail everything still waiting instead of hanging it.
        let waiting = reader_pending.lock().unwrap_or_else(|e| e.into_inner()).take();
        for (_, waiter) in waiting.into_iter().flatten() {
            let _ = waiter.send(Err("The connection to the server was lost.".into()));
        }
    });
    Ok(Connected { client: Client { out, pending, next: Arc::new(AtomicU64::new(1)) }, seq, resync, incoming })
}

impl Client {
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        match self.pending.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            Some(pending) => { pending.insert(id, tx); }
            None => return Err("The connection to the server was lost.".into()),
        }
        if self.out.send(Frame::Message(Message::Request { id, method: method.to_string(), params })).await.is_err() {
            if let Some(pending) = self.pending.lock().unwrap_or_else(|e| e.into_inner()).as_mut() { pending.remove(&id); }
            return Err("The connection to the server was lost.".into());
        }
        rx.await.unwrap_or_else(|_| Err("The connection to the server was lost.".into()))
    }

    pub async fn send_frame(&self, frame: Frame) -> Result<(), String> {
        self.out.send(frame).await.map_err(|_| "The connection to the server was lost.".to_string())
    }

    pub fn is_closed(&self) -> bool {
        self.out.is_closed() || self.pending.lock().unwrap_or_else(|e| e.into_inner()).is_none()
    }
}
