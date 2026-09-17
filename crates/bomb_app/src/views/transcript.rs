//! The transcript: a centered column (max 880px). User prompts are
//! right-aligned bubbles; the agent's replies are plain markdown; thoughts and
//! tool calls fold into one "Thought · Ran 4 commands" disclosure line.

use bomb_core::services;
use bomb_core::transcript::{ApprovalCard, Body, Entry, PlanDoc, Role, ToolRow};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Icon, IconName, Sizable};
use std::sync::Arc;
use crate::models::app::AppModelHandle;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::models::thread::ThreadModel;
use crate::runtime::{services as svc, spawn_service};
use crate::theme::{Fuse, Layout, Ui};
use crate::views::motion::{breathe, fade_in};

const TAIL_SLACK: f32 = 120.0;

pub struct TranscriptView {
    thread: Entity<ThreadModel>,
    scroll: ScrollHandle,
    follow: bool,
    seen_tail: u64,
}

/// One folded activity item: a thought or a tool call.
#[derive(Clone)]
enum Activity {
    Thought {
        id: u64,
        state: Entity<TextViewState>,
        expanded: bool,
    },
    Tool {
        id: u64,
        row: ToolRow,
        expanded: bool,
    },
}

enum Row {
    User {
        id: u64,
        text: String,
        images: Vec<Arc<Image>>,
    },
    Agent {
        id: u64,
        state: Entity<TextViewState>,
        raw: String,
        streaming: bool,
        last: bool,
        at: String,
    },
    Activity {
        first_id: u64,
        items: Vec<Activity>,
        collapsed: bool,
    },
    Plan {
        id: u64,
        state: Entity<TextViewState>,
        doc: PlanDoc,
    },
    Approval {
        id: u64,
        card: ApprovalCard,
    },
    Line {
        id: u64,
        role: Role,
        text: String,
    },
}

