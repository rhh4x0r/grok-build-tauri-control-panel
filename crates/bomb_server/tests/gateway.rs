//! The encrypted port, end to end on loopback: pairing, relaying to a core, revocation and abuse limits.

use std::sync::Arc;
use std::time::Duration;

use bomb_link::{Identity, PairingLink};
use bomb_proto::client::connect;
use bomb_server::gateway::Gateway;
use serde_json::{json, Value};

struct Server {
    gateway: Arc<Gateway>,
    address: String,
    _temp: tempfile::TempDir,
}

async fn server() -> Server {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let grok = home.join("grok");
    let panel = grok.join("panel");
    let state = Arc::new(
        bomb_core::AppState::initialize_with_paths(grok_config::GrokPaths {
            home_dir: home.clone(), grok_dir: grok.clone(), config_file: panel.join("config.toml"),
            grok_cli_config_file: grok.join("config.toml"), worktrees_dir: home.join("worktrees"),
            memory_dir: panel.join("memory"), sessions_dir: panel.join("sessions"), panel_dir: panel,
            project_config_file: None, project_root: None,
        })
        .await
        .unwrap(),
    );
    let socket = temp.path().join("core.sock");
    tokio::spawn({
        let socket = socket.clone();
        async move { bomb_server::core::serve(state, &socket, std::future::pending()).await.unwrap() }
    });
    while !socket.exists() { tokio::time::sleep(Duration::from_millis(10)).await; }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let gateway = Gateway::open(&temp.path().join("gateway"), &address).unwrap();
    gateway.add_user("max", &socket, true).unwrap();
    gateway.add_user("sam", &temp.path().join("sam-not-running.sock"), false).unwrap();
    tokio::spawn(gateway.clone().serve(listener, std::future::pending()));
    Server { gateway, address, _temp: temp }
}

/// One request on a fresh connection, the way the app talks to the gateway itself.
async fn ask(link: &PairingLink, device: &Identity, method: &str, params: Value) -> Result<Value, String> {
    let mut connection = bomb_proto::Connection::new(bomb_link::connect(&link.host, &link.fingerprint, device).await?);
    connection.send(&bomb_proto::Frame::Message(bomb_proto::Message::Request { id: 1, method: method.into(), params })).await.map_err(|e| e.to_string())?;
    match connection.recv().await.map_err(|e| e.to_string())? {
        Some(bomb_proto::Frame::Message(bomb_proto::Message::Response { ok, error, .. })) => match error { Some(e) => Err(e), None => Ok(ok.unwrap_or(Value::Null)) },
        other => Err(format!("unexpected reply: {other:?}")),
    }
}

#[tokio::test]
async fn pairing_is_single_use_and_a_paired_mac_reaches_only_its_own_core() {
    let server = server().await;
    let link = server.gateway.create_invite("max").unwrap();
    assert_eq!(link.host, server.address);
    let mac = Identity::generate("mac").unwrap();
    let stranger = Identity::generate("stranger").unwrap();

    // Before pairing a device can do nothing else, including talking to a core.
    assert!(ask(&link, &mac, "gateway.list_devices", Value::Null).await.unwrap_err().contains("not paired"));
    let tls = bomb_link::connect(&link.host, &link.fingerprint, &mac).await.unwrap();
    assert!(connect(tls, "mac", None).await.is_err());

    let paired = ask(&link, &mac, "gateway.pair", json!({ "secret": link.secret, "label": "Max’s MacBook" })).await.unwrap();
    assert_eq!(paired["user"], "max");
    // The same link does not work twice, even for someone who intercepted it.
    assert!(ask(&link, &stranger, "gateway.pair", json!({ "secret": link.secret })).await.unwrap_err().contains("not valid any more"));
    // The secret itself is never written down.
    let registry = std::fs::read_to_string(server.gateway.store().dir().join("registry.json")).unwrap();
    assert!(!registry.contains(&link.secret));

    // Paired: a normal session with this person's core, through the gateway.
    let tls = bomb_link::connect(&link.host, &link.fingerprint, &mac).await.unwrap();
    let session = connect(tls, "mac", None).await.unwrap();
    assert_eq!(session.client.request("ping", Value::Null).await.unwrap()["seq"], session.seq);
    assert_eq!(session.client.request("list_threads", Value::Null).await.unwrap(), json!([]));

    let me = ask(&link, &mac, "gateway.whoami", Value::Null).await.unwrap();
    assert_eq!((me["user"].as_str(), me["admin"].as_bool()), (Some("max"), Some(true)));
    let devices = ask(&link, &mac, "gateway.list_devices", Value::Null).await.unwrap();
    assert_eq!(devices.as_array().unwrap().len(), 1);
    assert_eq!(devices[0]["label"], "Max’s MacBook");

    // Revoking cuts the open session and refuses the device afterwards.
    let id = devices[0]["id"].as_str().unwrap().to_string();
    ask(&link, &mac, "gateway.revoke_device", json!({ "id": id })).await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(session.client.request("ping", Value::Null).await.is_err(), "the revoked session is cut");
    assert!(ask(&link, &mac, "gateway.whoami", Value::Null).await.unwrap_err().contains("not paired"));
}

