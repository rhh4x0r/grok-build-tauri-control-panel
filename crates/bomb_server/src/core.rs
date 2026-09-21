//! One person's core, served over a Unix socket.

use std::path::Path;
use std::sync::Arc;

use bomb_core::AppState;
use bomb_proto::{Connection, Frame, Message, PROTOCOL_VERSION};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::UnixListener;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Listen on `socket` until `shutdown` resolves. The socket is private to its owner.
pub async fn serve(state: Arc<AppState>, socket: &Path, shutdown: impl std::future::Future<Output = ()>) -> anyhow::Result<()> {
    if socket.exists() {
        std::fs::remove_file(socket)?;
    }
    if let Some(parent) = socket.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(socket)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The gateway reaches this socket through its group; nobody else may.
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o660))?;
    }
    info!(socket = %socket.display(), "core listening");
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let state = state.clone();
                    tokio::spawn(async move {
                        if let Err(error) = handle(state, stream).await { debug!(%error, "client disconnected"); }
                    });
                }
                Err(error) => warn!(%error, "accept failed"),
            },
        }
    }
    let _ = std::fs::remove_file(socket);
    Ok(())
}

/// Serve one client over any byte stream: greet, stream events, answer requests.
pub async fn handle<S>(state: Arc<AppState>, stream: S) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut tx, mut rx) = Connection::new(stream).split();
    let (client, since) = match rx.recv().await? {
        Some(Frame::Message(Message::Hello { version, client, since })) if version == PROTOCOL_VERSION => (client, since),
        Some(Frame::Message(Message::Hello { version, .. })) => {
            anyhow::bail!("client speaks protocol {version}, this core speaks {PROTOCOL_VERSION}")
        }
        _ => anyhow::bail!("expected a hello"),
    };
    let attached = state.journal.attach_remote(since);
    tx.send(&Frame::Message(Message::Welcome { version: PROTOCOL_VERSION, seq: attached.seq, resync: attached.resync })).await?;

    // One writer task owns the sending half; events and responses both go through it.
    let (out, mut outbox) = mpsc::channel::<Frame>(1024);
    let writer = tokio::spawn(async move {
        while let Some(frame) = outbox.recv().await {
            if tx.send(&frame).await.is_err() { break; }
        }
    });
    let events_out = out.clone();
    let journal = state.journal.clone();
    let streamer = tokio::spawn(async move {
        let mut events = attached.events;
        for (seq, event) in attached.backlog {
            if events_out.send(event_frame(seq, &event)).await.is_err() { return; }
        }
        while let Some((seq, event)) = events.recv().await {
            if events_out.send(event_frame(seq, &event)).await.is_err() { return; }
        }
        // The journal dropped us for falling behind: tell the client to start over.
        let _ = events_out.send(Frame::Message(Message::Resync { seq: journal.seq() })).await;
    });

    // Files a client is sending, by stream id. They live in a private folder and are removed after use.
    let transfer = state.paths.panel_dir.join("transfer");
    let mut uploads: std::collections::HashMap<u32, Upload> = Default::default();
    // Terminals and forwarded connections this client has open, by stream id.
    let mut live: std::collections::HashMap<u32, Live> = Default::default();
    while let Some(frame) = rx.recv().await? {
        let (id, method, mut params) = match frame {
            Frame::Message(Message::Request { id, method, params }) => (id, method, params),
            Frame::Chunk(chunk) if live.contains_key(&chunk.stream) => {
                // Keystrokes for a terminal, or bytes for a forwarded connection.
                let gone = match live.get(&chunk.stream) {
                    Some(Live::Terminal(terminal)) => terminal.write(&chunk.data).is_err(),
                    Some(Live::Forward(to_socket)) => to_socket.send(chunk.data).await.is_err(),
                    None => false,
                };
                if gone { live.remove(&chunk.stream); }
                continue;
            }
            Frame::Message(Message::StreamEnd { stream, .. }) if live.contains_key(&stream) => {
                live.remove(&stream);
                continue;
            }
            Frame::Chunk(chunk) => {
                if !uploads.contains_key(&chunk.stream) {
                    if uploads.len() >= MAX_UPLOADS { anyhow::bail!("too many uploads at once"); }
                    std::fs::create_dir_all(&transfer)?;
                    let path = transfer.join(format!("{}.upload", uuid::Uuid::new_v4()));
                    uploads.insert(chunk.stream, Upload { file: Some(tokio::fs::File::create(&path).await?), path, size: 0 });
                }
                let upload = uploads.get_mut(&chunk.stream).expect("just inserted");
                upload.size += chunk.data.len() as u64;
                if upload.size > MAX_UPLOAD_BYTES { anyhow::bail!("upload too large"); }
                if let Some(file) = upload.file.as_mut() { tokio::io::AsyncWriteExt::write_all(file, &chunk.data).await?; }
                continue;
            }
            Frame::Message(Message::StreamEnd { stream, .. }) => {
                if let Some(upload) = uploads.get_mut(&stream) {
                    if let Some(mut file) = upload.file.take() { tokio::io::AsyncWriteExt::flush(&mut file).await?; }
                }
                continue;
            }
            Frame::Message(_) => continue,
        };
        // These change what this connection has open, so they are answered here, in order.
        if matches!(method.as_str(), "terminal_open" | "terminal_resize" | "forward_open") {
            let result = open_live(&state, &method, &params, &mut live, &out).await;
            let (ok, error) = match result { Ok(value) => (Some(value), None), Err(error) => (None, Some(error)) };
            let _ = out.send(Frame::Message(Message::Response { id, ok, error })).await;
            continue;
        }
        // Only the server decides which file a request reads.
        if let Some(object) = params.as_object_mut() { object.remove("bundle_path"); }
        let upload = params.get("stream").and_then(serde_json::Value::as_u64).and_then(|n| uploads.remove(&(n as u32))).filter(|u| u.file.is_none());
        if let Some(upload) = &upload { params["bundle_path"] = serde_json::Value::from(upload.path.display().to_string()); }
        let download = (method == "export_bundle").then(|| params.get("stream").and_then(serde_json::Value::as_u64)).flatten().map(|n| n as u32);
        let (state, out, client) = (state.clone(), out.clone(), client.clone());
        // Requests run concurrently so a long one never blocks approvals or Stop.
        tokio::spawn(async move {
            let result = bomb_core::rpc::dispatch(&state, &client, &method, params).await;
            if let Some(upload) = upload { let _ = tokio::fs::remove_file(&upload.path).await; }
            // A finished export is answered with its size and hash, then sent as a stream and deleted.
            let file = result.as_ref().ok().and_then(|v| v.get("bundle_path")).and_then(serde_json::Value::as_str).map(std::path::PathBuf::from);
            let (ok, error) = match (result, &file, download) {
                (Ok(_), Some(path), Some(_)) => match describe(path).await {
                    Ok((size, sha256)) => (Some(serde_json::json!({ "size": size, "sha256": sha256 })), None),
                    Err(error) => (None, Some(error)),
                },
                (Ok(_), Some(_), None) => (None, Some("name a stream to receive the file on".to_string())),
                (Ok(value), None, _) => (Some(value), None),
                (Err(error), _, _) => (None, Some(error)),
            };
            let sending = ok.is_some();
            let _ = out.send(Frame::Message(Message::Response { id, ok, error })).await;
            if let (Some(path), Some(stream)) = (file, download) {
                if sending {
                    let error = send_file(&out, stream, &path).await.err();
                    let _ = out.send(Frame::Message(Message::StreamEnd { stream, error })).await;
                }
                let _ = tokio::fs::remove_file(&path).await;
            }
        });
    }
    streamer.abort();
    for upload in uploads.values() { let _ = std::fs::remove_file(&upload.path); }
    drop(out);
    let _ = writer.await;
    Ok(())
}

