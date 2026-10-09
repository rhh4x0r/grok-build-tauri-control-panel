//! One paired Mac or server: a connection that reconnects by itself, the
//! thread list, and the threads open on screen.
//!
//! The thread list stays current from lifecycle events, which reach every
//! client. Only open threads stream their text and tool calls (`watch`), and
//! each is folded by the same reducer the desktop app uses. A thread is opened
//! by watching it and then loading a snapshot; events that arrive while the
//! snapshot loads are held and applied after it, above its `as_of`.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use bomb_link::Identity;
use bomb_proto::client::{Client, Incoming};
use bomb_transcript::presence::BOOM_HOLD;
use bomb_transcript::transcript::{Change, ImageAttachment, Thread};
use grok_events::{ControlEvent, TranscriptEntry};
use serde_json::{json, Value};
use tokio::sync::Notify;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::pairing::{gateway_request, PairedMachine};
use crate::view::{patches, BackendChoice, BackendRow, PresenceView, SidebarPrefs, ThreadPatch, ThreadRow, ThreadSummary, UsageView};
use crate::{on_runtime, runtime, MobileError, Result};

/// Rows loaded when a thread opens, and added each time the person scrolls to the top.
const PAGE: u32 = 200;
/// Keeps the gateway from closing a quiet connection (it drops clients silent for 15 minutes).
const KEEPALIVE: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum LinkState {
    Connecting,
    Connected,
    Offline { reason: String },
    /// The app is in the background; [`Machine::set_active`] resumes.
    Paused,
}

/// How a [`Machine`] tells Swift what changed. Called from a background thread.
#[uniffi::export(callback_interface)]
pub trait MachineListener: Send + Sync {
    fn on_link(&self, state: LinkState);
    /// The whole list, newest first, whenever any row changes.
    fn on_threads(&self, threads: Vec<ThreadSummary>);
    /// Changes to a thread that is open on screen.
    fn on_thread(&self, thread_id: String, patches: Vec<ThreadPatch>, presence: PresenceView);
    /// Settings shared by the person's devices (read-aloud) changed on the machine: ask again.
    fn on_settings_changed(&self);
}

/// A web server one of a thread's processes is listening on.
#[derive(Debug, Clone, PartialEq, uniffi::Record, serde::Deserialize)]
pub struct ThreadServer {
    pub port: u16,
    /// The program ("node", "vite").
    pub name: String,
}

/// Read-aloud settings the machine shares with the person's devices.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechSettings {
    /// The Fish Audio API key.
    pub api_key: Option<String>,
    pub voice: Option<String>,
    pub voice_name: Option<String>,
    pub rate: Option<f32>,
}

/// An image attached to a prompt.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ImageUpload {
    pub mime_type: String,
    /// Base64.
    pub data: String,
    pub name: Option<String>,
}

/// Choices sent with a prompt; `None` keeps what the thread already uses.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct PromptOptions {
    pub backend: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// plan | ask | auto. The phone never turns on yolo.
    pub approval_mode: Option<String>,
    pub images: Vec<ImageUpload>,
}

/// A thread to start from the phone.
#[derive(Debug, Clone, uniffi::Record)]
pub struct NewThread {
    pub project_root: String,
    pub backend: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// plan | ask | auto; defaults to plan.
    pub approval_mode: Option<String>,
    pub prompt: String,
    /// Work on its own branch in a worktree (the desktop default) rather than in the project folder.
    pub own_worktree: bool,
    pub images: Vec<ImageUpload>,
}

struct OpenThread {
    thread: Thread,
    /// Rows asked for on the last load.
    limit: u32,
    has_earlier: bool,
    /// Events at or below this are already in the loaded rows.
    as_of: u64,
    /// Events held while a snapshot loads; `None` once loaded.
    held: Option<Vec<(u64, ControlEvent)>>,
}

impl OpenThread {
    fn loading(limit: u32) -> Self {
        Self { thread: Thread::new(), limit, has_earlier: false, as_of: 0, held: Some(Vec::new()) }
    }
}