#[tokio::test]
async fn people_cannot_manage_each_other_and_guessing_links_is_throttled() {
    let server = server().await;
    let (max, sam) = (Identity::generate("max").unwrap(), Identity::generate("sam").unwrap());
    let max_link = server.gateway.create_invite("max").unwrap();
    let sam_link = server.gateway.create_invite("sam").unwrap();
    ask(&max_link, &max, "gateway.pair", json!({ "secret": max_link.secret, "label": "max" })).await.unwrap();
    ask(&sam_link, &sam, "gateway.pair", json!({ "secret": sam_link.secret, "label": "sam" })).await.unwrap();

    // Sam sees and can remove only Sam's devices, and cannot invite for anyone else.
    let seen = ask(&sam_link, &sam, "gateway.list_devices", Value::Null).await.unwrap();
    assert_eq!(seen.as_array().unwrap().iter().map(|d| d["user"].as_str().unwrap()).collect::<Vec<_>>(), ["sam"]);
    let all = ask(&max_link, &max, "gateway.list_devices", Value::Null).await.unwrap();
    assert_eq!(all.as_array().unwrap().len(), 2);
    let max_device = all.as_array().unwrap().iter().find(|d| d["user"] == "max").unwrap()["id"].as_str().unwrap().to_string();
    assert!(ask(&sam_link, &sam, "gateway.revoke_device", json!({ "id": max_device })).await.unwrap_err().contains("not yours"));
    assert!(ask(&sam_link, &sam, "gateway.create_invite", json!({ "user": "max" })).await.unwrap_err().contains("admin"));
    assert!(ask(&sam_link, &sam, "gateway.list_users", Value::Null).await.is_err());
    assert!(PairingLink::parse(ask(&max_link, &max, "gateway.create_invite", json!({ "user": "sam" })).await.unwrap()["link"].as_str().unwrap()).is_ok());
    // Sam's core is not running: a clear failure, never someone else's core.
    let tls = bomb_link::connect(&sam_link.host, &sam_link.fingerprint, &sam).await.unwrap();
    assert!(connect(tls, "sam", None).await.is_err());

    // Guessing: after a handful of wrong secrets the address is refused even with a right one.
    let guesser = Identity::generate("guesser").unwrap();
    for _ in 0..5 {
        assert!(ask(&max_link, &guesser, "gateway.pair", json!({ "secret": "00".repeat(16) })).await.unwrap_err().contains("not valid"));
    }
    let real = server.gateway.create_invite("max").unwrap();
    assert!(ask(&real, &guesser, "gateway.pair", json!({ "secret": real.secret })).await.unwrap_err().contains("Too many"));
}

#[tokio::test]
async fn expired_links_and_garbage_are_refused() {
    let server = server().await;
    let link = server.gateway.create_invite("max").unwrap();
    server.gateway.store().update(|r| for invite in &mut r.invites { invite.expires = 1; }).unwrap();
    let mac = Identity::generate("mac").unwrap();
    assert!(ask(&link, &mac, "gateway.pair", json!({ "secret": link.secret })).await.unwrap_err().contains("not valid any more"));
    assert!(server.gateway.create_invite("nobody").is_err());

    // Raw junk after the handshake just ends the connection; the gateway keeps serving.
    use tokio::io::AsyncWriteExt;
    let mut tls = bomb_link::connect(&link.host, &link.fingerprint, &mac).await.unwrap();
    tls.write_all(&[0xff; 64]).await.unwrap();
    let _ = tls.shutdown().await;
    let fresh = server.gateway.create_invite("max").unwrap();
    assert!(ask(&fresh, &mac, "gateway.pair", json!({ "secret": fresh.secret })).await.is_ok());
}

