//! Right panel: Preview (an embedded WebKit view of the dev server, like Codex's
//! preview pane), Processes running from the thread's folder, and the file tree.
//! Changes is its own panel and shares this panel's tab bar (`panel_tabs`).

use std::collections::{HashMap, HashSet};

use crate::views::button::Button;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::Sizable;
use gpui_kit::base::Selectable;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::models::app::{AppModel, RightPanelRequest, RightTab};
use crate::theme::Ui;

pub struct PreviewPanel {
    model: Entity<AppModel>,
    tab: RightTab,
    webview: Option<Entity<gpui_wry::WebView>>,
    loaded_url: Option<String>,
    /// Per project: the page open in Preview when it isn't Bomb Code's dev server (a link
    /// from the chat, a port from Processes). Follows navigation, so switching back restores it.
    pages: HashMap<String, String>,
    /// The project whose page the web view is showing.
    page_project: Option<String>,
    error: Option<String>,
    files: Entity<super::file_tree::FileTree>,
    project_root_override: Option<std::path::PathBuf>,
    /// Subagent cards showing their whole task.
    open_tasks: std::collections::HashSet<String>,
    context_root: Option<std::path::PathBuf>,
    /// Processes asked to quit; a second press force-quits.
    stopping: HashSet<u32>,
}

impl PreviewPanel {
    pub fn new(model: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        let files = cx.new(|_| super::file_tree::FileTree::new());
        Self {
            files,
            tab: RightTab::Preview,
            context_root: None,
            project_root_override: None,
            open_tasks: Default::default(),
            model,
            webview: None,
            loaded_url: None,
            pages: HashMap::new(),
            page_project: None,
            error: None,
            stopping: HashSet::new(),
        }
    }

    pub fn set_tab(&mut self, tab: RightTab, cx: &mut Context<Self>) {
        if tab != RightTab::Changes {
            self.tab = tab;
        }
        cx.notify();
    }

    /// Show `url` in the Preview tab and remember it as this project's page.
    pub fn browse(&mut self, url: String, cx: &mut Context<Self>) {
        if let Some(project) = self.project_key(cx) {
            self.pages.insert(project, url);
        }
        self.tab = RightTab::Preview;
        cx.notify();
    }

    fn project_key(&self, cx: &App) -> Option<String> {
        self.model.read(cx).active_project.clone()
    }

    fn page(&self, cx: &App) -> Option<String> {
        self.project_key(cx).and_then(|p| self.pages.get(&p).cloned())
    }

    fn set_page(&mut self, url: Option<String>, cx: &App) {
        let Some(project) = self.project_key(cx) else { return };
        match url {
            Some(url) => { self.pages.insert(project, url); }
            None => { self.pages.remove(&project); }
        }
    }

    /// Keep the remembered page in step with where the user has browsed to.
    fn follow_navigation(&mut self, cx: &App) {
        let (Some(project), Some(view)) = (self.page_project.clone(), self.webview.as_ref()) else { return };
        if !self.pages.contains_key(&project) { return; }
        if let Ok(current) = view.read(cx).raw().url() {
            if !current.is_empty() && current != "about:blank" && self.loaded_url.as_deref() != Some(current.as_str()) {
                self.loaded_url = Some(current.clone());
                self.pages.insert(project, current);
            }
        }
    }

    pub fn reveal_file(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        self.tab = RightTab::Files;
        self.sync_root(cx);
        let root = self.current_root(cx);
        self.files.update(cx, |tree, cx| tree.reveal(path, root, cx));
        cx.notify();
    }

    fn sync_root(&mut self, cx: &mut Context<Self>) {
        let root = self.current_root(cx);
        if root != self.context_root {
            self.context_root = root;
            self.files = cx.new(|_| super::file_tree::FileTree::new());
            // Another thread's folder: its stop requests don't carry over.
            self.stopping.clear();
        }
    }