struct Shared {
    machine: PairedMachine,
    identity: Identity,
    listener: Box<dyn MachineListener>,
    client: RwLock<Option<Client>>,
    link: Mutex<LinkState>,
    threads: Mutex<BTreeMap<String, ThreadSummary>>,
    open: Mutex<HashMap<Uuid, OpenThread>>,
    active: AtomicBool,
    closed: AtomicBool,
    refresh_pending: AtomicBool,
    wake: Notify,
    /// The machine's web servers open in the phone's browser.
    previews: crate::preview::Previews,
}

/// A paired Mac or server. Connects as soon as it is made and keeps reconnecting until dropped.
#[derive(uniffi::Object)]
pub struct Machine {
    shared: Arc<Shared>,
}

impl Drop for Machine {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::SeqCst);
        self.shared.wake.notify_one();
    }
}

#[uniffi::export]
impl Machine {
    #[uniffi::constructor]
    pub fn new(machine: PairedMachine, listener: Box<dyn MachineListener>) -> Arc<Self> {
        let shared = Arc::new(Shared {
            identity: (&machine.identity).into(),
            machine,
            listener,
            client: RwLock::new(None),
            link: Mutex::new(LinkState::Connecting),
            threads: Mutex::new(BTreeMap::new()),
            open: Mutex::new(HashMap::new()),
            active: AtomicBool::new(true),
            closed: AtomicBool::new(false),
            refresh_pending: AtomicBool::new(false),
            wake: Notify::new(),
            previews: Default::default(),
        });
        runtime().spawn(run(shared.clone()));
        Arc::new(Self { shared })
    }

    pub fn info(&self) -> PairedMachine {
        self.shared.machine.clone()
    }

