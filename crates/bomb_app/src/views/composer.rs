//! The composer: a pill with a growing textarea, attachment tray, model and
//! mode pickers, attach, and a send button that morphs into stop while a
//! turn runs. Enter sends, Shift+Enter inserts a newline, Shift+Tab cycles
//! the approval mode.

use std::sync::Arc;

use base64::Engine;
use bomb_core::services::ImageInput;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Side, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::CycleApprovalMode;
use crate::models::app::{AppModel, ToastKind, APPROVAL_CYCLE};
use crate::theme::{Layout, Ui};

const MAX_ATTACHMENTS: usize = 8;
const MAX_TOTAL_BYTES: usize = 12 * 1024 * 1024;

#[derive(Clone)]
pub struct Attachment {
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
    pub image: Arc<Image>,
}

pub struct ComposerView {
    model: Entity<AppModel>,
    input: Entity<TextareaState>,
    attachments: Vec<Attachment>,
    drag_over: bool,
    model_menu_open: bool,
    model_search: Entity<InputState>,
    provider_filter: Option<String>,
    speed: Entity<super::speed::SpeedSelector>,
}

impl ComposerView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Do anything…")
                .auto_grow(1, 10)
                .submit_on_enter(true)
        });
        cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { shift: false, .. } = ev {
                this.send(window, cx);
            }
        })
        .detach();
        let speed = cx.new(|cx| super::speed::SpeedSelector::new(model.clone(), cx));
        Self {
            speed,
            model,
            input,
            attachments: Vec::new(),
            drag_over: false,
            model_menu_open: false,
            model_search: cx.new(|cx| InputState::new(window, cx).placeholder("Search models…")),
            provider_filter: None,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| s.focus(window, cx));
    }

    /// Prefill the input (empty-state suggestions) and focus it.
    pub fn set_text(&self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| s.set_value(text.to_string(), window, cx));
        self.focus(window, cx);
    }

    /// Context usage ring: tokens used vs the model's window; red past 85%.
    fn context_ring(&self, ui: &Ui, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.model.read(cx);
        let t = m.selected_thread()?;
        let used = t.read(cx).thread.context_tokens?;
        let window_tokens = context_window(&m.prefs.backend, &m.effective_model());
        let frac = (used as f32 / window_tokens as f32).clamp(0.0, 1.0);
        let pct = (frac * 100.0).round() as u32;
        let color = if frac >= 0.9 { ui.danger } else if frac >= 0.75 { ui.warning } else { ui.text_muted };
        let hover = ui.ink(0.05);
        Some(
            div()
                .id("context-ring")
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(5.))
                .px(px(6.))
                .h(px(24.))
                .rounded(px(6.))
                .text_size(px(11.))
                .text_color(color)
                .hover(move |s| s.bg(hover))
                .child(div().size(px(16.)).child(ProgressCircle::new("ctx-ring").value(pct as f32).color(color)))
                .child(format!("{pct}%"))
                .tooltip(move |window, cx| {
                    Tooltip::new(format!(
                        "{} of {} tokens ({pct}%)",
                        bomb_core::presence::format_count(used as usize),
                        bomb_core::presence::format_count(window_tokens as usize)
                    ))
                    .build(window, cx)
                })
                .into_any_element(),
        )
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.input.read(cx).value().to_string();
        let text = text.trim().to_string();
        if text.is_empty() && self.attachments.is_empty() {
            return;
        }
        let busy = self
            .model
            .read(cx)
            .selected_thread()
            .map(|t| t.read(cx).thread.presence.turn_active())
            .unwrap_or(false);
        if busy || self.model.read(cx).starting {
            self.model.update(cx, |m, cx| {
                m.toast(ToastKind::Warning, "The agent is still working — stop it first.");
                cx.notify();
            });
            return;
        }
        let images: Vec<ImageInput> = self
            .attachments
            .drain(..)
            .map(|a| ImageInput {
                mime_type: a.mime,
                data: base64::engine::general_purpose::STANDARD.encode(&a.bytes),
                name: Some(a.name),
            })
            .collect();
        self.input.update(cx, |s, cx| s.set_value("", window, cx));
        self.model.update(cx, |m, cx| m.send_prompt(text, images, cx));
        self.focus(window, cx);
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| m.cancel_selected(cx));
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        cx.spawn(async move |weak, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let _ = weak.update(cx, |this, cx| {
                for p in paths {
                    this.attach_path(&p, cx);
                }
            });
        })
        .detach();
    }

    fn attach_path(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let Some(format) = format_for_path(path) else {
            self.model.update(cx, |m, cx| {
                m.toast(ToastKind::Warning, format!("Not an image: {}", path.display()));
                cx.notify();
            });
            return;
        };
        match std::fs::read(path) {
            Ok(bytes) => {
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "image".into());
                self.attach_bytes(name, format, bytes, cx);
            }
            Err(e) => self.model.update(cx, |m, cx| {
                m.toast(ToastKind::Error, format!("Could not read {}: {e}", path.display()));
                cx.notify();
            }),
        }
    }

    fn attach_bytes(&mut self, name: String, format: ImageFormat, bytes: Vec<u8>, cx: &mut Context<Self>) {
        let total: usize = self.attachments.iter().map(|a| a.bytes.len()).sum::<usize>() + bytes.len();
        if self.attachments.len() >= MAX_ATTACHMENTS || total > MAX_TOTAL_BYTES {
            self.model.update(cx, |m, cx| {
                m.toast(ToastKind::Warning, "Attachment limit: 8 images, 12 MB total.");
                cx.notify();
            });
            return;
        }
        let image = Arc::new(Image::from_bytes(format, bytes.clone()));
        self.attachments.push(Attachment {
            name,
            mime: mime_for(format).to_string(),
            bytes,
            image,
        });
        cx.notify();
    }

    fn paste_from_clipboard(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(item) = cx.read_from_clipboard() else { return false };
        let mut took = false;
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(img) => {
                    let n = self.attachments.len() + 1;
                    self.attach_bytes(format!("pasted-{n}.{}", ext_for(img.format())), img.format(), img.bytes().to_vec(), cx);
                    took = true;
                }
                ClipboardEntry::ExternalPaths(paths) => {
                    for p in paths.paths() {
                        if format_for_path(p).is_some() {
                            self.attach_path(p, cx);
                            took = true;
                        }
                    }
                }
                ClipboardEntry::String(_) => {}
            }
        }
        took
    }

    fn remove_attachment(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.attachments.len() {
            self.attachments.remove(ix);
            cx.notify();
        }
    }

    // ── render pieces ───────────────────────────────────────────────────

    fn tray(&self, ui: &Ui, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.attachments.is_empty() {
            return None;
        }
        let border = ui.border;
        Some(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .px_4()
                .pt_3()
                .children(self.attachments.iter().enumerate().map(|(ix, a)| {
                    div()
                        .id(("att", ix))
                        .relative()
                        .size(px(56.))
                        .rounded(px(8.))
                        .overflow_hidden()
                        .border_1()
                        .border_color(border)
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| this.remove_attachment(ix, cx)))
                        .child(img(a.image.clone()).size_full().object_fit(ObjectFit::Cover))
                        .child(
                            div()
                                .absolute()
                                .top_0p5()
                                .right_0p5()
                                .size(px(16.))
                                .rounded_full()
                                .bg(ui.solid)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(div().size(px(10.)).text_color(ui.on_solid).child(Icon::from(Lucide::X))),
                        )
                }))
                .into_any_element(),
        )
    }

    /// Model + reasoning selector, Zeron's picker: a 32px trigger chip
    /// (mark · readable model · effort), and a 304px popover with a provider
    /// tab strip (underline on the viewed tab), a borderless search row,
    /// name + blurb rows with ⌘1–⌘9 hints, and a Reasoning list.
    fn model_selector(&self, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let backend = m.prefs.backend.clone();
        let model = m.effective_model();
        let effort = m.prefs.effort.clone();
        let app = self.model.clone();
        let this = cx.entity().clone();
        let search = self.model_search.clone();
        let trigger_label = if model.is_empty() { backend.clone() } else { crate::views::brand::pretty_model(&model) };
        let (_, effort_applies) = crate::views::brand::effort_levels(&backend);
        let eff_label = if effort_applies { crate::views::brand::effort_label(&effort) } else { "" };
        let open = self.model_menu_open;
        let trigger = Button::new("model-selector")
            .ghost()
            .compact()
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(32.))
                    .max_w(px(248.))
                    .min_w_0()
                    .gap(px(6.))
                    .px(px(2.))
                    .child(crate::views::brand::brand_mark(&backend, 16., true, ui))
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if open { ui.text } else { Ui::alpha(ui.text, 0.9) })
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(trigger_label),
                    )
                    .when(!eff_label.is_empty(), |el| {
                        el.child(
                            div()
                                .min_w_0()
                                .flex_shrink(1000.)
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(Ui::alpha(ui.text_muted, 0.7))
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(eff_label),
                        )
                    }),
            );
        let popover = Popover::new("model-selector-popover")
            .anchor(Anchor::BottomLeft)
            .trigger(trigger)
            .open(self.model_menu_open)
            .on_open_change({
                let this = this.clone();
                move |open, _, cx| {
                    let open = *open;
                    this.update(cx, |c, cx| {
                        c.model_menu_open = open;
                        cx.notify();
                    })
                }
            })
            .content(move |_, _, cx| {
                let ui = Ui::of(cx);
                let (backends, cur_backend, cur_model, cur_effort) = {
                    let m = app.read(cx);
                    (m.backends.clone(), m.prefs.backend.clone(), m.effective_model(), m.prefs.effort.clone())
                };
                let query = search.read(cx).value().to_lowercase();
                // The viewed tab: an explicit pick, else the current backend.
                let viewed: Option<String> = match &this.read(cx).provider_filter {
                    Some(f) if f == "*" => None,
                    Some(f) => Some(f.clone()),
                    None => Some(cur_backend.clone()),
                };
                let hairline = ui.hairline(0.08);
                let ink06 = ui.ink(0.06);
                let ink05 = ui.ink(0.05);
                let mut col = div().flex().flex_col().w(px(304.)).text_size(px(13.)).text_color(ui.text);

                // ── tab strip ────────────────────────────────────────
                let mut tabs = div()
                    .flex()
                    .items_center()
                    .h(px(40.))
                    .px(px(4.))
                    .gap(px(2.))
                    .border_b_1()
                    .border_color(hairline);
                let tab = |id: SharedString, on: bool, child: AnyElement, this: Entity<Self>, key: Option<String>| {
                    div()
                        .id(id)
                        .relative()
                        .size(px(32.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(8.))
                        .cursor_pointer()
                        .when(!on, move |el| el.hover(move |s| s.bg(ink06)))
                        .on_click(move |_, _, cx| {
                            this.update(cx, |c, cx| {
                                c.provider_filter = Some(key.clone().unwrap_or_else(|| "*".into()));
                                cx.notify();
                            })
                        })
                        .child(child)
                        .when(on, |el| {
                            el.child(div().absolute().bottom(px(-4.)).left(px(6.)).right(px(6.)).h(px(2.)).rounded(px(1.)).bg(ui.accent))
                        })
                };
                let all_on = viewed.is_none();
                tabs = tabs.child(tab(
                    "tab-all".into(),
                    all_on,
                    div().size(px(15.)).text_color(if all_on { ui.text } else { ui.text_muted }).child(Icon::from(Lucide::Star)).into_any_element(),
                    this.clone(),
                    None,
                ));
                for b in &backends {
                    let on = viewed.as_deref() == Some(&b.id);
                    let mark = if on || b.id == "claude" {
                        crate::views::brand::brand_mark(&b.id, 16., true, &ui)
                    } else {
                        crate::views::brand::brand_mark(&b.id, 16., false, &ui)
                    };
                    tabs = tabs.child(
                        tab(SharedString::from(format!("tab-{}", b.id)), on, mark, this.clone(), Some(b.id.clone()))
                            .when(!b.available, |el| el.opacity(0.35)),
                    );
                }
                col = col.child(tabs);

                // ── search row ───────────────────────────────────────
                col = col.child(
                    div()
                        .flex()
                        .items_center()
                        .h(px(40.))
                        .px(px(10.))
                        .gap(px(8.))
                        .border_b_1()
                        .border_color(hairline)
                        .child(div().size(px(14.)).text_color(ui.text_muted).child(Icon::from(Lucide::Search)))
                        .child(div().flex_1().min_w_0().child(Input::new(&search).appearance(false).bordered(false))),
                );

                // ── model rows ───────────────────────────────────────
                let mut list = div().flex().flex_col().py(px(4.)).px(px(4.)).bg(ui.ink(0.02));
                let mut any = false;
                let mut ix = 0usize;
                let selected_bg = ui.selected_bg();
                for b in &backends {
                    if let Some(v) = &viewed {
                        if v != &b.id {
                            continue;
                        }
                    }
                    let models: Vec<String> = if b.models.is_empty() { vec![b.default_model.clone()] } else { b.models.clone() };
                    let models: Vec<String> = models
                        .into_iter()
                        .filter(|md| {
                            query.is_empty()
                                || md.to_lowercase().contains(&query)
                                || crate::views::brand::pretty_model(md).to_lowercase().contains(&query)
                                || b.display_name.to_lowercase().contains(&query)
                        })
                        .collect();
                    for md in models {
                        any = true;
                        let selected = b.id == cur_backend && md == cur_model;
                        let app = app.clone();
                        let this = this.clone();
                        let (bid, mdl) = (b.id.clone(), md.clone());
                        let available = b.available;
                        let blurb = crate::views::brand::model_blurb(&md).map(str::to_string).unwrap_or_else(|| md.clone());
                        let hint = if ix < 9 { Some(format!("⌘{}", ix + 1)) } else { None };
                        ix += 1;
                        list = list.child(
                            div().pb(px(2.)).child(
                                div()
                                    .id(SharedString::from(format!("model-{}-{md}", b.id)))
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .px(px(8.))
                                    .py(px(if viewed.is_some() { 5. } else { 6. }))
                                    .rounded(px(8.))
                                    .when(selected, |el| el.bg(selected_bg))
                                    .when(!selected && available, |el| el.hover(move |s| s.bg(ink05)))
                                    .when(!available, |el| el.opacity(0.4))
                                    .when(available, |el| {
                                        el.cursor_pointer().on_click(move |_, _, cx| {
                                            app.update(cx, |a, cx| a.set_backend(&bid, Some(mdl.clone()), cx));
                                            this.update(cx, |c, cx| {
                                                c.model_menu_open = false;
                                                cx.notify();
                                            });
                                        })
                                    })
                                    .when(viewed.is_none(), |el| el.child(crate::views::brand::brand_mark(&b.id, 13., true, &ui)))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .items_baseline()
                                            .gap(px(6.))
                                            .child(
                                                div()
                                                    .flex_shrink_0()
                                                    .text_size(px(12.5))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(ui.text)
                                                    .child(crate::views::brand::pretty_model(&md)),
                                            )
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .text_size(px(11.))
                                                    .text_color(ui.text_muted)
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .whitespace_nowrap()
                                                    .child(blurb),
                                            ),
                                    )
                                    .when_some(hint, |el, h| {
                                        el.child(
                                            div()
                                                .flex_shrink_0()
                                                .px(px(5.))
                                                .py(px(1.))
                                                .rounded(px(5.))
                                                .bg(ink05)
                                                .text_size(px(10.))
                                                .font_family(ui.mono.clone())
                                                .text_color(ui.text_muted)
                                                .child(h),
                                        )
                                    })
                                    .when(selected, |el| el.child(div().size(px(14.)).flex_shrink_0().text_color(ui.text).child(Icon::from(Lucide::Check)))),
                            ),
                        );
                    }
                }
                if !any {
                    list = list.child(div().px(px(8.)).py(px(10.)).text_size(px(12.)).text_color(ui.text_faint).child(format!("No model matches \"{query}\"")));
                }
                col = col.child(list);

                // ── reasoning ────────────────────────────────────────
                let (levels, applies) = crate::views::brand::effort_levels(&cur_backend);
                if !levels.is_empty() {
                    let default_level = crate::views::brand::default_effort(&cur_backend);
                    let mut tray = div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .px(px(4.))
                        .py(px(4.))
                        .border_t_1()
                        .border_color(hairline)
                        .child(
                            div()
                                .px(px(8.))
                                .pt(px(6.))
                                .pb(px(4.))
                                .text_size(px(10.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ui.text_muted)
                                .child(tracked_upper("Reasoning")),
                        );
                    for e in levels {
                        let app = app.clone();
                        let on = cur_effort == *e;
                        let e: &'static str = e;
                        tray = tray.child(
                            div()
                                .id(SharedString::from(format!("effort-{e}")))
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .h(px(30.))
                                .px(px(8.))
                                .rounded(px(8.))
                                .text_size(px(13.))
                                .text_color(if !applies { ui.text_faint } else if on { ui.text } else { Ui::alpha(ui.text, 0.9) })
                                .when(on && applies, |el| el.bg(selected_bg))
                                .when(!on && applies, |el| el.hover(move |s| s.bg(selected_bg)))
                                .when(applies, |el| el.cursor_pointer().on_click(move |_, _, cx| app.update(cx, |a, cx| a.set_effort(e, cx))))
                                .child(div().flex_1().min_w_0().child(crate::views::brand::effort_label(e)))
                                .when(e == default_level, |el| {
                                    el.child(div().flex_shrink_0().text_size(px(10.)).font_weight(FontWeight::SEMIBOLD).text_color(ui.text_muted).child("Default"))
                                })
                                .when(on, |el| el.child(div().size(px(14.)).flex_shrink_0().text_color(ui.text).child(Icon::from(Lucide::Check)))),
                        );
                    }
                    if cur_backend != "claude" {
                        tray = tray.child(div().px(px(8.)).pt(px(2.)).pb(px(4.)).text_size(px(10.)).text_color(ui.text_faint).child("Applies to new conversations"));
                    }
                    col = col.child(tray);
                }
                col
            });
        popover.into_any_element()
    }

    fn mcp_picker(&self, ui: &Ui, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.model.read(cx);
        if m.selected.is_some() || m.mcp_names.is_empty() {
            return None;
        }
        let names = m.mcp_names.clone();
        let chosen = m.prefs.mcp_servers.clone();
        let app = self.model.clone();
        let label = if chosen.is_empty() { "mcp".to_string() } else { format!("mcp: {}", chosen.len()) };
        Some(
            Button::new("mcp-picker")
                .ghost()
                .compact()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .h(px(32.))
                        .px(px(2.))
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(Ui::alpha(ui.text_muted, 0.7))
                        .child(label),
                )
                .dropdown_menu(move |mut menu, _, _| {
                    menu = menu.label("Attach to the new thread (auto-attach servers are always included)");
                    for n in &names {
                        let app = app.clone();
                        let name = n.clone();
                        let on = chosen.contains(n);
                        menu = menu.item(PopupMenuItem::new(n.clone()).checked(on).on_click(move |_, _, cx| {
                            app.update(cx, |a, cx| a.toggle_mcp_pref(&name, cx));
                        }));
                    }
                    menu
                })
                .into_any_element(),
        )
    }

    fn mode_picker(&self, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.model.read(cx).prefs.mode.clone();
        let app = self.model.clone();
        let (label, icon, description) = mode_presentation(&mode);
        Button::new("mode-picker")
            .ghost()
            .compact()
            .tooltip(format!("{description} · Shift+Tab to cycle modes"))
            .accessibility_label(format!("Approval mode: {label}"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(28.))
                    .gap(px(6.))
                    .px(px(6.))
                    .rounded(px(6.))
                    .bg(Ui::alpha(if mode == "yolo" { ui.warning } else { ui.text }, 0.06))
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if mode == "yolo" { ui.warning } else { ui.text_muted })
                    .child(Icon::from(icon).size(px(14.)))
                    .child(label)
                    .child(Icon::from(Lucide::ChevronDown).size(px(12.))),
            )
            .dropdown_menu(move |mut menu, _, _| {
                menu = menu.min_w(px(320.)).max_w(px(340.)).check_side(Side::Right)
                    .item(PopupMenuItem::label("Approval mode"));
                for m in APPROVAL_CYCLE {
                    if m == "yolo" { menu = menu.separator().item(PopupMenuItem::label("Advanced · explicit opt-in")); }
                    let app = app.clone();
                    let (label, icon, description) = mode_presentation(m);
                    menu = menu.item(PopupMenuItem::element(move |_, cx| {
                        let ui = Ui::of(cx);
                        div().flex().items_start().gap(px(10.)).py(px(6.)).w(px(260.))
                            .child(div().mt(px(2.)).size(px(16.)).text_color(if m == "yolo" { ui.warning } else { ui.text_muted })
                                .child(Icon::from(icon).size(px(16.))))
                            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.))
                                .child(div().text_size(px(12.)).line_height(px(16.)).font_weight(FontWeight::MEDIUM)
                                    .text_color(if m == "yolo" { ui.warning } else { ui.text }).child(label))
                                .child(div().text_size(px(11.)).line_height(px(15.)).text_color(ui.text_muted)
                                    .whitespace_normal().child(description)))
                    }).checked(m == mode).on_click(move |_, window, cx| {
                        super::approval_mode::request(app.clone(), m, window, cx);
                    }));
                }
                menu.separator().item(PopupMenuItem::label("Shift+Tab: Plan → Ask first → Auto"))
            })
            .into_any_element()
    }

}