    fn current_root(&self, cx: &App) -> Option<std::path::PathBuf> {
        if let Some(root)=&self.project_root_override {return Some(root.clone());}
        let m = self.model.read(cx);
        m.selected.and_then(|id| m.threads.get(&id)).map(|t| std::path::PathBuf::from(&t.read(cx).meta.cwd))
            .or_else(|| m.active_project.as_ref().map(std::path::PathBuf::from))
    }

    /// Point the webview at `url`, creating it on first use.
    fn ensure(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.loaded_url.as_deref() == Some(url) && self.webview.is_some() {
            return;
        }
        match &self.webview {
            Some(wv) => {
                wv.update(cx, |w, _| w.load_url(url));
            }
            // `bomb-media://` plays videos linked in the chat (see `media`).
            None => match wry::WebViewBuilder::new()
                .with_custom_protocol(super::media::SCHEME.into(), |_, request| super::media::serve(request))
                .with_url(url)
                .build_as_child(window)
            {
                Ok(raw) => {
                    let entity = cx.new(|cx| gpui_wry::WebView::new(raw, window, cx));
                    self.webview = Some(entity);
                    self.error = None;
                }
                Err(e) => {
                    self.error = Some(e.to_string());
                }
            },
        }
        self.loaded_url = Some(url.to_string());
    }

    fn toggle_server(&mut self,cx:&mut Context<Self>) {
        let Some(root)=self.project_root_override.clone() else {self.model.update(cx,|m,cx|m.dev_server_toggle(cx));return;};
        let state=crate::runtime::services(cx);let weak=cx.entity().downgrade();
        crate::runtime::spawn_service(cx,async move {
            let status=state.dev_server.status().await;
            if status.running {
                if status.cwd.as_deref()!=root.to_str(){return Err("Another workspace has a preview running. Stop it there first.".into());}
                bomb_core::services::stop_dev_server(&state).await
            } else {bomb_core::services::start_dev_server(&state,Some(root.to_string_lossy().into_owned()),None,Some(false)).await}
        },move|result,cx|{let _=weak.update(cx,|v,cx|{match result {Ok(status)=>{v.error=None;v.model.update(cx,|m,cx|{m.dev_server=Some(status);cx.notify();});},Err(e)=>v.error=Some(e)}cx.notify();});});
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        if let (Some(wv), Some(url)) = (&self.webview, self.loaded_url.clone()) {
            wv.update(cx, |w, _| w.load_url(&url));
        }
    }

    /// The pane is going away: the native browser view must go with it, since layout alone never hides it.
    pub fn hidden(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.webview.take() else { return };
        view.update(cx, |w, _| w.hide());
        self.loaded_url = None;
        cx.notify();
    }

    fn drop_webview(&mut self) {
        self.webview = None;
        self.loaded_url = None;
    }
}

