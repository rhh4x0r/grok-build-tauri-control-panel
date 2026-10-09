//! The phone's whole path against a real gateway and core on loopback: pair from a code, start a thread,
//! watch it stream, answer an approval, see another device's prompt.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bomb_mobile::machine::{LinkState, Machine, MachineListener, NewThread, PromptOptions};
use bomb_mobile::view::{EntryBody, EntryRole, EntryView, PresenceView, ThreadPatch, ThreadSummary};
use serde_json::json;

/// What Swift would hold: the link state, the list, and one open thread's entries.
#[derive(Default)]
struct Screen {
    link: Option<LinkState>,
    threads: Vec<ThreadSummary>,
    entries: Vec<EntryView>,
    resets: usize,
    settings_changed: usize,
}

struct Listener(Arc<Mutex<Screen>>);

impl MachineListener for Listener {
    fn on_link(&self, state: LinkState) {
        self.0.lock().unwrap().link = Some(state);
    }
    fn on_threads(&self, threads: Vec<ThreadSummary>) {
        self.0.lock().unwrap().threads = threads;
    }
    fn on_thread(&self, _thread_id: String, patches: Vec<ThreadPatch>, _presence: PresenceView) {
        let mut screen = self.0.lock().unwrap();
        for patch in patches {
            match patch {
                ThreadPatch::Reset { entries, .. } => { screen.entries = entries; screen.resets += 1; }
                ThreadPatch::Upsert { index, entry } => {
                    let index = index as usize;
                    if index == screen.entries.len() { screen.entries.push(entry) } else { screen.entries[index] = entry }
                }
                ThreadPatch::Stream { index, delta } => {
                    if let Some(EntryView { body: EntryBody::Text { text }, .. }) = screen.entries.get_mut(index as usize) { text.push_str(&delta); }
                }
                ThreadPatch::Trim { count } => { screen.entries.drain(..count as usize); }
            }
        }
    }
    fn on_settings_changed(&self) {
        self.0.lock().unwrap().settings_changed += 1;
    }
}

