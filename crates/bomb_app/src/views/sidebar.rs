//! Left column: threads (project · time-ago / title / branch) and the
//! connected services footer.

use crate::views::button::Button;
use chrono::{DateTime, Utc};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Sizable, WindowExt};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use grok_cli_wrapper::BackendAuth;
use uuid::Uuid;

use crate::actions::NewThread;
use crate::models::app::{project_name, AppModel, ProjectGroup};
use crate::models::thread::ThreadModel;
use crate::theme::{Layout, Ui};

pub struct SidebarView {
    model: Entity<AppModel>,
    search: Entity<InputState>,
    /// Keyboard cursor through the filtered list.
    active_ix: usize,
    collapsed: std::collections::HashSet<String>,
    expanded: std::collections::HashSet<String>,
    /// The project last seen as the one in view, to notice when the user moves to another.
    last_active: Option<String>,
}

/// Recency bucket for thread search results.
/// Folder mark size on a project header, and where its name starts. Nested
/// rows begin at that x so their marks line up under the name (Codex).
const GROUP_ICON: f32 = 16.0;
const GROUP_INDENT: f32 = Layout::SPACE_SM + GROUP_ICON + Layout::SPACE_SM;

fn recency_group(iso: &str) -> &'static str {
    let Ok(t) = iso.parse::<DateTime<Utc>>() else { return "Earlier" };
    let now = Utc::now();
    let days = (now.date_naive() - t.with_timezone(&chrono::Local).date_naive()).num_days();
    match days {
        d if d <= 0 => "Today",
        1 => "Yesterday",
        d if d < 7 => "This week",
        _ => "Earlier",
    }
}

/// Open threads shown per project before "View more".
const VISIBLE_OPEN_THREADS: usize = 3;

