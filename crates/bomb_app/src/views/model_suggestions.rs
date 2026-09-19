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
        if !routing::thread_enabled(&state.persistence, id) {
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
                if !self.routing_candidates(cx).contains(&suggestion.candidate) {
                    cx.notify();
                    return;
                }
                self.model.update(cx, |m, cx| {
                    m.set_backend(
                        &suggestion.candidate.backend,
                        Some(suggestion.candidate.model),
                        cx,
                    )
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
        self.routing_feedback = Some((
            self.model.read(cx).selected,
            if switch {
                "Model suggestion accepted."
            } else {
                "Using your selected model."
            }
            .into(),
        ));
        self.routing_pending = None;
        self.routing_result = None;
        self.routing_bypass = true;
        self.send(window, cx);
        self.routing_bypass = false;
    }

    pub(super) fn routing_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = crate::runtime::services(cx);
        if !state
            .config
            .try_read()
            .ok()
            .is_some_and(|c| c.model_suggestions.enabled)
        {
            return div().into_any_element();
        }
        let id = self.model.read(cx).selected;
        let enabled = routing::thread_enabled(&state.persistence, id);
        let ui = Ui::of(cx);
        let mut card = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .text_xs()
            .text_color(ui.text_muted)
            .whitespace_normal()
            .child(
                Button::new("jev-thread-toggle")
                    .ghost()
                    .small()
                    .disabled(id.is_none())
                    .label(if id.is_none() {
                        "JEV suggestions enabled · configurable per thread after sending"
                    } else if enabled {
                        "JEV suggestions: on · disable for this thread"
                    } else {
                        "JEV suggestions: off · enable for this thread"
                    })
                    .on_click(cx.listener(move |v, _, _, cx| {
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
                            }
                        }
                        v.routing_pending = None;
                        v.routing_result = None;
                        v.routing_suggestion = None;
                        cx.notify();
                    })),
            );
        if let Some((thread, message)) = &self.routing_feedback {
            if *thread == id {
                card = card.child(div().child(message.clone()));
            }
        }
        if self.routing_pending.is_some() {
            card = card.child(div().child("Checking model fit…")).child(
                Button::new("jev-skip")
                    .ghost()
                    .small()
                    .label("Send with current model")
                    .on_click(cx.listener(|v, _, w, cx| v.use_routing_choice(false, w, cx))),
            );
        }
        if let Some((_, suggestion)) = &self.routing_suggestion {
            card=card.child(div().text_color(ui.text).child(format!("Suggested: {}",suggestion.candidate.label)))
                .child(div().child("JEV recommends this model using your preferences and recent conversation. Switching carries conversation history to the selected agent."))
                .child(div().flex().gap_2()
                    .child(Button::new("jev-switch").small().label("Switch & send").on_click(cx.listener(|v,_,w,cx|v.use_routing_choice(true,w,cx))))
                    .child(Button::new("jev-current").ghost().small().label("Use current").on_click(cx.listener(|v,_,w,cx|v.use_routing_choice(false,w,cx)))));
        }
        card.into_any_element()
    }
}
