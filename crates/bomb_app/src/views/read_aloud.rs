//! The read-aloud player, docked above the composer while a reply in this thread is being read:
//! play/pause, 15 s back and forward, a timeline you can click or drag (rendered vs still to come),
//! elapsed and total, speed, close, and the message's first line (click to scroll to it).

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::*;

use crate::models::read_aloud::{ReadAloud, RATES};
use crate::speech::clock;
use crate::theme::Ui;
use crate::views::button::Button;

pub struct PlayerBar {
    model: Entity<ReadAloud>,
    /// Where the timeline sits on screen, for turning a click or drag into a time.
    track: Rc<Cell<Bounds<Pixels>>>,
    /// While dragging: the fraction under the pointer (the timeline follows it, not the audio).
    dragging: Option<f32>,
}

impl PlayerBar {
    pub fn new(model: Entity<ReadAloud>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self { model, track: Rc::new(Cell::new(Bounds::default())), dragging: None }
    }

    fn fraction_at(&self, x: Pixels) -> f32 {
        let b = self.track.get();
        if b.size.width <= px(0.) { return 0.0; }
        ((x - b.origin.x) / b.size.width).clamp(0.0, 1.0)
    }

    fn seek_to_fraction(&mut self, fraction: f32, cx: &mut Context<Self>) {
        let total = self.model.read(cx).status.estimated_total();
        self.model.update(cx, |m, cx| m.seek(total * fraction as f64, cx));
    }
}

fn speed_label(rate: f32) -> String {
    if (rate - rate.round()).abs() < 0.01 { format!("{}×", rate as i32) } else { format!("{rate}×") }
}

impl Render for PlayerBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::of(cx);
        let (status, title, rate) = {
            let m = self.model.read(cx);
            (m.status.clone(), m.playing.as_ref().map(|p| p.title.clone()).unwrap_or_default(), m.settings.rate)
        };
        let total = status.estimated_total().max(0.001);
        let played = self.dragging.unwrap_or((status.position / total) as f32).clamp(0.0, 1.0);
        let rendered = (status.duration / total).clamp(0.0, 1.0) as f32;
        let model = self.model.clone();
        let icon_button = |id: &'static str, icon: Lucide, tip: &'static str| {
            Button::new(id).ghost().small().icon(icon).tooltip(tip)
        };
        let track = self.track.clone();
        let timeline = div()
            .id("read-aloud-timeline")
            .relative()
            .flex_1()
            .min_w(px(80.))
            .h(px(18.))
            .flex()
            .items_center()
            .cursor_pointer()
            .child(canvas(move |bounds, _, _| track.set(bounds), |_, _, _, _| {}).absolute().inset_0())
            .child(
                div().relative().w_full().h(px(4.)).rounded_full().bg(ui.ink(0.08))
                    // Rendered so far, then what's been played.
                    .child(div().absolute().left_0().top_0().h_full().w(relative(rendered)).rounded_full().bg(ui.ink(0.16)))
                    .child(div().absolute().left_0().top_0().h_full().w(relative(played)).rounded_full().bg(ui.accent)),
            )
            .child(div().absolute().top(px(4.)).left(relative(played)).ml(px(-5.)).size(px(10.)).rounded_full().bg(ui.accent))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                this.dragging = Some(this.fraction_at(e.position.x));
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                if this.dragging.is_some() && e.pressed_button == Some(MouseButton::Left) {
                    this.dragging = Some(this.fraction_at(e.position.x));
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, e: &MouseUpEvent, _, cx| {
                let fraction = this.fraction_at(e.position.x);
                this.dragging = None;
                this.seek_to_fraction(fraction, cx);
            }))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| {
                if let Some(fraction) = this.dragging.take() { this.seek_to_fraction(fraction, cx); }
            }));
        let total_label = if status.complete || status.total == 0 { clock(total) } else { format!("~{}", clock(total)) };
        let shown_position = self.dragging.map(|f| f as f64 * total).unwrap_or(status.position);
        div()
            .mx(px(16.))
            .mb(px(8.))
            .px(px(12.))
            .py(px(8.))
            .rounded(px(14.))
            .border_1()
            .border_color(ui.border)
            .bg(ui.glass)
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div().flex().items_center().gap(px(8.))
                    .child(div().size(px(13.)).flex_shrink_0().text_color(ui.accent).child(Icon::from(Lucide::AudioLines)))
                    .child(
                        div().id("read-aloud-title").flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap()
                            .text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).cursor_pointer()
                            .tooltip(|window, cx| Tooltip::new("Go to this message").build(window, cx))
                            .on_click({ let model = model.clone(); move |_, _, cx| model.update(cx, |m, cx| m.reveal_playing(cx)) })
                            .child(if status.failed.is_some() { status.failed.clone().unwrap_or_default() } else { title }),
                    )
                    .child({
                        let model = model.clone();
                        Button::new("read-aloud-speed").ghost().small().label(speed_label(rate)).tooltip("Speed")
                            .dropdown_menu(move |mut menu, _, _| {
                                for r in RATES {
                                    let model = model.clone();
                                    menu = menu.item(PopupMenuItem::new(speed_label(r)).checked((r - rate).abs() < 0.01)
                                        .on_click(move |_, _, cx| model.update(cx, |m, cx| m.set_rate(r, cx))));
                                }
                                menu
                            })
                    })
                    .child(icon_button("read-aloud-close", Lucide::X, "Stop reading")
                        .on_click({ let model = model.clone(); move |_, _, cx| model.update(cx, |m, cx| m.close(cx)) })),
            )
            .child(
                div().flex().items_center().gap(px(6.))
                    .child(icon_button("read-aloud-back", Lucide::RotateCcw, "Back 15 seconds")
                        .on_click({ let model = model.clone(); move |_, _, cx| model.update(cx, |m, cx| m.skip(-15.0, cx)) }))
                    .child(
                        div().id("read-aloud-play").size(px(30.)).flex_shrink_0().flex().items_center().justify_center().rounded_full()
                            .bg(ui.text).text_color(ui.bg).cursor_pointer()
                            .tooltip(move |window, cx| Tooltip::new(if status.playing { "Pause" } else { "Play" }).build(window, cx))
                            .on_click({ let model = model.clone(); move |_, _, cx| model.update(cx, |m, cx| m.play_pause(cx)) })
                            .child(div().size(px(13.)).child(Icon::from(if status.playing { Lucide::Pause } else { Lucide::Play }))),
                    )
                    .child(icon_button("read-aloud-forward", Lucide::RotateCw, "Forward 15 seconds")
                        .on_click({ let model = model.clone(); move |_, _, cx| model.update(cx, |m, cx| m.skip(15.0, cx)) }))
                    .child(div().w(px(36.)).flex_shrink_0().text_size(px(crate::theme::Type::CAPTION)).font_family(ui.mono.clone()).text_color(ui.text_faint).child(clock(shown_position)))
                    .child(timeline)
                    .child(div().w(px(44.)).flex_shrink_0().text_right().text_size(px(crate::theme::Type::CAPTION)).font_family(ui.mono.clone()).text_color(ui.text_faint).child(total_label)),
            )
    }
}