impl SidebarView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search projects and conversations"));
        cx.subscribe(&search, |this, _, ev: &gpui_kit::component::input::InputEvent, cx| {
            match ev {
                gpui_kit::component::input::InputEvent::Change => {
                    this.active_ix = 0;
                    cx.notify();
                }
                gpui_kit::component::input::InputEvent::PressEnter { .. } => {
                    this.select_active(cx);
                }
                _ => {}
            }
        })
        .detach();
        Self { model, search, active_ix: 0, collapsed: Default::default(), expanded: Default::default(), last_active: None }
    }

    fn query(&self, cx: &App) -> String {
        self.search.read(cx).value().trim().to_lowercase()
    }

    /// Threads matching the query (title, project, model), newest first.
    fn filtered(&self, cx: &App) -> Vec<(Uuid, Entity<ThreadModel>)> {
        let q = self.query(cx);
        let m = self.model.read(cx);
        let mut v: Vec<(Uuid, Entity<ThreadModel>, String)> = m
            .thread_order
            .iter()
            .filter(|id| !m.archived.contains(id))
            .filter_map(|id| m.threads.get(id).cloned().map(|t| (*id, t)))
            .filter(|(_, t)| {
                let t = t.read(cx);
                let hay = format!(
                    "{} {} {} {} {}",
                    m.workspaces.iter().find(|w| w.threads.contains(&t.meta.id)).map(|w| format!("{} {}", w.name, w.branch)).unwrap_or_default(),
                    t.title(),
                    project_name(t.meta.project_root.as_deref().unwrap_or(&t.meta.cwd)),
                    t.meta.model,
                    t.meta.backend
                )
                .to_lowercase();
                hay.contains(&q)
            })
            .map(|(id, t)| {
                let at = t.read(cx).meta.updated_at.clone();
                (id, t, at)
            })
            .collect();
        v.sort_by(|a, b| b.2.cmp(&a.2));
        v.into_iter().map(|(id, t, _)| (id, t)).collect()
    }

    fn select_active(&mut self, cx: &mut Context<Self>) {
        let list = self.filtered(cx);
        if let Some((id, _)) = list.get(self.active_ix) {
            let id = *id;
            self.model.update(cx, |m, cx| m.select(Some(id), cx));
        }
    }

    /// A project section: disclosure header (folder mark, name, hairline,
    /// `+`, chevron) at 28px, then its workspace and conversation rows 2px
    /// apart. Git state shows as one 11px subline only when it says something.
    fn group(&self, g: &ProjectGroup, ui: &Ui, cx: &mut Context<Self>) -> impl IntoElement {
        // Only the project you are in is open by default; a click on the chevron overrides either way.
        let open_key = format!("project-open:{}", g.root);
        // A project's Mac copy counts as the same project.
        let in_view = { let m = self.model.read(cx); m.active_project.as_deref().is_some_and(|a| m.sidebar_root(a) == g.root) };
        let collapsed = if self.collapsed.contains(&g.root) { true } else if self.expanded.contains(&open_key) { false } else { !in_view };
        // A closed project still says when it needs you or is busy.
        let (running, waiting) = {
            let m = self.model.read(cx);
            g.threads.iter().filter(|id| !m.archived.contains(id)).filter_map(|id| m.threads.get(id)).fold((false, false), |(run, wait), t| {
                let t = t.read(cx);
                let asks = t.thread.open_approvals().count() > 0 || t.meta.status.contains("wait") || t.meta.status.contains("approv");
                (run || t.thread.presence.turn_active() || t.meta.status == "running", wait || asks)
            })
        };
        let root = g.root.clone();
        let root2 = root.clone();
        let app = self.model.clone();
        let add = app.clone();
        let add_root = root.clone();
        let hover = ui.hover;
        let rows: Vec<_> = {
            let m = self.model.read(cx);
            // Threads on a project's Mac copy list under the same entry as the server's.
            let mut rows: Vec<_> = m.workspaces.iter().filter(|w| m.sidebar_root(&w.project_root) == root).cloned().map(|w| {
                let threads: Vec<_> = w.threads.iter().filter_map(|t| Uuid::parse_str(t).ok()).filter_map(|id| m.threads.get(&id)).collect();
                let key = crate::models::app::SortKey {
                    name: if let [only] = threads.as_slice() { only.read(cx).title() } else { w.name.clone() },
                    updated: threads.iter().map(|t| t.read(cx).meta.updated_at.clone()).max().unwrap_or_default(),
                    created: w.created_at.clone(),
                };
                (key, w)
            }).collect();
            crate::models::app::sort_rows(&mut rows, m.sidebar_sort);
            rows.into_iter().map(|(_, w)| w).collect()
        };
        let mut group = div().flex().flex_col().gap(px(Layout::SIDEBAR_LIST_GAP));
        group = group.child(
            div()
                .flex()
                .items_center()
                .gap(px(Layout::SPACE_SM))
                .h(px(32.))
                .px(px(Layout::SPACE_SM))
                .child({
                    // Server projects wear a server mark, tinted by the connection.
                    let server = crate::runtime::servers(cx).for_root(&g.root);
                    let (icon, color, hint) = match &server {
                        Some(s) => match s.state() {
                            crate::remote::LinkState::Connected => (Lucide::Server, ui.text_muted, format!("On {} · connected", s.config.name)),
                            crate::remote::LinkState::Connecting => (Lucide::Server, ui.warning, format!("On {} · connecting…", s.config.name)),
                            crate::remote::LinkState::Offline(why) => (Lucide::Server, ui.danger, format!("On {} · {why}", s.config.name)),
                        },
                        None if crate::remote::is_server_root(&g.root) => (Lucide::Server, ui.danger, "On a server this Mac is no longer paired with".to_string()),
                        None => (if collapsed { Lucide::Folder } else { Lucide::FolderOpen }, ui.text_muted, "On this Mac".to_string()),
                    };
                    div()
                        .id(SharedString::from(format!("home-{}", g.root)))
                        .size(px(GROUP_ICON))
                        .flex_shrink_0()
                        .text_color(color)
                        .tooltip(move |window, cx| Tooltip::new(hint.clone()).build(window, cx))
                        .child(Icon::from(icon))
                })
                .child(
                    div()
                        .id(SharedString::from(format!("project-{root}")))
                        .flex_1()
                        .min_w_0()
                        .text_size(px(crate::theme::Type::BODY))
                        .line_height(px(20.))
                        .text_color(Ui::alpha(ui.text, 0.8))
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .child(g.name.clone())
                        .on_click(move |_, _, cx| app.update(cx, |m, cx| m.set_active_project(root.clone(), cx))),
                )
                .when(collapsed && (running || waiting), |el| {
                    let (color, hint) = if waiting { (ui.warning, "A thread here is waiting for your permission") } else { (ui.accent, "A thread here is working") };
                    el.child(
                        div()
                            .id(SharedString::from(format!("busy-{}", g.root)))
                            .size(px(7.))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(color)
                            .tooltip(move |window, cx| Tooltip::new(hint).build(window, cx)),
                    )
                })
                .child({
                    let pinned = self.model.read(cx).pinned_projects.contains(&g.root);
                    let pin_app = self.model.clone();
                    let pin_root = g.root.clone();
                    div()
                        .id(SharedString::from(format!("pin-{}", g.root)))
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .text_color(if pinned { ui.text } else { Ui::alpha(ui.text_muted, 0.45) })
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .tooltip(move |window, cx| Tooltip::new(if pinned { "Unpin project" } else { "Pin project to the top" }).build(window, cx))
                        .on_click(move |_, _, cx| pin_app.update(cx, |m, cx| m.toggle_pinned_project(pin_root.clone(), cx)))
                        .child(div().size(px(12.)).child(Icon::from(if pinned { Lucide::PinOff } else { Lucide::Pin })))
                })
                .child(
                    div()
                        .id(SharedString::from(format!("new-{}", g.root)))
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .text_color(ui.text_muted)
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .on_click(move |_, _, cx| add.update(cx, |m, cx| { m.set_active_project(add_root.clone(), cx); m.new_thread(cx); }))
                        .child(div().size(px(12.)).child(Icon::from(Lucide::Plus))),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("collapse-{}", g.root)))
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .text_color(Ui::alpha(ui.text_muted, 0.6))
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if collapsed { this.collapsed.remove(&root2); this.expanded.insert(open_key.clone()); }
                            else { this.expanded.remove(&open_key); this.collapsed.insert(root2.clone()); }
                            cx.notify();
                        }))
                        .child(div().size(px(12.)).child(Icon::from(if collapsed { Lucide::ChevronRight } else { Lucide::ChevronDown }))),
                ),
        );
        if collapsed { return group; }
        // Only the few most recently active open threads show until "View more" is opened.
        let more_key = format!("more:{}", g.root);
        let show_all = self.expanded.contains(&more_key);
        let hidden: std::collections::HashSet<String> = {
            let m = self.model.read(cx);
            let updated = |id: &Uuid| m.threads.get(id).map(|t| t.read(cx).meta.updated_at.clone()).unwrap_or_default();
            let mut open: Vec<(String, bool, String)> = Vec::new();
            for w in rows.iter().filter(|w| w.archived_at.is_none()) {
                let live: Vec<Uuid> = w.threads.iter().filter_map(|t| Uuid::parse_str(t).ok()).filter(|id| !m.archived.contains(id)).collect();
                if live.is_empty() { continue; }
                if w.inline {
                    for id in live { open.push((updated(&id), m.selected == Some(id), id.to_string())); }
                } else {
                    let current = m.active_workspace.as_deref() == Some(&w.id);
                    open.push((live.iter().map(updated).max().unwrap_or_default(), current, w.id.clone()));
                }
            }
            crate::models::app::hidden_rows(open, VISIBLE_OPEN_THREADS)
        };
        let hidden_count = hidden.len();
        let hidden = if show_all { Default::default() } else { hidden };
        for archived in [false, true] {
            let archived_key = format!("archived:{}", g.root);
            let archive_open = self.expanded.contains(&archived_key);
            if archived && rows.iter().any(|w| w.archived_at.is_some()) {
                group = group.child(
                    div()
                        .id(SharedString::from(archived_key.clone()))
                        .flex()
                        .items_center()
                        .gap(px(Layout::SPACE_XS))
                        .h(px(28.))
                        .pl(px(GROUP_INDENT))
                        .pr(px(Layout::SPACE_SM))
                        .text_size(px(crate::theme::Type::CAPTION))
                        .text_color(ui.subline())
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.expanded.remove(&archived_key) { this.expanded.insert(archived_key.clone()); }
                            cx.notify();
                        }))
                        .child(div().size(px(13.)).child(Icon::from(if archive_open { Lucide::ChevronDown } else { Lucide::ChevronRight })))
                        .child("Archived"),
                );
            }
            if archived && !archive_open { continue; }
            for w in rows.iter().filter(|w| !w.inline && w.archived_at.is_some() == archived) {
                // These rows stand in for conversations. Empty workspaces remain
                // available in the project overview; archived threads live below.
                if !w.threads.iter().filter_map(|id| Uuid::parse_str(id).ok())
                    .any(|id| !self.model.read(cx).archived.contains(&id)) { continue; }
                if hidden.contains(&w.id) { continue; }
                let single_thread = if w.threads.len() == 1 { Uuid::parse_str(&w.threads[0]).ok() } else { None };
                let app = self.model.clone();
                let wid = w.id.clone();
                let active = self.model.read(cx).active_workspace.as_deref() == Some(&w.id);
                let title = single_thread.and_then(|id| self.model.read(cx).threads.get(&id))
                    .map(|t| t.read(cx).title()).unwrap_or_else(|| w.name.clone());
                let row_title = title.clone();
                let menu_model = self.model.clone();
                let menu_id = wid.clone();
                let count = w.threads.len();
                let expanded = self.expanded.contains(&wid);
                let mut status = "idle".to_string();
                let mut current = grok_persistence::ModelUsage { backend: "grok".into(), model: String::new() };
                let mut history: Vec<grok_persistence::ModelUsage> = Vec::new();
                let mut models: Vec<String> = Vec::new();
                let mut latest = String::new();
                for tid in &w.threads {
                    if let Some(t) = Uuid::parse_str(tid).ok().and_then(|id| self.model.read(cx).threads.get(&id)) {
                        let tm = t.read(cx);
                        let meta = &tm.meta;
                        let used = grok_persistence::ModelUsage { backend: meta.backend.clone(), model: meta.model.clone() };
                        for item in meta.models_used.iter().chain(std::iter::once(&used)) {
                            if !history.contains(item) { history.push(item.clone()); }
                        }
                        if meta.updated_at >= latest { current = used; }
                        if !models.contains(&meta.model) { models.push(meta.model.clone()); }
                        if meta.updated_at > latest { latest = meta.updated_at.clone(); }
                        let s = meta.status.as_str();
                        if tm.thread.presence.turn_active() || s == "running" { status = "running".into(); }
                        else if status == "idle" && (s.contains("wait") || s.contains("approv")) { status = "waiting".into(); }
                        else if status == "idle" && s == "failed" { status = "failed".into(); }
                    }
                }
                let model_label = models.join(" · ");
                let corner = status_corner(&status, &latest, ui);
                let selected_bg = ui.selected_bg();
                group = group.child(
                    div()
                        .id(SharedString::from(format!("workspace-{wid}")))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .ml(px(GROUP_INDENT - Layout::SPACE_SM))
                        .px(px(Layout::SPACE_SM))
                        .py(px(6.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .text_color(if active { ui.text } else { Ui::alpha(ui.text, 0.8) })
                        .when(active, |el| el.bg(selected_bg))
                        .when(!active, |el| el.hover(move |s| s.bg(hover).text_color(ui.text)))
                        .on_click(move |_, _, cx| app.update(cx, |m, cx| m.open_workspace(wid.clone(), cx)))
                        .tooltip(move |window, cx| Tooltip::new(model_label.clone()).build(window, cx))
                        .context_menu(move |menu, _, _| {
                            let app = menu_model.clone();
                            let wid = menu_id.clone();
                            let name = title.clone();
                            let menu = menu.item(PopupMenuItem::new("Rename thread…").on_click(move |_, window, cx| {
                                if let Some(id) = single_thread {
                                    open_rename_dialog(app.clone(), id, name.clone(), window, cx);
                                } else {
                                    crate::views::workspaces::text_action(app.clone(), wid.clone(), "rename", "Thread name", name.clone(), window, cx);
                                }
                            }));
                            if let Some(id) = single_thread { thread_lifecycle_menu(menu, menu_model.clone(), id, false) } else { menu }
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(Layout::SPACE_SM))
                                .child(crate::views::brand::brand_mark(&current.backend, 13., true, ui))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(px(crate::theme::Type::BODY))
                                        .line_height(px(20.))
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .child(row_title),
                                )
                                .child(corner),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(Layout::SPACE_XS))
                                .h(px(14.))
                                .text_size(px(crate::theme::Type::CAPTION))
                                .line_height(px(16.))
                                .text_color(ui.subline())
                                .child(div().size(px(13.)).flex_shrink_0().child(Icon::from(Lucide::GitBranch)))
                                .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(if active {self.model.read(cx).review.as_ref().map(|r|format!("{}{}",r.branch,if r.dirty.is_empty(){""}else{" · ●"})).unwrap_or_else(||w.branch.clone())}else{w.branch.clone()})),
                        )
                        .child(model_history_stack(&history, &current, ui)),
                );
                if count > 1 {
                    let wid = w.id.clone();
                    group = group.child(
                        div()
                            .id(SharedString::from(format!("threads-{wid}")))
                            .flex()
                            .items_center()
                            .gap(px(Layout::SPACE_XS))
                            .h(px(26.))
                            .pl(px(GROUP_INDENT + 21.))
                            .pr(px(Layout::SPACE_SM))
                            .text_size(px(crate::theme::Type::CAPTION))
                            .text_color(ui.subline())
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.expanded.remove(&wid) { this.expanded.insert(wid.clone()); }
                                cx.notify();
                            }))
                            .child(div().size(px(13.)).child(Icon::from(if expanded { Lucide::ChevronDown } else { Lucide::ChevronRight })))
                            .child(format!("{count} conversations")),
                    );
                    if expanded {
                        for id in &w.threads {
                            if let Ok(id) = Uuid::parse_str(id) {
                                if self.model.read(cx).archived.contains(&id) { continue; }
                                if let Some(t) = self.model.read(cx).threads.get(&id).cloned() {
                                    group = group.child(self.thread_row(id, &t, "", self.model.read(cx).selected == Some(id), ui, cx));
                                }
                            }
                        }
                    }
                }
            }
        }
        for w in rows.iter().filter(|w| w.inline) {
            for id in &w.threads {
                if let Ok(id) = Uuid::parse_str(id) {
                    if self.model.read(cx).archived.contains(&id) || hidden.contains(&id.to_string()) { continue; }
                    if let Some(t) = self.model.read(cx).threads.get(&id).cloned() {
                        group = group.child(self.thread_row(id, &t, "", self.model.read(cx).selected == Some(id), ui, cx));
                    }
                }
            }
        }
        if hidden_count > 0 {
            let hover = ui.hover;
            group = group.child(
                div()
                    .id(SharedString::from(more_key.clone()))
                    .flex()
                    .items_center()
                    .gap(px(Layout::SPACE_XS))
                    .h(px(28.))
                    .ml(px(GROUP_INDENT - 6.))
                    .px(px(6.))
                    .rounded(px(6.))
                    .text_size(px(crate::theme::Type::SMALL))
                    .text_color(ui.text_muted)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded.remove(&more_key) { this.expanded.insert(more_key.clone()); }
                        cx.notify();
                    }))
                    .child(div().size(px(13.)).child(Icon::from(if show_all { Lucide::ChevronUp } else { Lucide::ChevronDown })))
                    .child(if show_all { "Show fewer threads".to_string() } else if hidden_count == 1 { "View 1 more thread".to_string() } else { format!("View {hidden_count} more threads") }),
            );
        }
        // Archived conversations of this project: a collapsed shelf.
        let archived_threads: Vec<Uuid> = {
            let m = self.model.read(cx);
            g.threads.iter().copied().filter(|id| m.archived.contains(id)).collect()
        };
        if !archived_threads.is_empty() {
            let key = format!("archived-threads:{}", g.root);
            let open = self.expanded.contains(&key);
            group = group.child(
                div()
                    .id(SharedString::from(key.clone()))
                    .flex()
                    .items_center()
                    .gap(px(Layout::SPACE_XS))
                    .h(px(28.))
                    .pl(px(GROUP_INDENT))
                    .pr(px(Layout::SPACE_SM))
                    .text_size(px(crate::theme::Type::CAPTION))
                    .text_color(ui.subline())
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded.remove(&key) { this.expanded.insert(key.clone()); }
                        cx.notify();
                    }))
                    .child(div().size(px(13.)).child(Icon::from(if open { Lucide::ChevronDown } else { Lucide::ChevronRight })))
                    .child(format!("{} archived", archived_threads.len())),
            );
            if open {
                for id in archived_threads {
                    if let Some(t) = self.model.read(cx).threads.get(&id).cloned() {
                        group = group.child(self.thread_row(id, &t, "", self.model.read(cx).selected == Some(id), ui, cx));
                    }
                }
            }
        }
        group
    }

    /// One conversation, Zeron's chat row: optional 11/14 caption line
    /// (project, when the list is flat), 13/17 title with the model's mark,
    /// time-ago or status in the corner, and an 11/14 branch line.
    fn thread_row(
        &self,
        id: Uuid,
        t: &Entity<ThreadModel>,
        project: &str,
        selected: bool,
        ui: &Ui,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tm = t.read(cx);
        let title = tm.title();
        let status = tm.meta.status.clone();
        let turn_active = tm.thread.presence.turn_active();
        let state = if status.contains("wait") || status.contains("approv") {
            "waiting"
        } else if turn_active || status == "running" {
            "running"
        } else if status == "failed" {
            "failed"
        } else {
            "idle"
        };
        let branch = tm
            .meta
            .worktree
            .as_deref()
            .and_then(|w| std::path::Path::new(w).file_name())
            .map(|s| s.to_string_lossy().to_string());
        // Server and Mac threads of one project sit together; say where each one runs.
        let thread_root = tm.meta.project_root.clone().unwrap_or_else(|| tm.meta.cwd.clone());
        let runs_on_server = crate::remote::is_server_root(&thread_root);
        let mixed = self.model.read(cx).project_links.iter().any(|l| l.local == thread_root || l.server == thread_root);
        let backend = tm.meta.backend.clone();
        let current = grok_persistence::ModelUsage { backend: backend.clone(), model: tm.meta.model.clone() };
        let history = tm.meta.models_used.clone();
        let updated = tm.meta.updated_at.clone();
        let model = self.model.clone();
        let hover = ui.hover;
        let selected_bg = ui.selected_bg();
        let corner = status_corner(state, &updated, ui);

        let has_worktree = tm.meta.worktree.is_some();
        let is_archived = self.model.read(cx).archived.contains(&id);
        let menu_model = self.model.clone();
        let current_title = title.clone();
        div()
            .id(SharedString::from(format!("thread-{id}")))
            .when(is_archived, |el| el.opacity(0.6))
            .flex()
            .flex_col()
            .gap(px(2.))
            .when(project.is_empty(), |el| el.ml(px(GROUP_INDENT - Layout::SPACE_SM)))
            .px(px(Layout::SPACE_SM))
            .py(px(6.))
            .rounded(px(8.))
            .cursor_pointer()
            .text_color(if selected { ui.text } else { Ui::alpha(ui.text, 0.8) })
            .when(selected, move |s| s.bg(selected_bg))
            .when(!selected, move |s| s.hover(move |s| s.bg(hover).text_color(ui.text)))
            .on_click(move |_, _, cx| {
                model.update(cx, |m, cx| m.select(Some(id), cx));
            })
            .context_menu(move |mut menu, _, _| {
                let m = menu_model.clone();
                let t = current_title.clone();
                menu = menu.item(PopupMenuItem::new("Rename thread…").on_click(move |_, window, cx| {
                    open_rename_dialog(m.clone(), id, t.clone(), window, cx);
                }));
                if has_worktree {
                    let m = menu_model.clone();
                    menu = menu.item(PopupMenuItem::new("Update from main").on_click(move |_, _, cx| {
                        m.update(cx, |a, cx| a.sync_thread(id, cx));
                    }));
                    let m = menu_model.clone();
                    menu = menu.item(PopupMenuItem::new("Review / Ship…").on_click(move |_, _, cx| {
                        m.update(cx, |a, cx| a.land_thread(id, cx));
                    }));
                }
                let m = menu_model.clone();
                menu = menu.item(PopupMenuItem::new("Reveal in Finder").on_click(move |_, _, cx| {
                    m.update(cx, |a, cx| {
                        a.select(Some(id), cx);
                        a.reveal_project(cx);
                    });
                }));
                thread_lifecycle_menu(menu, menu_model.clone(), id, is_archived)
            })
            .when(!project.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(Layout::SPACE_SM))
                        .h(px(14.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(crate::theme::Type::CAPTION))
                                .line_height(px(16.))
                                .text_color(ui.subline())
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(project.to_string()),
                        )
                        .child(corner),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(Layout::SPACE_SM))
                    .child(crate::views::brand::brand_mark(&backend, 13., true, ui))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(crate::theme::Type::BODY))
                            .line_height(px(20.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(title),
                    )
                    .when(mixed, |el| {
                        let (icon, hint) = if runs_on_server { (Lucide::Server, "Runs on the server") } else { (Lucide::Laptop, "Runs on this Mac") };
                        el.child(
                            div()
                                .id(SharedString::from(format!("where-{id}")))
                                .size(px(12.))
                                .flex_shrink_0()
                                .text_color(ui.text_faint)
                                .tooltip(move |window, cx| Tooltip::new(hint).build(window, cx))
                                .child(Icon::from(icon)),
                        )
                    })
                    .when(project.is_empty(), |el| el.child(status_corner(state, &updated, ui))),
            )
            .when_some(branch, |el, b| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(Layout::SPACE_XS))
                        .h(px(14.))
                        .text_size(px(crate::theme::Type::CAPTION))
                        .line_height(px(16.))
                        .text_color(ui.subline())
                        .child(div().size(px(13.)).flex_shrink_0().child(Icon::from(Lucide::GitBranch)))
                        .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(b)),
                )
            })
            .child(model_history_stack(&history, &current, ui))
    }

    /// Filtered rows under Today / Yesterday / This week / Earlier labels.
    fn search_results(&self, ui: &Ui, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let list = self.filtered(cx);
        let selected = self.model.read(cx).selected;
        let query = self.query(cx);
        if list.is_empty() {
            return vec![div()
                .px_4()
                .py_3()
                .text_size(px(crate::theme::Type::SMALL))
                .text_color(ui.text_faint)
                .child(format!("No thread matches \"{query}\""))
                .into_any_element()];
        }
        let mut out: Vec<AnyElement> = Vec::new();
        let mut last_group = "";
        for (ix, (id, t)) in list.iter().enumerate() {
            let (group, project) = {
                let tm = t.read(cx);
                (recency_group(&tm.meta.updated_at), project_name(tm.meta.project_root.as_deref().unwrap_or(&tm.meta.cwd)))
            };
            if group != last_group {
                out.push(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(Layout::SPACE_SM))
                        .h(px(28.))
                        .px(px(Layout::SPACE_SM))
                        .when(!last_group.is_empty(), |el| el.mt(px(Layout::SIDEBAR_SECTION_GAP - Layout::SIDEBAR_LIST_GAP)))
                        .child(div().text_size(px(crate::theme::Type::SMALL)).font_weight(FontWeight::MEDIUM).text_color(Ui::alpha(ui.text_muted, 0.5)).child(group))
                        .child(div().flex_1().h(px(1.)).bg(Ui::alpha(ui.border, 0.6)))
                        .into_any_element(),
                );
                last_group = group;
            }
            let row = self.thread_row(*id, t, &project, selected == Some(*id) || ix == self.active_ix, ui, cx);
            out.push(row.into_any_element());
        }
        out
    }

    /// Service row plus, when the account exposes them, its usage bars.
    fn service_block(&self, a: &BackendAuth, ui: &Ui, cx: &Context<Self>) -> impl IntoElement {
        let usage = if a.logged_in { self.model.read(cx).usage_for(&a.backend).cloned() } else { None };
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .child(self.service_row(a, ui))
            .when_some(usage, |el, u| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .mx_2()
                        .px_2()
                        .pl(px(30.))
                        .pb_1()
                        .children(u.windows.iter().enumerate().map(|(ix, w)| usage_bar(&a.backend, ix, w, ui)))
                        .when_some(u.error.clone(), |el, error| el.child(
                            div().text_size(px(crate::theme::Type::CAPTION)).text_color(ui.text_faint)
                                .child(if u.windows.is_empty() { error } else { format!("Last known usage · {error}") })
                        )),
                )
            })
    }

    fn service_row(&self, a: &BackendAuth, ui: &Ui) -> impl IntoElement {
        let model = self.model.clone();
        let backend = a.backend.clone();
        let logged_in = a.logged_in;
        let runnable = a.runnable;
        let dot = if logged_in {
            ui.success
        } else if runnable {
            ui.text_faint
        } else {
            ui.danger
        };
        let detail = if logged_in {
            a.account
                .clone()
                .or_else(|| a.plan.clone())
                .unwrap_or_else(|| "signed in".into())
        } else if runnable {
            "signed out · connect from Home".into()
        } else {
            "not installed".into()
        };
        let display = a.display_name.clone();
        let menu_model = model.clone();
        let menu_backend = backend.clone();
        div()
            .id(SharedString::from(format!("svc-{}", a.backend)))
            .flex()
            .items_center()
            .gap_2()
            .mx_2()
            .px_2()
            .py_1()
            .rounded(px(6.))
            .child(crate::views::brand::brand_mark(&a.backend, 14., logged_in, ui))
            .child(div().text_sm().text_color(ui.text).child(display.clone()))
            .child(div().size(px(6.)).rounded_full().bg(dot))
            .child(
                div()
                    .flex_1()
                    .text_size(px(crate::theme::Type::SMALL))
                    .text_color(ui.text_faint)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(detail),
            )
            .when(logged_in, |el| {
                el.child(
                    Button::new(SharedString::from(format!("svc-menu-{}", a.backend)))
                        .ghost()
                        .small()
                        .compact()
                        .label("…")
                        .dropdown_menu(move |menu, _, _| {
                            let m = menu_model.clone();
                            let b = menu_backend.clone();
                            let name = display.clone();
                            menu.item(PopupMenuItem::new(format!("Sign out of {name}…")).on_click(move |_, window, cx| {
                                let m = m.clone();
                                let b = b.clone();
                                let name = name.clone();
                                window.open_alert_dialog(cx, move |dlg, _, _| {
                                    let m = m.clone();
                                    let b = b.clone();
                                    dlg.title(format!("Sign out of {name}?"))
                                        .description("Threads on this backend cannot run until you sign in again.")
                                        .on_ok(move |_, _, cx| {
                                            m.update(cx, |a, cx| a.sign_out(&b, cx));
                                            true
                                        })
                                })
                            }))
                        }),
                )
            })
    }
}

