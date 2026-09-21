//! Wire protocol between a Bomb Code client and a core.
//!
//! One connection carries length-delimited frames. The first byte of a frame is
//! its kind: a JSON [`Message`], or a binary [`Chunk`] belonging to a numbered
//! stream (Git bundles, forwarded TCP). Methods and their parameters stay JSON
//! so this crate does not depend on the core's types; [`methods`] names them.

pub mod client;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// Bumped on incompatible changes; both sides refuse a mismatch in [`Message::Hello`].
pub const PROTOCOL_VERSION: u32 = 1;
/// Largest frame either side accepts. Bundles and files travel as many chunks.
pub const MAX_FRAME: usize = 8 * 1024 * 1024;
/// Payload size senders use for binary streams.
pub const CHUNK_SIZE: usize = 256 * 1024;

const KIND_JSON: u8 = 0;
const KIND_CHUNK: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Message {
    /// First message from a client. `since` is the last event sequence it applied, if any.
    Hello { version: u32, client: String, since: Option<u64> },
    /// Core's answer. `resync` means the client must rebuild its view from snapshots.
    Welcome { version: u32, seq: u64, resync: bool },
    Request { id: u64, method: String, #[serde(default)] params: Value },
    Response { id: u64, #[serde(default)] ok: Option<Value>, #[serde(default)] error: Option<String> },
    /// One core event with its journal sequence number.
    Event { seq: u64, event: Value },
    /// The client fell too far behind; the core stops streaming until a new `Hello`.
    Resync { seq: u64 },
    /// A binary stream finished (`error` set when it failed).
    StreamEnd { stream: u32, #[serde(default)] error: Option<String> },
}

/// A piece of a numbered binary stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub stream: u32,
    pub data: Bytes,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    Message(Message),
    Chunk(Chunk),
}

#[derive(Debug)]
pub enum ProtoError {
    Io(std::io::Error),
    Malformed(String),
    Closed,
}

impl std::fmt::Display for ProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "connection error: {e}"),
            Self::Malformed(e) => write!(f, "malformed frame: {e}"),
            Self::Closed => write!(f, "connection closed"),
        }
    }
}
impl std::error::Error for ProtoError {}
impl From<std::io::Error> for ProtoError {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}

pub fn encode(frame: &Frame) -> Result<Bytes, ProtoError> {
    let mut out = BytesMut::new();
    match frame {
        Frame::Message(m) => {
            out.put_u8(KIND_JSON);
            out.extend_from_slice(&serde_json::to_vec(m).map_err(|e| ProtoError::Malformed(e.to_string()))?);
        }
        Frame::Chunk(c) => {
            out.put_u8(KIND_CHUNK);
            out.put_u32(c.stream);
            out.extend_from_slice(&c.data);
        }
    }
    if out.len() > MAX_FRAME { return Err(ProtoError::Malformed(format!("frame of {} bytes exceeds the limit", out.len()))); }
    Ok(out.freeze())
}

pub fn decode(mut bytes: BytesMut) -> Result<Frame, ProtoError> {
    if bytes.is_empty() { return Err(ProtoError::Malformed("empty frame".into())); }
    match bytes.get_u8() {
        KIND_JSON => serde_json::from_slice(&bytes).map(Frame::Message).map_err(|e| ProtoError::Malformed(e.to_string())),
        KIND_CHUNK => {
            if bytes.len() < 4 { return Err(ProtoError::Malformed("chunk without a stream id".into())); }
            let stream = bytes.get_u32();
            Ok(Frame::Chunk(Chunk { stream, data: bytes.freeze() }))
        }
        other => Err(ProtoError::Malformed(format!("unknown frame kind {other}"))),
    }
}

