//! Composer-owned advisory routing; a result is tied to the exact draft and destination.
use super::*;
use bomb_core::services::model_suggestions::{self as routing, Candidate};
use std::hash::{Hash, Hasher};

impl ComposerView {
    fn routing_signature(&self, cx: &App) -> String {
        let m = self.model.read(cx);
        let state = crate::runtime::services(cx);
        let config = state
            .config
            .try_read()
            .ok()
            .map(|c| format!("{:?}", c.model_suggestions))
            .unwrap_or_default();
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        for a in &self.attachments {
            a.name.hash(&mut hash);
            a.mime.hash(&mut hash);
            a.bytes.hash(&mut hash);
        }
        format!(
            "{:?}",
            (
                m.selected,
                m.active_project.clone(),
                m.active_workspace.clone(),
                format!("{:?}", m.prefs),
                m.effective_model(),
                self.input.read(cx).value().to_string(),
                hash.finish(),
                config
            )
        )
    }

    fn routing_candidates(&self, cx: &App) -> Vec<Candidate> {
        let m = self.model.read(cx);
        m.backends
            .iter()
            .filter(|b| {
                b.available
                    && b.model_error.is_none()
                    && m.auth
                        .iter()
                        .any(|a| a.backend == b.id && a.logged_in && a.runnable)
            })
            .flat_map(|b| {
                b.models.iter().map(|id| Candidate {
                    backend: b.id.clone(),
                    model: id.clone(),
                    label: format!(
                        "{} · {}",
                        b.model_names.get(id).unwrap_or(id),
                        b.display_name
                    ),
                    description: b.model_descriptions.get(id).cloned().unwrap_or_default(),
                })
            })
            .collect()
    }

    pub(super) fn check_model_suggestion(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let state = crate::runtime::services(cx);
        if !state
            .config
            .try_read()
            .ok()
            .is_some_and(|c| c.model_suggestions.enabled)
        {
            return false;
        }
        let id = self.model.read(cx).selected;
        if !id
            .map(|id| routing::thread_enabled(&state.persistence, Some(id)))
            .unwrap_or(true)
        {
            return false;
        }
        // Jev is text-only. Keep attachment turns on the user's selected model.
        if text.starts_with('/') || !self.attachments.is_empty() {
            self.routing_feedback = Some((
                id,
                "JEV skipped: attachment turns and slash commands use your selected model.".into(),
            ));
            return false;
        }
        let signature = self.routing_signature(cx);
        if self.routing_pending.as_ref() == Some(&signature)
            || self
                .routing_suggestion
                .as_ref()
                .is_some_and(|(s, _)| s == &signature)
        {
            return true;
        }
        let candidates = self.routing_candidates(cx);
        let m = self.model.read(cx);
        let Some(current) = candidates
            .iter()
            .find(|c| c.backend == m.prefs.backend && c.model == m.effective_model())
            .cloned()
        else {
            self.routing_feedback=Some((id,"JEV skipped: the current model is not in a signed-in provider’s discovered catalog. Refresh provider models.".into()));
            return false;
        };
        if candidates.len() < 2 {
            self.routing_feedback=Some((id,"JEV skipped: at least two available models are needed. Refresh provider models or connect another provider.".into()));
            return false;
        }
        self.routing_feedback = None;
        self.routing_serial = self.routing_serial.wrapping_add(1);
        let serial = self.routing_serial;
        self.routing_pending = Some(signature.clone());
        self.routing_suggestion = None;
        self.routing_result = None;
        let prompt = text.to_string();
        let weak = cx.entity().downgrade();
        crate::runtime::spawn_service(
            cx,
            async move { routing::suggest(&state, id, prompt, current, candidates).await },
            move |result, cx| {
                let _ = weak.update(cx, |v, cx| {
                    if v.routing_serial == serial && v.routing_pending.as_ref() == Some(&signature)
                    {
                        v.routing_result = Some((signature, result));
                    }
                    cx.notify();
                });
            },
        );
        cx.notify();
        true
    }

