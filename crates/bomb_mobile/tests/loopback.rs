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
    assert!(machine.list_backends().await.unwrap().iter().any(|b| b.id == "grok"));

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

    // Backgrounding pauses; coming back reconnects.
    machine.set_active(false);
    until(&screen, "paused", |s| s.link == Some(LinkState::Paused)).await;
    machine.set_active(true);
    until(&screen, "reconnected", |s| s.link == Some(LinkState::Connected)).await;

    // Removing the phone from the machine's devices.
    machine.unpair().await.unwrap();
    assert!(gateway.store().read().unwrap().devices.iter().all(|d| d.id != paired.device_id));
}
