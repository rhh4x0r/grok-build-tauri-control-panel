//! Paired servers.
//!
//! A server is another core, reached through its gateway. Its projects appear
//! in the app under roots of the form `bomb-server://<server>/<path on the
//! server>`, so everything that keys on a project root keeps working and
//! [`Core`](crate::runtime::Core) can tell where a call belongs.

pub mod install;
pub mod keychain;
pub mod live;
pub mod sync;

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use bomb_core::ControlEvent;
use bomb_link::{Identity, PairingLink};
use bomb_proto::client::{Client, Incoming};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{debug, info, warn};

pub const SERVER_SCHEME: &str = "bomb-server://";
/// Stored in the local settings table. Holds nothing secret: the device key lives in its own private file.
pub const SERVERS_KEY: &str = "paired_servers";

/// `bomb-server://<server>/<absolute path>`
pub fn server_root(server: &str, path: &str) -> String {
    format!("{SERVER_SCHEME}{server}{path}")
}

/// Split a namespaced root into (server id, path on the server).
pub fn split_root(root: &str) -> Option<(&str, &str)> {
    let rest = root.strip_prefix(SERVER_SCHEME)?;
    let slash = rest.find('/')?;
    Some((&rest[..slash], &rest[slash..]))
}

pub fn is_server_root(root: &str) -> bool {
    root.starts_with(SERVER_SCHEME)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServerConfig {
    /// Stable local id, also the file name of this server's device key.
    pub id: String,
    /// What the person calls it.
    pub name: String,
    pub host: String,
    pub fingerprint: String,
    pub user: String,
    pub device_id: String,
    #[serde(default)]
    pub admin: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LinkState {
    Connecting,
    Connected,
    /// Why the last attempt failed, in plain words.
    Offline(String),
}

/// Things a connection tells the app besides thread events.
pub enum Notice {
    /// Connection state changed, or the server's thread list may have changed.
    Changed,
    /// Our cursor could not be continued: open server threads must be rebuilt from snapshots.
    Rebuild(String),
}

pub struct RemoteCore {
    pub config: ServerConfig,
    client: RwLock<Option<Client>>,
    state: RwLock<LinkState>,
}

impl RemoteCore {
    pub fn state(&self) -> LinkState {
        self.state.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn root(&self, path: &str) -> String {
        server_root(&self.config.id, path)
    }

    /// The path on the server for one of this server's roots.
    pub fn path_of<'a>(&self, root: &'a str) -> &'a str {
        split_root(root).map(|(_, path)| path).unwrap_or(root)
    }

    /// The live connection, for transfers that stream files.
    pub fn client(&self) -> Result<Client, String> {
        self.client.read().unwrap_or_else(|e| e.into_inner()).clone().ok_or_else(|| format!("{} is not connected right now.", self.config.name))
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let client = self.client.read().unwrap_or_else(|e| e.into_inner()).clone();
        match client {
            Some(client) => client.request(method, params).await,
            None => Err(format!("{} is not connected right now.", self.config.name)),
        }
    }

    pub async fn call<T: serde::de::DeserializeOwned>(&self, method: &str, params: Value) -> Result<T, String> {
        serde_json::from_value(self.request(method, params).await?).map_err(|e| format!("Unexpected answer from {}: {e}", self.config.name))
    }

    /// One question for the gateway itself, on its own short connection.
    pub async fn gateway(&self, method: &str, params: Value) -> Result<Value, String> {
        let identity = keychain::load(&self.config.id).ok_or("This Mac’s key for that server is missing. Pair it again.")?;
        gateway_request(&self.config.host, &self.config.fingerprint, &identity, method, params).await
    }
}

async fn gateway_request(host: &str, fingerprint: &str, identity: &Identity, method: &str, params: Value) -> Result<Value, String> {
    let mut connection = bomb_proto::Connection::new(bomb_link::connect(host, fingerprint, identity).await?);
    connection.send(&bomb_proto::Frame::Message(bomb_proto::Message::Request { id: 1, method: method.into(), params })).await.map_err(|e| e.to_string())?;
    match tokio::time::timeout(Duration::from_secs(20), connection.recv()).await.map_err(|_| "The server did not answer.".to_string())?.map_err(|e| e.to_string())? {
        Some(bomb_proto::Frame::Message(bomb_proto::Message::Response { ok, error, .. })) => match error { Some(e) => Err(e), None => Ok(ok.unwrap_or(Value::Null)) },
        _ => Err("The server closed the connection.".into()),
    }
}

/// Pair this Mac using a link from the server. Generates this Mac's key for that server.
pub async fn pair(link_text: &str, name: &str, label: &str) -> Result<ServerConfig, String> {
    let link = PairingLink::parse(link_text)?;
    let identity = Identity::generate("bomb-code-mac")?;
    let paired = gateway_request(&link.host, &link.fingerprint, &identity, "gateway.pair", json!({ "secret": link.secret, "label": label })).await?;
    let id = format!("s{}", &link.fingerprint[..12]);
    keychain::save(&id, &identity)?;
    let me = gateway_request(&link.host, &link.fingerprint, &identity, "gateway.whoami", Value::Null).await.unwrap_or(Value::Null);
    Ok(ServerConfig {
        id,
        name: if name.trim().is_empty() { link.host.split(':').next().unwrap_or("Server").to_string() } else { name.trim().to_string() },
        host: link.host,
        fingerprint: link.fingerprint,
        user: paired["user"].as_str().unwrap_or_default().to_string(),
        device_id: paired["device_id"].as_str().unwrap_or_default().to_string(),
        admin: me["admin"].as_bool().unwrap_or(false),
    })
}

/// Every paired server, shared between the UI thread and tokio.
#[derive(Default)]
pub struct Remotes {
    servers: RwLock<HashMap<String, Arc<RemoteCore>>>,
}

impl Remotes {
    pub fn get(&self, id: &str) -> Option<Arc<RemoteCore>> {
        self.servers.read().unwrap_or_else(|e| e.into_inner()).get(id).cloned()
    }

    pub fn for_root(&self, root: &str) -> Option<Arc<RemoteCore>> {
        self.get(split_root(root)?.0)
    }

    pub fn all(&self) -> Vec<Arc<RemoteCore>> {
        let mut all: Vec<_> = self.servers.read().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
        all.sort_by(|a, b| a.config.name.to_lowercase().cmp(&b.config.name.to_lowercase()));
        all
    }

    pub fn remove(&self, id: &str) {
        // Dropping the core ends its connection loop on the next turn.
        self.servers.write().unwrap_or_else(|e| e.into_inner()).remove(id);
        keychain::forget(id);
    }

    /// Start (or restart) the connection to one server. Events flow into `events`; `notices` wakes the app.
    pub fn connect(self: &Arc<Self>, config: ServerConfig, events: async_channel::Sender<ControlEvent>, notices: async_channel::Sender<Notice>) {
        let core = Arc::new(RemoteCore { config, client: RwLock::new(None), state: RwLock::new(LinkState::Connecting) });
        self.servers.write().unwrap_or_else(|e| e.into_inner()).insert(core.config.id.clone(), core.clone());
        let registry = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut cursor: Option<u64> = None;
            let mut wait = Duration::from_secs(1);
            loop {
                // Unpaired while we slept?
                let Some(registry) = registry.upgrade() else { return };
                if !registry.get(&core.config.id).is_some_and(|current| Arc::ptr_eq(&current, &core)) { return; }
                drop(registry);
                match session(&core, &mut cursor, &events, &notices).await {
                    Ok(()) => wait = Duration::from_secs(1),
                    Err(reason) => {
                        debug!(server = %core.config.name, %reason, "server connection failed");
                        *core.state.write().unwrap_or_else(|e| e.into_inner()) = LinkState::Offline(reason);
                    }
                }
                *core.client.write().unwrap_or_else(|e| e.into_inner()) = None;
                if matches!(core.state(), LinkState::Connected) {
                    *core.state.write().unwrap_or_else(|e| e.into_inner()) = LinkState::Offline("The connection dropped. Reconnecting…".into());
                }
                let _ = notices.send(Notice::Changed).await;
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(Duration::from_secs(30));
            }
        });
    }
}

