//! Project overview and workspace review controls.
use crate::models::app::AppModel;
use crate::theme::Ui;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{Disableable, Sizable, WindowExt};
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
        d.title("Confirm thread action")
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
        let rows = if matches!(action, "pr" | "comment") {
            3
        } else {
            1
        };
        let mut s = TextareaState::new(window, cx)
            .auto_grow(rows, 12)
            .submit_on_enter(false);
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
                    if action == "comment" {
                        m.send_prompt(text, vec![], cx);
                    } else {
                        m.run_workspace_action(id.clone(), action.clone(), text, cx);
                    }
                });
                window.close_dialog(cx);
                true
            })
    });
}

pub use super::project::project_page;

pub fn commit_dialog(
    model: Entity<AppModel>,
    id: String,
    files: Vec<String>,
    window: &mut Window,
    cx: &mut App,
) {
    let input = cx.new(|cx| {
        let mut s = TextareaState::new(window, cx)
            .auto_grow(1, 3)
            .submit_on_enter(false);
        s.set_value("Update thread changes", window, cx);
        s
    });
    window.open_dialog(cx, move |d, _, _| {
        let (model, id, files, input) = (model.clone(), id.clone(), files.clone(), input.clone());
        let content = input.clone();
        let listed = files.clone();
        d.title(format!("Commit {} selected files", files.len()))
            .w(px(640.))
            .content(move |c, _, _| {
                c.child(
                    div()
                        .max_h(px(180.))
                        .id("commit-files")
                        .overflow_y_scroll()
                        .children(listed.iter().map(|p| div().text_sm().child(p.clone()))),
                )
                .child(Textarea::new(&content))
                .child("Creates a local commit. Nothing is pushed.")
            })
            .on_ok(move |_, window, cx| {
                let message = input.read(cx).value().to_string();
                if message.trim().is_empty() || files.is_empty() {
                    return false;
                }
                let value = serde_json::json!({"message":message,"files":files}).to_string();
                model.update(cx, |m, cx| {
                    m.run_workspace_action(id.clone(), "commit-selected".into(), value, cx)
                });
                window.close_dialog(cx);
                true
            })
    });
}
pub fn pr_dialog(
    model: Entity<AppModel>,
    id: String,
    branch: String,
    base: String,
    window: &mut Window,
    cx: &mut App,
) {
    let title = cx.new(|cx| {
        let mut s = TextareaState::new(window, cx)
            .auto_grow(1, 2)
            .submit_on_enter(false);
        s.set_value(format!("Changes from {branch}"), window, cx);
        s
    });
    let target = cx.new(|cx| {
        let mut s = TextareaState::new(window, cx)
            .auto_grow(1, 1)
            .submit_on_enter(false);
        s.set_value(base, window, cx);
        s
    });
    let body = cx.new(|cx| {
        TextareaState::new(window, cx)
            .auto_grow(3, 8)
            .submit_on_enter(false)
    });
    window.open_dialog(cx,move|d,_,_|{let (model,id,title,target,body)=(model.clone(),id.clone(),title.clone(),target.clone(),body.clone());let (t,b,to)=(title.clone(),body.clone(),target.clone());let source=branch.clone();
        d.title("Create pull request").w(px(680.)).content(move|c,_,_|c.child(format!("Source: {source} → Target branch" )).child(Textarea::new(&to)).child("Title").child(Textarea::new(&t)).child("Description").child(Textarea::new(&b)).child("Publishes this branch to origin, then creates the pull request."))
        .on_ok(move|_,window,cx|{let title=title.read(cx).value().to_string();let base=target.read(cx).value().to_string();if title.trim().is_empty()||base.trim().is_empty(){return false;}let value=serde_json::json!({"title":title,"body":body.read(cx).value().to_string(),"base":base}).to_string();model.update(cx,|m,cx|m.run_workspace_action(id.clone(),"pr".into(),value,cx));window.close_dialog(cx);true})
    });
}

