//! Session-advertised speed controls. Never substitute reasoning effort for speed.
use crate::views::button::Button;
use crate::{
    models::app::AppModel,
    runtime::{services, spawn_service},
};
use gpui_kit::component::{
    button::ButtonVariants,
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
        let chosen = m.prefs.fast_mode.unwrap_or(false);
        let label = if chosen { "Fast · On" } else { "Fast · Off" };
        let model = self.model.clone();
        Button::new("speed-selector").ghost().small().label(label).dropdown_caret(true)
            .tooltip("Applies to your next prompt. Fast mode uses more allowance; availability is checked when the agent connects.")
            .dropdown_menu(move |mut menu, _, _| {
                for (label, value) in [("Off (standard)", false), ("On (fast)", true)] {
                    let model = model.clone();
                    menu = menu.item(PopupMenuItem::new(label).checked(value == chosen).on_click(move |_, _, cx| {
                        model.update(cx, |m, cx| { m.prefs.fast_mode = Some(value); cx.notify(); });
                    }));
                }
                menu
            }).into_any_element()
    }
}
