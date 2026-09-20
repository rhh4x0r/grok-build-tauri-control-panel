//! Project board, live thread links and optional workflow controls.
use super::*;
impl FeaturesView {
    fn record_card(&self, record: &Record, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let feature = &record.feature;
        let edit = record.clone();
        let enabled = self.board.settings.enabled;
        let disposition = self
            .board
            .dispositions
            .get(&feature.id)
            .cloned()
            .unwrap_or_else(|| "Open".into());
        let weak = cx.entity().downgrade();
        let fid = feature.id.clone();
        let status_picker =
            Button::new(SharedString::from(format!("feature-state-{}", feature.id)))
                .ghost()
                .small()
                .label(disposition.clone())
                .disabled(!enabled || self.busy)
                .dropdown_caret(true)
                .tooltip("Your tracking status; changing it does not merge or deploy code")
                .dropdown_menu(move |mut menu, _, _| {
                    for status in ["Open", "Done", "Archived"] {
                        let weak = weak.clone();
                        let fid = fid.clone();
                        menu = menu.item(
                            PopupMenuItem::new(status)
                                .checked(disposition == status)
                                .on_click(move |_, _, cx| {
                                    let _ = weak.update(cx, |v, cx| {
                                        v.set_disposition(fid.clone(), status, cx)
                                    });
                                }),
                        );
                    }
                    menu
                });
        let mut card = crate::views::brand::handoff_card(ui)
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(17.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(feature.title.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(ui.text_faint)
                                    .child(feature.id.clone()),
                            ),
                    )
                    .child(status_picker)
                    .child(
                        Button::new(SharedString::from(format!("edit-{}", feature.id)))
                            .ghost()
                            .small()
                            .label("Edit brief")
                            .disabled(self.busy || !enabled)
                            .on_click(cx.listener(move |v, _, window, cx| {
                                v.edit(Some(edit.clone()), window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(ui.text_muted)
                    .child(feature.brief.lines().take(4).collect::<Vec<_>>().join("\n")),
            );
        for task in &feature.tasks {
            let key = features::task_key(&feature.id, &task.id);
            let run = self.board.runs.get(&key).cloned().unwrap_or_default();
            let thread = run.thread;
            let active = thread
                .and_then(|id| self.model.read(cx).threads.get(&id))
                .map(|t| t.read(cx).meta.clone());
            let status = if self.launching.contains(&key) {
                "Creating worktree…".into()
            } else if run.error.is_some() {
                format!(
                    "Last launch needs attention · {}",
                    run.error.as_deref().unwrap_or_default()
                )
            } else if let Some(t) = &active {
                if t.live
                    && ["running", "starting", "waiting_approval"].contains(&t.status.as_str())
                {
                    t.status.replace('_', " ")
                } else if run.ready_revision.as_deref() == Some(&record.revision) {
                    "Ready checkpoint recorded · revalidated before dependent tasks start".into()
                } else {
                    "Thread linked · review its progress".into()
                }
            } else if thread.is_some() {
                "Saved thread · open to continue".into()
            } else if !task.waits_for.is_empty() || task.review_of.is_some() {
                "Waiting for task checkpoint(s)".into()
            } else {
                "Ready to start".into()
            };
            let assignment = if thread.is_some() {
                run.applied.as_ref().or(run.assignment.as_ref())
            } else {
                task.assignment.as_ref()
            };
            let name = assignment
                .map(|a| {
                    format!(
                        "{}{} · {}",
                        if thread.is_some() { "At launch: " } else { "" },
                        self.model.read(cx).model_name(&a.backend, &a.model),
                        if a.effort.is_empty() {
                            "Reasoning not reported"
                        } else {
                            crate::views::brand::effort_label(&a.effort)
                        }
                    )
                })
                .unwrap_or_else(|| "Choose a model in Edit brief".into());
            let mut row = div()
                .border_t_1()
                .border_color(ui.border)
                .pt_3()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .child(task.title.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(ui.text_faint)
                                .child(task.role.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .when_some(assignment, |d, a| {
                            d.child(crate::views::brand::brand_mark(&a.backend, 16., true, ui))
                        })
                        .child(div().text_sm().text_color(ui.text_muted).child(name)),
                )
                .child(div().text_xs().text_color(ui.text_muted).child(status));
            let f = feature.id.clone();
            let t = task.id.clone();
            if let Some(id) = thread {
                row=row.child(div().flex().gap_2()
                    .child(Button::new(SharedString::from(format!("open-{key}"))).outline().small().label("Open thread").on_click(cx.listener(move|v,_,_,cx|v.open_thread(id,cx))))
                    .child(Button::new(SharedString::from(format!("ready-{key}"))).ghost().small().label("Ready for review").disabled(!enabled).tooltip("Record the current clean checkpoint; this does not merge or approve code").on_click(cx.listener(move|v,_,_,cx|v.ready(f.clone(),t.clone(),cx)))));
            } else {
                let connected = assignment.is_some_and(|a| {
                    self.candidates(cx)
                        .iter()
                        .any(|c| a.backend == c.backend && a.model == c.model)
                });
                let weak = cx.entity().downgrade();
                let af = feature.id.clone();
                let at = task.id.clone();
                let threads: Vec<_> = self
                    .model
                    .read(cx)
                    .threads
                    .iter()
                    .filter_map(|(id, t)| {
                        let t = t.read(cx);
                        if t.meta.project_root.as_deref() == Some(&self.project) {
                            Some((*id, t.title()))
                        } else {
                            None
                        }
                    })
                    .collect();
                row = row.child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new(SharedString::from(format!("start-{key}")))
                                .outline()
                                .small()
                                .icon(Lucide::Play)
                                .label("Start task")
                                .disabled(!enabled || !connected || self.launching.contains(&key))
                                .tooltip(if connected {
                                    "Start in a separate worktree with Ask permissions"
                                } else {
                                    "Select an available connected model in Edit brief"
                                })
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    v.start(f.clone(), t.clone(), cx)
                                })),
                        )
                        .child(
                            Button::new(SharedString::from(format!("attach-{key}")))
                                .ghost()
                                .small()
                                .label("Attach existing thread")
                                .disabled(!enabled || threads.is_empty())
                                .dropdown_caret(true)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for (id, title) in &threads {
                                        let id = *id;
                                        let f = af.clone();
                                        let t = at.clone();
                                        let weak = weak.clone();
                                        menu =
                                            menu.item(PopupMenuItem::new(title.clone()).on_click(
                                                move |_, _, cx| {
                                                    let _ = weak.update(cx, |v, cx| {
                                                        v.attach(f.clone(), t.clone(), id, cx)
                                                    });
                                                },
                                            ));
                                    }
                                    menu
                                }),
                        ),
                );
            }
            card = card.child(row);
        }
        card.into_any_element()
    }
}
impl Render for FeaturesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::of(cx);
        if self
            .suggestion_signature
            .as_ref()
            .is_some_and(|sig| *sig != self.signature(cx))
        {
            self.suggestion_signature = None;
            if let Some(e) = &mut self.editor {
                for t in &mut e.tasks {
                    t.proposal = None;
                    t.feedback = None;
                }
            }
        }
        if !self.loading && self.sync_guidelines {
            self.sync_guidelines = false;
            self.guidelines.update(cx, |s, cx| {
                s.set_value(self.board.settings.guidelines.clone(), window, cx)
            });
        }
        if self.new_after_load
            && !self.loading
            && self.board.settings.enabled
            && self.editor.is_none()
        {
            self.new_after_load = false;
            self.edit(None, window, cx);
        }
        let enabled = self.board.settings.enabled;
        let mut page = div()
            .id("project-features")
            .size_full()
            .overflow_y_scroll()
            .px_6()
            .py_4()
            .flex()
            .flex_col()
            .gap_4()
            .text_color(ui.text)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("features-back")
                            .ghost()
                            .small()
                            .icon(Lucide::ArrowLeft)
                            .label("Back to thread")
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.model.update(cx, |m, cx| {
                                    m.features_open = false;
                                    cx.notify();
                                })
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_sm()
                            .text_color(ui.text_muted)
                            .child(project_name(&self.project)),
                    ),
            )
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
                                    .text_size(px(24.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Features"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(ui.text_muted)
                                    .child("Capture ideas. Assign models. Work in parallel."),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("feature-preferences")
                                    .ghost()
                                    .small()
                                    .icon(Lucide::Settings)
                                    .tooltip("Project workflow & model preferences")
                                    .on_click(cx.listener(|v, _, _, cx| {
                                        v.preferences_open = !v.preferences_open;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("features-filter")
                                    .ghost()
                                    .small()
                                    .label(if self.show_completed {
                                        "All features"
                                    } else {
                                        "Open features"
                                    })
                                    .on_click(cx.listener(|v, _, _, cx| {
                                        v.show_completed = !v.show_completed;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("features-refresh")
                                    .ghost()
                                    .small()
                                    .icon(Lucide::RefreshCw)
                                    .disabled(self.loading || self.busy || self.editor.is_some())
                                    .tooltip("Reload project records and thread links")
                                    .on_click(cx.listener(|v, _, _, cx| v.reload(cx))),
                            )
                            .child(
                                Button::new("new-feature")
                                    .outline()
                                    .small()
                                    .icon(Lucide::Plus)
                                    .label("New feature")
                                    .disabled(
                                        self.busy
                                            || self.loading
                                            || !enabled
                                            || self.editor.is_some(),
                                    )
                                    .on_click(
                                        cx.listener(|v, _, window, cx| v.edit(None, window, cx)),
                                    ),
                            ),
                    ),
            );
        if let Some(message) = &self.message {
            page = page.child(
                crate::views::brand::handoff_card(&ui)
                    .child(div().text_sm().child(message.clone())),
            );
        }
        if !enabled && !self.loading {
            page=page.child(crate::views::brand::handoff_card(&ui).gap_3().child(div().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).child("Use features when they help"))
                .child(div().text_sm().text_color(ui.text_muted).child("Feature tracking is optional for every project, including imported repositories. Enabling it creates no files and starts no models. Saving a feature writes a Markdown brief under plan/features/. Existing repository instructions stay as they are."))
                .child(div().flex().gap_2().child(Button::new("enable-features").primary().label("Enable for this project").disabled(self.busy).on_click(cx.listener(|v,_,_,cx|v.save_preferences(true,false,cx))))
                    .child(Button::new("ordinary-thread").ghost().label("Use ordinary threads").on_click(|_,window,cx|window.dispatch_action(Box::new(crate::actions::NewThread),cx)))));
        }
        if self.preferences_open && enabled {
            page=page.child(crate::views::brand::handoff_card(&ui).gap_3().child(div().font_weight(FontWeight::SEMIBOLD).child("Project model preferences"))
                .when(self.editor.is_none(),|el|el.child(Textarea::new(&self.guidelines).disabled(self.busy)))
                .child(div().text_xs().text_color(ui.text_muted).child("Routing reads these preferences and the global Settings guidelines. Exact model + reasoning defaults can be saved from a feature's task cards. Suggestions always need your acceptance."))
                .child(div().flex().gap_2().flex_wrap()
                    .child(Button::new("save-feature-settings").outline().small().label("Save preferences").disabled(self.busy).on_click(cx.listener(|v,_,_,cx|v.save_preferences(true,false,cx))))
                    .child(Button::new("feature-guide").ghost().small().label("Create project guide").disabled(self.busy).on_click(cx.listener(|v,_,_,cx|v.create_guide(cx))))
                    .child(Button::new("feature-snapshot").ghost().small().label("Save status snapshot").disabled(self.busy).on_click(cx.listener(|v,_,_,cx|v.snapshot(cx))))
                    .child(Button::new("disable-features").ghost().small().label("Turn feature tracking off").disabled(self.busy||self.editor.is_some()).on_click(cx.listener(|v,_,_,cx|v.save_preferences(false,false,cx))))));
        }
        if self.loading {
            page = page.child(
                div()
                    .text_sm()
                    .text_color(ui.text_muted)
                    .child("Loading project records…"),
            );
        }
        if enabled && self.editor.is_some() {
            page = page.child(self.editor_panel(&ui, cx));
        } else if !self.loading {
            if self.board.records.is_empty() && enabled {
                page=page.child(crate::views::brand::handoff_card(&ui).child(div().text_sm().child("Start with an idea. One task is enough; add frontend, backend or review tasks when you want to split the work.")));
            }
            if !self.board.records.is_empty()
                && !self.show_completed
                && self.board.records.iter().all(|r| {
                    self.board
                        .dispositions
                        .get(&r.feature.id)
                        .is_some_and(|s| s != "Open")
                })
            {
                page=page.child(div().text_sm().text_color(ui.text_muted).child("No open features. Select Open features above to show completed and archived work."));
            }
            for record in &self.board.records {
                if self.show_completed
                    || self
                        .board
                        .dispositions
                        .get(&record.feature.id)
                        .is_none_or(|s| s == "Open")
                {
                    page = page.child(self.record_card(record, &ui, cx));
                }
            }
        }
        div()
            .size_full()
            .flex()
            .justify_center()
            .child(div().w_full().max_w(px(1040.)).min_h_0().child(page))
    }
}
