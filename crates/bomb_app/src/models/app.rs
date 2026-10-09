//! Top-level UI state: projects, threads, services, selection, composer
//! preferences, and every user action that talks to the backend.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use bomb_core::devserver::DevServerStatus;
use bomb_core::queue::{Batch, PromptQueue, TurnEnd};
use bomb_core::services::{self, BackendInfo, ImageInput};
use bomb_core::transcript::ImageAttachment;
use bomb_core::ControlEvent;
use gpui_kit::*;
use grok_cli_wrapper::{BackendAuth, LoginPhase, LoginSessionState};
use grok_control_core::{ApprovalMode, SpawnOptions};
use grok_persistence::ThreadDto;
use tracing::debug;
use uuid::Uuid;

use crate::models::thread::ThreadModel;
use crate::runtime::{services as svc, spawn_service};

/// The one `AppModel` entity, reachable from any view.
pub struct AppModelHandle(pub Entity<AppModel>);
impl Global for AppModelHandle {}

const ARCHIVED_KEY: &str = "archived_threads";
const SIDEBAR_SORT_KEY: &str = "sidebar_sort";
const RECENT_WINDOW_KEY: &str = "sidebar_recent_window";
/// When each thread was last looked at (JSON map of id → RFC 3339).
const THREAD_SEEN_KEY: &str = "thread_seen";
/// Activity before this counts as seen: threads don't all light up the first time.
const THREAD_SEEN_SINCE_KEY: &str = "thread_seen_since";
const PINNED_PROJECTS_KEY: &str = "pinned_projects";
const PROJECT_INTRO_KEY: &str = "project_intro_seen";
/// How long a queued message waits for the last turn to wind down before it goes anyway.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct ProjectGroup {
    pub root: String,
    pub name: String,
    pub threads: Vec<Uuid>,
}

/// Keeps the forwarded port open for as long as the server's dev server is in use.
struct ServerPreview {
    server: String,
    _forward: crate::remote::live::ForwardGuard,
    local_url: String,
}

/// How the sidebar orders projects and the threads inside them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarSort {
    /// Last activity first; a project is as recent as its most recent thread.
    #[default]
    Recent,
    /// Newest created first.
    Created,
    Alphabetical,
}

impl SidebarSort {
    pub const ALL: [SidebarSort; 3] = [SidebarSort::Recent, SidebarSort::Created, SidebarSort::Alphabetical];
    pub fn key(self) -> &'static str {
        match self { Self::Recent => "recent", Self::Created => "created", Self::Alphabetical => "alphabetical" }
    }
    pub fn from_key(key: &str) -> Self {
        Self::ALL.into_iter().find(|s| s.key() == key).unwrap_or_default()
    }
    pub fn label(self) -> &'static str {
        match self { Self::Recent => "Most recent", Self::Created => "Time created", Self::Alphabetical => "Alphabetical" }
    }
}

/// The project to open when none is chosen yet: where the most recent real thread ran,
/// else the first listed project. Never the home folder, which only opens when picked.
pub fn startup_project(threads: &[ThreadDto], projects: &[String]) -> Option<String> {
    let home = std::env::var("HOME").ok();
    let usable = |root: &str| !root.is_empty() && Some(root.trim_end_matches('/')) != home.as_deref();
    threads
        .iter()
        .filter(|t| !t.model.eq_ignore_ascii_case("mock"))
        .map(|t| (t.updated_at.as_str(), t.project_root.clone().unwrap_or_else(|| t.cwd.clone())))
        .filter(|(_, root)| usable(root))
        .max_by(|a, b| a.0.cmp(b.0))
        .map(|(_, root)| root)
        .or_else(|| projects.iter().find(|p| usable(p)).cloned())
}

/// Tabs of the right panel. Changes is its own panel; the others share the preview panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RightTab {
    #[default]
    Preview,
    Processes,
    Files,
    Changes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightPanelRequest {
    Show(RightTab),
    Close,
}

/// How far back a closed project still lists its threads in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecentWindow {
    Off,
    Day,
    #[default]
    ThreeDays,
    Week,
    Month,
}

impl RecentWindow {
    pub const ALL: [RecentWindow; 5] = [Self::Off, Self::Day, Self::ThreeDays, Self::Week, Self::Month];
    pub fn key(self) -> &'static str {
        match self { Self::Off => "off", Self::Day => "1d", Self::ThreeDays => "3d", Self::Week => "1w", Self::Month => "1m" }
    }
    pub fn from_key(key: &str) -> Self {
        Self::ALL.into_iter().find(|w| w.key() == key).unwrap_or_default()
    }
    pub fn label(self) -> &'static str {
        match self { Self::Off => "Off", Self::Day => "1 day", Self::ThreeDays => "3 days", Self::Week => "1 week", Self::Month => "1 month" }
    }
    /// For the sidebar button, where width is tight.
    pub fn short_label(self) -> &'static str {
        match self { Self::Off => "Off", Self::Day => "1d", Self::ThreeDays => "3d", Self::Week => "1w", Self::Month => "1m" }
    }
    pub fn duration(self) -> Option<chrono::Duration> {
        match self {
            Self::Off => None,
            Self::Day => Some(chrono::Duration::days(1)),
            Self::ThreeDays => Some(chrono::Duration::days(3)),
            Self::Week => Some(chrono::Duration::weeks(1)),
            Self::Month => Some(chrono::Duration::days(30)),
        }
    }
}

/// Threads active since `now - window`, most recent first. Rows are (last change as RFC 3339, id).
pub fn recent_ids<T: Clone>(rows: &[(String, T)], window: RecentWindow, now: chrono::DateTime<chrono::Utc>) -> Vec<T> {
    let Some(span) = window.duration() else { return Vec::new() };
    let since = now - span;
    let mut recent: Vec<(chrono::DateTime<chrono::Utc>, T)> = rows
        .iter()
        .filter_map(|(at, id)| at.parse::<chrono::DateTime<chrono::Utc>>().ok().filter(|t| *t >= since).map(|t| (t, id.clone())))
        .collect();
    recent.sort_by_key(|r| std::cmp::Reverse(r.0));
    recent.into_iter().map(|(_, id)| id).collect()
}

/// Ids of the open rows to tuck behind "View more": everything past the `keep` most recently changed,
/// except the row the user is on. Each row is (last change, is current, id).
pub fn hidden_rows(mut rows: Vec<(String, bool, String)>, keep: usize) -> HashSet<String> {
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    rows.into_iter().enumerate().filter(|(ix, (_, current, _))| *ix >= keep && !current).map(|(_, (_, _, id))| id).collect()
}

/// What the sidebar sorts by, for one project or one thread row.
pub struct SortKey {
    pub name: String,
    pub updated: String,
    pub created: String,
}