impl TranscriptView {
    pub fn new(thread: Entity<ThreadModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&thread, |_, _, cx| cx.notify()).detach();
        Self {
            thread,
            scroll: ScrollHandle::new(),
            follow: true,
            seen_tail: 0,
        }
    }

    fn build_rows(&self, cx: &mut Context<Self>) -> Vec<Row> {
        self.thread.update(cx, |t, cx| {
            let entries: Vec<Entry> = t.thread.entries.clone();
            let last_agent = entries
                .iter()
                .rposition(|e| e.role == Role::Agent)
                .map(|i| entries[i].id);
            let mut rows: Vec<Row> = Vec::with_capacity(entries.len());
            let mut i = 0;
            while i < entries.len() {
                let e = &entries[i];
                let is_activity =
                    |e: &Entry| matches!((&e.role, &e.body), (Role::Tool, Body::Tool(_)) | (Role::Thought, Body::Text(_)));
                if is_activity(e) {
                    let first_id = e.id;
                    let mut items = Vec::new();
                    while i < entries.len() && is_activity(&entries[i]) {
                        let a = &entries[i];
                        match &a.body {
                            Body::Tool(r) => items.push(Activity::Tool {
                                id: a.id,
                                row: r.clone(),
                                expanded: t.expanded.contains(&a.id),
                            }),
                            Body::Text(s) => {
                                let state = t.markdown_state(a.id, s, cx);
                                items.push(Activity::Thought {
                                    id: a.id,
                                    state,
                                    expanded: t.expanded.contains(&a.id),
                                });
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                    let running = items
                        .iter()
                        .any(|a| matches!(a, Activity::Tool { row, .. } if !row.is_terminal()));
                    // Open while running; the user can override either way.
                    let collapsed = if t.collapsed_groups.contains(&first_id) {
                        true
                    } else if t.expanded.contains(&first_id) {
                        false
                    } else {
                        !running
                    };
                    rows.push(Row::Activity {
                        first_id,
                        items,
                        collapsed,
                    });
                    continue;
                }
                match (&e.role, &e.body) {
                    (Role::Agent, Body::Text(s)) => {
                        let state = t.markdown_state(e.id, s, cx);
                        rows.push(Row::Agent {
                            id: e.id,
                            state,
                            raw: s.clone(),
                            streaming: e.streaming,
                            last: last_agent == Some(e.id),
                            at: e.at.with_timezone(&chrono::Local).format("%b %-d, %-I:%M %p").to_string(),
                        });
                    }
                    (Role::You, Body::Text(s)) => {
                        let images = if e.images.is_empty() { Vec::new() } else { t.images_for(e.id) };
                        rows.push(Row::User {
                            id: e.id,
                            text: s.clone(),
                            images,
                        })
                    }
                    (Role::Plan, Body::Plan(doc)) => {
                        let state = t.markdown_state(e.id, &doc.markdown, cx);
                        rows.push(Row::Plan {
                            id: e.id,
                            state,
                            doc: doc.clone(),
                        });
                    }
                    (Role::Approval, Body::Approval(card)) => rows.push(Row::Approval {
                        id: e.id,
                        card: card.clone(),
                    }),
                    (role, Body::Text(s)) => rows.push(Row::Line {
                        id: e.id,
                        role: *role,
                        text: s.clone(),
                    }),
                    (role, other) => rows.push(Row::Line {
                        id: e.id,
                        role: *role,
                        text: format!("{other:?}"),
                    }),
                }
                i += 1;
            }
            rows
        })
    }

    fn near_bottom(&self) -> bool {
        let off = -self.scroll.offset().y;
        let max = self.scroll.max_offset().y;
        (max - off) <= px(TAIL_SLACK)
    }

    // ── row renderers ───────────────────────────────────────────────────

    fn user_row(&self, id: u64, text: &str, images: &[Arc<Image>], ui: &Ui) -> AnyElement {
        let bubble = div()
            .w_full()
            .flex()
            .flex_col()
            .items_end()
            .gap_2()
            .pt_4()
            .pb_2()
            .when(!images.is_empty(), |el| {
                el.child(div().flex().gap_2().justify_end().children(images.iter().map(|im| {
                    div()
                        .size(px(120.))
                        .rounded(px(10.))
                        .overflow_hidden()
                        .border_1()
                        .border_color(ui.border)
                        .child(img(im.clone()).size_full().object_fit(ObjectFit::Cover))
                })))
            })
            .child(
                div()
                    .min_w_0()
                    .max_w(relative(0.8))
                    .bg(ui.bubble)
                    .rounded(px(Layout::BUBBLE_RADIUS))
                    .px(px(16.))
                    .py(px(10.))
                    .text_sm()
                    .text_color(ui.text)
                    .whitespace_normal()
                    .child(text.to_string()),
            );
        fade_in(("user", id), bubble).into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn agent_row(
        &self,
        id: u64,
        state: &Entity<TextViewState>,
        raw: &str,
        streaming: bool,
        last: bool,
        at: &str,
        ui: &Ui,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let body = div()
            .flex()
            .flex_col()
            .gap_1()
            .py_1()
            .text_sm()
            .child(TextView::new(state).selectable(true))
            .when(streaming, |el| {
                el.child(breathe(
                    ("caret", id),
                    0.2,
                    div().w(px(6.)).h(px(13.)).rounded_sm().bg(ui.text),
                ))
            })
            .when(last && !streaming, |el| {
                let hover = ui.hover;
                let text_for_copy: SharedString = raw.to_string().into();
                let text_for_mem = text_for_copy.clone();
                let action = |id: &'static str, label: &'static str| {
                    div()
                        .id(id)
                        .px_1p5()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .child(label)
                };
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .pt_1()
                        .text_xs()
                        .text_color(ui.text_faint)
                        .child(at.to_string())
                        .child(action("copy-reply", "Copy").on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(text_for_copy.to_string()));
                        }))
                        .child(action("remember-reply", "Remember").on_click(move |_, _, cx| {
                            let app = cx.global::<AppModelHandle>().0.clone();
                            let t = text_for_mem.to_string();
                            app.update(cx, |m, cx| m.remember(t, cx));
                        })),
                )
            });
        fade_in(("agent", id), body).into_any_element()
    }

    fn activity_row(
        &self,
        first_id: u64,
        items: &[Activity],
        collapsed: bool,
        ui: &Ui,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let thread = self.thread.clone();
        let running = items
            .iter()
            .any(|a| matches!(a, Activity::Tool { row, .. } if !row.is_terminal()));
        let failed = items.iter().any(
            |a| matches!(a, Activity::Tool { row, .. } if row.status.contains("fail") || row.status.contains("denied")),
        );
        let summary = activity_summary(items);
        let hover = ui.hover;
        let color = if failed {
            ui.danger
        } else if running {
            ui.text
        } else {
            ui.text_muted
        };
        let mut header = div()
            .id(("act-head", first_id))
            .flex()
            .items_center()
            .gap_2()
            .h(px(28.))
            .px_1()
            .ml(px(-4.))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .on_click({
                let thread = thread.clone();
                move |_, _, cx| {
                    thread.update(cx, |t, cx| {
                        if t.collapsed_groups.remove(&first_id) {
                            t.expanded.insert(first_id);
                        } else if t.expanded.remove(&first_id) {
                            t.collapsed_groups.insert(first_id);
                        } else {
                            // Not overridden yet: flip whatever the default is.
                            let running = false;
                            let _ = running;
                            t.collapsed_groups.insert(first_id);
                        }
                        cx.notify();
                    })
                }
            })
            .child(
                div()
                    .size(px(14.))
                    .text_color(ui.text_faint)
                    .child(if collapsed {
                        IconName::ChevronRight
                    } else {
                        IconName::ChevronDown
                    }),
            )
            .child(div().text_sm().text_color(color).child(summary));
        if running {
            header = header.child(breathe(
                ("act-dot", first_id),
                0.3,
                div()
                    .size(px(6.))
                    .rounded_full()
                    .bg(crate::theme::c(Fuse::HOT)),
            ));
        }
        // Un-collapsing a collapsed-by-default group needs the header click to
        // land in `expanded`; fix the toggle so a collapsed group opens.
        let header = header.on_click({
            let thread = thread.clone();
            move |_, _, cx| {
                thread.update(cx, |t, cx| {
                    if collapsed {
                        t.collapsed_groups.remove(&first_id);
                        t.expanded.insert(first_id);
                    } else {
                        t.expanded.remove(&first_id);
                        t.collapsed_groups.insert(first_id);
                    }
                    cx.notify();
                })
            }
        });

        let chips: Vec<AnyElement> = if collapsed {
            Vec::new()
        } else {
            items
                .iter()
                .map(|a| match a {
                    Activity::Tool { id, row, expanded } => self.tool_chip(*id, row, *expanded, ui, cx),
                    Activity::Thought { id, state, expanded } => {
                        self.thought_chip(*id, state, *expanded, ui, cx)
                    }
                })
                .collect()
        };
        div()
            .flex()
            .flex_col()
            .py_1()
            .child(header)
            .when(!collapsed, |el| {
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .pl_2()
                        .pt_1()
                        .border_l_1()
                        .border_color(ui.border)
                        .ml(px(3.))
                        .children(chips),
                )
            })
            .into_any_element()
    }

    fn thought_chip(
        &self,
        id: u64,
        state: &Entity<TextViewState>,
        expanded: bool,
        ui: &Ui,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let thread = self.thread.clone();
        let hover = ui.hover;
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(("thought", id))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(26.))
                    .px_2()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(move |_, _, cx| thread.update(cx, |t, cx| t.toggle_expanded(id, cx)))
                    .child(
                        div()
                            .size(px(14.))
                            .text_color(ui.text_faint)
                            .child(Icon::from(Lucide::Brain)),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(ui.text_muted)
                            .child("Thought process"),
                    ),
            )
            .when(expanded, |el| {
                el.child(
                    div()
                        .pl_8()
                        .pr_2()
                        .pb_2()
                        .text_sm()
                        .child(TextView::new(state).selectable(true)),
                )
            })
            .into_any_element()
    }

    fn tool_chip(&self, id: u64, r: &ToolRow, expanded: bool, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let _ = cx;
        let thread = self.thread.clone();
        let mono = ui.mono.clone();
        let terminal = r.is_terminal();
        let failed = r.status.contains("fail") || r.status.contains("denied");
        let hover = ui.hover;
        let icon = tool_icon(&r.name);
        let glyph: AnyElement = if !terminal {
            breathe(
                ("chip-dot", id),
                0.3,
                div()
                    .size(px(14.))
                    .text_color(crate::theme::c(Fuse::HOT))
                    .child(Icon::from(icon)),
            )
            .into_any_element()
        } else {
            div()
                .size(px(14.))
                .text_color(if failed { ui.danger } else { ui.text_faint })
                .child(Icon::from(icon))
                .into_any_element()
        };
        let first_line = r.args.lines().next().unwrap_or("").trim().to_string();
        let head = div()
            .id(("chip", id))
            .flex()
            .items_center()
            .gap_2()
            .h(px(26.))
            .px_2()
            .rounded(px(6.))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .on_click(move |_, _, cx| thread.update(cx, |t, cx| t.toggle_expanded(id, cx)))
            .child(glyph)
            .child(
                div()
                    .text_sm()
                    .text_color(if failed { ui.danger } else { ui.text_muted })
                    .child(tool_label(&r.name)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .font_family(mono.clone())
                    .text_color(ui.text_faint)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(first_line),
            )
            .when(failed, |el| {
                el.child(div().text_xs().text_color(ui.danger).child(r.status.clone()))
            });
        let detail = expanded.then(|| {
            let mut blocks: Vec<AnyElement> = Vec::new();
            if !r.args.trim().is_empty() {
                blocks.push(if looks_like_diff(&r.args) {
                    diff_block(("args-diff", id), &r.args)
                } else {
                    mono_block(&r.args, &mono, ui.text_muted, ui)
                });
            }
            if let Some(res) = r.result.as_deref().filter(|s| !s.trim().is_empty()) {
                blocks.push(if looks_like_diff(res) {
                    diff_block(("res-diff", id), res)
                } else {
                    mono_block(res, &mono, ui.text, ui)
                });
            }
            div().flex().flex_col().gap_1().pl_8().pr_2().pb_2().children(blocks)
        });
        div()
            .flex()
            .flex_col()
            .child(head)
            .children(detail)
            .into_any_element()
    }

    fn plan_row(
        &self,
        id: u64,
        state: &Entity<TextViewState>,
        doc: &PlanDoc,
        ui: &Ui,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let app = cx.global::<AppModelHandle>().0.clone();
        let backends = app.read(cx).backends.clone();
        let code_it = Button::new(("code-it", id))
            .outline()
            .xsmall()
            .label("Code it")
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for b in &backends {
                    if !b.available {
                        continue;
                    }
                    let models: Vec<String> = if b.models.is_empty() { vec![b.default_model.clone()] } else { b.models.clone() };
                    for md in models {
                        let app = app.clone();
                        let bid = b.id.clone();
                        let mdl = md.clone();
                        menu = menu.item(PopupMenuItem::new(format!("{} · {md}", b.display_name)).on_click(move |_, _, cx| {
                            app.update(cx, |m, cx| m.code_plan_with(&bid, Some(mdl.clone()), cx));
                        }));
                    }
                }
                menu
            });
        let card = div()
            .flex()
            .flex_col()
            .gap_2()
            .my_2()
            .p_3()
            .rounded(px(Layout::PANEL_RADIUS))
            .border_1()
            .border_color(ui.border)
            .bg(ui.ink(0.03))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ui.text_muted)
                            .child(doc.title.clone().unwrap_or_else(|| "Plan".into())),
                    )
                    .child(div().flex_1())
                    .child(code_it),
            )
            .child(div().text_sm().child(TextView::new(state).selectable(true)));
        fade_in(("plan", id), card).into_any_element()
    }

    fn approval_row(&self, id: u64, card: &ApprovalCard, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let sid = self.thread.read(cx).id();
        let rid = card.request_id.clone();
        let open = card.is_open();
        let mono = ui.mono.clone();

        let mut buttons: Vec<AnyElement> = Vec::new();
        if open {
            let allow = card
                .options
                .iter()
                .find(|o| o.kind.contains("allow") && !o.kind.contains("always"))
                .map(|o| o.id.clone());
            let always = card
                .options
                .iter()
                .find(|o| o.kind.contains("allow_always"))
                .map(|o| o.id.clone());
            let deny = card
                .options
                .iter()
                .find(|o| o.kind.contains("reject") || o.kind.contains("deny"))
                .map(|o| o.id.clone());
            if let Some(opt) = allow.clone() {
                buttons.push(respond_button(("allow", id), "Allow", true, sid.clone(), rid.clone(), Some(opt), None));
            }
            match (always, card.allow_pattern.clone(), allow) {
                (Some(opt), _, _) => buttons.push(respond_button(
                    ("always", id), "Always allow", false, sid.clone(), rid.clone(), Some(opt), None,
                )),
                (None, Some(pattern), Some(opt)) => buttons.push(respond_button(
                    ("always", id), format!("Always allow {pattern}"), false, sid.clone(), rid.clone(), Some(opt), Some(pattern),
                )),
                _ => {}
            }
            buttons.push(respond_button(("deny", id), "Deny", false, sid.clone(), rid.clone(), deny, None));
        }

        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .my_2()
            .p_3()
            .rounded(px(Layout::PANEL_RADIUS))
            .border_1()
            .border_color(if open { ui.warning.opacity(0.5) } else { ui.border })
            .bg(if open { ui.warning.opacity(0.05) } else { ui.ink(0.02) })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if open { ui.warning } else { ui.text_muted })
                            .child(if open { "Needs your OK" } else { "Approval" }),
                    )
                    .child(div().text_xs().font_family(mono.clone()).text_color(ui.text_faint).child(card.tool.clone()))
                    .child(div().flex_1())
                    .when_some(card.resolution.clone(), |el, r| {
                        el.child(div().text_xs().text_color(ui.text_faint).child(r))
                    }),
            )
            .child(div().text_sm().font_family(mono).text_color(ui.text).whitespace_normal().child(card.summary.clone()))
            .when_some(card.explanation.clone(), |el, ex| {
                el.child(div().text_sm().text_color(ui.text_muted).child(ex))
            })
            .when(!buttons.is_empty(), |el| el.child(div().flex().gap_2().pt_1().children(buttons)));
        fade_in(("approval", id), body).into_any_element()
    }

    fn line_row(&self, id: u64, role: Role, text: &str, ui: &Ui) -> AnyElement {
        let color = match role {
            Role::Error => ui.danger,
            _ => ui.text_faint,
        };
        fade_in(
            ("line", id),
            div().py_1().text_xs().text_color(color).whitespace_normal().child(text.to_string()),
        )
        .into_any_element()
    }
}

