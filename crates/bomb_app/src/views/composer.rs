//! The composer: a pill with a growing textarea, attachment tray, model and
//! mode pickers, attach, and a send button that morphs into stop while a
//! turn runs. Enter sends, Shift+Enter inserts a newline, Shift+Tab cycles
//! the approval mode.

use std::sync::Arc;

use base64::Engine;
use bomb_core::services::ImageInput;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Icon, Sizable};
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
        Self {
            model,
            input,
            attachments: Vec::new(),
            drag_over: false,
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| s.focus(window, cx));
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

    fn model_picker(&self, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let m = self.model.read(cx);
        let backend = m.prefs.backend.clone();
        let model = m.effective_model();
        let backends = m.backends.clone();
        let app = self.model.clone();
        let label = if model.is_empty() { backend.clone() } else { model };
        let dot = ui.backend(&backend);
        Button::new("model-picker")
            .ghost()
            .small()
            .compact()
            .label(label)
            .icon(Icon::empty())
            .dropdown_menu(move |mut menu, _, _| {
                for b in &backends {
                    menu = menu.label(format!(
                        "{}{}",
                        b.display_name,
                        if b.available { "" } else { " · unavailable" }
                    ));
                    let models: Vec<String> = if b.models.is_empty() {
                        vec![b.default_model.clone()]
                    } else {
                        b.models.clone()
                    };
                    for md in models {
                        let app = app.clone();
                        let bid = b.id.clone();
                        let mdl = md.clone();
                        let item = PopupMenuItem::new(md.clone())
                            .disabled(!b.available)
                            .on_click(move |_, _, cx| {
                                app.update(cx, |m, cx| m.set_backend(&bid, Some(mdl.clone()), cx));
                            });
                        menu = menu.item(item);
                    }
                    menu = menu.separator();
                }
                menu
            })
            .map(|el| {
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().size(px(7.)).rounded_full().bg(dot))
                    .child(el)
                    .into_any_element()
            })
    }

    fn mcp_picker(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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
                .small()
                .compact()
                .label(label)
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
        let _ = ui;
        Button::new("mode-picker")
            .ghost()
            .small()
            .compact()
            .label(mode)
            .dropdown_menu(move |mut menu, _, _| {
                for m in APPROVAL_CYCLE {
                    let app = app.clone();
                    let desc = match m {
                        "plan" => "plan · investigate and propose, no execution",
                        "ask" => "ask · confirm every tool call",
                        "auto" => "auto · approve safe reads, edits and commands",
                        _ => "yolo · approve everything (deny rules still apply)",
                    };
                    menu = menu.item(PopupMenuItem::new(desc).on_click(move |_, _, cx| {
                        app.update(cx, |a, cx| a.set_mode(m, cx));
                    }));
                }
                menu
            })
            .into_any_element()
    }
}

impl Render for ComposerView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::of(cx);
        let (busy, starting, branch, ctx_tokens, worktree_on, has_thread) = {
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
            let ctx = t
                .as_ref()
                .and_then(|t| t.read(cx).thread.context_tokens)
                .map(|n| format!("ctx {}", bomb_core::presence::format_count(n as usize)));
            (busy, m.starting, branch, ctx, m.prefs.worktree, t.is_some())
        };
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
                .child(div().size(px(11.)).rounded(px(3.)).bg(ui.bg))
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

        let drag_border = if self.drag_over { ui.accent } else { ui.border };
        let tray = self.tray(&ui, cx);
        let model_picker = self.model_picker(&ui, cx);
        let mode_picker = self.mode_picker(&ui, cx);
        let mcp_picker = self.mcp_picker(cx);
        let hover = ui.hover;
        let _ = danger;

        div()
            .id("composer")
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .px_6()
            .pb_4()
            .on_action(cx.listener(|_, _: &CycleApprovalMode, _, cx| {
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                let k = &ev.keystroke;
                if k.modifiers.platform && k.key == "v" && this.paste_from_clipboard(cx) {
                    cx.stop_propagation();
                }
                if k.modifiers.shift && k.key == "tab" {
                    this.model.update(cx, |m, cx| m.cycle_mode(cx));
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .w_full()
                    .max_w(px(Layout::COMPOSER_MAX))
                    .flex()
                    .flex_col()
                    .gap_2()
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
                            .when(!ui.dark, |el| el.shadow_sm())
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
                                    .gap_2()
                                    .pl_5()
                                    .pr_3()
                                    .py_3()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(14.))
                                            .child(Textarea::new(&self.input).appearance(false).bordered(false)),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_0p5()
                                            .pb_0p5()
                                            .child(model_picker)
                                            .child(mode_picker)
                                            .children(mcp_picker)
                                            .child(
                                                div()
                                                    .id("attach")
                                                    .size(px(26.))
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .rounded(px(6.))
                                                    .text_color(ui.text_muted)
                                                    .cursor_pointer()
                                                    .hover(move |s| s.bg(hover))
                                                    .on_click(cx.listener(|this, _, _, cx| this.pick_files(cx)))
                                                    .child(div().size(px(15.)).child(Icon::from(Lucide::Paperclip))),
                                            )
                                            .child(send_button),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .px_2()
                            .text_xs()
                            .text_color(ui.text_faint)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .child(div().size(px(11.)).child(Icon::from(Lucide::Folder)))
                                    .child(if has_thread { "Local checkout" } else { "New thread" }),
                            )
                            .when_some(branch, |el, b| {
                                el.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(div().size(px(11.)).child(Icon::from(Lucide::GitBranch)))
                                        .child(b),
                                )
                            })
                            .when(!has_thread, |el| {
                                el.child(
                                    div()
                                        .id("worktree-toggle")
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .cursor_pointer()
                                        .text_color(if worktree_on { ui.text_muted } else { ui.text_faint })
                                        .on_click(move |_, _, cx| {
                                            app.update(cx, |m, cx| {
                                                m.prefs.worktree = !m.prefs.worktree;
                                                cx.notify();
                                            })
                                        })
                                        .child(div().size(px(11.)).child(Icon::from(Lucide::GitBranch)))
                                        .child(if worktree_on { "isolated worktree" } else { "shared checkout" }),
                                )
                            })
                            .child(div().flex_1())
                            .when_some(ctx_tokens, |el, c| el.child(c))
                            .when(starting, |el| el.child("starting agent…")),
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