/// The task in one paragraph, for a card: the whole prompt with its line breaks and headings
/// run together (the card shows two lines of it).
fn task_summary(task: &str) -> String {
    task.lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The right panel's tab bar, shared by this panel and Changes. Tabs and close go
/// through the model so the window decides which panel shows.
pub fn panel_tabs(active: RightTab, model: &Entity<AppModel>, ui: &Ui, cx: &App) -> Div {
    // Only count processes scanned for the folder in view, never the last one's.
    let running = { let m = model.read(cx); if m.processes_folder == m.process_folder(cx) { m.processes.len() } else { 0 } };
    let ask = |request: RightPanelRequest| {
        let model = model.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| model.update(cx, |m, cx| { m.right_panel_request = Some(request); cx.notify(); })
    };
    // Icons, with the open tab also named: fits any width, so nothing changes shape as the
    // panel is resized. Counts stay beside their icon; names are tooltips.
    let tab = |id: &'static str, name: &'static str, icon: Lucide, count: usize, which: RightTab| {
        let open = active == which;
        let tooltip = if count > 0 { format!("{name} · {count}") } else { name.to_string() };
        let button = Button::new(id).ghost().small().icon(icon).selected(open).on_click(ask(RightPanelRequest::Show(which)));
        match (open, count) {
            (true, 0) => button.label(name),
            (true, n) => button.label(format!("{name} {n}")),
            (false, 0) => button.tooltip(tooltip),
            (false, n) => button.label(n.to_string()).tooltip(tooltip),
        }
    };
    let working = model.read(cx).subagents_in_view().1.iter().filter(|s| s.state == "running").count();
    div()
        .flex()
        .items_center()
        .gap_1()
        .h(px(40.))
        .px_2()
        .flex_shrink_0()
        .border_b_1()
        .border_color(ui.border)
        .child(div().flex().items_center().gap_1().flex_1().min_w_0().overflow_hidden()
            .child(tab("tab-preview", "Preview", Lucide::Monitor, 0, RightTab::Preview))
            .child(tab("tab-processes", "Processes", Lucide::Activity, running, RightTab::Processes))
            .child(tab("tab-files", "Files", Lucide::FolderTree, 0, RightTab::Files))
            .child(tab("tab-changes", "Changes", Lucide::GitCompare, 0, RightTab::Changes))
            .child(tab("tab-subagents", "Subagents", Lucide::Bot, working, RightTab::Subagents)))
        .child(div().flex_shrink_0().child(Button::new("right-panel-close").ghost().small().icon(Lucide::X).tooltip("Close panel")
            .on_click(ask(RightPanelRequest::Close))))
}

fn running_for(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86_400 => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
        _ => format!("{}d {}h", secs / 86_400, secs % 86_400 / 3600),
    }
}

fn memory(kb: u64) -> String {
    if kb >= 1024 * 1024 { format!("{:.1} GB", kb as f64 / (1024.0 * 1024.0)) } else { format!("{} MB", kb / 1024) }
}

impl PreviewPanel {
    fn empty(message: impl Into<SharedString>, ui: &Ui) -> Div {
        div().max_w(px(320.)).text_sm().text_color(ui.text_muted).whitespace_normal().text_center().child(message.into())
    }

    fn render_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let ui = Ui::of(cx);
        // Leaving a project: save where its page had got to, and let the next project load its own.
        let project = self.project_key(cx);
        if self.page_project != project {
            self.follow_navigation(cx);
            self.drop_webview();
            self.page_project = project.clone();
        }
        // The dev server belongs to one folder; another project's server isn't this one's preview.
        let here = self.current_root(cx).map(|r| r.to_string_lossy().into_owned());
        let status = self.model.read(cx).dev_server.clone().filter(|s| {
            let cwd = s.cwd.as_deref().unwrap_or("");
            here.as_deref() == Some(cwd) || project.as_deref().is_some_and(|p| cwd == p || cwd.starts_with(&format!("{p}/")))
        });
        let running = status.as_ref().map(|s| s.running).unwrap_or(false);
        let server_url = status.as_ref().and_then(|s| s.url.clone()).filter(|_| running);
        // Bomb Code's own server wins; otherwise a page picked from Processes, while it still listens.
        let ports: Vec<(String, u16)> = self.model.read(cx).processes.iter()
            .flat_map(|p| p.ports.iter().map(move |port| (p.name.clone(), *port))).collect();
        self.follow_navigation(cx);
        let url = server_url.clone().or_else(|| self.page(cx));
        match &url {
            Some(u) => self.ensure(u, window, cx),
            None if self.webview.is_some() => self.drop_webview(),
            None => {}
        }
        // The native view takes its frame when the web view element is laid out; a cached
        // element keeps the old frame (clipped until a resize), so lay it out with the panel.
        if let Some(view) = &self.webview {
            view.update(cx, |_, cx| cx.notify());
        }