/// UI names are separate from the backend's stable approval-mode identifiers.
fn mode_presentation(mode: &str) -> (&'static str, Lucide, &'static str) {
    match mode {
        "plan" => ("Plan", Lucide::BookOpen, "Investigate and propose changes before execution."),
        "auto" => ("Auto", Lucide::Zap, "Use the agent’s automatic approval policy."),
        "yolo" => ("Full access", Lucide::ShieldAlert, "Approve tools automatically. Deny rules still apply."),
        _ => ("Ask first", Lucide::ShieldCheck, "Request approval before running tools."),
    }
}

impl Render for ComposerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::of(cx);
        let (busy, starting, branch, worktree_on, has_thread) = {
            let m = self.model.read(cx);
            let t = m.selected_thread();
            let busy = t
                .as_ref()
                .map(|t| t.read(cx).thread.presence.turn_active())
                .unwrap_or(false);
            let branch = t.as_ref().and_then(|t| {
                t.read(cx)
                    .meta
                    .worktree
                    .as_deref()
                    .and_then(|w| std::path::Path::new(w).file_name())
                    .map(|s| s.to_string_lossy().to_string())
            });
            (busy, m.starting, branch, m.prefs.worktree && !m.prefs.temporary, t.is_some())
        };
        let new_target = self.model.read(cx).active_workspace.is_none();
        let location_label = self.model.read(cx).active_workspace.as_deref().and_then(|id| self.model.read(cx).workspaces.iter().find(|w| w.id == id)).map(|w| if w.inline { "Questions · files unchanged".into() } else { w.name.clone() }).unwrap_or_else(|| "New conversation".into());
        let has_text = !self.input.read(cx).value().trim().is_empty() || !self.attachments.is_empty();
        let app = self.model.clone();
        let (solid, on_solid, danger) = (ui.solid, ui.on_solid, ui.danger);

        let send_button: AnyElement = if busy {
            div()
                .id("composer-stop")
                .size(px(28.))
                .flex_shrink_0()
                .rounded_full()
                .bg(solid)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
                .child(div().size(px(11.)).rounded(px(3.)).bg(on_solid))
                .into_any_element()
        } else {
            let enabled = has_text && !starting;
            div()
                .id("composer-send")
                .size(px(28.))
                .flex_shrink_0()
                .rounded_full()
                .bg(solid)
                .flex()
                .items_center()
                .justify_center()
                .when(!enabled, |el| el.opacity(0.35))
                .when(enabled, |el| {
                    el.cursor_pointer()
                        .hover(|s| s.opacity(0.85))
                        .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))
                })
                .child(div().size(px(14.)).text_color(on_solid).child(Icon::from(Lucide::ArrowUp)))
                .into_any_element()
        };

        let drag_border = if self.drag_over { ui.accent } else { ui.pill_border() };
        let tray = self.tray(&ui, cx);
        let model_picker = self.model_selector(&ui, cx);
        let mode_picker = self.mode_picker(&ui, cx);
        let mcp_picker = self.mcp_picker(&ui, cx);
        let context_ring = self.context_ring(&ui, cx);
        let attach_hover = ui.ink(0.10);
        let hover = ui.hover;
        let _ = danger;

        div()
            .id("composer")
            .key_context("BombComposer")
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .px_6()
            .pb_4()
            .on_action(cx.listener(|this, _: &CycleApprovalMode, _, cx| {
                this.model.update(cx, |m, cx| m.cycle_mode(cx));
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                let k = &ev.keystroke;
                if k.modifiers.platform && k.key == "v" && this.paste_from_clipboard(cx) {
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .w_full()
                    .max_w(px(Layout::COMPOSER_MAX))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("composer-frame")
                            .w_full()
                            .flex()
                            .flex_col()
                            .rounded(px(Layout::COMPOSER_RADIUS))
                            .border_1()
                            .border_color(drag_border)
                            .bg(ui.input_bg)
                            .when(!ui.dark, |el| el.shadow_lg())
                            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                                this.drag_over = false;
                                for p in paths.paths() {
                                    this.attach_path(p, cx);
                                }
                            }))
                            .drag_over::<ExternalPaths>(move |s, _, _, _| s.border_color(hover))
                            .children(tray)
                            .child(
                                div()
                                    .flex()
                                    .items_end()
                                    .gap(px(Layout::SPACE_SM))
                                    .pl(px(16.))
                                    .pr(px(8.))
                                    .py(px(7.))
                                    .min_h(px(47.))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .py(px(5.))
                                            .text_size(px(14.))
                                            .line_height(px(22.75))
                                            .child(Textarea::new(&self.input).appearance(false).bordered(false)),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .flex_shrink_0()
                                            .gap(px(2.))
                                            .child(model_picker)
                                            .child(self.speed.clone())
                                            .child(mode_picker)
                                            .children(mcp_picker)
                                            .child(
                                                div()
                                                    .id("attach")
                                                    .size(px(28.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded_full()
                                                    .text_color(ui.text_muted)
                                                    .cursor_pointer()
                                                    .hover(move |s| s.bg(attach_hover))
                                                    .on_click(cx.listener(|this, _, _, cx| this.pick_files(cx)))
                                                    .child(div().size(px(16.)).child(Icon::from(Lucide::Paperclip))),
                                            )
                                            .child(div().w(px(6.)))
                                            .child(send_button),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .h(px(24.))
                            .gap(px(Layout::SPACE_XS))
                            .px(px(10.))
                            .child(footer_label(Lucide::MessageCircle, location_label, &ui))
                            .when(worktree_on, |el| el.child(footer_label(Lucide::GitBranch, "Isolated workspace".into(), &ui)))
                            .when_some(branch, |el, b| el.child(footer_label(Lucide::GitBranch, b, &ui)))
                            .when(!has_thread && new_target, |el| {
                                el.child(
                                    Button::new("conversation-intent")
                                        .ghost().small().compact()
                                        .label(if worktree_on { "Make changes" } else { "Ask a question" })
                                        .dropdown_caret(true)
                                        .dropdown_menu(move |menu, _, _| {
                                            let questions = app.clone(); let changes = app.clone();
                                            menu.item(PopupMenuItem::new("Ask a question — leave files unchanged").on_click(move |_, _, cx| questions.update(cx, |m, cx| m.set_new_intent(true, cx))))
                                                .item(PopupMenuItem::new("Make changes — isolated workspace").on_click(move |_, _, cx| changes.update(cx, |m, cx| m.set_new_intent(false, cx))))
                                        })
                                )
                            })
                            .child(div().flex_1())
                            .when(starting, |el| el.child(div().text_size(px(11.)).text_color(ui.text_faint).child("starting agent…")))
                            .children(context_ring),
                    ),
            )
    }
}