async fn until(screen: &Arc<Mutex<Screen>>, what: &str, done: impl Fn(&Screen) -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !done(&screen.lock().unwrap()) {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn texts(screen: &Screen, role: EntryRole) -> Vec<String> {
    screen.entries.iter().filter(|e| e.role == role).filter_map(|e| match &e.body { EntryBody::Text { text } => Some(text.clone()), _ => None }).collect()
}

#[tokio::test]
async fn a_paired_phone_starts_a_thread_watches_it_and_answers_an_approval() {
    let temp = tempfile::tempdir().unwrap();
    // Mock-model threads are hidden from thread lists outside smoke runs.
    std::env::set_var("BOMB_SMOKE", "1");
    let home = temp.path().join("home");
    let grok = home.join("grok");
    let panel = grok.join("panel");
    let state = Arc::new(bomb_core::AppState::initialize_with_paths(grok_config::GrokPaths {
        home_dir: home.clone(), grok_dir: grok.clone(), bomb_dir: panel.clone(), config_file: panel.join("config.toml"),
        grok_cli_config_file: grok.join("config.toml"), worktrees_dir: home.join("worktrees"),
        memory_dir: panel.join("memory"), sessions_dir: panel.join("sessions"), panel_dir: panel,
        project_config_file: None, project_root: None,
    }).await.unwrap());
    let socket = temp.path().join("core.sock");
    tokio::spawn({ let (state, socket) = (state.clone(), socket.clone()); async move { bomb_server::core::serve(state, &socket, std::future::pending()).await.unwrap() } });
    while !socket.exists() { tokio::time::sleep(Duration::from_millis(10)).await; }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let gateway = bomb_server::gateway::Gateway::open(&temp.path().join("gateway"), &address).unwrap();
    gateway.add_user("max", &socket, true).unwrap();
    tokio::spawn(gateway.clone().serve(listener, std::future::pending()));
    let root = bomb_core::rpc::dispatch(&state, "setup", "create_project", json!({ "name": "game" })).await.unwrap().as_str().unwrap().to_string();

    // One scan: a bundle naming this machine and one nobody answers. The good one still pairs.
    let invite = gateway.create_invite("max").unwrap();
    let dead = bomb_link::PairingLink::new("127.0.0.1:1", &"ab".repeat(32));
    let code = bomb_link::bundle(&[invite, dead]);
    assert_eq!(bomb_mobile::pairing::pairing_hosts(code.clone()).unwrap(), vec![address.clone(), "127.0.0.1:1".to_string()]);
    let outcomes = bomb_mobile::pairing::pair_all(code.clone(), "Test iPhone".into()).await.unwrap();
    assert!(outcomes[1].error.is_some() && outcomes[1].machine.is_none());
    let paired = outcomes[0].machine.clone().unwrap_or_else(|| panic!("{:?}", outcomes[0].error));
    assert_eq!((paired.user.as_str(), paired.admin, paired.kind.as_str()), ("max", true, "server"));
    // The secret was single-use.
    assert!(bomb_mobile::pairing::pair_all(code, "Again".into()).await.unwrap()[0].error.is_some());

    let screen = Arc::new(Mutex::new(Screen::default()));
    let machine = Machine::new(paired.clone(), Box::new(Listener(screen.clone())));
    until(&screen, "connected", |s| s.link == Some(LinkState::Connected)).await;
    assert_eq!(machine.list_projects().await.unwrap(), vec![root.clone()]);
    // A project made from the phone: an empty one; a clone takes only a remote Git address.
    let made = machine.create_project("Pocket Game".into()).await.unwrap();
    assert!(made.ends_with("pocket-game") && std::path::Path::new(&made).join(".git").exists());
    assert!(machine.create_project("Pocket Game".into()).await.is_err(), "names don't collide");
    assert!(machine.list_projects().await.unwrap().contains(&made));
    assert!(machine.clone_project(made.clone(), Some("pocket-copy".into())).await.is_err(), "no local paths");
    assert!(machine.list_backends().await.unwrap().iter().any(|b| b.id == "grok"));

    // A web server on the machine opens in the phone's browser: through a phone-side port, over the link.
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let site = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = site.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = site.accept().await.unwrap();
                let mut request = [0u8; 256];
                let n = socket.read(&mut request).await.unwrap();
                let line = String::from_utf8_lossy(&request[..n]).lines().next().unwrap_or_default().to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{line}", line.len()).as_bytes()).await.unwrap();
            }
        });
        // A backend started anywhere (here, by this test) is listed, so it opens beside the page.
        // (Another process: this one is the core, whose own ports are left out.)
        let api_port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut api = std::process::Command::new("python3")
            .args(["-c", &format!("import socket,time; s=socket.socket(); s.bind(('127.0.0.1',{api_port})); s.listen(); time.sleep(30)")])
            .spawn().unwrap();
        let mut listed = false;
        for _ in 0..40 {
            if machine.local_servers().await.unwrap().iter().any(|s| s.port == api_port) { listed = true; break; }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let _ = api.kill();
        let _ = api.wait();
        assert!(listed, "the backend's port is listed");
        let local = machine.open_preview(port).await.unwrap();
        // The machine's port is taken here (same computer), so the phone side picked another.
        assert_ne!(local, port);
        assert_eq!(machine.open_preview(port).await.unwrap(), local, "opening again reuses it");
        for _ in 0..2 {
            let mut browser = tokio::net::TcpStream::connect(("127.0.0.1", local)).await.unwrap();
            browser.write_all(b"GET /hello HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
            let mut page = String::new();
            let _ = tokio::time::timeout(Duration::from_secs(5), browser.read_to_string(&mut page)).await;
            assert!(page.ends_with("GET /hello HTTP/1.1"), "{page}");
        }
        machine.close_previews();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(tokio::net::TcpStream::connect(("127.0.0.1", local)).await.is_err(), "closed previews stop listening");
        assert!(machine.thread_servers("not-a-thread".into()).await.unwrap().is_empty());
    }

    // Read-aloud settings are shared: what the phone sets, the machine keeps, and every device hears of it.
    assert_eq!(machine.speech_settings().await.unwrap(), Some(Default::default()));
    machine.set_speech_settings(Some("fish-key".into()), Some("v123".into()), Some("Sarah".into()), Some(1.25)).await.unwrap();
    until(&screen, "the settings notice", |s| s.settings_changed > 0).await;
    let shared = machine.speech_settings().await.unwrap().unwrap();
    assert_eq!((shared.api_key.as_deref(), shared.voice.as_deref(), shared.voice_name.as_deref(), shared.rate), (Some("fish-key"), Some("v123"), Some("Sarah"), Some(1.25)));
    machine.set_speech_settings(Some(String::new()), None, None, None).await.unwrap();
    assert_eq!(machine.speech_settings().await.unwrap().unwrap().api_key, None, "an empty key removes it");

    // The phone never turns on always-approve.
    let yolo = NewThread { project_root: root.clone(), backend: "grok".into(), model: Some("mock".into()), effort: None, approval_mode: Some("yolo".into()), prompt: "x".into(), own_worktree: true, images: vec![] };
    assert!(machine.start_thread(yolo).await.is_err());

    let id = machine.start_thread(NewThread {
        project_root: root.clone(), backend: "grok".into(), model: Some("mock".into()), effort: None, approval_mode: None,
        prompt: "build tetris".into(), own_worktree: true, images: vec![],
    }).await.unwrap();
    until(&screen, "the mock reply", |s| !texts(s, EntryRole::Agent).is_empty() && s.entries.iter().all(|e| !e.streaming)).await;
    assert_eq!(texts(&screen.lock().unwrap(), EntryRole::You), vec!["build tetris".to_string()], "the phone's own prompt shows once");
    until(&screen, "the thread in the list", |s| s.threads.iter().any(|t| t.id == id && t.project_root == root)).await;

    // A question from the agent arrives as a card; answering it from the phone reaches the machine.
    let session_id = uuid::Uuid::parse_str(&id).unwrap();
    let options = vec![grok_events::PermissionOptionInfo { id: "allow".into(), kind: "allow_once".into(), label: "Allow".into() }];
    state.event_bus.emit(grok_events::ControlEvent::ApprovalRequired {
        session_id, request_id: "r1".into(), tool: "Bash".into(), summary: "cargo test".into(), options, auto_approved: false,
        selected_option: None, plan_approval: false, at: chrono::Utc::now(),
    });
    until(&screen, "the approval card", |s| s.entries.iter().any(|e| matches!(&e.body, EntryBody::Approval { request_id, resolution: None, .. } if request_id == "r1"))).await;
    until(&screen, "the list to show it waiting", |s| s.threads.iter().any(|t| t.id == id && t.needs_approval)).await;
    // The mock agent has no live request, so the core reports it as already answered; the call itself works.
    assert!(machine.respond_approval(id.clone(), "r1".into(), "allow".into()).await.unwrap());
    state.event_bus.emit(grok_events::ControlEvent::ApprovalResolved { session_id, request_id: "r1".into(), option_id: Some("allow".into()), cancelled: false, at: chrono::Utc::now() });
    until(&screen, "the card to close", |s| s.entries.iter().any(|e| matches!(&e.body, EntryBody::Approval { resolution: Some(r), .. } if r == "allow"))).await;

    // A prompt from the Mac shows up on the phone.
    state.event_bus.emit(grok_events::ControlEvent::UserMessage { session_id, text: "from the mac".into(), origin: "some-mac".into(), at: chrono::Utc::now() });
    until(&screen, "the Mac's prompt", |s| texts(s, EntryRole::You).contains(&"from the mac".to_string())).await;

    // Reopening loads the saved history in one reset.
    let resets = screen.lock().unwrap().resets;
    machine.close_thread(id.clone()).await.unwrap();
    machine.open_thread(id.clone()).await.unwrap();
    until(&screen, "the reload", |s| s.resets > resets).await;
    assert!(texts(&screen.lock().unwrap(), EntryRole::You).contains(&"build tetris".to_string()));

    // A follow-up from the phone, with an option it may choose.
    machine.send_prompt(id.clone(), "now add scoring".into(), PromptOptions { approval_mode: Some("ask".into()), ..Default::default() }).await.unwrap();
    until(&screen, "the follow-up", |s| texts(s, EntryRole::You).contains(&"now add scoring".to_string())).await;
    assert!(machine.send_prompt(id.clone(), "x".into(), PromptOptions { approval_mode: Some("yolo".into()), ..Default::default() }).await.is_err());

    // A picture in the thread's project arrives whole, in several pieces; files outside the
    // thread, or that aren't pictures or videos, don't.
    let picture = std::path::Path::new(&root).join("out").join("still.png");
    std::fs::create_dir_all(picture.parent().unwrap()).unwrap();
    let bytes: Vec<u8> = (0..5 * 1024 * 1024 + 123).map(|i| (i % 251) as u8).collect();
    std::fs::write(&picture, &bytes).unwrap();
    let local = machine.fetch_media(id.clone(), picture.display().to_string()).await.unwrap();
    assert_eq!(std::fs::read(&local).unwrap(), bytes);
    assert_eq!(machine.fetch_media(id.clone(), picture.display().to_string()).await.unwrap(), local, "a fetched file is reused");
    let elsewhere = tempfile::tempdir().unwrap();
    let outside = elsewhere.path().join("other.png");
    std::fs::write(&outside, b"not yours").unwrap();
    assert!(machine.fetch_media(id.clone(), outside.display().to_string()).await.is_err());
    let notes = std::path::Path::new(&root).join("notes.txt");
    std::fs::write(&notes, b"secret").unwrap();
    assert!(machine.fetch_media(id.clone(), notes.display().to_string()).await.is_err());

    // A subagent: its own saved thread under the parent, a step in the parent, never a thread of its own in the list.
    let child = grok_acp::subagent_thread_id(session_id, "child-acp");
    let raw = |payload: serde_json::Value| grok_events::ControlEvent::Raw { session_id: Some(session_id), payload };
    state.event_bus.emit(raw(json!({ "channel": "subagent", "kind": "spawned", "child": child, "name": "Explore", "task": "Find the shader", "prompt": "Find where the fibre shader is defined." })));
    state.event_bus.emit_tool_call(session_id, grok_events::ToolCallEvent { id: child.to_string(), tool: "Subagent · Explore".into(), args_summary: "Find the shader".into(), status: grok_events::ToolCallStatus::Running, result_summary: None, at: chrono::Utc::now() });
    state.event_bus.emit(grok_events::ControlEvent::AgentMessage { session_id: child, text: "It's in index.html.".into(), at: chrono::Utc::now() });
    state.event_bus.emit(raw(json!({ "channel": "subagent", "kind": "state", "child": child, "state": "completed" })));
    until(&screen, "the subagent step", |s| s.entries.iter().any(|e| matches!(&e.body, EntryBody::Tool { name, .. } if name == "Subagent · Explore"))).await;
    let mut listed = Vec::new();
    for _ in 0..100 {
        listed = bomb_core::rpc::dispatch(&state, "setup", "list_subagents", json!({ "thread": id })).await.unwrap().as_array().cloned().unwrap_or_default();
        if listed.first().is_some_and(|s| s["state"] == "completed") { break; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!((listed[0]["id"].as_str(), listed[0]["name"].as_str(), listed[0]["state"].as_str()), (Some(child.to_string().as_str()), Some("Explore"), Some("completed")));
    let threads = bomb_core::rpc::dispatch(&state, "setup", "list_threads", json!({})).await.unwrap();
    assert!(threads.as_array().unwrap().iter().all(|t| t["id"] != child.to_string()), "a subagent isn't a thread in the list");
    let rows = state.persistence.transcript_entries(child).unwrap();
    assert!(rows.iter().any(|r| r.body.contains("fibre shader")) && rows.iter().any(|r| r.body.contains("index.html")), "{rows:?}");
    assert!(machine.send_prompt(child.to_string(), "hi".into(), PromptOptions::default()).await.is_err(), "a subagent takes no messages");

    // A helper: the thread's agent starts one on another agent and model through the `bomb` tools.
    tokio::spawn(bomb_core::helpers::serve(state.clone()));
    for _ in 0..100 { if bomb_core::helpers::mcp_server(&state, session_id).is_some() { break; } tokio::time::sleep(Duration::from_millis(20)).await; }
    let started = bomb_core::helpers::call(&state, session_id, "start_helper", &json!({ "agent": "grok", "model": "mock", "task": "Say what's in this repo.", "name": "Repo tour" })).await.unwrap();
    let helper = started["id"].as_str().unwrap().to_string();
    assert_eq!(started["status"], "running");
    let waited = bomb_core::helpers::call(&state, session_id, "wait_helpers", &json!({ "ids": [helper], "timeout_secs": 30 })).await.unwrap();
    let report = &waited["helpers"][0];
    assert_eq!(report["status"], "finished", "{waited}");
    assert!(report["report"].as_str().is_some_and(|r| r.contains("mock")), "its report comes back: {waited}");
    let listed = bomb_core::rpc::dispatch(&state, "setup", "list_subagents", json!({ "thread": id })).await.unwrap();
    assert!(listed.as_array().unwrap().iter().any(|s| s["id"] == helper.as_str() && s["name"] == "Repo tour" && s["state"] == "completed"), "{listed}");
    let threads = bomb_core::rpc::dispatch(&state, "setup", "list_threads", json!({})).await.unwrap();
    assert!(threads.as_array().unwrap().iter().all(|t| t["id"] != helper.as_str()), "a helper isn't a thread in the list");
    until(&screen, "the helper step", |s| s.entries.iter().any(|e| matches!(&e.body, EntryBody::Tool { name, status, .. } if name == "Subagent · Repo tour" && status == "completed"))).await;
    // A follow-up, and helpers that aren't this thread's are refused.
    bomb_core::helpers::call(&state, session_id, "message_helper", &json!({ "id": helper, "text": "One more thing." })).await.unwrap();
    let again = bomb_core::helpers::call(&state, session_id, "wait_helpers", &json!({ "ids": [helper], "timeout_secs": 30 })).await.unwrap();
    assert_eq!(again["helpers"][0]["status"], "finished", "{again}");
    assert!(bomb_core::helpers::call(&state, uuid::Uuid::new_v4(), "wait_helpers", &json!({ "ids": [helper] })).await.is_err());

    // Backgrounding pauses; coming back reconnects.
    machine.set_active(false);
    until(&screen, "paused", |s| s.link == Some(LinkState::Paused)).await;
    machine.set_active(true);
    until(&screen, "reconnected", |s| s.link == Some(LinkState::Connected)).await;

    // Removing the phone from the machine's devices.
    machine.unpair().await.unwrap();
    assert!(gateway.store().read().unwrap().devices.iter().all(|d| d.id != paired.device_id));
}
