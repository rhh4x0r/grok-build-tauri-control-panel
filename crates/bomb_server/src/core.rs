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
    while let Some(frame) = rx.recv().await? {
        let (id, method, mut params) = match frame {
            Frame::Message(Message::Request { id, method, params }) => (id, method, params),
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
