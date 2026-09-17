//! Main window: unified title bar over a resizable sidebar | thread view split.
//! Also owns app-level actions and drains toasts into the notification layer.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{h_resizable, resizable_panel, Icon, Root, TitleBar, WindowExt};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::{
    CycleApprovalMode, DeleteThread, LandThread, NewMockSession, NewThread, OpenProject, OpenSettings,
    FindInThread, OpenCommandPalette, RevealProject, StopTurn, SyncThread, ToggleDevPreview,
    ToggleExplainer,
};
use crate::views::preview::PreviewPanel;
use crate::models::app::{project_name, AppModel, ToastKind};
use crate::theme::{Layout, Ui};
use crate::views::sidebar::SidebarView;
use crate::views::thread_view::ThreadView;

pub struct RootView {
    model: Entity<AppModel>,
    sidebar: Entity<SidebarView>,
    thread: Entity<ThreadView>,
    preview: Entity<PreviewPanel>,
    review: Entity<crate::views::workspaces::ReviewPanel>,
    focus: FocusHandle,
    sidebar_open: bool,
    preview_open: bool,
}

impl RootView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        let sidebar = cx.new(|cx| SidebarView::new(model.clone(), window, cx));
        let thread = cx.new(|cx| ThreadView::new(model.clone(), window, cx));
        let preview = cx.new(|cx| PreviewPanel::new(model.clone(), cx));
        let review = cx.new(|cx| crate::views::workspaces::ReviewPanel::new(model.clone(), cx));
        Self {
            review,
            model,
            sidebar,
            thread,
            preview,
            focus: cx.focus_handle(),
            sidebar_open: true,
            preview_open: false,
        }
    }

    fn drain_toasts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let toasts: Vec<(ToastKind, String)> = self.model.update(cx, |m, _| m.toasts.drain(..).collect());
        for (kind, msg) in toasts {
            let note = match kind {
                ToastKind::Info => Notification::info(msg),
                ToastKind::Success => Notification::success(msg),
                ToastKind::Warning => Notification::warning(msg),
                ToastKind::Error => Notification::error(msg),
            };
            window.push_notification(note, cx);
        }
    }

    fn title_bar(&self, ui: &Ui, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let selected = model.selected_thread();
        let title = selected
            .as_ref()
            .map(|t| t.read(cx).title())
            .unwrap_or_else(|| "New thread".to_string());
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
                .child(icon_button("toggle-sidebar", Lucide::PanelLeft).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.sidebar_open = !this.sidebar_open;
                        cx.notify();
                    },
                )))
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
                        .child(div().size(px(14.)).text_color(ui.text_muted).child(Icon::from(Lucide::Bomb)))
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
        self.drain_toasts(window, cx);
        let ui = Ui::of(cx);
        let model = self.model.clone();
        let m2 = self.model.clone();
        let m3 = self.model.clone();
        let m4 = self.model.clone();
        let m5 = self.model.clone();
        div()
            .id("root")
            .track_focus(&self.focus)
            .on_action(cx.listener(move |_, _: &NewMockSession, _, cx| {
                model.update(cx, |m, cx| m.new_mock_session(cx));
            }))
            .on_action(cx.listener(|this, _: &crate::actions::NewWorkspaceConversation, window, cx| {
                this.model.update(cx, |m, cx| m.new_workspace_thread(cx));
                this.thread.update(cx, |t, cx| t.focus_composer(window, cx));
            }))
            .on_action(cx.listener(move |this, _: &NewThread, window, cx| {
                m2.update(cx, |m, cx| m.new_thread(cx));
                this.thread.update(cx, |t, cx| t.focus_composer(window, cx));
            }))
            .on_action(cx.listener(move |_, _: &OpenProject, _, cx| {
                m3.update(cx, |m, cx| m.open_project(cx));
            }))
            .on_action(cx.listener(move |_, _: &RevealProject, _, cx| {
                m4.update(cx, |m, cx| m.reveal_project(cx));
            }))
            .on_action(cx.listener(move |_, _: &CycleApprovalMode, _, cx| {
                m5.update(cx, |m, cx| m.cycle_mode(cx));
            }))
            .on_action(cx.listener(|this, _: &StopTurn, _, cx| {
                this.model.update(cx, |m, cx| m.cancel_selected(cx));
            }))
            .on_action(cx.listener(|_, _: &OpenSettings, _, cx| {
                crate::views::settings::open_settings_window(cx);
            }))
            .on_action(cx.listener(|this, _: &OpenCommandPalette, window, cx| {
                crate::views::palette::open_palette(this.model.clone(), window, cx);
            }))
            .on_action(cx.listener(|this, _: &FindInThread, window, cx| {
                this.thread.update(cx, |t, cx| t.toggle_search(window, cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleDevPreview, _, cx| {
                let from_review = this.model.read(cx).review_open;
                this.model.update(cx, |m, cx| { m.review_open = false; cx.notify(); });
                if from_review { this.preview_open = false; }
                let running = this.model.read(cx).dev_server.as_ref().map(|s| s.running).unwrap_or(false);
                if !this.preview_open {
                    this.preview_open = true;
                    if !running {
                        this.model.update(cx, |m, cx| m.dev_server_toggle(cx));
                    }
                } else {
                    this.preview_open = false;
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleExplainer, _, cx| {
                if let Some(t) = this.model.read(cx).selected_thread() {
                    t.update(cx, |t, cx| {
                        t.explain_open = !t.explain_open;
                        cx.notify();
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &LandThread, _, cx| {
                let sel = this.model.read(cx).selected;
                if let Some(id) = sel {
                    this.model.update(cx, |m, cx| m.land_thread(id, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &SyncThread, _, cx| {
                let sel = this.model.read(cx).selected;
                if let Some(id) = sel {
                    this.model.update(cx, |m, cx| m.sync_thread(id, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &DeleteThread, window, cx| {
                let sel = this.model.read(cx).selected;
                if let Some(id) = sel {
                    let m = this.model.clone();
                    window.open_alert_dialog(cx, move |dlg, _, _| {
                        let m = m.clone();
                        dlg.title("Delete this thread?")
                            .description("Its transcript is removed. The workspace and files are kept. This cannot be undone.")
                            .on_ok(move |_, _, cx| {
                                m.update(cx, |a, cx| a.remove_thread(id, cx));
                                true
                            })
                    });
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(ui.glass)
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
                    .child(resizable_panel().child(self.thread.clone()))
                    .when(self.model.read(cx).review_open, |el| {
                        el.child(resizable_panel().size(px(480.)).size_range(px(320.)..px(1000.)).child(self.review.clone()))
                    })
                    .when(self.preview_open && !self.model.read(cx).review_open, |el| {
                        el.child(
                            resizable_panel()
                                .size(px(520.))
                                .size_range(px(360.)..px(1100.))
                                .child(self.preview.clone()),
                        )
                    }),
            )
    }
}

pub fn open_main_window(model: Entity<AppModel>, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1380.), px(900.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(960.), px(640.))),
        window_background: crate::theme::window_background(cx),
        ..TitleBar::window_options()
    };
    cx.spawn(async move |cx| {
        let result = cx.open_window(options, |window, cx| {
            let view = cx.new(|cx| RootView::new(model.clone(), window, cx));
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