/// Order rows in place. Timestamps are ISO strings, so text order is time order; rows without activity go last.
pub fn sort_rows<T>(rows: &mut [(SortKey, T)], sort: SidebarSort) {
    rows.sort_by(|(a, _), (b, _)| {
        let by_name = || a.name.to_lowercase().cmp(&b.name.to_lowercase());
        match sort {
            SidebarSort::Recent => b.updated.cmp(&a.updated).then_with(by_name),
            SidebarSort::Created => b.created.cmp(&a.created).then_with(by_name),
            SidebarSort::Alphabetical => by_name(),
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

/// What the composer sends with each prompt.
#[derive(Debug, Clone)]
pub struct ComposerPrefs {
    pub backend: String,
    pub model: Option<String>,
    /// plan | ask | auto | yolo
    pub mode: String,
    pub worktree: bool,
    pub location: String,
    pub base_branch: Option<String>,
    pub existing_branch: Option<String>,
    pub read_only: bool,
    pub mcp_servers: Vec<String>,
    /// low | medium | high (Grok only today).
    pub effort: String,
    pub fast_mode: Option<bool>,
    /// Temporary chat: run in the project checkout with no worktree, and
    /// don't keep the thread when it's deleted. (Today: no worktree.)
    pub temporary: bool,
}

impl Default for ComposerPrefs {
    fn default() -> Self {
        Self {
            backend: "grok".into(),
            model: None,
            mode: "plan".into(),
            worktree: true,
            location: "new".into(),
            base_branch: None,
            existing_branch: None,
            read_only: false,
            mcp_servers: Vec::new(),
            effort: "high".into(),
            fast_mode: Some(false),
            temporary: false,
        }
    }
}

pub const APPROVAL_CYCLE: [&str; 4] = ["plan", "ask", "auto", "yolo"];

pub struct AppModel {
    pub projects: Vec<String>,
    pub workspaces: Vec<grok_persistence::WorkspaceRecord>,
    pub project_status: HashMap<String, bomb_core::services::workspaces::ProjectStatus>,
    pub project_overviews: HashMap<String, Result<bomb_core::services::project_overview::ProjectOverview, String>>,
    pub overview_loading: HashSet<String>,
    pub overview_branch: Option<String>,
    /// The project page's explanation of the default branch has been dismissed.
    pub project_intro_seen: bool,
    /// Projects whose "In main" column is showing every card.
    pub show_all_merged: HashSet<String>,
    /// Lists as each core last reported them; the combined lists below are rebuilt from these,
    /// so a server that is briefly offline keeps its projects and threads on screen.
    local_threads: Vec<ThreadDto>,
    /// This Mac's thread list has arrived at least once.
    threads_loaded: bool,
    local_workspaces: Vec<grok_persistence::WorkspaceRecord>,
    local_projects: Vec<String>,
    #[allow(clippy::type_complexity)]
    server_lists: HashMap<String, (Vec<ThreadDto>, Vec<grok_persistence::WorkspaceRecord>, Vec<String>)>,
    /// Which folder on this Mac is a copy of which server project.
    pub project_links: Vec<crate::remote::sync::Link>,
    /// A dev server running on a paired server, reached through a forwarded local port.
    server_preview: Option<ServerPreview>,
    /// A provider sign-in to show in a server terminal: (folder on the server, command, title). The root view opens it.
    pub server_login_request: Option<(String, String, String)>,
    /// Which threads build on which, per project root.
    pub stacks: HashMap<String, Vec<bomb_core::services::workspaces::StackLink>>,
    /// For a linked project: which copy is ahead, as last checked.
    pub sync_state: HashMap<String, String>,
    /// A send, download or sync is running.
    pub syncing: bool,
    /// A pairing attempt is in flight.
    pub pairing: bool,
    pub sidebar_sort: SidebarSort,
    /// Closed projects list their threads active within this window.
    pub recent_window: RecentWindow,
    /// When each thread was last on screen; finished work after that is unseen.
    seen: HashMap<Uuid, chrono::DateTime<chrono::Utc>>,
    /// Unset until loaded, and nothing is unseen until then.
    seen_since: Option<chrono::DateTime<chrono::Utc>>,
    /// Project roots shown in the sidebar's Pinned section, in the order they were pinned.
    pub pinned_projects: Vec<String>,
    /// Workspaces to close once their agent finishes merging.
    pub close_after_merge: HashSet<String>,
    pub active_workspace: Option<String>,
    pub source_thread: Option<String>,
    pub review: Option<bomb_core::services::workspaces::WorkspaceReview>,
    pub review_open: bool,
    pub git_busy: bool,
    pub review_loading: bool,
    review_loading_for: Option<String>,
    pub active_project: Option<String>,
    pub thread_order: Vec<Uuid>,
    pub threads: HashMap<Uuid, Entity<ThreadModel>>,
    pub selected: Option<Uuid>,
    pub new_thread_open: bool,
    pub foundry_request: Option<String>,
    pub foundry_insert: Option<String>,
    pub foundry_close: bool,
    pub foundry_show_runs: bool,
    pub file_reveal_request: Option<std::path::PathBuf>,
    /// Asks the window to switch the right panel's tab or close it.
    pub right_panel_request: Option<RightPanelRequest>,
    /// A web link to open in the right panel's Preview tab.
    pub browse_request: Option<String>,
    /// Processes running from the selected thread's folder (see `refresh_processes`).
    pub processes: Vec<bomb_core::services::processes::ProcessInfo>,
    /// The folder `processes` was read for.
    pub processes_folder: Option<String>,
    /// Last scan for every local thread and project folder, so switching shows a list at once.
    processes_by_folder: HashMap<String, Vec<bomb_core::services::processes::ProcessInfo>>,
    processes_scanning: bool,
    /// Another scan was asked for while one ran.
    processes_again: bool,
    pub auth: Vec<BackendAuth>,
    /// Account usage limits (5h / weekly) per backend, refreshed slowly.
    pub usage: Vec<bomb_core::usage::AccountUsage>,
    pub backends: Vec<BackendInfo>,
    pub models_loading: bool,
    pub prefs: ComposerPrefs,
    pub dev_server: Option<DevServerStatus>,
    /// Names of enabled MCP servers (for the composer picker).
    pub mcp_names: Vec<String>,
    pub login: Option<LoginSessionState>,
    /// A device-code login was requested and the first status is pending.
    pub login_starting: bool,
    pub last_error: Option<String>,
    /// Pending toasts; the root view drains them into the notification layer.
    pub toasts: VecDeque<(ToastKind, String)>,
    /// A prompt is in flight for a not-yet-created thread.
    pub starting: bool,
    pub start_failure_serial: u64,
    /// Messages typed while a thread's agent works, per thread; `None` is the
    /// thread still being started. See `bomb_core::queue`.
    pub queues: HashMap<Option<Uuid>, PromptQueue>,
    /// Threads with a prompt on its way to the agent.
    sending: HashSet<Uuid>,
    /// Threads hidden from the sidebar (kv "archived_threads"). Nothing is
    /// deleted; the group's Archived shelf lists them.
    pub archived: HashSet<Uuid>,
    login_poll: Option<Task<()>>,
    dev_poll: Option<Task<()>>,
}

impl AppModel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            projects: Vec::new(),
            workspaces: Vec::new(),
            project_status: HashMap::new(),
            project_overviews: HashMap::new(),
            overview_loading: HashSet::new(),
            overview_branch: None,
            project_intro_seen: false,
            show_all_merged: HashSet::new(),
            local_threads: Vec::new(),
            threads_loaded: false,
            local_workspaces: Vec::new(),
            local_projects: Vec::new(),
            server_lists: HashMap::new(),
            project_links: Vec::new(),
            server_preview: None,
            server_login_request: None,
            stacks: HashMap::new(),
            sync_state: HashMap::new(),
            syncing: false,
            pairing: false,
            sidebar_sort: SidebarSort::default(),
            recent_window: RecentWindow::default(),
            seen: HashMap::new(),
            seen_since: None,
            pinned_projects: Vec::new(),
            close_after_merge: HashSet::new(),
            active_workspace: None,
            source_thread: None,
            review: None,
            review_open: false,
            git_busy: false,
            review_loading: false,
            review_loading_for: None,
            active_project: None,
            thread_order: Vec::new(),
            threads: HashMap::new(),
            selected: None,
            new_thread_open: false,
            foundry_request: None,
            foundry_insert: None,
            foundry_close: false,
            foundry_show_runs: false,
            file_reveal_request: None,
            right_panel_request: None,
            browse_request: None,
            processes: Vec::new(),
            processes_folder: None,
            processes_by_folder: HashMap::new(),
            processes_scanning: false,
            processes_again: false,
            auth: Vec::new(),
            usage: Vec::new(),
            backends: Vec::new(),
            models_loading: false,
            prefs: ComposerPrefs::default(),
            dev_server: None,
            mcp_names: Vec::new(),
            login: None,
            login_starting: false,
            last_error: None,
            toasts: VecDeque::new(),
            starting: false,
            start_failure_serial: 0,
            queues: HashMap::new(),
            sending: HashSet::new(),
            archived: HashSet::new(),
            login_poll: None,
            dev_poll: None,
        };
        this.refresh_all(cx);
        this.refresh_project_status(false, cx);
        this.start_service_poll(cx);
        this
    }

    pub fn refresh_project_overview(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.active_project.clone() else { return; };
        self.refresh_sync_state(cx);
        self.refresh_stacks(cx);
        if !self.overview_loading.insert(root.clone()) { return; }
        let key = root.clone();
        let this = cx.entity().downgrade();
        let core = crate::runtime::core_for_root(cx, &root);
        spawn_service(cx, async move { core.project_overview(&root).await }, move |result, cx| {
            let _ = this.update(cx, |m, cx| {
                m.overview_loading.remove(&key);
                m.project_overviews.insert(key, result);
                cx.notify();
            });
        });
        cx.notify();
    }

    pub fn initialize_project_git(&mut self, root: String, cx: &mut Context<Self>) {
        if !self.overview_loading.insert(root.clone()) { return; }
        let key = root.clone();
        let this = cx.entity().downgrade();
        spawn_service(cx, async move {
            services::project_overview::initialize_repository(&root).await?;
            services::project_overview::load(&root, &Default::default()).await
        }, move |result, cx| {
            let _ = this.update(cx, |m, cx| {
                m.overview_loading.remove(&key);
                match result {
                    Ok(overview) => {
                        m.project_overviews.insert(key, Ok(overview));
                        m.toast(ToastKind::Success, "Git repository ready");
                        m.refresh_project_status(false, cx);
                    }
                    Err(error) => m.fail(error, cx),
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    pub fn refresh_project_status(&mut self, fetch: bool, cx: &mut Context<Self>) {
        let state = svc(cx); let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::workspaces::refresh_projects(&state, fetch).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                if let Ok(rows) = res {
                    // Replace this Mac's entries only; server projects report separately.
                    m.project_status.retain(|root, _| crate::remote::is_server_root(root));
                    m.project_status.extend(rows);
                }
                cx.notify();
            });
        });
        for root in self.projects.iter().filter(|root| crate::remote::is_server_root(root)).cloned().collect::<Vec<_>>() {
            let core = crate::runtime::core_for_root(cx, &root);
            let this = cx.entity().downgrade();
            spawn_service(cx, async move { let status = core.project_status(&root).await; (root, status) }, move |(root, status), cx| {
                let _ = this.update(cx, |m, cx| { if let Ok(status) = status { m.project_status.insert(root, status); cx.notify(); } });
            });
        }
    }

    pub fn pull_project(&mut self, root: String, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::workspaces::pull_project(root).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| { match res { Ok(note) => m.toast(ToastKind::Success, note), Err(e) => m.fail(e, cx) } m.refresh_project_status(false, cx); m.refresh_project_overview(cx); cx.notify(); });
        });
    }

    pub fn refresh_workspaces(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::workspaces::list_workspaces(&state).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                match res {
                    Ok(rows) => {
                        m.local_workspaces = rows;
                        m.combine_lists(cx);
                        if let Some(id) = m.selected {
                            m.active_workspace = m.workspaces.iter().find(|w| w.threads.contains(&id.to_string())).map(|w| w.id.clone());
                        }
                        m.refresh_review(cx);
                    }
                    Err(e) => m.fail(e, cx),
                }
                cx.notify();
            });
        });
    }

    pub fn open_workspace(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(w) = self.workspaces.iter().find(|w| w.id == id).cloned() {
            self.active_project = Some(w.project_root);
            self.select(w.threads.first().and_then(|t| Uuid::parse_str(t).ok()), cx);
            self.active_workspace = Some(id);
            if self.selected.is_none() {
                self.new_thread_open = true;
                if self.prefs.mode == "yolo" { self.prefs.mode = "plan".into(); }
            }
            self.prefs.temporary = w.inline;
            self.prefs.worktree = !w.inline;
            if w.inline { self.prefs.mode = "plan".into(); }
            self.refresh_review(cx);
            cx.notify();
        }
    }

    pub fn workspace_from_inline(&mut self, cx: &mut Context<Self>) {
        let source = self.selected.map(|id| id.to_string());
        self.new_thread(cx);
        self.source_thread = source;
        self.prefs.mode = "ask".into();
        self.toast(ToastKind::Info, "Your next message starts a thread with this conversation as context.");
        cx.notify();
    }

    pub fn new_workspace_thread(&mut self, cx: &mut Context<Self>) {
        self.new_thread_open = true;
        self.prefs.fast_mode = Some(false);
        if self.prefs.mode == "yolo" { self.prefs.mode = "plan".into(); }
        self.selected = None;
        cx.notify();
    }

    pub fn refresh_review(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active_workspace.clone() else { self.review = None; return; };
        if self.workspaces.iter().any(|w| w.id == id && w.archived_at.is_some()) { self.review = None; return; }
        // One load per workspace at a time; switching starts the new one at once.
        if self.review_loading && self.review_loading_for.as_deref() == Some(id.as_str()) {return;}
        self.review_loading = true;
        self.review_loading_for = Some(id.clone());
        let this = cx.entity().downgrade();
        let selected = id.clone();
        let core = self.core_of_workspace(&id, cx);
        spawn_service(cx, async move { core.review_workspace(id).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                // A load for a workspace the user has left: drop it, and leave the flag to the newer load.
                if m.review_loading_for.as_deref() != Some(selected.as_str()) { return; }
                m.review_loading = false;
                m.review_loading_for = None;
                if m.active_workspace.as_deref() != Some(&selected) { return; }
                match res { Ok(r) => m.review = Some(r), Err(e) => { m.review = None; m.last_error = Some(e); } }
                cx.notify();
            });
        });
    }

    pub fn run_workspace_action(&mut self, id: String, action: String, value: String, cx: &mut Context<Self>) {
        if self.git_busy {return;}
        self.git_busy=true;cx.notify();
        let core = self.core_of_workspace(&id, cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move {
            core.workspace_action(id, action, value).await
        }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                m.git_busy=false;
                match res { Ok(note) => m.toast(ToastKind::Success, note), Err(e) => m.fail(e, cx) }
                m.refresh_threads(cx);m.refresh_review(cx);
                cx.notify();
            });
        });
    }

    // ── refresh ─────────────────────────────────────────────────────────

    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        self.refresh_threads(cx);
        self.refresh_projects(cx);
        self.refresh_services(cx);
        self.refresh_usage(cx);
        self.load_archived(cx);
        self.load_sidebar_prefs(cx);
        self.refresh_backends(cx);
        self.refresh_dev_server(cx);
        self.refresh_mcp_names(cx);
    }

    pub fn refresh_mcp_names(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::list_mcp_servers(&state).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    if let Ok(list) = res {
                        m.mcp_names = list.into_iter().filter(|s| s.enabled).map(|s| s.name).collect();
                        cx.notify();
                    }
                });
            },
        );
    }

    pub fn refresh_threads(&mut self, cx: &mut Context<Self>) {
        self.refresh_workspaces(cx);
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::list_threads(&state).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| match res {
                    Ok(list) => { m.local_threads = list; m.threads_loaded = true; m.combine_lists(cx); }
                    Err(e) => m.fail(e, cx),
                });
            },
        );
        self.refresh_servers(cx);
    }

    /// Ask every paired server for its projects, threads and workspaces.
    pub fn refresh_servers(&mut self, cx: &mut Context<Self>) {
        for remote in crate::runtime::servers(cx).all() {
            let core = crate::runtime::Core::Remote(remote.clone());
            let this = cx.entity().downgrade();
            let server = remote.config.id.clone();
            spawn_service(cx, async move { Ok::<_, String>((core.list_threads().await?, core.list_workspaces().await?, core.list_projects().await?)) }, move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    // Offline: keep showing what we last knew.
                    if let Ok(lists) = res { m.server_lists.insert(server, lists); m.combine_lists(cx); }
                    cx.notify();
                });
            });
        }
    }

    // ── paired servers ──────────────────────────────────────────────────

    /// Connect to every server this Mac has been paired with.
    pub fn load_servers(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::kv_get(&state, crate::remote::SERVERS_KEY).await }, move |res, cx| {
            let saved: Vec<crate::remote::ServerConfig> = res.ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
            let _ = this.update(cx, |m, cx| { for config in saved { m.connect_server(config, cx); } });
        });
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::kv_get(&state, crate::remote::sync::LINKS_KEY).await }, move |res, cx| {
            let links = res.ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok()).unwrap_or_default();
            let _ = this.update(cx, |m, cx| { m.project_links = links; cx.notify(); });
        });
    }

    // ── moving projects between this Mac and a server ───────────────────

    fn transfer_dir() -> PathBuf {
        grok_config::paths::bomb_home().unwrap_or_else(|| PathBuf::from("/tmp/bombcode")).join("transfer")
    }

    /// The other side of a linked project, if this project has one.
    pub fn linked_project(&self, root: &str) -> Option<String> {
        self.project_links.iter().find_map(|l| if l.local == root { Some(l.server.clone()) } else if l.server == root { Some(l.local.clone()) } else { None })
    }

    fn remember_link(&mut self, local: String, server: String, cx: &mut Context<Self>) {
        self.project_links.retain(|l| l.local != local && l.server != server);
        self.project_links.push(crate::remote::sync::Link { local, server });
        let raw = serde_json::to_string(&self.project_links).unwrap_or_else(|_| "[]".into());
        let state = svc(cx);
        spawn_service(cx, async move { services::kv_set(&state, crate::remote::sync::LINKS_KEY, &raw).await }, |_, _| {});
    }

    /// Put the project in view on a server, and open it there.
    pub fn send_project_to_server(&mut self, server: String, cx: &mut Context<Self>) {
        let (Some(local), Some(remote)) = (self.active_project.clone().filter(|r| !crate::remote::is_server_root(r)), crate::runtime::servers(cx).get(&server)) else { return; };
        if self.syncing { return; }
        self.syncing = true; cx.notify();
        let this = cx.entity().downgrade();
        let name = project_name(&local);
        let source = local.clone();
        spawn_service(cx, async move { crate::remote::sync::send_to_server(&remote, &source, &name, &Self::transfer_dir()).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                m.syncing = false;
                match res {
                    Ok(root) => {
                        m.toast(ToastKind::Success, format!("{} is now on the server. Threads you start there keep running with this Mac closed.", project_name(&root)));
                        m.remember_link(local, root.clone(), cx);
                        m.refresh_servers(cx);
                        m.set_active_project(root, cx);
                    }
                    Err(e) => m.fail(e, cx),
                }
                cx.notify();
            });
        });
    }

    /// A normal copy of the server project in view, in Documents/BombCode, for working offline.
    pub fn download_project_copy(&mut self, cx: &mut Context<Self>) {
        let Some(server_root) = self.active_project.clone().filter(|r| crate::remote::is_server_root(r)) else { return; };
        let Some(remote) = crate::runtime::servers(cx).for_root(&server_root) else { return; };
        if self.syncing { return; }
        let Ok(folder) = services::default_projects_dir() else { return; };
        let target = folder.join(project_name(&server_root));
        self.syncing = true; cx.notify();
        let this = cx.entity().downgrade();
        let (source, destination) = (server_root.clone(), target.clone());
        spawn_service(cx, async move { crate::remote::sync::download_copy(&remote, &source, &destination, &Self::transfer_dir()).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                m.syncing = false;
                match res {
                    Ok(()) => {
                        m.toast(ToastKind::Success, format!("Copied to {}. Use Sync to send offline work back.", target.display()));
                        m.remember_link(target.display().to_string(), server_root, cx);
                        m.add_project(target, cx);
                    }
                    Err(e) => m.fail(e, cx),
                }
                cx.notify();
            });
        });
    }

    /// Which threads in the project in view build on other threads.
    pub fn refresh_stacks(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.active_project.clone() else { return; };
        let core = crate::runtime::core_for_root(cx, &root);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { let links = core.stack_links(&root).await; (root, links) }, move |(root, links), cx| {
            let _ = this.update(cx, |m, cx| { if let Ok(links) = links { m.stacks.insert(root, links); cx.notify(); } });
        });
    }

    /// The open thread this workspace builds on, if any.
    pub fn parent_link(&self, workspace: &str) -> Option<&bomb_core::services::workspaces::StackLink> {
        self.stacks.values().flatten().find(|l| l.child == workspace)
    }

    /// Ask which copy is ahead, for the note beside the Sync button.
    pub fn refresh_sync_state(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.active_project.clone() else { return; };
        let Some(other) = self.linked_project(&root) else { return; };
        let (local, server_root) = if crate::remote::is_server_root(&root) { (other, root.clone()) } else { (root.clone(), other) };
        let Some(remote) = crate::runtime::servers(cx).for_root(&server_root) else { return; };
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { crate::remote::sync::compare(&remote, &local, &server_root).await.map(|c| c.summary()) }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                if let Ok(summary) = res { m.sync_state.insert(root.clone(), summary); cx.notify(); }
            });
        });
    }

    /// Bring this Mac's copy and the server's copy of the project in view up to date with each other.
    pub fn sync_project(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.active_project.clone() else { return; };
        let Some(other) = self.linked_project(&root) else { return; };
        let (local, server_root) = if crate::remote::is_server_root(&root) { (other, root) } else { (root, other) };
        let Some(remote) = crate::runtime::servers(cx).for_root(&server_root) else { self.fail("That server is not paired with this Mac any more.".into(), cx); return; };
        if self.syncing { return; }
        self.syncing = true; cx.notify();
        // The branch the person is looking at, so the Mac copy lands on it afterwards.
        let branch = self.selected.and_then(|id| self.workspaces.iter().find(|w| w.threads.contains(&id.to_string()))).map(|w| w.branch.clone());
        let this = cx.entity().downgrade();
        spawn_service(cx, async move {
            // Unsaved work travels too: save a checkpoint in every idle server thread first.
            let saved: usize = remote.call("checkpoint_project", serde_json::json!({ "root": remote.path_of(&server_root), "message": "Checkpoint before sync" })).await.unwrap_or(0);
            let summary = crate::remote::sync::sync(&remote, &local, &server_root, &Self::transfer_dir()).await?;
            let landed = match &branch {
                Some(b) => crate::remote::sync::check_out_if_clean(&local, b).await,
                None => Ok(false),
            }?;
            Ok::<_, String>((saved, summary, landed, branch))
        }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                m.syncing = false;
                match res {
                    Ok((saved, summary, landed, branch)) => {
                        let mut note = summary;
                        if saved > 0 { note = format!("Saved unsaved work in {saved} server thread{} · {note}", if saved == 1 { "" } else { "s" }); }
                        if note.starts_with("Nothing new on the server · Nothing new on this Mac") {
                            note = "Nothing to sync yet: the threads have no saved work beyond what both copies already have.".into();
                        }
                        if landed { if let Some(b) = branch { note = format!("{note} · Mac copy is now on {b}"); } }
                        m.toast(ToastKind::Success, note);
                    }
                    Err(e) => m.fail(e, cx),
                }
                m.refresh_project_overview(cx); m.refresh_project_status(false, cx); m.refresh_servers(cx); m.refresh_sync_state(cx);
                cx.notify();
            });
        });
    }

    /// Clone a repository (for example from GitHub) straight onto a server.
    pub fn clone_on_server(&mut self, server: String, url: String, cx: &mut Context<Self>) {
        let Some(remote) = crate::runtime::servers(cx).get(&server) else { return; };
        if self.syncing { return; }
        self.syncing = true; cx.notify();
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { remote.call::<String>("clone_project", serde_json::json!({ "url": url })).await.map(|path| remote.root(&path)) }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                m.syncing = false;
                match res { Ok(root) => { m.refresh_servers(cx); m.set_active_project(root, cx); } Err(e) => m.fail(e, cx) }
                cx.notify();
            });
        });
    }

    fn connect_server(&mut self, config: crate::remote::ServerConfig, cx: &mut Context<Self>) {
        let inbox = cx.global::<crate::runtime::ServerInbox>();
        let (events, notices) = (inbox.events.clone(), inbox.notices.clone());
        let servers = crate::runtime::servers(cx);
        // Connection loops live on tokio.
        cx.global::<crate::runtime::Tokio>().0.spawn(async move { servers.connect(config, events, notices); });
    }

    fn save_servers(&self, cx: &mut Context<Self>) {
        let configs: Vec<_> = crate::runtime::servers(cx).all().iter().map(|s| s.config.clone()).collect();
        let raw = serde_json::to_string(&configs).unwrap_or_else(|_| "[]".into());
        let state = svc(cx);
        spawn_service(cx, async move { services::kv_set(&state, crate::remote::SERVERS_KEY, &raw).await }, |_, _| {});
    }

    /// Pair this Mac with a server from its pairing link.
    pub fn pair_server(&mut self, link: String, name: String, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        let label = std::process::Command::new("scutil").args(["--get", "ComputerName"]).output().ok()
            .and_then(|o| String::from_utf8(o.stdout).ok()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "Mac".into());
        self.pairing = true; cx.notify();
        spawn_service(cx, async move { crate::remote::pair(&link, &name, &label).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                m.pairing = false;
                match res {
                    Ok(config) => {
                        m.toast(ToastKind::Success, format!("Paired with {}", config.name));
                        m.connect_server(config.clone(), cx);
                        // The registry gains the server on tokio a moment later; save what we know now.
                        let mut configs: Vec<_> = crate::runtime::servers(cx).all().iter().map(|s| s.config.clone()).filter(|c| c.id != config.id).collect();
                        configs.push(config);
                        let raw = serde_json::to_string(&configs).unwrap_or_else(|_| "[]".into());
                        let state = svc(cx);
                        spawn_service(cx, async move { services::kv_set(&state, crate::remote::SERVERS_KEY, &raw).await }, |_, _| {});
                    }
                    Err(e) => m.fail(e, cx),
                }
                cx.notify();
            });
        });
    }

    /// Stop using a server from this Mac. The server keeps its projects; remove the device there to revoke it.
    pub fn unpair_server(&mut self, server: String, cx: &mut Context<Self>) {
        if let Some(remote) = crate::runtime::servers(cx).get(&server) {
            // Best effort: also remove this Mac from the server's device list.
            let device = remote.config.device_id.clone();
            spawn_service(cx, async move { remote.gateway("gateway.revoke_device", serde_json::json!({ "id": device })).await }, |_, _| {});
        }
        if self.server_preview.as_ref().is_some_and(|p| p.server == server) { self.server_preview = None; self.dev_server = None; }
        crate::runtime::servers(cx).remove(&server);
        self.forget_server(&server, cx);
        self.save_servers(cx);
        cx.notify();
    }

    /// Create an empty Git project in the person's projects folder on a server and open it.
    pub fn create_server_project(&mut self, server: String, name: String, cx: &mut Context<Self>) {
        let Some(remote) = crate::runtime::servers(cx).get(&server) else { return; };
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { remote.call::<String>("create_project", serde_json::json!({ "name": name })).await.map(|path| remote.root(&path)) }, move |res, cx| {
            let _ = this.update(cx, |m, cx| match res {
                Ok(root) => { m.refresh_servers(cx); m.set_active_project(root, cx); }
                Err(e) => m.fail(e, cx),
            });
        });
    }

    pub fn server_notice(&mut self, notice: crate::remote::Notice, cx: &mut Context<Self>) {
        match notice {
            crate::remote::Notice::Changed => {
                self.refresh_servers(cx);
                if self.on_server() { self.refresh_services(cx); self.refresh_backends(cx); self.refresh_project_overview(cx); }
            }
            // The server could not continue our event cursor: rebuild what is open from its saved history.
            crate::remote::Notice::Rebuild(server) => {
                let stale: Vec<Uuid> = self.threads.iter().filter(|(_, t)| {
                    let meta = &t.read(cx).meta;
                    crate::remote::split_root(meta.project_root.as_deref().unwrap_or(&meta.cwd)).is_some_and(|(id, _)| id == server)
                }).map(|(id, _)| *id).collect();
                for id in stale {
                    if let Some(t) = self.threads.get(&id) {
                        t.update(cx, |t, cx| { t.thread = bomb_core::transcript::Thread::new(); t.hydrated = false; t.markdown.clear(); cx.notify(); });
                    }
                    if self.selected == Some(id) { self.hydrate(id, cx); }
                }
            }
        }
        cx.notify();
    }

    /// Forget a server's projects and threads (after unpairing).
    pub fn forget_server(&mut self, server: &str, cx: &mut Context<Self>) {
        self.server_lists.remove(server);
        if self.active_project.as_deref().is_some_and(|root| crate::remote::split_root(root).is_some_and(|(id, _)| id == server)) {
            self.active_project = None;
            self.selected = None;
        }
        self.combine_lists(cx);
    }

    /// Rebuild the combined project, thread and workspace lists from this Mac and every server.
    fn combine_lists(&mut self, cx: &mut Context<Self>) {
        let mut threads = self.local_threads.clone();
        let mut workspaces = self.local_workspaces.clone();
        let mut projects = self.local_projects.clone();
        for (t, w, p) in self.server_lists.values() {
            threads.extend(t.iter().cloned());
            workspaces.extend(w.iter().cloned());
            projects.extend(p.iter().cloned());
        }
        self.workspaces = workspaces;
        self.projects = projects;
        // Wait for this Mac's thread list: "most recent" needs it.
        if self.active_project.is_none() && self.threads_loaded { self.active_project = startup_project(&threads, &self.projects); }
        self.set_threads(threads, cx);
    }

    /// The core that owns a thread, from the project folder it belongs to.
    pub fn core_of_thread(&self, id: Uuid, cx: &App) -> crate::runtime::Core {
        let root = self.threads.get(&id).map(|t| { let meta = &t.read(cx).meta; meta.project_root.clone().unwrap_or_else(|| meta.cwd.clone()) }).unwrap_or_default();
        crate::runtime::core_for_root(cx, &root)
    }

    pub fn core_of_workspace(&self, workspace: &str, cx: &App) -> crate::runtime::Core {
        let root = self.workspaces.iter().find(|w| w.id == workspace).map(|w| w.project_root.clone()).unwrap_or_default();
        crate::runtime::core_for_root(cx, &root)
    }

    /// True when the project in view lives on a paired server.
    pub fn on_server(&self) -> bool {
        self.active_project.as_deref().is_some_and(crate::remote::is_server_root)
    }

    pub fn refresh_projects(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::list_projects(&state).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    if let Ok(list) = res {
                        m.local_projects = list;
                        m.combine_lists(cx);
                        cx.notify();
                    }
                });
            },
        );
    }

    /// Copy Claude Code CLI conversations in as saved threads (safe to repeat).
    pub fn import_claude_sessions(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        self.run_import("Claude Code", async move { services::claude_import::import_claude_code_sessions(&state).await }, cx);
    }

    /// Copy Codex app and CLI conversations in as saved threads (safe to repeat).
    pub fn import_codex_sessions(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        self.run_import("Codex", async move { services::codex_import::import_codex_sessions(&state).await }, cx);
    }

    fn run_import(
        &mut self,
        source: &'static str,
        work: impl std::future::Future<Output = Result<services::claude_import::ImportReport, String>> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.toast(ToastKind::Info, format!("Importing {source} conversations…"));
        cx.notify();
        let this = cx.entity().downgrade();
        spawn_service(cx, work, move |res, cx| {
            let _ = this.update(cx, |m, cx| match res {
                Ok(r) => {
                    let mut msg = match r.imported {
                        0 => format!("No new {source} conversations to import"),
                        1 => format!("Imported 1 {source} conversation"),
                        n => format!("Imported {n} {source} conversations"),
                    };
                    if r.already_here > 0 {
                        msg.push_str(&format!(" · {} already here", r.already_here));
                    }
                    if r.skipped + r.failed > 0 {
                        msg.push_str(&format!(" · {} skipped (folder gone or empty)", r.skipped + r.failed));
                    }
                    m.toast(ToastKind::Success, msg);
                    m.refresh_threads(cx);
                }
                Err(e) => m.fail(format!("Import failed: {e}"), cx),
            });
        });
    }

    pub fn refresh_services(&mut self, cx: &mut Context<Self>) {
        let core = crate::runtime::core_for_root(cx, self.active_project.as_deref().unwrap_or_default());
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { core.backend_auth_status().await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    if let Ok(list) = res {
                        let auth_changed = list.iter().any(|a| m.auth.iter().find(|old| old.backend == a.backend).is_some_and(|old| old.logged_in != a.logged_in));
                        m.auth = list;
                        if auth_changed { m.refresh_backends(cx); }
                        cx.notify();
                    }
                });
            },
        );
    }

    pub fn refresh_backends(&mut self, cx: &mut Context<Self>) {
        if self.models_loading { return; }
        self.models_loading = true;
        cx.notify();
        let core = crate::runtime::core_for_root(cx, self.active_project.as_deref().unwrap_or_default());
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { core.list_backends().await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    m.models_loading = false;
                    if let Ok(list) = res {
                        // Keep the preferred backend runnable.
                        if !list.iter().any(|b| b.id == m.prefs.backend && b.available) {
                            if let Some(b) = list.iter().find(|b| b.available) {
                                m.prefs.backend = b.id.clone();
                                m.prefs.model = None;
                            }
                        }
                        m.backends = list;
                    }
                    cx.notify();
                });
            },
        );
    }

    pub fn refresh_dev_server(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::dev_server_status(&state).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    if let Ok(s) = res {
                        if m.server_preview.is_some() { return; }
                        let running = s.running;
                        m.dev_server = Some(s);
                        if running {
                            m.ensure_dev_poll(cx);
                        }
                        cx.notify();
                    }
                });
            },
        );
    }

    /// Services change rarely; a slow poll keeps the footer honest after a
    /// sign-in that happened in a terminal.
    fn start_service_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |weak, cx| {
            let mut tick: u32 = 0;
            loop {
                cx.background_executor().timer(Duration::from_secs(20)).await;
                tick += 1;
                // Usage limits move slowly and hit vendor APIs: every 2 minutes.
                let usage = tick.is_multiple_of(6);
                if weak
                    .update(cx, |m, cx| {
                        m.refresh_services(cx);
                        if usage {
                            m.refresh_usage(cx);
                            m.refresh_review(cx);
                        }
                        if tick.is_multiple_of(15) { m.refresh_project_status(true, cx); }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn refresh_usage(&mut self, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        spawn_service(cx, services::account_usage(), move |list, cx| {
            let _ = this.update(cx, |m, cx| {
                if m.usage != list {
                    m.usage = list;
                    cx.notify();
                }
            });
        });
    }

    pub fn usage_for(&self, backend: &str) -> Option<&bomb_core::usage::AccountUsage> {
        self.usage.iter().find(|u| u.backend == backend)
    }

    fn ensure_dev_poll(&mut self, cx: &mut Context<Self>) {
        if self.dev_poll.is_some() {
            return;
        }
        self.dev_poll = Some(cx.spawn(async move |weak, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let state = cx.update(|cx| svc(cx));
                let status = {
                    let (tx, rx) = async_channel::bounded(1);
                    let handle = cx.update(|cx| cx.global::<crate::runtime::Tokio>().0.clone());
                    handle.spawn(async move {
                        let _ = tx.send(services::dev_server_status(&state).await).await;
                    });
                    rx.recv().await
                };
                let keep = match status {
                    Ok(Ok(s)) => {
                        let running = s.running;
                        weak.update(cx, |m, cx| {
                            if m.server_preview.is_some() { return; }
                            m.dev_server = Some(s);
                            if !running {
                                m.dev_poll = None;
                            }
                            cx.notify();
                        })
                        .map(|_| running)
                        .unwrap_or(false)
                    }
                    _ => false,
                };
                if !keep {
                    break;
                }
            }
        }));
    }

    pub(crate) fn set_threads(&mut self, list: Vec<ThreadDto>, cx: &mut Context<Self>) {
        let mut order = Vec::with_capacity(list.len());
        for dto in list {
            let Ok(id) = Uuid::parse_str(&dto.id) else {
                continue;
            };
            order.push(id);
            match self.threads.get(&id) {
                Some(entity) => entity.update(cx, |t, cx| {
                    t.meta = dto;
                    cx.notify();
                }),
                None => {
                    let entity = cx.new(|_| ThreadModel::new(dto));
                    self.threads.insert(id, entity);
                }
            }
        }
        self.threads.retain(|id, _| order.contains(id));
        self.thread_order = order;
        if let Some(open) = self.selected { self.seen.insert(open, chrono::Utc::now()); }
        if let Some(sel) = self.selected {
            if !self.threads.contains_key(&sel) {
                self.selected = None;
            }
        }
        cx.notify();
    }

    fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        tracing::warn!(%message, "backend call failed");
        self.last_error = Some(message.clone());
        self.toast(ToastKind::Error, message);
        cx.notify();
    }

    pub fn toast(&mut self, kind: ToastKind, message: impl Into<String>) {
        self.toasts.push_back((kind, message.into()));
    }

    // ── queries ─────────────────────────────────────────────────────────

    /// Threads grouped by project root, in list order.
    /// The sidebar entry a folder belongs to: a Mac copy of a server project lists under that project.
    pub fn sidebar_root(&self, root: &str) -> String {
        self.project_links.iter().find(|l| l.local == root).map(|l| l.server.clone()).unwrap_or_else(|| root.to_string())
    }

    pub fn groups(&self, cx: &App) -> Vec<ProjectGroup> {
        let mut groups: Vec<ProjectGroup> = Vec::new();
        for id in &self.thread_order {
            let Some(t) = self.threads.get(id) else { continue };
            let meta = &t.read(cx).meta;
            let root = self.sidebar_root(&meta
                .project_root
                .clone()
                .unwrap_or_else(|| meta.cwd.clone()));
            match groups.iter_mut().find(|g| g.root == root) {
                Some(g) => g.threads.push(*id),
                None => groups.push(ProjectGroup {
                    name: project_name(&root),
                    root,
                    threads: vec![*id],
                }),
            }
        }
        for p in self.projects.iter().filter(|p| self.sidebar_root(p) == **p) {
            if !groups.iter().any(|g| &g.root == p) {
                groups.push(ProjectGroup {
                    name: project_name(p),
                    root: p.clone(),
                    threads: Vec::new(),
                });
            }
        }
        // Threads first, then projects by their most relevant thread.
        let sort = self.sidebar_sort;
        let key = |id: &Uuid| {
            let t = self.threads.get(id).map(|t| t.read(cx));
            SortKey {
                name: t.map(|t| t.title()).unwrap_or_default(),
                updated: t.map(|t| t.meta.updated_at.clone()).unwrap_or_default(),
                created: t.map(|t| t.meta.created_at.clone()).unwrap_or_default(),
            }
        };
        let mut keyed: Vec<(SortKey, ProjectGroup)> = groups
            .into_iter()
            .map(|mut g| {
                let mut rows: Vec<(SortKey, Uuid)> = g.threads.iter().map(|id| (key(id), *id)).collect();
                sort_rows(&mut rows, sort);
                let project = SortKey {
                    name: g.name.clone(),
                    updated: rows.iter().map(|(k, _)| k.updated.clone()).max().unwrap_or_default(),
                    created: rows.iter().map(|(k, _)| k.created.clone()).max().unwrap_or_default(),
                };
                g.threads = rows.into_iter().map(|(_, id)| id).collect();
                (project, g)
            })
            .collect();
        sort_rows(&mut keyed, sort);
        keyed.into_iter().map(|(_, g)| g).collect()
    }

    pub fn selected_thread(&self) -> Option<Entity<ThreadModel>> {
        self.selected.and_then(|id| self.threads.get(&id).cloned())
    }

    pub fn backend_info(&self, id: &str) -> Option<&BackendInfo> {
        self.backends.iter().find(|b| b.id == id)
    }

    /// Model shown in the composer: explicit choice, else the backend default.
    pub fn effective_model(&self) -> String {
        self.prefs
            .model
            .clone()
            .or_else(|| self.backend_info(&self.prefs.backend).map(|b| b.default_model.clone()))
            .unwrap_or_default()
    }

    pub fn model_ready(&self) -> bool {
        let selected = self.effective_model();
        self.backend_info(&self.prefs.backend).is_some_and(|backend| backend.models.contains(&selected))
    }

    pub fn model_name(&self, backend: &str, model: &str) -> String {
        self.backend_info(backend).and_then(|b|b.model_names.get(model)).cloned()
            .unwrap_or_else(|| crate::views::brand::pretty_model(model))
    }

    // ── selection ───────────────────────────────────────────────────────

    pub fn select(&mut self, id: Option<Uuid>, cx: &mut Context<Self>) {
        if self.selected == id {
            return;
        }
        // Leaving a thread counts as having seen what it did while it was open.
        for seen in self.selected.into_iter().chain(id) { self.mark_seen(seen, cx); }
        self.selected = id;
        self.prefs.fast_mode = Some(false);
        self.new_thread_open = false;
        self.review = None;
        self.review_loading=false;
        self.active_workspace = id.and_then(|id| self.workspaces.iter().find(|w| w.threads.contains(&id.to_string())).map(|w| w.id.clone()));
        if let Some(w) = self.active_workspace.as_deref().and_then(|id| self.workspaces.iter().find(|w| w.id == id)) {
            self.prefs.temporary = w.inline;
            self.prefs.worktree = !w.inline;
        } else if id.is_some() {
            // A thread with no workspace (imported, or from before workspaces) works in its
            // own folder; don't carry over the question-only state of the last conversation.
            self.prefs.temporary = false;
            self.prefs.worktree = false;
        }
        self.refresh_review(cx);
        if let Some(id) = id {
            if let Some(t) = self.threads.get(&id) {
                let meta = t.read(cx).meta.clone();
                self.active_project = Some(meta.project_root.clone().unwrap_or(meta.cwd.clone()));
                // The composer follows the thread's own backend/model/mode,
                // but never a model id the backend no longer offers.
                self.prefs.backend = meta.backend.clone();
                let known = self
                    .backend_info(&meta.backend)
                    .map(|b| b.models.contains(&meta.model) || b.default_model == meta.model)
                    .unwrap_or(false);
                self.prefs.model = if meta.model.is_empty() || !known { None } else { Some(meta.model.clone()) };
                if let Some(mode) = meta.approval_mode.clone() {
                    self.prefs.mode = mode;
                }
            }
            self.hydrate(id, cx);
            let effort_state=svc(cx);
            let weak=cx.entity().downgrade();
            let expected=(self.prefs.backend.clone(),self.prefs.model.clone(),self.prefs.effort.clone());
            spawn_service(cx,async move {
                effort_state.registry.current_effort(id).await.or_else(||effort_state.persistence.get_kv(&format!("session-effort/{id}")).ok().flatten())
            },move |effort,cx|{let _=weak.update(cx,|m,cx|{
                if m.selected==Some(id) && (m.prefs.backend.clone(),m.prefs.model.clone(),m.prefs.effort.clone())==expected {
                    if let Some(effort)=effort {m.prefs.effort=effort;cx.notify();}
                }
            });});
            let state = svc(cx);
            spawn_service(
                cx,
                async move { services::explainer_focus(&state, Some(id.to_string())).await },
                |_, _| {},
            );
        }
        cx.notify();
    }

    /// Deselect: the composer starts a fresh thread in the active project.
    pub fn new_thread(&mut self, cx: &mut Context<Self>) {
        self.prefs.fast_mode = Some(false);
        self.new_thread_open = true;
        if self.prefs.mode == "yolo" { self.prefs.mode = "plan".into(); }
        self.source_thread = None;
        self.active_workspace = None;
        self.prefs.temporary = false;
        self.prefs.worktree = true;
        self.prefs.location="new".into();
        self.prefs.base_branch=None;self.prefs.existing_branch=None;self.prefs.read_only=false;
        self.selected = None;
        // Fresh thread → backend default model, never a stale id.
        self.prefs.model = None;
        cx.notify();
    }

    /// A new thread that starts from another thread's branch instead of main.
    pub fn new_thread_from(&mut self, branch: String, cx: &mut Context<Self>) {
        self.new_thread(cx);
        self.prefs.base_branch = Some(branch);
        cx.notify();
    }

    /// A folder-free chat still needs a private working directory for tools.
    pub fn temporary_chat(&mut self, cx: &mut Context<Self>) {
        let Some(home) = std::env::var_os("HOME") else { return; };
        let base = std::path::PathBuf::from(home).join(".bombcode/chats");
        let weak = cx.entity().downgrade();
        spawn_service(cx, async move {
            services::scratch::create(&base).await
        }, move |result, cx| {
            let _ = weak.update(cx, |m, cx| match result {
                Ok(path) => {
                    m.new_thread(cx);
                    m.active_project = Some(path.to_string_lossy().into_owned());
                    m.prefs.temporary = false;
                    m.prefs.mode = "plan".into();
                    m.prefs.worktree = true;
                    cx.notify();
                }
                Err(error) => { m.toast(ToastKind::Error, format!("Could not start temporary chat: {error}")); cx.notify(); }
            });
        });
    }

    /// Return to the welcome screen without removing projects or conversations.
    pub fn open_home(&mut self, cx: &mut Context<Self>) {
        self.new_thread(cx);
        self.active_project = None;
        self.review = None;
        self.review_open = false;
    }

    fn hydrate(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let Some(entity) = self.threads.get(&id).cloned() else {
            return;
        };
        let needs = {
            let t = entity.read(cx);
            !t.hydrated && !t.loading
        };
        if !needs {
            return;
        }
        // A thread that already streamed in this session is the truth; the
        // saved copy can only be older. Never replace live entries with it.
        if !entity.read(cx).thread.entries.is_empty() {
            entity.update(cx, |t, _| t.hydrated = true);
            return;
        }
        entity.update(cx, |t, _| t.loading = true);
        let core = self.core_of_thread(id, cx);
        let remote = core.is_remote();
        let weak = entity.downgrade();
        spawn_service(
            cx,
            async move { core.snapshot(id.to_string()).await },
            move |res, cx| {
                let _ = weak.update(cx, |t, cx| {
                    t.loading = false;
                    t.hydrated = true;
                    match res {
                        Ok(snapshot) => {
                            let rows = snapshot.rows;
                            tracing::debug!(%id, rows = rows.len(), live = t.thread.entries.len(), "thread hydrated");
                            if t.thread.entries.is_empty() {
                                t.thread.hydrate(&rows);
                                t.after_hydrate(cx);
                                // Saved rows drop an approval's id and choices; a server thread may be
                                // waiting on one that was asked before this Mac connected.
                                if remote { for approval in &snapshot.pending_approvals { t.apply(approval, cx); } }
                                // The agent may still be working there: say so now, not at its next event.
                                if remote && (t.meta.status == "running" || t.meta.status.contains("wait")) {
                                    let ch = t.thread.resume_in_progress(std::time::Instant::now());
                                    t.absorb(&ch, cx);
                                }
                            }
                        }
                        Err(e) => tracing::warn!(%id, error = %e, "transcript load failed"),
                    }
                    cx.notify();
                });
            },
        );
    }

    // ── events ──────────────────────────────────────────────────────────

    pub fn apply_events(&mut self, batch: Vec<ControlEvent>, cx: &mut Context<Self>) {
        let mut need_refresh = false;
        let mut ended = Vec::new();
        let mut touched = HashSet::new();
        for ev in batch {
            match &ev {
                ControlEvent::SessionCreated { session_id, .. } => {
                    if !self.threads.contains_key(session_id) {
                        need_refresh = true;
                    }
                }
                ControlEvent::SessionStatusChanged { session_id, status, .. } => {
                    need_refresh = true;
                    if matches!(status, grok_events::SessionStatus::Completed | grok_events::SessionStatus::Idle) { ended.push(*session_id); }
                    if matches!(status, grok_events::SessionStatus::Cancelled | grok_events::SessionStatus::Failed) {
                        // A stopped or failed merge must not close anything.
                        if let Some(w) = self.workspaces.iter().find(|w| w.threads.contains(&session_id.to_string())) { let id = w.id.clone(); self.close_after_merge.remove(&id); }
                    }
                }
                ControlEvent::SessionCancelled { .. }
                | ControlEvent::SessionCompleted { .. } => need_refresh = true,
                ControlEvent::Error {
                    session_id: None,
                    message,
                    ..
                } => {
                    self.last_error = Some(message.clone());
                    self.toast(ToastKind::Error, message.clone());
                    cx.notify();
                }
                ControlEvent::McpChanged { .. } => self.refresh_mcp_names(cx),
                ControlEvent::Raw { payload, .. }
                    if matches!(payload.get("channel").and_then(|c| c.as_str()), Some("provider_commands" | "foundry")) =>
                {
                    // The composer observes AppModel; refresh its advertised command menu.
                    cx.notify();
                }
                ControlEvent::Raw { payload, session_id: Some(id) }
                    if payload.get("channel").and_then(|c| c.as_str()) == Some("thread") =>
                {
                    if self.selected == Some(*id) && payload.get("kind").and_then(|v| v.as_str()) == Some("model_switch") {
                        if payload.get("backend").and_then(|v|v.as_str()) == Some(self.prefs.backend.as_str()) {
                            if let Some(model) = payload.get("model").and_then(|v|v.as_str()) { self.prefs.model = Some(model.to_owned()); }
                        }
                        if let Some(effort)=payload.get("effort").and_then(|v|v.as_str()) {
                            self.prefs.effort=effort.to_owned();
                            cx.notify();
                        }
                    }
                    need_refresh = true;
                }
                _ => {}
            }
            if let Some(sid) = session_of(&ev) {
                if let Some(t) = self.threads.get(&sid) {
                    t.update(cx, |t, cx| {
                        t.apply(&ev, cx);
                    });
                } else {
                    debug!(%sid, "event for unknown thread");
                    need_refresh = true;
                }
                if let Some(end) = TurnEnd::of(&ev) { self.queues.entry(Some(sid)).or_default().on_turn_end(end); }
                touched.insert(sid);
            }
        }
        if need_refresh {
            self.refresh_threads(cx);
        }
        for session in ended { self.turn_ended(session, cx); }
        // The agent may be free now: send what was typed while it worked.
        for session in touched { self.pump_queue(Some(session), cx); }
    }

    // ── prompts / sessions ──────────────────────────────────────────────

    /// Files attached to a message. On this Mac the agent opens them where they are; for a project on a
    /// server they are uploaded first and the message names their paths there.
    pub fn send_prompt_with_files(&mut self, text: String, images: Vec<ImageInput>, files: Vec<(PathBuf, u64)>, cx: &mut Context<Self>) {
        let note = with_files_note;
        let remote = self.active_project.as_deref().and_then(|root| crate::runtime::servers(cx).for_root(root));
        let (Some(remote), false) = (remote, files.is_empty()) else {
            let text = note(&files, &text);
            return self.send_prompt(text, images, cx);
        };
        self.starting = true; cx.notify();
        let this = cx.entity().downgrade();
        spawn_service(cx, upload_files(remote, files), move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                m.starting = false;
                match res {
                    Ok(placed) => { let text = note(&placed, &text); m.send_prompt(text, images, cx); }
                    Err(e) => {
                        // Same as a failed start: the composer keeps the unsent prompt.
                        m.start_failure_serial += 1;
                        m.fail(format!("Could not send the attached files to the server: {e}"), cx);
                    }
                }
            });
        });
    }

    /// Send to the selected thread, or start a new one in the active project.
    pub fn send_prompt(&mut self, text: String, images: Vec<ImageInput>, cx: &mut Context<Self>) {
        if std::env::var("BOMB_SMOKE").ok().as_deref() != Some("1") && !self.model_ready() {
            self.toast(ToastKind::Warning, "Choose a model from the provider's current list. Open the model picker and refresh if needed.");
            cx.notify(); return;
        }
        if self.active_workspace.as_deref().is_some_and(|id| self.workspaces.iter().any(|w| w.id == id && w.archived_at.is_some())) {
            self.fail("This thread is archived. Start a new thread to make changes.".into(), cx); return;
        }
        let mut prefs = self.prefs.clone();
        let model = self.effective_model();
        prefs.model = (!model.is_empty()).then_some(model);
        let attachments: Vec<ImageAttachment> = images
            .iter()
            .map(|i| ImageAttachment {
                mime_type: i.mime_type.clone(),
                data: i.data.clone(),
                name: i.name.clone(),
            })
            .collect();
        match self.selected_thread() {
            Some(t) => {
                let id = Uuid::parse_str(&t.read(cx).id()).unwrap_or_default();
                self.send_to_thread(id, vec![(text.clone(), attachments)], text, images, Some(prefs), false, cx);
            }
            None => {
                let Some(cwd) = self.active_project.clone() else {
                    self.toast(ToastKind::Warning, "Open a project first (⌘O).");
                    cx.notify();
                    return;
                };
                self.starting = true;
                cx.notify();
                let this = cx.entity().downgrade();
                let backend = grok_config::Backend::from_key(&prefs.backend).unwrap_or_default();
                let approval_mode = match prefs.mode.as_str() {
                    "plan" => Some(ApprovalMode::Plan),
                    "ask" => Some(ApprovalMode::Ask),
                    "auto" => Some(ApprovalMode::Auto),
                    "yolo" => Some(ApprovalMode::Yolo),
                    _ => None,
                };
                // Spawn with the model the composer shows, never the stale
                // config default ("grok-4").
                let model = Some(self.effective_model()).filter(|m| !m.trim().is_empty());
                let opts = SpawnOptions {
                    backend,
                    model: model.clone(),
                    approval_mode,
                    isolate_worktree: prefs.location=="new" && !prefs.temporary,
                    edit_checkout: prefs.location=="checkout" && !prefs.temporary,
                    checkout_branch: if prefs.location=="branch"{prefs.existing_branch.clone()}else{None},
                    base_ref: prefs.base_branch.clone(),
                    read_only: prefs.read_only,
                    workspace_id: self.active_workspace.clone(),
                    source_thread: self.source_thread.take(),
                    prompt: Some(text.clone()),
                    project_root: Some(cwd.clone()),
                    mcp_server_names: prefs.mcp_servers.clone(),
                    effort: Some(prefs.effort.clone()),
                    ..Default::default()
                };
                // Two steps so the thread is selected (and the prompt visible)
                // the moment it exists, even if the send then fails.
                let core = crate::runtime::core_for_root(cx, &cwd);
                let core2 = core.clone();
                let text2 = text.clone();
                let images2 = images.clone();
                let mut prefs2 = prefs.clone();
                prefs2.model = model;
                spawn_service(
                    cx,
                    async move { core.start_session(cwd, opts).await.map(|id| services::SessionIdResponse { id }) },
                    move |res, cx| {
                        let _ = this.update(cx, |m, cx| match res {
                            Ok(started) => {
                                if let Ok(id) = Uuid::parse_str(&started.id) {
                                    m.selected = Some(id);
                                    // What was typed while it started now waits on the thread itself.
                                    if let Some(q) = m.queues.remove(&None) { m.queues.insert(Some(id), q); }
                                    // Started on another thread's branch: say what that means once, up front.
                                    if let Some(parent) = prefs2.base_branch.as_deref().and_then(|b| m.workspaces.iter().find(|w| w.branch == b && w.archived_at.is_none() && !w.inline)) {
                                        m.toast(ToastKind::Info, format!("This thread builds on “{}”. Merge that one first; this one is brought up to date automatically and merges after it.", parent.name));
                                    }
                                    if !m.threads.contains_key(&id) {
                                        let dto = ThreadDto {
                                            models_used: Vec::new(),
                                            id: started.id.clone(),
                                            cwd: String::new(),
                                            mode: "acp".into(),
                                            model: prefs2.model.clone().unwrap_or_default(),
                                            backend: prefs2.backend.clone(),
                                            status: "starting".into(),
                                            live: true,
                                            message_count: 0,
                                            created_at: chrono::Utc::now().to_rfc3339(),
                                            updated_at: chrono::Utc::now().to_rfc3339(),
                                            worktree: None,
                                            mcp_servers: Vec::new(),
                                            label: None,
                                            approval_mode: Some(prefs2.mode.clone()),
                                            project_root: m.active_project.clone(),
                                            brain_mode: None,
                                        };
                                        let entity = cx.new(|_| ThreadModel::new(dto));
                                        entity.update(cx, |t, _| { t.hydrated = true; });
                                        m.threads.insert(id, entity);
                                        m.thread_order.insert(0, id);
                                    }
                                    if let Some(t) = m.threads.get(&id) {
                                        let atts: Vec<ImageAttachment> = images2
                                            .iter()
                                            .map(|i| ImageAttachment { mime_type: i.mime_type.clone(), data: i.data.clone(), name: i.name.clone() })
                                            .collect();
                                        t.update(cx, |t, cx| {
                                            let ch = t.thread.note_prompt(&text2, atts, std::time::Instant::now());
                                            t.absorb(&ch, cx);
                                        });
                                    }
                                    cx.notify();
                                    let sid = started.id.clone();
                                    let weak = m.threads.get(&id).map(|t| t.downgrade());
                                    let this = cx.entity().downgrade();
                                    spawn_service(
                                        cx,
                                        async move {
                                            // The ACP handshake is still in flight right
                                            // after start_session; sending now is refused.
                                            core2.wait_until_idle(&sid, std::time::Duration::from_secs(90)).await?;
                                            core2.send_prompt(
                                                sid,
                                                text2,
                                                Some(prefs2.backend),
                                                prefs2.model,
                                                Some(prefs2.mode),
                                                None,
                                                None,
                                                Some(images2),
                                                prefs2.fast_mode,
                                                Some(prefs2.effort),
                                            )
                                            .await
                                        },
                                        move |res, cx| {
                                            let _ = this.update(cx, |m, cx| {
                                                m.starting = false;
                                                if let Err(e) = res {
                                                    if let Some(w) = &weak {
                                                        let _ = w.update(cx, |t, cx| {
                                                            let ch = t.thread.note_failure(&format!("send failed: {e}"), std::time::Instant::now());
                                                            t.absorb(&ch, cx);
                                                        });
                                                    }
                                                    if let Some(q) = m.queues.get_mut(&Some(id)) { q.on_turn_end(TurnEnd::Failed); }
                                                    m.fail(e, cx);
                                                }
                                                m.refresh_threads(cx);
                                                m.pump_queue(Some(id), cx);
                                            });
                                        },
                                    );
                                } else {
                                    m.starting = false;
                                    if let Some(q) = m.queues.get_mut(&None) { q.on_turn_end(TurnEnd::Failed); }
                                    m.refresh_threads(cx);
                                }
                            }
                            Err(e) => {
                                m.starting = false;
                                m.start_failure_serial += 1;
                                if let Some(q) = m.queues.get_mut(&None) { q.on_turn_end(TurnEnd::Failed); }
                                m.fail(e, cx);
                            }
                        });
                    },
                );
            }
        }
    }

    /// Send to a thread that exists: one bubble per message, one prompt to the agent.
    /// `prefs` carries the composer's picks for the thread in view; a thread in the
    /// background keeps its own. `settle` first waits for the last turn to wind down.
    #[allow(clippy::too_many_arguments)]
    fn send_to_thread(&mut self, id: Uuid, bubbles: Vec<(String, Vec<ImageAttachment>)>, text: String, images: Vec<ImageInput>, prefs: Option<ComposerPrefs>, settle: bool, cx: &mut Context<Self>) {
        let Some(t) = self.threads.get(&id).cloned() else { return };
        t.update(cx, |t, cx| {
            for (text, attachments) in bubbles {
                let ch = t.thread.note_prompt(&text, attachments, std::time::Instant::now());
                t.absorb(&ch, cx);
            }
        });
        if let Some(q) = self.queues.get_mut(&Some(id)) { q.turn_started(); }
        self.sending.insert(id);
        let core = self.core_of_thread(id, cx);
        let this = cx.entity().downgrade();
        let weak = t.downgrade();
        let (backend, model, mode, fast_mode, effort) = match prefs {
            Some(p) => (Some(p.backend), p.model, Some(p.mode), p.fast_mode, Some(p.effort)),
            None => (None, None, None, None, None),
        };
        spawn_service(
            cx,
            async move {
                let sid = id.to_string();
                // A server too old to know this call just sends straight away.
                if settle { let _ = core.wait_turn_settled(&sid, SETTLE_TIMEOUT).await; }
                core.send_prompt(sid, text, backend, model, mode, None, None, Some(images), fast_mode, effort).await
            },
            move |res, cx| {
                if let Err(e) = &res {
                    let _ = weak.update(cx, |t, cx| {
                        let ch = t.thread.note_failure(&format!("send failed: {e}"), std::time::Instant::now());
                        t.absorb(&ch, cx);
                    });
                }
                let _ = this.update(cx, |m, cx| {
                    m.sending.remove(&id);
                    if let Err(e) = res {
                        if let Some(q) = m.queues.get_mut(&Some(id)) { q.on_turn_end(TurnEnd::Failed); }
                        m.fail(e, cx);
                    }
                    m.pump_queue(Some(id), cx);
                });
            },
        );
    }

    // ── type-ahead queue ────────────────────────────────────────────────

    /// A thread's agent is mid-turn, or about to be.
    fn thread_busy(&self, id: Uuid, cx: &App) -> bool {
        self.sending.contains(&id)
            || (self.starting && self.selected == Some(id))
            || self.threads.get(&id).is_some_and(|t| t.read(cx).thread.presence.turn_active())
    }

    /// The composer's next message waits in the queue: the thread in view is working or still starting.
    pub fn queue_instead(&self, cx: &App) -> bool {
        match self.selected {
            Some(id) => self.thread_busy(id, cx),
            None => self.starting,
        }
    }

    /// The messages waiting on a thread (`None`: the thread still being started).
    pub fn queue(&self, thread: Option<Uuid>) -> Option<&PromptQueue> {
        self.queues.get(&thread).filter(|q| !q.is_empty())
    }

    /// Hold a message for the thread in view until its agent is free.
    pub fn enqueue(&mut self, text: String, images: Vec<ImageInput>, files: Vec<(PathBuf, u64)>, cx: &mut Context<Self>) {
        let images = images.into_iter().map(|i| ImageAttachment { mime_type: i.mime_type, data: i.data, name: i.name }).collect();
        self.queues.entry(self.selected).or_default().push(text, images, files);
        cx.notify();
    }

    /// Open a queued message for editing; returns its text. It is held back until saved.
    pub fn begin_queued_edit(&mut self, thread: Option<Uuid>, item: u64, cx: &mut Context<Self>) -> Option<String> {
        let text = self.queues.get_mut(&thread)?.begin_edit(item).map(|m| m.text.clone());
        cx.notify();
        text
    }

    pub fn save_queued_edit(&mut self, thread: Option<Uuid>, item: u64, text: String, cx: &mut Context<Self>) {
        if let Some(q) = self.queues.get_mut(&thread) { q.edit(item, text); }
        cx.notify();
        self.pump_queue(thread, cx);
    }

    pub fn cancel_queued_edit(&mut self, thread: Option<Uuid>, cx: &mut Context<Self>) {
        if let Some(q) = self.queues.get_mut(&thread) { q.cancel_edit(); }
        cx.notify();
        self.pump_queue(thread, cx);
    }

    pub fn remove_queued(&mut self, thread: Option<Uuid>, item: u64, cx: &mut Context<Self>) {
        if let Some(q) = self.queues.get_mut(&thread) { q.remove(item); }
        cx.notify();
    }

    /// "Send now": stop the current turn, wait until the agent confirms it has
    /// ended, then send this message on its own. The rest wait for that turn.
    pub fn send_queued_now(&mut self, thread: Option<Uuid>, item: u64, cx: &mut Context<Self>) {
        // A thread that is still starting has no turn to stop yet.
        let Some(id) = thread else { return };
        if !self.thread_busy(id, cx) {
            let Some(message) = self.queues.get_mut(&thread).and_then(|q| q.remove(item)) else { return };
            self.send_batch(id, Batch { messages: vec![message] }, cx);
            return;
        }
        if self.starting && self.selected == Some(id) {
            self.toast(ToastKind::Info, "The agent is still starting. This message goes out after its first reply.");
            cx.notify(); return;
        }
        if !self.queues.get_mut(&thread).is_some_and(|q| q.send_now(item)) { return; }
        cx.notify();
        let core = self.core_of_thread(id, cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move {
            let sid = id.to_string();
            core.cancel_session(sid.clone()).await?;
            // Stop returns before the agent has wound down; the new prompt must not race the old turn.
            let _ = core.wait_turn_settled(&sid, SETTLE_TIMEOUT).await;
            Ok::<_, String>(())
        }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                let quiet = !m.threads.get(&id).is_some_and(|t| t.read(cx).thread.presence.turn_active());
                let Some(q) = m.queues.get_mut(&Some(id)) else { return };
                match res {
                    Ok(()) => {
                        q.send_now_settled();
                        // The thread already shows the turn over: the stop got there.
                        if quiet { q.on_turn_end(TurnEnd::Stopped); }
                        m.pump_queue(Some(id), cx);
                    }
                    Err(e) => { q.send_now_failed(); m.fail(e, cx); }
                }
                cx.notify();
            });
        });
    }

    /// Let a paused queue go again after a failed or stopped turn: its messages go
    /// together as soon as the agent is free, at once if it already is. `None` is the
    /// queue of a thread that could not start; resuming it starts a new thread.
    /// Dismissing or retrying a failed turn should call this for that thread too.
    pub fn resume_queue(&mut self, thread_id: Option<Uuid>, cx: &mut Context<Self>) {
        let Some(q) = self.queues.get_mut(&thread_id) else { return };
        q.resume();
        cx.notify();
        self.pump_queue(thread_id, cx);
    }

    /// Send what a thread's queue has ready, if its agent is free.
    fn pump_queue(&mut self, thread: Option<Uuid>, cx: &mut Context<Self>) {
        let busy = match thread {
            Some(id) => self.thread_busy(id, cx),
            // A thread that could not start goes out as a new one, from the project page.
            None => self.starting || self.selected.is_some() || self.active_project.is_none() || !self.model_ready(),
        };
        let Some(batch) = self.queues.get_mut(&thread).and_then(|q| q.next(busy)) else { return };
        cx.notify();
        match thread {
            Some(id) => self.send_batch(id, batch, cx),
            None => {
                let images = batch.images().into_iter().map(image_input).collect();
                self.send_prompt_with_files(batch.prompt(), images, batch.files(), cx);
            }
        }
    }

    /// Send a drained batch to its thread; files go up first for a thread on a server.
    fn send_batch(&mut self, id: Uuid, mut batch: Batch, cx: &mut Context<Self>) {
        let root = self.threads.get(&id).map(|t| { let meta = &t.read(cx).meta; meta.project_root.clone().unwrap_or_else(|| meta.cwd.clone()) }).unwrap_or_default();
        let files = batch.files();
        let (Some(remote), false) = (crate::runtime::servers(cx).for_root(&root), files.is_empty()) else {
            return self.send_batch_resolved(id, batch, cx);
        };
        self.sending.insert(id);
        let this = cx.entity().downgrade();
        spawn_service(cx, upload_files(remote, files), move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                m.sending.remove(&id);
                match res {
                    Ok(placed) => {
                        let mut placed = placed.into_iter();
                        for file in batch.messages.iter_mut().flat_map(|m| m.files.iter_mut()) {
                            if let Some(there) = placed.next() { *file = there; }
                        }
                        m.send_batch_resolved(id, batch, cx);
                    }
                    Err(e) => {
                        m.queues.entry(Some(id)).or_default().requeue(batch);
                        m.fail(format!("Could not send the attached files to the server: {e}"), cx);
                    }
                }
            });
        });
    }

    fn send_batch_resolved(&mut self, id: Uuid, mut batch: Batch, cx: &mut Context<Self>) {
        for message in &mut batch.messages {
            message.text = with_files_note(&message.files, &message.text);
            message.files.clear();
        }
        let bubbles = batch.messages.iter().map(|m| (m.text.clone(), m.images.clone())).collect();
        let images = batch.images().into_iter().map(image_input).collect();
        // The thread in view sends with what the composer shows, like a typed message.
        let prefs = (self.selected == Some(id)).then(|| {
            let mut prefs = self.prefs.clone();
            let model = self.effective_model();
            prefs.model = (!model.is_empty()).then_some(model);
            prefs
        });
        self.send_to_thread(id, bubbles, batch.prompt(), images, prefs, true, cx);
    }

    /// Pin a reply into project memory.
    pub fn remember(&mut self, text: String, cx: &mut Context<Self>) {
        let Some(id) = self.selected else { return };
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::remember(&state, id.to_string(), text).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    match res {
                        Ok(_) => m.toast(ToastKind::Success, "Saved to project memory"),
                        Err(e) => m.fail(e, cx),
                    }
                    cx.notify();
                });
            },
        );
    }

    /// Plan handoff: switch to `backend`/`model`, drop to auto approvals, and
    /// ask the (possibly different) agent to implement the plan.
    pub fn code_plan_with(&mut self, backend: &str, model: Option<String>, cx: &mut Context<Self>) {
        if self.active_workspace.as_deref().is_some_and(|id| self.workspaces.iter().any(|w| w.id == id && w.inline)) { self.workspace_from_inline(cx); }
        self.set_backend(backend, model, cx);
        self.set_mode("auto", cx);
        self.send_prompt("Implement the plan above. Work through it step by step and report when done.".into(), Vec::new(), cx);
    }

    pub fn cancel_selected(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selected else { return };
        let core = self.core_of_thread(id, cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { core.cancel_session(id.to_string()).await },
            move |res, cx| {
                if let Err(e) = res {
                    let _ = this.update(cx, |m, cx| m.fail(e, cx));
                }
            },
        );
    }

    pub fn set_mode(&mut self, mode: &str, cx: &mut Context<Self>) {
        if mode != "plan" && (self.prefs.temporary || self.active_workspace.as_deref().is_some_and(|id| self.workspaces.iter().any(|w| w.id == id && w.inline))) {
            self.toast(ToastKind::Info, "This conversation is for questions. Choose Make changes to continue in a thread.");
            cx.notify(); return;
        }
        let previous = self.prefs.mode.clone();
        self.prefs.mode = mode.to_string();
        if let Some(id) = self.selected {
            if let Some(t) = self.threads.get(&id) {
                t.update(cx, |t, cx| { t.meta.approval_mode = Some(mode.to_string()); cx.notify(); });
            }
            let core = self.core_of_thread(id, cx);
            let requested = mode.to_string();
            let mode = requested.clone();
            let this = cx.entity().downgrade();
            spawn_service(cx, async move { core.set_approval_mode(id.to_string(), mode).await }, move |result, cx| {
                let _ = this.update(cx, |m, cx| {
                    if let Err(error) = result {
                        if m.selected == Some(id) && m.prefs.mode == requested { m.prefs.mode = previous.clone(); }
                        if let Some(t) = m.threads.get(&id) {
                            t.update(cx, |t, cx| {
                                if t.meta.approval_mode.as_deref() == Some(&requested) { t.meta.approval_mode = Some(previous); }
                                cx.notify();
                            });
                        }
                        m.fail(error, cx);
                    }
                    cx.notify();
                });
            });
        }
        cx.notify();
    }

    pub fn provider_mode_options(&self, cx: &App) -> serde_json::Value {
        self.selected.and_then(|id| self.threads.get(&id)).filter(|t| t.read(cx).meta.model == self.effective_model()).map(|t| &t.read(cx).provider_modes)
            .filter(|v| v.get("backend").and_then(|v|v.as_str()) == Some(self.prefs.backend.as_str()))
            .cloned().unwrap_or_default()
    }

    pub fn cycle_mode(&mut self, cx: &mut Context<Self>) {
        let options = self.provider_mode_options(cx);
        let mut next = next_shortcut_mode(&self.prefs.mode);
        for _ in 0..3 {
            if crate::views::composer::provider_mode_presentation(next, &options).is_some() { break; }
            next = next_shortcut_mode(next);
        }
        self.set_mode(next, cx);
    }

    pub fn toggle_mcp_pref(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(i) = self.prefs.mcp_servers.iter().position(|n| n == name) {
            self.prefs.mcp_servers.remove(i);
        } else {
            self.prefs.mcp_servers.push(name.to_string());
        }
        cx.notify();
    }

    pub fn set_effort(&mut self, effort: &str, cx: &mut Context<Self>) {
        self.prefs.effort = effort.to_string();
        // Live threads whose agent exposes an `effort` option take it now.
        if let Some(t) = self.selected_thread() {
            let (id, live) = {
                let t = t.read(cx);
                (t.id(), t.meta.live)
            };
            if live {
                let core = self.core_of_thread(Uuid::parse_str(&id).unwrap_or_default(), cx);
                let e = effort.to_string();
                let this = cx.entity().downgrade();
                spawn_service(
                    cx,
                    async move { core.set_session_effort(id, e).await },
                    move |res, cx| {
                        if let Ok(true) = res {
                            let _ = this.update(cx, |m, cx| {
                                m.toast(ToastKind::Info, "Reasoning effort updated for this thread");
                                cx.notify();
                            });
                        }
                    },
                );
            }
        }
        cx.notify();
    }

    pub fn set_backend(&mut self, backend: &str, model: Option<String>, cx: &mut Context<Self>) {
        if self.prefs.backend != backend || self.prefs.model != model { self.prefs.fast_mode = Some(false); }
        self.prefs.backend = backend.to_string();
        self.prefs.model = model;
        let (levels, _) = crate::views::brand::effort_levels(backend);
        if !levels.contains(&self.prefs.effort.as_str()) {
            self.prefs.effort = levels.iter().find(|l| **l == "high").or(levels.last()).map(|s| s.to_string()).unwrap_or_default();
        }
        cx.notify();
    }

    pub fn load_archived(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::kv_get(&state, ARCHIVED_KEY).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    if let Ok(Some(raw)) = res {
                        m.archived = raw
                            .split(',')
                            .filter_map(|s| Uuid::parse_str(s.trim()).ok())
                            .collect();
                        cx.notify();
                    }
                });
            },
        );
    }

    fn save_archived(&self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let raw = self.archived.iter().map(|u| u.to_string()).collect::<Vec<_>>().join(",");
        spawn_service(cx, async move { services::kv_set(&state, ARCHIVED_KEY, &raw).await }, |_, _| {});
    }

    pub fn load_sidebar_prefs(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move {
            let sort = services::kv_get(&state, SIDEBAR_SORT_KEY).await.ok().flatten();
            let pinned = services::kv_get(&state, PINNED_PROJECTS_KEY).await.ok().flatten();
            let intro = services::kv_get(&state, PROJECT_INTRO_KEY).await.ok().flatten();
            let recent = services::kv_get(&state, RECENT_WINDOW_KEY).await.ok().flatten();
            let seen = services::kv_get(&state, THREAD_SEEN_KEY).await.ok().flatten();
            let since = match services::kv_get(&state, THREAD_SEEN_SINCE_KEY).await.ok().flatten() {
                Some(since) => since,
                None => {
                    let now = chrono::Utc::now().to_rfc3339();
                    let _ = services::kv_set(&state, THREAD_SEEN_SINCE_KEY, &now).await;
                    now
                }
            };
            (sort, pinned, intro, recent, seen, since)
        }, move |(sort, pinned, intro, recent, seen, since), cx| {
            let _ = this.update(cx, |m, cx| {
                if let Some(seen) = seen.and_then(|raw| serde_json::from_str::<HashMap<Uuid, chrono::DateTime<chrono::Utc>>>(&raw).ok()) {
                    for (id, at) in seen { m.seen.entry(id).and_modify(|t| *t = (*t).max(at)).or_insert(at); }
                }
                m.seen_since = since.parse().ok();
                if let Some(sort) = sort { m.sidebar_sort = SidebarSort::from_key(&sort); }
                if let Some(recent) = recent { m.recent_window = RecentWindow::from_key(&recent); }
                m.project_intro_seen = intro.as_deref() == Some("1");
                if let Some(pinned) = pinned.and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok()) { m.pinned_projects = pinned; }
                cx.notify();
            });
        });
    }

    pub fn dismiss_project_intro(&mut self, cx: &mut Context<Self>) {
        self.project_intro_seen = true;
        let state = svc(cx);
        spawn_service(cx, async move { services::kv_set(&state, PROJECT_INTRO_KEY, "1").await }, |_, _| {});
        cx.notify();
    }

    /// Close finished threads and delete branches whose work is already in the default branch.
    pub fn cleanup_merged(&mut self, root: String, cx: &mut Context<Self>) {
        if self.git_busy { return; }
        self.git_busy = true; cx.notify();
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::workspaces::cleanup_merged(&state, root).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                m.git_busy = false;
                match res { Ok(note) => m.toast(ToastKind::Success, note), Err(e) => m.fail(e, cx) }
                m.refresh_workspaces(cx); m.refresh_threads(cx); m.refresh_project_overview(cx);
                cx.notify();
            });
        });
    }

    pub fn set_sidebar_sort(&mut self, sort: SidebarSort, cx: &mut Context<Self>) {
        self.sidebar_sort = sort;
        let state = svc(cx);
        spawn_service(cx, async move { services::kv_set(&state, SIDEBAR_SORT_KEY, sort.key()).await }, |_, _| {});
        cx.notify();
    }

    /// The folder whose processes the right panel lists: the selected thread's, else the
    /// open project's. Server projects run elsewhere, so they have none here.
    pub fn process_folder(&self, cx: &App) -> Option<String> {
        let folder = self
            .selected
            .and_then(|id| self.threads.get(&id))
            .map(|t| t.read(cx).meta.cwd.clone())
            .or_else(|| self.active_project.clone())?;
        (!folder.is_empty() && !crate::remote::is_server_root(&folder)).then_some(folder)
    }

    /// Show the folder in view from the last scan, without waiting for a new one.
    pub fn show_cached_processes(&mut self, cx: &mut Context<Self>) {
        let folder = self.process_folder(cx);
        let cached = folder.as_ref().and_then(|f| self.processes_by_folder.get(f.trim_end_matches('/'))).cloned();
        let (list, scanned) = match cached {
            Some(list) => (list, folder),
            None => (Vec::new(), None),
        };
        if self.processes != list || self.processes_folder != scanned {
            self.processes = list;
            self.processes_folder = scanned;
            cx.notify();
        }
    }

    /// Rescan every local thread and project folder in one pass (one `ps` + `lsof`).
    pub fn refresh_processes(&mut self, cx: &mut Context<Self>) {
        if self.processes_scanning {
            self.processes_again = true;
            return;
        }
        let mut folders: Vec<String> = self.local_projects.clone();
        folders.extend(self.local_threads.iter().filter(|t| !self.archived.contains(&Uuid::parse_str(&t.id).unwrap_or_default())).map(|t| t.cwd.clone()));
        folders.extend(self.process_folder(cx));
        folders.retain(|f| !f.is_empty() && !crate::remote::is_server_root(f));
        folders.sort();
        folders.dedup();
        if folders.is_empty() {
            self.show_cached_processes(cx);
            return;
        }
        self.processes_scanning = true;
        self.processes_again = false;
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { bomb_core::services::processes::list_for_folders(&folders).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    m.processes_scanning = false;
                    if let Ok(map) = res { m.processes_by_folder = map; }
                    m.show_cached_processes(cx);
                    if std::mem::take(&mut m.processes_again) { m.refresh_processes(cx); }
                });
            },
        );
    }

    /// Ask a listed process to quit; `force` kills it.
    pub fn stop_process(&mut self, pid: u32, force: bool, cx: &mut Context<Self>) {
        let Some(folder) = self.processes_folder.clone() else { return };
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { bomb_core::services::processes::stop(&folder, pid, force).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    match res {
                        Err(e) => m.fail(e, cx),
                        // A killed process is gone; one asked to quit usually goes within a second.
                        Ok(()) if force => {
                            m.processes.retain(|p| p.pid != pid);
                            for list in m.processes_by_folder.values_mut() { list.retain(|p| p.pid != pid); }
                            cx.notify();
                        }
                        Ok(()) => {}
                    }
                    m.refresh_processes(cx);
                });
                // Catch the exit as soon as it happens instead of at the next 3 s tick.
                let this = this.clone();
                cx.spawn(async move |cx| {
                    for wait in [300u64, 800, 1500] {
                        cx.background_executor().timer(std::time::Duration::from_millis(wait)).await;
                        if this.update(cx, |m, cx| m.refresh_processes(cx)).is_err() { return; }
                    }
                }).detach();
            },
        );
    }

    /// The thread finished work since it was last on screen. Working or waiting threads
    /// have their own marks, and the open thread is being looked at.
    pub fn is_unseen(&self, id: Uuid, cx: &App) -> bool {
        let Some(since) = self.seen_since else { return false };
        if self.selected == Some(id) { return false; }
        let Some(t) = self.threads.get(&id) else { return false };
        let t = t.read(cx);
        if t.thread.presence.turn_active() || t.meta.status == "running" { return false; }
        let Ok(updated) = t.meta.updated_at.parse::<chrono::DateTime<chrono::Utc>>() else { return false };
        updated > self.seen.get(&id).copied().unwrap_or(since).max(since)
    }

    /// Record that a thread was looked at, and save it so a restart agrees.
    fn mark_seen(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.seen.insert(id, chrono::Utc::now());
        let raw = serde_json::to_string(&self.seen).unwrap_or_default();
        let state = svc(cx);
        spawn_service(cx, async move { services::kv_set(&state, THREAD_SEEN_KEY, &raw).await }, |_, _| {});
    }

    pub fn set_recent_window(&mut self, window: RecentWindow, cx: &mut Context<Self>) {
        self.recent_window = window;
        let state = svc(cx);
        spawn_service(cx, async move { services::kv_set(&state, RECENT_WINDOW_KEY, window.key()).await }, |_, _| {});
        cx.notify();
    }

    pub fn toggle_pinned_project(&mut self, root: String, cx: &mut Context<Self>) {
        if let Some(ix) = self.pinned_projects.iter().position(|p| p == &root) { self.pinned_projects.remove(ix); } else { self.pinned_projects.push(root); }
        let raw = serde_json::to_string(&self.pinned_projects).unwrap_or_else(|_| "[]".into());
        let state = svc(cx);
        spawn_service(cx, async move { services::kv_set(&state, PINNED_PROJECTS_KEY, &raw).await }, |_, _| {});
        cx.notify();
    }

    /// Hide a thread without deleting anything.
    pub fn archive_thread(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.archived.insert(id);
        if self.selected == Some(id) {
            self.selected = None;
        }
        self.save_archived(cx);
        cx.notify();
    }

    pub fn unarchive_thread(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.archived.remove(&id);
        self.save_archived(cx);
        cx.notify();
    }

    pub fn remove_thread(&mut self, id: Uuid, cx: &mut Context<Self>) {
        tracing::info!(%id, "delete: removing thread");
        self.archived.remove(&id);
        let core = self.core_of_thread(id, cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { core.remove_session(id.to_string(), Some(true)).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    match res {
                        Ok(()) => {
                            tracing::info!(%id, "delete: removed");
                            if m.selected == Some(id) {
                                m.selected = None;
                            }
                            m.threads.remove(&id);
                            m.thread_order.retain(|x| *x != id);
                        }
                        Err(e) => m.fail(e, cx),
                    }
                    m.refresh_threads(cx);
                });
            },
        );
    }

    pub fn rename_thread(&mut self, id: Uuid, label: String, cx: &mut Context<Self>) {
        if let Some(t) = self.threads.get(&id) {
            t.update(cx, |t, cx| {
                t.thread.label = Some(label.clone());
                cx.notify();
            });
        }
        let core = self.core_of_thread(id, cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { core.rename_thread(id.to_string(), label).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| match res {
                    Ok(()) => m.refresh_threads(cx),
                    Err(e) => m.fail(e, cx),
                });
            },
        );
    }

    pub fn land_thread(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.select(Some(id), cx);
        self.review_open = true;
        self.refresh_review(cx);
    }

    pub fn sync_thread(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if let Some(w) = self.workspaces.iter().find(|w| w.threads.contains(&id.to_string())) {
            self.run_workspace_action(w.id.clone(), "update".into(), String::new(), cx);
        }
    }

    /// Ask the thread's own agent to do the merge in its chat. With `then_close`, the feature is closed once the merge has landed.
    pub fn merge_to_main(&mut self, workspace: String, then_close: bool, cx: &mut Context<Self>) {
        let core = self.core_of_workspace(&workspace, cx);
        let this = cx.entity().downgrade();
        let id = workspace.clone();
        spawn_service(cx, async move { core.merge_request(id).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| match res {
                Ok(request) => {
                    m.review_open = false;
                    m.open_workspace(workspace.clone(), cx);
                    // A planning-only turn cannot merge anything.
                    if m.prefs.mode == "plan" { m.prefs.mode = "ask".into(); }
                    if then_close { m.close_after_merge.insert(workspace.clone()); } else { m.close_after_merge.remove(&workspace); }
                    m.send_prompt(request, vec![], cx);
                    cx.notify();
                }
                Err(e) => m.fail(e, cx),
            });
        });
    }

    /// A turn ended: refresh the board, and finish any pending "merge and close".
    fn turn_ended(&mut self, session: Uuid, cx: &mut Context<Self>) {
        let Some(w) = self.workspaces.iter().find(|w| w.threads.contains(&session.to_string())).cloned() else { return; };
        if self.active_project.as_deref() == Some(&w.project_root) { self.refresh_project_overview(cx); }
        // A resumed session can report idle before its turn starts; wait for the real end.
        if self.threads.get(&session).is_some_and(|t| t.read(cx).thread.presence.turn_active()) { return; }
        let then_close = self.close_after_merge.remove(&w.id);
        // Only a thread other threads build on needs the after-merge check when it stays open.
        let has_children = self.stacks.get(&w.project_root).into_iter().flatten().any(|l| l.parent == w.id);
        if !then_close && !has_children { return; }
        let core = self.core_of_workspace(&w.id, cx);
        let this = cx.entity().downgrade();
        let id = w.id.clone();
        spawn_service(cx, async move { core.is_merged(&id).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| match res {
                Ok(true) if then_close => m.close_feature(w.id.clone(), cx),
                Ok(true) => m.restack_after_merge(w.clone(), cx),
                Ok(false) if then_close => { m.toast(ToastKind::Info, format!("“{}” was not closed because its work is not fully in main yet. Check the chat.", w.name)); cx.notify(); }
                Ok(false) => {}
                Err(e) => m.fail(e, cx),
            });
        });
    }

    /// A parent thread's work landed and the thread stays open: bring its children up to date,
    /// and hand any conflicted child to its own agent.
    fn restack_after_merge(&mut self, parent: grok_persistence::WorkspaceRecord, cx: &mut Context<Self>) {
        let core = self.core_of_workspace(&parent.id, cx);
        let this = cx.entity().downgrade();
        let id = parent.id.clone();
        spawn_service(cx, async move { core.restack(id).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| match res {
                Ok(results) => {
                    m.refresh_project_overview(cx);
                    m.refresh_workspaces(cx);
                    for r in results {
                        use bomb_core::services::workspaces::Restack;
                        match r.outcome {
                            Restack::Updated => m.toast(ToastKind::Success, format!("“{}” was brought up to date now that “{}” is in main.", r.name, parent.name)),
                            Restack::Skipped => m.toast(ToastKind::Info, format!("“{}” builds on “{}” but is busy or has unsaved edits; it is brought up to date next time.", r.name, parent.name)),
                            Restack::NeedsHand(files) => {
                                m.toast(ToastKind::Warning, format!("“{}” needs a hand: bringing in “{}” hit conflicts. Its agent has been asked to sort them out.", r.name, parent.name));
                                m.ask_agent_to_finish_update(r.workspace.clone(), parent.name.clone(), files, cx);
                            }
                        }
                    }
                    cx.notify();
                }
                Err(e) => m.fail(e, cx),
            });
        });
    }

    /// Open the child's chat and ask its agent to finish the conflicted update.
    fn ask_agent_to_finish_update(&mut self, workspace: String, parent_name: String, files: Vec<String>, cx: &mut Context<Self>) {
        self.review_open = false;
        self.open_workspace(workspace, cx);
        if self.prefs.mode == "plan" { self.prefs.mode = "ask".into(); }
        let list = files.iter().take(12).map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n");
        let request = format!(
            "“{parent_name}” was merged into main, and bringing the latest main into this branch hit conflicts in these files:\n{list}\n\nResolve the conflicts so the merge keeps this thread's changes and the new work from main, run `git add` on the resolved files and `git commit` to finish the merge in progress, and say when it is done."
        );
        self.send_prompt(request, vec![], cx);
    }

    /// Archive the thread and its chats; the branch is removed only when its work is already merged.
    pub fn close_feature(&mut self, workspace: String, cx: &mut Context<Self>) {
        if self.git_busy { return; }
        self.git_busy = true; cx.notify();
        let core = self.core_of_workspace(&workspace, cx);
        let this = cx.entity().downgrade();
        let id = workspace.clone();
        spawn_service(cx, async move { core.close_feature(id).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                m.git_busy = false;
                match res {
                    Ok(note) => {
                        if let Some(w) = m.workspaces.iter().find(|w| w.id == workspace).cloned() {
                            m.archived.extend(w.threads.iter().filter_map(|t| Uuid::parse_str(t).ok()));
                            m.save_archived(cx);
                            if m.active_workspace.as_deref() == Some(&workspace) { m.review_open = false; m.set_active_project(w.project_root, cx); }
                        }
                        m.toast(ToastKind::Success, note);
                    }
                    Err(e) => m.fail(e, cx),
                }
                m.refresh_workspaces(cx); m.refresh_threads(cx); m.refresh_project_overview(cx);
                cx.notify();
            });
        });
    }

    // ── projects ────────────────────────────────────────────────────────

    pub fn create_project(&mut self, cx: &mut Context<Self>) {
        // New projects start in ~/Documents/BombCode rather than loose in the home folder.
        let directory = match services::default_projects_dir() {
            Ok(dir) if std::fs::create_dir_all(&dir).is_ok() => dir,
            _ => std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp")),
        };
        let rx=cx.prompt_for_new_path(&directory,Some("New project"));
        cx.spawn(async move |weak,cx| {
            let Ok(Ok(Some(path)))=rx.await else {return;};
            let _=weak.update(cx,|_,cx| {
                let target=path.clone();let app=weak.clone();
                spawn_service(cx,async move {
                    services::thread_setup::create(&target).await?;
                    Ok::<_,String>(target)
                },move |result,cx| {let _=app.update(cx,|m,cx|match result {Ok(path)=>m.add_project(path,cx),Err(e)=>m.fail(e,cx)});});
            });
        }).detach();
    }

    pub fn open_project(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn(async move |weak, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = weak.update(cx, |m, cx| m.add_project(path, cx));
        })
        .detach();
    }

    pub fn add_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // The whole home folder (or the disk) as one project makes every thread copy everything.
        if path.parent().is_none() || std::env::var_os("HOME").is_some_and(|home| home == path) {
            self.fail("Choose a project folder inside your home folder, not the home folder itself. New projects go in Documents/BombCode.".into(), cx);
            return;
        }
        let p = path.display().to_string();
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::add_project(&state, p.clone()).await.map(|list| (p, list)) },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| match res {
                    Ok((p, list)) => {
                        m.local_projects = list;
                        m.combine_lists(cx);
                        m.set_active_project(p, cx);
                    }
                    Err(e) => m.fail(e, cx),
                });
            },
        );
    }

    pub fn set_active_project(&mut self, root: String, cx: &mut Context<Self>) {
        self.new_thread_open = false;
        if self.prefs.mode == "yolo" { self.prefs.mode = "plan".into(); }
        self.source_thread = None;
        self.active_project = Some(root);
        self.overview_branch = None;
        self.refresh_project_overview(cx);
        self.refresh_project_status(false, cx);
        self.active_workspace = None;
        self.review = None;
        self.review_open = false;
        self.selected = None;
        self.prefs.temporary = false;
        self.prefs.worktree = true;
        cx.notify();
    }

    pub fn reveal_project(&mut self, cx: &mut Context<Self>) {
        let cwd = self.active_project.clone();
        let sid = self.selected.map(|s| s.to_string());
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::reveal_project(&state, cwd, sid).await },
            move |res, cx| {
                if let Err(e) = res {
                    let _ = this.update(cx, |m, cx| m.fail(e, cx));
                }
            },
        );
    }

    // ── dev server ──────────────────────────────────────────────────────

    /// The folder in view, when it is on a paired server.
    fn server_folder_in_view(&self, cx: &App) -> Option<String> {
        self.selected.and_then(|id| self.threads.get(&id)).map(|t| t.read(cx).meta.cwd.clone()).or_else(|| self.active_project.clone()).filter(|f| crate::remote::is_server_root(f))
    }

    /// Start or stop the dev server on the paired server, and carry its port to this Mac for the preview.
    fn server_dev_toggle(&mut self, folder: String, cx: &mut Context<Self>) {
        let Some(remote) = crate::runtime::servers(cx).for_root(&folder) else { return; };
        let this = cx.entity().downgrade();
        if self.server_preview.take().is_some() {
            spawn_service(cx, async move { remote.call::<DevServerStatus>("stop_dev_server", serde_json::Value::Null).await }, move |res, cx| {
                let _ = this.update(cx, |m, cx| { match res { Ok(s) => { m.dev_server = Some(s); m.toast(ToastKind::Info, "Dev server stopped"); } Err(e) => m.fail(e, cx) } cx.notify(); });
            });
            return;
        }
        let server = remote.config.id.clone();
        spawn_service(cx, async move {
            let mut status: DevServerStatus = remote.call("start_dev_server", serde_json::json!({ "root": remote.path_of(&folder) })).await?;
            let port = status.port.ok_or_else(|| if status.message.is_empty() { "The dev server did not report a port.".to_string() } else { status.message.clone() })?;
            let (local, guard) = crate::remote::live::forward_port(remote, port).await?;
            let url = format!("http://127.0.0.1:{local}");
            status.url = Some(url.clone());
            status.cwd = Some(folder);
            Ok::<_, String>((status, guard, url))
        }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                match res {
                    Ok((status, guard, url)) => {
                        m.toast(ToastKind::Success, format!("Dev server running on the server, shown here at {url}"));
                        m.dev_server = Some(status);
                        m.server_preview = Some(ServerPreview { server, _forward: guard, local_url: url });
                    }
                    Err(e) => m.fail(e, cx),
                }
                cx.notify();
            });
        });
    }

    pub fn dev_server_toggle(&mut self, cx: &mut Context<Self>) {
        if let Some(folder) = self.server_folder_in_view(cx) { self.server_dev_toggle(folder, cx); return; }
        if self.server_preview.is_some() { self.fail("A dev server is running on a server project. Stop it from that project first.".into(), cx); return; }
        let running = self.dev_server.as_ref().map(|s| s.running).unwrap_or(false);
        let state = svc(cx);
        let this = cx.entity().downgrade();
        if running {
            spawn_service(
                cx,
                async move { services::stop_dev_server(&state).await },
                move |res, cx| {
                    let _ = this.update(cx, |m, cx| match res {
                        Ok(s) => {
                            m.dev_server = Some(s);
                            m.toast(ToastKind::Info, "Dev server stopped");
                            cx.notify();
                        }
                        Err(e) => m.fail(e, cx),
                    });
                },
            );
        } else {
            let cwd = self.active_project.clone();
            let sid = self.selected.map(|s| s.to_string());
            spawn_service(
                cx,
                async move { services::start_dev_server(&state, cwd, sid, Some(true)).await },
                move |res, cx| {
                    let _ = this.update(cx, |m, cx| match res {
                        Ok(s) => {
                            let msg = s.url.clone().map(|u| format!("Dev server at {u}")).unwrap_or(s.message.clone());
                            m.dev_server = Some(s);
                            m.ensure_dev_poll(cx);
                            m.toast(ToastKind::Success, msg);
                            cx.notify();
                        }
                        Err(e) => m.fail(e, cx),
                    });
                },
            );
        }
    }

    pub fn dev_server_open(&mut self, cx: &mut Context<Self>) {
        if let Some(preview) = &self.server_preview { cx.open_url(&preview.local_url); return; }
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::open_dev_server(&state).await },
            move |res, cx| {
                if let Err(e) = res {
                    let _ = this.update(cx, |m, cx| m.fail(e, cx));
                }
            },
        );
    }

    // ── sign-in ─────────────────────────────────────────────────────────

    /// Grok signs in inside the app (device code); other CLIs open a terminal.
    pub fn sign_in(&mut self, backend: &str, cx: &mut Context<Self>) {
        // A server project uses the server's own sign-ins: run the provider's login there, in a terminal
        // shown here, so the link or code can be approved in this Mac's browser.
        if let Some(folder) = self.active_project.clone().filter(|r| crate::remote::is_server_root(r)) {
            let Some(remote) = crate::runtime::servers(cx).for_root(&folder) else { return; };
            let this = cx.entity().downgrade();
            let (name, b) = (remote.config.name.clone(), backend.to_string());
            spawn_service(cx, async move { remote.call::<String>("login_command", serde_json::json!({ "backend": b })).await }, move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    match res {
                        Ok(command) => m.server_login_request = Some((folder, command, format!("Sign in on {name}"))),
                        Err(e) => m.fail(e, cx),
                    }
                    cx.notify();
                });
            });
            return;
        }
        if backend == "grok" {
            self.start_grok_login(cx);
            return;
        }
        let b = backend.to_string();
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::open_backend_login(b, false).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| match res {
                    Ok(()) => m.toast(ToastKind::Info, "Sign in from the terminal window that just opened."),
                    Err(e) => m.fail(e, cx),
                });
            },
        );
    }

    pub fn sign_out(&mut self, backend: &str, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        let b = backend.to_string();
        spawn_service(
            cx,
            async move {
                if b == "grok" {
                    services::logout_grok(&state).await.map(|_| ())
                } else {
                    services::open_backend_login(b, true).await
                }
            },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    if let Err(e) = res {
                        m.fail(e, cx);
                    }
                    m.refresh_services(cx);
                });
            },
        );
    }

    pub fn start_grok_login(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        self.login = None;
        self.login_starting = true;
        cx.notify();
        spawn_service(
            cx,
            async move { services::start_grok_login(&state).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| {
                    m.login_starting = false;
                    match res {
                        Ok(s) => {
                            m.login = Some(s);
                            m.ensure_login_poll(cx);
                            cx.notify();
                        }
                        Err(e) => {
                            m.login = None;
                            m.fail(e, cx);
                        }
                    }
                });
            },
        );
    }

    fn ensure_login_poll(&mut self, cx: &mut Context<Self>) {
        if self.login_poll.is_some() {
            return;
        }
        self.login_poll = Some(cx.spawn(async move |weak, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let state = cx.update(|cx| svc(cx));
                let handle = cx.update(|cx| cx.global::<crate::runtime::Tokio>().0.clone());
                let (tx, rx) = async_channel::bounded(1);
                handle.spawn(async move {
                    let _ = tx.send(services::grok_login_status(&state).await).await;
                });
                let keep = match rx.recv().await {
                    Ok(Ok(s)) => {
                        let done = matches!(s.phase, LoginPhase::Completed | LoginPhase::Failed) || !s.active;
                        weak.update(cx, |m, cx| {
                            if m.login.is_some() || m.login_starting {
                                m.login = Some(s);
                            }
                            if done {
                                m.login_poll = None;
                                m.refresh_services(cx);
                            }
                            cx.notify();
                        })
                        .map(|_| !done)
                        .unwrap_or(false)
                    }
                    _ => false,
                };
                if !keep {
                    break;
                }
            }
        }));
    }

    pub fn submit_login_code(&mut self, code: String, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::submit_grok_login_code(&state, code).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| match res {
                    Ok(s) => {
                        m.login = Some(s);
                        cx.notify();
                    }
                    Err(e) => m.fail(e, cx),
                });
            },
        );
    }

    pub fn open_login_url(&mut self, cx: &mut Context<Self>) {
        if let Some(url) = self.login.as_ref().and_then(|l| l.login_url.clone()) {
            cx.open_url(&url);
        }
    }

    pub fn cancel_login(&mut self, cx: &mut Context<Self>) {
        self.login = None;
        self.login_poll = None;
        let state = svc(cx);
        spawn_service(cx, async move { services::cancel_grok_login(&state).await }, |_, _| {});
        cx.notify();
    }

    pub fn close_login(&mut self, cx: &mut Context<Self>) {
        self.login = None;
        self.login_poll = None;
        self.refresh_services(cx);
        cx.notify();
    }

    pub fn new_mock_session(&mut self, cx: &mut Context<Self>) {
        let cwd = self
            .active_project
            .clone()
            .or_else(|| std::env::var("HOME").ok())
            .unwrap_or_else(|| "/tmp".into());
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { services::start_mock_session(&state, cwd).await },
            move |res, cx| {
                let _ = this.update(cx, |m, cx| match res {
                    Ok(r) => {
                        if let Ok(id) = Uuid::parse_str(&r.id) {
                            m.selected = Some(id);
                        }
                        m.refresh_threads(cx);
                    }
                    Err(e) => m.fail(e, cx),
                });
            },
        );
    }
}