    pub fn link_state(&self) -> LinkState {
        self.shared.link.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The app moved to the foreground (`true`) or background (`false`). Coming back reconnects at once.
    pub fn set_active(&self, active: bool) {
        self.shared.active.store(active, Ordering::SeqCst);
        self.shared.wake.notify_one();
    }

    /// Try now instead of waiting out the backoff (pull to refresh, network came back).
    pub fn reconnect_now(&self) {
        self.shared.wake.notify_one();
    }

    /// The last thread list, newest first.
    pub fn threads(&self) -> Vec<ThreadSummary> {
        self.shared.summaries()
    }

    /// Fetch the thread list again.
    pub async fn refresh_threads(&self) -> Result<Vec<ThreadSummary>> {
        let shared = self.shared.clone();
        on_runtime(async move { shared.refresh_threads().await }).await
    }

    pub async fn list_projects(&self) -> Result<Vec<String>> {
        self.call("list_projects", Value::Null).await
    }

    /// A new empty Git project (with a first commit, so threads can start in it): in
    /// `~/Documents/BombCode` on a Mac, `~/projects` on a server. Returns its path.
    pub async fn create_project(&self, name: String) -> Result<String> {
        self.call("create_project", json!({ "name": name })).await
    }

    /// Clone a Git repository (GitHub or any URL the machine's own Git can reach) as a new project,
    /// next to the ones `create_project` makes. `name` defaults to the repository's. Returns its path.
    pub async fn clone_project(&self, url: String, name: Option<String>) -> Result<String> {
        self.call("clone_project", json!({ "url": url, "name": name })).await
    }

    pub async fn list_backends(&self) -> Result<Vec<BackendChoice>> {
        let rows: Vec<BackendRow> = self.call("list_backends", Value::Null).await?;
        Ok(rows.into_iter().map(BackendChoice::from).collect())
    }

    /// The machine's read-aloud settings. None from a machine too old to share them.
    pub async fn speech_settings(&self) -> Result<Option<SpeechSettings>> {
        Ok(self.call("speech_settings", Value::Null).await.ok())
    }

    /// Change the machine's read-aloud settings (`None` leaves one as it is; an empty key removes it).
    pub async fn set_speech_settings(&self, api_key: Option<String>, voice: Option<String>, voice_name: Option<String>, rate: Option<f32>) -> Result<()> {
        self.request("set_speech_settings", json!({ "apiKey": api_key, "voice": voice, "voiceName": voice_name, "rate": rate })).await?;
        Ok(())
    }

    /// Open `port` on the machine (its own loopback, where dev servers listen) for the phone's
    /// browser: returns the phone's port to load, the same one where it's free. Stays open until
    /// `close_previews`.
    pub async fn open_preview(&self, port: u16) -> Result<u16> {
        let shared = self.shared.clone();
        on_runtime(async move {
            let link = Arc::downgrade(&shared);
            shared.previews.open(port, move || link.upgrade().and_then(|s| s.client().ok())).await.map_err(MobileError::from)
        }).await
    }

    /// Stop every preview this machine has open.
    pub fn close_previews(&self) {
        self.shared.previews.close_all();
    }

    /// Web servers a thread's processes are listening on (port and program), to offer as previews.
    pub async fn thread_servers(&self, thread_id: String) -> Result<Vec<ThreadServer>> {
        Ok(self.call("thread_servers", json!({ "thread": thread_id })).await.unwrap_or_default())
    }

    /// Archived threads and pinned projects from the machine's sidebar. Empty from a machine too old to say.
    pub async fn sidebar_prefs(&self) -> Result<SidebarPrefs> {
        Ok(self.call("sidebar_prefs", Value::Null).await.unwrap_or_default())
    }

    /// Download a picture or video a thread shows (see `EntryView::media`) into this app's temporary
    /// files, and return the local path. A copy fetched before is reused while its size matches.
    pub async fn fetch_media(&self, thread_id: String, path: String) -> Result<String> {
        use base64::Engine;
        use std::hash::{Hash, Hasher};
        parse_id(&thread_id)?;
        let shared = self.shared.clone();
        on_runtime(async move {
            let ask = |offset: u64| {
                let shared = shared.clone();
                let (thread_id, path) = (thread_id.clone(), path.clone());
                async move { shared.request("read_media", json!({ "thread": thread_id, "path": path, "offset": offset })).await }
            };
            let first = ask(0).await?;
            let size = first["size"].as_u64().unwrap_or(0);
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            (&shared.machine.id, &path, size).hash(&mut hasher);
            let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
            let dir = std::env::temp_dir().join("bomb-media");
            let local = dir.join(format!("{:016x}.{ext}", hasher.finish()));
            if std::fs::metadata(&local).is_ok_and(|m| m.len() == size) {
                return Ok(local.display().to_string());
            }
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let part = local.with_extension(format!("{ext}.part"));
            let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
            let mut got: u64 = 0;
            let mut chunk = first;
            loop {
                let data = base64::engine::general_purpose::STANDARD.decode(chunk["data"].as_str().unwrap_or_default()).map_err(|e| e.to_string())?;
                if data.is_empty() { break; }
                std::io::Write::write_all(&mut file, &data).map_err(|e| e.to_string())?;
                got += data.len() as u64;
                if got >= size { break; }
                chunk = ask(got).await?;
            }
            drop(file);
            if got < size {
                let _ = std::fs::remove_file(&part);
                return Err("The file stopped arriving partway.".into());
            }
            std::fs::rename(&part, &local).map_err(|e| e.to_string())?;
            Ok(local.display().to_string())
        })
        .await
    }

    pub async fn account_usage(&self) -> Result<Vec<UsageView>> {
        self.call("account_usage", Value::Null).await
    }

    /// Start showing a thread: its patches arrive through `on_thread`, beginning with a `Reset`.
    pub async fn open_thread(&self, thread_id: String) -> Result<()> {
        let id = parse_id(&thread_id)?;
        let shared = self.shared.clone();
        on_runtime(async move {
            shared.open.lock().unwrap_or_else(|e| e.into_inner()).entry(id).or_insert_with(|| OpenThread::loading(PAGE));
            // Offline, it loads when the connection comes back.
            if shared.client().is_err() { return Ok(()); }
            shared.watch().await?;
            // Start its agent now if it's asleep, so the next message doesn't wait for it.
            // A machine too old to know `wake_thread` just says so; nothing to do then.
            let waker = shared.clone();
            runtime().spawn(async move { let _ = waker.request("wake_thread", json!({ "id": id.to_string() })).await; });
            shared.load(id).await
        })
        .await
    }

    /// Stop streaming a thread that left the screen.
    pub async fn close_thread(&self, thread_id: String) -> Result<()> {
        let id = parse_id(&thread_id)?;
        let shared = self.shared.clone();
        on_runtime(async move {
            shared.open.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            if shared.client().is_err() { return Ok(()); }
            shared.watch().await
        })
        .await
    }

    /// Load older history above what is shown. Arrives as a `Reset`.
    pub async fn load_earlier(&self, thread_id: String) -> Result<()> {
        let id = parse_id(&thread_id)?;
        let shared = self.shared.clone();
        on_runtime(async move {
            {
                let mut open = shared.open.lock().unwrap_or_else(|e| e.into_inner());
                let Some(thread) = open.get_mut(&id) else { return Err("That thread isn’t open.".into()) };
                if !thread.has_earlier { return Ok(()); }
                let limit = thread.limit + PAGE;
                *thread = OpenThread::loading(limit);
            }
            shared.load(id).await
        })
        .await
    }

    /// What the open thread's status line says now; call about once a second while a turn runs.
    pub fn presence(&self, thread_id: String) -> Option<PresenceView> {
        let id = Uuid::parse_str(&thread_id).ok()?;
        let open = self.shared.open.lock().unwrap_or_else(|e| e.into_inner());
        open.get(&id).map(|t| PresenceView::of(&t.thread.presence, Instant::now()))
    }

    /// Send a prompt to a thread. It shows on the phone at once; other devices see it as it arrives.
    pub async fn send_prompt(&self, thread_id: String, text: String, options: PromptOptions) -> Result<()> {
        let id = parse_id(&thread_id)?;
        if text.trim().is_empty() && options.images.is_empty() { return Err("Write something to send.".into()); }
        let approval_mode = safe_mode(options.approval_mode.clone())?;
        let shared = self.shared.clone();
        on_runtime(async move {
            shared.echo_prompt(id, &text, &options.images);
            let result = shared.request("send_prompt", json!({
                "id": thread_id, "prompt": text, "backend": options.backend, "model": options.model,
                "approval_mode": approval_mode, "images": images_param(&options.images), "effort": options.effort,
            })).await;
            if let Err(error) = &result { shared.note_failure(id, &error.to_string()); }
            result.map(|_| ())
        })
        .await
    }

    /// Start a thread in a project on this machine and send its first prompt. Returns the new thread's id.
    pub async fn start_thread(&self, new: NewThread) -> Result<String> {
        if new.prompt.trim().is_empty() { return Err("Write what the thread should do.".into()); }
        let approval_mode = safe_mode(new.approval_mode.clone())?.unwrap_or_else(|| "plan".into());
        let shared = self.shared.clone();
        on_runtime(async move {
            let opts = json!({
                "backend": new.backend, "model": new.model, "effort": new.effort, "approvalMode": approval_mode,
                "prompt": new.prompt, "projectRoot": new.project_root, "isolateWorktree": new.own_worktree,
            });
            let started = shared.request("start_session", json!({ "cwd": new.project_root, "opts": opts })).await?;
            let id = started["id"].as_str().ok_or("The machine did not say which thread it started.")?.to_string();
            let uuid = parse_id(&id)?;
            shared.open.lock().unwrap_or_else(|e| e.into_inner()).entry(uuid).or_insert_with(|| OpenThread::loading(PAGE));
            shared.watch().await?;
            shared.load(uuid).await?;
            shared.request("wait_until_idle", json!({ "id": id, "seconds": 90 })).await?;
            shared.echo_prompt(uuid, &new.prompt, &new.images);
            let sent = shared.request("send_prompt", json!({
                "id": id, "prompt": new.prompt, "approval_mode": approval_mode, "images": images_param(&new.images), "effort": new.effort,
            })).await;
            if let Err(error) = &sent { shared.note_failure(uuid, &error.to_string()); }
            sent?;
            shared.schedule_refresh();
            Ok(id)
        })
        .await
    }

    /// Answer an approval card. Returns `true` when another device answered it first.
    pub async fn respond_approval(&self, thread_id: String, request_id: String, option_id: String) -> Result<bool> {
        let answer = self.request("respond_approval", json!({ "id": thread_id, "request_id": request_id, "option_id": option_id })).await?;
        Ok(answer["already_resolved"].as_bool().unwrap_or(false))
    }

    pub async fn cancel(&self, thread_id: String) -> Result<()> {
        self.request("cancel_session", json!({ "id": thread_id })).await.map(|_| ())
    }

    pub async fn rename(&self, thread_id: String, label: String) -> Result<()> {
        self.request("rename_thread", json!({ "id": thread_id, "label": label })).await?;
        self.shared.schedule_refresh();
        Ok(())
    }

    pub async fn set_effort(&self, thread_id: String, effort: String) -> Result<()> {
        self.request("set_session_effort", json!({ "id": thread_id, "effort": effort })).await.map(|_| ())
    }

    /// Ask this machine to notify the phone (APNs device token, hex) when a thread needs it.
    pub async fn register_push(&self, token: String, sandbox: bool) -> Result<()> {
        self.request("register_push", json!({ "token": token, "sandbox": sandbox })).await.map(|_| ())
    }

    pub async fn unregister_push(&self) -> Result<()> {
        self.request("unregister_push", Value::Null).await.map(|_| ())
    }

    /// Remove this phone from the machine's devices. Forget the [`PairedMachine`] afterwards.
    pub async fn unpair(&self) -> Result<()> {
        let shared = self.shared.clone();
        on_runtime(async move {
            let m = &shared.machine;
            gateway_request(&m.host, &m.fingerprint, &shared.identity, "gateway.revoke_device", json!({ "id": m.device_id })).await?;
            shared.closed.store(true, Ordering::SeqCst);
            shared.wake.notify_one();
            Ok(())
        })
        .await
    }
}

impl Machine {
    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let shared = self.shared.clone();
        let method = method.to_string();
        on_runtime(async move { shared.request(&method, params).await }).await
    }

