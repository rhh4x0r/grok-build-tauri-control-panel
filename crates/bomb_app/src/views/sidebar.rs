//! Left column: threads (project · time-ago / title / branch) and the
//! connected services footer.

use chrono::{DateTime, Utc};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenuItem};
use gpui_kit::component::button::{Button, ButtonVariants};
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
}

impl SidebarView {
    pub fn new(model: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self { model }
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
        let selected = self.model.read(cx).selected;
        let rows: Vec<(Uuid, Entity<ThreadModel>)> = {
            let m = self.model.read(cx);
            g.threads
                .iter()
                .filter_map(|id| m.threads.get(id).cloned().map(|t| (*id, t)))
                .collect()
        };
        let name = g.name.clone();
        div().flex().flex_col().children(
            rows.into_iter()
                .map(|(id, t)| self.thread_row(id, &t, &name, selected == Some(id), ui, cx)),
        )
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
            .py_1p5()
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
                    menu = menu.item(PopupMenuItem::new("Sync from project branch").on_click(move |_, _, cx| {
                        m.update(cx, |a, cx| a.sync_thread(id, cx));
                    }));
                    let m = menu_model.clone();
                    menu = menu.item(PopupMenuItem::new("Land into project branch").on_click(move |_, _, cx| {
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
                            .description("Its transcript and worktree are removed. This cannot be undone.")
                            .on_ok(move |_, _, cx| {
                                m.update(cx, |a, cx| a.remove_thread(id, cx));
                                true
                            })
                    });
                }))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(ui.text_faint)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(project.to_string()),
                    )
                    .child(corner),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        div()
                            .size(px(7.))
                            .rounded_full()
                            .flex_shrink_0()
                            .bg(ui.backend(&backend)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(ui.text)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(title),
                    ),
            )
            .when_some(branch, |el, b| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .text_color(ui.text_faint)
                        .child(div().size(px(11.)).child(Icon::from(Lucide::GitBranch)))
                        .child(b),
                )
            })
    }

    fn service_row(&self, a: &BackendAuth, ui: &Ui) -> impl IntoElement {
        let model = self.model.clone();
        let backend = a.backend.clone();
        let logged_in = a.logged_in;
        let runnable = a.runnable;
        let dot = if a.logged_in {
            ui.success
        } else if a.runnable {
            ui.text_faint
        } else {
            ui.danger
        };
        let detail = if a.logged_in {
            a.account
                .clone()
                .or_else(|| a.plan.clone())
                .unwrap_or_else(|| "signed in".into())
        } else if a.runnable {
            "click to sign in".into()
        } else {
            "not installed".into()
        };
        let hover = ui.hover;
        div()
            .id(SharedString::from(format!("svc-{}", a.backend)))
            .flex()
            .items_center()
            .gap_2()
            .mx_2()
            .px_2()
            .py_1()
            .rounded(px(6.))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .on_click(move |_, window, cx| {
                if logged_in {
                    model.update(cx, |m, cx| m.sign_out(&backend, cx));
                } else if backend == "grok" {
                    crate::views::login_dialog::open_login_dialog(model.clone(), window, cx);
                } else if runnable {
                    model.update(cx, |m, cx| m.sign_in(&backend, cx));
                }
            })
            .child(div().size(px(7.)).rounded_full().bg(dot))
            .child(
                div()
                    .text_sm()
                    .text_color(ui.text)
                    .child(a.display_name.clone()),
            )
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
    }
}

impl Render for SidebarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::of(cx);
        let groups = self.model.read(cx).groups(cx);
        let auth = self.model.read(cx).auth.clone();
        div()
            .id("sidebar")
            .size_full()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(ui.border)
            .child(self.header(&ui, cx))
            .child(
                div()
                    .id("thread-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .py_1()
                    .children(groups.iter().map(|g| self.group(g, &ui, cx)))
                    .when(groups.iter().all(|g| g.threads.is_empty()), |el| {
                        el.child(
                            div()
                                .px_4()
                                .py_3()
                                .text_xs()
                                .text_color(ui.text_faint)
                                .child("No threads yet. Press + to start one."),
                        )
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
                    .children(auth.iter().map(|a| self.service_row(a, &ui))),
            )
    }
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