fn image_input(a: ImageAttachment) -> ImageInput {
    ImageInput { mime_type: a.mime_type, data: a.data, name: a.name }
}

/// A message with a note naming its attached files, as the agent receives it.
fn with_files_note(files: &[(PathBuf, u64)], text: &str) -> String {
    if files.is_empty() { text.to_string() } else if text.is_empty() { crate::views::composer::attached_files_note(files) } else { format!("{text}\n\n{}", crate::views::composer::attached_files_note(files)) }
}

/// Copy attached files to a server; returns where each one landed there.
async fn upload_files(remote: std::sync::Arc<crate::remote::RemoteCore>, files: Vec<(PathBuf, u64)>) -> Result<Vec<(PathBuf, u64)>, String> {
    let client = remote.client()?;
    let mut placed = Vec::new();
    for (path, size) in &files {
        let stream = client.upload(path).await?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        let there: String = client.request("store_attachment", serde_json::json!({ "stream": stream, "name": name })).await.and_then(|v| v.as_str().map(str::to_owned).ok_or_else(|| "bad reply".to_string()))?;
        placed.push((PathBuf::from(there), *size));
    }
    Ok(placed)
}

fn session_of(ev: &ControlEvent) -> Option<Uuid> {
    match ev {
        ControlEvent::SessionCreated { session_id, .. }
        | ControlEvent::SessionStatusChanged { session_id, .. }
        | ControlEvent::SessionCancelled { session_id, .. }
        | ControlEvent::SessionCompleted { session_id, .. }
        | ControlEvent::ToolCall { session_id, .. }
        | ControlEvent::PlanUpdate { session_id, .. }
        | ControlEvent::AgentMessage { session_id, .. }
        | ControlEvent::ApprovalRequired { session_id, .. }
        | ControlEvent::ApprovalResolved { session_id, .. } => Some(*session_id),
        ControlEvent::Error { session_id, .. } | ControlEvent::Raw { session_id, .. } => *session_id,
        _ => None,
    }
}

