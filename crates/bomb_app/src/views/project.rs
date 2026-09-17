//! Visual branch map and repository pull-request overview.
use crate::{
    models::app::{project_name, AppModel},
    theme::Ui,
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Disableable, Icon, Sizable};
use gpui_kit::{prelude::FluentBuilder as _, *};

pub fn project_page(model: Entity<AppModel>, ui: &Ui, cx: &App) -> AnyElement {
    let m = model.read(cx);
    let root = m.active_project.clone().unwrap_or_default();
    let loading = m.overview_loading.contains(&root);
    let data = m.project_overviews.get(&root);
    let refresh = model.clone();
    let reveal = model.clone();
    let ask = model.clone();
    let ask_root = root.clone();
    let fetch = model.clone();
    let mut page = div()
        .id("project-overview")
        .size_full()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_5()
        .p_6()
        .text_size(px(13.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .flex_wrap()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(22.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(project_name(&root)),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(ui.text_faint)
                                .child(root.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("project-fetch")
                                .ghost()
                                .small()
                                .label("Fetch remote")
                                .disabled(!m.project_status.get(&root).is_some_and(|s| s.remote))
                                .on_click(move |_, _, cx| {
                                    fetch.update(cx, |m, cx| m.fetch_project(cx))
                                }),
                        )
                        .child(
                            Button::new("project-refresh")
                                .ghost()
                                .small()
                                .icon(Lucide::RefreshCw)
                                .label(if loading { "Refreshing…" } else { "Refresh" })
                                .disabled(loading)
                                .tooltip("Refresh local branches and GitHub pull requests")
                                .on_click(move |_, _, cx| {
                                    refresh.update(cx, |m, cx| {
                                        m.refresh_project_overview(cx);
                                        m.refresh_project_status(false, cx);
                                    })
                                }),
                        )
                        .child(
                            Button::new("project-reveal")
                                .ghost()
                                .small()
                                .icon(Lucide::Folder)
                                .tooltip("Reveal project folder")
                                .on_click(move |_, _, cx| {
                                    reveal.update(cx, |m, cx| m.reveal_project(cx))
                                }),
                        ),
                ),
        );
    let overview = match data {
        Some(Ok(data)) => data,
        Some(Err(error)) => {
            return page
                .child(
                    panel(ui)
                        .child("Git overview unavailable")
                        .child(div().text_color(ui.text_muted).child(error.clone()))
                        .child(
                            Button::new("folder-ask")
                                .outline()
                                .small()
                                .label("Ask about this folder")
                                .on_click(move |_, _, cx| {
                                    ask.update(cx, |m, cx| m.inline_project(ask_root.clone(), cx))
                                }),
                        ),
                )
                .into_any_element()
        }
        None => {
            return page
                .child(
                    div()
                        .text_color(ui.text_muted)
                        .child("Loading branches and pull requests…"),
                )
                .into_any_element()
        }
    };
    let selected = m
        .overview_branch
        .as_deref()
        .and_then(|name| overview.branches.iter().find(|b| b.name == name))
        .or_else(|| overview.branches.first());
    let status = m.project_status.get(&root);
    page = page.child(
        div()
            .flex()
            .items_center()
            .gap_2()
            .flex_wrap()
            .child(badge(
                format!("{} local branches", overview.branches.len()),
                ui,
            ))
            .child(badge(
                if overview.pr_error.is_some() {
                    "PR status unavailable".into()
                } else {
                    format!("{} open PRs", overview.prs.len())
                },
                ui,
            ))
            .when_some(status, |el, s| {
                el.child(badge(
                    if s.dirty {
                        "Checkout has edits"
                    } else {
                        "Checkout clean"
                    }
                    .into(),
                    ui,
                ))
            })
            .child(div().flex_1())
            .child(
                Button::new("project-ask")
                    .ghost()
                    .small()
                    .label("Ask a question")
                    .on_click(move |_, _, cx| {
                        ask.update(cx, |m, cx| m.inline_project(ask_root.clone(), cx))
                    }),
            )
            .child(
                Button::new("new-workspace")
                    .outline()
                    .small()
                    .icon(Lucide::Plus)
                    .label("New workspace")
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::actions::NewThread), cx)
                    }),
            ),
    );
    let mut map = panel(ui)
        .flex_1()
        .min_w(px(240.))
        .child(section("Branch map", ui))
        .child(
            div()
                .text_size(px(11.))
                .text_color(ui.text_faint)
                .child(format!(
                    "Compared with local {} · select a branch",
                    overview.default_branch
                )),
        );
    if overview.branches.is_empty() {
        map = map.child(
            div()
                .text_color(ui.text_muted)
                .child("No local branches yet. Create an initial Git commit to get started."),
        );
    }
    for b in &overview.branches {
        let is_base = b.name == overview.default_branch;
        let active = selected.is_some_and(|s| s.name == b.name);
        let workspace = m.workspaces.iter().find(|w| {
            w.project_root == root && w.branch == b.name && !w.inline && w.archived_at.is_none()
        });
        let pr = overview.prs.iter().find(|pr| pr.head_ref_name == b.name);
        let app = model.clone();
        let name = b.name.clone();
        map = map.child(
            div()
                .flex()
                .items_center()
                .when(!is_base, |el| {
                    el.child(
                        div()
                            .ml_3()
                            .w(px(20.))
                            .h(px(46.))
                            .border_l_2()
                            .border_color(ui.border)
                            .child(div().mt(px(23.)).w_full().h(px(1.)).bg(ui.border)),
                    )
                })
                .child(
                    div()
                        .id(SharedString::from(format!("branch-{}", b.name)))
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_3()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(if active { ui.accent } else { ui.border })
                        .bg(Ui::alpha(
                            if active { ui.accent } else { ui.text },
                            if active { 0.08 } else { 0.025 },
                        ))
                        .cursor_pointer()
                        .hover(move |s| s.bg(ui.hover))
                        .on_click(move |_, _, cx| {
                            app.update(cx, |m, cx| {
                                m.overview_branch = Some(name.clone());
                                cx.notify();
                            })
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    Icon::from(Lucide::GitBranch).size(px(14.)).text_color(
                                        if is_base { ui.accent } else { ui.text_muted },
                                    ),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(b.name.clone()),
                                )
                                .when(is_base, |el| el.child(badge("Default".into(), ui)))
                                .when(b.current, |el| el.child(badge("Checked out".into(), ui))),
                        )
                        .child(div().text_size(px(11.)).text_color(ui.text_muted).child(
                            if is_base {
                                "Base for branch comparisons".into()
                            } else {
                                format!(
                                    "{} ahead · {} behind · {} committed files changed",
                                    b.ahead,
                                    b.behind,
                                    b.files.len()
                                )
                            },
                        ))
                        .when_some(workspace, |el, w| {
                            el.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(ui.text_faint)
                                    .child(format!("{} · {} chats", w.name, w.threads.len())),
                            )
                        })
                        .when_some(pr, |el, pr| {
                            el.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(ui.accent)
                                    .child(format!("PR #{} · {}", pr.number, pr.checks())),
                            )
                        }),
                ),
        );
    }
    let mut detail = panel(ui)
        .flex_1()
        .min_w(px(240.))
        .child(section("Branch details", ui));
    if let Some(b) = selected {
        detail = detail.child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::MEDIUM)
                .child(b.name.clone()),
        );
        if let Some(w) = m.workspaces.iter().find(|w| {
            w.project_root == root && w.branch == b.name && !w.inline && w.archived_at.is_none()
        }) {
            let app = model.clone();
            let id = w.id.clone();
            detail = detail.child(
                div().flex().gap_2().child(
                    Button::new("open-workspace")
                        .outline()
                        .small()
                        .label("Open workspace")
                        .on_click(move |_, _, cx| {
                            app.update(cx, |m, cx| m.open_workspace(id.clone(), cx))
                        }),
                ),
            );
            for tid in &w.threads {
                if let Ok(id) = uuid::Uuid::parse_str(tid) {
                    if let Some(thread) = m.threads.get(&id) {
                        let app = model.clone();
                        detail = detail.child(
                            Button::new(SharedString::from(format!("chat-{id}")))
                                .ghost()
                                .small()
                                .icon(Lucide::MessageCircle)
                                .label(thread.read(cx).title())
                                .on_click(move |_, _, cx| {
                                    app.update(cx, |m, cx| m.select(Some(id), cx))
                                }),
                        );
                    }
                }
            }
        }
        detail = detail.child(section("Recent commits", ui));
        for (sha, title) in &b.commits {
            detail = detail.child(
                div()
                    .flex()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .font_family(ui.mono.clone())
                            .text_size(px(11.))
                            .text_color(ui.text_faint)
                            .child(sha.clone()),
                    )
                    .child(div().flex_1().text_size(px(12.)).child(title.clone())),
            );
        }
        detail = detail.child(section("Committed changes", ui));
        if b.files.is_empty() {
            detail = detail.child(
                div()
                    .text_size(px(12.))
                    .text_color(ui.text_faint)
                    .child("No branch changes relative to its merge base."),
            );
        }
        for file in b.files.iter().take(12) {
            detail = detail.child(
                div()
                    .text_size(px(11.))
                    .font_family(ui.mono.clone())
                    .text_color(ui.text_muted)
                    .child(file.clone()),
            );
        }
        if b.files.len() > 12 {
            detail = detail.child(
                div()
                    .text_size(px(11.))
                    .text_color(ui.text_faint)
                    .child(format!("…and {} more files", b.files.len() - 12)),
            );
        }
        if b.name == overview.default_branch && status.is_some_and(|s| s.remote) {
            let app = model.clone();
            let root = root.clone();
            detail = detail.child(Button::new("pull-default").outline().small().label("Pull latest…").on_click(move |_, window, cx| {
                use gpui_kit::component::WindowExt;
                let app = app.clone(); let root = root.clone();
                window.open_alert_dialog(cx, move |d, _, _| {
                    let app = app.clone(); let root = root.clone();
                    d.confirm().title("Update default branch?").description("Fast-forward from origin. Requires a clean checkout on the default branch.")
                        .on_ok(move |_, _, cx| { app.update(cx, |m, cx| m.pull_project(root.clone(), cx)); true })
                });
            }));
        }
    }
    page = page.child(
        div()
            .flex()
            .gap_4()
            .flex_wrap()
            .items_start()
            .child(map)
            .child(detail),
    );
    let mut prs = panel(ui).child(section("Open pull requests", ui));
    if let Some(error) = &overview.pr_error {
        prs = prs.child(
            div()
                .text_size(px(12.))
                .text_color(ui.text_muted)
                .child(error.clone()),
        );
    } else if overview.prs.is_empty() {
        prs = prs.child(
            div()
                .text_size(px(12.))
                .text_color(ui.text_faint)
                .child("No open pull requests."),
        );
    }
    for pr in &overview.prs {
        let url = pr.url.clone();
        let review = match pr.review_decision.as_str() {
            "APPROVED" => "Approved",
            "CHANGES_REQUESTED" => "Changes requested",
            _ => "Awaiting review",
        };
        let merge = match pr.mergeable.as_str() {
            "CONFLICTING" => "Merge conflicts",
            "MERGEABLE" => "No merge conflicts",
            _ => "Mergeability pending",
        };
        prs = prs.child(
            div()
                .id(SharedString::from(format!("pr-{}", pr.number)))
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .rounded(px(8.))
                .border_1()
                .border_color(ui.border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::from(Lucide::GitPullRequest)
                                .size(px(14.))
                                .text_color(ui.accent),
                        )
                        .child(div().flex_1().child(format!("#{} {}", pr.number, pr.title)))
                        .child(
                            Button::new(SharedString::from(format!("open-pr-{}", pr.number)))
                                .ghost()
                                .small()
                                .icon(Lucide::ExternalLink)
                                .tooltip("Open pull request on GitHub")
                                .on_click(move |_, _, cx| {
                                    if url.starts_with("https://") {
                                        cx.open_url(&url);
                                    }
                                }),
                        ),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(ui.text_faint)
                        .child(format!("{} → {}", pr.head_ref_name, pr.base_ref_name)),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .flex_wrap()
                        .child(badge(if pr.is_draft { "Draft" } else { review }.into(), ui))
                        .child(badge(pr.checks().into(), ui))
                        .child(badge(merge.into(), ui)),
                ),
        );
    }
    if overview.prs.len() == 100 {
        prs = prs.child(
            div()
                .text_xs()
                .child("Showing the first 100 open pull requests."),
        );
    }
    page.child(prs).into_any_element()
}
fn panel(ui: &Ui) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .rounded(px(12.))
        .border_1()
        .border_color(ui.border)
        .bg(Ui::alpha(ui.text, 0.025))
}
fn section(label: &str, ui: &Ui) -> Div {
    div()
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ui.text_muted)
        .child(label.to_string())
}
fn badge(label: String, ui: &Ui) -> Div {
    div()
        .text_size(px(10.))
        .text_color(ui.text_muted)
        .px_2()
        .py_1()
        .rounded(px(5.))
        .bg(ui.hover)
        .child(label)
}
