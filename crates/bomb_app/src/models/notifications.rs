//! Telling you when a thread is done while you're elsewhere: finished, waiting for your approval,
//! or failed, as a macOS notification, only while Bomb Code isn't the app in front (in front, the
//! sidebar already shows it). Clicking one opens the thread. Asked for once, after your first
//! thread, with a card; "Not now" asks again a week later. See `crate::notify` for the helper.

use std::collections::HashSet;

use gpui_kit::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::notify::{self, Notifier, NotifyEvent};
use crate::runtime::{services, spawn_service};

const SETTINGS_KEY: &str = "notifications";
/// How long "Not now" puts off asking again.
const SNOOZE_SECS: i64 = 7 * 24 * 3600;

pub struct NotificationsHandle(pub Entity<Notifications>);
impl Global for NotificationsHandle {}

pub fn notifications(cx: &App) -> Option<Entity<Notifications>> {
    cx.try_global::<NotificationsHandle>().map(|h| h.0.clone())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NotificationSettings {
    /// The person chose to have them.
    pub enabled: bool,
    /// The card was answered (either way); "Not now" puts it off until `snoozed_until`.
    pub asked: bool,
    pub snoozed_until: Option<i64>,
}

/// What a notification says happened.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Happened {
    Finished,
    NeedsApproval,
    Failed,
}

#[derive(Default)]
pub struct Notifications {
    pub available: bool,
    pub settings: NotificationSettings,
    /// What macOS allows ("notDetermined", "denied", "authorized", "provisional"), once known.
    pub status: Option<String>,
    /// Bomb Code is the app in front.
    pub app_active: bool,
    /// Threads with a turn under way, so a thread that merely connects or resumes isn't "finished".
    busy: HashSet<Uuid>,
    helper: Option<Notifier>,
    loaded: bool,
}

impl Notifications {
    pub fn load(&mut self, cx: &mut Context<Self>) {
        self.available = notify::available();
        self.app_active = true;
        let state = services(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { bomb_core::services::kv_get(&state, SETTINGS_KEY).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                if let Some(saved) = res.ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()) { m.settings = saved; }
                m.loaded = true;
                // Learn what macOS allows (without asking), for Settings and the card.
                if m.available { if let Some(helper) = m.helper(cx) { helper.status(); } }
                cx.notify();
            });
        });
    }

    fn save(&self, cx: &mut Context<Self>) {
        let raw = serde_json::to_string(&self.settings).unwrap_or_default();
        let state = services(cx);
        spawn_service(cx, async move { bomb_core::services::kv_set(&state, SETTINGS_KEY, &raw).await }, |_, _| {});
    }

    fn helper(&mut self, cx: &mut Context<Self>) -> Option<&mut Notifier> {
        if self.helper.is_none() {
            let (helper, events) = Notifier::start().ok()?;
            self.helper = Some(helper);
            cx.spawn(async move |this, cx| {
                while let Ok(event) = events.recv().await {
                    if this.update(cx, |m, cx| m.on_event(event, cx)).is_err() { break; }
                }
            })
            .detach();
        }
        self.helper.as_mut()
    }

    fn on_event(&mut self, event: NotifyEvent, cx: &mut Context<Self>) {
        match event {
            NotifyEvent::Status(status) => self.status = Some(status),
            NotifyEvent::Clicked(thread) => {
                // Bring Bomb Code forward on that thread.
                cx.activate(true);
                if let (Ok(id), Some(app)) = (Uuid::parse_str(&thread), cx.try_global::<crate::models::app::AppModelHandle>().map(|h| h.0.clone())) {
                    app.update(cx, |m, cx| m.select(Some(id), cx));
                }
            }
        }
        cx.notify();
    }

    /// Show the "Get notified?" card now: never answered (or "Not now" has run out), and macOS
    /// hasn't been asked or still allows it.
    pub fn should_offer(&self) -> bool {
        let now = chrono::Utc::now().timestamp();
        self.available && self.loaded && !self.settings.enabled
            && (!self.settings.asked || self.settings.snoozed_until.is_some_and(|until| now >= until))
            && self.status.as_deref() != Some("denied")
    }

    /// "Turn on": ask macOS (its prompt shows the first time only).
    pub fn turn_on(&mut self, cx: &mut Context<Self>) {
        self.settings = NotificationSettings { enabled: true, asked: true, snoozed_until: None };
        self.save(cx);
        if let Some(helper) = self.helper(cx) { helper.authorize(); }
        cx.notify();
    }

    pub fn not_now(&mut self, cx: &mut Context<Self>) {
        self.settings.asked = true;
        self.settings.snoozed_until = Some(chrono::Utc::now().timestamp() + SNOOZE_SECS);
        self.save(cx);
        cx.notify();
    }

    pub fn turn_off(&mut self, cx: &mut Context<Self>) {
        self.settings.enabled = false;
        self.settings.asked = true;
        self.settings.snoozed_until = None;
        self.save(cx);
        cx.notify();
    }

    /// Ask macOS again what it allows (back from System Settings).
    pub fn refresh_status(&mut self, cx: &mut Context<Self>) {
        if let Some(helper) = self.helper(cx) { helper.status(); }
    }

    pub fn set_app_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.app_active != active {
            self.app_active = active;
            // Coming back from System Settings: maybe notifications were just allowed.
            if active && self.available && self.settings.enabled { self.refresh_status(cx); }
        }
    }

    /// A thread started a turn.
    pub fn note_busy(&mut self, thread: Uuid) {
        self.busy.insert(thread);
    }

    /// Something happened in a thread: notify if it's worth it and Bomb Code isn't in front.
    pub fn happened(&mut self, thread: Uuid, what: Happened, title: &str, detail: &str, cx: &mut Context<Self>) {
        let was_busy = match what {
            Happened::NeedsApproval => self.busy.contains(&thread),
            _ => self.busy.remove(&thread),
        };
        let allowed = matches!(self.status.as_deref(), Some("authorized" | "provisional"));
        if !was_busy || !self.settings.enabled || !allowed || self.app_active { return; }
        let body = match what {
            Happened::Finished if detail.is_empty() => "Done".to_string(),
            Happened::Finished => format!("Done: {detail}"),
            Happened::NeedsApproval => "Needs your approval".to_string(),
            Happened::Failed if detail.is_empty() => "Failed".to_string(),
            Happened::Failed => format!("Failed: {detail}"),
        };
        let id = format!("{thread}-{}", chrono::Utc::now().timestamp_millis());
        let thread = thread.to_string();
        if let Some(helper) = self.helper(cx) { helper.notify(&id, title, &body, &thread); }
    }

    /// The thread is on screen: its notifications are no longer news.
    pub fn opened(&mut self, thread: Uuid, cx: &mut Context<Self>) {
        if self.helper.is_some() {
            let thread = thread.to_string();
            if let Some(helper) = self.helper(cx) { helper.withdraw(&thread); }
        }
    }
}

/// The first line of a reply, short enough for a notification.
pub fn gist(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line = line.trim_start_matches(['#', '*', '-', '>', ' ']).replace("**", "");
    if line.chars().count() > 140 { format!("{}…", line.chars().take(139).collect::<String>()) } else { line }
}
