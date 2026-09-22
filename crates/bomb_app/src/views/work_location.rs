//! A working location and read-only access are independent choices.
use crate::views::button::Button;
use crate::{
    models::app::AppModel,
    runtime::{services, spawn_service},
    theme::Ui,
};
use bomb_core::services::git_ui::BranchChoices;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    button::ButtonVariants,
    menu::{DropdownMenu, PopupMenuItem},
    Sizable,
};
use gpui_kit::*;
pub struct WorkLocation {
    model: Entity<AppModel>,
    root: String,
    choices: BranchChoices,
    error: Option<String>,
}
impl WorkLocation {
    pub fn new(model: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self {
            model,
            root: String::new(),
            choices: Default::default(),
            error: None,
        }
    }
}
impl Render for WorkLocation {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::of(cx);
        let root = self
            .model
            .read(cx)
            .active_project
            .clone()
            .unwrap_or_default();
        if self.root != root {
            self.root = root.clone();
            self.model.update(cx,|m,_|{m.prefs.base_branch=None;m.prefs.existing_branch=None;if m.prefs.location=="branch"{m.prefs.location="new".into();m.prefs.worktree=true;}});
            self.choices = Default::default();
            self.error = None;
            let weak = cx.entity().downgrade();
            let core = crate::runtime::core_for_root(cx, &root);
            spawn_service(
                cx,
                async move {
                    (
                        root.clone(),
                        core.branches(&root).await,
                    )
                },
                move |(root, res), cx| {
                    let _ = weak.update(cx, |v, cx| {
                        if v.root == root {
                            match res {
                                Ok(c) => v.choices = c,
                                Err(e) => v.error = Some(e),
                            }
                            cx.notify();
                        }
                    });
                },
            );
        }
        let prefs = self.model.read(cx).prefs.clone();
        let current = if self.choices.current.is_empty() {
            "current checkout".into()
        } else {
            self.choices.current.clone()
        };
        let label = match prefs.location.as_str() {
            "checkout" => format!("Work in · {current} directly"),
            "branch" => format!(
                "Work in · {}",
                prefs.existing_branch.as_deref().unwrap_or("Choose branch")
            ),
            _ => "Work in · New branch".into(),
        };
        let app = self.model.clone();
        let branches = self.choices.branches.clone();
        let checkout = current.clone();
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .text_size(px(crate::theme::Type::SMALL))
            .text_color(ui.text_muted)
            .child(
                Button::new("work-location")
                    .ghost()
                    .small()
                    .icon(Lucide::GitBranch)
                    .label(label)
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut menu, _, _| {
                        let a = app.clone();
                        menu = menu.item(
                            PopupMenuItem::new("New branch · separate working copy").on_click(
                                move |_, _, cx| {
                                    a.update(cx, |m, cx| {
                                        m.prefs.location = "new".into();
                                        m.prefs.worktree = true;
                                        m.prefs.temporary = false;
                                        m.prefs.existing_branch = None;
                                        cx.notify();
                                    })
                                },
                            ),
                        );
                        let a = app.clone();
                        menu = menu.item(
                            PopupMenuItem::new(format!("Current checkout · {checkout} (shared)"))
                                .on_click(move |_, _, cx| {
                                    a.update(cx, |m, cx| {
                                        m.prefs.location = "checkout".into();
                                        m.prefs.worktree = false;
                                        m.prefs.temporary = false;
                                        m.prefs.existing_branch = None;
                                        cx.notify();
                                    })
                                }),
                        );
                        for b in &branches {
                            let a = app.clone();
                            let branch = b.clone();
                            menu = menu.item(
                                PopupMenuItem::new(format!("Existing branch · {b}")).on_click(
                                    move |_, _, cx| {
                                        a.update(cx, |m, cx| {
                                            m.prefs.location = "branch".into();
                                            m.prefs.existing_branch = Some(branch.clone());
                                            m.prefs.worktree = false;
                                            m.prefs.temporary = false;
                                            cx.notify();
                                        })
                                    },
                                ),
                            );
                        }
                        menu
                    }),
            );
        if prefs.location == "new" {
            let branches = self.choices.branches.clone();
            let app = self.model.clone();
            let default = if self.choices.base.is_empty() { "the default branch".to_string() } else { self.choices.base.clone() };
            // Open threads in this project whose branch has saved changes not on the default branch yet.
            let (threads, ahead_of): (Vec<(String, String)>, std::collections::HashMap<String, usize>) = {
                let m = self.model.read(cx);
                let ahead: std::collections::HashMap<String, usize> = m
                    .project_overviews
                    .get(&self.root)
                    .and_then(|o| o.as_ref().ok())
                    .map(|o| o.branches.iter().map(|b| (b.name.clone(), b.ahead)).collect())
                    .unwrap_or_default();
                let threads = m
                    .workspaces
                    .iter()
                    .filter(|w| w.project_root == self.root && !w.inline && !w.shared_checkout && w.archived_at.is_none())
                    .filter(|w| ahead.get(&w.branch).copied().unwrap_or(0) > 0)
                    .map(|w| (w.name.clone(), w.branch.clone()))
                    .collect();
                (threads, ahead)
            };
            let chosen = prefs.base_branch.clone().unwrap_or_else(|| self.choices.base.clone());
            let label = match threads.iter().find(|(_, b)| *b == chosen) {
                Some((name, _)) => format!("Continue from {name}"),
                None => format!("Start from {}", if chosen.is_empty() { default.clone() } else { chosen.clone() }),
            };
            let default_branch = self.choices.base.clone();
            let builds_on = threads.iter().find(|(_, b)| *b == chosen).map(|(n, _)| n.clone());
            row = row.child(
                Button::new("base-branch")
                    .ghost()
                    .small()
                    .label(label)
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut menu, _, _| {
                        let a = app.clone();
                        menu = menu.item(
                            PopupMenuItem::new(format!("Start from {default}"))
                                .checked(chosen == default_branch)
                                .on_click(move |_, _, cx| {
                                    a.update(cx, |m, cx| { m.prefs.base_branch = None; cx.notify(); })
                                }),
                        );
                        if !threads.is_empty() {
                            menu = menu.separator();
                        }
                        for (name, branch) in &threads {
                            let a = app.clone();
                            let b = branch.clone();
                            let n = ahead_of.get(branch).copied().unwrap_or(0);
                            let note = if n == 1 { "1 saved change not in main yet".to_string() } else { format!("{n} saved changes not in main yet") };
                            menu = menu.item(
                                PopupMenuItem::new(format!("Continue from {name} · {note}"))
                                    .checked(chosen == *branch)
                                    .on_click(move |_, _, cx| {
                                        a.update(cx, |m, cx| { m.prefs.base_branch = Some(b.clone()); cx.notify(); })
                                    }),
                            );
                        }
                        let others: Vec<String> = branches
                            .iter()
                            .filter(|b| **b != default_branch && !threads.iter().any(|(_, t)| t == *b))
                            .cloned()
                            .collect();
                        if !others.is_empty() {
                            menu = menu.separator();
                        }
                        for b in others {
                            let a = app.clone();
                            let branch = b.clone();
                            menu = menu.item(
                                PopupMenuItem::new(format!("Branch · {b}"))
                                    .checked(chosen == b)
                                    .on_click(move |_, _, cx| {
                                        a.update(cx, |m, cx| { m.prefs.base_branch = Some(branch.clone()); cx.notify(); })
                                    }),
                            );
                        }
                        menu
                    }),
            );
            if let Some(name) = builds_on {
                row = row.child(format!("Builds on {name} · merge that thread first, then this one"));
            }
        }
        let app = self.model.clone();
        row = row.child(
            Button::new("location-access")
                .ghost()
                .small()
                .icon(if prefs.read_only {
                    Lucide::ShieldCheck
                } else {
                    Lucide::Pencil
                })
                .label(if prefs.read_only {
                    "Read-only"
                } else {
                    "Can edit files"
                })
                .on_click(move |_, _, cx| {
                    app.update(cx, |m, cx| {
                        m.prefs.read_only = !m.prefs.read_only;
                        if m.prefs.read_only {
                            m.prefs.mode = "plan".into();
                        }
                        cx.notify();
                    })
                }),
        );
        if prefs.location == "checkout" {
            let shared = services(cx)
                .persistence
                .list_workspaces()
                .unwrap_or_default()
                .iter()
                .filter(|w| w.path == self.root)
                .map(|w| w.threads.len())
                .sum::<usize>();
            row = row.child(format!(
                "Shared working copy · {shared} existing conversations"
            ));
        }
        row
    }
}