impl Render for SidebarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::of(cx);
        let groups = self.model.read(cx).groups(cx);
        let active = self.model.read(cx).active_project.clone();
        if active != self.last_active {
            // Arriving in a project opens it, even if it was closed by hand earlier.
            if let Some(root) = &active { self.collapsed.remove(root); }
            self.last_active = active;
        }
        let auth = self.model.read(cx).auth.clone();
        let query = self.query(cx);
        div()
            .id("sidebar")
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if !this.search.read(cx).focus_handle(cx).is_focused(window) {
                    return;
                }
                let n = this.filtered(cx).len().max(1);
                match ev.keystroke.key.as_str() {
                    "down" => {
                        this.active_ix = (this.active_ix + 1) % n;
                        cx.stop_propagation();
                        cx.notify();
                    }
                    "up" => {
                        this.active_ix = (this.active_ix + n - 1) % n;
                        cx.stop_propagation();
                        cx.notify();
                    }
                    _ => {}
                }
            }))
            .child(
                div()
                    .px(px(Layout::SPACE_SM))
                    .pt(px(Layout::SPACE_SM))
                    .pb(px(Layout::SPACE_XS))
                    .child(
                        div()
                            .id("bomb-home")
                            .flex()
                            .items_center()
                            .gap(px(Layout::SPACE_SM))
                            .px(px(Layout::SPACE_SM))
                            .h(px(36.))
                            .rounded(px(8.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(ui.hover))
                            .tooltip(|window, cx| Tooltip::new("Home").build(window, cx))
                            .child(div().text_size(px(crate::theme::Type::TITLE)).child("💣"))
                            .child(
                                div()
                                    .font_family(ui.mono.clone())
                                    .text_size(px(crate::theme::Type::BODY))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ui.text)
                                    .child("Bomb Code"),
                            )
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(crate::actions::OpenHome), cx);
                            }),
                    ),
            )
            .child(div().flex().gap_1().px(px(Layout::SPACE_SM)).pb(px(Layout::SPACE_SM))
                .child({
                    // With a server paired, a project's home is a choice: that server (default) or this Mac.
                    let servers = crate::runtime::servers(cx).all();
                    let button = Button::new("sidebar-add-project").ghost().small().icon(Lucide::FolderPlus).label("Add project");
                    if servers.is_empty() {
                        button.on_click(|_, window, cx| window.dispatch_action(Box::new(crate::actions::OpenProject), cx)).into_any_element()
                    } else {
                        let app = self.model.clone();
                        button.dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
                            let self_app = app.clone();
                            for server in &servers {
                                let (app, id, name) = (app.clone(), server.config.id.clone(), server.config.name.clone());
                                menu = menu.item(PopupMenuItem::new(format!("New project on {}…", server.config.name)).on_click(move |_, window, cx| {
                                    open_server_project_dialog(app.clone(), id.clone(), name.clone(), window, cx)
                                }));
                                let (app, id, name) = (self_app.clone(), server.config.id.clone(), server.config.name.clone());
                                menu = menu.item(PopupMenuItem::new(format!("Clone from GitHub onto {}…", server.config.name)).on_click(move |_, window, cx| {
                                    open_server_clone_dialog(app.clone(), id.clone(), name.clone(), window, cx)
                                }));
                            }
                            let create = app.clone();
                            menu.separator()
                                .item(PopupMenuItem::new("New project on this Mac…").on_click(move |_, _, cx| create.update(cx, |m, cx| m.create_project(cx))))
                                .item(PopupMenuItem::new("Open a folder on this Mac…").on_click(|_, window, cx| window.dispatch_action(Box::new(crate::actions::OpenProject), cx)))
                        }).into_any_element()
                    }
                })
                .child(Button::new("sidebar-new-chat").ghost().small().icon(Lucide::Plus).label("New chat")
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(NewThread), cx))))
            .child(div().flex().items_center().gap_1().px(px(Layout::SPACE_SM)).pb(px(Layout::SPACE_XS))
                .child(div().flex_1().min_w_0().child(Input::new(&self.search).cleanable(true).appearance(true)))
                .child({
                    let app = self.model.clone();
                    let current = self.model.read(cx).sidebar_sort;
                    Button::new("sidebar-sort").ghost().small().icon(Lucide::ListFilter)
                        .tooltip(format!("Sort projects and threads · {}", current.label()))
                        .dropdown_menu(move |mut menu, _, _| {
                            menu = menu.item(PopupMenuItem::new("Sort by").disabled(true));
                            for sort in crate::models::app::SidebarSort::ALL {
                                let app = app.clone();
                                menu = menu.item(PopupMenuItem::new(sort.label()).checked(sort == current)
                                    .on_click(move |_, _, cx| app.update(cx, |m, cx| m.set_sidebar_sort(sort, cx))));
                            }
                            menu
                        })
                }))
            .child(
                div()
                    .id("thread-list")
                    .min_h_0()
                    .flex_1()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .px(px(Layout::SPACE_SM))
                    .pt(px(Layout::SPACE_XS))
                    .pb(px(Layout::SPACE_SM))
                    .map(|el| {
                        if query.is_empty() {
                            let pinned_roots = self.model.read(cx).pinned_projects.clone();
                            let (pinned, rest): (Vec<_>, Vec<_>) = groups.iter().partition(|g| pinned_roots.contains(&g.root));
                            let heading = |label: &'static str| {
                                div()
                                    .px(px(Layout::SPACE_SM))
                                    .pt(px(Layout::SPACE_XS))
                                    .text_size(px(crate::theme::Type::SMALL))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ui.text_faint)
                                    .child(label)
                            };
                            el.gap(px(Layout::SIDEBAR_SECTION_GAP))
                                .when(!pinned.is_empty(), |el| {
                                    el.child(heading("Pinned"))
                                        .children(pinned.iter().map(|g| self.group(g, &ui, cx)))
                                        .when(!rest.is_empty(), |el| el.child(heading("Projects")))
                                })
                                .children(rest.iter().map(|g| self.group(g, &ui, cx)))
                                .when(groups.iter().all(|g| g.threads.is_empty()), |el| {
                                    el.child(
                                        div()
                                            .px(px(Layout::SPACE_SM))
                                            .pb(px(Layout::SPACE_SM))
                                            .text_size(px(crate::theme::Type::SMALL))
                                            .text_color(ui.text_faint)
                                            .child(if self.model.read(cx).projects.is_empty() { "No projects yet — add a folder" } else { "No conversations yet — start a new chat" }),
                                    )
                                })
                        } else {
                            el.gap(px(Layout::SIDEBAR_LIST_GAP)).children(self.search_results(&ui, cx))
                        }
                    }),
            )
            .child(
                div()
                    .id("services-scroll")
                    .flex_shrink_0()
                    .max_h(window.viewport_size().height * 0.45)
                    .overflow_y_scroll()
                    .border_t_1()
                    .border_color(ui.border)
                    .py_2()
                    .child(
                        div()
                            .px_4()
                            .pb_1()
                            .text_size(px(crate::theme::Type::SMALL))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ui.text_faint)
                            .child("Services"),
                    )
                    .children(auth.iter().map(|a| self.service_block(a, &ui, cx))),
            )
    }
}