    async fn call<T: serde::de::DeserializeOwned + Send + 'static>(&self, method: &str, params: Value) -> Result<T> {
        let value = self.request(method, params).await?;
        serde_json::from_value(value).map_err(|e| MobileError::from(format!("Unexpected answer from {}: {e}", self.shared.machine.name)))
    }
}

fn parse_id(id: &str) -> Result<Uuid> {
    Uuid::parse_str(id).map_err(|_| "That isn’t a thread id.".into())
}

/// The phone can ask for plan, ask or auto, never yolo: always-approve stays a choice made at the desk.
fn safe_mode(mode: Option<String>) -> Result<Option<String>> {
    match mode.as_deref() {
        None => Ok(None),
        Some("plan" | "ask" | "auto") => Ok(mode),
        Some(other) => Err(format!("“{other}” can’t be chosen from the phone.").into()),
    }
}

fn images_param(images: &[ImageUpload]) -> Value {
    if images.is_empty() { return Value::Null; }
    json!(images.iter().map(|i| json!({ "mimeType": i.mime_type, "data": i.data, "name": i.name })).collect::<Vec<_>>())
}

impl Shared {
    fn emit_link(&self, state: LinkState) {
        let changed = {
            let mut link = self.link.lock().unwrap_or_else(|e| e.into_inner());
            let changed = *link != state;
            *link = state.clone();
            changed
        };
        if changed { self.listener.on_link(state); }
    }

