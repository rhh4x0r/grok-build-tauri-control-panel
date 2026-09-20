//! Dev-server preview: an embedded WebKit view docked to the right of the
//! thread, pointed at the dev server's URL (like Codex's preview pane).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::Icon;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::Sizable;
use gpui_kit::base::Selectable;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::ToggleDevPreview;
use crate::models::app::AppModel;
use crate::theme::Ui;

pub struct PreviewPanel {
    model: Entity<AppModel>,
    webview: Option<Entity<gpui_wry::WebView>>,
    loaded_url: Option<String>,
    error: Option<String>,
    files: Entity<super::file_tree::FileTree>,
    show_files: bool,
    project_root_override: Option<std::path::PathBuf>,
    context_root: Option<std::path::PathBuf>,
}

impl PreviewPanel {
    pub fn new(model: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        let files = cx.new(|_| super::file_tree::FileTree::new());
        Self {
            files,
            show_files: false,
            context_root: None,
            project_root_override: None,
            model,
            webview: None,
            loaded_url: None,
            error: None,
        }
    }

    pub fn set_project_root(&mut self, root: std::path::PathBuf, cx: &mut Context<Self>) {
        self.project_root_override=Some(root); self.webview=None;self.loaded_url=None;self.sync_root(cx);cx.notify();
    }

    pub fn reveal_file(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        self.show_files = true;
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
            None => match wry::WebViewBuilder::new().with_url(url).build_as_child(window) {
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
}

impl Render for PreviewPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_root(cx);
        let ui = Ui::of(cx);
        let status = self.model.read(cx).dev_server.clone().filter(|s| self.project_root_override.as_ref().is_none_or(|r| s.cwd.as_deref()==r.to_str()));
        let running = status.as_ref().map(|s| s.running).unwrap_or(false);
        let url = status.as_ref().and_then(|s| s.url.clone()).filter(|_| running);
        let message = status.as_ref().map(|s| s.message.clone()).unwrap_or_default();
        if self.show_files {
            self.webview = None;
            self.loaded_url = None;
        } else if let Some(u) = &url {
            self.ensure(u, window, cx);
        } else if self.webview.is_some() {
            // Server stopped: drop the native view (hides it).
            self.webview = None;
            self.loaded_url = None;
        }
        let hover = ui.hover;
        let app = self.model.clone();
        let icon_button = move |id: &'static str, icon: Lucide, muted: Hsla| {
            div()
                .id(id)
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .text_color(muted)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(div().size(px(13.)).child(Icon::from(icon)))
        };

        let tabs = div().flex().items_center().gap_1().p_2().border_b_1().border_color(ui.border)
            .child(Button::new("dev-tab").ghost().small().label("Dev server").selected(!self.show_files)
                .on_click(cx.listener(|this, _, _, cx| { this.show_files = false; cx.notify(); })))
            .child(Button::new("files-tab").ghost().small().label("File tree").selected(self.show_files)
                .on_click(cx.listener(|this, _, _, cx| { this.show_files = true; cx.notify(); })))
            .child(div().flex_1())
            .when(self.project_root_override.is_none(),|el|el.child(icon_button("sidebar-close", Lucide::X, ui.text_muted).on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleDevPreview), cx))));
        if self.show_files {
            if let Some(root) = self.current_root(cx) { self.files.update(cx, |tree, cx| tree.set_root(root, cx)); }
            return div().size_full().flex().flex_col().border_l_1().border_color(ui.border).child(tabs)
                .child(div().flex_1().min_h_0().child(self.files.clone())).into_any_element();
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(tabs)
            .border_l_1()
            .border_color(ui.border)
            .when(self.project_root_override.is_none(),|el|el.child(Button::new("preview-changes-tab").ghost().small().label("Changes").on_click({
                let app = app.clone(); move |_, _, cx| app.update(cx, |m, cx| { m.review_open = true; m.refresh_review(cx); cx.notify(); })
            })))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .h(px(40.))
                    .px_2()
                    .border_b_1()
                    .border_color(ui.border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .text_xs()
                            .font_family(ui.mono.clone())
                            .text_color(ui.text_muted)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(url.clone().unwrap_or_else(|| "no dev server running".into())),
                    )
                    .child(icon_button("preview-reload", Lucide::RefreshCw, ui.text_muted).on_click(
                        cx.listener(|this, _, _, cx| this.reload(cx)),
                    ))
                    .child(icon_button("preview-open", Lucide::ExternalLink, ui.text_muted).on_click({
                        let url=url.clone();let app=app.clone();let candidate=self.project_root_override.is_some();
                        move |_, _, cx| {if candidate {if let Some(url)=&url {cx.open_url(url);}}else{app.update(cx,|m,cx|m.dev_server_open(cx));}}
                    }))
                    .child(Button::new("preview-server-toggle").ghost().small()
                        .label(if running { "Stop server" } else { "Start server" })
                        .on_click(cx.listener(|v,_,_,cx|v.toggle_server(cx))))
                    .when(self.project_root_override.is_none(),|el|el.child(icon_button("preview-close", Lucide::X, ui.text_muted).on_click(
                        |_, window, cx| window.dispatch_action(Box::new(ToggleDevPreview), cx),
                    ))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .bg(ui.bg)
                    .when_some(self.webview.clone(), |el, wv| el.child(wv))
                    .when(self.webview.is_none(), |el| {
                        el.flex().items_center().justify_center().child(
                            div()
                                .max_w(px(320.))
                                .text_sm()
                                .text_color(ui.text_muted)
                                .whitespace_normal()
                                .child(match &self.error {
                                    Some(e) => format!("Could not create the preview: {e}"),
                                    None if running => "Waiting for the server to report its URL…".into(),
                                    None => if message.is_empty() {
                                        "Click Start server above to preview your app here.".into()
                                    } else {
                                        message
                                    },
                                }),
                        )
                    }),
            ).into_any_element()
    }
}