/// One usage window: label, thin track, percentage. Track turns amber past
/// 75% and red past 90%.
fn usage_bar(backend: &str, ix: usize, w: &bomb_core::usage::UsageWindow, ui: &Ui) -> impl IntoElement {
    let pct = w.used_pct.clamp(0.0, 100.0);
    let fill = if pct >= 90.0 {
        ui.danger
    } else if pct >= 75.0 {
        ui.warning
    } else {
        ui.text_muted
    };
    let resets = w.resets_at.map(|t| {
        let secs = (t - Utc::now()).num_seconds().max(0);
        if secs < 3600 {
            format!("resets in {}m", (secs / 60).max(1))
        } else if secs < 86_400 {
            format!("resets in {}h", secs / 3600)
        } else {
            format!("resets in {}d", secs / 86_400)
        }
    });
    let mut track = ui.border;
    track.a *= 0.9;
    div()
        .id(SharedString::from(format!("usage-{backend}-{ix}")))
        .flex()
        .items_center()
        .gap_2()
        .when_some(resets, |el, r| el.tooltip(move |window, cx| Tooltip::new(r.clone()).build(window, cx)))
        .child(div().w(px(56.)).flex_shrink_0().whitespace_nowrap().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child(w.label.clone()))
        .child(
            div()
                .flex_1()
                .h(px(4.))
                .rounded_full()
                .bg(track)
                .overflow_hidden()
                .child(div().h_full().rounded_full().w(relative(pct / 100.0)).bg(fill)),
        )
        .child(
            div()
                .w(px(40.))
                .flex_shrink_0()
                .text_size(px(crate::theme::Type::SMALL))
                .text_right()
                .text_color(ui.text_faint)
                .child(format!("{}%", pct.round() as i64)),
        )
}