    fn summaries(&self) -> Vec<ThreadSummary> {
        let mut list: Vec<_> = self.threads.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
        list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        list
    }

    fn emit_threads(&self) {
        self.listener.on_threads(self.summaries());
    }

    fn client(&self) -> Result<Client> {
        self.client.read().unwrap_or_else(|e| e.into_inner()).clone().ok_or_else(|| format!("{} isn’t connected right now.", self.machine.name).into())
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        Ok(self.client()?.request(method, params).await?)
    }

    async fn refresh_threads(&self) -> Result<Vec<ThreadSummary>> {
        let rows: Vec<ThreadRow> = serde_json::from_value(self.request("list_threads", Value::Null).await?)
            .map_err(|e| format!("Unexpected thread list from {}: {e}", self.machine.name))?;
        {
            let mut threads = self.threads.lock().unwrap_or_else(|e| e.into_inner());
            *threads = rows.into_iter().map(|row| (row.id.clone(), ThreadSummary::from_row(&self.machine.id, row))).collect();
        }
        self.emit_threads();
        Ok(self.summaries())
    }

    /// Refresh the list soon, once, however many events ask for it.
    fn schedule_refresh(self: &Arc<Self>) {
        if self.refresh_pending.swap(true, Ordering::SeqCst) { return; }
        let shared = self.clone();
        runtime().spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            shared.refresh_pending.store(false, Ordering::SeqCst);
            if let Err(error) = shared.refresh_threads().await { debug!(%error, "thread list refresh failed"); }
        });
    }

    /// Tell the machine which threads to stream.
    async fn watch(&self) -> Result<()> {
        let ids: Vec<String> = self.open.lock().unwrap_or_else(|e| e.into_inner()).keys().map(Uuid::to_string).collect();
        self.request("watch", json!({ "threads": ids })).await.map(|_| ())
    }

    /// Load (or reload) an open thread from a snapshot, then apply what arrived meanwhile.
    async fn load(&self, id: Uuid) -> Result<()> {
        let limit = match self.open.lock().unwrap_or_else(|e| e.into_inner()).get(&id) {
            Some(thread) => thread.limit,
            None => return Ok(()),
        };
        let snapshot = self.request("snapshot", json!({ "id": id.to_string(), "limit": limit })).await?;
        let rows: Vec<TranscriptEntry> = serde_json::from_value(snapshot["rows"].clone()).map_err(|e| format!("Unexpected history: {e}"))?;
        let pending: Vec<ControlEvent> = serde_json::from_value(snapshot["pending_approvals"].clone()).unwrap_or_default();
        let as_of = snapshot["as_of"].as_u64().unwrap_or(0);
        let has_earlier = snapshot["has_more"].as_bool().unwrap_or(false);
        let working = self.threads.lock().unwrap_or_else(|e| e.into_inner()).get(&id.to_string()).is_some_and(|t| t.running);
        let now = Instant::now();
        let update = {
            let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
            let Some(entry) = open.get_mut(&id) else { return Ok(()) };
            let held = entry.held.take().unwrap_or_default();
            let mut thread = Thread::new();
            // Saved images live on the machine; keep them as paths for the app to fetch.
            thread.keep_unreadable_images = true;
            thread.hydrate(&rows);
            // Saved rows drop an approval's id and choices; the machine still has them.
            for approval in &pending { thread.apply(approval, now); }
            // The agent may still be working: say so now, not at its next event.
            if working { thread.resume_in_progress(now); }
            for (_, event) in held.iter().filter(|(seq, _)| *seq > as_of) {
                thread.apply(event, now);
            }
            let reset = ThreadPatch::Reset { entries: thread.entries.iter().map(Into::into).collect(), has_earlier };
            let presence = PresenceView::of(&thread.presence, now);
            *entry = OpenThread { thread, limit, has_earlier, as_of, held: None };
            (reset, presence)
        };
        self.listener.on_thread(id.to_string(), vec![update.0], update.1);
        Ok(())
    }

    /// Show the phone's own prompt right away; the machine doesn't echo it back.
    fn echo_prompt(&self, id: Uuid, text: &str, images: &[ImageUpload]) {
        let images = images.iter().map(|i| ImageAttachment { mime_type: i.mime_type.clone(), data: i.data.clone(), name: i.name.clone() }).collect();
        self.change_thread(id, |thread, now| thread.note_prompt(text, images, now));
    }

    fn note_failure(&self, id: Uuid, message: &str) {
        self.change_thread(id, |thread, now| thread.note_failure(message, now));
    }

    fn change_thread(&self, id: Uuid, change: impl FnOnce(&mut Thread, Instant) -> Vec<Change>) {
        let now = Instant::now();
        let update = {
            let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
            let Some(entry) = open.get_mut(&id) else { return };
            if entry.held.is_some() { return; }
            let changes = change(&mut entry.thread, now);
            (patches(&entry.thread, &changes), PresenceView::of(&entry.thread.presence, now))
        };
        self.listener.on_thread(id.to_string(), update.0, update.1);
    }

    fn handle_event(self: &Arc<Self>, seq: u64, event: ControlEvent) {
        if matches!(&event, ControlEvent::UserMessage { origin, .. } if *origin == self.machine.device_id) { return; }
        if let ControlEvent::Raw { session_id: None, payload } = &event {
            if payload["channel"] == "settings" { self.listener.on_settings_changed(); }
            return;
        }
        let Some(id) = event.session_id() else { return };
        self.note_in_list(id, &event);

        let now = Instant::now();
        let update = {
            let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
            let Some(entry) = open.get_mut(&id) else { return };
            if let Some(held) = entry.held.as_mut() {
                held.push((seq, event));
                return;
            }
            if seq <= entry.as_of { return; }
            let changes = entry.thread.apply(&event, now);
            let boom = changes.contains(&Change::Boom);
            (patches(&entry.thread, &changes), PresenceView::of(&entry.thread.presence, now), boom)
        };
        let (patches, presence, boom) = update;
        self.listener.on_thread(id.to_string(), patches, presence);
        if boom {
            // A finished turn shows its moment, then returns to idle.
            let shared = self.clone();
            runtime().spawn(async move {
                tokio::time::sleep(BOOM_HOLD).await;
                shared.change_thread(id, |thread, now| thread.settle(now));
            });
        }
    }

    /// Keep the thread list's row for `id` current.
    fn note_in_list(self: &Arc<Self>, id: Uuid, event: &ControlEvent) {
        let key = id.to_string();
        let changed = {
            let mut threads = self.threads.lock().unwrap_or_else(|e| e.into_inner());
            match (threads.get_mut(&key), event) {
                (None, _) | (_, ControlEvent::SessionCreated { .. }) => {
                    drop(threads);
                    self.schedule_refresh();
                    return;
                }
                (Some(row), ControlEvent::SessionStatusChanged { status, at, .. }) => {
                    let status = serde_json::to_value(status).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
                    row.set_status(&status);
                    row.updated_at = at.to_rfc3339();
                    true
                }
                (Some(row), ControlEvent::ApprovalRequired { auto_approved: false, .. }) => {
                    row.needs_approval = true;
                    row.running = true;
                    true
                }
                (Some(row), ControlEvent::ApprovalResolved { .. }) => {
                    row.needs_approval = false;
                    true
                }
                // Named after its first prompt, then renamed by the title model or a person.
                (Some(row), ControlEvent::Raw { payload, .. })
                    if payload["channel"] == "thread" && payload["kind"] == "label" =>
                {
                    match payload["label"].as_str().map(str::trim).filter(|l| !l.is_empty()) {
                        Some(label) if row.label.as_deref() != Some(label) => {
                            row.label = Some(label.to_string());
                            true
                        }
                        _ => false,
                    }
                }
                (Some(row), ControlEvent::UserMessage { at, .. }) => {
                    row.updated_at = at.to_rfc3339();
                    row.running = true;
                    true
                }
                (Some(_), ControlEvent::SessionCompleted { .. } | ControlEvent::SessionCancelled { .. }) => {
                    drop(threads);
                    self.schedule_refresh();
                    return;
                }
                _ => false,
            }
        };
        if changed { self.emit_threads(); }
    }
}

