//! Session-advertised speed controls. Never substitute reasoning effort for speed.
use crate::{
    models::app::{AppModel, ToastKind},
    runtime::{services, spawn_service},
};
use gpui_kit::base::Disableable;
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
        let key = m.selected.filter(|id| m.threads.get(id).is_some_and(|thread| thread.read(cx).meta.live)).map(|id| (id, m.effective_model()));
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
        let Some((option, values, current)) = self.option.clone() else {
            return div().into_any_element();
        };
        let Some((id, _)) = self.key.clone() else {
            return div().into_any_element();
        };
        let weak = cx.entity().downgrade();
        let model = self.model.clone();
        Button::new("speed-selector").ghost().small().label(format!("Speed · {}", current.as_deref().map(|value| speed_label(&option, value)).unwrap_or_else(|| "Agent default".into())))
            .disabled(self.loading).dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
                for value in &values {
                    let value = value.clone(); let option = option.clone(); let weak = weak.clone(); let model = model.clone();
                    menu = menu.item(PopupMenuItem::new(speed_label(&option, &value)).on_click(move |_, _, cx| {
                        let value = value.clone(); let option = option.clone(); let weak = weak.clone(); let model = model.clone();
                        let _ = weak.update(cx, |this, cx| { this.loading = true; cx.notify(); });
                        let state = services(cx); let requested = value.clone();
                        spawn_service(cx, async move { state.registry.set_speed_option(id, &option, &requested).await }, move |result, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.loading = false;
                                if matches!(result, Ok(true)) && this.key.as_ref().is_some_and(|(sid,_)| *sid == id) {
                                    if let Some((_,_,current)) = &mut this.option { *current = Some(value); }
                                }
                                cx.notify();
                            });
                            if !matches!(result, Ok(true)) { model.update(cx, |m, cx| { m.toast(ToastKind::Error, match result { Err(e) => e.to_string(), _ => "This agent no longer supports that speed option.".into() }); cx.notify(); }); }
                        });
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