impl Render for TranscriptView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::of(cx);
        let rows = self.build_rows(cx);
        let tail_version = self.thread.read(cx).tail_version;

        if !self.follow && self.near_bottom() {
            self.follow = true;
        }
        let should_pin = self.follow && (tail_version != self.seen_tail);
        self.seen_tail = tail_version;

        let count = rows.len();
        let children: Vec<AnyElement> = rows
            .iter()
            .map(|row| match row {
                Row::User { id, text, images } => self.user_row(*id, text, images, &ui),
                Row::Agent { id, state, raw, streaming, last, at } => {
                    self.agent_row(*id, state, raw, *streaming, *last, at, &ui, cx)
                }
                Row::Activity { first_id, items, collapsed } => {
                    self.activity_row(*first_id, items, *collapsed, &ui, cx)
                }
                Row::Plan { id, state, doc } => self.plan_row(*id, state, doc, &ui, cx),
                Row::Approval { id, card } => self.approval_row(*id, card, &ui, cx),
                Row::Line { id, role, text } => self.line_row(*id, *role, text, &ui),
            })
            .collect();

        if should_pin && count > 0 {
            self.scroll.scroll_to_item(count - 1);
        }

        div()
            .id("transcript")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                let dy = match ev.delta {
                    ScrollDelta::Pixels(p) => f32::from(p.y),
                    ScrollDelta::Lines(l) => l.y * 20.0,
                };
                if dy > 0.0 {
                    this.follow = false;
                    cx.notify();
                }
            }))
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .w_full()
                    .max_w(px(Layout::CONTENT_MAX))
                    .px_6()
                    .pt_8()
                    .pb_10()
                    .flex()
                    .flex_col()
                    .children(children)
                    .when(count == 0, |el| {
                        el.child(div().py_4().text_sm().text_color(ui.text_faint).child("Nothing here yet."))
                    }),
            )
    }
}

