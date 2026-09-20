//! Inline feature brief, task assignments and routing proposals.
use super::*;

impl FeaturesView {
    fn task_card(&self, index: usize, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let e = self.editor.as_ref().unwrap();
        let t = &e.tasks[index];
        let linked = self
            .board
            .runs
            .get(&features::task_key(&e.feature.id, &t.task.id))
            .is_some_and(|r| r.thread.is_some());
        let role = t.task.role.clone();
        let assignment = t.task.assignment.clone();
        let candidates = self.candidates(cx);
        let label = assignment
            .as_ref()
            .map(|a| self.model.read(cx).model_name(&a.backend, &a.model))
            .unwrap_or_else(|| "Choose model".into());
        let weak = cx.entity().downgrade();
        let picker = Button::new(SharedString::from(format!("task-model-{index}")))
            .ghost()
            .small()
            .disabled(self.busy || linked)
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for candidate in &candidates {
                    let candidate = candidate.clone();
                    let weak = weak.clone();
                    let checked = assignment.as_ref().is_some_and(|a| {
                        a.backend == candidate.backend && a.model == candidate.model
                    });
                    menu = menu.item(
                        PopupMenuItem::new(candidate.label.clone())
                            .checked(checked)
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |v, cx| {
                                    if let Some(t) =
                                        v.editor.as_mut().and_then(|e| e.tasks.get_mut(index))
                                    {
                                        t.task.assignment =
                                            Some(Assignment::from_candidate(&candidate));
                                        t.proposal = None;
                                        t.feedback = None;
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            });
        let a = t.task.assignment.clone();
        let effort = a.as_ref().map(|a| a.effort.clone()).unwrap_or_default();
        let levels = crate::views::brand::effort_levels(
            a.as_ref().map(|a| a.backend.as_str()).unwrap_or(""),
        )
        .0;
        let weak = cx.entity().downgrade();
        let effort_picker = Button::new(SharedString::from(format!("task-effort-{index}")))
            .ghost()
            .small()
            .disabled(self.busy || linked || a.is_none())
            .label(format!(
                "Reasoning: {}",
                crate::views::brand::effort_label(&effort)
            ))
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for level in levels {
                    let weak = weak.clone();
                    let level = *level;
                    menu = menu.item(
                        PopupMenuItem::new(crate::views::brand::effort_label(level))
                            .checked(effort == level)
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |v, cx| {
                                    if let Some(a) = v
                                        .editor
                                        .as_mut()
                                        .and_then(|e| e.tasks.get_mut(index))
                                        .and_then(|t| t.task.assignment.as_mut())
                                    {
                                        a.effort = level.into();
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            });
        let task_id = t.task.id.clone();
        let peers: Vec<_> = e
            .tasks
            .iter()
            .filter(|p| p.task.id != task_id)
            .map(|p| (p.task.id.clone(), p.title.read(cx).value().to_string()))
            .collect();
        let waits = t.task.waits_for.clone();
        let review = t.task.review_of.clone();
        let weak = cx.entity().downgrade();
        let dependency = Button::new(SharedString::from(format!("task-dep-{index}")))
            .ghost()
            .small()
            .disabled(self.busy || peers.is_empty())
            .label(if role == "Review" {
                review
                    .as_ref()
                    .and_then(|id| peers.iter().find(|p| &p.0 == id))
                    .map(|p| format!("Review: {}", p.1))
                    .unwrap_or_else(|| "Choose task to review".into())
            } else if waits.is_empty() {
                "Can start independently".into()
            } else {
                format!("Waits for {} task(s)", waits.len())
            })
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for (id, title) in &peers {
                    let id = id.clone();
                    let weak = weak.clone();
                    let is_review = role == "Review";
                    let checked = if is_review {
                        review.as_ref() == Some(&id)
                    } else {
                        waits.contains(&id)
                    };
                    menu = menu.item(PopupMenuItem::new(title.clone()).checked(checked).on_click(
                        move |_, _, cx| {
                            let _ = weak.update(cx, |v, cx| {
                                if let Some(t) =
                                    v.editor.as_mut().and_then(|e| e.tasks.get_mut(index))
                                {
                                    if is_review {
                                        t.task.review_of = Some(id.clone());
                                    } else if t.task.waits_for.contains(&id) {
                                        t.task.waits_for.retain(|dep| dep != &id);
                                    } else {
                                        t.task.waits_for.push(id.clone());
                                    }
                                }
                                cx.notify();
                            });
                        },
                    ));
                }
                menu
            });
        let removable = !self
            .board
            .runs
            .get(&features::task_key(&e.feature.id, &t.task.id))
            .is_some_and(|r| r.thread.is_some());
        let mut card = crate::views::brand::handoff_card(ui)
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(ui.text_muted)
                            .child(t.task.role.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&t.title).disabled(self.busy)),
                    )
                    .child(
                        Button::new(SharedString::from(format!("remove-task-{index}")))
                            .ghost()
                            .small()
                            .icon(Lucide::X)
                            .disabled(self.busy || !removable)
                            .tooltip("Remove this unstarted task")
                            .on_click(cx.listener(move |v, _, _, cx| {
                                if let Some(e) = &mut v.editor {
                                    let id = e.tasks.remove(index).task.id;
                                    for t in &mut e.tasks {
                                        t.task.waits_for.retain(|x| x != &id);
                                        if t.task.review_of.as_ref() == Some(&id) {
                                            t.task.review_of = None;
                                        }
                                    }
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(Textarea::new(&t.brief).disabled(self.busy))
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .when_some(t.task.assignment.as_ref(), |d, a| {
                        d.child(crate::views::brand::brand_mark(&a.backend, 18., true, ui))
                    })
                    .child(picker)
                    .child(effort_picker)
                    .child(dependency),
            );
        if let Some(proposal) = &t.proposal {
            let proposed = proposal.clone();
            let name = self
                .model
                .read(cx)
                .model_name(&proposal.backend, &proposal.model);
            card = card.child(
                div()
                    .border_t_1()
                    .border_color(ui.border)
                    .pt_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_wrap()
                    .child(crate::views::brand::model_identity(
                        &proposal.backend,
                        format!(
                            "Suggested: {name} · {}",
                            crate::views::brand::effort_label(&proposal.effort)
                        ),
                        ui,
                    ))
                    .child(
                        Button::new(SharedString::from(format!("accept-model-{index}")))
                            .outline()
                            .small()
                            .label("Use suggestion")
                            .disabled(self.busy)
                            .on_click(cx.listener(move |v, _, _, cx| {
                                if v.suggestion_signature.as_deref()!=Some(v.signature(cx).as_str()) {
                                    v.message=Some("This suggestion is out of date. Request fresh suggestions for the current draft.".into());cx.notify();return;
                                }
                                if let Some(t) =
                                    v.editor.as_mut().and_then(|e| e.tasks.get_mut(index))
                                {
                                    t.task.assignment = Some(proposed.clone());
                                    t.proposal = None;
                                }
                                v.suggestion_signature = Some(v.signature(cx));
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("keep-model-{index}")))
                            .ghost()
                            .small()
                            .label("Keep selection")
                            .on_click(cx.listener(move |v, _, _, cx| {
                                if let Some(t) =
                                    v.editor.as_mut().and_then(|e| e.tasks.get_mut(index))
                                {
                                    t.proposal = None;
                                }
                                cx.notify();
                            })),
                    ),
            );
        }
        if let Some(feedback) = &t.feedback {
            card = card.child(
                div()
                    .text_xs()
                    .text_color(ui.text_muted)
                    .child(feedback.clone()),
            );
        }
        card.into_any_element()
    }
    pub(super) fn editor_panel(&self, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let e = self.editor.as_ref().unwrap();
        let routing = self.routing_enabled(cx);
        let mut panel = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_size(px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if e.revision.is_some() {
                        "Edit feature"
                    } else {
                        "New feature"
                    }),
            )
            .child(Input::new(&e.title).disabled(self.busy))
            .child(Textarea::new(&e.brief).disabled(self.busy))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("feature-enhance")
                            .ghost()
                            .small()
                            .icon(Lucide::Sparkles)
                            .label("Enhance brief")
                            .disabled(self.busy || !self.model.read(cx).model_ready())
                            .on_click(cx.listener(|v, _, _, cx| v.enhance(cx))),
                    )
                    .when(e.undo.is_some(), |el| {
                        el.child(
                            Button::new("feature-undo")
                                .ghost()
                                .small()
                                .label("Undo enhancement")
                                .disabled(self.busy)
                                .on_click(cx.listener(|v, _, window, cx| {
                                    if let Some(e) = &mut v.editor {
                                        if let Some(text) = e.undo.take() {
                                            e.brief
                                                .update(cx, |s, cx| s.set_value(text, window, cx));
                                        }
                                    }
                                    cx.notify();
                                })),
                        )
                    }),
            );
        if let Some(enhanced) = &self.enhanced {
            panel = panel.child(
                crate::views::brand::handoff_card(ui)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Proposed brief"),
                    )
                    .child(
                        div()
                            .id("enhanced-feature-preview")
                            .max_h(px(180.))
                            .overflow_y_scroll()
                            .text_sm()
                            .child(enhanced.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("apply-feature-enhance")
                                    .outline()
                                    .small()
                                    .label("Use this brief")
                                    .on_click(cx.listener(|v, _, window, cx| {
                                        if let (Some(text), Some(e)) =
                                            (v.enhanced.take(), v.editor.as_mut())
                                        {
                                            e.undo = Some(e.brief.read(cx).value().to_string());
                                            e.brief
                                                .update(cx, |s, cx| s.set_value(text, window, cx));
                                        }
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("dismiss-feature-enhance")
                                    .ghost()
                                    .small()
                                    .label("Keep draft")
                                    .on_click(cx.listener(|v, _, _, cx| {
                                        v.enhanced = None;
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        }
        panel = panel
            .child(
                div()
                    .text_xs()
                    .text_color(ui.text_muted)
                    .child("Routing preferences · optional"),
            )
            .child(Textarea::new(&self.guidelines).disabled(self.busy));
        panel=panel.child(div().flex().items_center().justify_between().gap_2().flex_wrap()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Tasks & model assignments"))
            .child(Button::new("feature-suggest").ghost().small().icon(Lucide::GitBranch).label(if self.suggesting{"Suggesting…"}else{"Smart Model Routing"})
                .disabled(self.busy||self.suggesting||!routing||e.tasks.is_empty())
                .tooltip(if routing{"Suggest connected models for these tasks; review each suggestion before accepting"}else{"Enable JEV in Settings and Smart Model Routing in the source thread to use suggestions"})
                .on_click(cx.listener(|v,_,_,cx|v.suggest(cx)))));
        for i in 0..e.tasks.len() {
            panel = panel.child(self.task_card(i, ui, cx));
        }
        let mut add = div().flex().gap_2().flex_wrap();
        for role in ["Build", "Frontend", "Backend", "Review", "Other"] {
            add = add.child(
                Button::new(SharedString::from(format!("add-task-{role}")))
                    .ghost()
                    .small()
                    .icon(Lucide::Plus)
                    .label(role)
                    .disabled(self.busy || e.tasks.len() >= 24)
                    .on_click(cx.listener(move |v, _, window, cx| v.add_task(role, window, cx))),
            );
        }
        panel.child(add).child(div().text_xs().text_color(ui.text_muted).child("Linked threads keep their model; use their composer to switch models or continue with a revised brief. Independent tasks start in separate worktrees. Dependent tasks combine the selected Ready for review checkpoints in their new worktree; conflicts pause before sending. A Review task examines one selected checkpoint."))
            .child(div().flex().items_center().gap_2().flex_wrap()
                .child(Button::new("save-feature").primary().label(if self.busy{"Working…"}else{"Save feature"}).disabled(self.busy||self.suggesting).on_click(cx.listener(|v,_,_,cx|v.save(cx))))
                .child(Button::new("save-role-defaults").ghost().small().label("Use assignments as project defaults").disabled(self.busy).on_click(cx.listener(|v,_,_,cx|v.save_preferences(true,true,cx))))
                .child(Button::new("cancel-feature").ghost().small().label("Discard draft").disabled(self.busy).on_click(cx.listener(|v,_,_,cx|{v.editor=None;v.enhanced=None;v.generation+=1;v.suggesting=false;v.reload(cx);cx.notify();}))))
            .child(div().text_xs().text_color(ui.text_faint).child(format!("Save writes plan/features/{}.md in this project. It does not start a model or commit code.",e.feature.id)))
            .into_any_element()
    }
}