    pub(super) fn apply_routing_result(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let signature = self.routing_signature(cx);
        if self
            .routing_pending
            .as_ref()
            .is_some_and(|s| s != &signature)
        {
            self.routing_pending = None;
        }
        if self
            .routing_suggestion
            .as_ref()
            .is_some_and(|(s, _)| s != &signature)
        {
            self.routing_suggestion = None;
        }
        if let Some((expected, result)) = self.routing_result.take() {
            if self.routing_pending.as_ref() != Some(&expected) || signature != expected {
                return;
            }
            self.routing_pending = None;
            let id = self.model.read(cx).selected;
            match result {
                Ok(evaluation) => {
                    self.routing_feedback = Some((id, evaluation.message));
                    if let Some(suggestion) = evaluation.suggestion {
                        if self.routing_candidates(cx).contains(&suggestion.candidate) {
                            self.routing_effort = suggested_effort(
                                &suggestion.candidate.backend,
                                &self.model.read(cx).prefs.effort,
                            );
                            self.routing_choice = Some(suggestion.candidate.clone());
                            self.routing_details = false;
                            self.routing_suggestion = Some((expected, suggestion));
                            return;
                        }
                        self.routing_feedback = Some((
                            id,
                            "The suggested model is no longer available. Using the current model."
                                .into(),
                        ));
                    }
                }
                Err(error) => {
                    self.routing_feedback =
                        Some((id, format!("{error} Sent with the current model.")));
                    self.model.update(cx, |m, cx| {
                        m.toast(
                            ToastKind::Warning,
                            format!("{error} Using the current model."),
                        );
                        cx.notify();
                    });
                }
            }
            self.routing_bypass = true;
            self.send(window, cx);
            self.routing_bypass = false;
        }
    }

    fn use_routing_choice(&mut self, switch: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((signature, suggestion)) = self.routing_suggestion.take() {
            if signature != self.routing_signature(cx) {
                cx.notify();
                return;
            }
            if switch {
                let Some(candidate) = self.routing_choice.take() else {
                    return;
                };
                if !self.routing_candidates(cx).contains(&candidate) {
                    cx.notify();
                    return;
                }
                self.model.update(cx, |m, cx| {
                    m.set_backend(&candidate.backend, Some(candidate.model), cx);
                    m.prefs.effort = self.routing_effort.clone();
                    cx.notify();
                });
            } else if let Some(id) = self.model.read(cx).selected {
                let state = crate::runtime::services(cx);
                if let Err(error) = routing::dismiss(&state.persistence, id, &suggestion.candidate)
                {
                    self.model.update(cx, |m, cx| {
                        m.toast(ToastKind::Warning, error);
                        cx.notify();
                    });
                }
            }
        }
        self.routing_feedback = None;
        self.routing_pending = None;
        self.routing_result = None;
        self.routing_bypass = true;
        self.send(window, cx);
        self.routing_bypass = false;
    }