/// Shared by ordinary rows, workspace rows and the open thread's overflow menu.
pub(super) fn thread_lifecycle_menu(mut menu: PopupMenu, model: Entity<AppModel>, id: Uuid, archived: bool) -> PopupMenu {
    let app = model.clone();
    menu = menu.separator().item(
        PopupMenuItem::new(if archived { "Unarchive thread" } else { "Archive thread" })
            .on_click(move |_, _, cx| app.update(cx, |m, cx| {
                if archived { m.unarchive_thread(id, cx); } else { m.archive_thread(id, cx); }
            })),
    );
    menu.item(PopupMenuItem::new("Delete thread…").on_click(move |_, window, cx| {
        let model = model.clone();
        after_layers_settle(window, cx, move |window, cx| confirm_delete(model, id, window, cx));
    }))
}

/// Two confirmations before a thread is gone for good: the second one
/// spells out that it is permanent.
fn confirm_delete(model: Entity<AppModel>, id: Uuid, window: &mut Window, cx: &mut App) {
    tracing::info!(%id, "delete: opening first confirmation");
    window.open_alert_dialog(cx, move |dlg, _, _| {
        let model = model.clone();
        dlg.confirm().title("Delete this thread?")
            .description("Archiving hides it instead and keeps everything. Deleting removes its transcript; the branch and files are kept.")
            .on_ok(move |_, window, cx| {
                tracing::info!(%id, "delete: first confirmation accepted");
                let model = model.clone();
                after_layers_settle(window, cx, move |window, cx| {
                    tracing::info!(%id, "delete: opening second confirmation");
                    window.open_alert_dialog(cx, move |dlg, _, _| {
                        let model = model.clone();
                        dlg.confirm().title("Permanently delete?")
                            .description("This cannot be undone.")
                            .on_ok(move |_, _, cx| {
                                tracing::info!(%id, "delete: second confirmation accepted");
                                model.update(cx, |a, cx| a.remove_thread(id, cx));
                                true
                            })
                    });
                });
                true
            })
    });
}

