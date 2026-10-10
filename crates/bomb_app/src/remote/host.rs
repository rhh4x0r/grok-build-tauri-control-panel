//! Phone access: this Mac hosts its own gateway so a paired phone can reach the threads that live here.
//!
//! It is the same gateway and core a server runs (`bomb_server`), wrapped around this app's own
//! [`AppState`], so a phone sees the threads on this Mac live and the Mac sees what the phone does.
//! It listens only on the address the person picks (their Tailscale or home network address), and
//! only paired devices get past the handshake. Data lives in `~/.bombcode/host`.

use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bomb_core::AppState;
use bomb_link::PairingLink;
use bomb_server::gateway::Gateway;
use bomb_server::store::Device;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tracing::{info, warn};

/// Stored in the local settings table.
pub const PHONE_KEY: &str = "phone_access";
/// Not `bombd`'s 7443, so a server on the same network never collides with a Mac.
pub const DEFAULT_PORT: u16 = 7444;
/// The one account on a Mac's gateway: the person at the Mac.
const OWNER: &str = "owner";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PhoneSettings {
    pub enabled: bool,
    /// The IP to listen on and to put in pairing links; the first suggested address when unset.
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
}

/// An address the phone could dial, and what it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Interface {
    pub ip: IpAddr,
    /// "Tailscale" or "Home network".
    pub kind: &'static str,
}

/// This Mac's addresses a phone could reach, Tailscale first. Loopback and link-local are left out.
pub fn interfaces() -> Vec<Interface> {
    let mut found = Vec::new();
    // SAFETY: getifaddrs fills a linked list we only read, then free with freeifaddrs.
    unsafe {
        let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut list) != 0 { return found; }
        let mut cursor = list;
        while !cursor.is_null() {
            let entry = &*cursor;
            cursor = entry.ifa_next;
            if entry.ifa_addr.is_null() || (*entry.ifa_addr).sa_family as i32 != libc::AF_INET { continue; }
            let addr = &*(entry.ifa_addr as *const libc::sockaddr_in);
            let ip = Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr));
            if ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() { continue; }
            // 100.64.0.0/10 is where Tailscale hands out addresses.
            let tailscale = ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 64;
            let kind = if tailscale { "Tailscale" } else if ip.is_private() { "Home network" } else { continue };
            if !found.iter().any(|i: &Interface| i.ip == IpAddr::V4(ip)) { found.push(Interface { ip: IpAddr::V4(ip), kind }); }
        }
        libc::freeifaddrs(list);
    }
    found.sort_by_key(|i| i.kind != "Tailscale");
    found
}

/// The running host. Dropping it stops listening; the threads themselves are untouched.
pub struct PhoneHost {
    gateway: Arc<Gateway>,
    stop: watch::Sender<bool>,
    _awake: KeepAwake,
}

