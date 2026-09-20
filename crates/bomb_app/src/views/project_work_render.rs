use super::*;
use gpui_kit::component::text::TextView;

impl ProjectWorkView {
    fn plan_card(&self, p: &Proposal, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let mut card=super::super::brand::handoff_card(ui).gap_3()
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Proposed work"))
            .child(div().text_sm().text_color(ui.text_muted).child("Review scope, models, reasoning and check commands. Accepting starts only this approved work; tool permissions still apply."));
        for (fi, f) in p.features.iter().enumerate() {
            let mut feature = div()
                .flex()
                .flex_col()
                .gap_2()
                .border_t_1()
                .border_color(ui.border)
                .pt_3()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(f.title.clone()),
                )
                .child(div().text_sm().child(f.brief.clone()))
                .child(div().text_xs().text_color(ui.text_muted).child(
                    if f.depends_on.is_empty() {
                        "Can start independently".into()
                    } else {
                        format!("After: {}", f.depends_on.join(", "))
                    },
                ));
            for (ti, t) in f.tasks.iter().enumerate() {
                feature = feature.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .pl_3()
                        .border_l_1()
                        .border_color(ui.border)
                        .child(div().text_sm().child(format!("{} · {}", t.role, t.title)))
                        .child(
                            div()
                                .text_xs()
                                .text_color(ui.text_muted)
                                .child(t.brief.clone()),
                        )
                        .when(!t.waits_for.is_empty(), |d| {
                            d.child(
                                div()
                                    .text_xs()
                                    .child(format!("Waits for: {}", t.waits_for.join(", "))),
                            )
                        })
                        .child(self.picker(
                            format!("plan-{fi}-{ti}"),
                            Choice::Task(fi, ti),
                            t.assignment.clone(),
                            cx,
                        )),
                );
            }
            feature = feature
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child("Independent review"),
                )
                .child(self.picker(
                    format!("reviewer-{fi}"),
                    Choice::Reviewer(fi),
                    f.verification.reviewer.clone(),
                    cx,
                ))
                .child(div().text_sm().child(format!(
                        "Done when:\n{}",
                        f.verification
                            .criteria
                            .iter()
                            .map(|c| format!("• {c}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    )))
                .child(
                    div()
                        .text_sm()
                        .child(format!("How to test: {}", f.verification.test_steps)),
                );
            for check in &f.verification.checks {
                feature = feature.child(
                    div()
                        .text_xs()
                        .font_family(ui.mono.clone())
                        .child(command_label(check)),
                );
            }
            card = card.child(feature);
        }
        for update in &p.updates {
            card = card.child(
                div()
                    .text_sm()
                    .child(format!("Update {}: {}", update.feature_id, update.feedback)),
            );
        }
        for question in &p.questions {
            card = card.child(div().text_sm().child(question.clone()));
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
                        .label("Approve plan & Go")
                        .disabled(self.busy || !p.questions.is_empty())
                        .on_click(cx.listener(|v, _, _, cx| v.accept(true, cx))),
                )
                .child(
                    Button::new("plan-save")
                        .outline()
                        .small()
                        .label("Save for later")
                        .disabled(self.busy || !p.questions.is_empty())
                        .on_click(cx.listener(|v, _, _, cx| v.accept(false, cx))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(ui.text_muted)
                        .child("To refine it, reply below."),
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
                    .child(f.state.label()),
            );
        if let Ok(spec) = f.feature() {
            for t in &spec.tasks {
                let actual = f.tasks.get(&t.id);
                let assignment = actual
                    .and_then(|t| t.applied.as_ref())
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
                    actual.map(|t| format!("{:?}", t.state)).unwrap_or_default(),
                    label
                )));
            }
        }
        let note = f
            .note
            .lines()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(180)
            .collect::<String>();
        card.child(div().text_xs().text_color(ui.text_muted).child(note))
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
        let mut columns = div()
            .id("project-kanban")
            .flex()
            .gap_3()
            .w_full()
            .overflow_x_scroll()
            .min_h_0();
        for name in ["Ideas", "Queued", "Working", "Review", "Done"] {
            let items = p
                .features
                .iter()
                .filter(|f| {
                    f.state.column() == name
                        && (!self.needs_only
                            || matches!(f.state, FeatureState::NeedsInput | FeatureState::Review))
                })
                .collect::<Vec<_>>();
            let count = items.len() + usize::from(name == "Ideas" && p.draft.is_some());
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
            if name == "Ideas" && p.draft.is_some() {
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
                    .child(div().text_sm().child(a.explanation.unwrap_or(a.summary)))
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
                        cx.notify();
                    })),
            );
        for tab in ["Result", "Changes", "Thread", "Preview"] {
            tabs = tabs.child(
                Button::new(SharedString::from(format!("result-{tab}")))
                    .ghost()
                    .small()
                    .label(tab)
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
        card = card.child(tabs).child(
            div()
                .text_sm()
                .text_color(ui.text_muted)
                .child(f.note.clone()),
        );
        let spec = f.feature().ok();
        match self.detail {
            "Changes" => {
                card = card.child(
                    div()
                        .id("candidate-diff")
                        .max_h(px(460.))
                        .overflow_y_scroll()
                        .child(
                            TextView::markdown(
                                "project-diff-text",
                                format!(
                                    "```diff\n{}\n```",
                                    self.diff.as_deref().unwrap_or("Loading changes…")
                                ),
                            )
                            .selectable(true),
                        ),
                );
            }
            "Preview" => {
                card = card
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
                        card = card.child(div().text_sm().child(format!(
                            "{}: {}\n{}",
                            criterion.criterion + 1,
                            criterion.status,
                            criterion.evidence.join("\n")
                        )));
                    }
                }
                for (i, check) in f.checks.iter().enumerate() {
                    card = card.child(check_card(format!("feature-check-{i}"), check, ui));
                }
            }
        }
        if matches!(f.state, FeatureState::Review | FeatureState::NeedsInput) {
            let reviewed = f.clone();
            card = card
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .when(f.state == FeatureState::Review, |d| {
                            d.child(
                                Button::new("result-approve")
                                    .primary()
                                    .small()
                                    .label(if f.approved_head.is_some() {
                                        "Approved · merge queued"
                                    } else {
                                        "Approve & merge"
                                    })
                                    .disabled(self.busy || f.approved_head.is_some())
                                    .on_click(cx.listener(move |v, _, _, cx| {
                                        v.approve(reviewed.clone(), cx)
                                    })),
                            )
                        })
                        .child(
                            Button::new("next-review")
                                .ghost()
                                .small()
                                .label("Next needs you")
                                .on_click(cx.listener(|v, _, _, cx| {
                                    let p = v.snapshot(cx);
                                    let ids = p
                                        .features
                                        .iter()
                                        .filter(|f| {
                                            matches!(
                                                f.state,
                                                FeatureState::Review | FeatureState::NeedsInput
                                            )
                                        })
                                        .map(|f| f.id.clone())
                                        .collect::<Vec<_>>();
                                    if !ids.is_empty() {
                                        let current = ids
                                            .iter()
                                            .position(|id| Some(id) == v.selected.as_ref())
                                            .unwrap_or(0);
                                        v.inspect(ids[(current + 1) % ids.len()].clone(), cx);
                                    }
                                })),
                        ),
                )
                .child(Textarea::new(&self.feedback))
                .child(
                    Button::new("feature-revise")
                        .outline()
                        .small()
                        .label("Keep working")
                        .disabled(self.busy)
                        .on_click(cx.listener(|v, _, window, cx| v.revise(window, cx))),
                );
        }
        card.into_any_element()
    }
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
        if self.feedback_context != self.selected {
            self.feedback_context = self.selected.clone();
            self.feedback
                .update(cx, |s, cx| s.set_value("", window, cx));
        }
        let ui = Ui::of(cx);
        let p = self.snapshot(cx);
        let tail = p.messages.last().map(|m| m.id.clone());
        if self.last_message != tail {
            self.last_message = tail;
            if self.tab == "Conversation" && self.selected.is_none() {
                self.pin_frames = 2;
            }
        }
        if self.pin_frames > 0 {
            self.scroll
                .set_offset(point(px(0.), -self.scroll.max_offset().y));
            self.pin_frames -= 1;
            cx.notify();
        }
        let needs = p
            .features
            .iter()
            .filter(|f| matches!(f.state, FeatureState::NeedsInput | FeatureState::Review))
            .count()
            + self
                .model
                .read(cx)
                .threads
                .values()
                .filter(|t| t.read(cx).meta.project_root.as_deref() == Some(&self.project))
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
                            .child(format!("{:?}", p.state)),
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
                        .label(if p.state == RunState::Idle {
                            "Go"
                        } else {
                            "Resume"
                        })
                        .disabled(p.features.is_empty() || self.busy || p.planning)
                        .on_click(cx.listener(|v, _, _, cx| v.command("go", cx))),
                );
            }
            tabs = tabs.child(
                Button::new("stop-project")
                    .ghost()
                    .small()
                    .label("Stop")
                    .disabled(p.state != RunState::Running && !p.planning)
                    .on_click(cx.listener(|v, _, _, cx| v.command("stop", cx))),
            );
        }
        top = top.child(tabs);
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
            body=body.child(super::super::brand::handoff_card(&ui).gap_3()
            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child("Turn ideas into finished features"))
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
                        .disabled(p.state == RunState::Running)
                        .on_click(cx.listener(move |v, _, _, cx| {
                            let mut policy = v.snapshot(cx).policy;
                            policy.concurrency = count;
                            v.policy(policy, cx);
                        })),
                )
            }
            settings = settings
                .child(parallel)
                .child(
                    Button::new("manual-project-features")
                        .ghost()
                        .small()
                        .label("Manual features & saved model defaults")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(crate::actions::ManualFeature), cx)
                        }),
                )
                .child(
                    Button::new("disable-project-work")
                        .ghost()
                        .small()
                        .label("Turn workflow off")
                        .disabled(p.state == RunState::Running || p.planning || self.busy)
                        .on_click(cx.listener(|v, _, _, cx| v.enable(false, cx))),
                );
            body = body.child(settings);
        }
        body = body.child(self.approval_cards(&ui, cx));
        if let Some(f) = self
            .selected
            .as_ref()
            .and_then(|id| p.features.iter().find(|f| &f.id == id))
        {
            body = body.child(self.inspection(f, &ui, cx));
        } else if self.tab == "Git" {
            body = body.child(div().h(px(600.)).child(super::super::project::project_page(
                self.model.clone(),
                &ui,
                cx,
            )));
        } else if self.tab == "Board" {
            body = body.child(self.board(&p, &ui, cx));
        } else {
            if p.messages.is_empty() && p.enabled {
                body=body.child(div().text_sm().text_color(ui.text_muted).child("What do you want to build? You can describe several ideas at once and say which models you prefer for each part."));
            }
            for m in p.messages.iter().rev().take(60).rev() {
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
            for (i, check) in p.final_checks.iter().enumerate() {
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
            .child(Textarea::new(&self.input));
        let mut controls = div().flex().flex_wrap().items_center().gap_2();
        if let Some(a) = self.planner(cx) {
            controls = controls.child(self.picker("planner".into(), Choice::Planner, a, cx));
        }
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
            .child(composer)
    }
}
