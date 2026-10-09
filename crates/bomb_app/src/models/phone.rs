//! Settings → Phone: whether this Mac hosts its threads for a paired phone, on which address, and
//! the code a phone scans to pair with this Mac and the servers it is linked to.

use std::sync::Arc;

use bomb_link::PairingLink;
use bomb_server::store::Device;
use gpui_kit::*;

use crate::remote::host::{self, PhoneHost, PhoneSettings};
use crate::runtime::{services, spawn_service};

pub struct PhoneHandle(pub Entity<PhoneModel>);
impl Global for PhoneHandle {}

pub fn phone(cx: &App) -> Entity<PhoneModel> {
    cx.global::<PhoneHandle>().0.clone()
}

/// What the phone scans: the text, its QR modules, and servers that could not be included.
#[derive(Clone)]
pub struct PairingCode {
    pub text: String,
    pub rows: Vec<Vec<bool>>,
    pub hosts: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Default)]
pub struct PhoneModel {
    pub settings: PhoneSettings,
    pub host: Option<Arc<PhoneHost>>,
    /// `ip:port` the running host listens on, which pairing codes carry. It can lag `address()`
    /// when no address was picked and a better one appeared since (Tailscale installed later).
    pub listening: Option<String>,
    /// Make a pairing code as soon as the host has (re)started.
    code_after_start: bool,
    pub busy: bool,
    pub error: Option<String>,
    pub code: Option<PairingCode>,
    pub devices: Vec<Device>,
}

impl PhoneModel {
    /// Read the saved choice and, if it was on, start listening again.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        let state = services(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { bomb_core::services::kv_get(&state, host::PHONE_KEY).await }, move |res, cx| {
            let saved: PhoneSettings = res.ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
            let _ = this.update(cx, |m, cx| {
                m.settings = saved;
                if m.settings.enabled { m.start(cx); }
                cx.notify();
            });
        });
    }

    fn save(&self, cx: &mut Context<Self>) {
        let raw = serde_json::to_string(&self.settings).unwrap_or_default();
        let state = services(cx);
        spawn_service(cx, async move { bomb_core::services::kv_set(&state, host::PHONE_KEY, &raw).await }, |_, _| {});
    }

    /// `ip:port` to listen on: the chosen address, or the first suggestion (Tailscale before the home network).
    pub fn address(&self) -> Option<String> {
        let ip = self.settings.address.clone().or_else(|| host::interfaces().first().map(|i| i.ip.to_string()))?;
        Some(format!("{ip}:{}", self.settings.port.unwrap_or(host::DEFAULT_PORT)))
    }

    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.settings.enabled = enabled;
        self.save(cx);
        if enabled { self.start(cx) } else { self.stop(cx) }
    }

    pub fn set_address(&mut self, ip: String, cx: &mut Context<Self>) {
        self.settings.address = Some(ip);
        self.save(cx);
        if self.settings.enabled {
            self.stop(cx);
            self.start(cx);
        }
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.host = None;
        self.listening = None;
        self.code = None;
        self.error = None;
        cx.notify();
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let Some(address) = self.address() else {
            self.error = Some("This Mac has no network address a phone could reach. Join Wi-Fi or Tailscale first.".into());
            cx.notify();
            return;
        };
        self.busy = true;
        self.error = None;
        let state = services(cx);
        let this = cx.entity().downgrade();
        let listening = address.clone();
        spawn_service(cx, async move {
            let name = std::process::Command::new("scutil").args(["--get", "ComputerName"]).output().ok()
                .and_then(|o| String::from_utf8(o.stdout).ok()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            PhoneHost::start(state, address, name).await
        }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                m.busy = false;
                match res {
                    // Turned off again while starting: let it go.
                    Ok(_) if !m.settings.enabled => {}
                    Ok(host) => {
                        m.devices = host.devices();
                        m.host = Some(Arc::new(host));
                        m.listening = Some(listening);
                        if std::mem::take(&mut m.code_after_start) { m.show_code(cx); }
                    }
                    Err(error) => {
                        m.code_after_start = false;
                        m.error = Some(error);
                    }
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    /// Make a fresh code: this Mac's link plus a new one from each connected server.
    pub fn show_code(&mut self, cx: &mut Context<Self>) {
        // The best address changed since this started listening: move first, so the code carries the new one.
        if self.host.is_some() && self.listening != self.address() {
            self.stop(cx);
            self.code_after_start = true;
            self.start(cx);
            return;
        }
        let Some(host) = self.host.clone() else { return };
        let mac = match host.invite() {
            Ok(link) => link,
            Err(error) => { self.error = Some(error); cx.notify(); return; }
        };
        let servers = crate::runtime::servers(cx).all();
        self.busy = true;
        let this = cx.entity().downgrade();
        spawn_service(cx, async move {
            let mut links = Vec::new();
            let mut skipped = Vec::new();
            for server in servers {
                let invite = server.gateway("gateway.create_invite", serde_json::json!({})).await
                    .and_then(|v| PairingLink::parse(v["link"].as_str().unwrap_or_default()));
                match invite {
                    Ok(link) => links.push(link),
                    Err(_) => skipped.push(server.config.name.clone()),
                }
            }
            (links, skipped)
        }, move |(links, skipped), cx| {
            let _ = this.update(cx, |m, cx| {
                m.busy = false;
                let mut hosts = vec![mac.host.clone()];
                hosts.extend(links.iter().map(|l| l.host.clone()));
                let text = host::pairing_code(mac, links);
                match host::qr_rows(&text) {
                    Some(rows) => m.code = Some(PairingCode { text, rows, hosts, skipped }),
                    None => m.error = Some("The pairing code is too long for a QR code.".into()),
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    pub fn refresh_devices(&mut self, cx: &mut Context<Self>) {
        self.devices = self.host.as_ref().map(|h| h.devices()).unwrap_or_default();
        cx.notify();
    }

    pub fn revoke(&mut self, device: String, cx: &mut Context<Self>) {
        if let Some(host) = &self.host {
            if let Err(error) = host.revoke(&device) { self.error = Some(error); }
        }
        self.refresh_devices(cx);
    }
}
