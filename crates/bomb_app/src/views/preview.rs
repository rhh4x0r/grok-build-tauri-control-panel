//! Dev-server preview: an embedded WebKit view docked to the right of the
//! thread, pointed at the dev server's URL (like Codex's preview pane).

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::ToggleDevPreview;
use crate::models::app::{AppModel, ToastKind};
use crate::theme::Ui;

pub struct PreviewPanel {
    model: Entity<AppModel>,
    webview: Option<Entity<gpui_wry::WebView>>,
    loaded_url: Option<String>,
    error: Option<String>,
}

impl PreviewPanel {
    pub fn new(model: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self {
            model,
            webview: None,
            loaded_url: None,
            error: None,
        }
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

    fn reload(&mut self, cx: &mut Context<Self>) {
        if let (Some(wv), Some(url)) = (&self.webview, self.loaded_url.clone()) {
            wv.update(cx, |w, _| w.load_url(&url));
        }
    }
}

impl Render for PreviewPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::of(cx);
        let status = self.model.read(cx).dev_server.clone();
        let running = status.as_ref().map(|s| s.running).unwrap_or(false);
        let url = status.as_ref().and_then(|s| s.url.clone()).filter(|_| running);
        let message = status.as_ref().map(|s| s.message.clone()).unwrap_or_default();
        if let Some(u) = &url {
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

        div()
            .size_full()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(ui.border)
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
                        let app = app.clone();
                        move |_, _, cx| app.update(cx, |m, cx| m.dev_server_open(cx))
                    }))
                    .child(icon_button("preview-stop", Lucide::Square, ui.text_muted).on_click({
                        let app = app.clone();
                        move |_, _, cx| {
                            app.update(cx, |m, cx| {
                                if m.dev_server.as_ref().map(|s| s.running).unwrap_or(false) {
                                    m.dev_server_toggle(cx);
                                } else {
                                    m.toast(ToastKind::Info, "Dev server is not running");
                                    cx.notify();
                                }
                            })
                        }
                    }))
                    .child(icon_button("preview-close", Lucide::X, ui.text_muted).on_click(
                        |_, window, cx| window.dispatch_action(Box::new(ToggleDevPreview), cx),
                    )),
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
                                        "Start the dev server from the thread header to preview it here.".into()
                                    } else {
                                        message
                                    },
                                }),
                        )
                    }),
            )
    }
}