pub struct ReviewPanel {
    model: Entity<AppModel>,
    branch_tab: bool,
    history: bool,
    excluded: std::collections::HashSet<String>,
    workspace: String,
    diff: Option<(String, String)>,
    diff_busy: bool,
}
impl ReviewPanel {
    pub fn new(model: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        // Refresh local Git state while the panel is visible, including external edits.
        cx.spawn(async move |weak, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(5))
                .await;
            let alive = weak.update(cx, |v, cx| {
                v.model.update(cx, |m, cx| {
                    if m.review_open && !m.review_loading {
                        m.refresh_review(cx);
                    }
                });
            });
            if alive.is_err() {
                break;
            }
        })
        .detach();
        Self {
            model,
            branch_tab: false,
            history: false,
            excluded: Default::default(),
            workspace: String::new(),
            diff: None,
            diff_busy: false,
        }
    }
    fn open_diff(&mut self, cwd: String, reference: String, file: String, cx: &mut Context<Self>) {
        self.diff = Some((file.clone(), "Loading diff…".into()));
        self.diff_busy = true;
        let selected_file=file.clone();
        let weak = cx.entity().downgrade();
        let wid = self.workspace.clone();
        crate::runtime::spawn_service(
            cx,
            async move { bomb_core::services::git_ui::file_diff(&cwd, &reference, &file).await },
            move |result, cx| {
                let _ = weak.update(cx, |v, cx| {
                    if v.workspace == wid {
                        if let Some((selected, body)) = &mut v.diff {
                            if *selected != selected_file {return;}
                            *body = result.unwrap_or_else(|e| e);
                        }
                        v.diff_busy = false;
                        cx.notify();
                    }
                });
            },
        );
        cx.notify();
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
        let busy=m.git_busy;
        if self.workspace != id {
            self.workspace = id.clone();
            self.excluded.clear();
            self.diff = None;
            self.history = false;
        }
        let app = self.model.clone();
        let mut body = div()
            .id("changes-panel")
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .p_4()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Changes"),
                    )
                    .child(
                        Button::new("refresh-changes")
                            .ghost()
                            .small()
                            .icon(Lucide::RefreshCw)
                            .tooltip("Refresh Git state")
                            .disabled(loading)
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.model.update(cx, |m, cx| m.refresh_review(cx))
                            })),
                    )
                    .child(
                        Button::new("close-changes")
                            .ghost()
                            .small()
                            .icon(Lucide::X)
                            .tooltip("Close Changes")
                            .on_click(move |_, _, cx| {
                                app.update(cx, |m, cx| {
                                    m.review_open = false;
                                    cx.notify();
                                })
                            }),
                    ),
            );
        let (Some(w), Some(r)) = (w, review) else {
            return body
                .child(if loading {
                    "Loading changes…"
                } else {
                    "Select a Git thread to review changes."
                })
                .into_any_element();
        };
        if let Some((file, diff)) = &self.diff {
            return body
                .child(
                    Button::new("diff-back")
                        .ghost()
                        .small()
                        .icon(Lucide::ArrowLeft)
                        .label("Back to files")
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.diff = None;
                            cx.notify();
                        })),
                )
                .child(div().text_sm().child(file.clone()))
                .child(
                    div()
                        .id("file-diff-scroll")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(
                            TextView::markdown(
                                "selected-file-diff",
                                format!("```diff\n{diff}\n```"),
                            )
                            .selectable(true),
                        ),
                )
                .into_any_element();
        }
        let files = if self.branch_tab {
            &r.branch_files
        } else {
            &r.dirty
        };
        body = body
            .child(div().text_sm().text_color(ui.text_muted).child(format!(
                "{} · {} files · +{} −{}",
                r.branch,
                files.len(),
                files.iter().map(|f| f.added).sum::<usize>(),
                files.iter().map(|f| f.removed).sum::<usize>()
            )))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("uncommitted-tab")
                            .small()
                            .label(format!("Uncommitted ({})", r.dirty.len()))
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.branch_tab = false;
                                v.history = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("branch-tab")
                            .ghost()
                            .small()
                            .label(format!("Branch changes ({})", r.branch_files.len()))
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.branch_tab = true;
                                v.history = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(ui.text_faint)
                    .child(if self.branch_tab {
                        format!(
                            "Compared with {} · {} ahead, {} behind",
                            r.default_branch, r.ahead, r.behind
                        )
                    } else {
                        "Choose files to commit. Click a filename to inspect its diff.".into()
                    }),
            );
        let mut list = div()
            .id("change-files")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1();
        if files.is_empty() {
            list = list.child(div().p_4().text_sm().text_color(ui.text_muted).child(
                if self.branch_tab {
                    "No changes compared with the base branch."
                } else {
                    "Working copy clean. All changes are committed."
                },
            ));
        }
        for (i, file) in files.iter().enumerate() {
            let path = file.path.clone();
            let pick = path.clone();
            let cwd = w.path.clone();
            let reference = if self.branch_tab {
                r.comparison.clone()
            } else {
                "HEAD".into()
            };
            let selected = !self.excluded.contains(&path);
            let mut row = div().flex().items_center().gap_1().py_1();
            if !self.branch_tab {
                row = row.child(
                    Button::new(("select-file", i))
                        .ghost()
                        .small()
                        .icon(if selected {
                            Lucide::Check
                        } else {
                            Lucide::Circle
                        })
                        .tooltip("Include in commit")
                        .on_click(cx.listener(move |v, _, _, cx| {
                            if !v.excluded.remove(&pick) {
                                v.excluded.insert(pick.clone());
                            }
                            cx.notify();
                        })),
                );
            }
            row = row
                .child(
                    Button::new(("inspect-file", i))
                        .ghost()
                        .small()
                        .icon(Lucide::File)
                        .label(format!("{}  {}", file.status, path))
                        .on_click(cx.listener(move |v, _, _, cx| {
                            v.open_diff(cwd.clone(), reference.clone(), path.clone(), cx)
                        })),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .text_xs()
                        .text_color(ui.text_muted)
                        .child(format!("+{} −{}", file.added, file.removed)),
                );
            list = list.child(row);
        }
        if self.history {
            list = list.child(div().mt_3().text_sm().child("Commit history"));
            for (sha, title) in &r.checkpoints {
                list = list.child(
                    div()
                        .text_xs()
                        .text_color(ui.text_muted)
                        .child(format!("{}  {title}", &sha[..sha.len().min(8)])),
                );
                if !w.shared_checkout && !w.inline && !w.read_only {
                    let (app, id, sha) = (self.model.clone(), id.clone(), sha.clone());
                    list=list.child(Button::new(SharedString::from(format!("restore-{sha}"))).ghost().small().label("Restore this commit…").on_click(move|_,window,cx|confirm_action(app.clone(),id.clone(),"restore",sha.clone(),format!("Replace this working copy with commit {sha} and record a new commit. History is preserved."),window,cx)));
                }
            }
        }
        body = body.child(list);
        if !r.conflicts.is_empty() {
            body=body.child(div().text_sm().text_color(ui.warning).child(format!("Resolve {} conflicting files before continuing.",r.conflicts.len()))).child(Button::new("resolve-conflicts").ghost().small().label("Ask agent to resolve conflicts").on_click({let app=self.model.clone();let prompt=format!("Resolve conflicts in {}, preserving intended changes from both branches.",r.conflicts.join(", "));move|_,_,cx|app.update(cx,|m,cx|m.send_prompt(prompt.clone(),vec![],cx))}));
        }
        let can_edit = !w.inline && !w.read_only && w.archived_at.is_none();
        let mut footer = div()
            .border_t_1()
            .border_color(ui.border)
            .pt_3()
            .flex()
            .flex_col()
            .gap_2();
        if can_edit && !r.dirty.is_empty() {
            let selected: Vec<_> = r
                .dirty
                .iter()
                .filter(|f| !self.excluded.contains(&f.path))
                .map(|f| f.path.clone())
                .collect();
            let (app, id) = (self.model.clone(), id.clone());
            footer = footer.child(
                Button::new("commit-changes")
                    .primary()
                    .label(format!("Commit {} files…", selected.len()))
                    .disabled(busy || selected.is_empty() || !r.conflicts.is_empty())
                    .on_click(move |_, window, cx| {
                        commit_dialog(app.clone(), id.clone(), selected.clone(), window, cx)
                    }),
            );
        } else if can_edit && r.remote && r.main_unpushed>0 && r.ahead==0 && r.branch!=r.default_branch {
            let(app,id,base)=(self.model.clone(),id.clone(),r.default_branch.clone());
            footer=footer.child(Button::new("push-merged-main").primary().label(format!("Push {base} to origin…")).on_click(move|_,window,cx|confirm_action(app.clone(),id.clone(),"push-main",String::new(),format!("Publish local {base} to origin/{base}."),window,cx)));
        } else if can_edit && !r.remote && r.branch!=r.default_branch && r.ahead>0 {
            let(app,id,base,branch)=(self.model.clone(),id.clone(),r.default_branch.clone(),r.branch.clone());
            footer=footer.child(Button::new("merge-local").primary().label(format!("Merge into {base}…")).on_click(move|_,window,cx|confirm_action(app.clone(),id.clone(),"merge",String::new(),format!("Merge {branch} → local {base}. Nothing will be pushed."),window,cx)));
        } else if can_edit && r.remote && (r.upstream.is_none() || r.unpushed > 0) {
            let (app, id, branch) = (self.model.clone(), id.clone(), r.branch.clone());
            footer = footer.child(
                Button::new("push-branch")
                    .primary()
                    .label(format!("Push {}", r.branch))
                    .on_click(move |_, window, cx| {
                        confirm_action(
                            app.clone(),
                            id.clone(),
                            "push",
                            String::new(),
                            format!("Publish local commits from {branch} to origin/{branch}."),
                            window,
                            cx,
                        )
                    }),
            );
        } else if can_edit && r.remote && r.branch != r.default_branch && r.pr.is_none() {
            let (app, id, branch, base) = (
                self.model.clone(),
                id.clone(),
                r.branch.clone(),
                r.default_branch.clone(),
            );
            footer = footer.child(
                Button::new("create-pr")
                    .primary()
                    .label("Create pull request…")
                    .on_click(move |_, window, cx| {
                        pr_dialog(
                            app.clone(),
                            id.clone(),
                            branch.clone(),
                            base.clone(),
                            window,
                            cx,
                        )
                    }),
            );
        }
        if let Some(pr) = &r.pr_url {
            let url = pr.clone();
            footer = footer.child(
                Button::new("view-pr")
                    .ghost()
                    .small()
                    .label("Open pull request")
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            );
        }
        footer = footer.child(
            div()
                .text_xs()
                .text_color(ui.text_muted)
                .child(if !r.remote {
                    "No remote configured · commits stay local".into()
                } else if let Some(up) = &r.upstream {
                    format!(
                        "{up} · {} not pushed · {} to pull",
                        r.unpushed, r.remote_behind
                    )
                } else {
                    "Branch not published to origin".into()
                }),
        );
        let app = self.model.clone();
        let wid = id.clone();
        let base = r.default_branch.clone();
        let branch = r.branch.clone();
        let remote = r.remote;
        let shared = w.shared_checkout;
        let count = r.checkpoints.len();
        let weak = cx.entity().downgrade();
        footer=footer.child(Button::new("git-more").ghost().small().icon(Lucide::Ellipsis).label("More").dropdown_menu(move|mut menu,_,_|{
            let weak=weak.clone();menu=menu.item(PopupMenuItem::new("Commit history").on_click(move|_,_,cx|{let _=weak.update(cx,|v,cx|{v.history = !v.history;cx.notify();});}));
            if can_edit{
                if remote && branch!=base {let(app,id,branch,base)=(app.clone(),wid.clone(),branch.clone(),base.clone());menu=menu.item(PopupMenuItem::new("Create pull request…").on_click(move|_,window,cx|pr_dialog(app.clone(),id.clone(),branch.clone(),base.clone(),window,cx)));}
                if branch!=base{let(app,id,base,branch)=(app.clone(),wid.clone(),base.clone(),branch.clone());menu=menu.item(PopupMenuItem::new(format!("Merge into {base} locally…")).on_click(move|_,window,cx|confirm_action(app.clone(),id.clone(),"merge",String::new(),format!("Merge {branch} → local {base}. Both working copies must be clean. Nothing will be pushed."),window,cx)));}
                if remote{let(app,id,base)=(app.clone(),wid.clone(),base.clone());menu=menu.item(PopupMenuItem::new(format!("Push {base} to origin…")).on_click(move|_,window,cx|confirm_action(app.clone(),id.clone(),"push-main",String::new(),format!("Publish commits on local {base} to origin/{base}."),window,cx)));}
                let(app2,id2,base2)=(app.clone(),wid.clone(),base.clone());menu=menu.item(PopupMenuItem::new(format!("Update from {base}…")).on_click(move|_,window,cx|confirm_action(app2.clone(),id2.clone(),"update",String::new(),format!("Fetch and merge the latest {base2} into this thread."),window,cx)));
                if !shared&&count>1{let(app,id)=(app.clone(),wid.clone());menu=menu.item(PopupMenuItem::new("Squash local commits…").on_click(move|_,window,cx|text_action(app.clone(),id.clone(),"squash","Squash unpublished commits (backup kept)","Complete thread changes".into(),window,cx)));}
            }menu
        }));
        if r.main_unpushed > 0 && r.branch != r.default_branch {
            footer = footer.child(div().text_xs().child(format!(
                "Local {} has {} commits not pushed",
                r.default_branch, r.main_unpushed
            )));
        }
        if busy {footer=footer.child(div().text_sm().child("Applying Git action…"));}
        body.child(footer).into_any_element()
    }
}

