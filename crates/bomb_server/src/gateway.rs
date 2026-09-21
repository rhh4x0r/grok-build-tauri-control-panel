//! The one encrypted port on a server.
//!
//! The gateway knows devices, not projects. It finishes the TLS handshake,
//! looks the device up by its certificate fingerprint, and either answers a
//! few `gateway.*` requests itself or relays the connection, frame by frame,
//! to the Unix socket of that person's core. A device it does not know may do
//! exactly one thing: present a pairing secret.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bomb_link::{Identity, PairingLink};
use bomb_proto::{Connection, Frame, Message};
use serde_json::{json, Value};
use tokio::net::{TcpListener, UnixStream};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::store::{now, Device, Invite, Store, User, INVITE_TTL_SECS};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// A paired device that says nothing for this long is dropped.
const IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const PAIR_FAILURES_PER_IP: usize = 5;
const PAIR_FAILURES_GLOBAL: usize = 50;
const PAIR_WINDOW: Duration = Duration::from_secs(60);

pub struct Gateway {
    store: Store,
    identity: Identity,
    /// `host:port` that Macs should dial, as written into pairing links.
    public: String,
    revoked: broadcast::Sender<String>,
    /// How privileged work gets done: sudo on a shared server, unavailable on a one-person install.
    privileged: Box<dyn crate::admin::Privileged>,
    failures: Mutex<HashMap<IpAddr, Vec<Instant>>>,
}

impl Gateway {
    pub fn open(data: &Path, public: &str) -> Result<Arc<Self>, String> {
        Self::open_with(data, public, Box::new(crate::admin::Unavailable))
    }

    pub fn open_with(data: &Path, public: &str, privileged: Box<dyn crate::admin::Privileged>) -> Result<Arc<Self>, String> {
        let store = Store::open(data).map_err(|e| e.to_string())?;
        let identity_path = data.join("identity.json");
        let identity = match std::fs::read(&identity_path).ok().and_then(|bytes| serde_json::from_slice::<Identity>(&bytes).ok()) {
            Some(identity) => identity,
            None => {
                let identity = Identity::generate("bombd")?;
                std::fs::write(&identity_path, serde_json::to_vec(&identity).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&identity_path, std::fs::Permissions::from_mode(0o600));
                }
                identity
            }
        };
        Ok(Arc::new(Self { store, identity, public: public.to_string(), revoked: broadcast::channel(64).0, privileged, failures: Default::default() }))
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn fingerprint(&self) -> String {
        self.identity.fingerprint().unwrap_or_default()
    }

    pub fn add_user(&self, name: &str, socket: &Path, admin: bool) -> Result<(), String> {
        if !valid_user_name(name) { return Err("User names are 2-32 lowercase letters, digits or dashes.".into()); }
        self.store
            .update(|r| {
                match r.users.iter_mut().find(|u| u.name == name) {
                    Some(user) => { user.socket = socket.to_path_buf(); user.admin = admin; }
                    None => r.users.push(User { name: name.into(), socket: socket.to_path_buf(), admin, locked: false }),
                }
            })
            .map_err(|e| e.to_string())
    }

    /// A one-time link that pairs one new device to `user`. Only its hash is kept.
    pub fn create_invite(&self, user: &str) -> Result<PairingLink, String> {
        let link = PairingLink::new(&self.public, &self.fingerprint());
        let known = self
            .store
            .update(|r| {
                if !r.users.iter().any(|u| u.name == user && !u.locked) { return false; }
                r.invites.push(Invite { secret_hash: PairingLink::secret_hash(&link.secret), user: user.into(), expires: now() + INVITE_TTL_SECS });
                true
            })
            .map_err(|e| e.to_string())?;
        if known { Ok(link) } else { Err(format!("There is no active user named {user} on this server.")) }
    }

