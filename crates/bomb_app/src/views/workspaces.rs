//! Project overview and workspace review controls.
use crate::models::app::{AppModel, project_name};
use crate::theme::Ui;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{Sizable, WindowExt};
use gpui_kit::component::Disableable;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub fn confirm_action(
    model: Entity<AppModel>,
    id: String,
    action: &str,
    value: String,
    description: String,
    window: &mut Window,
    cx: &mut App,
) {
    let action = action.to_string();
    window.open_alert_dialog(cx, move |d, _, _| {
        let (model, id, action, value) = (model.clone(), id.clone(), action.clone(), value.clone());
        d.title("Confirm workspace action")
            .description(description.clone())
            .on_ok(move |_, _, cx| {
                model.update(cx, |m, cx| {
                    m.run_workspace_action(id.clone(), action.clone(), value.clone(), cx)
                });
                true
            })
    });
}

pub fn text_action(
    model: Entity<AppModel>,
    id: String,
    action: &str,
    title: &str,
    initial: String,
    window: &mut Window,
    cx: &mut App,
) {
    let input = cx.new(|cx| {
        let rows = if matches!(action, "pr" | "comment") { 3 } else { 1 };
        let mut s = TextareaState::new(window, cx).auto_grow(rows, 12).submit_on_enter(false);
        s.set_value(initial, window, cx);
        s
    });
    let action = action.to_string();
    let title = title.to_string();
    window.open_dialog(cx, move |d, _, _| {
        let (model, id, action, input) = (model.clone(), id.clone(), action.clone(), input.clone());
        let content = input.clone();
        d.title(title.clone())
            .w(px(620.))
            .content(move |c, _, _| c.child(Textarea::new(&content)))
            .on_ok(move |_, window, cx| {
                let text = input.read(cx).value().to_string();
                if text.trim().is_empty() {
                    return false;
                }
                model.update(cx, |m, cx| {
                    if action == "comment" { m.send_prompt(text, vec![], cx); }
                    else { m.run_workspace_action(id.clone(), action.clone(), text, cx); }
                });
                window.close_dialog(cx);
                true
            })
    });
}