/// Run `f` once the current popup/dialog layer has finished closing (its
/// close is animated), so a dialog opened next is not popped with it.
fn after_layers_settle(window: &mut Window, cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    window
        .spawn(cx, async move |cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(260)).await;
            let _ = cx.update(|window, cx| f(window, cx));
        })
        .detach();
}

/// Row corner: time-ago for idle rows (10px medium, subline), otherwise a
/// glyph + status word in the status color.
fn status_corner(state: &str, updated_at: &str, ui: &Ui) -> AnyElement {
    let (word, color): (&str, Hsla) = match state {
        "waiting" => ("Input", ui.warning),
        "running" => ("Working", ui.accent),
        "failed" => ("Failed", ui.danger),
        _ => {
            return div()
                .flex_shrink_0()
                .h(px(14.))
                .text_size(px(crate::theme::Type::CAPTION))
                .line_height(px(16.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ui.subline())
                .child(time_ago(updated_at))
                .into_any_element();
        }
    };
    div()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(Layout::SPACE_XS))
        .h(px(14.))
        .child(div().size(px(6.)).rounded_full().bg(color))
        .child(div().text_size(px(crate::theme::Type::CAPTION)).line_height(px(16.)).font_weight(FontWeight::MEDIUM).text_color(color).child(word))
        .into_any_element()
}

