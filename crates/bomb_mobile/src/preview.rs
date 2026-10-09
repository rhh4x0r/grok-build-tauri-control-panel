//! Opening a machine's local web servers on the phone. A small listener on the phone's own
//! loopback, on the same port where possible, carries each connection over the machine's link to
//! that port on the machine's loopback (`forward_open`), like `ssh -L`. The page loads and runs in
//! the phone's browser; the machine only relays bytes. Nothing is exposed on any network.

use std::collections::HashMap;
use std::sync::Mutex;

use bomb_proto::client::{Client, StreamItem};
use bomb_proto::{Chunk, Frame, Message};
use serde_json::json;
use tokio::net::TcpListener;
use std::sync::Arc;

/// The ports being shown, by the machine's port: the phone's port, and its listeners (which stop
/// when dropped).
#[derive(Default)]
pub(crate) struct Previews(Mutex<HashMap<u16, (u16, Listening)>>);

/// Listening tasks, stopped when dropped.
pub(crate) struct Listening(Vec<tokio::task::AbortHandle>);

impl Listening {
    fn alive(&self) -> bool {
        self.0.iter().any(|task| !task.is_finished())
    }
}

impl Drop for Listening {
    fn drop(&mut self) {
        for task in &self.0 { task.abort(); }
    }
}

impl Previews {
    /// The phone's port for `port` on the machine, starting to listen if it isn't yet. `client`
    /// gives the machine's current connection for each new browser connection.
    pub(crate) async fn open(&self, port: u16, client: impl Fn() -> Option<Client> + Send + Sync + 'static) -> Result<u16, String> {
        // Reuse a tunnel only while it still listens: iOS closes a suspended app's listening sockets,
        // so one opened before the app went to the background is dead when it comes back.
        let open = self.0.lock().unwrap_or_else(|e| e.into_inner()).get(&port).map(|(local, listening)| (*local, listening.alive()));
        if let Some((local, true)) = open {
            if tokio::time::timeout(std::time::Duration::from_millis(300), tokio::net::TcpStream::connect(("127.0.0.1", local))).await.is_ok_and(|c| c.is_ok()) {
                return Ok(local);
            }
        }
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(&port);
        // The same port, so pages that name it (and their live reload) keep working; else any free one.
        let v4 = match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => listener,
            Err(_) => TcpListener::bind("127.0.0.1:0").await.map_err(|e| format!("Couldn't open a local port: {e}"))?,
        };
        let local = v4.local_addr().map_err(|e| e.to_string())?.port();
        let client: Arc<dyn Fn() -> Option<Client> + Send + Sync> = Arc::new(client);
        let mut tasks = vec![serve(v4, port, client.clone())];
        // "localhost" may resolve to IPv6 first.
        if let Ok(v6) = TcpListener::bind(("::1", local)).await { tasks.push(serve(v6, port, client)); }
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(port, (local, Listening(tasks)));
        Ok(local)
    }

    /// Stop showing every port.
    pub(crate) fn close_all(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

fn serve(listener: TcpListener, port: u16, client: Arc<dyn Fn() -> Option<Client> + Send + Sync>) -> tokio::task::AbortHandle {
    tokio::spawn(async move {
        // A listener that errors (its socket closed while the app was suspended) stops, rather
        // than spinning; the next `open` starts a fresh one.
        while let Ok((socket, _)) = listener.accept().await {
            let Some(link) = client() else { continue };
            tokio::spawn(carry(link, port, socket));
        }
    })
    .abort_handle()
}

/// One browser connection, both ways, until either side closes.
async fn carry(link: Client, port: u16, socket: tokio::net::TcpStream) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (stream, mut incoming) = link.open_stream();
    if link.request("forward_open", json!({ "port": port, "stream": stream })).await.is_err() { return; }
    let (mut from_browser, mut to_browser) = socket.into_split();
    let upstream = link.clone();
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
}