/// Stands in for `sudo bombd admin …`, which needs root and Linux.
#[derive(Default)]
struct FakeRoot(std::sync::Mutex<Vec<(bomb_server::admin::Verb, String)>>);
struct FakeRootHandle(Arc<FakeRoot>);
impl bomb_server::admin::Privileged for FakeRootHandle {
    fn run(&self, verb: bomb_server::admin::Verb, account: &str) -> Result<(), String> {
        assert!(bomb_server::admin::valid_account(account), "the gateway only ever asks about generated accounts");
        self.0 .0.lock().unwrap().push((verb, account.to_string()));
        Ok(())
    }
}

#[tokio::test]
async fn the_admin_invites_a_person_who_gets_their_own_account_and_can_be_locked_out() {
    use bomb_server::admin::Verb;
    let temp = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let root = Arc::new(FakeRoot::default());
    let gateway = Gateway::open_with(&temp.path().join("gateway"), &address, Box::new(FakeRootHandle(root.clone()))).unwrap();
    gateway.add_user("max", &temp.path().join("max.sock"), true).unwrap();
    tokio::spawn(gateway.clone().serve(listener, std::future::pending()));
    let max = Identity::generate("max").unwrap();
    let max_link = gateway.create_invite("max").unwrap();
    ask(&max_link, &max, "gateway.pair", json!({ "secret": max_link.secret })).await.unwrap();

    let invited = ask(&max_link, &max, "gateway.invite_person", json!({ "label": "Sam" })).await.unwrap();
    let account = invited["user"].as_str().unwrap().to_string();
    assert!(bomb_server::admin::valid_account(&account));
    assert_eq!(*root.0.lock().unwrap(), [(Verb::CreateUser, account.clone()), (Verb::StartCore, account.clone())]);
    let registry = gateway.store().read().unwrap();
    let sam_user = registry.users.iter().find(|u| u.name == account).unwrap();
    assert!(!sam_user.admin);
    assert_eq!(sam_user.socket, std::path::PathBuf::from(format!("/run/bombd/{account}.sock")));

    // Sam pairs with the link, is not an admin, and cannot invite or lock anyone.
    let sam = Identity::generate("sam").unwrap();
    let sam_link = PairingLink::parse(invited["link"].as_str().unwrap()).unwrap();
    assert_eq!(ask(&sam_link, &sam, "gateway.pair", json!({ "secret": sam_link.secret })).await.unwrap()["user"], account.as_str());
    assert_eq!(ask(&sam_link, &sam, "gateway.whoami", Value::Null).await.unwrap()["admin"], false);
    assert!(ask(&sam_link, &sam, "gateway.invite_person", json!({})).await.is_err());
    assert!(ask(&sam_link, &sam, "gateway.lock_person", json!({ "user": "max" })).await.is_err());

    // Locking Sam stops their core, removes their devices and refuses them from then on.
    assert!(ask(&max_link, &max, "gateway.lock_person", json!({ "user": "max" })).await.unwrap_err().contains("yourself"));
    ask(&max_link, &max, "gateway.lock_person", json!({ "user": account })).await.unwrap();
    assert_eq!(root.0.lock().unwrap().last().unwrap(), &(Verb::LockUser, account.clone()));
    assert!(ask(&sam_link, &sam, "gateway.whoami", Value::Null).await.unwrap_err().contains("not paired"));
    assert!(gateway.create_invite(&account).is_err(), "no new links for a locked person");

    // A one-person install has no way to create accounts, and says so.
    let solo = Gateway::open(&temp.path().join("solo"), "127.0.0.1:1").unwrap();
    solo.add_user("owner", &temp.path().join("o.sock"), true).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let solo_link = PairingLink { host: listener.local_addr().unwrap().to_string(), ..solo.create_invite("owner").unwrap() };
    tokio::spawn(solo.clone().serve(listener, std::future::pending()));
    ask(&solo_link, &max, "gateway.pair", json!({ "secret": solo_link.secret })).await.unwrap();
    assert!(ask(&solo_link, &max, "gateway.invite_person", json!({})).await.unwrap_err().contains("one person"));
}