impl Drop for PhoneHost {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

impl PhoneHost {
    /// Start listening on `address` (`ip:port`). Fails if the address is taken or not this Mac's.
    pub async fn start(state: Arc<AppState>, address: String, mac_name: Option<String>) -> Result<Self, String> {
        let dir: PathBuf = state.paths.bomb_dir.join("host");
        std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        // A fresh socket each start, so a stopping core's cleanup can never remove the new one's.
        for stale in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if stale.file_name().to_string_lossy().ends_with(".sock") { let _ = std::fs::remove_file(stale.path()); }
        }
        let socket = dir.join(format!("core-{}.sock", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        let gateway = Gateway::open(&dir.join("gateway"), &address)?;
        gateway.describe_as("mac", mac_name);
        if gateway.store().read().map_err(|e| e.to_string())?.users.iter().any(|u| u.name == OWNER) {
            gateway.store().update(|r| if let Some(owner) = r.users.iter_mut().find(|u| u.name == OWNER) { owner.socket = socket.clone(); }).map_err(|e| e.to_string())?;
        } else {
            gateway.add_user(OWNER, &socket, true)?;
        }
        let listener = tokio::net::TcpListener::bind(&address).await.map_err(|e| format!("Could not listen on {address}: {e}"))?;
        let (stop, stopped) = watch::channel(false);
        let until_stopped = |mut rx: watch::Receiver<bool>| async move { let _ = rx.wait_for(|s| *s).await; };

        let core_stopped = until_stopped(stopped.clone());
        let core_state = state.clone();
        let core_socket = socket.clone();
        tokio::spawn(async move {
            if let Err(error) = bomb_server::core::serve(core_state, &core_socket, core_stopped).await { warn!(%error, "phone access core stopped"); }
        });
        // The gateway relays to the core's socket; wait for it to exist.
        for _ in 0..100 {
            if socket.exists() { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let gateway_stopped = until_stopped(stopped.clone());
        tokio::spawn({
            let gateway = gateway.clone();
            async move { if let Err(error) = gateway.serve(listener, gateway_stopped).await { warn!(%error, "phone access gateway stopped"); } }
        });
        info!(%address, "phone access on");
        Ok(Self { gateway, stop, _awake: KeepAwake::watch(state, stopped) })
    }

    /// A one-time pairing link for this Mac (10 minutes).
    pub fn invite(&self) -> Result<PairingLink, String> {
        self.gateway.create_invite(OWNER)
    }

    pub fn devices(&self) -> Vec<Device> {
        self.gateway.store().read().map(|r| r.devices).unwrap_or_default()
    }

    /// Remove a paired phone; its live connection is cut at once.
    pub fn revoke(&self, device: &str) -> Result<(), String> {
        self.gateway.revoke_device(device)
    }
}

/// While a thread is working, keep this Mac from idle sleep, so a turn started from the phone finishes.
/// A closed lid still sleeps; that is what a server is for.
struct KeepAwake;

impl KeepAwake {
    fn watch(state: Arc<AppState>, mut stopped: watch::Receiver<bool>) -> Self {
        tokio::spawn(async move {
            let mut caffeinate: Option<tokio::process::Child> = None;
            loop {
                let busy = bomb_core::services::list_threads(&state).await.map(|threads| {
                    threads.iter().any(|t| t.live && matches!(t.status.as_str(), "starting" | "running" | "waiting_approval" | "cancelling"))
                }).unwrap_or(false);
                match (busy, caffeinate.is_some()) {
                    (true, false) => {
                        // -i: no idle sleep; -w: give up if the app goes away.
                        caffeinate = tokio::process::Command::new("/usr/bin/caffeinate").args(["-i", "-w", &std::process::id().to_string()]).kill_on_drop(true).spawn().ok();
                    }
                    (false, true) => caffeinate = None,
                    _ => {}
                }
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(20)) => {}
                    _ = stopped.wait_for(|s| *s) => break,
                }
            }
        });
        Self
    }
}

/// One scan for the phone: this Mac's link, then one for each server this Mac can reach.
pub fn pairing_code(mac: PairingLink, servers: Vec<PairingLink>) -> String {
    let mut links = vec![mac];
    links.extend(servers);
    bomb_link::bundle(&links)
}

/// The QR code for `text`, as rows of dark (`true`) and light modules.
pub fn qr_rows(text: &str) -> Option<Vec<Vec<bool>>> {
    let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::L).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    Some(colors.chunks(width).map(|row| row.iter().map(|c| *c == qrcode::Color::Dark).collect()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pairing_code_fits_a_qr_code_and_lists_the_mac_first() {
        let mac = PairingLink::new("100.101.102.103:7444", &"ab".repeat(32));
        let server = PairingLink::new("vps.example.com:7443", &"cd".repeat(32));
        let code = pairing_code(mac.clone(), vec![server.clone()]);
        assert_eq!(bomb_link::parse_links(&code).unwrap(), vec![mac, server]);
        let rows = qr_rows(&code).unwrap();
        assert!(rows.len() >= 21 && rows.iter().all(|r| r.len() == rows.len()));
    }

    /// The phone's own client, against this Mac's host: it sees a Mac, starts a thread, and the Mac's UI stream shows it.
    #[tokio::test]
    async fn a_phone_pairs_with_this_mac_and_its_thread_shows_up_here() {
        use bomb_mobile::machine::{LinkState, Machine, MachineListener, NewThread};
        /// Only the link state matters here.
        struct Quiet(Arc<std::sync::Mutex<Option<LinkState>>>);
        impl MachineListener for Quiet {
            fn on_link(&self, state: LinkState) { *self.0.lock().unwrap() = Some(state); }
            fn on_threads(&self, _: Vec<bomb_mobile::view::ThreadSummary>) {}
            fn on_thread(&self, _: String, _: Vec<bomb_mobile::view::ThreadPatch>, _: bomb_mobile::view::PresenceView) {}
            fn on_settings_changed(&self) {}
        }
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let grok = home.join("grok");
        let panel = grok.join("panel");
        let state = Arc::new(AppState::initialize_with_paths(grok_config::GrokPaths {
            home_dir: home.clone(), grok_dir: grok.clone(), bomb_dir: panel.clone(), config_file: panel.join("config.toml"),
            grok_cli_config_file: grok.join("config.toml"), worktrees_dir: home.join("worktrees"),
            memory_dir: panel.join("memory"), sessions_dir: panel.join("sessions"), panel_dir: panel,
            project_config_file: None, project_root: None,
        }).await.unwrap());
        let mut mac_ui = state.journal.attach_local().events;
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let host = PhoneHost::start(state.clone(), format!("127.0.0.1:{port}"), Some("Test Mac".into())).await.unwrap();
        let root = bomb_core::rpc::dispatch(&state, "setup", "create_project", serde_json::json!({ "name": "game" })).await.unwrap().as_str().unwrap().to_string();

        let outcomes = bomb_mobile::pairing::pair_all(pairing_code(host.invite().unwrap(), vec![]), "Test iPhone".into()).await.unwrap();
        let paired = outcomes[0].machine.clone().unwrap_or_else(|| panic!("{:?}", outcomes[0].error));
        assert_eq!((paired.kind.as_str(), paired.name.as_str()), ("mac", "Test Mac"));
        assert_eq!(host.devices().len(), 1);

        let link: Arc<std::sync::Mutex<Option<LinkState>>> = Default::default();
        let machine = Machine::new(paired, Box::new(Quiet(link.clone())));
        for _ in 0..100 {
            if *link.lock().unwrap() == Some(LinkState::Connected) { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let id = machine.start_thread(NewThread {
            project_root: root, backend: "grok".into(), model: Some("mock".into()), effort: None, approval_mode: None,
            prompt: "from the phone".into(), own_worktree: true, images: vec![],
        }).await.unwrap();

        // The Mac's own window hears about the thread and the phone's prompt, live.
        let (mut created, mut prompt) = (false, false);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while !(created && prompt) && tokio::time::Instant::now() < deadline {
            if let Ok(Some((_, event))) = tokio::time::timeout(Duration::from_millis(500), mac_ui.recv()).await {
                match event {
                    bomb_core::ControlEvent::SessionCreated { session_id, .. } if session_id.to_string() == id => created = true,
                    bomb_core::ControlEvent::UserMessage { text, .. } if text == "from the phone" => prompt = true,
                    _ => {}
                }
            }
        }
        assert!(created && prompt, "created {created}, prompt {prompt}");

        // Later prompts reach the Mac too, including after the phone was backgrounded and came back.
        async fn heard(mac_ui: &mut bomb_core::journal::EventReceiver, want: &str) -> bool {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            while tokio::time::Instant::now() < deadline {
                if let Ok(Some((_, bomb_core::ControlEvent::UserMessage { text, .. }))) = tokio::time::timeout(Duration::from_millis(500), mac_ui.recv()).await {
                    if text == want { return true; }
                }
            }
            false
        }
        let options = || bomb_mobile::machine::PromptOptions { backend: None, model: None, effort: None, approval_mode: None, images: vec![] };
        let settle = || bomb_core::rpc::dispatch(&state, "test", "wait_turn_settled", serde_json::json!({ "id": id, "seconds": 30 }));
        settle().await.unwrap();
        machine.send_prompt(id.clone(), "second".into(), options()).await.unwrap();
        assert!(heard(&mut mac_ui, "second").await, "the second prompt reaches the Mac");
        machine.set_active(false);
        tokio::time::sleep(Duration::from_secs(1)).await;
        machine.set_active(true);
        for _ in 0..100 {
            if *link.lock().unwrap() == Some(LinkState::Connected) { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        settle().await.unwrap();
        machine.send_prompt(id.clone(), "after coming back".into(), options()).await.unwrap();
        assert!(heard(&mut mac_ui, "after coming back").await, "a prompt after the phone reconnects reaches the Mac");

        // Removing the phone here cuts it off.
        let device = host.devices()[0].id.clone();
        host.revoke(&device).unwrap();
        for _ in 0..100 {
            if matches!(*link.lock().unwrap(), Some(LinkState::Offline { .. })) { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(matches!(*link.lock().unwrap(), Some(LinkState::Offline { .. })), "{:?}", link.lock().unwrap());
    }

    #[test]
    fn suggested_addresses_never_include_loopback() {
        assert!(interfaces().iter().all(|i| !i.ip.is_loopback()));
    }
}
