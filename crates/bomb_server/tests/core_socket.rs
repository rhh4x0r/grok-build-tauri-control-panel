//! Drive a headless core through its Unix socket, the way the gateway will.

use std::sync::Arc;
use std::time::Duration;

use bomb_core::AppState;
use bomb_proto::client::{connect, Connected, Incoming};
use serde_json::{json, Value};
use tokio::net::UnixStream;

async fn core(home: &std::path::Path) -> Arc<AppState> {
    let grok = home.join("grok");
    let panel = grok.join("panel");
    Arc::new(
        AppState::initialize_with_paths(grok_config::GrokPaths {
            home_dir: home.to_path_buf(), grok_dir: grok.clone(), config_file: panel.join("config.toml"),
            grok_cli_config_file: grok.join("config.toml"), worktrees_dir: home.join("worktrees"),
            memory_dir: panel.join("memory"), sessions_dir: panel.join("sessions"), panel_dir: panel,
            project_config_file: None, project_root: None,
        })
        .await
        .unwrap(),
    )
}

async fn attach(socket: &std::path::Path, since: Option<u64>) -> Connected {
    connect(UnixStream::connect(socket).await.unwrap(), "test-mac", since).await.unwrap()
}

/// Collect events until the stream has been quiet for a moment.
async fn drain(connected: &mut Connected) -> Vec<(u64, Value)> {
    let mut events = Vec::new();
    while let Ok(Some(incoming)) = tokio::time::timeout(Duration::from_millis(400), connected.incoming.recv()).await {
        if let Incoming::Event { seq, event } = incoming { events.push((seq, event)); }
    }
    events
}

#[tokio::test]
async fn a_headless_core_saves_history_streams_events_and_resumes_a_cursor() {
    let temp = tempfile::tempdir().unwrap();
    let state = core(temp.path()).await;
    let socket = temp.path().join("core.sock");
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn({
        let (state, socket) = (state.clone(), socket.clone());
        async move { bomb_server::core::serve(state, &socket, async { let _ = stopped.await; }).await.unwrap() }
    });
    while !socket.exists() { tokio::time::sleep(Duration::from_millis(10)).await; }

    let mut first = attach(&socket, None).await;
    assert!(!first.resync);
    assert_eq!(first.client.request("ping", Value::Null).await.unwrap()["seq"], first.seq);
    assert!(first.client.request("kv_get", json!({"key": "settings/credentials/jev/x"})).await.unwrap_err().contains("unknown method"));

    let cwd = temp.path().display().to_string();
    let started = first.client.request("start_mock_session", json!({ "cwd": cwd })).await.unwrap();
    let id = started["id"].as_str().or_else(|| started["session_id"].as_str()).expect("session id").to_string();
    first.client.request("wait_until_idle", json!({ "id": id, "seconds": 10 })).await.unwrap();
    first.client.request("send_prompt", json!({ "id": id, "prompt": "build the thing" })).await.unwrap();
    // The mock agent answers after a short think; keep reading until the turn is over.
    let mut events = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline {
        events.extend(drain(&mut first).await);
        let replied = events.iter().any(|(_, e)| e["type"] == "agent_message");
        let finished = events.iter().rev().find(|(_, e)| e["type"] == "session_status_changed").is_some_and(|(_, e)| e["status"] != "running");
        if replied && finished { break; }
    }
    assert!(events.iter().any(|(_, e)| e["type"] == "agent_message"), "the mock turn should stream a reply: {events:?}");
    // Sequence numbers are gapless and increasing: a client can trust its cursor.
    for pair in events.windows(2) { assert_eq!(pair[1].0, pair[0].0 + 1); }
    let cursor = events.last().unwrap().0;

    // Nothing but the core wrote this history: no UI is running.
    let snapshot = first.client.request("snapshot", json!({ "id": id })).await.unwrap();
    assert!(!snapshot["rows"].as_array().unwrap().is_empty(), "a headless core must save history itself");
    assert_eq!(snapshot["as_of"], cursor);

    // Drop the connection mid-way, let more happen, then come back with the cursor.
    let seen_before: Vec<u64> = events.iter().map(|(s, _)| *s).collect();
    drop(first);
    state.event_bus.emit(bomb_core::ControlEvent::Error { session_id: None, message: "while away".into(), at: Default::default() });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut second = attach(&socket, Some(cursor)).await;
    assert!(!second.resync);
    let missed = drain(&mut second).await;
    assert_eq!(missed.len(), 1, "exactly the event sent while away: {missed:?}");
    assert_eq!(missed[0].0, cursor + 1);
    assert!(!seen_before.contains(&missed[0].0));
    assert_eq!(missed[0].1["message"], "while away");

    // A cursor this core never issued cannot be continued.
    assert!(attach(&socket, Some(cursor + 10_000)).await.resync);

    // Another device's prompt is mirrored to everyone, tagged with who sent it.
    let mut third = attach(&socket, None).await;
    second.client.request("send_prompt", json!({ "id": id, "prompt": "hello from the second mac" })).await.unwrap();
    let mirrored = drain(&mut third).await;
    let user = mirrored.iter().find(|(_, e)| e["type"] == "user_message").expect("prompt mirrored");
    assert_eq!((user.1["text"].as_str(), user.1["origin"].as_str()), (Some("hello from the second mac"), Some("test-mac")));

    let _ = stop.send(());
    server.await.unwrap();
    assert!(!socket.exists(), "the socket is removed on shutdown");
}