/// "49m", "2d", "4w" from an RFC3339 timestamp.
pub fn time_ago(iso: &str) -> String {
    let Ok(t) = iso.parse::<DateTime<Utc>>() else {
        return String::new();
    };
    let secs = (Utc::now() - t).num_seconds().max(0);
    match secs {
        s if s < 60 => "now".into(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s if s < 7 * 86_400 => format!("{}d", s / 86_400),
        s if s < 5 * 7 * 86_400 => format!("{}w", s / (7 * 86_400)),
        s if s < 365 * 86_400 => format!("{}mo", s / (30 * 86_400)),
        s => format!("{}y", s / (365 * 86_400)),
    }
}

/// Rename a thread through a small dialog (inline editing needs focus
/// plumbing the sidebar rows do not have).
pub fn open_rename_dialog(model: Entity<AppModel>, id: Uuid, current: String, window: &mut Window, cx: &mut App) {
    let input = cx.new(|cx| {
        let mut s = InputState::new(window, cx).placeholder("Thread name");
        s.set_value(current.clone(), window, cx);
        s
    });
    let focus_input = input.clone();
    window.open_dialog(cx, move |dialog, window, cx| {
        let model = model.clone();
        let input = input.clone();
        let input_for_content = input.clone();
        focus_input.update(cx, |s, cx| s.focus(window, cx));
        dialog
            .title("Rename thread")
            .w(px(420.))
            .content(move |content, _, _| {
                content.child(Input::new(&input_for_content))
            })
            .on_ok(move |_, window, cx| {
                let label = input.read(cx).value().to_string();
                if !label.trim().is_empty() {
                    model.update(cx, |m, cx| m.rename_thread(id, label, cx));
                }
                window.close_dialog(cx);
                true
            })
    });
}

/// Name a new project that will live on a paired server.
pub fn open_server_project_dialog(model: Entity<AppModel>, server: String, server_name: String, window: &mut Window, cx: &mut App) {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("Project name"));
    let focus_input = input.clone();
    window.open_dialog(cx, move |dialog, window, cx| {
        let (model, server, input) = (model.clone(), server.clone(), input.clone());
        let input_for_content = input.clone();
        focus_input.update(cx, |s, cx| s.focus(window, cx));
        dialog
            .title(format!("New project on {server_name}"))
            .w(px(420.))
            .content(move |content, _, _| {
                content
                    .child(Input::new(&input_for_content))
                    .child(div().pt_2().text_size(px(crate::theme::Type::SMALL)).child("Its threads run on the server, so they keep going with this Mac closed."))
            })
            .on_ok(move |_, window, cx| {
                let name = input.read(cx).value().trim().to_string();
                if !name.is_empty() {
                    model.update(cx, |m, cx| m.create_server_project(server.clone(), name, cx));
                }
                window.close_dialog(cx);
                true
            })
    });
}

