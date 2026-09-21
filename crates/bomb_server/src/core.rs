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

    while let Some(frame) = rx.recv().await? {
        let Frame::Message(Message::Request { id, method, params }) = frame else { continue };
        let (state, out, client) = (state.clone(), out.clone(), client.clone());
        // Requests run concurrently so a long one never blocks approvals or Stop.
        tokio::spawn(async move {
            let (ok, error) = match bomb_core::rpc::dispatch(&state, &client, &method, params).await {
                Ok(value) => (Some(value), None),
                Err(error) => (None, Some(error)),
            };
            let _ = out.send(Frame::Message(Message::Response { id, ok, error })).await;
        });
    }
    streamer.abort();
    drop(out);
    let _ = writer.await;
    Ok(())
}

fn event_frame(seq: u64, event: &bomb_core::ControlEvent) -> Frame {
    Frame::Message(Message::Event { seq, event: serde_json::to_value(event).unwrap_or(serde_json::Value::Null) })
}