/// Unified-diff heuristics: hunk headers or +++/--- file markers.
fn looks_like_diff(text: &str) -> bool {
    let mut markers = 0;
    for l in text.lines().take(40) {
        if l.starts_with("@@ ") || l.starts_with("+++ ") || l.starts_with("--- ") || l.starts_with("diff --git") {
            markers += 1;
        }
    }
    markers >= 2
}

/// Render a diff through the markdown view so tree-sitter-diff highlights it.
fn diff_block(id: impl Into<ElementId>, text: &str) -> AnyElement {
    let md = format!("```diff\n{}\n```", text.trim_end());
    div()
        .text_xs()
        .max_h(px(400.))
        .overflow_hidden()
        .child(TextView::markdown(id, md).selectable(true))
        .into_any_element()
}

fn mono_block(text: &str, mono: &SharedString, color: Hsla, ui: &Ui) -> AnyElement {
    div()
        .p_2()
        .rounded(px(6.))
        .bg(ui.ink(0.04))
        .border_1()
        .border_color(ui.border)
        .text_xs()
        .font_family(mono.clone())
        .text_color(color)
        .whitespace_normal()
        .max_h(px(260.))
        .overflow_hidden()
        .child(text.to_string())
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn respond_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    primary: bool,
    session_id: String,
    request_id: String,
    option_id: Option<String>,
    add_rule: Option<String>,
) -> AnyElement {
    let mut b = Button::new(id).label(label).small().compact();
    b = if primary { b.primary() } else { b.outline() };
    b.on_click(move |_, _, cx| {
        let state = svc(cx);
        let sid = session_id.clone();
        let rid = request_id.clone();
        let opt = option_id.clone();
        let rule = add_rule.clone();
        spawn_service(
            cx,
            async move {
                if let Some(rule) = rule {
                    let _ = services::add_session_allow_rule(&state, sid.clone(), rule).await;
                }
                services::respond_approval(&state, sid, rid, opt).await
            },
            |res, _| {
                if let Err(e) = res {
                    tracing::warn!(error = %e, "approval response failed");
                }
            },
        );
    })
    .into_any_element()
}