/// One connected stretch. Returns when the connection ends.
async fn session(core: &Arc<RemoteCore>, cursor: &mut Option<u64>, events: &async_channel::Sender<ControlEvent>, notices: &async_channel::Sender<Notice>) -> Result<(), String> {
    let identity = keychain::load(&core.config.id).ok_or("This Mac’s key for that server is missing. Pair it again.")?;
    let tls = bomb_link::connect(&core.config.host, &core.config.fingerprint, &identity).await?;
    let mut connected = bomb_proto::client::connect(tls, &core.config.device_id, *cursor).await.map_err(|e| match e {
        bomb_proto::ProtoError::Closed => "The server refused this Mac. It may have been removed from the server’s devices.".to_string(),
        other => other.to_string(),
    })?;
    *core.client.write().unwrap_or_else(|e| e.into_inner()) = Some(connected.client.clone());
    *core.state.write().unwrap_or_else(|e| e.into_inner()) = LinkState::Connected;
    info!(server = %core.config.name, "connected to server");
    if connected.resync && cursor.is_some() {
        let _ = notices.send(Notice::Rebuild(core.config.id.clone())).await;
    }
    *cursor = Some(connected.seq);
    let _ = notices.send(Notice::Changed).await;
    while let Some(incoming) = connected.incoming.recv().await {
        match incoming {
            Incoming::Event { seq, event } => {
                *cursor = Some(seq);
                match serde_json::from_value::<ControlEvent>(event) {
                    // Our own prompts are already on screen.
                    Ok(ControlEvent::UserMessage { origin, .. }) if origin == core.config.device_id => {}
                    Ok(mut event) => {
                        if let ControlEvent::SessionCreated { cwd, .. } = &mut event { *cwd = core.root(cwd); }
                        if events.send(event).await.is_err() { return Ok(()); }
                    }
                    Err(error) => warn!(%error, "server sent an event this app does not understand"),
                }
            }
            Incoming::Resync { .. } => {
                // We fell behind: reconnect without a cursor and rebuild what is open.
                *cursor = None;
                let _ = notices.send(Notice::Rebuild(core.config.id.clone())).await;
                return Ok(());
            }
            Incoming::Chunk(_) | Incoming::StreamEnd { .. } => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_router::Core;

    #[test]
    fn server_roots_round_trip_and_never_look_local() {
        let root = server_root("sabc123", "/home/max/projects/game");
        assert_eq!(root, "bomb-server://sabc123/home/max/projects/game");
        assert_eq!(split_root(&root), Some(("sabc123", "/home/max/projects/game")));
        assert!(is_server_root(&root));
        assert_eq!(crate::models::app::project_name(&root), "game");
        assert_eq!(split_root("/Users/max/src/game"), None);
        assert_eq!(split_root("bomb-server://nopath"), None);
    }

    /// The whole client path, against a real gateway and core on loopback.
    #[tokio::test]
    async fn a_paired_mac_runs_a_project_on_the_server_and_survives_a_dropped_connection() {
        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("BOMB_KEY_DIR", temp.path().join("keys"));
        // Mock-model threads are hidden from thread lists outside smoke runs.
        std::env::set_var("BOMB_SMOKE", "1");
        let home = temp.path().join("server-home");
        let grok = home.join("grok");
        let panel = grok.join("panel");
        let state = Arc::new(bomb_core::AppState::initialize_with_paths(grok_config::GrokPaths {
            home_dir: home.clone(), grok_dir: grok.clone(), config_file: panel.join("config.toml"),
            grok_cli_config_file: grok.join("config.toml"), worktrees_dir: home.join("worktrees"),
            memory_dir: panel.join("memory"), sessions_dir: panel.join("sessions"), panel_dir: panel,
            project_config_file: None, project_root: None,
        }).await.unwrap());
        let socket = temp.path().join("core.sock");
        tokio::spawn({ let socket = socket.clone(); async move { bomb_server::core::serve(state, &socket, std::future::pending()).await.unwrap() } });
        while !socket.exists() { tokio::time::sleep(Duration::from_millis(10)).await; }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let gateway = bomb_server::gateway::Gateway::open(&temp.path().join("gateway"), &address).unwrap();
        gateway.add_user("max", &socket, true).unwrap();
        tokio::spawn(gateway.clone().serve(listener, std::future::pending()));

        // Pair from a link, the way Settings → Servers does.
        assert!(pair("bomb://pair?nonsense", "", "Mac").await.is_err());
        let config = pair(&gateway.create_invite("max").unwrap().to_string(), "My VPS", "Test Mac").await.unwrap();
        assert_eq!((config.name.as_str(), config.user.as_str(), config.admin), ("My VPS", "max", true));

        let remotes = Arc::new(Remotes::default());
        let (events_tx, events) = async_channel::unbounded();
        let (notices_tx, notices) = async_channel::unbounded();
        remotes.connect(config.clone(), events_tx, notices_tx);
        let remote = remotes.get(&config.id).unwrap();
        while remote.state() != LinkState::Connected { let _ = notices.recv().await; }
        let core = Core::Remote(remote.clone());

        // Server folders are namespaced on this side, and plain paths on the server.
        let path: String = remote.call("create_project", json!({ "name": "Tetris Game!" })).await.unwrap();
        assert!(path.ends_with("/projects/tetris-game"), "{path}");
        let root = remote.root(&path);
        assert_eq!(core.list_projects().await.unwrap(), vec![root.clone()]);
        assert_eq!(remotes.for_root(&root).unwrap().config.id, config.id);
        assert!(remotes.for_root("/Users/max/local").is_none());
        assert!(core.project_overview(&root).await.unwrap().git_detected);
        assert_eq!(core.thread_setup_check(&root).await.unwrap(), bomb_core::services::thread_setup::Readiness::Ready);

        // Start a thread there and talk to it; its events arrive in the app's stream.
        let opts = grok_control_core::SpawnOptions { model: Some("mock".into()), prompt: Some("build tetris".into()), project_root: Some(root.clone()), isolate_worktree: true, ..Default::default() };
        let id = core.start_session(root.clone(), opts).await.unwrap();
        core.wait_until_idle(&id, Duration::from_secs(20)).await.unwrap();
        core.send_prompt(id.clone(), "build tetris".into(), None, None, None, None, None, None, None, None).await.unwrap();
        let mut created_in = None;
        let mut replied = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while !replied && tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(1), events.recv()).await {
                Ok(Ok(ControlEvent::SessionCreated { cwd, .. })) => created_in = Some(cwd),
                Ok(Ok(ControlEvent::AgentMessage { .. })) => replied = true,
                // Our own prompt is never echoed back to us.
                Ok(Ok(ControlEvent::UserMessage { .. })) => panic!("own prompt echoed"),
                _ => {}
            }
        }
        assert!(replied, "the server thread's reply reaches this Mac");
        assert!(created_in.is_some_and(|cwd| is_server_root(&cwd)), "event paths are namespaced too");

        let threads = core.list_threads().await.unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].project_root.as_deref(), Some(root.as_str()));
        assert!(is_server_root(&threads[0].cwd));
        let workspaces = core.list_workspaces().await.unwrap();
        assert_eq!(workspaces[0].project_root, root);
        assert!(!core.snapshot(id.clone()).await.unwrap().rows.is_empty());
        assert!(core.workspace_action(workspaces[0].id.clone(), "squash".into(), String::new()).await.unwrap_err().contains("server"));

        // A project that starts on this Mac goes up as Git history, comes back down, and stays in step.
        let git = |dir: std::path::PathBuf, args: Vec<&'static str>| async move { grok_worktree::run_git(&dir, &args).await.unwrap() };
        let local = temp.path().join("mac/site");
        std::fs::create_dir_all(&local).unwrap();
        for args in [vec!["init", "-q", "-b", "main"], vec!["config", "user.name", "Mac"], vec!["config", "user.email", "mac@example.com"]] { git(local.clone(), args).await; }
        std::fs::write(local.join("index.html"), "<h1>v1</h1>").unwrap();
        git(local.clone(), vec!["add", "-A"]).await;
        git(local.clone(), vec!["commit", "-q", "-m", "v1"]).await;
        let scratch = temp.path().join("scratch");
        let site = sync::send_to_server(&remote, local.to_str().unwrap(), "Site", &scratch).await.unwrap();
        assert!(is_server_root(&site) && core.list_projects().await.unwrap().contains(&site));
        let on_server = std::path::PathBuf::from(remote.path_of(&site));
        assert_eq!(std::fs::read_to_string(on_server.join("index.html")).unwrap(), "<h1>v1</h1>");
        assert_eq!(sync::sync(&remote, local.to_str().unwrap(), &site, &scratch).await.unwrap(), "Nothing new on the server · Nothing new on this Mac");

        // Overnight, a server thread commits. In the morning the Mac's copy catches up, files included.
        for args in [vec!["config", "user.name", "Server"], vec!["config", "user.email", "server@example.com"]] { git(on_server.clone(), args).await; }
        std::fs::write(on_server.join("index.html"), "<h1>v2 from the server</h1>").unwrap();
        git(on_server.clone(), vec!["commit", "-q", "-am", "v2"]).await;
        assert_eq!(sync::sync(&remote, local.to_str().unwrap(), &site, &scratch).await.unwrap(), "Nothing new on the server · 1 branch updated on this Mac");
        assert_eq!(std::fs::read_to_string(local.join("index.html")).unwrap(), "<h1>v2 from the server</h1>");
        // And offline work on the Mac goes back up.
        std::fs::write(local.join("about.html"), "about").unwrap();
        git(local.clone(), vec!["add", "-A"]).await;
        git(local.clone(), vec!["commit", "-q", "-m", "about page"]).await;
        assert_eq!(sync::sync(&remote, local.to_str().unwrap(), &site, &scratch).await.unwrap(), "1 branch updated on the server · Nothing new on this Mac");
        assert!(on_server.join("about.html").exists());

        let copy = temp.path().join("mac/site-copy");
        sync::download_copy(&remote, &site, &copy, &scratch).await.unwrap();
        assert!(copy.join("about.html").exists());
        assert!(std::fs::read_dir(&scratch).unwrap().next().is_none(), "no bundles are left lying around");

        // A file attached to a server thread is uploaded and lands in the person's own folder there.
        let local_file = temp.path().join("notes (final).md");
        std::fs::write(&local_file, "remember this").unwrap();
        let stream = remote.client().unwrap().upload(&local_file).await.unwrap();
        let stored: String = remote.call("store_attachment", json!({ "stream": stream, "name": "../../etc/notes (final).md" })).await.unwrap();
        assert!(stored.contains("/attachments/") && stored.ends_with("-notes (final).md"), "{stored}");
        assert_eq!(std::fs::read_to_string(&stored).unwrap(), "remember this");
        assert!(remote.request("store_attachment", json!({ "name": "x", "bundle_path": "/etc/passwd" })).await.is_err(), "a client cannot name a server file");

        // A terminal in the server project: typed here, run there, screen rendered here.
        let terminal = live::open_terminal(remote.clone(), &site).await.unwrap();
        terminal.write(b"printf 'BOMB_%s\\n' ON_SERVER; ls\r").unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !(terminal.screen().contents().contains("BOMB_ON_SERVER") && terminal.screen().contents().contains("about.html")) {
            assert!(tokio::time::Instant::now() < deadline, "terminal screen: {}", terminal.screen().contents());
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        terminal.write(b"exit\r").unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !terminal.exited() { assert!(tokio::time::Instant::now() < deadline, "the shell's exit reaches this Mac"); tokio::time::sleep(Duration::from_millis(50)).await; }
        assert!(live::open_terminal(remote.clone(), "bomb-server://other/etc").await.is_err());

        // A dev server that only listens on the server's loopback loads through a local port here.
        let dev = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dev_port = dev.local_addr().unwrap().port();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let Ok((mut socket, _)) = dev.accept().await else { return };
                let mut request = [0u8; 256];
                let _ = socket.read(&mut request).await;
                let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\nfrom site").await;
            }
        });
        let (local_port, guard) = live::forward_port(remote.clone(), dev_port).await.unwrap();
        assert_ne!(local_port, dev_port);
        let fetch = |port: u16| async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
            socket.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").await?;
            let mut body = String::new();
            socket.read_to_string(&mut body).await?;
            Ok::<_, std::io::Error>(body)
        };
        for _ in 0..2 { assert!(fetch(local_port).await.unwrap().ends_with("from site")); }
        drop(guard);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(fetch(local_port).await.is_err(), "the local port closes with the preview");

        // The server only touches this person's registered projects, and only files it received itself.
        assert!(remote.request("branch_tips", json!({ "root": "/etc" })).await.unwrap_err().contains("not one of your projects"));
        assert!(remote.request("import_bundle", json!({ "root": on_server, "bundle_path": "/etc/passwd" })).await.unwrap_err().contains("bundle_path"));

        // Removing the device on the server cuts this Mac off; it reports why instead of hanging.
        gateway.revoke_device(&config.device_id).unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !matches!(remote.state(), LinkState::Offline(_)) && tokio::time::Instant::now() < deadline { let _ = tokio::time::timeout(Duration::from_millis(200), notices.recv()).await; }
        assert!(matches!(remote.state(), LinkState::Offline(_)));
        assert!(core.list_threads().await.unwrap_err().contains("not connected"));
        remotes.remove(&config.id);
        assert!(keychain::load(&config.id).is_none());
    }
}
