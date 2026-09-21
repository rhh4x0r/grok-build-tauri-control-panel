//! Things that stay open on a server connection: a terminal, and a forwarded preview port.

use std::sync::Arc;

use bomb_core::terminal::{RemoteOp, TerminalSession};
use bomb_proto::client::StreamItem;
use bomb_proto::{Chunk, Frame, Message};
use serde_json::json;

use super::RemoteCore;

/// A shell in one of the person's folders on the server, shown through the normal terminal panel.
pub async fn open_terminal(remote: Arc<RemoteCore>, folder: &str) -> Result<Arc<TerminalSession>, String> {
    let client = remote.client()?;
    let (stream, mut output) = client.open_stream();
    client.request("terminal_open", json!({ "root": remote.path_of(folder), "stream": stream })).await?;
    // The panel calls `write`/`resize` from the UI thread; a queue carries them to the connection.
    let (ops_tx, mut ops) = tokio::sync::mpsc::unbounded_channel::<RemoteOp>();
    let session = TerminalSession::remote(Box::new(move |op| { let _ = ops_tx.send(op); }));
    let sender = client.clone();
    tokio::spawn(async move {
        while let Some(op) = ops.recv().await {
            let sent = match op {
                RemoteOp::Input(bytes) => sender.send_frame(Frame::Chunk(Chunk { stream, data: bytes.into() })).await,
                RemoteOp::Resize(rows, cols) => sender.request("terminal_resize", json!({ "stream": stream, "rows": rows, "cols": cols })).await.map(|_| ()),
                RemoteOp::Close => { let _ = sender.send_frame(Frame::Message(Message::StreamEnd { stream, error: None })).await; return; }
            };
            if sent.is_err() { return; }
        }
    });
    let screen = Arc::downgrade(&session);
    tokio::spawn(async move {
        while let Some(item) = output.recv().await {
            let Some(session) = screen.upgrade() else { return };
            match item {
                StreamItem::Data(bytes) => session.feed(Some(&bytes)),
                StreamItem::End(_) => { session.feed(None); return; }
            }
        }
        if let Some(session) = screen.upgrade() { session.feed(None); }
    });
    Ok(session)
}

/// Listen on a free local port and carry every connection to `port` on the server's own loopback,
/// so a dev server running there loads in the preview pane. Returns the local port.
/// Forwarding stops when the returned guard is dropped.
pub async fn forward_port(remote: Arc<RemoteCore>, port: u16) -> Result<(u16, ForwardGuard), String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.map_err(|e| e.to_string())?;
    let local = listener.local_addr().map_err(|e| e.to_string())?.port();
    let (stop, mut stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        loop {
            let socket = tokio::select! { _ = &mut stopped => return, accepted = listener.accept() => match accepted { Ok((socket, _)) => socket, Err(_) => return } };
            let remote = remote.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let Ok(client) = remote.client() else { return };
                let (stream, mut incoming) = client.open_stream();
                if client.request("forward_open", json!({ "port": port, "stream": stream })).await.is_err() { return; }
                let (mut from_browser, mut to_browser) = socket.into_split();
                let upstream = client.clone();
                let up = tokio::spawn(async move {
                    let mut buffer = vec![0u8; 64 * 1024];
                    loop {
                        match from_browser.read(&mut buffer).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => if upstream.send_frame(Frame::Chunk(Chunk { stream, data: bytes::Bytes::copy_from_slice(&buffer[..n]) })).await.is_err() { return; },
                        }
                    }
                    let _ = upstream.send_frame(Frame::Message(Message::StreamEnd { stream, error: None })).await;
                });
                while let Some(item) = incoming.recv().await {
                    match item {
                        StreamItem::Data(bytes) => if to_browser.write_all(&bytes).await.is_err() { break; },
                        StreamItem::End(_) => break,
                    }
                }
                let _ = to_browser.shutdown().await;
                up.abort();
            });
        }
    });
    Ok((local, ForwardGuard { _stop: stop }))
}

/// Keeps a forwarded port alive.
pub struct ForwardGuard {
    _stop: tokio::sync::oneshot::Sender<()>,
}