fn tool_kind(name: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    if ["bash", "shell", "terminal", "exec", "command"].iter().any(|k| n.contains(k)) {
        "command"
    } else if ["edit", "write", "patch", "create", "apply"].iter().any(|k| n.contains(k)) {
        "edit"
    } else if ["read", "glob", "grep", "search", "ls", "list", "find", "cat", "view"].iter().any(|k| n.contains(k)) {
        "read"
    } else if ["fetch", "web", "http", "browse"].iter().any(|k| n.contains(k)) {
        "web"
    } else {
        "other"
    }
}

fn tool_icon(name: &str) -> Lucide {
    match tool_kind(name) {
        "command" => Lucide::Terminal,
        "edit" => Lucide::FilePen,
        "read" => Lucide::FileText,
        "web" => Lucide::Globe,
        _ => Lucide::Wrench,
    }
}

fn tool_label(name: &str) -> String {
    match tool_kind(name) {
        "command" => "Ran".into(),
        "edit" => "Edited".into(),
        "read" => "Read".into(),
        "web" => "Fetched".into(),
        _ => name.to_string(),
    }
}

/// "Thought · Ran 4 commands · Edited 1 file"
fn activity_summary(items: &[Activity]) -> String {
    let thoughts = items.iter().filter(|a| matches!(a, Activity::Thought { .. })).count();
    let rows: Vec<&ToolRow> = items
        .iter()
        .filter_map(|a| match a {
            Activity::Tool { row, .. } => Some(row),
            _ => None,
        })
        .collect();
    let base = tool_group_summary(rows.into_iter());
    match (base.is_empty(), thoughts) {
        (_, 0) => base,
        (true, 1) => "Thought".into(),
        (true, n) => format!("Thought {n} times"),
        (false, 1) => format!("Thought · {base}"),
        (false, n) => format!("Thought {n} times · {base}"),
    }
}

