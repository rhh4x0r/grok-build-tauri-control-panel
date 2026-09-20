use super::*;
use gpui_kit::component::{text::TextView, Selectable};

impl ProjectWorkView {
    fn plan_card(&self, p: &Proposal, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let project = self.snapshot(cx);
        let selected = p
            .features
            .iter()
            .filter(|f| !self.excluded.contains(&f.id))
            .count();
        let title_for = |id: &String| {
            p.features
                .iter()
                .find(|f| &f.id == id)
                .map(|f| f.title.clone())
                .or_else(|| {
                    project
                        .features
                        .iter()
                        .find(|f| &f.id == id)
                        .map(|f| f.title())
                })
                .unwrap_or_else(|| id.clone())
        };
        let mut card=super::super::brand::handoff_card(ui).min_w_0().overflow_hidden().gap_3()
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(format!("Proposed work · {selected}/{} selected",p.features.len())))
            .child(div().text_sm().child(format!("{} parallel jobs · local target {} · {} · up to {} code repair(s)",project.policy.concurrency,project.policy.target,if project.policy.auto_merge {"automatic merge after checks + AI review"}else{"you test and approve before merging"},project.policy.max_repairs)))
            .child(div().text_xs().text_color(ui.text_muted).child("Unselected features stay in Backlog. Tool permissions still apply. Nothing is pushed or deployed."))
            .child(Button::new("plan-details").ghost().small().selected(self.expanded_plan).label(if self.expanded_plan {"Hide acceptance details"}else{"Review acceptance criteria, test steps & commands"}).on_click(cx.listener(|v,_,_,cx|{v.expanded_plan= !v.expanded_plan;cx.notify();})));
        for (fi, f) in p.features.iter().enumerate() {
            let fid = f.id.clone();
            let included = !self.excluded.contains(&fid);
            let mut feature = div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_2()
                .border_t_1()
                .border_color(ui.border)
                .pt_3()
                .child(
                    Button::new(SharedString::from(format!("plan-select-{fid}")))
                        .ghost()
                        .small()
                        .selected(included)
                        .label(format!("{} {}", if included { "✓" } else { "○" }, f.title))
                        .on_click(cx.listener(move |v, _, _, cx| {
                            if !v.excluded.remove(&fid) {
                                v.excluded.insert(fid.clone());
                            }
                            cx.notify();
                        })),
                )
                .child(div().text_sm().child(if self.expanded_plan {
                    f.brief.clone()
                } else {
                    f.brief.chars().take(200).collect::<String>()
                }))
                .child(div().text_xs().text_color(ui.text_muted).child(
                    if f.depends_on.is_empty() {
                        "Can start independently".into()
                    } else {
                        format!(
                            "After {} merges",
                            f.depends_on
                                .iter()
                                .map(&title_for)
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    },
                ));
            for (ti, t) in f.tasks.iter().enumerate() {
                feature = feature.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(div().text_sm().child(format!("{} · {}", t.role, t.title)))
                        .child(self.picker(
                            format!("plan-{fi}-{ti}"),
                            Choice::Task(fi, ti),
                            t.assignment.clone(),
                            cx,
                        )),
                );
                if !t.waits_for.is_empty() {
                    feature = feature.child(
                        div().text_xs().text_color(ui.text_muted).child(format!(
                            "Waits for {}",
                            t.waits_for
                                .iter()
                                .map(|id| f
                                    .tasks
                                    .iter()
                                    .find(|t| &t.id == id)
                                    .map(|t| t.title.clone())
                                    .unwrap_or_else(|| id.clone()))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )),
                    );
                }
                if let Some(suggestion) = project
                    .routing_suggestions
                    .get(&format!("{}/{}", f.id, t.id))
                {
                    let a = suggestion.assignment.clone();
                    feature = feature
                        .child(div().text_xs().child(suggestion.reason.clone()))
                        .child(
                            Button::new(SharedString::from(format!("route-{fi}-{ti}")))
                                .outline()
                                .small()
                                .label(format!(
                                    "Use suggestion: {} · {}",
                                    self.model.read(cx).model_name(&a.backend, &a.model),
                                    super::super::brand::effort_label(&a.effort)
                                ))
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    v.assignment(Choice::Task(fi, ti), a.clone(), cx)
                                })),
                        );
                }
                if self.expanded_plan {
                    feature = feature.child(div().text_sm().child(t.brief.clone()));
                }
            }
            feature = feature
                .child(div().text_xs().child("Independent reviewer"))
                .child(self.picker(
                    format!("reviewer-{fi}"),
                    Choice::Reviewer(fi),
                    f.verification.reviewer.clone(),
                    cx,
                ));
            if self.expanded_plan {
                feature = feature.child(
                    div().text_sm().child(format!(
                        "Done when:\n{}\nHow to test: {}",
                        f.verification
                            .criteria
                            .iter()
                            .map(|c| format!("• {c}"))
                            .collect::<Vec<_>>()
                            .join("\n"),
                        f.verification.test_steps
                    )),
                );
                for command in &f.verification.checks {
                    feature = feature.child(
                        div()
                            .text_xs()
                            .font_family(ui.mono.clone())
                            .child(command_label(command)),
                    );
                }
            }
            card = card.child(feature);
        }
        for update in &p.updates {
            card = card.child(div().text_sm().child(format!(
                "Update {}: {}",
                title_for(&update.feature_id),
                update.feedback
            )));
        }
        card.child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("plan-go")
                        .primary()
                        .small()
                        .label("Start selected")
                        .disabled(
                            self.busy
                                || !project.questions.is_empty()
                                || !p.questions.is_empty()
                                || (selected == 0 && p.updates.is_empty()),
                        )
                        .on_click(cx.listener(|v, _, _, cx| v.accept(true, cx))),
                )
                .child(
                    Button::new("plan-save")
                        .outline()
                        .small()
                        .label("Save to Backlog")
                        .disabled(
                            self.busy || !project.questions.is_empty() || !p.questions.is_empty(),
                        )
                        .on_click(cx.listener(|v, _, _, cx| v.accept(false, cx))),
                )
                .child(
                    Button::new("plan-revise")
                        .ghost()
                        .small()
                        .label("Revise")
                        .on_click(
                            cx.listener(|v, _, w, cx| v.input.update(cx, |s, cx| s.focus(w, cx))),
                        ),
                )
                .child(
                    Button::new("plan-discard")
                        .ghost()
                        .small()
                        .label("Discard proposal")
                        .disabled(self.busy)
                        .on_click(cx.listener(|v, _, _, cx| v.discard(cx))),
                ),
        )
        .into_any_element()
    }
    fn feature_card(&self, f: &FeatureWork, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let id = f.id.clone();
        let mut card = super::super::brand::handoff_card(ui)
            .gap_2()
            .min_w_0()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(f.title()))
            .child(
                div()
                    .text_xs()
                    .text_color(ui.text_muted)
                    .child(f.status_label().to_string()),
            );
        if let Ok(spec) = f.feature() {
            for t in &spec.tasks {
                let actual = f.tasks.get(&t.id);
                let assignment = actual
                    .and_then(|t| t.applied.as_ref())
                    .or(f.assignments.get(&t.id))
                    .or(t.assignment.as_ref());
                let label = assignment
                    .map(|a| {
                        format!(
                            "{} · {}",
                            self.model.read(cx).model_name(&a.backend, &a.model),
                            super::super::brand::effort_label(&a.effort)
                        )
                    })
                    .unwrap_or_default();
                card = card.child(div().text_xs().child(format!(
                    "{} · {}\n{}",
                    t.role,
                    actual.map(|t| t.state.label()).unwrap_or_default(),
                    label
                )));
            }
        }
        let waiting = self.snapshot(cx).waiting_reason(f);
        let note = waiting
            .lines()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(180)
            .collect::<String>();
        card.child(div().text_xs().text_color(ui.text_muted).child(note))
            .child(div().text_xs().child(format!(
                "Unblocks {} feature(s)",
                self.snapshot(cx).unblocks(&f.id)
            )))
            .child(
                Button::new(SharedString::from(format!("inspect-{}", f.id)))
                    .outline()
                    .small()
                    .label(if f.state == FeatureState::Review {
                        "Test & review"
                    } else {
                        "Inspect"
                    })
                    .on_click(cx.listener(move |v, _, _, cx| v.inspect(id.clone(), cx))),
            )
            .into_any_element()
    }
    fn board(&self, p: &ProjectWork, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        if self.list_board || self.needs_only {
            let mut list = div().flex().flex_col().gap_3();
            for f in p.features.iter().filter(|f| {
                !self.needs_only
                    || f.state == FeatureState::NeedsInput
                    || (f.state == FeatureState::Review && f.approved_head.is_none())
            }) {
                list = list.child(self.feature_card(f, ui, cx));
            }
            return list.into_any_element();
        }
        let mut columns = div()
            .id("project-kanban")
            .flex()
            .gap_3()
            .w_full()
            .overflow_x_scroll()
            .min_h_0();
        for name in ["Backlog", "Queued", "In progress", "Needs you", "Done"] {
            let items = p
                .features
                .iter()
                .filter(|f| {
                    f.state.column() == name
                        && (!self.needs_only
                            || matches!(f.state, FeatureState::NeedsInput | FeatureState::Review))
                })
                .collect::<Vec<_>>();
            let count = items.len() + usize::from(name == "Backlog" && p.draft.is_some());
            let mut column = div()
                .flex()
                .flex_col()
                .gap_3()
                .w(px(250.))
                .flex_shrink_0()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("{name} · {count}")),
                );
            if name == "Backlog" && p.draft.is_some() {
                column = column.child(
                    super::super::brand::handoff_card(ui)
                        .gap_2()
                        .child("Your proposed plan is ready.")
                        .child(
                            Button::new("board-draft")
                                .outline()
                                .small()
                                .label("Review plan")
                                .on_click(cx.listener(|v, _, _, cx| {
                                    v.tab = "Conversation";
                                    cx.notify();
                                })),
                        ),
                );
            }
            for f in items {
                column = column.child(self.feature_card(f, ui, cx));
            }
            if count == 0 {
                column = column.child(
                    div()
                        .p_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(ui.border)
                        .text_xs()
                        .text_color(ui.text_faint)
                        .child("No features"),
                );
            }
            columns = columns.child(column);
        }
        columns.into_any_element()
    }
    fn approval_cards(&self, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let cards = self
            .model
            .read(cx)
            .threads
            .iter()
            .flat_map(|(sid, t)| {
                let t = t.read(cx);
                if t.meta.project_root.as_deref() != Some(&self.project) || !t.meta.live {
                    return vec![];
                }
                t.thread
                    .open_approvals()
                    .map(|(_, a)| (*sid, t.title(), a.clone()))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let mut area = div().flex().flex_col().gap_3();
        for (sid, title, a) in cards {
            let mut buttons = div().flex().flex_wrap().gap_2();
            for option in &a.options {
                let state = services(cx);
                let rid = a.request_id.clone();
                let opt = option.id.clone();
                let weak = cx.entity().downgrade();
                buttons = buttons.child(
                    Button::new(SharedString::from(format!(
                        "project-approval-{sid}-{rid}-{opt}"
                    )))
                    .outline()
                    .small()
                    .label(option.label.clone())
                    .on_click(move |_, _, cx| {
                        let state = state.clone();
                        let rid = rid.clone();
                        let opt = opt.clone();
                        let weak = weak.clone();
                        spawn_service(
                            cx,
                            async move {
                                svc::respond_approval(&state, sid.to_string(), rid, Some(opt)).await
                            },
                            move |result, cx| {
                                let _ = weak.update(cx, |v, cx| {
                                    if let Err(e) = result {
                                        v.message = Some(e);
                                    }
                                    cx.notify();
                                });
                            },
                        );
                    }),
                );
            }
            let state = services(cx);
            let rid = a.request_id.clone();
            let weak = cx.entity().downgrade();
            buttons = buttons.child(
                Button::new(SharedString::from(format!("project-deny-{sid}-{rid}")))
                    .ghost()
                    .small()
                    .label("Cancel request")
                    .on_click(move |_, _, cx| {
                        let state = state.clone();
                        let rid = rid.clone();
                        let weak = weak.clone();
                        spawn_service(
                            cx,
                            async move {
                                svc::respond_approval(&state, sid.to_string(), rid, None).await
                            },
                            move |result, cx| {
                                let _ = weak.update(cx, |v, cx| {
                                    if let Err(e) = result {
                                        v.message = Some(e);
                                    }
                                    cx.notify();
                                });
                            },
                        );
                    }),
            );
            area = area.child(
                super::super::brand::handoff_card(ui)
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("Permission needed · {title}")),
                    )
                    .min_w_0()
                    .overflow_hidden()
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "approval-detail-{sid}-{}",
                                a.request_id
                            )))
                            .max_h(px(160.))
                            .overflow_y_scroll()
                            .overflow_x_hidden()
                            .child(
                                TextView::markdown(
                                    SharedString::from(format!(
                                        "approval-text-{sid}-{}",
                                        a.request_id
                                    )),
                                    a.explanation.unwrap_or(a.summary),
                                )
                                .selectable(true),
                            ),
                    )
                    .child(buttons)
                    .child(
                        Button::new(SharedString::from(format!(
                            "approval-thread-{sid}-{}",
                            a.request_id
                        )))
                        .ghost()
                        .small()
                        .label("Inspect thread")
                        .on_click(cx.listener(move |v, _, _, cx| v.open_thread(sid, cx))),
                    ),
            );
        }
        area.into_any_element()
    }
    fn inspection(&self, f: &FeatureWork, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let mut card = super::super::brand::handoff_card(ui)
            .gap_3()
            .min_w_0()
            .overflow_hidden();
        let mut tabs = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(f.title()),
            )
            .child(
                Button::new("inspect-close")
                    .ghost()
                    .small()
                    .label("Close")
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.selected = None;
                        v.scroll.set_offset(v.board_offset);
                        cx.notify();
                    })),
            );
        for tab in [
            "Result", "Changes", "Thread", "Preview", "Records", "Models",
        ] {
            tabs = tabs.child(
                Button::new(SharedString::from(format!("result-{tab}")))
                    .ghost()
                    .small()
                    .label(tab)
                    .selected(self.detail == tab)
                    .on_click(cx.listener(move |v, _, _, cx| match tab {
                        "Changes" => v.changes(cx),
                        "Preview" => v.preview(cx),
                        _ => {
                            v.detail = tab;
                            cx.notify();
                        }
                    })),
            );
        }
        card = card.child(tabs).child(self.decision.clone()).child(
            div()
                .text_sm()
                .text_color(ui.text_muted)
                .child(f.note.clone()),
        );
        let spec = f.feature().ok();
        match self.detail {
            "Records" => {
                card=card.child(div().text_sm().child("Approved briefs and checkpoints are portable files. Live status, actual checks and approvals are stored locally in Bomb Code. Records enter the project when the feature merges."))
                    .child(div().text_xs().child(format!("plan/features/{}.md · plan/tasks/{}-*.md · plan/results/{}.md",f.id,f.id,f.id)))
                    .child(TextView::markdown("approved-feature-record",f.document.clone()).selectable(true));
            }
            "Models" => {
                if let Some(fi) = self.snapshot(cx).features.iter().position(|v| v.id == f.id) {
                    if let Some(spec) = &spec {
                        for (ti, t) in spec.tasks.iter().enumerate() {
                            if let Some(a) =
                                f.assignments.get(&t.id).cloned().or(t.assignment.clone())
                            {
                                card = card
                                    .child(
                                        div().text_sm().child(format!(
                                            "{} · {}",
                                            t.title,
                                            f.tasks
                                                .get(&t.id)
                                                .map(|t| t.state.label())
                                                .unwrap_or("Waiting")
                                        )),
                                    )
                                    .child(self.picker(
                                        format!("existing-{fi}-{ti}"),
                                        Choice::ExistingTask(fi, ti),
                                        a,
                                        cx,
                                    ));
                            }
                        }
                        if let Some(v) = &spec.verification {
                            card = card
                                .child(div().text_sm().child("Independent reviewer"))
                                .child(
                                    self.picker(
                                        format!("existing-reviewer-{fi}"),
                                        Choice::ExistingReviewer(fi),
                                        f.assignments
                                            .get("reviewer")
                                            .cloned()
                                            .unwrap_or(v.reviewer.clone()),
                                        cx,
                                    ),
                                );
                        }
                    }
                }
                card=card.child(div().text_xs().child("Changes apply only to unfinished work. Then use the stage retry action below; completed task outputs remain unchanged."));
            }
            "Changes" => {
                let parts = diff_files(self.diff.as_deref().unwrap_or("Loading changes…"));
                let selected = self.selected_file.as_deref().unwrap_or_else(|| {
                    parts
                        .first()
                        .map(|(name, _)| name.as_str())
                        .unwrap_or("Changes")
                });
                let mut files = div().flex().flex_wrap().gap_1();
                for (name, _) in &parts {
                    let name = name.clone();
                    files = files.child(
                        Button::new(SharedString::from(format!("diff-file-{name}")))
                            .ghost()
                            .small()
                            .selected(name == selected)
                            .label(name.clone())
                            .on_click(cx.listener(move |v, _, _, cx| {
                                v.selected_file = Some(name.clone());
                                cx.notify();
                            })),
                    );
                }
                let text = parts
                    .iter()
                    .find(|(name, _)| name == selected)
                    .or(parts.first())
                    .map(|(_, text)| text.clone())
                    .unwrap_or_default();
                card = card.child(files).child(
                    div()
                        .id("candidate-diff")
                        .max_h(px(460.))
                        .min_w_0()
                        .overflow_y_scroll()
                        .overflow_x_hidden()
                        .child(
                            TextView::markdown(
                                "project-diff-text",
                                format!("```diff\n{text}\n```"),
                            )
                            .selectable(true),
                        ),
                );
            }
            "Preview" => {
                card = card
                    .when_some(self.preview_message.clone(), |d, message| {
                        d.child(div().text_sm().child(message))
                    })
                    .child(
                        Button::new("preview-retry")
                            .outline()
                            .small()
                            .label(if self.preview_switch {
                                "Switch preview"
                            } else {
                                "Open / retry preview"
                            })
                            .disabled(self.preview_busy || f.candidate.is_none())
                            .on_click(
                                cx.listener(|v, _, _, cx| v.start_preview(v.preview_switch, cx)),
                            ),
                    )
                    .child(
                        div().text_xs().child(
                            f.candidate
                                .as_ref()
                                .map(|w| format!("Candidate: {}", w.path))
                                .unwrap_or_else(|| {
                                    "The combined candidate will be available after tasks finish."
                                        .into()
                                }),
                        ),
                    )
                    .when(f.candidate.is_some(), |d| {
                        d.child(div().h(px(460.)).child(self.preview.clone()))
                    });
            }
            "Thread" => {
                for attempt in &f.review_history {
                    let sid = attempt.thread;
                    card = card
                        .child(
                            Button::new(SharedString::from(format!("review-history-{sid}")))
                                .ghost()
                                .small()
                                .label(format!("AI review · {} · {}", attempt.outcome, attempt.at))
                                .on_click(cx.listener(move |v, _, _, cx| v.open_thread(sid, cx))),
                        )
                        .child(div().text_xs().child(attempt.summary.clone()));
                }
                for (id, t) in &f.tasks {
                    if let Some(sid) = t.workspace.as_ref().and_then(|w| w.session) {
                        let title = spec
                            .as_ref()
                            .and_then(|s| s.tasks.iter().find(|t| &t.id == id))
                            .map(|t| t.title.clone())
                            .unwrap_or_else(|| id.clone());
                        card = card.child(
                            Button::new(SharedString::from(format!("result-thread-{sid}")))
                                .outline()
                                .small()
                                .label(title)
                                .on_click(cx.listener(move |v, _, _, cx| v.open_thread(sid, cx))),
                        );
                    }
                }
                for (label, sid) in [
                    (
                        "Combined result / repairs",
                        f.candidate.as_ref().and_then(|w| w.session),
                    ),
                    ("Independent review", f.reviewer_thread),
                ] {
                    if let Some(sid) = sid {
                        card = card.child(
                            Button::new(SharedString::from(format!("result-review-{sid}")))
                                .outline()
                                .small()
                                .label(label)
                                .on_click(cx.listener(move |v, _, _, cx| v.open_thread(sid, cx))),
                        );
                    }
                }
            }
            _ => {
                if let Some(spec) = &spec {
                    card = card.child(div().text_sm().child(format!("Requested: {}", spec.brief)));
                    if let Some(v) = &spec.verification {
                        card = card
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("How to test"),
                            )
                            .child(div().text_sm().child(v.test_steps.clone()));
                    }
                }
                for t in f.tasks.values() {
                    if !t.summary.is_empty() {
                        card = card.child(div().text_sm().child(t.summary.clone()));
                    }
                    if !t.note.is_empty() {
                        card = card.child(div().text_sm().child(t.note.clone()));
                    }
                }
                if let Some(review) = &f.review {
                    card = card
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child("Independent review"),
                        )
                        .child(div().text_sm().child(review.summary.clone()))
                        .child(div().text_xs().text_color(ui.text_muted).child(format!(
                                "{} · {} · checked commit {}",
                                self.model
                                    .read(cx)
                                    .model_name(&review.reviewer.backend, &review.reviewer.model),
                                super::super::brand::effort_label(&review.reviewer.effort),
                                review.candidate
                            )));
                    for criterion in &review.criteria {
                        card = card.child(
                            div().text_sm().child(format!(
                                "{}: {}\n{}",
                                spec.as_ref()
                                    .and_then(|s| s.verification.as_ref())
                                    .and_then(|v| v.criteria.get(criterion.criterion))
                                    .cloned()
                                    .unwrap_or_else(|| format!(
                                        "Criterion {}",
                                        criterion.criterion + 1
                                    )),
                                criterion.status,
                                criterion.evidence.join("\n")
                            )),
                        );
                    }
                }
                card = card.child(
                    Button::new("check-details")
                        .ghost()
                        .small()
                        .selected(self.show_checks)
                        .label(if self.show_checks {
                            "Hide passing checks"
                        } else {
                            "Show all check output"
                        })
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.show_checks = !v.show_checks;
                            cx.notify();
                        })),
                );
                for (i, check) in f
                    .checks
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| self.show_checks || c.exit_code != Some(0))
                {
                    card = card.child(check_card(format!("feature-check-{i}"), check, ui));
                }
            }
        }
        card.into_any_element()
    }
}
fn diff_files(text: &str) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = Vec::new();
    for line in text.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            files.push((
                line.trim()
                    .strip_prefix("diff --git ")
                    .unwrap_or(line)
                    .to_string(),
                String::new(),
            ));
        }
        if files.is_empty() {
            files.push(("Changes".into(), String::new()));
        }
        if let Some((_, body)) = files.last_mut() {
            body.push_str(line);
        }
    }
    files
}
fn command_label(c: &svc::features::CheckCommand) -> String {
    format!(
        "{} {}",
        c.program,
        c.args
            .iter()
            .map(|a| if a.contains(char::is_whitespace) {
                format!("{a:?}")
            } else {
                a.clone()
            })
            .collect::<Vec<_>>()
            .join(" ")
    )
}
fn check_card(id: String, c: &CheckEvidence, ui: &Ui) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .overflow_hidden()
        .gap_1()
        .border_t_1()
        .border_color(ui.border)
        .pt_2()
        .child(div().text_xs().font_family(ui.mono.clone()).child(format!(
                "{} · exit {}",
                command_label(&c.command),
                c.exit_code
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "not reported".into())
            )))
        .child(
            div()
                .id(SharedString::from(id.clone()))
                .max_h(px(130.))
                .overflow_y_scroll()
                .overflow_x_hidden()
                .child(
                    TextView::markdown(
                        SharedString::from(format!("{id}-out")),
                        format!("```text\n{}\n```", c.output),
                    )
                    .selectable(true),
                ),
        )
        .into_any_element()
}
impl Render for ProjectWorkView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(window, cx);
        let narrow = window.viewport_size().width < px(1200.);
        if let Some(text) = self.clear_submission.take() {
            if self.input.read(cx).value().as_ref() == text {
                self.input.update(cx, |s, cx| s.set_value("", window, cx));
            }
        }
        if let Some(id) = self.selected.clone() {
            self.decision.update(cx, |d, cx| {
                d.set_context(self.project.clone(), id, window, cx)
            });
        }
        if let Some(text) = self.sync_guidelines.take() {
            self.guidelines
                .update(cx, |s, cx| s.set_value(text, window, cx));
        }
        let ui = Ui::of(cx);
        let p = self.snapshot(cx);
        let tail = p.messages.last().map(|m| m.id.clone());
        if self.last_message != tail {
            self.last_message = tail;
            if self.tab == "Conversation"
                && self.selected.is_none()
                && (self.scroll.max_offset().y + self.scroll.offset().y < px(80.))
            {
                self.pin_frames = 2;
            } else {self.new_activity=true;}
        }
        if self.pin_frames > 0 {
            self.scroll
                .set_offset(point(px(0.), -self.scroll.max_offset().y));
            self.pin_frames -= 1;
            cx.notify();
        }
        let needs = p.needs_attention()
            + self
                .model
                .read(cx)
                .threads
                .values()
                .filter(|t| {
                    let t = t.read(cx);
                    t.meta.live && t.meta.project_root.as_deref() == Some(&self.project)
                })
                .map(|t| t.read(cx).thread.open_approvals().count())
                .sum::<usize>();
        let mut top = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .border_b_1()
            .border_color(ui.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        div()
                            .text_size(px(22.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(project_name(&self.project)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(ui.text_muted)
                            .child(p.activity_label()),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("project-ordinary-thread")
                            .ghost()
                            .small()
                            .label("New thread")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(crate::actions::NewThread), cx)
                            }),
                    )
                    .child(
                        Button::new("project-work-settings")
                            .ghost()
                            .small()
                            .label("Workflow settings")
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.settings = !v.settings;
                                cx.notify();
                            })),
                    ),
            );
        let mut tabs = div().flex().flex_wrap().gap_2();
        for tab in ["Conversation", "Board", "Git"] {
            tabs = tabs.child(
                Button::new(SharedString::from(format!("project-tab-{tab}")))
                    .ghost()
                    .small()
                    .label(tab)
                    .selected(self.tab == tab && self.selected.is_none())
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.tab = tab;
                        v.selected = None;
                        v.scroll.set_offset(point(px(0.), px(0.)));
                        if tab == "Git" {
                            v.model.update(cx, |m, cx| m.refresh_project_overview(cx));
                        }
                        cx.notify();
                    })),
            );
        }
        tabs = tabs
            .child(
                Button::new("needs-you-filter")
                    .outline()
                    .small()
                    .label(format!(
                        "Needs you · {needs}{}",
                        if self.needs_only { " · showing" } else { "" }
                    ))
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.selected = None;
                        v.needs_only = !v.needs_only;
                        v.tab = "Board";
                        cx.notify();
                    })),
            )
            .child(
                Button::new("next-project-decision")
                    .ghost()
                    .small()
                    .label("Next decision")
                    .disabled(needs == 0)
                    .on_click(cx.listener(|v, _, w, cx| v.next_decision(w, cx))),
            )
            .child(div().flex_1());
        if p.enabled {
            if p.state == RunState::Running {
                tabs = tabs.child(
                    Button::new("pause-project")
                        .outline()
                        .small()
                        .label("Pause")
                        .on_click(cx.listener(|v, _, _, cx| v.command("pause", cx))),
                );
            } else {
                tabs = tabs.child(
                    Button::new("go-project")
                        .primary()
                        .small()
                        .label("Resume ready work")
                        .disabled(p.features.is_empty() || self.busy || p.planning)
                        .on_click(cx.listener(|v, _, _, cx| v.command("go", cx))),
                );
            }
            tabs = tabs.child(
                Button::new("stop-project")
                    .ghost()
                    .small()
                    .label("Stop")
                    .disabled(!p.has_activity() && p.state != RunState::Running)
                    .on_click(cx.listener(|v, _, _, cx| v.command("stop", cx))),
            );
        }
        top = top.child(tabs);
        if self.new_activity {top=top.child(Button::new("project-new-activity").ghost().small().label("New activity ↓").on_click(cx.listener(|v,_,_,cx|{v.new_activity=false;v.selected=None;v.tab="Conversation";v.pin_frames=2;cx.notify();})));}
        let mut body = div()
            .id("project-work-content")
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_4()
            .flex()
            .flex_col()
            .gap_4();
        if self.loading {
            body = body.child("Loading project work…");
        }
        if let Some(message) = &self.message {
            body = body.child(div().text_sm().child(message.clone()));
        }
        if let Some(sid) = p.planner_thread {
            body = body.child(
                Button::new("project-planning-thread")
                    .ghost()
                    .small()
                    .label("Planning thread")
                    .on_click(cx.listener(move |v, _, _, cx| v.open_thread(sid, cx))),
            );
        }
        if !p.note.is_empty() {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(ui.text_muted)
                    .child(p.note.clone()),
            );
        }
        if !p.enabled {
            body = body.child(super::super::workspaces::project_page(
                self.model.clone(),
                &ui,
                cx,
            ));
            body=body.child(super::super::brand::handoff_card(&ui).gap_3()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Optional: plan and track parallel features"))
            .child(div().text_sm().child("Describe what you want. Review a plan, run tasks in parallel, then test and approve each result. This workflow is optional for every project."))
            .child(Button::new("enable-project-work").primary().small().label("Enable project workflow").disabled(self.busy).on_click(cx.listener(|v,_,_,cx|v.enable(true,cx)))));
        }
        if self.settings {
            let auto = p.policy.auto_merge;
            let mut settings=super::super::brand::handoff_card(&ui).gap_2()
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Run settings"))
                .child(div().text_sm().child(format!("Local target: {} · {} concurrent tasks · {} automatic repair attempt(s)",p.policy.target,p.policy.concurrency,p.policy.max_repairs)))
                .child(Button::new("toggle-auto-merge").outline().small().disabled(p.state==RunState::Running).label(if auto{"Automatic local merging: on"}else{"Review before merging: on"}).tooltip("Pause to change. Auto merge requires passing checks and independent review; nothing is pushed or deployed.").on_click(cx.listener(|v,_,_,cx|{let mut policy=v.snapshot(cx).policy;policy.auto_merge = !policy.auto_merge;v.policy(policy,cx);})))
                .child(div().text_xs().child("Planning and review use read-only agents. Builders keep Ask permissions. Smart Model Routing uses JEV when enabled in Settings."));
            let mut parallel = div().flex().gap_2();
            for count in 1..=4 {
                parallel = parallel.child(
                    Button::new(SharedString::from(format!("parallel-{count}")))
                        .ghost()
                        .small()
                        .label(format!("{count} parallel"))
                        .selected(p.policy.concurrency == count)
                        .disabled(p.state == RunState::Running)
                        .on_click(cx.listener(move |v, _, _, cx| {
                            let mut policy = v.snapshot(cx).policy;
                            policy.concurrency = count;
                            v.policy(policy, cx);
                        })),
                )
            }
            let routing_ready = services(cx)
                .config
                .try_read()
                .ok()
                .is_some_and(|c| c.model_suggestions.enabled);
            settings=settings.child(Button::new("project-routing").outline().small().label(format!("Smart Model Routing · {}",match p.routing {Some(true)=>"on",Some(false)=>"off",None=>"inherit settings"})).disabled(!routing_ready).tooltip(if routing_ready {"Suggestions are shown for your selection; explicit assignments remain unchanged."}else{"Enable JEV in Settings to use Smart Model Routing."}).on_click(cx.listener(|v,_,_,cx|{let next=match v.snapshot(cx).routing {None=>Some(true),Some(true)=>Some(false),Some(false)=>None};if let Err(e)=work::set_routing(&services(cx),&v.project,next) {v.message=Some(e);}cx.notify();})))
                .child(div().text_sm().child("Project model defaults · used for future plans"));
            for (i, role) in ["Frontend", "Backend", "Build", "Review"]
                .iter()
                .enumerate()
            {
                if let Some(a) = self
                    .defaults
                    .roles
                    .get(*role)
                    .cloned()
                    .or_else(|| self.planner(cx))
                {
                    settings = settings
                        .child(div().text_xs().child(*role))
                        .child(self.picker(format!("default-{role}"), Choice::Role(i), a, cx));
                }
            }
            settings = settings.child(Textarea::new(&self.guidelines)).child(
                Button::new("save-role-defaults")
                    .outline()
                    .small()
                    .label("Save model defaults & guidelines")
                    .disabled(self.busy)
                    .on_click(cx.listener(|v, _, _, cx| v.save_defaults(cx))),
            );
            settings = settings
                .child(parallel)
                .child(
                    Button::new("manual-project-features")
                        .ghost()
                        .small()
                        .label("Manual feature authoring…")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(crate::actions::ManualFeature), cx)
                        }),
                )
                .child(
                    Button::new("disable-project-work")
                        .ghost()
                        .small()
                        .label("Turn workflow off")
                        .disabled(p.state == RunState::Running || p.has_activity() || self.busy)
                        .on_click(cx.listener(|v, _, _, cx| v.enable(false, cx))),
                );
            body = body.child(settings);
        }
        if !p.questions.is_empty() {
            let mut questions = super::super::brand::handoff_card(&ui).gap_2().child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Plan needs your answer"),
            );
            for q in &p.questions {
                questions = questions.child(div().text_sm().child(q.clone()));
            }
            body = body.child(
                questions.child(
                    Button::new("answer-plan")
                        .outline()
                        .small()
                        .label("Answer in project conversation")
                        .on_click(cx.listener(|v, _, w, cx| {
                            v.selected = None;
                            v.tab = "Conversation";
                            v.input.update(cx, |s, cx| s.focus(w, cx));
                            cx.notify();
                        })),
                ),
            );
        }
        if let Some(blocker) = &p.final_blocker {
            let prompt=format!("Plan a bounded integration repair for these final project check failures. Preserve the already merged features. Failure: {}",blocker.message);
            body = body.child(
                super::super::brand::handoff_card(&ui)
                    .gap_2()
                    .min_w_0()
                    .overflow_hidden()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Final project verification needs you"),
                    )
                    .child(div().text_sm().child(blocker.message.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(
                                Button::new("retry-final")
                                    .primary()
                                    .small()
                                    .label("Retry final checks")
                                    .disabled(p.has_activity() || self.busy)
                                    .on_click(cx.listener(|v, _, _, cx| v.retry_final(cx))),
                            )
                            .child(
                                Button::new("plan-final-repair")
                                    .outline()
                                    .small()
                                    .label("Plan integration repair")
                                    .on_click(cx.listener(move |v, _, w, cx| {
                                        v.selected = None;
                                        v.tab = "Conversation";
                                        v.input.update(cx, |s, cx| {
                                            s.set_value(prompt.clone(), w, cx);
                                            s.focus(w, cx);
                                        });
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        }
        body = body.child(self.approval_cards(&ui, cx));
        if let Some(f) = self
            .selected
            .as_ref()
            .and_then(|id| p.features.iter().find(|f| &f.id == id))
        {
            if self.tab == "Board" && !narrow {
                let mut adjacent = div().w(px(260.)).flex_shrink_0().flex().flex_col().gap_2();
                for item in p.features.iter().filter(|f| {
                    !self.needs_only
                        || f.state == FeatureState::NeedsInput
                        || (f.state == FeatureState::Review && f.approved_head.is_none())
                }) {
                    adjacent = adjacent.child(self.feature_card(item, &ui, cx));
                }
                body = body.child(
                    div()
                        .flex()
                        .gap_3()
                        .min_w_0()
                        .child(adjacent)
                        .child(div().flex_1().min_w_0().child(self.inspection(f, &ui, cx))),
                );
            } else {
                body = body.child(self.inspection(f, &ui, cx));
            }
        } else if self.tab == "Git" {
            body = body.child(div().h(px(600.)).child(super::super::project::project_page(
                self.model.clone(),
                &ui,
                cx,
            )));
        } else if self.tab == "Board" {
            body = body.child(
                Button::new("board-layout")
                    .ghost()
                    .small()
                    .label(if self.list_board {
                        "Show columns"
                    } else {
                        "Show compact list"
                    })
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.list_board = !v.list_board;
                        cx.notify();
                    })),
            );
            if narrow && !self.list_board && !self.needs_only {
                let mut list = div().flex().flex_col().gap_3();
                for f in &p.features {
                    list = list.child(self.feature_card(f, &ui, cx));
                }
                body = body
                    .child(
                        div()
                            .text_xs()
                            .text_color(ui.text_muted)
                            .child("Compact list for this window width"),
                    )
                    .child(list);
            } else {
                body = body.child(self.board(&p, &ui, cx));
            }
        } else {
            if p.messages.is_empty() && p.enabled {
                body=body.child(div().text_sm().text_color(ui.text_muted).child("What do you want to build? You can describe several ideas at once and say which models you prefer for each part."));
            }
            if p.messages.len() > self.history_limit {
                body = body.child(
                    Button::new("older-project-history")
                        .ghost()
                        .small()
                        .label("Load earlier activity")
                        .on_click(cx.listener(|v, _, _, cx| {
                            v.history_limit += 60;
                            cx.notify();
                        })),
                );
            }
            for m in p.messages.iter().rev().take(self.history_limit).rev() {
                let mut message = div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .when(m.role == "user", |d| {
                        d.ml_8().p_3().rounded_lg().bg(ui.hover)
                    })
                    .child(
                        TextView::markdown(SharedString::from(m.id.clone()), m.text.clone())
                            .selectable(true),
                    );
                for fid in &m.features {
                    if let Some(f) = p.features.iter().find(|f| &f.id == fid) {
                        let id = f.id.clone();
                        message = message.child(
                            Button::new(SharedString::from(format!("message-{}-{}", m.id, fid)))
                                .outline()
                                .small()
                                .label(format!("{} · {}", f.title(), f.state.label()))
                                .on_click(
                                    cx.listener(move |v, _, _, cx| v.inspect(id.clone(), cx)),
                                ),
                        );
                    }
                }
                body = body.child(message);
            }
            if let Some(draft) = &p.draft {
                body = body.child(self.plan_card(draft, &ui, cx));
            }
            if p.planning {
                body = body.child("Planning from your repository and connected models…");
            }
        }
        if !p.final_checks.is_empty() {
            body = body.child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Assembled project checks"),
            );
            body = body.child(
                Button::new("final-output-toggle")
                    .ghost()
                    .small()
                    .label(if self.show_checks {
                        "Hide passing check output"
                    } else {
                        "Show all final check output"
                    })
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.show_checks = !v.show_checks;
                        cx.notify();
                    })),
            );
            for (i, check) in p
                .final_checks
                .iter()
                .enumerate()
                .filter(|(_, c)| self.show_checks || c.exit_code != Some(0))
            {
                body = body.child(check_card(format!("final-check-{i}"), check, &ui));
            }
        }
        let mut composer = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_4()
            .border_t_1()
            .border_color(ui.border)
            .flex_shrink_0()
            .child(div().text_xs().text_color(ui.text_muted).child("New project work or a project question · Enter to send · Shift+Enter for a new line"))
            .child(Textarea::new(&self.input));
        let mut controls = div().flex().flex_wrap().items_center().gap_2();
        if let Some(a) = self.planner(cx) {
            controls = controls.child(self.picker("planner".into(), Choice::Planner, a, cx));
        }
        let routing_ready=services(cx).config.try_read().ok().is_some_and(|c|c.model_suggestions.enabled);
        controls=controls.child(Button::new("composer-smart-routing").ghost().small().label("Smart Model Routing").selected(p.routing.unwrap_or(routing_ready)).disabled(!routing_ready).tooltip(if routing_ready {"Toggle routing suggestions for this project's future plans. Model choices remain yours."} else {"Enable JEV in Settings to use Smart Model Routing."}).on_click(cx.listener(move|v,_,_,cx|{let enabled=v.snapshot(cx).routing.unwrap_or(routing_ready);if let Err(e)=work::set_routing(&services(cx),&v.project,Some(!enabled)){v.message=Some(e);}cx.notify();})));
        controls = controls.child(div().flex_1()).child(
            Button::new("describe-project-work")
                .primary()
                .small()
                .label(if p.planning {
                    "Planning…"
                } else {
                    "Plan this"
                })
                .disabled(
                    !p.enabled
                        || self.busy
                        || p.planning
                        || self.input.read(cx).value().trim().is_empty(),
                )
                .on_click(cx.listener(|v, _, window, cx| v.describe(window, cx))),
        );
        composer = composer.child(controls);
        div()
            .size_full()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .text_color(ui.text)
            .child(top)
            .child(body)
            .when(p.enabled && self.selected.is_none(), |d| d.child(composer))
    }
}