pub fn project_name(root: &str) -> String {
    if root.contains("/.bombcode/chats/") { return "Temporary chat".into(); }
    std::path::Path::new(root)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| root.to_string())
}

fn next_shortcut_mode(current: &str) -> &'static str {
    match current { "plan" => "ask", "ask" => "auto", _ => "plan" }
}

#[cfg(test)]
mod startup_project_tests {
    use super::startup_project;
    use grok_persistence::ThreadDto;

    fn thread(root: &str, updated: &str, model: &str) -> ThreadDto {
        serde_json::from_value(serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000000", "cwd": root, "mode": "acp", "model": model,
            "backend": "claude", "status": "idle", "live": false, "messageCount": 0,
            "createdAt": updated, "updatedAt": updated, "projectRoot": root, "mcpServers": [],
        })).unwrap()
    }

    #[test]
    fn opens_the_most_recent_project_and_never_home() {
        let home = std::env::var("HOME").unwrap();
        let threads = vec![
            thread("/src/old", "2026-09-01T00:00:00Z", "opus"),
            thread("/src/new", "2026-10-01T00:00:00Z", "opus"),
            thread(&home, "2026-10-05T00:00:00Z", "opus"),
            thread("/src/mock", "2026-10-06T00:00:00Z", "mock"),
        ];
        assert_eq!(startup_project(&threads, &[]).as_deref(), Some("/src/new"));
        let projects = vec![home.clone(), "/src/a".to_string()];
        assert_eq!(startup_project(&[], &projects).as_deref(), Some("/src/a"));
        assert_eq!(startup_project(&[], &[home]), None);
    }
}