/// A framed connection over any byte stream (Unix socket, TLS).
pub struct Connection<S> {
    inner: Framed<S, LengthDelimitedCodec>,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Connection<S> {
    pub fn new(stream: S) -> Self {
        let codec = LengthDelimitedCodec::builder().max_frame_length(MAX_FRAME).new_codec();
        Self { inner: Framed::new(stream, codec) }
    }

    pub async fn send(&mut self, frame: &Frame) -> Result<(), ProtoError> {
        self.inner.send(encode(frame)?).await.map_err(ProtoError::Io)
    }

    /// `Ok(None)` when the peer closed cleanly.
    pub async fn recv(&mut self) -> Result<Option<Frame>, ProtoError> {
        match self.inner.next().await {
            None => Ok(None),
            Some(Ok(bytes)) => decode(bytes).map(Some),
            Some(Err(e)) => Err(ProtoError::Io(e)),
        }
    }

    /// Split into independent halves so events can stream while requests are read.
    pub fn split(self) -> (Sender<S>, Receiver<S>) {
        let (sink, stream) = self.inner.split();
        (Sender { sink }, Receiver { stream })
    }
}

pub struct Sender<S> {
    sink: futures::stream::SplitSink<Framed<S, LengthDelimitedCodec>, Bytes>,
}
impl<S: AsyncRead + AsyncWrite + Unpin> Sender<S> {
    pub async fn send(&mut self, frame: &Frame) -> Result<(), ProtoError> {
        self.sink.send(encode(frame)?).await.map_err(ProtoError::Io)
    }
}

pub struct Receiver<S> {
    stream: futures::stream::SplitStream<Framed<S, LengthDelimitedCodec>>,
}
impl<S: AsyncRead + AsyncWrite + Unpin> Receiver<S> {
    pub async fn recv(&mut self) -> Result<Option<Frame>, ProtoError> {
        match self.stream.next().await {
            None => Ok(None),
            Some(Ok(bytes)) => decode(bytes).map(Some),
            Some(Err(e)) => Err(ProtoError::Io(e)),
        }
    }
}

/// Method names understood by `bomb_core::rpc::dispatch`.
pub mod methods {
    pub const PING: &str = "ping";
    pub const LIST_THREADS: &str = "list_threads";
    pub const SNAPSHOT: &str = "snapshot";
    pub const START_SESSION: &str = "start_session";
    pub const START_MOCK_SESSION: &str = "start_mock_session";
    pub const SEND_PROMPT: &str = "send_prompt";
    pub const CANCEL_SESSION: &str = "cancel_session";
    pub const RESPOND_APPROVAL: &str = "respond_approval";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_reject_garbage() {
        let message = Frame::Message(Message::Request { id: 7, method: "ping".into(), params: serde_json::json!({"a": 1}) });
        assert_eq!(decode(BytesMut::from(&encode(&message).unwrap()[..])).unwrap(), message);
        let chunk = Frame::Chunk(Chunk { stream: 9, data: Bytes::from_static(b"\x00\xffbundle") });
        assert_eq!(decode(BytesMut::from(&encode(&chunk).unwrap()[..])).unwrap(), chunk);
        assert!(decode(BytesMut::new()).is_err());
        assert!(decode(BytesMut::from(&[9u8, 1, 2][..])).is_err());
        assert!(decode(BytesMut::from(&[KIND_CHUNK, 1][..])).is_err());
        assert!(decode(BytesMut::from(&b"\x00not json"[..])).is_err());
        let huge = Frame::Chunk(Chunk { stream: 1, data: Bytes::from(vec![0u8; MAX_FRAME]) });
        assert!(encode(&huge).is_err());
    }

    #[tokio::test]
    async fn connection_carries_frames_both_ways() {
        let (a, b) = tokio::io::duplex(1 << 20);
        let (mut a, mut b) = (Connection::new(a), Connection::new(b));
        a.send(&Frame::Message(Message::Hello { version: PROTOCOL_VERSION, client: "test".into(), since: Some(4) })).await.unwrap();
        assert_eq!(b.recv().await.unwrap(), Some(Frame::Message(Message::Hello { version: PROTOCOL_VERSION, client: "test".into(), since: Some(4) })));
        b.send(&Frame::Chunk(Chunk { stream: 2, data: Bytes::from_static(b"xyz") })).await.unwrap();
        assert_eq!(a.recv().await.unwrap(), Some(Frame::Chunk(Chunk { stream: 2, data: Bytes::from_static(b"xyz") })));
        drop(a);
        assert_eq!(b.recv().await.unwrap(), None);
    }
}