fn event_frame(seq: u64, event: &bomb_core::ControlEvent) -> Frame {
    Frame::Message(Message::Event { seq, event: serde_json::to_value(event).unwrap_or(serde_json::Value::Null) })
}

const MAX_UPLOADS: usize = 4;
/// Large enough for a big repository's first upload, small enough to protect the disk.
const MAX_UPLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;

struct Upload {
    /// `None` once the client has finished sending.
    file: Option<tokio::fs::File>,
    path: std::path::PathBuf,
    size: u64,
}

async fn describe(path: &std::path::Path) -> Result<(u64, String), String> {
    use sha2::Digest;
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await.map_err(|e| e.to_string())?;
    let (mut hasher, mut size, mut buffer) = (sha2::Sha256::new(), 0u64, vec![0u8; bomb_proto::CHUNK_SIZE]);
    loop {
        let n = file.read(&mut buffer).await.map_err(|e| e.to_string())?;
        if n == 0 { break; }
        hasher.update(&buffer[..n]);
        size += n as u64;
    }
    Ok((size, hex::encode(hasher.finalize())))
}

async fn send_file(out: &mpsc::Sender<Frame>, stream: u32, path: &std::path::Path) -> Result<(), String> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path).await.map_err(|e| e.to_string())?;
    let mut buffer = vec![0u8; bomb_proto::CHUNK_SIZE];
    loop {
        let n = file.read(&mut buffer).await.map_err(|e| e.to_string())?;
        if n == 0 { return Ok(()); }
        out.send(Frame::Chunk(bomb_proto::Chunk { stream, data: bytes::Bytes::copy_from_slice(&buffer[..n]) })).await.map_err(|_| "connection closed".to_string())?;
    }
}

