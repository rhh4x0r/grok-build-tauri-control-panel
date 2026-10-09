//! Pairing the phone with a Mac or a server, the same way a Mac pairs with a server.
//!
//! The phone makes its own key per machine, proves the one-time secret from
//! the link, and pins the machine's certificate. Swift keeps the returned
//! [`PairedMachine`] (the key in the Keychain) and hands it back to
//! [`crate::machine::Machine::new`] on every launch.

use std::time::Duration;

use bomb_link::{Identity, PairingLink};
use bomb_proto::{Connection, Frame, Message};
use serde_json::{json, Value};

use crate::{on_runtime, Result};

/// This phone's key for one machine. The private key never leaves the phone.
#[derive(Debug, Clone, uniffi::Record)]
pub struct DeviceIdentity {
    pub cert_pem: String,
    pub key_pem: String,
}

impl From<Identity> for DeviceIdentity {
    fn from(identity: Identity) -> Self {
        Self { cert_pem: identity.cert_pem, key_pem: identity.key_pem }
    }
}

impl From<&DeviceIdentity> for Identity {
    fn from(identity: &DeviceIdentity) -> Self {
        Self { cert_pem: identity.cert_pem.clone(), key_pem: identity.key_pem.clone() }
    }
}

/// A Mac or server this phone is paired with.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PairedMachine {
    /// Stable id derived from the machine's fingerprint.
    pub id: String,
    pub name: String,
    /// `host:port` the phone dials.
    pub host: String,
    pub fingerprint: String,
    /// What the machine calls this phone; also the name it connects with.
    pub device_id: String,
    pub user: String,
    pub admin: bool,
    /// "mac" for a Mac running Bomb Code, "server" for `bombd`.
    pub kind: String,
    pub identity: DeviceIdentity,
}

/// How pairing with one link from a scanned code went.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PairOutcome {
    pub host: String,
    pub machine: Option<PairedMachine>,
    pub error: Option<String>,
}

/// The hosts named in a scanned or pasted pairing code, to show before pairing.
#[uniffi::export]
pub fn pairing_hosts(text: String) -> Result<Vec<String>> {
    Ok(bomb_link::parse_links(&text)?.into_iter().map(|link| link.host).collect())
}

/// Pair with every machine in a scanned code. One that can't be reached doesn't stop the others.
#[uniffi::export]
pub async fn pair_all(text: String, device_label: String) -> Result<Vec<PairOutcome>> {
    let links = bomb_link::parse_links(&text)?;
    Ok(on_runtime(async move {
        let mut outcomes = Vec::new();
        for link in links {
            let host = link.host.clone();
            outcomes.push(match pair_link(link, &device_label).await {
                Ok(machine) => PairOutcome { host, machine: Some(machine), error: None },
                Err(error) => PairOutcome { host, machine: None, error: Some(error) },
            });
        }
        outcomes
    })
    .await)
}

async fn pair_link(link: PairingLink, device_label: &str) -> Result<PairedMachine, String> {
    let identity = Identity::generate("bomb-code-phone")?;
    let paired = gateway_request(&link.host, &link.fingerprint, &identity, "gateway.pair", json!({ "secret": link.secret, "label": device_label })).await?;
    let me = gateway_request(&link.host, &link.fingerprint, &identity, "gateway.whoami", Value::Null).await.unwrap_or(Value::Null);
    let text = |value: &Value, key: &str| value[key].as_str().unwrap_or_default().to_string();
    let kind = me["kind"].as_str().unwrap_or("server").to_string();
    let name = me["name"].as_str().map(str::to_string).unwrap_or_else(|| link.host.rsplit_once(':').map_or(link.host.as_str(), |(h, _)| h).to_string());
    Ok(PairedMachine {
        id: format!("m{}", &link.fingerprint[..12]),
        name,
        host: link.host,
        fingerprint: link.fingerprint,
        device_id: text(&paired, "device_id"),
        user: text(&paired, "user"),
        admin: me["admin"].as_bool().unwrap_or(false),
        kind,
        identity: identity.into(),
    })
}

/// One question for the machine's gateway, on its own short connection.
pub(crate) async fn gateway_request(host: &str, fingerprint: &str, identity: &Identity, method: &str, params: Value) -> Result<Value, String> {
    let mut connection = Connection::new(bomb_link::connect(host, fingerprint, identity).await?);
    connection.send(&Frame::Message(Message::Request { id: 1, method: method.into(), params })).await.map_err(|e| e.to_string())?;
    match tokio::time::timeout(Duration::from_secs(20), connection.recv()).await.map_err(|_| "The machine did not answer.".to_string())?.map_err(|e| e.to_string())? {
        Some(Frame::Message(Message::Response { ok, error, .. })) => match error { Some(e) => Err(e), None => Ok(ok.unwrap_or(Value::Null)) },
        _ => Err("The machine closed the connection.".into()),
    }
}
