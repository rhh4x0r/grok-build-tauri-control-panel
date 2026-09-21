//! Client half of the protocol: request/response correlation plus the event stream.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};

use crate::{Chunk, Connection, Frame, Message, ProtoError, CHUNK_SIZE, PROTOCOL_VERSION};

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
    streams: Streams,
}

/// A piece of a download, or how it ended.
#[derive(Debug)]
pub enum StreamItem {
    Data(bytes::Bytes),
    End(Option<String>),
}

type Streams = Arc<Mutex<HashMap<u32, mpsc::UnboundedSender<StreamItem>>>>;

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
    let streams: Streams = Default::default();
    let reader_streams = streams.clone();
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
                // Downloads this client asked for go to whoever is waiting on that stream.
                Frame::Message(Message::StreamEnd { stream, error }) => match reader_streams.lock().unwrap_or_else(|e| e.into_inner()).remove(&stream) {
                    Some(waiter) => { let _ = waiter.send(StreamItem::End(error)); true }
                    None => incoming_tx.send(Incoming::StreamEnd { stream, error }).is_ok(),
                },
                Frame::Chunk(chunk) => {
                    let waiter = reader_streams.lock().unwrap_or_else(|e| e.into_inner()).get(&chunk.stream).cloned();
                    match waiter {
                        Some(waiter) => { let _ = waiter.send(StreamItem::Data(chunk.data)); true }
                        None => incoming_tx.send(Incoming::Chunk(chunk)).is_ok(),
                    }
                }
                Frame::Message(_) => true,
            };
            if !delivered { break; }
        }
        // Connection gone: fail everything still waiting instead of hanging it.
        let waiting = reader_pending.lock().unwrap_or_else(|e| e.into_inner()).take();
        for (_, waiter) in waiting.into_iter().flatten() {
            let _ = waiter.send(Err("The connection to the server was lost.".into()));
        }
        for (_, waiter) in reader_streams.lock().unwrap_or_else(|e| e.into_inner()).drain() {
            let _ = waiter.send(StreamItem::End(Some("The connection to the server was lost.".into())));
        }
    });
    Ok(Connected { client: Client { out, pending, next: Arc::new(AtomicU64::new(1)), streams }, seq, resync, incoming })
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

    /// Reserve a stream id and start listening on it, before asking the server to send.
    pub fn open_stream(&self) -> (u32, mpsc::UnboundedReceiver<StreamItem>) {
        let id = (self.next.fetch_add(1, Ordering::Relaxed) & 0x7fff_ffff) as u32;
        let (tx, rx) = mpsc::unbounded_channel();
        self.streams.lock().unwrap_or_else(|e| e.into_inner()).insert(id, tx);
        (id, rx)
    }

    /// Send a file as a numbered stream. Returns the stream id to name in the request that uses it.
    pub async fn upload(&self, path: &std::path::Path) -> Result<u32, String> {
        use tokio::io::AsyncReadExt;
        let id = (self.next.fetch_add(1, Ordering::Relaxed) & 0x7fff_ffff) as u32;
        let mut file = tokio::fs::File::open(path).await.map_err(|e| e.to_string())?;
        let mut buffer = vec![0u8; CHUNK_SIZE];
        loop {
            let n = file.read(&mut buffer).await.map_err(|e| e.to_string())?;
            if n == 0 { break; }
            self.send_frame(Frame::Chunk(Chunk { stream: id, data: bytes::Bytes::copy_from_slice(&buffer[..n]) })).await?;
        }
        self.send_frame(Frame::Message(Message::StreamEnd { stream: id, error: None })).await?;
        Ok(id)
    }

    /// Ask for a file and write what arrives to `path`. `request` gets the stream id to pass along.
    pub async fn download(&self, method: &str, mut params: Value, path: &std::path::Path) -> Result<Value, String> {
        use tokio::io::AsyncWriteExt;
        let (id, mut items) = self.open_stream();
        params["stream"] = Value::from(id);
        let answer = match self.request(method, params).await {
            Ok(answer) => answer,
            Err(e) => { self.streams.lock().unwrap_or_else(|e| e.into_inner()).remove(&id); return Err(e); }
        };
        // Nothing to send: the server says so and never opens the stream.
        if answer.get("empty").and_then(Value::as_bool) == Some(true) {
            self.streams.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            return Ok(answer);
        }
        let mut file = tokio::fs::File::create(path).await.map_err(|e| e.to_string())?;
        while let Some(item) = items.recv().await {
            match item {
                StreamItem::Data(data) => file.write_all(&data).await.map_err(|e| e.to_string())?,
                StreamItem::End(None) => { file.flush().await.map_err(|e| e.to_string())?; return Ok(answer); }
                StreamItem::End(Some(error)) => return Err(error),
            }
        }
        Err("The connection to the server was lost.".into())
    }

    pub fn is_closed(&self) -> bool {
        self.out.is_closed() || self.pending.lock().unwrap_or_else(|e| e.into_inner()).is_none()
    }
}