#[tokio::test]
async fn a_client_that_falls_behind_is_told_to_resync_rather_than_losing_events_silently() {
    let temp = tempfile::tempdir().unwrap();
    let state = core(temp.path()).await;
    // Attach but never read, then overflow the queue.
    let mut attached = state.journal.attach_remote(None);
    for n in 0..20_000 {
        state.journal.record(bomb_core::ControlEvent::Error { session_id: None, message: n.to_string(), at: Default::default() });
    }
    let mut received = 0u64;
    let mut last = attached.seq;
    while let Some((seq, _)) = attached.events.recv().await {
        assert_eq!(seq, last + 1, "what does arrive is in order with no gaps");
        last = seq;
        received += 1;
    }
    assert!(received < 20_000, "the slow client was cut off");
    // Its cursor is now older than the backlog, so reconnecting demands a rebuild.
    assert!(state.journal.attach_remote(Some(attached.seq)).resync);
}

#[tokio::test]
async fn files_terminals_and_previews_work_over_the_connection_and_stay_inside_the_project() {
    use bomb_proto::client::StreamItem;
    use bomb_proto::{Chunk, Frame, Message};
    let temp = tempfile::tempdir().unwrap();
    let state = core(temp.path()).await;
    let socket = temp.path().join("core.sock");
    tokio::spawn({ let (state, socket) = (state.clone(), socket.clone()); async move { bomb_server::core::serve(state, &socket, std::future::pending()).await.unwrap() } });
    while !socket.exists() { tokio::time::sleep(Duration::from_millis(10)).await; }
    let mac = attach(&socket, None).await.client;

    let root = mac.request("create_project", json!({ "name": "site" })).await.unwrap().as_str().unwrap().to_string();
    std::fs::write(std::path::Path::new(&root).join("index.html"), "<h1>hello</h1>").unwrap();
    std::fs::create_dir(std::path::Path::new(&root).join("src")).unwrap();
    std::fs::write(temp.path().join("secret.txt"), "not yours").unwrap();
    std::os::unix::fs::symlink(temp.path().join("secret.txt"), std::path::Path::new(&root).join("link.txt")).unwrap();

    // Browse and read, without .git and never outside the project, not even through a symlink.
    let listing = mac.request("list_dir", json!({ "root": root })).await.unwrap();
    let names: Vec<&str> = listing.as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"index.html") && names.contains(&"src") && !names.contains(&".git"), "{names:?}");
    assert_eq!(mac.request("read_file", json!({ "root": root, "path": "index.html" })).await.unwrap()["text"], "<h1>hello</h1>");
    for (folder, path) in [(root.as_str(), "../secret.txt"), (root.as_str(), "/etc/passwd"), (root.as_str(), "link.txt"), ("/etc", "passwd")] {
        let error = mac.request("read_file", json!({ "root": folder, "path": path })).await.unwrap_err();
        assert!(error.contains("inside the project") || error.contains("not one of your folders"), "{folder} {path}: {error}");
    }

    // A terminal: type a command, read its output, resize, close.
    let (stream, mut output) = mac.open_stream();
    assert!(mac.request("terminal_open", json!({ "root": "/etc", "stream": stream })).await.is_err());
    mac.request("terminal_open", json!({ "root": root, "stream": stream, "rows": 20, "cols": 80 })).await.unwrap();
    mac.send_frame(Frame::Chunk(Chunk { stream, data: "printf 'BOMB_%s\\n' REMOTE_OK; pwd\r".into() })).await.unwrap();
    let mut screen = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !(screen.contains("BOMB_REMOTE_OK") && screen.contains("/projects/site")) {
        assert!(tokio::time::Instant::now() < deadline, "terminal output never arrived: {screen}");
        if let Ok(Some(StreamItem::Data(bytes))) = tokio::time::timeout(Duration::from_millis(500), output.recv()).await { screen.push_str(&String::from_utf8_lossy(&bytes)); }
    }
    mac.request("terminal_resize", json!({ "stream": stream, "rows": 30, "cols": 120 })).await.unwrap();
    mac.send_frame(Frame::Message(Message::StreamEnd { stream, error: None })).await.unwrap();

    // A preview: bytes reach a server that only listens on the core machine's loopback, and come back.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 64];
        let n = socket.read(&mut request).await.unwrap();
        socket.write_all(format!("HTTP/1.1 200 OK\r\n\r\nsaw {}", String::from_utf8_lossy(&request[..n]).trim()).as_bytes()).await.unwrap();
    });
    let (stream, mut reply) = mac.open_stream();
    mac.request("forward_open", json!({ "port": port, "stream": stream })).await.unwrap();
    mac.send_frame(Frame::Chunk(Chunk { stream, data: "GET / HTTP/1.1\r\n".into() })).await.unwrap();
    let mut body = String::new();
    while let Ok(Some(item)) = tokio::time::timeout(Duration::from_secs(5), reply.recv()).await {
        match item { StreamItem::Data(bytes) => body.push_str(&String::from_utf8_lossy(&bytes)), StreamItem::End(_) => break }
    }
    assert!(body.contains("saw GET / HTTP/1.1"), "{body}");
    let (stream, _) = mac.open_stream();
    assert!(mac.request("forward_open", json!({ "port": 1, "stream": stream })).await.is_err(), "a closed port is reported, not hung");
}