fn format_for_path(p: &std::path::Path) -> Option<ImageFormat> {
    match p.extension()?.to_string_lossy().to_ascii_lowercase().as_str() {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "webp" => Some(ImageFormat::Webp),
        "gif" => Some(ImageFormat::Gif),
        "bmp" => Some(ImageFormat::Bmp),
        "svg" => Some(ImageFormat::Svg),
        _ => None,
    }
}

fn mime_for(f: ImageFormat) -> &'static str {
    match f {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Webp => "image/webp",
        ImageFormat::Gif => "image/gif",
        ImageFormat::Svg => "image/svg+xml",
        ImageFormat::Bmp => "image/bmp",
        _ => "application/octet-stream",
    }
}

fn ext_for(f: ImageFormat) -> &'static str {
    match f {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        _ => "bin",
    }
}

/// Best-known context windows (tokens). Unknown models get a conservative
/// 200k so the ring errs toward warning early.
fn context_window(backend: &str, model: &str) -> u64 {
    let m = model.to_ascii_lowercase();
    match backend {
        "grok" => 256_000,
        "claude" => 200_000,
        "codex" if m.contains("codex") || m.contains("gpt-5") || m.contains("gpt-6") => 400_000,
        "codex" => 200_000,
        _ => 200_000,
    }
}

/// Uppercase with hair-space tracking (gpui has no letter-spacing), as
/// Zeron's menu headings do.
fn tracked_upper(label: &str) -> String {
    let mut out = String::new();
    for (i, ch) in label.to_uppercase().chars().enumerate() {
        if i > 0 {
            out.push('\u{200A}');
        }
        out.push(ch);
    }
    out
}

/// Read-only footer label under the pill: 12px icon + 12px medium text at
/// 60% muted (Zeron `footer_label`).
fn footer_label(icon: Lucide, text: String, ui: &Ui) -> AnyElement {
    let color = Ui::alpha(ui.text_muted, 0.6);
    div()
        .flex()
        .items_center()
        .h(px(20.))
        .max_w(px(160.))
        .min_w_0()
        .gap(px(6.))
        .px(px(8.))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .child(div().size(px(12.)).flex_shrink_0().child(Icon::from(icon)))
        .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(text))
        .into_any_element()
}