    pub(super) fn routing_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = crate::runtime::services(cx);
        let configured = state
            .config
            .try_read()
            .ok()
            .is_some_and(|c| c.model_suggestions.enabled);
        let id = self.model.read(cx).selected;
        let enabled = configured
            && id
                .map(|id| routing::thread_enabled(&state.persistence, Some(id)))
                .unwrap_or(true);
        let ui = Ui::of(cx);
        let weak = cx.entity().downgrade();
        let feedback = self
            .routing_feedback
            .as_ref()
            .filter(|(thread, _)| *thread == id)
            .map(|(_, text)| text.clone());
        let trigger = Button::new("smart-routing")
            .ghost()
            .small()
            .icon(Lucide::Route)
            .selected(enabled)
            .tooltip(if !configured {
                "Enable JEV in Settings to use Smart Model Routing."
            } else if enabled {
                "Smart Model Routing · On"
            } else {
                "Smart Model Routing · Off"
            });
        if !configured {
            // Keep hover handling on the wrapper so the disabled control explains itself.
            return div()
                .id("smart-routing-disabled")
                .tooltip(|window, cx| {
                    Tooltip::new("Enable JEV in Settings to use Smart Model Routing.")
                        .build(window, cx)
                })
                .child(trigger.disabled(true))
                .into_any_element();
        }
        Popover::new("smart-routing-menu")
            .anchor(Anchor::BottomLeft)
            .trigger(trigger)
            .content(move |_, _, _cx| {
                let weak = weak.clone();
                div()
                    .w(px(300.))
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .text_sm()
                    .text_color(ui.text)
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Smart Model Routing"),
                    )
                    .child(
                        div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child(
                            "Suggest a model for each prompt. You choose whether to switch.",
                        ),
                    )
                    .child(
                        Button::new("routing-toggle")
                            .ghost()
                            .small()
                            .disabled(id.is_none())
                            .label(if id.is_none() {
                                "Thread toggle available after first send"
                            } else if enabled {
                                "Turn off for this thread"
                            } else {
                                "Turn on for this thread"
                            })
                            .on_click(move |_, _, cx| {
                                let _ = weak.update(cx, |v, cx| {
                                    if let Some(id) = id {
                                        if let Err(error) = routing::set_thread_enabled(
                                            &crate::runtime::services(cx).persistence,
                                            id,
                                            !enabled,
                                        ) {
                                            v.model.update(cx, |m, cx| {
                                                m.toast(ToastKind::Warning, error);
                                                cx.notify();
                                            });
                                            return;
                                        }
                                    }
                                    v.routing_pending = None;
                                    v.routing_result = None;
                                    v.routing_suggestion = None;
                                    v.routing_feedback = None;
                                    cx.notify();
                                });
                            }),
                    )
                    .when_some(feedback.clone(), |el, text| {
                        el.child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child(text))
                    })
                    .child(
                        Button::new("routing-settings")
                            .ghost()
                            .small()
                            .icon(Lucide::Settings)
                            .label("Settings · Smart Model Routing…")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(crate::actions::OpenSettings), cx)
                            }),
                    )
            })
            .into_any_element()
    }

    pub(super) fn routing_card(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.routing_pending.is_none() && self.routing_suggestion.is_none() {
            return div().into_any_element();
        }
        let ui = Ui::of(cx);
        let mut card = super::super::brand::handoff_card(&ui)
            .text_size(px(crate::theme::Type::SMALL))
            .text_color(ui.text_muted)
            .whitespace_normal();
        if self.routing_pending.is_some() {
            return card
                .child(div().child("Checking model fit…"))
                .child(
                    Button::new("routing-skip")
                        .ghost()
                        .small()
                        .label("Send with current model")
                        .on_click(cx.listener(|v, _, w, cx| v.use_routing_choice(false, w, cx))),
                )
                .into_any_element();
        }
        if let (Some(_), Some(choice)) = (&self.routing_suggestion, &self.routing_choice) {
            let m = self.model.read(cx);
            let current = m.model_name(&m.prefs.backend, &m.effective_model());
            let current_effort = super::super::brand::effort_label(&m.prefs.effort);
            let choices = self.routing_candidates(cx);
            let selected = choice.id();
            let weak = cx.entity().downgrade();
            let picker = Button::new("routing-model")
                .ghost()
                .compact()
                .child(super::super::brand::model_identity(
                    &choice.backend,
                    m.model_name(&choice.backend, &choice.model),
                    &ui,
                ))
                .dropdown_caret(true)
                .dropdown_menu(move |mut menu, _, _| {
                    for candidate in &choices {
                        let weak = weak.clone();
                        let candidate = candidate.clone();
                        menu = menu.item(
                            PopupMenuItem::new(candidate.label.clone())
                                .checked(candidate.id() == selected)
                                .on_click(move |_, _, cx| {
                                    let _ = weak.update(cx, |v, cx| {
                                        v.routing_effort =
                                            suggested_effort(&candidate.backend, &v.routing_effort);
                                        v.routing_choice = Some(candidate.clone());
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                    menu
                });
            let (levels, applies) = super::super::brand::effort_levels(&choice.backend);
            let effort = self.routing_effort.clone();
            let weak = cx.entity().downgrade();
            let effort_picker = Button::new("routing-effort")
                .ghost()
                .small()
                .disabled(!applies)
                .label(if applies {
                    format!("Reasoning: {}", super::super::brand::effort_label(&effort))
                } else {
                    "Reasoning: provider default".into()
                })
                .dropdown_caret(true)
                .dropdown_menu(move |mut menu, _, _| {
                    for level in levels {
                        let weak = weak.clone();
                        let level = *level;
                        menu = menu.item(
                            PopupMenuItem::new(super::super::brand::effort_label(level))
                                .checked(effort == level)
                                .on_click(move |_, _, cx| {
                                    let _ = weak.update(cx, |v, cx| {
                                        v.routing_effort = level.into();
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                    menu
                });
            card = card
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_wrap()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(super::super::brand::model_identity(
                                    &m.prefs.backend,
                                    current.clone(),
                                    &ui,
                                ))
                                .child(
                                    div()
                                        .text_size(px(crate::theme::Type::CAPTION))
                                        .text_color(ui.text_faint)
                                        .child(current_effort),
                                ),
                        )
                        .child(Icon::from(Lucide::ArrowRight).size(px(14.)))
                        .child(picker)
                        .child(effort_picker),
                )
                .child(
                    div().text_size(px(crate::theme::Type::CAPTION)).text_color(ui.text_faint).child(
                        "Suggested for this prompt. Recent conversation history carries over.",
                    ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new("routing-switch")
                                .primary()
                                .small()
                                .label("Switch & send")
                                .on_click(
                                    cx.listener(|v, _, w, cx| v.use_routing_choice(true, w, cx)),
                                ),
                        )
                        .child(
                            Button::new("routing-current")
                                .ghost()
                                .small()
                                .label("Keep current")
                                .on_click(
                                    cx.listener(|v, _, w, cx| v.use_routing_choice(false, w, cx)),
                                ),
                        )
                        .child(
                            Button::new("routing-why")
                                .ghost()
                                .small()
                                .label("Why this model")
                                .icon(if self.routing_details {
                                    Lucide::ChevronUp
                                } else {
                                    Lucide::ChevronDown
                                })
                                .on_click(cx.listener(|v, _, _, cx| {
                                    v.routing_details = !v.routing_details;
                                    cx.notify();
                                })),
                        ),
                );
            if self.routing_details {
                card=card.child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child("Based on your model preferences, this prompt and recent conversation. Conversation history carries over when switching providers."));
                if let Some((_, message)) = &self.routing_feedback {
                    card = card.child(
                        div()
                            .text_size(px(crate::theme::Type::SMALL))
                            .text_color(ui.text_muted)
                            .child(message.clone()),
                    );
                }
            }
        }
        card.into_any_element()
    }
}

fn suggested_effort(backend: &str, current: &str) -> String {
    let (levels, _) = super::super::brand::effort_levels(backend);
    if levels.contains(&current) {
        current.into()
    } else {
        levels
            .iter()
            .find(|e| **e == "high")
            .or(levels.last())
            .copied()
            .unwrap_or_default()
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::suggested_effort;
    #[test]
    fn destination_effort_preserves_supported_levels_and_replaces_unsupported_ones() {
        assert_eq!(suggested_effort("claude", "high"), "high");
        assert_eq!(suggested_effort("claude", "minimal"), "high");
        assert_eq!(suggested_effort("codex", "max"), "high");
        assert_eq!(suggested_effort("unknown", "high"), "");
    }
}