/// "Ran 2 commands · Edited 1 file · Read 3 files"
pub fn tool_group_summary<'a>(rows: impl Iterator<Item = &'a ToolRow>) -> String {
    let (mut cmd, mut edit, mut read, mut web, mut other) = (0, 0, 0, 0, 0);
    let mut last_name = String::new();
    for r in rows {
        last_name = r.name.clone();
        match tool_kind(&r.name) {
            "command" => cmd += 1,
            "edit" => edit += 1,
            "read" => read += 1,
            "web" => web += 1,
            _ => other += 1,
        }
    }
    let mut parts = Vec::new();
    if cmd > 0 {
        parts.push(format!("Ran {cmd} command{}", plural(cmd)));
    }
    if edit > 0 {
        parts.push(format!("Edited {edit} file{}", plural(edit)));
    }
    if read > 0 {
        parts.push(format!("Read {read} file{}", plural(read)));
    }
    if web > 0 {
        parts.push(format!("Fetched {web} page{}", plural(web)));
    }
    if other > 0 {
        if parts.is_empty() && other == 1 {
            parts.push(last_name);
        } else {
            parts.push(format!("{other} other tool call{}", plural(other)));
        }
    }
    parts.join(" · ")
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    // No glob import: `gpui_kit::*` carries a `test` macro that shadows `#[test]`.
    use super::{tool_group_summary, ToolRow};

    fn row(name: &str) -> ToolRow {
        ToolRow {
            tool_id: name.into(),
            name: name.into(),
            status: "completed".into(),
            args: String::new(),
            result: None,
        }
    }

    #[test]
    fn summarises_groups() {
        let rows = [row("Bash"), row("Bash"), row("Edit"), row("Read"), row("Sparkle")];
        assert_eq!(
            tool_group_summary(rows.iter()),
            "Ran 2 commands · Edited 1 file · Read 1 file · 1 other tool call"
        );
        let one = [row("Sparkle")];
        assert_eq!(tool_group_summary(one.iter()), "Sparkle");
    }
}
