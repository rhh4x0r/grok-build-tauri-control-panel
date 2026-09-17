//! Left column: sessions grouped by project, and the connected services.

use gpui_kit::component::{ActiveTheme, IconName};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use grok_cli_wrapper::BackendAuth;
use uuid::Uuid;

use crate::actions::NewThread;
use crate::models::app::{AppModel, ProjectGroup};
use crate::models::thread::ThreadModel;
use crate::theme::{c, ca, Palette};

pub struct SidebarView {
    model: Entity<AppModel>,
}

impl SidebarView {
    pub fn new(model: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self { model }
    }

    fn section_label(text: &str) -> impl IntoElement {
        div()
            .px_3()
            .pt_3()
            .pb_1()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(c(Palette::DIM))
            .child(text.to_string())
    }

    fn group(&self, g: &ProjectGroup, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.clone();
        let root = g.root.clone();
        let root_for_click = root.clone();
        let active = self.model.read(cx).active_project.as_deref() == Some(g.root.as_str());
        let selected = self.model.read(cx).selected;
        let chevron = if g.collapsed {
            IconName::ChevronRight
        } else {
            IconName::ChevronDown
        };
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(SharedString::from(format!("group-{root}")))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .mx_1()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|s| s.bg(cx.theme().sidebar_accent))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model.update(cx, |m, cx| {
                            m.active_project = Some(root_for_click.clone());
                            cx.notify();
                        });
                    }))
                    .child(
                        div()
                            .id(SharedString::from(format!("toggle-{root}")))
                            .size(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(c(Palette::DIM))
                            .on_click({
                                let model = model.clone();
                                let root = root.clone();
                                move |_, _, cx| {
                                    model.update(cx, |m, cx| m.toggle_group(&root, cx));
                                }
                            })
                            .child(chevron),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(if active {
                                cx.theme().foreground
                            } else {
                                c(Palette::MUTED)
                            })
                            .child(g.name.clone()),
                    )
                    .when(active, |el| {
                        el.child(
                            div()
                                .size(px(6.))
                                .rounded_full()
                                .bg(c(Palette::ACCENT)),
                        )
                    }),
            )
            .when(!g.collapsed, |el| {
                let rows: Vec<(Uuid, Entity<ThreadModel>)> = {
                    let m = self.model.read(cx);
                    g.threads
                        .iter()
                        .filter_map(|id| m.threads.get(id).cloned().map(|t| (*id, t)))
                        .collect()
                };
                el.children(
                    rows.into_iter()
                        .map(|(id, t)| self.thread_row(id, &t, selected == Some(id), cx)),
                )
            })
    }

    fn thread_row(
        &self,
        id: Uuid,
        t: &Entity<ThreadModel>,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tm = t.read(cx);
        let title = tm.title();
        let status = tm.meta.status.clone();
        let live = tm.meta.live;
        let turn_active = tm.thread.presence.turn_active();
        let waiting = status.contains("wait") || status.contains("approv");
        let dot = if waiting {
            c(Palette::WARN)
        } else if turn_active || status == "running" {
            c(Palette::AGENT)
        } else if live {
            c(Palette::ACCENT)
        } else {
            c(Palette::DIM)
        };
        let backend = tm.meta.backend.clone();
        let model = self.model.clone();
        div()
            .id(SharedString::from(format!("thread-{id}")))
            .flex()
            .items_center()
            .gap_2()
            .pl_6()
            .pr_2()
            .py_1()
            .mx_1()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |s| s.bg(cx.theme().sidebar_accent))
            .hover(|s| s.bg(cx.theme().sidebar_accent))
            .on_click(move |_, _, cx| {
                model.update(cx, |m, cx| m.select(Some(id), cx));
            })
            .child(div().size(px(7.)).rounded_full().flex_shrink_0().bg(dot))
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_color(if selected {
                        cx.theme().foreground
                    } else {
                        c(Palette::TEXT)
                    })
                    .child(title),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(ca(Palette::backend(&backend), 0.8))
                    .child(backend),
            )
    }

    fn service_row(&self, a: &BackendAuth, cx: &mut Context<Self>) -> impl IntoElement {
        let dot = if a.logged_in {
            c(Palette::OK)
        } else if a.runnable {
            c(Palette::DIM)
        } else {
            c(Palette::ERR)
        };
        let detail = if a.logged_in {
            a.account.clone().or_else(|| a.plan.clone()).unwrap_or_default()
        } else if a.runnable {
            "not signed in".into()
        } else {
            "not installed".into()
        };
        div()
            .id(SharedString::from(format!("svc-{}", a.backend)))
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .rounded_md()
            .hover(|s| s.bg(cx.theme().sidebar_accent))
            .child(div().size(px(7.)).rounded_full().bg(dot))
            .child(
                div()
                    .text_sm()
                    .text_color(c(Palette::backend(&a.backend)))
                    .child(a.display_name.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(c(Palette::DIM))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(detail),
            )
    }
}

impl Render for SidebarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let groups = self.model.read(cx).groups(cx);
        let auth = self.model.read(cx).auth.clone();
        div()
            .id("sidebar")
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pr_2()
                    .child(Self::section_label("SESSIONS"))
                    .child(
                        div()
                            .id("new-thread")
                            .mt_2()
                            .size(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .text_color(c(Palette::MUTED))
                            .cursor_pointer()
                            .hover(|s| s.bg(cx.theme().sidebar_accent))
                            .on_click(cx.listener(|_, _, window, cx| {
                                window.dispatch_action(Box::new(NewThread), cx);
                            }))
                            .child(IconName::Plus),
                    ),
            )
            .child(
                div()
                    .id("thread-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .children(groups.iter().map(|g| self.group(g, cx)))
                    .when(groups.is_empty(), |el| {
                        el.child(
                            div()
                                .px_3()
                                .py_2()
                                .text_xs()
                                .text_color(c(Palette::DIM))
                                .child("No projects yet. Open one with ⌘O."),
                        )
                    }),
            )
            .child(
                div()
                    .border_t_1()
                    .border_color(cx.theme().sidebar_border)
                    .pb_2()
                    .child(Self::section_label("SERVICES"))
                    .children(auth.iter().map(|a| self.service_row(a, cx))),
            )
    }
}