const MAX_LIVE: usize = 32;

enum Live {
    Terminal(Arc<bomb_core::terminal::TerminalSession>),
    /// Bytes headed for a TCP connection on this machine's loopback.
    Forward(mpsc::Sender<bytes::Bytes>),
}

async fn open_live(state: &Arc<AppState>, method: &str, params: &serde_json::Value, live: &mut std::collections::HashMap<u32, Live>, out: &mpsc::Sender<Frame>) -> Result<serde_json::Value, String> {
    let stream = params.get("stream").and_then(serde_json::Value::as_u64).ok_or("name a stream")? as u32;
    let number = |name: &str| params.get(name).and_then(serde_json::Value::as_u64);
    match method {
        "terminal_resize" => {
            if let Some(Live::Terminal(terminal)) = live.get(&stream) { terminal.resize(number("rows").unwrap_or(14) as u16, number("cols").unwrap_or(100) as u16); }
            Ok(serde_json::Value::Null)
        }
        _ if live.len() >= MAX_LIVE => Err("Too many terminals and previews are open on this connection.".into()),
        "terminal_open" => {
            let folder = bomb_core::rpc::own_path(state, params).await?;
            let out = out.clone();
            // The PTY reader is a plain thread; hand its bytes to the connection without blocking it for long.
            let tap: bomb_core::terminal::OutputTap = Box::new(move |bytes| {
                let frame = match bytes {
                    Some(bytes) => Frame::Chunk(bomb_proto::Chunk { stream, data: bytes::Bytes::copy_from_slice(bytes) }),
                    None => Frame::Message(Message::StreamEnd { stream, error: None }),
                };
                let _ = out.blocking_send(frame);
            });
            let terminal = tokio::task::spawn_blocking(move || bomb_core::terminal::TerminalSession::spawn_tapped(&folder, tap)).await.map_err(|e| e.to_string())??;
            terminal.resize(number("rows").unwrap_or(14) as u16, number("cols").unwrap_or(100) as u16);
            live.insert(stream, Live::Terminal(terminal));
            Ok(serde_json::Value::Null)
        }
        "forward_open" => {
            let port = number("port").filter(|p| (1..=65535).contains(p)).ok_or("name a port")? as u16;
            // Only this machine's own loopback, where a project's dev server listens.
            let socket = tokio::time::timeout(std::time::Duration::from_secs(3), tokio::net::TcpStream::connect(("127.0.0.1", port))).await
                .map_err(|_| "Nothing answered on that port.".to_string())?
                .map_err(|e| format!("Nothing is listening on that port: {e}"))?;
            let (mut from_socket, mut to_socket) = socket.into_split();
            let (tx, mut rx) = mpsc::channel::<bytes::Bytes>(64);
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                while let Some(data) = rx.recv().await { if to_socket.write_all(&data).await.is_err() { break; } }
                let _ = to_socket.shutdown().await;
            });
            let out = out.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut buffer = vec![0u8; 64 * 1024];
                loop {
                    match from_socket.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => if out.send(Frame::Chunk(bomb_proto::Chunk { stream, data: bytes::Bytes::copy_from_slice(&buffer[..n]) })).await.is_err() { return; },
                    }
                }
                let _ = out.send(Frame::Message(Message::StreamEnd { stream, error: None })).await;
            });
            live.insert(stream, Live::Forward(tx));
            Ok(serde_json::Value::Null)
        }
        _ => Err("unknown method".into()),
    }
}