pub fn project_page(model: Entity<AppModel>, ui: &Ui, cx: &App) -> AnyElement {
    let m = model.read(cx);
    let root = m.active_project.clone().unwrap_or_default();
    let workspaces: Vec<_> = m
        .workspaces
        .iter()
        .filter(|w| w.project_root == root && !w.inline)
        .cloned()
        .collect();
    let active: Vec<_> = workspaces
        .iter()
        .filter(|w| w.archived_at.is_none())
        .cloned()
        .collect();
    let m1 = model.clone();
    let m2 = model.clone();
    let m3 = model.clone();
    let inline_root = root.clone();
    let changes_model = model.clone();
    let status = m.project_status.get(&root).cloned().unwrap_or_default();
    let pull_model = model.clone();
    let pull_root = root.clone();
    div().id("project-overview").size_full().overflow_y_scroll().flex().flex_col().gap_5().p_6()
        .child(div().text_size(px(24.)).child(project_name(&root)))
        .child(div().text_sm().text_color(ui.text_faint).child(root))
        .child(div().flex().gap_2()
            .child(Button::new("project-fetch").outline().small().label("Fetch latest").disabled(!status.remote).on_click(move |_, _, cx| m1.update(cx, |m, cx| m.fetch_project(cx))))
            .child(Button::new("project-reveal").ghost().small().label("Reveal folder").on_click(move |_, _, cx| m2.update(cx, |m, cx| m.reveal_project(cx)))))
        .child(div().flex().flex_col().gap_2()
            .child(div().flex().gap_2()
                .child(Button::new("project-inline").outline().label("Ask a question").on_click(move |_, _, cx| m3.update(cx, |m, cx| m.inline_project(inline_root.clone(), cx))))
                .child(Button::new("project-make-changes").outline().label("Make changes").disabled(status.error.is_some()).on_click(move |_, _, cx| changes_model.update(cx, |m, cx| { m.new_thread(cx); m.set_new_intent(false, cx); }))))
            .child(div().text_xs().text_color(ui.text_faint).child("Ask to understand the project. Make changes starts a separate workspace with restore points.")))
        .child(div().text_sm().text_color(ui.text_muted).child(if status.error.is_some() { "You can ask questions about this folder. To make changes in a workspace, initialize Git first.".into() } else { format!("Main branch · {} · ↑{} ↓{}{}", status.branch, status.ahead, status.behind, if status.dirty { " · uncommitted changes" } else { " · clean" }) }))
        .when(status.remote, |el| el.child(Button::new("project-pull").outline().small().label("Pull latest into main…").on_click(move |_, window, cx| {
            let app = pull_model.clone(); let root = pull_root.clone();
            window.open_alert_dialog(cx, move |d, _, _| {
                let app = app.clone(); let root = root.clone();
                d.title("Update main checkout?").description("git pull --ff-only origin <default branch>
Only runs when the checkout is clean and on its default branch.").on_ok(move |_, _, cx| { app.update(cx, |m, cx| m.pull_project(root.clone(), cx)); true })
            });
        })))
        .child(div().text_sm().text_color(ui.text_muted).child("Workspaces"))
        .when(active.is_empty(), |el| el.child(div().text_color(ui.text_faint).child("No workspaces yet. Describe a change below to get started.")))
        .children(active.into_iter().map(|w| {
            let app = model.clone(); let id = w.id.clone();
            let mut providers = Vec::new();
            for tid in &w.threads {
                if let Some(t) = uuid::Uuid::parse_str(tid).ok().and_then(|id| m.threads.get(&id)) {
                    let backend = t.read(cx).meta.backend.clone();
                    if !providers.contains(&backend) { providers.push(backend); }
                }
            }
            let status = w.threads.iter().filter_map(|id| uuid::Uuid::parse_str(id).ok()).filter_map(|id| m.threads.get(&id)).find_map(|t| {
                let t = t.read(cx); if t.thread.presence.turn_active() { Some("Working") } else { None }
            }).unwrap_or("Ready");
            div().id(SharedString::from(w.id)).p_4().rounded(px(12.)).border_1().border_color(ui.border).flex().flex_col().gap_2()
                .child(div().flex().justify_between().child(div().flex().gap_2().items_center().children(providers.iter().map(|p| crate::views::brand::brand_mark(p, 16., true, ui))).child(div().text_size(px(16.)).child(w.name))).child(div().text_xs().text_color(ui.text_muted).child(status)))
                .child(div().text_xs().text_color(ui.text_faint).child(format!("{} · {} conversations · checkpoints on", w.branch, w.threads.len())))
                .child(Button::new(SharedString::from(format!("open-{id}"))).outline().small().label("Open workspace").on_click(move |_, _, cx| app.update(cx, |m, cx| m.open_workspace(id.clone(), cx))))
        }))
        .when(workspaces.iter().any(|w| w.archived_at.is_some()), |el| el.child(div().text_sm().text_color(ui.text_faint).child(format!("{} archived workspaces — available in the sidebar", workspaces.iter().filter(|w| w.archived_at.is_some()).count()))))
        .child(div().text_xs().text_color(ui.text_faint).child("New changes start in a separate workspace. Your main checkout stays untouched."))
        .into_any_element()
}

pub struct ReviewPanel {
    model: Entity<AppModel>,
}
impl ReviewPanel {
    pub fn new(model: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self { model }
    }
}
impl Render for ReviewPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::of(cx);
        let m = self.model.read(cx);
        let id = m.active_workspace.clone().unwrap_or_default();
        let w = m.workspaces.iter().find(|w| w.id == id).cloned();
        let review = m.review.clone();
        let loading = m.review_loading;
        let app = self.model.clone();
        let refresh = self.model.clone();
        let mut body = div()
            .id("workspace-review")
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .child(
                div()
                    .flex()
                    .gap_2()
                    .justify_between()
                    .child(div().text_size(px(18.)).child("Changes"))
                    .child(
                        Button::new("review-preview-tab")
                            .ghost()
                            .small()
                            .label("Preview")
                            .on_click(|_, window, cx| {
                                window
                                    .dispatch_action(Box::new(crate::actions::ToggleDevPreview), cx)
                            }),
                    )
                    .child(
                        Button::new("review-close")
                            .ghost()
                            .small()
                            .label("Close")
                            .on_click(move |_, _, cx| {
                                app.update(cx, |m, cx| {
                                    m.review_open = false;
                                    cx.notify();
                                })
                            }),
                    ),
            )
            .child(
                Button::new("review-refresh")
                    .ghost()
                    .small()
                    .label(if loading { "Refreshing…" } else { "Refresh" })
                    .on_click(move |_, _, cx| refresh.update(cx, |m, cx| m.refresh_review(cx))),
            );
        let (Some(w), Some(r)) = (w, review) else {
            return body
                .child("Select an active workspace to review its changes.")
                .into_any_element();
        };
        body = body.child(div().text_xs().text_color(ui.text_muted).child(format!(
            "{} · ↑{} ↓{} vs {} · {} files",
            r.branch,
            r.ahead,
            r.behind,
            r.base,
            r.files.len()
        )));
        if let Some(pr) = &r.pr {
            body = body.child(div().text_sm().text_color(ui.accent).child(pr.clone()));
        }
        if !r.conflicts.is_empty() {
            let app = self.model.clone();
            let prompt = format!(
                "Resolve the merge conflicts in these files, preserving the intended changes from both branches: {}",
                r.conflicts.join(", ")
            );
            body = body
                .child(
                    div()
                        .text_color(ui.warning)
                        .child(format!("Needs attention: {}", r.conflicts.join(", "))),
                )
                .child(
                    Button::new("resolve-conflicts")
                        .outline()
                        .label("Ask agent to resolve conflicts")
                        .on_click(move |_, _, cx| {
                            app.update(cx, |m, cx| m.send_prompt(prompt.clone(), vec![], cx))
                        }),
                );
        }
        if !w.inline && w.archived_at.is_none() {
            let mut actions = div().flex().flex_wrap().gap_2();
            for (action, label, command) in if r.remote {
                vec![(
                    "push",
                    "Push only",
                    format!("git push -u origin {}", w.branch),
                )]
            } else {
                vec![(
                    "merge",
                    "Merge into main locally",
                    format!(
                        "git merge --no-ff {} (in the clean project checkout)",
                        w.branch
                    ),
                )]
            } {
                let app = self.model.clone();
                let id = id.clone();
                actions = actions.child(
                    Button::new(SharedString::from(action))
                        .outline()
                        .small()
                        .label(label)
                        .on_click(move |_, window, cx| {
                            confirm_action(
                                app.clone(),
                                id.clone(),
                                action,
                                String::new(),
                                command.clone(),
                                window,
                                cx,
                            )
                        }),
                );
            }
            if r.remote {
                let app = self.model.clone();
                let wid = id.clone();
                let draft = r
                    .checkpoints
                    .iter()
                    .map(|(_, title)| format!("- {title}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                actions = actions.child(
                    Button::new("open-pr")
                        .outline()
                        .small()
                        .label("Push & open PR…")
                        .on_click(move |_, window, cx| {
                            text_action(
                                app.clone(),
                                wid.clone(),
                                "pr",
                                "Review PR body — OK pushes this branch and creates a pull request",
                                draft.clone(),
                                window,
                                cx,
                            )
                        }),
                );
            }
            let app = self.model.clone();
            let wid = id.clone();
            actions = actions.child(
                Button::new("checkpoint")
                    .ghost()
                    .small()
                    .label("Save checkpoint…")
                    .on_click(move |_, window, cx| {
                        text_action(
                            app.clone(),
                            wid.clone(),
                            "checkpoint",
                            "Checkpoint message",
                            "Save workspace changes".into(),
                            window,
                            cx,
                        )
                    }),
            );
            if r.checkpoints.len() > 1 {
                let app = self.model.clone();
                let wid = id.clone();
                actions = actions.child(Button::new("squash-checkpoints").ghost().small().label("Squash checkpoints…").on_click(move |_, window, cx| text_action(app.clone(), wid.clone(), "squash", "Squash message — git reset --soft to merge-base, then commit (backup branch kept)", "Complete workspace changes".into(), window, cx)));
            }
            let app = self.model.clone();
            let wid = id.clone();
            let path = w.path.clone();
            actions = actions.child(Button::new("archive-workspace").ghost().small().label("Archive…").on_click(move |_, window, cx| confirm_action(app.clone(), wid.clone(), "archive", String::new(), format!("git worktree remove {path}\nThe branch and conversations are kept. Unsaved changes prevent archiving."), window, cx)));
            body = body.child(actions);
        }
        let mut history = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_sm().child("Checkpoints"));
        for (sha, title) in &r.checkpoints {
            let app = self.model.clone();
            let wid = id.clone();
            let sha = sha.clone();
            let title = title.clone();
            let label = format!("Restore {}…", &sha[..8]);
            history = history.child(div().flex().flex_col().gap_1().child(div().text_xs().child(title))
                .child(Button::new(SharedString::from(format!("restore-{sha}"))).ghost().small().label(label).on_click(move |_, window, cx| confirm_action(app.clone(), wid.clone(), "restore", sha.clone(), format!("git restore --source {sha} --staged --worktree .\nRestore this milestone and save a new checkpoint. Existing history is kept."), window, cx))));
        }
        body.child(div().id("review-scroll").flex_1().overflow_y_scroll().flex().flex_col().gap_3()
            .child(history)
            .child(div().text_sm().child("Files changed"))
            .children(r.files.iter().enumerate().map(|(index, p)| {
                let app = self.model.clone(); let wid = id.clone(); let file = p.clone();
                let comment_app = self.model.clone(); let comment_file = p.clone(); let comment_id = id.clone();
                div().flex().flex_col().gap_1().child(div().text_xs().text_color(ui.text_muted).child(p.clone()))
                    .child(div().flex().gap_1()
                        .child(Button::new(("revert-file", index)).ghost().xsmall().label("Revert file…").on_click(move |_, window, cx| confirm_action(app.clone(), wid.clone(), "revert-file", file.clone(), format!("git restore --source <merge-base> --staged --worktree -- {file}\nRestore the original file and create a checkpoint."), window, cx)))
                        .child(Button::new(("comment-file", index)).ghost().xsmall().label("Add review comment…").on_click(move |_, window, cx| text_action(comment_app.clone(), comment_id.clone(), "comment", "Send a review comment to this conversation", format!("In {comment_file}: "), window, cx))))
            }))
            .child(div().font_family(ui.mono).text_size(px(11.)).children(r.diff.lines().map(|l| {
                let color = if l.starts_with('+') { ui.success } else if l.starts_with('-') { ui.danger } else { ui.text_muted };
                div().text_color(color).child(l.to_string())
            }))))
            .into_any_element()
    }
}
