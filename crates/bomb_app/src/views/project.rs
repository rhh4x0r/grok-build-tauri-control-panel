//! Project page: what is finished, what is in progress, and how each thread relates to the default branch.
use crate::views::button::Button;
use crate::{
    models::app::{project_name, AppModel},
    theme::{Layout, Ui},
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::{Disableable, Icon, Sizable};
use gpui_kit::{prelude::FluentBuilder as _, *};

pub fn project_page(model: Entity<AppModel>, ui: &Ui, cx: &App) -> AnyElement {
    let m = model.read(cx);
    let root = m.active_project.clone().unwrap_or_default();
    let loading = m.overview_loading.contains(&root);
    let data = m.project_overviews.get(&root);
    let refresh = model.clone();
    let reveal = model.clone();
    let mut page = div()
        .id("project-overview")
        .size_full()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap_5()
        .p_6()
        .text_size(px(crate::theme::Type::BODY))
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
                                .text_size(px(crate::theme::Type::DISPLAY))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(project_name(&root)),
                        )
                        .child(
                            div()
                                .text_size(px(crate::theme::Type::CAPTION))
                                .text_color(ui.text_faint)
                                .child(root.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new("project-refresh")
                                .ghost()
                                .small()
                                .icon(Lucide::RefreshCw)
                                .label(if loading { "Refreshing…" } else { "Refresh" })
                                .disabled(loading)
                                .tooltip("Check again for new work and merges")
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
                        .child(div().text_color(ui.text_muted).child(error.clone())),
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
    if !overview.git_detected {
        let app = model.clone();
        let name = project_name(&root);
        // Centered like the welcome screen: one clear next step.
        return div()
            .id("project-no-git")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_4()
            .p_6()
            .child(Icon::from(Lucide::GitBranch).size(px(28.)).text_color(ui.text_muted))
            .child(div().text_size(px(crate::theme::Type::DISPLAY)).font_weight(FontWeight::SEMIBOLD).child(format!("Set up {name} for threads")))
            .child(
                div()
                    .max_w(px(460.))
                    .text_center()
                    .text_size(px(Layout::BODY_SIZE))
                    .line_height(px(Layout::BODY_LINE))
                    .text_color(ui.text_muted)
                    .child("This folder isn’t tracked by Git yet. Turning it on lets each thread work on its own copy, so you can run several features at once and merge the ones you like."),
            )
            .child(
                Button::new("initialize-git")
                    .primary()
                    .large()
                    .icon(Lucide::GitBranch)
                    .label(if loading { "Setting up…" } else { "Turn on Git for this folder" })
                    .disabled(loading)
                    .on_click(move |_, _, cx| app.update(cx, |m, cx| m.initialize_project_git(root.clone(), cx))),
            )
            .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child("Stays on your computer. Nothing is committed or uploaded."))
            .into_any_element();
    }
    let status = m.project_status.get(&root);
    let base = overview.default_branch.clone();
    let busy = m.git_busy;
    let workspace_for = |branch: &str| {
        m.workspaces.iter().find(|w| {
            w.project_root == root && w.branch == branch && !w.inline && !w.shared_checkout && w.archived_at.is_none()
        })
    };
    let features: Vec<_> = overview.branches.iter().filter(|b| b.name != base).collect();
    let waiting = features.iter().filter(|b| b.ahead > 0).count();
    let dirty = status.is_some_and(|s| s.dirty);

    let mut main = panel(ui)
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(Icon::from(Lucide::GitBranch).size(px(15.)).text_color(ui.accent))
                .child(div().text_size(px(crate::theme::Type::TITLE)).font_weight(FontWeight::MEDIUM).child(base.clone()))
                .child(badge("Finished work".into(), ui))
                .child(div().flex_1())
                .when(status.is_some_and(|s| s.remote), |el| {
                    let app = model.clone();
                    let root = root.clone();
                    el.child(Button::new("pull-default").ghost().small().label("Get latest from GitHub…").on_click(move |_, window, cx| {
                        use gpui_kit::component::WindowExt;
                        let app = app.clone(); let root = root.clone();
                        window.open_alert_dialog(cx, move |d, _, _| {
                            let app = app.clone(); let root = root.clone();
                            d.confirm().title("Update the finished version?").description("Downloads the newest version from GitHub. Needs a project folder with no unsaved edits.")
                                .on_ok(move |_, _, cx| { app.update(cx, |m, cx| m.pull_project(root.clone(), cx)); true })
                        });
                    }))
                })
                .child(
                    Button::new("new-workspace")
                        .primary()
                        .small()
                        .icon(Lucide::Plus)
                        .label("New thread")
                        .tooltip("Start a feature on its own copy of the project. You can run several at once with different models.")
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::actions::NewThread), cx)),
                ),
        )
        .when(!m.project_intro_seen, |el| {
            let app = model.clone();
            el.child(
                div()
                    .flex()
                    .items_start()
                    .gap_3()
                    .child(div().flex_1().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child(format!(
                        "The finished version of your project. Every thread works on its own copy, and {base} only changes when you merge a thread into it."
                    )))
                    .child(Button::new("project-intro-dismiss").ghost().small().label("Got it").on_click(move |_, _, cx| app.update(cx, |m, cx| m.dismiss_project_intro(cx)))),
            )
        })
        .child(div().text_size(px(crate::theme::Type::BODY)).child(match waiting {
            0 => format!("Nothing is waiting. Everything finished is already in {base}."),
            1 => format!("1 feature has work that is not in {base} yet."),
            n => format!("{n} features have work that is not in {base} yet."),
        }));
    if dirty {
        main = main.child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.warning).child(format!(
            "The project folder has unsaved edits on {base}. Agents will stop and ask before merging until they are committed or discarded."
        )));
    }
    page = page.child(main);

    // Board: a thread moves right as its work gets saved and then merged.
    let running = |w: &grok_persistence::WorkspaceRecord| {
        w.threads.iter().filter_map(|t| uuid::Uuid::parse_str(t).ok()).any(|id| {
            m.threads.get(&id).is_some_and(|t| t.read(cx).thread.presence.turn_active())
        })
    };
    let mut columns: [Vec<_>; 3] = Default::default();
    for b in features {
        let workspace = workspace_for(&b.name);
        let working = workspace.is_some_and(|w| running(w));
        let column = if working || (workspace.is_some() && b.ahead == 0 && b.behind == 0) { 0 } else if b.ahead > 0 { 1 } else { 2 };
        columns[column].push((b, workspace, working));
    }
    let titles = [
        ("Working", "An agent is on it, or nothing is saved yet.".to_string()),
        ("In progress", format!("Has saved work that is not in {base} yet. Merge it when you’re happy with it.")),
        ("In main", format!("Everything here is already in {base}. Close to tidy up.")),
    ];
    let mut board = div().id("feature-board").flex().gap_3().items_start().overflow_x_scroll();
    for ((title, hint), cards) in titles.into_iter().zip(columns) {
        // Finished work piles up: show a handful, and offer to tidy it away.
        let merged_column = title == "In main";
        let total = cards.len();
        let show_all = m.show_all_merged.contains(&root);
        let cards: Vec<_> = if merged_column && !show_all { cards.into_iter().take(MERGED_SHOWN).collect() } else { cards };
        let mut column = panel(ui)
            .flex_1()
            .min_w(px(250.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(section(title, ui))
                    .child(badge(total.to_string(), ui))
                    .child(div().flex_1())
                    .when(merged_column && total > 0, |el| {
                        let (app, project) = (model.clone(), root.clone());
                        el.child(Button::new("project-cleanup").ghost().small().label("Clean up").disabled(busy)
                            .tooltip("Close these threads and delete their branches. Only work that is already merged is removed.")
                            .on_click(move |_, window, cx| {
                                use gpui_kit::component::WindowExt;
                                let (app, project) = (app.clone(), project.clone());
                                window.open_alert_dialog(cx, move |d, _, _| {
                                    let (app, project) = (app.clone(), project.clone());
                                    d.confirm()
                                        .title("Clean up finished work?")
                                        .description("Threads and branches whose work is already merged are closed and removed. Chats move to the archive. Anything unmerged, busy or checked out is left alone.")
                                        .on_ok(move |_, _, cx| { app.update(cx, |m, cx| m.cleanup_merged(project.clone(), cx)); true })
                                });
                            }))
                    }),
            )
            .child(div().text_size(px(crate::theme::Type::CAPTION)).text_color(ui.text_faint).child(hint));
        if cards.is_empty() {
            column = column.child(div().py_3().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child("Nothing here"));
        }
        for (b, workspace, working) in cards {
            let pr = overview.prs.iter().find(|pr| pr.head_ref_name == b.name);
            let open = m.overview_branch.as_deref() == Some(b.name.as_str());
            let toggle = model.clone();
            let name = b.name.clone();
            let agent = workspace
                .and_then(|w| w.threads.iter().filter_map(|t| uuid::Uuid::parse_str(t).ok()).find_map(|id| m.threads.get(&id)))
                .map(|t| { let meta = &t.read(cx).meta; if meta.model.is_empty() { meta.backend.clone() } else { meta.model.clone() } });
            let mut card = div()
                .flex()
                .flex_col()
                .gap_1p5()
                .p_3()
                .rounded(px(8.))
                .border_1()
                .border_color(if working { ui.accent } else { ui.border })
                .bg(Ui::alpha(ui.text, 0.03))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .font_weight(FontWeight::MEDIUM)
                                .child(workspace.map(|w| w.name.clone()).unwrap_or_else(|| b.name.clone())),
                        )
                        .when(working, |el| el.child(badge("Agent working".into(), ui)))
                        .child(
                            Button::new(SharedString::from(format!("feature-toggle-{}", b.name)))
                                .ghost()
                                .small()
                                .icon(if open { Lucide::ChevronDown } else { Lucide::ChevronRight })
                                .tooltip("Show what changed")
                                .on_click(move |_, _, cx| {
                                    toggle.update(cx, |m, cx| {
                                        m.overview_branch = if m.overview_branch.as_deref() == Some(name.as_str()) { None } else { Some(name.clone()) };
                                        cx.notify();
                                    })
                                }),
                        ),
                )
                .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(if b.ahead > 0 { ui.text } else { ui.text_muted }).child(
                    bomb_core::services::project_overview::describe_relation(b.ahead, b.behind, &b.base),
                ))
                .child(div().text_size(px(crate::theme::Type::CAPTION)).text_color(ui.text_faint).child(format!(
                    "{}{} files changed{}",
                    match (&agent, workspace) {
                        (Some(agent), Some(w)) => format!("{agent} · {} chats · ", w.threads.len()),
                        (None, Some(w)) => format!("{} chats · ", w.threads.len()),
                        _ => "No open thread · ".into(),
                    },
                    b.files.len(),
                    pr.map(|pr| format!(" · PR #{} · {}", pr.number, pr.checks())).unwrap_or_default(),
                )));
            if open {
                let mut detail = div().pt_1().flex().flex_col().gap_1();
                detail = detail.child(div().text_size(px(crate::theme::Type::CAPTION)).font_family(ui.mono.clone()).text_color(ui.text_faint).child(b.name.clone()));
                if b.ahead > 0 {
                    detail = detail.child(section("Latest saved changes", ui));
                    for (_, title) in b.commits.iter().take(b.ahead.min(6)) {
                        detail = detail.child(div().text_size(px(crate::theme::Type::SMALL)).child(title.clone()));
                    }
                }
                if !b.files.is_empty() {
                    detail = detail.child(section("Files changed", ui));
                }
                for file in b.files.iter().take(10) {
                    detail = detail.child(div().text_size(px(crate::theme::Type::CAPTION)).font_family(ui.mono.clone()).text_color(ui.text_muted).child(file.clone()));
                }
                if b.files.len() > 10 {
                    detail = detail.child(div().text_size(px(crate::theme::Type::CAPTION)).text_color(ui.text_faint).child(format!("…and {} more files", b.files.len() - 10)));
                }
                card = card.child(detail);
            }
            if let Some(w) = workspace {
                let open_app = model.clone();
                let open_id = w.id.clone();
                let merge_app = model.clone();
                let merge_id = w.id.clone();
                let both_app = model.clone();
                let both_id = w.id.clone();
                let close_app = model.clone();
                let close_id = w.id.clone();
                let close_name = w.name.clone();
                card = card.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .pt_1()
                        .child(
                            Button::new(SharedString::from(format!("feature-open-{}", w.id)))
                                .outline()
                                .small()
                                .label("Open chat")
                                .on_click(move |_, _, cx| open_app.update(cx, |m, cx| m.open_workspace(open_id.clone(), cx))),
                        )
                        .when(b.ahead > 0 && !working, |el| {
                            el.child(
                                Button::new(SharedString::from(format!("feature-merge-{}", w.id)))
                                    .primary()
                                    .small()
                                    .label(format!("Merge to {}", b.base))
                                    .disabled(busy)
                                    .tooltip(format!("Opens the chat and asks its agent to merge this work into {}, resolving any conflicts it finds.", b.base))
                                    .on_click(move |_, _, cx| merge_app.update(cx, |m, cx| m.merge_to_main(merge_id.clone(), false, cx))),
                            )
                            .child(
                                Button::new(SharedString::from(format!("feature-merge-close-{}", w.id)))
                                    .outline()
                                    .small()
                                    .label("Merge & close")
                                    .disabled(busy)
                                    .tooltip(format!("Same merge in the chat, then closes this feature once its work is confirmed in {}.", b.base))
                                    .on_click(move |_, _, cx| both_app.update(cx, |m, cx| m.merge_to_main(both_id.clone(), true, cx))),
                            )
                        })
                        .when(!working, |el| {
                            el.child(
                                Button::new(SharedString::from(format!("feature-close-{}", w.id)))
                                    .ghost()
                                    .small()
                                    .label("Close")
                                    .disabled(busy)
                                    .tooltip("Archive this thread and tidy up its branch. Unmerged work is kept on the branch.")
                                    .on_click(move |_, window, cx| confirm_close(close_app.clone(), close_id.clone(), close_name.clone(), window, cx)),
                            )
                        }),
                );
            }
            column = column.child(card);
        }
        if merged_column && total > MERGED_SHOWN {
            let (app, project) = (model.clone(), root.clone());
            column = column.child(
                Button::new("project-merged-more").ghost().small()
                    .label(if show_all { "Show fewer".to_string() } else { format!("Show all {total}") })
                    .on_click(move |_, _, cx| app.update(cx, |m, cx| { if !m.show_all_merged.remove(&project) { m.show_all_merged.insert(project.clone()); } cx.notify(); })),
            );
        }
        board = board.child(column);
    }
    page = page.child(board);
    if overview.prs.is_empty() {
        return page.into_any_element();
    }
    let mut prs = panel(ui).child(section("Open pull requests", ui));
    if let Some(error) = &overview.pr_error {
        prs = prs.child(
            div()
                .text_size(px(crate::theme::Type::SMALL))
                .text_color(ui.text_muted)
                .child(error.clone()),
        );
    } else if overview.prs.is_empty() {
        prs = prs.child(
            div()
                .text_size(px(crate::theme::Type::SMALL))
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
                        .text_size(px(crate::theme::Type::CAPTION))
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
                .text_size(px(crate::theme::Type::SMALL))
                .child("Showing the first 100 open pull requests."),
        );
    }
    page.child(prs).into_any_element()
}
/// Cards shown in the "In main" column before "Show all".
const MERGED_SHOWN: usize = 5;

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
        .text_size(px(crate::theme::Type::SMALL))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ui.text_muted)
        .child(label.to_string())
}
fn badge(label: String, ui: &Ui) -> Div {
    div()
        .text_size(px(crate::theme::Type::CAPTION))
        .text_color(ui.text_muted)
        .px_2()
        .py_1()
        .rounded(px(5.))
        .bg(ui.hover)
        .child(label)
}

/// Closing keeps the chats in the archive and never deletes unmerged work.
pub fn confirm_close(app: Entity<AppModel>, id: String, name: String, window: &mut Window, cx: &mut App) {
    use gpui_kit::component::WindowExt;
    window.open_alert_dialog(cx, move |d, _, _| {
        let app = app.clone();
        let id = id.clone();
        d.confirm()
            .title(format!("Close “{name}”?"))
            .description("Its chats move to the archive and its working copy is removed. The branch is deleted only if its work is already merged; otherwise it is kept.")
            .on_ok(move |_, _, cx| {
                app.update(cx, |m, cx| m.close_feature(id.clone(), cx));
                true
            })
    });
}