/// Paste a repository address; the server clones it.
pub fn open_server_clone_dialog(model: Entity<AppModel>, server: String, server_name: String, window: &mut Window, cx: &mut App) {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("https://github.com/you/project.git"));
    let focus_input = input.clone();
    window.open_dialog(cx, move |dialog, window, cx| {
        let (model, server, input) = (model.clone(), server.clone(), input.clone());
        let input_for_content = input.clone();
        focus_input.update(cx, |s, cx| s.focus(window, cx));
        dialog
            .title(format!("Clone onto {server_name}"))
            .w(px(480.))
            .content(move |content, _, _| {
                content
                    .child(Input::new(&input_for_content))
                    .child(div().pt_2().text_size(px(crate::theme::Type::SMALL)).child("The server downloads it directly. For a private repository, sign in to GitHub on the server first."))
            })
            .on_ok(move |_, window, cx| {
                let url = input.read(cx).value().trim().to_string();
                if !url.is_empty() {
                    model.update(cx, |m, cx| m.clone_on_server(server.clone(), url, cx));
                }
                window.close_dialog(cx);
                true
            })
    });
}

/// One mark per previously used model; tooltips distinguish models from the same provider.
fn model_history_stack(history: &[grok_persistence::ModelUsage], current: &grok_persistence::ModelUsage, ui: &Ui) -> AnyElement {
    let previous: Vec<_> = history.iter().filter(|item| *item != current).collect();
    if previous.is_empty() { return div().into_any_element(); }
    let names = previous.iter().map(|item| crate::views::brand::pretty_model(&item.model)).collect::<Vec<_>>().join(" · ");
    div().id("model-history").flex().items_center().pl(px(21.)).py_1()
        .tooltip(move |window, cx| Tooltip::new(format!("Previously used: {names}")).build(window, cx))
        .children(previous.iter().take(4).enumerate().map(|(i,item)| {
            let label = crate::views::brand::pretty_model(&item.model);
            div().id(("past-model", i)).size(px(20.)).rounded_full().border_1().border_color(ui.border).bg(ui.bg)
                .when(i > 0, |el| el.ml(px(-5.))).flex().items_center().justify_center()
                .child(crate::views::brand::brand_mark(&item.backend, 11., true, ui))
                .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
        }))
        .when(previous.len() > 4, |el| el.child(div().pl_1().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child(format!("+{}", previous.len()-4))))
        .into_any_element()
}