pub fn branch_control(model: Entity<AppModel>, cx: &mut App) -> AnyElement {
    let m = model.read(cx);
    let review = m.review.clone();
    let w = m
        .active_workspace
        .as_ref()
        .and_then(|id| m.workspaces.iter().find(|w| &w.id == id))
        .cloned();
    let Some(w) = w else {
        return div().into_any_element();
    };
    let branch = review
        .as_ref()
        .map(|r| r.branch.clone())
        .unwrap_or_else(|| w.branch.clone());
    let dirty = review.as_ref().map(|r| r.dirty.len()).unwrap_or(0);
    let shared = w.shared_checkout;
    let label = if shared {
        format!("{branch} · Direct checkout")
    } else {
        branch.clone()
    };
    Button::new("thread-branch")
        .ghost()
        .small()
        .icon(Lucide::GitBranch)
        .label(if dirty > 0 {
            format!("{label} · {dirty} uncommitted")
        } else {
            label
        })
        .dropdown_caret(true)
        .dropdown_menu(move |mut menu, _, _| {
            menu =
                menu.item(PopupMenuItem::new(format!("Working branch: {branch}")).disabled(true)).item(PopupMenuItem::new(format!("Base reference: {}",w.base_ref)).disabled(true));
            if let Some(r) = &review {
                menu = menu
                    .item(
                        PopupMenuItem::new(format!(
                            "Compared with {}: {} ahead · {} behind",
                            r.default_branch, r.ahead, r.behind
                        ))
                        .disabled(true),
                    )
                    .item(
                        PopupMenuItem::new(if let Some(up) = &r.upstream {
                            format!(
                                "Remote {up}: {} not pushed · {} to pull",
                                r.unpushed, r.remote_behind
                            )
                        } else {
                            "Remote: branch not published".into()
                        })
                        .disabled(true),
                    );
            }
            if shared {
                menu = menu.item(
                    PopupMenuItem::new(format!(
                        "Shared working copy · {} conversations",
                        w.threads.len()
                    ))
                    .disabled(true),
                );
            }
            let app = model.clone();
            menu = menu.item(PopupMenuItem::new("View changes & history").on_click(
                move |_, _, cx| {
                    app.update(cx, |m, cx| {
                        m.review_open = true;
                        m.refresh_review(cx);
                        cx.notify();
                    })
                },
            ));
            let path = w.path.clone();
            menu.item(
                PopupMenuItem::new("Open working folder").on_click(move |_, _, _| {
                    let _ = std::process::Command::new("open").arg(&path).spawn();
                }),
            )
        })
        .into_any_element()
}
