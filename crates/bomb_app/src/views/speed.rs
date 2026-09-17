//! Session-advertised speed controls. Never substitute reasoning effort for speed.
use crate::{
    models::app::AppModel,
    runtime::{services, spawn_service},
};
use gpui_kit::component::{
    button::{Button, ButtonVariants},
    menu::{DropdownMenu, PopupMenuItem},
    Sizable,
};
use gpui_kit::*;
type SpeedOption = (String, Vec<String>, Option<String>);
pub struct SpeedSelector {
    model: Entity<AppModel>,
    key: Option<(uuid::Uuid, String)>,
    option: Option<SpeedOption>,
    loading: bool,
}
impl SpeedSelector {
    pub fn new(model: Entity<AppModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |this, _, cx| {
            this.refresh(true, cx);
            cx.notify();
        })
        .detach();
        Self {
            model,
            key: None,
            option: None,
            loading: false,
        }
    }
    fn refresh(&mut self, force: bool, cx: &mut Context<Self>) {
        let m = self.model.read(cx);
        let key = m.selected.filter(|id| m.threads.get(id).is_some_and(|thread| { let meta = &thread.read(cx).meta; meta.live && meta.backend == m.prefs.backend && meta.model == m.effective_model() })).map(|id| (id, m.effective_model()));
        if self.loading || (!force && self.key == key) {
            return;
        }
        if self.key != key { self.option = None; }
        self.key = key.clone();
        let Some((id, _)) = key.clone() else {
            return;
        };
        self.loading = true;
        let state = services(cx);
        let weak = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { state.registry.speed_option(id).await },
            move |result, cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.loading = false;
                    if this.key == key {
                        match result {
                            Ok(option) => this.option = option,
                            Err(_) => this.option = None,
                        }
                    }
                    cx.notify();
                });
            },
        );
    }
}
impl Render for SpeedSelector {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh(false, cx);
        let m = self.model.read(cx);
        if m.prefs.backend != "codex" && self.option.is_none() { return div().into_any_element(); }
        let chosen = m.prefs.fast_mode;
        let current = self.option.as_ref().and_then(|(option, _, value)| value.as_deref().map(|value| speed_label(option, value)));
        let label = match chosen {
            Some(true) => "Fast · On".to_string(), Some(false) => "Fast · Off".to_string(),
            None => match current.as_deref() { Some("Fast" | "Priority") => "Fast · On".into(), Some("Standard") => "Fast · Off".into(), _ => "Fast · Default".into() }
        };
        let model = self.model.clone();
        Button::new("speed-selector").ghost().small().label(label).dropdown_caret(true)
            .tooltip("Applies to your next prompt. Fast mode uses more allowance; availability is checked when the agent connects.")
            .dropdown_menu(move |mut menu, _, _| {
                for (label, value) in [("Agent default", None), ("Standard", Some(false)), ("Fast", Some(true))] {
                    let model = model.clone();
                    menu = menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                        model.update(cx, |m, cx| { m.prefs.fast_mode = value; cx.notify(); });
                    }));
                }
                menu
            }).into_any_element()
    }
}

fn speed_label(option: &str, value: &str) -> String {
    match (option, value) {
        ("fast-mode" | "fast_mode" | "fastMode" | "fast", "true" | "on" | "enabled") => "Fast".into(),
        ("fast-mode" | "fast_mode" | "fastMode" | "fast", "false" | "off" | "disabled") => "Standard".into(),
        (_, "fast") => "Fast".into(),
        (_, "standard" | "normal" | "default") => "Standard".into(),
        (_, "priority") => "Priority".into(),
        _ => value.replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use super::speed_label;
    #[test]
    fn codex_fast_mode_has_readable_labels() {
        assert_eq!(speed_label("fast-mode", "on"), "Fast");
        assert_eq!(speed_label("fast-mode", "off"), "Standard");
    }
}
