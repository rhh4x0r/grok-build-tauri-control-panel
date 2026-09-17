//! Main window: unified title bar over a resizable sidebar | thread view split.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{h_resizable, resizable_panel, Icon, Root, TitleBar};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::{NewMockSession, NewThread, OpenProject, OpenSettings};
use crate::models::app::{project_name, AppModel};
use crate::theme::{Layout, Ui};
use crate::views::sidebar::SidebarView;
use crate::views::thread_view::ThreadView;

pub struct RootView {
    model: Entity<AppModel>,
    sidebar: Entity<SidebarView>,
    thread: Entity<ThreadView>,
    focus: FocusHandle,
    sidebar_open: bool,
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
            sidebar_open: true,
        }
    }

    fn title_bar(&self, ui: &Ui, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let selected = model.selected_thread();
        let title = selected
            .as_ref()
            .map(|t| t.read(cx).title())
            .unwrap_or_else(|| "Bomb Code".to_string());
        let project = model
            .active_project
            .as_deref()
            .map(project_name)
            .unwrap_or_else(|| "No project".into());
        let host = hostname();
        let hover = ui.hover;
        let muted = ui.text_muted;
        let icon_button = move |id: &'static str, icon: Lucide| {
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
                .child(div().size(px(14.)).child(Icon::from(icon)))
        };
        TitleBar::new().child(
            div()
                .flex()
                .items_center()
                .w_full()
                .h(px(Layout::TITLEBAR))
                .pl_1()
                .pr_3()
                .gap_1()
                .child(icon_button("toggle-sidebar", Lucide::PanelLeft).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.sidebar_open = !this.sidebar_open;
                        cx.notify();
                    }),
                ))
                .child(icon_button("new-thread-tb", Lucide::Plus).on_click(cx.listener(
                    |_, _, window, cx| {
                        window.dispatch_action(Box::new(NewThread), cx);
                    },
                )))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .ml_3()
                        .min_w_0()
                        .child(
                            div()
                                .size(px(14.))
                                .text_color(ui.text_muted)
                                .child(Icon::from(Lucide::Bomb)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ui.text)
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(title),
                        )
                        .child(
                            div()
                                .id("project-chip")
                                .text_xs()
                                .text_color(ui.text_faint)
                                .cursor_pointer()
                                .hover(|s| s.opacity(0.8))
                                .on_click(cx.listener(|_, _, window, cx| {
                                    window.dispatch_action(Box::new(OpenProject), cx);
                                }))
                                .child(format!("{project} @ {host}")),
                        ),
                ),
        )
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::theme::follow_system(window, cx);
        let ui = Ui::of(cx);
        let model = self.model.clone();
        div()
            .id("root")
            .track_focus(&self.focus)
            .on_action(cx.listener(move |_, _: &NewMockSession, _, cx| {
                model.update(cx, |m, cx| m.new_mock_session(cx));
            }))
            .on_action(cx.listener(|_, _: &NewThread, _, cx| {
                tracing::info!("new thread requested (Phase 3)");
                cx.notify();
            }))
            .on_action(cx.listener(|_, _: &OpenSettings, _, _| {
                tracing::info!("settings requested (Phase 4)");
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(ui.bg)
            .text_color(ui.text)
            .child(self.title_bar(&ui, cx))
            .child(
                h_resizable("main-split")
                    .when(self.sidebar_open, |el| {
                        el.child(
                            resizable_panel()
                                .size(px(Layout::SIDEBAR))
                                .size_range(px(224.)..px(400.))
                                .child(self.sidebar.clone()),
                        )
                    })
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

fn hostname() -> String {
    std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "local".into())
}