    pub async fn serve(self: Arc<Self>, listener: TcpListener, shutdown: impl std::future::Future<Output = ()>) -> Result<(), String> {
        let acceptor = tokio_rustls::TlsAcceptor::from(bomb_link::server_config(&self.identity)?);
        info!(address = ?listener.local_addr().ok(), fingerprint = %self.fingerprint(), "gateway listening");
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                _ = &mut shutdown => return Ok(()),
                accepted = listener.accept() => {
                    let Ok((tcp, peer)) = accepted else { continue };
                    let _ = tcp.set_nodelay(true);
                    let (gateway, acceptor) = (self.clone(), acceptor.clone());
                    tokio::spawn(async move {
                        // Strangers get a short window to finish TLS, so idle sockets cannot pile up.
                        let Ok(Ok(tls)) = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await else { return };
                        if let Err(error) = gateway.connection(tls, peer.ip()).await { debug!(%error, %peer, "connection ended"); }
                    });
                }
            }
        }
    }

    async fn connection(&self, tls: tokio_rustls::server::TlsStream<tokio::net::TcpStream>, ip: IpAddr) -> Result<(), String> {
        let fingerprint = bomb_link::peer_fingerprint(&tls).ok_or("no client certificate")?;
        let mut client = Connection::new(tls);
        let registry = self.store.read().map_err(|e| e.to_string())?;
        let device = registry.devices.iter().find(|d| bomb_link::same_digest(&d.fingerprint, &fingerprint)).cloned();
        let first = tokio::time::timeout(HANDSHAKE_TIMEOUT, client.recv()).await.map_err(|_| "no first message")?.map_err(|e| e.to_string())?.ok_or("closed")?;

        let Some(device) = device else {
            // Unknown device: pairing is the only thing on offer.
            let Frame::Message(Message::Request { id, method, params }) = first else { return Err("unpaired device".into()) };
            let result = if method == "gateway.pair" { self.pair(&fingerprint, ip, &params) } else { Err("This Mac is not paired with this server.".into()) };
            return respond(&mut client, id, result).await;
        };
        let Some(user) = registry.users.iter().find(|u| u.name == device.user && !u.locked).cloned() else {
            return Err("this person's access is locked".into());
        };
        let _ = self.store.update(|r| if let Some(d) = r.devices.iter_mut().find(|d| d.id == device.id) { d.last_seen = now(); });

        let mut revoked = self.revoked.subscribe();
        let cut = async {
            loop {
                match revoked.recv().await {
                    Ok(id) if id == device.id => return,
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => std::future::pending::<()>().await,
                }
            }
        };
        tokio::select! {
            _ = cut => Err("device revoked".into()),
            result = self.serve_device(client, first, &device, &user) => result,
        }
    }

    async fn serve_device<S>(&self, mut client: Connection<S>, first: Frame, device: &Device, user: &User) -> Result<(), String>
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        match first {
            // A session with this person's core: relay frames both ways until either side ends.
            Frame::Message(Message::Hello { .. }) => {
                let core = UnixStream::connect(&user.socket).await.map_err(|e| format!("this person's core is not running: {e}"))?;
                let mut core = Connection::new(core);
                core.send(&first).await.map_err(|e| e.to_string())?;
                loop {
                    tokio::select! {
                        from_client = tokio::time::timeout(IDLE_TIMEOUT, client.recv()) => match from_client {
                            Ok(Ok(Some(frame))) => core.send(&frame).await.map_err(|e| e.to_string())?,
                            Ok(Ok(None)) => return Ok(()),
                            Ok(Err(error)) => return Err(error.to_string()),
                            Err(_) => return Err("idle".into()),
                        },
                        from_core = core.recv() => match from_core.map_err(|e| e.to_string())? {
                            Some(frame) => client.send(&frame).await.map_err(|e| e.to_string())?,
                            None => return Ok(()),
                        },
                    }
                }
            }
            // Questions for the gateway itself.
            Frame::Message(Message::Request { id, method, params }) => {
                let (mut id, mut method, mut params) = (id, method, params);
                loop {
                    let result = self.gateway_request(&method, &params, device, user);
                    respond(&mut client, id, result).await?;
                    match tokio::time::timeout(IDLE_TIMEOUT, client.recv()).await {
                        Ok(Ok(Some(Frame::Message(Message::Request { id: i, method: m, params: p })))) => { id = i; method = m; params = p; }
                        _ => return Ok(()),
                    }
                }
            }
            _ => Err("expected a hello or a request".into()),
        }
    }

    fn pair(&self, fingerprint: &str, ip: IpAddr, params: &Value) -> Result<Value, String> {
        if self.throttled(ip) { return Err("Too many pairing attempts. Wait a minute and try again.".into()); }
        let secret = params.get("secret").and_then(Value::as_str).unwrap_or_default();
        let label: String = params.get("label").and_then(Value::as_str).unwrap_or("Mac").chars().filter(|c| !c.is_control()).take(60).collect();
        let hash = PairingLink::secret_hash(secret);
        let paired = self
            .store
            .update(|r| {
                let time = now();
                let index = r.invites.iter().position(|i| i.expires > time && bomb_link::same_digest(&i.secret_hash, &hash))?;
                // Single use: gone the moment it is accepted.
                let invite = r.invites.remove(index);
                if !r.users.iter().any(|u| u.name == invite.user && !u.locked) { return None; }
                let device = Device { id: hex::encode(rand::random::<[u8; 8]>()), label: label.clone(), user: invite.user, fingerprint: fingerprint.into(), created: time, last_seen: time };
                r.devices.push(device.clone());
                Some(device)
            })
            .map_err(|e| e.to_string())?;
        match paired {
            Some(device) => {
                info!(device = %device.id, user = %device.user, "device paired");
                Ok(json!({ "device_id": device.id, "user": device.user }))
            }
            None => {
                self.failures.lock().unwrap_or_else(|e| e.into_inner()).entry(ip).or_default().push(Instant::now());
                warn!(%ip, "pairing refused");
                Err("That pairing link is not valid any more. Links work once and expire after 10 minutes.".into())
            }
        }
    }

    fn throttled(&self, ip: IpAddr) -> bool {
        let mut failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let cutoff = Instant::now() - PAIR_WINDOW;
        failures.retain(|_, times| { times.retain(|t| *t > cutoff); !times.is_empty() });
        failures.get(&ip).map(Vec::len).unwrap_or(0) >= PAIR_FAILURES_PER_IP || failures.values().map(Vec::len).sum::<usize>() >= PAIR_FAILURES_GLOBAL
    }

    fn gateway_request(&self, method: &str, params: &Value, device: &Device, user: &User) -> Result<Value, String> {
        let registry = self.store.read().map_err(|e| e.to_string())?;
        match method {
            "gateway.whoami" => Ok(json!({ "device_id": device.id, "user": user.name, "admin": user.admin })),
            "gateway.list_devices" => Ok(json!(registry.devices.iter().filter(|d| user.admin || d.user == user.name).collect::<Vec<_>>())),
            "gateway.revoke_device" => {
                let id = params.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                let allowed = registry.devices.iter().any(|d| d.id == id && (user.admin || d.user == user.name));
                if !allowed { return Err("That device is not yours to remove.".into()); }
                self.revoke_device(&id)?;
                Ok(Value::Null)
            }
            "gateway.create_invite" => {
                let target = params.get("user").and_then(Value::as_str).unwrap_or(&user.name);
                if target != user.name && !user.admin { return Err("Only the server admin can invite other people.".into()); }
                Ok(json!({ "link": self.create_invite(target)?.to_string() }))
            }
            // Admin only: a new person gets their own Linux account and core, and a link for their first Mac.
            "gateway.invite_person" if user.admin => {
                let label: String = params.get("label").and_then(Value::as_str).unwrap_or("Invited person").chars().filter(|c| !c.is_control()).take(60).collect();
                let account = crate::admin::new_account_name();
                self.privileged.run(crate::admin::Verb::CreateUser, &account)?;
                self.privileged.run(crate::admin::Verb::StartCore, &account)?;
                self.add_user(&account, &crate::admin::socket_path(&account), false)?;
                info!(%account, %label, "person invited");
                Ok(json!({ "user": account, "label": label, "link": self.create_invite(&account)?.to_string() }))
            }
            // Admin only: stop someone's core, lock their account and cut every device they paired.
            "gateway.lock_person" if user.admin => {
                let target = params.get("user").and_then(Value::as_str).unwrap_or_default().to_string();
                if target == user.name { return Err("You cannot lock yourself out.".into()); }
                if !registry.users.iter().any(|u| u.name == target) { return Err("There is no such person on this server.".into()); }
                self.privileged.run(crate::admin::Verb::LockUser, &target)?;
                let devices: Vec<String> = self.store.update(|r| {
                    if let Some(u) = r.users.iter_mut().find(|u| u.name == target) { u.locked = true; }
                    r.invites.retain(|i| i.user != target);
                    let ids = r.devices.iter().filter(|d| d.user == target).map(|d| d.id.clone()).collect();
                    r.devices.retain(|d| d.user != target);
                    ids
                }).map_err(|e| e.to_string())?;
                for id in devices { let _ = self.revoked.send(id); }
                Ok(Value::Null)
            }
            "gateway.list_users" if user.admin => Ok(json!(registry.users.iter().map(|u| json!({ "name": u.name, "admin": u.admin, "locked": u.locked })).collect::<Vec<_>>())),
            _ => Err(format!("unknown method `{method}`")),
        }
    }

    /// Forget a device and cut any connection it has open right now.
    pub fn revoke_device(&self, id: &str) -> Result<(), String> {
        self.store.update(|r| r.devices.retain(|d| d.id != id)).map_err(|e| e.to_string())?;
        let _ = self.revoked.send(id.to_string());
        info!(device = %id, "device revoked");
        Ok(())
    }
}

async fn respond<S>(client: &mut Connection<S>, id: u64, result: Result<Value, String>) -> Result<(), String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (ok, error) = match result { Ok(value) => (Some(value), None), Err(error) => (None, Some(error)) };
    client.send(&Frame::Message(Message::Response { id, ok, error })).await.map_err(|e| e.to_string())
}

/// Names the registry accepts: the installer's own login, or a generated `bc-…` account.
pub fn valid_user_name(name: &str) -> bool {
    (2..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && !name.starts_with('-')
}
