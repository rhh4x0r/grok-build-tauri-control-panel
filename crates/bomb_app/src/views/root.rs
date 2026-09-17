//! Main window: custom title bar over a resizable sidebar | thread view split.

use gpui_kit::component::{h_resizable, resizable_panel, ActiveTheme, Root, TitleBar};
use gpui_kit::*;

use crate::actions::{NewMockSession, NewThread, OpenProject, OpenSettings};
use crate::models::app::AppModel;
use crate::theme::{c, Palette};
use crate::views::sidebar::SidebarView;
use crate::views::thread_view::ThreadView;

pub struct RootView {
    model: Entity<AppModel>,
    sidebar: Entity<SidebarView>,
    thread: Entity<ThreadView>,
    focus: FocusHandle,
}

impl RootView {
    pub fn new(model: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        let sidebar = cx.new(|cx| SidebarView::new(model.clone(), cx));
        let thread = cx.new(|cx| ThreadView::new(model.clone(), cx));
        Self {
            model,
            sidebar,
            thread,
            focus: cx.focus_handle(),
        }
    }

    fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let title = model
            .selected_thread()
            .map(|t| t.read(cx).title())
            .unwrap_or_else(|| "Bomb Code".to_string());
        let project = model
            .active_project
            .clone()
            .map(|p| crate::models::app::project_name(&p))
            .unwrap_or_else(|| "No project".into());
        TitleBar::new()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .px_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                img("assets/logo.png")
                                    .size(px(16.))
                                    .flex_shrink_0(),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child(title),
                            ),
                    )
                    .child(
                        div()
                            .id("project-chip")
                            .flex()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .text_xs()
                            .text_color(c(Palette::MUTED))
                            .hover(|s| s.bg(cx.theme().accent))
                            .cursor_pointer()
                            .on_click(cx.listener(|_, _, window, cx| {
                                window.dispatch_action(Box::new(OpenProject), cx);
                            }))
                            .child(
                                div()
                                    .size(px(8.))
                                    .rounded_sm()
                                    .bg(c(Palette::ACCENT)),
                            )
                            .child(project),
                    ),
            )
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.clone();
        div()
            .id("root")
            .track_focus(&self.focus)
            .on_action(cx.listener(move |_, _: &NewMockSession, _, cx| {
                model.update(cx, |m, cx| m.new_mock_session(cx));
            }))
            .on_action(cx.listener(|_, _: &NewThread, _, cx| {
                // Phase 3 wires real sessions; until then this is a no-op.
                tracing::info!("new thread requested");
                cx.notify();
            }))
            .on_action(cx.listener(|_, _: &OpenSettings, _, _| {
                tracing::info!("settings requested (Phase 4)");
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.title_bar(cx))
            .child(
                h_resizable("main-split")
                    .child(
                        resizable_panel()
                            .size(px(240.))
                            .size_range(px(200.)..px(400.))
                            .child(self.sidebar.clone()),
                    )
                    .child(resizable_panel().child(self.thread.clone())),
            )
    }
}

pub fn open_main_window(model: Entity<AppModel>, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1380.), px(900.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(960.), px(640.))),
        ..TitleBar::window_options()
    };
    cx.spawn(async move |cx| {
        let result = cx.open_window(options, |window, cx| {
            let view = cx.new(|cx| RootView::new(model.clone(), cx));
            cx.new(|cx| Root::new(view, window, cx))
        });
        if let Err(e) = result {
            tracing::error!(error = %e, "failed to open main window");
        }
    })
    .detach();
}