        let Some(url) = url else {
            // Nothing to show: no toolbar. Pages arrive as links from the agent, or from a
            // project process that is already listening.
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .p_4()
                .child(div().text_base().font_weight(FontWeight::MEDIUM).text_color(ui.text).child("Nothing to preview yet"))
                .child(Self::empty(match &self.error {
                    Some(e) => format!("Could not create the preview: {e}"),
                    None => "Links in the chat open here. Ask your agent to run the app and share its link.".into(),
                }, &ui))
                // A server started before this change may still be running without an address.
                .when(running, |el| el.child(Button::new("preview-server-toggle").small().outline().label("Stop server")
                    .on_click(cx.listener(|v, _, _, cx| v.toggle_server(cx)))))
                .when(!ports.is_empty(), |el| {
                    el.child(div().pt_2().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child("Running in this project:"))
                        .children(ports.iter().enumerate().map(|(i, (name, port))| {
                            let target = format!("http://localhost:{port}");
                            Button::new(SharedString::from(format!("preview-port-{i}"))).ghost().small()
                                .icon(Lucide::Globe).label(format!("{name} · localhost:{port}"))
                                .on_click(cx.listener(move |v, _, _, cx| v.browse(target.clone(), cx)))
                        }))
                })
                .into_any_element();
        };