/// Keep connected while the machine exists, with backoff while it can't be reached.
async fn run(shared: Arc<Shared>) {
    let mut cursor: Option<u64> = None;
    let mut rebuild = false;
    let mut wait = Duration::from_secs(1);
    loop {
        if shared.closed.load(Ordering::SeqCst) { break; }
        if !shared.active.load(Ordering::SeqCst) {
            shared.emit_link(LinkState::Paused);
            shared.wake.notified().await;
            wait = Duration::from_secs(1);
            continue;
        }
        if !matches!(*shared.link.lock().unwrap_or_else(|e| e.into_inner()), LinkState::Offline { .. }) {
            shared.emit_link(LinkState::Connecting);
        }
        match session(&shared, &mut cursor, &mut rebuild).await {
            Ok(()) => wait = Duration::from_secs(1),
            Err(reason) => {
                debug!(machine = %shared.machine.name, %reason, "connection failed");
                shared.emit_link(LinkState::Offline { reason });
            }
        }
        *shared.client.write().unwrap_or_else(|e| e.into_inner()) = None;
        if shared.closed.load(Ordering::SeqCst) { break; }
        if matches!(*shared.link.lock().unwrap_or_else(|e| e.into_inner()), LinkState::Connected) {
            shared.emit_link(LinkState::Offline { reason: "The connection dropped. Reconnecting…".into() });
        }
        if !shared.active.load(Ordering::SeqCst) { continue; }
        tokio::select! {
            _ = tokio::time::sleep(wait) => wait = (wait * 2).min(Duration::from_secs(30)),
            _ = shared.wake.notified() => wait = Duration::from_secs(1),
        }
    }
    debug!(machine = %shared.machine.name, "machine closed");
}

