//! Left column: threads (project · time-ago / title / branch) and the
//! connected services footer.

use chrono::{DateTime, Utc};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenuItem};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Sizable, WindowExt};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use grok_cli_wrapper::BackendAuth;
use uuid::Uuid;

use crate::actions::NewThread;
use crate::models::app::{project_name, AppModel, ProjectGroup};
use crate::models::thread::ThreadModel;
use crate::theme::Ui;

pub struct SidebarView {
    model: Entity<AppModel>,
    search: Entity<InputState>,
    /// Keyboard cursor through the filtered list.
    active_ix: usize,
    collapsed: std::collections::HashSet<String>,
    expanded: std::collections::HashSet<String>,
}

/// Recency bucket for thread search results.
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
        Self { model, search, active_ix: 0, collapsed: Default::default(), expanded: Default::default() }
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

    fn header(&self, ui: &Ui, cx: &mut Context<Self>) -> impl IntoElement {
        let hover = ui.hover;
        let projects = self.model.read(cx).projects.clone();
        let model = self.model.clone();
        let project = self
            .model
            .read(cx)
            .active_project
            .as_deref()
            .map(project_name)
            .unwrap_or_else(|| "All projects".into());
        div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(40.))
            .px_3()
            .child(
                div()
                    .size(px(14.))
                    .text_color(ui.text_muted)
                    .child(Icon::from(Lucide::Folder)),
            )
            .child(
                Button::new("project-menu")
                    .ghost()
                    .small()
                    .compact()
                    .label(project)
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut menu, _, _| {
                        for p in &projects {
                            let m = model.clone();
                            let root = p.clone();
                            menu = menu.item(PopupMenuItem::new(project_name(p)).on_click(move |_, _, cx| {
                                m.update(cx, |a, cx| a.set_active_project(root.clone(), cx));
                            }));
                        }
                        menu = menu.separator();
                        let m = model.clone();
                        menu.item(PopupMenuItem::new("Open project…").on_click(move |_, _, cx| {
                            m.update(cx, |a, cx| a.open_project(cx));
                        }))
                    }),
            )
            .child(
                div()
                    .id("new-thread")
                    .size(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .text_color(ui.text_muted)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(|_, _, window, cx| {
                        window.dispatch_action(Box::new(NewThread), cx);
                    }))
                    .child(div().size(px(14.)).child(Icon::from(Lucide::Plus))),
            )
    }

    fn group(&self, g: &ProjectGroup, ui: &Ui, cx: &mut Context<Self>) -> impl IntoElement {
        let collapsed = self.collapsed.contains(&g.root);
        let root = g.root.clone(); let root2 = root.clone();
        let app = self.model.clone(); let add = app.clone(); let add_root = root.clone();
        let rows: Vec<_> = self.model.read(cx).workspaces.iter().filter(|w| w.project_root == root).cloned().collect();
        let mut group = div().flex().flex_col().gap_1().child(
            div().flex().items_center().px_3().py_2().gap_2()
                .child(Button::new(SharedString::from(format!("collapse-{root}"))).ghost().xsmall().label(if collapsed { "▸" } else { "▾" }).on_click(cx.listener(move |this, _, _, cx| {
                    if !this.collapsed.remove(&root2) { this.collapsed.insert(root2.clone()); } cx.notify();
                })))
                .child(div().id(SharedString::from(format!("project-{root}"))).flex_1().text_sm().cursor_pointer().child(g.name.clone()).on_click(move |_, _, cx| app.update(cx, |m, cx| m.set_active_project(root.clone(), cx))))
                .child(Button::new(SharedString::from(format!("new-{}", g.root))).ghost().xsmall().label("+").on_click(move |_, _, cx| add.update(cx, |m, cx| { m.set_active_project(add_root.clone(), cx); m.new_thread(cx); })))
        );
        if let Some(status) = self.model.read(cx).project_status.get(&g.root) {
            group = group.child(div().px_4().text_xs().text_color(ui.text_faint).child(if status.error.is_some() { "Folder".into() } else { format!("{} · ↑{} ↓{}{}", status.branch, status.ahead, status.behind, if status.dirty { " · uncommitted" } else { "" }) }));
        }
        if collapsed { return group; }
        for archived in [false, true] {
            let archived_key = format!("archived:{}", g.root);
            let archive_open = self.expanded.contains(&archived_key);
            if archived && rows.iter().any(|w| w.archived_at.is_some()) {
                group = group.child(Button::new(SharedString::from(archived_key.clone())).ghost().small().label(if archive_open { "▾ Archived" } else { "▸ Archived" }).on_click(cx.listener(move |this, _, _, cx| {
                    if !this.expanded.remove(&archived_key) { this.expanded.insert(archived_key.clone()); } cx.notify();
                })));
            }
            if archived && !archive_open { continue; }
            for w in rows.iter().filter(|w| !w.inline && w.archived_at.is_some() == archived) {
                let app = self.model.clone(); let wid = w.id.clone();
                let active = self.model.read(cx).active_workspace.as_deref() == Some(&w.id);
                let title = w.name.clone(); let menu_model = self.model.clone(); let menu_id = wid.clone();
                let count = w.threads.len(); let expanded = self.expanded.contains(&wid);
                let status = w.threads.iter().filter_map(|t| Uuid::parse_str(t).ok()).filter_map(|t| self.model.read(cx).threads.get(&t)).map(|t| t.read(cx).meta.status.clone()).find(|s| matches!(s.as_str(), "running" | "waiting_approval" | "failed")).unwrap_or_else(|| "idle".into());
                let mut providers: Vec<String> = Vec::new();
                let mut models: Vec<String> = Vec::new();
                for tid in &w.threads {
                    if let Some(t) = Uuid::parse_str(tid).ok().and_then(|id| self.model.read(cx).threads.get(&id)) {
                        let meta = &t.read(cx).meta;
                        if !providers.contains(&meta.backend) { providers.push(meta.backend.clone()); }
                        if !models.contains(&meta.model) { models.push(meta.model.clone()); }
                    }
                }
                let model_label = models.join(" · ");
                let dot = match status.as_str() { "running" => ui.accent, "waiting_approval" => ui.warning, "failed" => ui.danger, _ => ui.text_faint };
                group = group.child(div().id(SharedString::from(format!("workspace-{wid}"))).mx_2().px_3().py_2().rounded(px(8.)).cursor_pointer().when(active, |el| el.bg(ui.active)).hover(|s| s.bg(ui.hover))
                    .on_click(move |_, _, cx| app.update(cx, |m, cx| m.open_workspace(wid.clone(), cx)))
                    .tooltip(move |window, cx| Tooltip::new(model_label.clone()).build(window, cx))
                    .context_menu(move |menu, _, _| {
                        let app = menu_model.clone(); let wid = menu_id.clone(); let name = title.clone();
                        menu.item(PopupMenuItem::new("Rename workspace…").on_click(move |_, window, cx| crate::views::workspaces::text_action(app.clone(), wid.clone(), "rename", "Workspace name", name.clone(), window, cx)))
                    })
                    .child(div().flex().gap_2().items_center()
                        .children(providers.iter().map(|provider| crate::views::brand::brand_mark(provider, 14., true, ui)))
                        .child(div().flex_1().min_w_0().text_sm().overflow_hidden().text_ellipsis().whitespace_nowrap().child(w.name.clone()))
                        .child(div().size(px(6.)).rounded_full().bg(dot)))
                    .child(div().text_xs().text_color(ui.text_faint).child(w.branch.clone())));
                if count > 1 {
                    let wid = w.id.clone();
                    group = group.child(Button::new(SharedString::from(format!("threads-{wid}"))).ghost().xsmall().label(format!("{} {count} conversations", if expanded { "▾" } else { "▸" })).on_click(cx.listener(move |this, _, _, cx| {
                        if !this.expanded.remove(&wid) { this.expanded.insert(wid.clone()); } cx.notify();
                    })));
                    if expanded {
                        for id in &w.threads {
                            if let Ok(id) = Uuid::parse_str(id) {
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
                    if let Some(t) = self.model.read(cx).threads.get(&id).cloned() {
                        group = group.child(self.thread_row(id, &t, "", self.model.read(cx).selected == Some(id), ui, cx));
                    }
                }
            }
        }
        group
    }

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
        let waiting = status.contains("wait") || status.contains("approv");
        let working = turn_active || status == "running";
        let branch = tm
            .meta
            .worktree
            .as_deref()
            .and_then(|w| std::path::Path::new(w).file_name())
            .map(|s| s.to_string_lossy().to_string());
        let ago = time_ago(&tm.meta.updated_at);
        let backend = tm.meta.backend.clone();
        let model = self.model.clone();
        let (hover, active) = (ui.hover, ui.active);

        let corner: AnyElement = if waiting {
            div()
                .text_xs()
                .text_color(ui.warning)
                .child("Input")
                .into_any_element()
        } else if working {
            div()
                .text_xs()
                .text_color(ui.accent)
                .child("Working")
                .into_any_element()
        } else {
            div()
                .text_xs()
                .text_color(ui.text_faint)
                .child(ago)
                .into_any_element()
        };

        let has_worktree = tm.meta.worktree.is_some();
        let menu_model = self.model.clone();
        let current_title = title.clone();
        div()
            .id(SharedString::from(format!("thread-{id}")))
            .flex()
            .flex_col()
            .gap_0p5()
            .mx_2()
            .px_2()
            .py_2()
            .rounded(px(8.))
            .cursor_pointer()
            .when(selected, move |s| s.bg(active))
            .hover(move |s| s.bg(hover))
            .on_click(move |_, _, cx| {
                model.update(cx, |m, cx| m.select(Some(id), cx));
            })
            .context_menu(move |mut menu, _, _| {
                let m = menu_model.clone();
                let t = current_title.clone();
                menu = menu.item(PopupMenuItem::new("Rename…").on_click(move |_, window, cx| {
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
                menu = menu.separator();
                let m = menu_model.clone();
                menu.item(PopupMenuItem::new("Delete thread…").on_click(move |_, window, cx| {
                    let m = m.clone();
                    window.open_alert_dialog(cx, move |dlg, _, _| {
                        let m = m.clone();
                        dlg.title("Delete this thread?")
                            .description("Its transcript is removed. The workspace and files are kept. This cannot be undone.")
                            .on_ok(move |_, _, cx| {
                                m.update(cx, |a, cx| a.remove_thread(id, cx));
                                true
                            })
                    });
                }))
            })
            .when(!project.is_empty(), |el| el.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.))
                            .text_color(ui.text_faint)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(project.to_string()),
                    )
                    .child(div().text_size(px(11.)).child(corner)),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(crate::views::brand::brand_mark(&backend, 14., true, ui))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.))
                            .text_color(ui.text)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(title),
                    )
                    .when(project.is_empty(), |el| el.child(div().text_size(px(11.)).text_color(if waiting { ui.warning } else if working { ui.accent } else { ui.text_faint }).child(if waiting { "Input".into() } else if working { "Working".into() } else { time_ago(&tm.meta.updated_at) }))),
            )
            .when_some(branch, |el, b| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_size(px(11.))
                        .text_color(ui.text_faint)
                        .child(div().size(px(11.)).child(Icon::from(Lucide::GitBranch)))
                        .child(b),
                )
            })
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
                .text_xs()
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
                        .px_4()
                        .pt_2()
                        .pb_1()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ui.text_faint)
                        .child(group)
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
                        .children(u.windows.iter().enumerate().map(|(ix, w)| usage_bar(&a.backend, ix, w, ui))),
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
            "click to sign in".into()
        } else {
            "not installed".into()
        };
        let hover = ui.hover;
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
            .when(!logged_in && runnable, |el| {
                el.cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(move |_, window, cx| {
                        if backend == "grok" {
                            crate::views::login_dialog::open_login_dialog(model.clone(), window, cx);
                        } else {
                            model.update(cx, |m, cx| m.sign_in(&backend, cx));
                        }
                    })
            })
            .child(crate::views::brand::brand_mark(&a.backend, 14., logged_in, ui))
            .child(div().text_sm().text_color(ui.text).child(display.clone()))
            .child(div().size(px(6.)).rounded_full().bg(dot))
            .child(
                div()
                    .flex_1()
                    .text_xs()
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
                        .xsmall()
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::of(cx);
        let groups = self.model.read(cx).groups(cx);
        let auth = self.model.read(cx).auth.clone();
        let query = self.query(cx);
        div()
            .id("sidebar")
            .size_full()
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
            .child(self.header(&ui, cx))
            .child(Button::new("open-project-sidebar").ghost().small().label("+ Open project…").on_click({ let app = self.model.clone(); move |_, _, cx| app.update(cx, |m, cx| m.open_project(cx)) }))
            .child(div().px_3().pb_2().child(Input::new(&self.search).cleanable(true).appearance(true)))
            .child(
                div()
                    .id("thread-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .py_1()
                    .map(|el| {
                        if query.is_empty() {
                            el.children(groups.iter().map(|g| self.group(g, &ui, cx)))
                                .when(groups.iter().all(|g| g.threads.is_empty()), |el| {
                                    el.child(
                                        div()
                                            .px_4()
                                            .py_3()
                                            .text_xs()
                                            .text_color(ui.text_faint)
                                            .child("No threads yet. Press + to start one."),
                                    )
                                })
                        } else {
                            el.children(self.search_results(&ui, cx))
                        }
                    }),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(ui.border)
                    .py_2()
                    .child(
                        div()
                            .px_4()
                            .pb_1()
                            .text_xs()
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
        .child(div().w(px(40.)).text_xs().text_color(ui.text_faint).child(w.label.clone()))
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
                .w(px(32.))
                .text_xs()
                .text_right()
                .text_color(ui.text_faint)
                .child(format!("{}%", pct.round() as i64)),
        )
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
        s if s < 30 * 86_400 => format!("{}w", s / (7 * 86_400)),
        s => format!("{}mo", s / (30 * 86_400)),
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