        let icon_button = |id: &'static str, icon: Lucide, tip: &'static str| Button::new(id).ghost().small().icon(icon).tooltip(tip);
        let manual = server_url.is_none();
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(px(36.))
                    .px_2()
                    .border_b_1()
                    .border_color(ui.border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .text_size(px(crate::theme::Type::SMALL))
                            .font_family(ui.mono.clone())
                            .text_color(ui.text_muted)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(url.clone()),
                    )
                    .child(icon_button("preview-back", Lucide::ArrowLeft, "Back").on_click(cx.listener(|this, _, _, cx| {
                        if let Some(view) = &this.webview { view.update(cx, |w, _| { let _ = w.back(); }); }
                    })))
                    .child(icon_button("preview-reload", Lucide::RefreshCw, "Reload").on_click(cx.listener(|this, _, _, cx| this.reload(cx))))
                    .child(icon_button("preview-open", Lucide::ExternalLink, "Open in browser").on_click({
                        // Bomb Code's own server opens through the model, which also knows server projects.
                        let (url, app) = (url.clone(), self.model.clone());
                        move |_, _, cx| if manual { cx.open_url(&url) } else { app.update(cx, |m, cx| m.dev_server_open(cx)) }
                    }))
                    .child(if manual {
                        Button::new("preview-stop-viewing").ghost().small().label("Close preview")
                            .on_click(cx.listener(|v, _, _, cx| { v.set_page(None, cx); v.drop_webview(); cx.notify(); }))
                    } else {
                        Button::new("preview-server-toggle").ghost().small().label("Stop server")
                            .on_click(cx.listener(|v, _, _, cx| v.toggle_server(cx)))
                    }),
            )
            .child(div().flex_1().min_h_0().bg(ui.bg).when_some(self.webview.clone(), |el, wv| el.child(wv)))
            .into_any_element()
    }

    /// The open thread's subagents: what each was asked, how it's going, and a way into its transcript.
    fn render_subagents(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ui = Ui::of(cx);
        let (parent, list) = self.model.read(cx).subagents_in_view();
        let selected = self.model.read(cx).selected;
        if parent.is_none() || list.is_empty() {
            let (title, body) = if parent.is_none() {
                ("No thread open", "Open a thread to see the subagents it starts.")
            } else {
                ("No subagents yet", "When the agent hands work to subagents, each one shows here with what it was asked and how it's going. Ask for it in a prompt: \"use two subagents to…\"")
            };
            return div().flex_1().flex().flex_col().items_center().justify_center().gap_2().p_4()
                .child(div().text_base().font_weight(FontWeight::MEDIUM).text_color(ui.text).child(title))
                .child(Self::empty(body, &ui))
                .into_any_element();
        }
        let hover = ui.hover;
        let selected_bg = ui.selected_bg();
        let working = list.iter().filter(|s| s.state == "running").count();
        let summary = match (working, list.len()) {
            (0, 1) => "1 subagent, finished".to_string(),
            (0, n) => format!("{n} subagents, all finished"),
            (w, n) => format!("{w} of {n} working"),
        };
        div()
            .id("subagent-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .p_2()
            .gap_2()
            .child(div().px_1().text_size(px(crate::theme::Type::CAPTION)).text_color(ui.text_faint).child(summary))
            .children(list.into_iter().filter_map(|info| {
                let child = uuid::Uuid::parse_str(&info.id).ok()?;
                let (icon, color, word) = match info.state.as_str() {
                    "running" => (Lucide::CircleDot, ui.accent, "Working".to_string()),
                    "completed" => (Lucide::Check, ui.success, format!("Finished {}", crate::views::sidebar::time_ago(info.ended_at.as_deref().unwrap_or(&info.started_at)))),
                    "cancelled" => (Lucide::X, ui.text_faint, "Stopped".to_string()),
                    _ => (Lucide::X, ui.danger, "Failed".to_string()),
                };
                let is_open = selected == Some(child);
                let model = self.model.clone();
                let task_open = self.open_tasks.contains(&info.id);
                let panel = cx.entity().downgrade();
                let task_id = info.id.clone();
                Some(
                    div()
                        .id(SharedString::from(format!("subagent-card-{child}")))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .w_full()
                        .min_w_0()
                        .overflow_hidden()
                        .p_3()
                        .rounded(px(12.))
                        .border_1()
                        .border_color(if is_open { ui.accent } else { ui.border })
                        .cursor_pointer()
                        .when(is_open, move |el| el.bg(selected_bg))
                        .when(!is_open, move |el| el.hover(move |s| s.bg(hover)))
                        .on_click(move |_, _, cx| model.update(cx, |m, cx| m.open_subagent(child, cx)))
                        .child(
                            div().flex().items_center().gap_2()
                                .child(div().size(px(12.)).flex_shrink_0().text_color(color).child(gpui_kit::component::Icon::from(icon)))
                                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().font_weight(FontWeight::MEDIUM).text_color(ui.text).child(info.name.clone()))
                                .child(div().flex_shrink_0().text_size(px(crate::theme::Type::CAPTION)).text_color(if info.state == "running" { ui.accent } else { ui.text_faint }).child(word)),
                        )
                        .when(info.model.is_some() || info.tokens.is_some(), |el| {
                            let facts = [info.model.clone(), info.tokens.map(|t| format!("{} tokens", bomb_core::presence::format_count(t as usize)))]
                                .into_iter().flatten().collect::<Vec<_>>().join(" · ");
                            el.child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().font_family(ui.mono.clone()).text_size(px(crate::theme::Type::CAPTION)).text_color(ui.text_faint).child(facts))
                        })
                        .when(!info.task.is_empty(), |el| {
                            let text = div().w_full().min_w_0().text_size(px(crate::theme::Type::SMALL)).line_height(px(18.)).text_color(ui.text_muted);
                            el.child(if task_open {
                                text.whitespace_normal().child(info.task.clone())
                            } else {
                                text.line_clamp(2).child(task_summary(&info.task))
                            })
                            .child(
                                div()
                                    .id(SharedString::from(format!("subagent-task-{child}")))
                                    .text_size(px(crate::theme::Type::CAPTION))
                                    .text_color(ui.text_faint)
                                    .cursor_pointer()
                                    .hover(|s| s.underline())
                                    // Toggling doesn't open the subagent's thread.
                                    .on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        let _ = panel.update(cx, |p, cx| {
                                            if !p.open_tasks.remove(&task_id) { p.open_tasks.insert(task_id.clone()); }
                                            cx.notify();
                                        });
                                    })
                                    .child(if task_open { "Hide task" } else { "Show task" }),
                            )
                        })
                        .into_any_element(),
                )
            }))
            .into_any_element()
    }

    fn render_processes(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let ui = Ui::of(cx);
        let (folder, scanned, list) = {
            let m = self.model.read(cx);
            (m.process_folder(cx), m.processes_folder.clone(), m.processes.clone())
        };
        self.stopping.retain(|pid| list.iter().any(|p| p.pid == *pid));
        let centered = |title: &'static str, body: &'static str| {
            div().flex_1().flex().flex_col().items_center().justify_center().gap_2().p_4()
                .child(div().text_base().font_weight(FontWeight::MEDIUM).text_color(ui.text).child(title))
                .child(Self::empty(body, &ui))
                .into_any_element()
        };
        if folder.is_none() {
            return centered("No folder here", "Processes are listed for threads and projects on this Mac.");
        }
        if scanned != folder {
            return centered("Checking…", "Looking for processes running from this thread's folder.");
        }
        if list.is_empty() {
            return centered("Nothing running", "Apps, dev servers and watchers started from this thread's folder show up here, so you can stop them.");
        }
        let hover = ui.hover;
        div()
            .id("process-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .p_2()
            .gap_1()
            .children(list.into_iter().map(|p| {
                let pid = p.pid;
                let asked = self.stopping.contains(&pid);
                let preview_url = p.ports.first().map(|port| format!("http://localhost:{port}"));
                let facts = format!("running {} · {:.0}% CPU · {}", running_for(p.running_secs), p.cpu_percent, memory(p.memory_kb));
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .px_2()
                    .py(px(8.))
                    .rounded(px(8.))
                    .hover(move |s| s.bg(hover))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div().flex().items_center().gap_2().min_w_0()
                                    .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap()
                                        .text_size(px(crate::theme::Type::BODY)).font_weight(FontWeight::MEDIUM).text_color(ui.text).child(p.name.clone()))
                                    .children(p.ports.iter().map(|port| {
                                        div().flex_shrink_0().px(px(6.)).rounded(px(4.)).bg(ui.hover)
                                            .text_size(px(crate::theme::Type::CAPTION)).font_family(ui.mono.clone()).text_color(ui.text_muted)
                                            .child(format!(":{port}"))
                                    })),
                            )
                            .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap()
                                .text_size(px(crate::theme::Type::CAPTION)).font_family(ui.mono.clone()).text_color(ui.text_faint).child(p.command.clone()))
                            .child(div().text_size(px(crate::theme::Type::CAPTION)).text_color(ui.subline()).child(facts)),
                    )
                    .child(
                        div().flex().items_center().gap_1().flex_shrink_0()
                            .when_some(preview_url, |el, url| {
                                el.child(Button::new(SharedString::from(format!("process-preview-{pid}"))).ghost().small().label("Preview")
                                    .on_click(cx.listener(move |v, _, _, cx| v.browse(url.clone(), cx))))
                            })
                            .child(Button::new(SharedString::from(format!("process-stop-{pid}"))).outline().small()
                                .label(if asked { "Force quit" } else { "Stop" })
                                .tooltip(if asked { "It hasn't quit yet. Kill it now." } else { "Ask it to quit" })
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    let force = v.stopping.contains(&pid);
                                    v.stopping.insert(pid);
                                    v.model.update(cx, |m, cx| m.stop_process(pid, force, cx));
                                    cx.notify();
                                }))),
                    )
            }))
            .into_any_element()
    }
}

impl Render for PreviewPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_root(cx);
        let ui = Ui::of(cx);
        // The native browser view only lives while the Preview tab shows.
        if self.tab != RightTab::Preview && self.webview.is_some() {
            self.drop_webview();
        }
        let body = match self.tab {
            RightTab::Files => {
                if let Some(root) = self.current_root(cx) { self.files.update(cx, |tree, cx| tree.set_root(root, cx)); }
                div().flex_1().min_h_0().child(self.files.clone()).into_any_element()
            }
            RightTab::Processes => self.render_processes(cx),
            RightTab::Subagents => self.render_subagents(cx),
            RightTab::Preview | RightTab::Changes => self.render_preview(window, cx),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(ui.border)
            .child(panel_tabs(self.tab, &self.model, &ui, cx))
            .child(body)
    }
}