#[cfg(test)]
mod recent_window_tests {
    use super::{recent_ids, RecentWindow};

    #[test]
    fn keeps_threads_inside_the_window_newest_first() {
        let now: chrono::DateTime<chrono::Utc> = "2026-10-07T12:00:00Z".parse().unwrap();
        let rows = vec![
            ("2026-10-07T10:00:00Z".to_string(), "today"),
            ("2026-10-05T12:00:01Z".to_string(), "two-days"),
            ("2026-09-30T12:00:00Z".to_string(), "week"),
            ("2026-08-01T00:00:00Z".to_string(), "old"),
            ("not a time".to_string(), "broken"),
        ];
        assert_eq!(recent_ids(&rows, RecentWindow::Day, now), ["today"]);
        assert_eq!(recent_ids(&rows, RecentWindow::ThreeDays, now), ["today", "two-days"]);
        assert_eq!(recent_ids(&rows, RecentWindow::Week, now), ["today", "two-days", "week"]);
        assert!(recent_ids(&rows, RecentWindow::Off, now).is_empty());
        assert_eq!(RecentWindow::from_key("nonsense"), RecentWindow::ThreeDays);
    }
}

#[cfg(test)]
mod sidebar_sort_tests {
    use super::{sort_rows, SidebarSort, SortKey};
    fn rows() -> Vec<(SortKey, &'static str)> {
        let row = |name: &str, updated: &str, created: &str| SortKey { name: name.into(), updated: updated.into(), created: created.into() };
        vec![
            (row("beta", "2026-09-20T10:00:00Z", "2026-09-01T00:00:00Z"), "beta"),
            (row("Alpha", "2026-09-19T10:00:00Z", "2026-09-10T00:00:00Z"), "alpha"),
            (row("empty", "", ""), "empty"),
            (row("gamma", "2026-09-21T10:00:00Z", "2026-08-01T00:00:00Z"), "gamma"),
        ]
    }
    fn order(sort: SidebarSort) -> Vec<&'static str> {
        let mut r = rows();
        sort_rows(&mut r, sort);
        r.into_iter().map(|(_, id)| id).collect()
    }
    #[test]
    fn recent_created_and_alphabetical_orders() {
        assert_eq!(order(SidebarSort::Recent), ["gamma", "beta", "alpha", "empty"]);
        assert_eq!(order(SidebarSort::Created), ["alpha", "beta", "gamma", "empty"]);
        assert_eq!(order(SidebarSort::Alphabetical), ["alpha", "beta", "empty", "gamma"]);
        assert_eq!(SidebarSort::from_key("nonsense"), SidebarSort::Recent);
    }
    #[test]
    fn only_the_most_recently_changed_rows_stay_visible_plus_the_current_one() {
        let row = |at: &str, current: bool, id: &str| (at.to_string(), current, id.to_string());
        let rows = vec![row("2026-09-01", false, "old"), row("2026-09-20", false, "a"), row("2026-09-19", false, "b"), row("2026-09-18", false, "c"), row("2026-08-01", true, "current")];
        let hidden = super::hidden_rows(rows.clone(), 3);
        assert_eq!(hidden, std::collections::HashSet::from(["old".to_string()]));
        assert!(super::hidden_rows(rows[..3].to_vec(), 3).is_empty());
    }
}

#[cfg(test)]
mod mode_shortcut_tests {
    #[test]
    fn shortcut_never_enters_full_access() {
        assert_eq!(super::next_shortcut_mode("plan"), "ask");
        assert_eq!(super::next_shortcut_mode("ask"), "auto");
        assert_eq!(super::next_shortcut_mode("auto"), "plan");
        assert_eq!(super::next_shortcut_mode("yolo"), "plan");
    }
}