/// One connected stretch. Returns when the connection ends or the app pauses.
async fn session(shared: &Arc<Shared>, cursor: &mut Option<u64>, rebuild: &mut bool) -> Result<(), String> {
    let m = &shared.machine;
    let tls = bomb_link::connect(&m.host, &m.fingerprint, &shared.identity).await?;
    let mut connected = bomb_proto::client::connect(tls, &m.device_id, *cursor).await.map_err(|e| match e {
        bomb_proto::ProtoError::Closed => format!("{} refused this phone. It may have been removed from its devices; pair it again.", m.name),
        other => other.to_string(),
    })?;
    *shared.client.write().unwrap_or_else(|e| e.into_inner()) = Some(connected.client.clone());
    if connected.resync && cursor.is_some() { *rebuild = true; }
    *cursor = Some(connected.seq);
    shared.emit_link(LinkState::Connected);

    // Catch up: the list, which threads to stream, and the open threads that need loading.
    let rebuild_now = std::mem::take(rebuild);
    let catch_up = shared.clone();
    runtime().spawn(async move {
        if let Err(error) = catch_up.refresh_threads().await { warn!(%error, "thread list failed"); }
        let to_load: Vec<Uuid> = {
            let mut open = catch_up.open.lock().unwrap_or_else(|e| e.into_inner());
            if open.is_empty() { return; }
            for thread in open.values_mut() {
                if rebuild_now && thread.held.is_none() { *thread = OpenThread::loading(thread.limit); }
            }
            open.iter().filter(|(_, t)| t.held.is_some()).map(|(id, _)| *id).collect()
        };
        if let Err(error) = catch_up.watch().await { warn!(%error, "watch failed"); }
        for id in to_load {
            if let Err(error) = catch_up.load(id).await { warn!(%id, %error, "thread load failed"); }
        }
    });

    let mut keepalive = tokio::time::interval_at(tokio::time::Instant::now() + KEEPALIVE, KEEPALIVE);
    loop {
        tokio::select! {
            incoming = connected.incoming.recv() => match incoming {
                None => return Ok(()),
                Some(Incoming::Event { seq, event }) => {
                    *cursor = Some(seq);
                    match serde_json::from_value::<ControlEvent>(event) {
                        Ok(event) => shared.handle_event(seq, event),
                        Err(error) => warn!(%error, "an event this app does not understand"),
                    }
                }
                Some(Incoming::Resync { .. }) => {
                    // We fell behind: reconnect without a cursor and rebuild what is open.
                    *cursor = None;
                    *rebuild = true;
                    return Ok(());
                }
                Some(Incoming::Chunk(_) | Incoming::StreamEnd { .. }) => {}
            },
            _ = keepalive.tick() => {
                let client = connected.client.clone();
                runtime().spawn(async move { let _ = client.request("ping", Value::Null).await; });
            }
            _ = shared.wake.notified() => {
                if shared.closed.load(Ordering::SeqCst) || !shared.active.load(Ordering::SeqCst) { return Ok(()); }
            }
        }
    }
}
