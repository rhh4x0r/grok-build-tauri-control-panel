//! The transcript: a centered column (max 880px). User prompts are
//! right-aligned bubbles; the agent's replies are plain markdown; thoughts and
//! tool calls fold into one "Thought · Ran 4 commands" disclosure line.

use crate::views::button::Button;
use crate::models::app::AppModelHandle;
use bomb_core::services;
use bomb_core::transcript::{ApprovalCard, Body, Entry, PlanDoc, Role, ToolRow};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenuItem};
use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::component::{Icon, IconName, Sizable};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::sync::Arc;

use crate::models::thread::ThreadModel;
use crate::runtime::{services as svc, spawn_service};
use crate::theme::{Layout, Ui};
use crate::views::motion::{breathe, fade_in};

const TAIL_SLACK: f32 = 120.0;

pub struct TranscriptView {
    thread: Entity<ThreadModel>,
    scroll: ScrollHandle,
    follow: bool,
    seen_tail: u64,
    /// Frames left to keep forcing the bottom: freshly hydrated rows have no
    /// measured bounds yet, so one `scroll_to_item` lands short.
    pin_frames: u8,
    /// Find-in-conversation: query and the active hit (index into `matches`).
    search: Option<(String, usize)>,
    matches: Vec<usize>,
    scrolled_to: Option<usize>,
    stage_expanded: std::collections::HashSet<u64>,
    technical_expanded: std::collections::HashSet<u64>,
}

impl TranscriptView {
    pub fn set_search(&mut self, query: Option<String>, cx: &mut Context<Self>) {
        self.search = query
            .filter(|q| !q.trim().is_empty())
            .map(|q| (q.to_lowercase(), 0));
        self.scrolled_to = None;
        cx.notify();
    }

    pub fn step_search(&mut self, delta: i32, cx: &mut Context<Self>) {
        if let Some((_, ix)) = &mut self.search {
            let n = self.matches.len();
            if n > 0 {
                *ix = ((*ix as i32 + delta).rem_euclid(n as i32)) as usize;
            }
            self.scrolled_to = None;
            cx.notify();
        }
    }

    pub fn search_status(&self) -> Option<(usize, usize)> {
        self.search.as_ref().map(|(_, ix)| {
            (
                if self.matches.is_empty() { 0 } else { ix + 1 },
                self.matches.len(),
            )
        })
    }
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
        /// Local image files the reply refers to, resolved against the cwd.
        images: Vec<std::path::PathBuf>,
        /// Images the agent/tool returned inline (decoded).
        attached: Vec<Arc<Image>>,
    },
    GeneratingImage {
        id: u64,
        args: String,
        running: bool,
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
    /// Several answered requests in a row, folded into one line.
    AnsweredApprovals {
        first_id: u64,
        cards: Vec<(u64, ApprovalCard)>,
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
            seen_tail: u64::MAX,
            pin_frames: 0,
            search: None,
            matches: Vec::new(),
            scrolled_to: None,
            stage_expanded: Default::default(),
            technical_expanded: Default::default(),
        }
    }

    fn build_rows(&self, cx: &mut Context<Self>) -> Vec<Row> {
        self.thread.update(cx, |t, cx| {
            let entries: Vec<Entry> = t.thread.entries.clone();
            let cwd = std::path::PathBuf::from(&t.meta.cwd);
            let project_root = t.meta.project_root.clone().map(std::path::PathBuf::from);
            let last_agent = entries
                .iter()
                .rposition(|e| e.role == Role::Agent && e.images.is_empty())
                .map(|i| entries[i].id);
            let mut rows: Vec<Row> = Vec::with_capacity(entries.len());
            let turn_start = entries
                .iter()
                .rposition(|e| e.role == Role::You)
                .unwrap_or(0);
            let mut image_aliases = std::collections::HashMap::new();
            let mut i = 0;
            while i < entries.len() {
                let e = &entries[i];
                let is_activity = |e: &Entry| {
                    matches!(
                        (&e.role, &e.body),
                        (Role::Tool, Body::Tool(_)) | (Role::Thought, Body::Text(_))
                    )
                };
                if is_activity(e) {
                    let first_id = e.id;
                    let mut items = Vec::new();
                    while i < entries.len() && is_activity(&entries[i]) {
                        let a = &entries[i];
                        match &a.body {
                            Body::Tool(r) => {
                                if let Some((alias, path)) = generated_image_alias(r) {
                                    image_aliases.insert(alias, path);
                                }
                                items.push(Activity::Tool {
                                    id: a.id,
                                    row: r.clone(),
                                    expanded: t.expanded.contains(&a.id),
                                });
                            }
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
                    let failed = items.iter().any(|a| matches!(a, Activity::Tool { row, .. } if row.status.contains("fail") || row.status.contains("denied")));
                    // Closed by default, running or not: the header says what is happening. A group with a
                    // failed step opens itself, because that is the one worth reading. The user can override.
                    let _ = running;
                    let collapsed = if t.collapsed_groups.contains(&first_id) {
                        true
                    } else if t.expanded.contains(&first_id) {
                        false
                    } else {
                        !failed
                    };
                    let image_rows: Vec<Row> = if t.meta.live && i > turn_start {
                        items
                            .iter()
                            .filter_map(|item| match item {
                                Activity::Tool { id, row, .. } if generating_image(row) => {
                                    Some(Row::GeneratingImage {
                                        id: *id,
                                        args: row.args.clone(),
                                        running: row.status != "pending",
                                    })
                                }
                                _ => None,
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    rows.push(Row::Activity {
                        first_id,
                        items,
                        collapsed,
                    });
                    rows.extend(image_rows);
                    continue;
                }
                match (&e.role, &e.body) {
                    (Role::Agent, Body::Text(s)) => {
                        let state = t.markdown_state(e.id, bomb_foundry::presentation::stage_prose(s), cx);
                        let images = if e.streaming {
                            Vec::new()
                        } else {
                            local_images(s, &cwd, project_root.as_deref(), &image_aliases)
                        };
                        let attached = if e.images.is_empty() {
                            Vec::new()
                        } else {
                            t.images_for(e.id)
                        };
                        rows.push(Row::Agent {
                            id: e.id,
                            state,
                            raw: s.clone(),
                            streaming: e.streaming,
                            images,
                            attached,
                            last: last_agent == Some(e.id),
                            at: e
                                .at
                                .with_timezone(&chrono::Local)
                                .format("%b %-d, %-I:%M %p")
                                .to_string(),
                        });
                    }
                    (Role::You, Body::Text(s)) => {
                        let images = if e.images.is_empty() {
                            Vec::new()
                        } else {
                            t.images_for(e.id)
                        };
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
                    (Role::Approval, Body::Approval(card)) => {
                        // Answered requests next to each other share one line; a waiting one always stands alone.
                        let answered = !card.is_open() && !card.plan_approval;
                        match rows.last_mut() {
                            Some(Row::AnsweredApprovals { cards, .. }) if answered => cards.push((e.id, card.clone())),
                            Some(Row::Approval { id, card: previous }) if answered && !previous.is_open() && !previous.plan_approval => {
                                let group = Row::AnsweredApprovals { first_id: *id, cards: vec![(*id, previous.clone()), (e.id, card.clone())] };
                                *rows.last_mut().expect("just matched") = group;
                            }
                            _ => rows.push(Row::Approval { id: e.id, card: card.clone() }),
                        }
                    }
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
        // The agent is told where attached files are; the person just sees that they are attached.
        let (text, files) = split_attached_files(text);
        let hover = ui.hover;
        let bubble = div()
            .w_full()
            .flex()
            .flex_col()
            .items_end()
            .gap_2()
            .pt_4()
            .pb_2()
            .when(!images.is_empty(), |el| {
                el.child(div().flex().gap_2().justify_end().children(
                    images.iter().enumerate().map(|(ix, im)| {
                        div()
                            .id(("user-image", id * 64 + ix as u64))
                            .context_menu({
                                let source = super::image_actions::Source::Attachment(im.clone());
                                move |menu, _, _| super::image_actions::menu(menu, source.clone())
                            })
                            .size(px(120.))
                            .rounded(px(10.))
                            .overflow_hidden()
                            .border_1()
                            .border_color(ui.border)
                            .child(img(im.clone()).size_full().object_fit(ObjectFit::Cover))
                    }),
                ))
            })
            .when(!files.is_empty(), |el| {
                el.child(div().flex().flex_wrap().gap_2().justify_end().max_w(relative(0.8)).children(files.iter().enumerate().map(|(ix, (path, size))| {
                    let target = path.clone();
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
                    let full = path.display().to_string();
                    div()
                        .id(("user-file", id * 64 + ix as u64))
                        .flex()
                        .items_center()
                        .gap_2()
                        .h(px(36.))
                        .max_w(px(280.))
                        .px_3()
                        .rounded(px(10.))
                        .border_1()
                        .border_color(ui.border)
                        .bg(ui.ink(0.03))
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(format!("{full} · click to show in Finder")).build(window, cx))
                        .on_click(move |_, _, _| reveal_in_finder(&target))
                        .child(Icon::from(Lucide::File).size(px(15.)).text_color(ui.text_muted))
                        .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(crate::theme::Type::SMALL)).child(name))
                        .child(div().flex_shrink_0().text_size(px(crate::theme::Type::CAPTION)).text_color(ui.text_faint).child(size.clone()))
                })))
            })
            .when(!text.is_empty(), |el| {
                el.child(
                    div()
                        .min_w_0()
                        .max_w(relative(0.8))
                        .bg(ui.bubble)
                        .rounded(px(Layout::BUBBLE_RADIUS))
                        .px(px(16.))
                        .py(px(10.))
                        .text_size(px(Layout::BODY_SIZE))
                        .line_height(px(Layout::BODY_LINE))
                        .text_color(ui.text)
                        .whitespace_normal()
                        .child(text.to_string()),
                )
            });
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
        images: &[std::path::PathBuf],
        attached: &[Arc<Image>],
        ui: &Ui,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cwd = std::path::PathBuf::from(&self.thread.read(cx).meta.cwd);
        let link_cwd = cwd.clone();
        let body_text: AnyElement = if streaming {
            streaming_text(id, bomb_foundry::presentation::stage_prose(raw), ui)
        } else {
            TextView::new(state)
                .selectable(true)
                .code_block_actions(|block, _, _| copy_code_button(block.code()))
                .on_link_click(move |href, _, _, cx| {
                    open_link(href, &link_cwd, cx);
                })
                .into_any_element()
        };
        let body = div()
            .flex()
            .flex_col()
            .gap_1()
            .py_1()
            .text_size(px(Layout::BODY_SIZE))
            .line_height(px(Layout::BODY_LINE))
            // The kit lays out a line with inline code as separate boxes sized to the shaped text, then
            // wraps each box again by summing single-character widths. With kerning on, the sum is a hair
            // wider than the box, so a word drops onto the next line and overlaps it. No kerning, no gap.
            .font_features(prose_font_features())
            .when(!raw.trim().is_empty() || streaming, |el| {
                el.child(body_text)
            })
            .when(!attached.is_empty(), |el| {
                el.child(div().flex().flex_wrap().gap_2().py_2().children(
                    attached.iter().enumerate().map(|(ix, im)| {
                        let im = im.clone();
                        div()
                            .id(("att-img", id * 64 + ix as u64))
                            .context_menu({
                                let source = super::image_actions::Source::Attachment(im.clone());
                                move |menu, _, _| super::image_actions::menu(menu, source.clone())
                            })
                            .max_w(px(420.))
                            .rounded(px(10.))
                            .overflow_hidden()
                            .border_1()
                            .border_color(ui.border)
                            .child(crate::views::motion::reveal(
                                ("att-reveal", id * 64 + ix as u64),
                                img(im)
                                    .max_w(px(420.))
                                    .max_h(px(420.))
                                    .object_fit(ObjectFit::Contain),
                            ))
                    }),
                ))
            })
            .when(!images.is_empty(), |el| {
                el.child(div().flex().flex_wrap().gap_2().py_2().children(
                    images.iter().enumerate().map(|(ix, p)| {
                        let path = p.clone();
                        div()
                            .id(("gen-img", id * 64 + ix as u64))
                            .max_w(px(420.))
                            .rounded(px(10.))
                            .overflow_hidden()
                            .border_1()
                            .border_color(ui.border)
                            .cursor_pointer()
                            .on_click(move |_, _, _| open_path(&path))
                            .context_menu({
                                let source = super::image_actions::Source::File(p.clone());
                                move |menu, _, _| super::image_actions::menu(menu, source.clone())
                            })
                            .child(crate::views::motion::reveal(
                                ("reveal", id * 64 + ix as u64),
                                img(p.clone())
                                    .max_w(px(420.))
                                    .max_h(px(420.))
                                    .object_fit(ObjectFit::Contain),
                            ))
                    }),
                ))
            })
            .when(streaming, |el| {
                el.child(breathe(
                    ("caret", id),
                    0.2,
                    div().w(px(6.)).h(px(13.)).rounded_sm().bg(ui.text),
                ))
            })
            .when(last && !streaming, |el| {
                let hover = ui.hover;
                let text_for_copy: SharedString = bomb_foundry::presentation::stage_prose(raw).to_string().into();
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
                        .text_size(px(crate::theme::Type::SMALL))
                        .text_color(ui.text_faint)
                        .child(at.to_string())
                        .child(action("copy-reply", "Copy").on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                text_for_copy.to_string(),
                            ));
                        }))
                        .child(
                            action("remember-reply", "Remember").on_click(move |_, _, cx| {
                                let app = cx.global::<AppModelHandle>().0.clone();
                                let t = text_for_mem.to_string();
                                app.update(cx, |m, cx| m.remember(t, cx));
                            }),
                        ),
                )
            });
        if raw.contains("<foundry-result>") && !streaming {
            let prose = bomb_foundry::presentation::stage_prose(raw);
            let summary: String = prose.lines().find(|line| !line.trim().is_empty()).unwrap_or("Stage finished").trim_matches('*').chars().take(90).collect();
            let stage_result = raw.split_once("<foundry-result>").and_then(|(_,tail)|tail.split_once("</foundry-result>")).and_then(|(json,_)|serde_json::from_str::<bomb_foundry::StageResult>(json).ok());
            let run=svc(cx).foundry.for_thread(&self.thread.read(cx).meta.id);
            let attempt=run.as_ref().and_then(|run|run.attempts.iter().find(|a|a.result.as_ref().zip(stage_result.as_ref()).is_some_and(|(a,b)|a.summary==b.summary)));
            let meta=attempt.map(|a| {
                let seconds=chrono::DateTime::parse_from_rfc3339(&a.started_at).ok().zip(a.finished_at.as_deref().and_then(|s|chrono::DateTime::parse_from_rfc3339(s).ok())).map(|(s,e)|(e-s).num_seconds().max(0)).unwrap_or(0);
                div().flex().items_center().gap_2().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted)
                    .child(super::brand::brand_mark(&a.backend,13.,true,ui))
                    .child(format!("{} · {}m {}s{}",a.model,seconds/60,seconds%60,if a.invalidated {" · Superseded by revision"}else{""}))
            });
            let expanded=self.stage_expanded.contains(&id);
            let technical=self.technical_expanded.contains(&id);
            return div().my_2().p_3().rounded_lg().border_1().border_color(ui.border).bg(ui.glass).flex().flex_col().gap_2()
                .child(Button::new(("stage-summary",id)).ghost().small().icon(if expanded {Lucide::ChevronDown}else{Lucide::ChevronRight}).label(summary)
                    .on_click(cx.listener(move|v,_,_,cx|{if !v.stage_expanded.remove(&id){v.stage_expanded.insert(id);}cx.notify();})))
                .when_some(meta,|el,meta|el.child(meta))
                .when(expanded,|el|el.child(body))
                .child(Button::new(("stage-technical",id)).ghost().small().label(if technical {"Hide technical details"}else{"Technical details"})
                    .on_click(cx.listener(move|v,_,_,cx|{if !v.technical_expanded.remove(&id){v.technical_expanded.insert(id);}cx.notify();})))
                .when(technical,|el|el.child(div().id(("stage-raw",id)).max_h(px(240.)).overflow_y_scroll().child(TextView::markdown(("stage-raw-text",id),format!("```text\n{raw}\n```")).selectable(true))))
                .into_any_element();
        }
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
            .cursor_pointer()
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
            ;
        if running {
            // What it is doing right now, with a breathing dot so a closed group still reads as alive.
            let steps = items.iter().filter(|a| matches!(a, Activity::Tool { .. })).count();
            let current = items.iter().rev().find_map(|a| match a { Activity::Tool { row, .. } if !row.is_terminal() => Some(row), _ => None });
            let doing = current.map(running_label).unwrap_or_else(|| "Working".into());
            header = header
                .child(crate::views::motion::breathe(("act-live", first_id), 0.3, div().size(px(7.)).flex_shrink_0().rounded_full().bg(ui.accent)))
                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_sm().text_color(ui.text).child(doing))
                .when(steps > 1, |el| el.child(div().flex_shrink_0().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child(format!("{steps} steps"))));
        } else {
            header = header.child(div().text_sm().text_color(color).child(summary));
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
                    Activity::Tool { id, row, expanded } => {
                        self.tool_chip(*id, row, *expanded, ui, cx)
                    }
                    Activity::Thought {
                        id,
                        state,
                        expanded,
                    } => self.thought_chip(*id, state, *expanded, ui, cx),
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
                        .relative()
                        .flex()
                        .flex_col()
                        .pl(px(22.))
                        .pt_1()
                        .ml(px(6.))
                        .child(
                            // the rail: one hairline the ticks branch off
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom(px(13.))
                                .w(px(1.))
                                .bg(ui.border),
                        )
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
                    .relative()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(26.))
                    .px_1()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(move |_, _, cx| thread.update(cx, |t, cx| t.toggle_expanded(id, cx)))
                    .child(rail_tick(ui))
                    .child(
                        div()
                            .size(px(14.))
                            .text_color(ui.text_faint)
                            .child(Icon::from(Lucide::MessageSquare)),
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

    fn tool_chip(
        &self,
        id: u64,
        r: &ToolRow,
        expanded: bool,
        ui: &Ui,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _ = cx;
        let thread = self.thread.clone();
        let mono = ui.mono.clone();
        let terminal = r.is_terminal();
        let failed = r.status.contains("fail") || r.status.contains("denied");
        let hover = ui.hover;
        let icon = tool_icon(&r.name);
        let glyph: AnyElement = div()
            .size(px(14.))
            .text_color(if failed {
                ui.danger
            } else if terminal {
                ui.text_faint
            } else {
                ui.text_muted
            })
            .child(Icon::from(icon))
            .into_any_element();
        let first_line = r.args.lines().next().unwrap_or("").trim().to_string();
        let head = div()
            .id(("chip", id))
            .min_w_0().overflow_hidden()
            .relative()
            .flex()
            .items_center()
            .gap_2()
            .h(px(26.))
            .px_1()
            .rounded(px(6.))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .on_click(move |_, _, cx| thread.update(cx, |t, cx| t.toggle_expanded(id, cx)))
            .child(rail_tick(ui))
            .child(glyph)
            .child(
                div()
                    .text_size(px(crate::theme::Type::BODY))
                    .text_color(if failed { ui.danger } else { ui.text_muted })
                    .child(tool_label(&r.name)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(crate::theme::Type::BODY))
                    .text_color(ui.text_faint)
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(strip_arg_prefix(&first_line)),
            )
            .when(failed, |el| {
                el.child(
                    div()
                        .text_size(px(crate::theme::Type::SMALL))
                        .text_color(ui.danger)
                        .child(r.status.clone()),
                )
            });
        let kind = tool_kind(&r.name);
        let detail = expanded.then(|| {
            let mut blocks: Vec<AnyElement> = Vec::new();
            if kind == "command" {
                blocks.push(terminal_block(
                    id,
                    &first_line,
                    r.result.as_deref().unwrap_or(""),
                    terminal,
                    failed,
                    ui,
                ));
                return div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pl_8()
                    .pr_2()
                    .pb_2()
                    .children(blocks);
            }
            if !r.args.trim().is_empty() {
                blocks.push(if looks_like_diff(&r.args) {
                    diff_block(("args-diff", id), &r.args)
                } else {
                    mono_block(("tool-args-scroll",id), &r.args, &mono, ui.text_muted, ui)
                });
            }
            if let Some(res) = r.result.as_deref().filter(|s| !s.trim().is_empty()) {
                blocks.push(if looks_like_diff(res) {
                    diff_block(("res-diff", id), res)
                } else {
                    mono_block(("tool-result-scroll",id),res, &mono, ui.text, ui)
                });
            }
            div()
                .flex()
                .flex_col()
                .gap_1()
                .pl_8()
                .pr_2()
                .pb_2()
                .children(blocks)
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
            .small()
            .label("Code it")
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for b in &backends {
                    if !b.available {
                        continue;
                    }
                    let models = b.models.clone();
                    for md in models {
                        let app = app.clone();
                        let bid = b.id.clone();
                        let mdl = md.clone();
                        menu = menu.item(
                            PopupMenuItem::new(format!("{} · {md}", b.display_name)).on_click(
                                move |_, _, cx| {
                                    app.update(cx, |m, cx| {
                                        m.code_plan_with(&bid, Some(mdl.clone()), cx)
                                    });
                                },
                            ),
                        );
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
                            .text_size(px(crate::theme::Type::SMALL))
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

    fn approval_row(
        &self,
        id: u64,
        card: &ApprovalCard,
        ui: &Ui,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Once answered, a request is history: one quiet line that opens on demand.
        if !card.is_open() && !card.plan_approval {
            return self.answered_row(id, card, ui, cx);
        }
        let sid = self.thread.read(cx).id();
        let rid = card.request_id.clone();
        let open = card.is_open();
        let mono = ui.mono.clone();
        let expanded = self.thread.read(cx).expanded.contains(&id);
        let thread = self.thread.clone();
        let summary = approval_summary(&card.summary, true);

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
                buttons.push(respond_button(
                    ("allow", id),
                    "Allow",
                    true,
                    sid.clone(),
                    rid.clone(),
                    Some(opt),
                    None,
                ));
            }
            match (always, card.allow_pattern.clone(), allow) {
                (Some(opt), _, _) => buttons.push(respond_button(
                    ("always", id),
                    "Always allow",
                    false,
                    sid.clone(),
                    rid.clone(),
                    Some(opt),
                    None,
                )),
                (None, Some(pattern), Some(opt)) => buttons.push(respond_button(
                    ("always", id),
                    format!("Always allow {pattern}"),
                    false,
                    sid.clone(),
                    rid.clone(),
                    Some(opt),
                    Some(pattern),
                )),
                _ => {}
            }
            buttons.push(respond_button(
                ("deny", id),
                "Deny",
                false,
                sid.clone(),
                rid.clone(),
                deny,
                None,
            ));
        }

        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .my_2()
            .p_3()
            .rounded(px(Layout::PANEL_RADIUS))
            .border_1()
            .border_color(if open {
                ui.warning.opacity(0.5)
            } else {
                ui.border
            })
            .bg(if open {
                ui.warning.opacity(0.05)
            } else {
                ui.ink(0.02)
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(crate::theme::Type::SMALL))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if open { ui.warning } else { ui.text_muted })
                            .child(if open {
                                "Permission requested"
                            } else {
                                "Permission request"
                            }),
                    )
                    .child(
                        Icon::from(Lucide::Shield)
                            .size(px(14.))
                            .text_color(ui.text_muted),
                    )
                    .child(
                        div()
                            .text_size(px(crate::theme::Type::SMALL))
                            .font_family(mono.clone())
                            .text_color(ui.text_faint)
                            .child(
                                card.tool
                                    .split_whitespace()
                                    .next()
                                    .unwrap_or("Tool")
                                    .to_string(),
                            ),
                    )
                    .child(div().flex_1())
                    .when_some(
                        card.resolution.clone().filter(|r| r != "restored"),
                        |el, r| {
                            el.child(
                                div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child(
                                    card.options
                                        .iter()
                                        .find(|o| o.id == r)
                                        .map(|o| o.label.clone())
                                        .unwrap_or(r),
                                ),
                            )
                        },
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .font_family(mono)
                    .text_color(ui.text)
                    .whitespace_normal()
                    .child(summary),
            )
            .child(
                Button::new(("approval-details", id))
                    .ghost()
                    .small()
                    .label(if expanded { "Hide details" } else { "Details" })
                    .on_click(move |_, _, cx| thread.update(cx, |t, cx| t.toggle_expanded(id, cx))),
            )
            .when(expanded, |el| {
                el.child(
                    div()
                        .text_size(px(crate::theme::Type::SMALL))
                        .font_family(ui.mono.clone())
                        .text_color(ui.text_muted)
                        .whitespace_normal()
                        .child(card.summary.clone()),
                )
            })
            .when_some(card.explanation.clone(), |el, ex| {
                el.child(div().text_sm().text_color(ui.text_muted).child(ex))
            })
            .when(!buttons.is_empty(), |el| {
                el.child(div().flex().gap_2().pt_1().children(buttons))
            });
        fade_in(("approval", id), body).into_any_element()
    }

    /// An answered request: shield, outcome, the command on one line, and a chevron for the rest.
    fn answered_row(&self, id: u64, card: &ApprovalCard, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let expanded = self.thread.read(cx).expanded.contains(&id);
        let thread = self.thread.clone();
        let resolution = card.resolution.clone().unwrap_or_default();
        let kind = card.options.iter().find(|o| o.id == resolution).map(|o| o.kind.clone());
        let outcome = bomb_core::transcript::approval_outcome_label(&resolution, kind.as_deref());
        let refused = matches!(outcome, Some("Denied" | "Cancelled"));
        let summary = approval_summary(&card.summary, false);
        let first_line = summary.lines().next().unwrap_or_default().to_string();
        let hover = ui.hover;
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(("approval-line", id))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(28.))
                    .px_1()
                    .ml(px(-4.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .on_click(move |_, _, cx| thread.update(cx, |t, cx| t.toggle_expanded(id, cx)))
                    .child(div().size(px(13.)).flex_shrink_0().text_color(ui.text_faint).child(Icon::from(if expanded { Lucide::ChevronDown } else { Lucide::ChevronRight })))
                    .child(Icon::from(Lucide::Shield).size(px(13.)).text_color(if refused { ui.warning } else { ui.text_faint }))
                    .child(div().flex_shrink_0().text_size(px(crate::theme::Type::SMALL)).text_color(if refused { ui.warning } else { ui.text_muted }).child(outcome.unwrap_or("Asked")))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(crate::theme::Type::SMALL)).font_family(ui.mono.clone()).text_color(ui.text_faint).child(first_line)),
            )
            .when(expanded, |el| {
                el.child(
                    div()
                        .ml(px(22.))
                        .my_1()
                        .p_2()
                        .rounded(px(6.))
                        .bg(ui.ink(0.03))
                        .text_size(px(crate::theme::Type::SMALL))
                        .font_family(ui.mono.clone())
                        .text_color(ui.text_muted)
                        .whitespace_normal()
                        .child(summary.clone()),
                )
                .when_some(card.explanation.clone(), |el, ex| el.child(div().ml(px(22.)).text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child(ex)))
            })
            .into_any_element()
    }

    /// Several answered requests in a row: one line that counts them, opening to the individual lines.
    fn answered_group_row(&self, first_id: u64, cards: &[(u64, ApprovalCard)], ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        // The group's own open state must not collide with its first member's.
        let key = first_id ^ (1 << 62);
        let expanded = self.thread.read(cx).expanded.contains(&key);
        let thread = self.thread.clone();
        let refused = cards.iter().filter(|(_, c)| matches!(c.resolution.as_deref(), Some(r) if r.contains("cancel") || r.contains("reject") || r.contains("deny"))).count();
        let label = match refused {
            0 => format!("{} requests allowed", cards.len()),
            n if n == cards.len() => format!("{} requests not allowed", cards.len()),
            n => format!("{} requests · {} allowed, {n} not", cards.len(), cards.len() - n),
        };
        let hover = ui.hover;
        let mut group = div().flex().flex_col().child(
            div()
                .id(("approval-group", first_id))
                .flex()
                .items_center()
                .gap_2()
                .h(px(28.))
                .px_1()
                .ml(px(-4.))
                .rounded(px(6.))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .on_click(move |_, _, cx| thread.update(cx, |t, cx| t.toggle_expanded(key, cx)))
                .child(div().size(px(13.)).flex_shrink_0().text_color(ui.text_faint).child(Icon::from(if expanded { Lucide::ChevronDown } else { Lucide::ChevronRight })))
                .child(Icon::from(Lucide::Shield).size(px(13.)).text_color(if refused > 0 { ui.warning } else { ui.text_faint }))
                .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child(label)),
        );
        if expanded {
            for (id, card) in cards {
                group = group.child(div().pl(px(18.)).child(self.answered_row(*id, card, ui, cx)));
            }
        }
        group.into_any_element()
    }

    fn line_row(&self, id: u64, role: Role, text: &str, ui: &Ui) -> AnyElement {
        // Normalize old saved creation notices without rewriting conversation history.
        let renamed = (role == Role::System)
            .then(|| text.strip_prefix("Workspace created"))
            .flatten()
            .map(|suffix| format!("Thread created{suffix}"));
        let text = renamed.as_deref().unwrap_or(text);
        if role == Role::System {
            if let Some(notice) = model_switch_notice(text, ui) {
                return fade_in(("switch", id), div().child(notice)).into_any_element();
            }
        }
        let color = match role {
            Role::Error => ui.danger,
            _ => ui.text_faint,
        };
        fade_in(
            ("line", id),
            div()
                .py_1()
                .text_size(px(crate::theme::Type::SMALL))
                .text_color(color)
                .whitespace_normal()
                .when(text.starts_with("Switched model:"), |el| {
                    el.my_2()
                        .p_3()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(ui.border)
                        .bg(ui.ink(0.03))
                        .text_color(ui.text_muted)
                })
                .child(text.to_string()),
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
        if self.follow && tail_version != self.seen_tail {
            self.pin_frames = 3;
        }
        self.seen_tail = tail_version;
        let should_pin = self.follow && self.pin_frames > 0;

        let count = rows.len();
        // Find-in-conversation: which rows contain the query.
        self.matches = match &self.search {
            Some((q, _)) => rows
                .iter()
                .enumerate()
                .filter(|(_, r)| match r {
                    Row::User { text, .. } => text.to_lowercase().contains(q.as_str()),
                    Row::Agent { raw, .. } => raw.to_lowercase().contains(q.as_str()),
                    Row::Plan { doc, .. } => doc.markdown.to_lowercase().contains(q.as_str()),
                    Row::Line { text, .. } => text.to_lowercase().contains(q.as_str()),
                    _ => false,
                })
                .map(|(i, _)| i)
                .collect(),
            None => Vec::new(),
        };
        let active_match = self
            .search
            .as_ref()
            .and_then(|(_, ix)| self.matches.get(*ix).copied());
        let match_bg = ui.warning;
        let children: Vec<AnyElement> = rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let el = match row {
                    Row::User { id, text, images } => self.user_row(*id, text, images, &ui),
                    Row::Agent {
                        id,
                        state,
                        raw,
                        streaming,
                        last,
                        at,
                        images,
                        attached,
                    } => self.agent_row(
                        *id, state, raw, *streaming, *last, at, images, attached, &ui, cx,
                    ),
                    Row::Activity {
                        first_id,
                        items,
                        collapsed,
                    } => self.activity_row(*first_id, items, *collapsed, &ui, cx),
                    Row::GeneratingImage { id, args, running } => {
                        image_placeholder(*id, args, *running, &ui)
                    }
                    Row::Plan { id, state, doc } => self.plan_row(*id, state, doc, &ui, cx),
                    Row::Approval { id, card } => self.approval_row(*id, card, &ui, cx),
                    Row::AnsweredApprovals { first_id, cards } => self.answered_group_row(*first_id, cards, &ui, cx),
                    Row::Line { id, role, text } => self.line_row(*id, *role, text, &ui),
                };
                if self.matches.contains(&i) {
                    let active = active_match == Some(i);
                    let mut bg = match_bg;
                    bg.a = if active { 0.22 } else { 0.10 };
                    div().rounded(px(8.)).bg(bg).child(el).into_any_element()
                } else {
                    el
                }
            })
            .collect();

        if let Some(target) = active_match {
            if self.scrolled_to != Some(target) {
                self.scroll.scroll_to_item(target);
                self.scrolled_to = Some(target);
                self.follow = false;
            }
        } else if should_pin && count > 0 {
            self.scroll.scroll_to_item(count - 1);
            let max = self.scroll.max_offset().y;
            if max > px(0.) {
                self.scroll.set_offset(point(px(0.), -max));
            }
            self.pin_frames -= 1;
            if self.pin_frames > 0 {
                cx.notify();
            }
        }
        let marks: Vec<AnyElement> = if self.search.is_some() && count > 0 {
            self.matches
                .iter()
                .map(|i| {
                    let active = active_match == Some(*i);
                    let mut c = match_bg;
                    c.a = if active { 1.0 } else { 0.45 };
                    div()
                        .absolute()
                        .right(px(2.))
                        .w(px(6.))
                        .h(px(3.))
                        .rounded_full()
                        .bg(c)
                        .top(relative(*i as f32 / count as f32))
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };

        div().relative().size_full().children(marks).child(
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
                            el.child(
                                div()
                                    .py_4()
                                    .text_sm()
                                    .text_color(ui.text_faint)
                                    .child("Nothing here yet."),
                            )
                        }),
                ),
        )
    }
}

/// Streaming text: the newest two words land in the accent color, the two
/// before them settle back to ink over 700ms (assistant-ui / Zeron veil).
fn streaming_text(id: u64, raw: &str, ui: &Ui) -> AnyElement {
    let text: SharedString = raw.to_string().into();
    // Word boundaries by byte offset.
    let mut words: Vec<(usize, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    for (i, ch) in raw.char_indices() {
        if ch.is_whitespace() {
            if let Some(s) = start.take() {
                words.push((s, i));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        words.push((s, raw.len()));
    }
    let n = words.len();
    let accent = ui.accent;
    let ink = ui.text;
    let key = ("stream", id * 1_000_003 + n as u64);
    div()
        .whitespace_normal()
        .with_animation(
            key,
            Animation::new(std::time::Duration::from_millis(700)),
            move |el, t| {
                let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
                for (k, (a, b)) in words.iter().enumerate() {
                    let from_end = n - 1 - k;
                    let color = if from_end < 2 {
                        accent
                    } else if from_end < 4 {
                        lerp_hsla(accent, ink, t)
                    } else {
                        continue;
                    };
                    highlights.push((
                        *a..*b,
                        HighlightStyle {
                            color: Some(color),
                            ..Default::default()
                        },
                    ));
                }
                el.child(StyledText::new(text.clone()).with_highlights(highlights))
            },
        )
        .into_any_element()
}

fn lerp_hsla(a: Hsla, b: Hsla, t: f32) -> Hsla {
    Hsla {
        h: a.h + (b.h - a.h) * t,
        s: a.s + (b.s - a.s) * t,
        l: a.l + (b.l - a.l) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

/// Terminal block: the command as header with a spinner-or-check, output
/// lines below with the newest line brightest, exit line at the bottom.
fn terminal_block(
    id: u64,
    command: &str,
    output: &str,
    done: bool,
    failed: bool,
    ui: &Ui,
) -> AnyElement {
    let lines: Vec<&str> = output.lines().collect();
    let n = lines.len();
    let mono = ui.mono.clone();
    div()
        .flex()
        .flex_col()
        .rounded(px(8.))
        .border_1()
        .border_color(ui.border)
        .bg(ui.ink(0.04))
        .min_h(px(72.))
        .min_w_0().overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .h(px(30.))
                .border_b_1()
                .border_color(ui.border)
                .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child("$"))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(crate::theme::Type::SMALL))
                        .font_family(mono.clone())
                        .text_color(ui.text)
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(command.to_string()),
                )
                .child(if !done {
                    crate::views::motion::breathe(
                        ("term-spin", id),
                        0.3,
                        div().size(px(6.)).rounded_full().bg(ui.text_muted),
                    )
                    .into_any_element()
                } else {
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_size(px(crate::theme::Type::SMALL))
                        .text_color(if failed { ui.danger } else { ui.success })
                        .child(div().size(px(12.)).child(Icon::from(if failed {
                            Lucide::X
                        } else {
                            Lucide::Check
                        })))
                        .child(if failed { "failed" } else { "exit 0" })
                        .into_any_element()
                }),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .px_3()
                .py_2()
                .id(("terminal-output-scroll",id))
                .max_h(px(320.))
                .min_w_0().overflow_y_scroll().overflow_x_hidden()
                .text_size(px(crate::theme::Type::SMALL))
                .font_family(mono)
                .when(n == 0, |el| {
                    el.child(div().text_color(ui.text_faint).child(if done {
                        "(no output)"
                    } else {
                        "running…"
                    }))
                })
                .children(lines.iter().enumerate().map(|(i, l)| {
                    let last = i + 1 == n;
                    div()
                        .whitespace_normal()
                        .text_color(if last { ui.text } else { ui.text_muted })
                        .child(l.to_string())
                })),
        )
        .into_any_element()
}

/// Image generation placeholder: an 8×8 pulsing dot grid holds a square
/// frame over a soft gradient until the image arrives.
fn image_placeholder(id: u64, args: &str, running: bool, ui: &Ui) -> AnyElement {
    let (ratio, caption) = generation_frame(args);
    let height = 320. / ratio;

    let dot = ui.text_muted;
    let mut grid = div()
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        .justify_around()
        .px(px(24.))
        .py(px(24.));
    for row in 0..8u64 {
        let mut r = div().flex().justify_around();
        for col in 0..8u64 {
            let i = row * 8 + col;
            let phase = (i as f32 * 0.09) % 1.0;
            let dot = div().size(px(4.)).rounded_full().bg(dot);
            let dot = if running {
                dot.with_animation(
                    ("gen-dot", id * 100 + i),
                    Animation::new(std::time::Duration::from_millis(1600))
                        .repeat()
                        .with_easing(move |t| pulsating_between(0.15, 0.9)((t + phase) % 1.0))
                        .with_max_fps(30.),
                    |el, t| el.opacity(t),
                )
                .into_any_element()
            } else {
                dot.opacity(0.25).into_any_element()
            };
            r = r.child(dot);
        }
        grid = grid.child(r);
    }
    let label = if running {
        crate::views::motion::breathe(("gen-label", id), 0.4, div().child("Generating image…"))
            .into_any_element()
    } else {
        div().child("Preparing image…").into_any_element()
    };
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .child(
            div()
                .relative()
                .w(px(320.))
                .h(px(height))
                .rounded(px(12.))
                .overflow_hidden()
                .border_1()
                .border_color(ui.border)
                .bg(linear_gradient(
                    135.,
                    linear_color_stop(ui.accent.opacity(0.16), 0.0),
                    linear_color_stop(ui.ink(0.03), 1.0),
                ))
                .child(grid),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(crate::theme::Type::SMALL))
                .font_family(ui.mono.clone())
                .text_color(ui.text_faint)
                .child(label),
        )
        .when_some(caption, |el, caption| {
            el.child(
                div()
                    .max_w(px(320.))
                    .text_size(px(crate::theme::Type::SMALL))
                    .text_color(ui.text_muted)
                    .child(caption),
            )
        })
        .into_any_element()
}

fn switch_parts(text: &str) -> Option<(&str, &str, &str)> {
    let (from, rest) = text.strip_prefix("Switched model: ")?.split_once(" → ")?;
    let (to, continuity) = rest.split_once(". ").unwrap_or((rest, ""));
    Some((from, to, continuity))
}

fn switch_identity(value: &str) -> (&str, &str) {
    if let Some((provider, model)) = value.split_once(" · ") {
        return (provider, model);
    }
    let provider = if value.starts_with("grok") {
        "grok"
    } else if value.starts_with("gpt") || value.contains("codex") {
        "codex"
    } else if ["claude", "opus", "sonnet", "haiku", "fable"]
        .iter()
        .any(|prefix| value.starts_with(prefix))
    {
        "claude"
    } else {
        "unknown"
    };
    (provider, value)
}

fn model_switch_notice(text: &str, ui: &Ui) -> Option<AnyElement> {
    let (from, to, continuity) = switch_parts(text)?;
    let identity = |value: &str| {
        let (provider, model) = switch_identity(value);
        super::brand::model_identity(provider, super::brand::pretty_model(model), ui)
    };
    Some(
        super::brand::handoff_card(ui)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .text_color(ui.text_muted)
                    .child(identity(from))
                    .child(Icon::from(Lucide::ArrowRight).size(px(14.)))
                    .child(identity(to)),
            )
            .child(
                div()
                    .text_size(px(crate::theme::Type::CAPTION))
                    .text_color(ui.text_faint)
                    .child(continuity.to_string()),
            )
            .into_any_element(),
    )
}

fn generating_image(row: &ToolRow) -> bool {
    let name = row.name.to_ascii_lowercase().replace('-', "_");
    let name = if name.starts_with("imagine:") {
        "image_gen"
    } else {
        &name
    };
    let name = name.rsplit([':', '.']).next().unwrap_or(name);
    matches!(row.status.as_str(), "pending" | "running" | "in_progress")
        && matches!(
            name,
            "image_gen"
                | "imagegen"
                | "image_edit"
                | "generate_image"
                | "edit_image"
                | "image_generation"
                | "image generation"
        )
}

fn generation_frame(args: &str) -> (f32, Option<String>) {
    let args = serde_json::from_str::<serde_json::Value>(args).unwrap_or_default();
    let ratio = args
        .get("aspect_ratio")
        .and_then(|v| v.as_str())
        .and_then(|s| s.split_once(':'))
        .and_then(|(w, h)| Some(w.parse::<f32>().ok()? / h.parse::<f32>().ok()?))
        .filter(|r| r.is_finite() && *r > 0.)
        .unwrap_or(1.)
        .clamp(0.5, 2.);
    let caption = args
        .get("prompt")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            let mut text: String = s.chars().take(140).collect();
            if s.chars().count() > 140 {
                text.push('…');
            }
            text
        });
    (ratio, caption)
}

/// Unified-diff heuristics: hunk headers or +++/--- file markers.
fn looks_like_diff(text: &str) -> bool {
    let mut markers = 0;
    for l in text.lines().take(40) {
        if l.starts_with("@@ ")
            || l.starts_with("+++ ")
            || l.starts_with("--- ")
            || l.starts_with("diff --git")
        {
            markers += 1;
        }
    }
    markers >= 2
}

/// Render a diff through the markdown view so tree-sitter-diff highlights it.
fn diff_block(id: impl Into<ElementId>, text: &str) -> AnyElement {
    let id=id.into();
    let md = format!("```diff\n{}\n```", text.trim_end());
    div()
        .id(id.clone())
        .text_size(px(crate::theme::Type::SMALL)).min_w_0()
        .max_h(px(400.))
        .overflow_y_scroll().overflow_x_hidden()
        .child(TextView::markdown(id, md).selectable(true).code_block_actions(|block, _, _| copy_code_button(block.code())))
        .into_any_element()
}

const IMAGE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

fn is_image_path(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    IMAGE_EXTS.iter().any(|e| lower.ends_with(&format!(".{e}")))
}

/// Resolve a path the agent mentioned against the thread cwd (then the
/// project root); only existing files count.
fn resolve_local(
    raw: &str,
    cwd: &std::path::Path,
    root: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    let raw = raw.trim().trim_start_matches("file://");
    let p = std::path::Path::new(raw);
    let candidates = if p.is_absolute() {
        vec![p.to_path_buf()]
    } else {
        let mut v = vec![cwd.join(p)];
        if let Some(r) = root {
            v.push(r.join(p));
        }
        v
    };
    candidates.into_iter().find(|c| c.is_file())
}

/// Resolve only explicit provider artifact metadata, never a guessed session directory.
fn generated_image_alias(row: &ToolRow) -> Option<(String, std::path::PathBuf)> {
    if row.status != "completed" {
        return None;
    }
    let result: serde_json::Value = serde_json::from_str(row.result.as_deref()?).ok()?;
    let path = std::path::PathBuf::from(result.get("path")?.as_str()?);
    let filename = result.get("filename")?.as_str()?;
    let folder = result.get("session_folder")?.as_str()?;
    let alias = std::path::Path::new(folder).join(filename);
    if !path.is_absolute()
        || !is_image_path(filename)
        || path.file_name()?.to_str()? != filename
        || !alias
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some((alias.to_str()?.to_owned(), path))
}

/// Image files referenced in a reply: markdown images/links and bare paths.
fn local_images(
    text: &str,
    cwd: &std::path::Path,
    root: Option<&std::path::Path>,
    aliases: &std::collections::HashMap<String, std::path::PathBuf>,
) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    let mut push = |cand: &str| {
        let cand = cand.trim_matches(|c: char| {
            matches!(
                c,
                '`' | '*' | '"' | '\'' | '(' | ')' | '<' | '>' | ',' | '.' | ':' | ';'
            )
        });
        if !is_image_path(cand) || cand.starts_with("http") {
            return;
        }
        if let Some(p) = aliases
            .get(cand)
            .filter(|p| p.is_file())
            .cloned()
            .or_else(|| resolve_local(cand, cwd, root))
        {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    };
    // markdown targets: ![alt](path) and [text](path)
    let mut rest = text;
    while let Some(i) = rest.find("](") {
        let after = &rest[i + 2..];
        if let Some(j) = after.find(')') {
            push(&after[..j]);
            rest = &after[j + 1..];
        } else {
            break;
        }
    }
    for token in text.split_whitespace() {
        push(token);
    }
    out
}

fn open_path(path: &std::path::Path) {
    let _ = std::process::Command::new("open").arg(path).spawn();
}

/// Show a file selected in Finder (a folder is opened instead).
pub(super) fn reveal_in_finder(path: &std::path::Path) {
    let mut command = std::process::Command::new("open");
    if path.is_dir() { command.arg(path); } else { command.arg("-R").arg(path); }
    let _ = command.spawn();
}

/// Split a sent message into what the person typed and the files attached to it.
/// The composer appends "Attached file(s) (open … path(s)):" followed by `- <path> (<size>)` lines.
pub(super) fn split_attached_files(text: &str) -> (String, Vec<(std::path::PathBuf, String)>) {
    let heading = ["Attached file (open it from this path):", "Attached files (open them from these paths):"];
    let Some(start) = heading.iter().filter_map(|h| text.rfind(h)).max() else { return (text.to_string(), Vec::new()); };
    // Only a note at the very end, made of well-formed lines, counts; anything else is the person's own text.
    if start > 0 && !text[..start].ends_with('\n') { return (text.to_string(), Vec::new()); }
    let mut files = Vec::new();
    for line in text[start..].lines().skip(1) {
        let parsed = line.strip_prefix("- ").and_then(|l| l.rsplit_once(" (")).and_then(|(path, size)| size.strip_suffix(')').map(|size| (path, size)));
        match parsed {
            Some((path, size)) if path.starts_with('/') => files.push((std::path::PathBuf::from(path), size.to_string())),
            _ => return (text.to_string(), Vec::new()),
        }
    }
    if files.is_empty() { return (text.to_string(), Vec::new()); }
    (text[..start].trim_end().to_string(), files)
}

/// Links in replies: web links open in the browser; file paths are shown in Finder
/// (Finder's -50 came from treating `images/1.jpg` as a URL).
pub(super) fn open_link(href: &str, cwd: &std::path::Path, cx: &mut App) {
    if href.starts_with("http://") || href.starts_with("https://") || href.starts_with("mailto:") {
        cx.open_url(href);
    } else if let Some(p) = resolve_local(href, cwd, None) {
        // A file an agent mentions: show where it is, rather than launching whatever app owns the type.
        reveal_in_finder(&p);
    } else {
        cx.open_url(href);
    }
}

/// "cmd: cargo test" → "cargo test": the verb already says what it is.
fn strip_arg_prefix(line: &str) -> String {
    let l = line.trim();
    for pre in [
        "cmd:", "command:", "path:", "file:", "pattern:", "query:", "url:",
    ] {
        if let Some(rest) = l.strip_prefix(pre) {
            return rest.trim().to_string();
        }
    }
    l.to_string()
}

/// Horizontal tick from the group rail into a row.
fn rail_tick(ui: &Ui) -> AnyElement {
    div()
        .absolute()
        .left(px(-22.))
        .top(px(13.))
        .w(px(16.))
        .h(px(1.))
        .bg(ui.border)
        .into_any_element()
}

fn mono_block(id: impl Into<ElementId>, text: &str, mono: &SharedString, color: Hsla, ui: &Ui) -> AnyElement {
    let id=id.into();
    div().id(id.clone()).min_w_0()
        .p_2()
        .rounded(px(6.))
        .bg(ui.ink(0.04))
        .border_1()
        .border_color(ui.border)
        .text_size(px(crate::theme::Type::SMALL))
        .font_family(mono.clone())
        .text_color(color)
        .whitespace_normal()
        .max_h(px(260.))
        .overflow_y_scroll().overflow_x_hidden()
        .child(TextView::markdown(id,format!("```text\n{text}\n```")).selectable(true).code_block_actions(|block, _, _| copy_code_button(block.code())))
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
    if ["bash", "shell", "terminal", "exec", "command"]
        .iter()
        .any(|k| n.contains(k))
    {
        "command"
    } else if ["edit", "write", "patch", "create", "apply"]
        .iter()
        .any(|k| n.contains(k))
    {
        "edit"
    } else if [
        "read", "glob", "grep", "search", "ls", "list", "find", "cat", "view",
    ]
    .iter()
    .any(|k| n.contains(k))
    {
        "read"
    } else if ["fetch", "web", "http", "browse"]
        .iter()
        .any(|k| n.contains(k))
    {
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
    let thoughts = items
        .iter()
        .filter(|a| matches!(a, Activity::Thought { .. }))
        .count();
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
/// A running step in a few words: "Running `npm test`", "Editing src/app.ts", "Reading 3 files…".
fn running_label(row: &ToolRow) -> String {
    let first = row.args.lines().next().unwrap_or_default().trim();
    // Arguments often arrive as JSON; pull out the part a person would recognise.
    let detail = serde_json::from_str::<serde_json::Value>(&row.args).ok().and_then(|v| {
        ["command", "file_path", "path", "url", "pattern", "query"].iter().find_map(|k| v.get(*k).and_then(|x| x.as_str()).map(str::to_owned))
    }).unwrap_or_else(|| first.to_string());
    let detail: String = detail.lines().next().unwrap_or_default().chars().take(80).collect();
    let verb = match tool_kind(&row.name) { "command" => "Running", "edit" => "Editing", "read" => "Reading", "web" => "Fetching", _ => "Working on" };
    match (detail.is_empty(), tool_kind(&row.name)) {
        (true, "command") => "Running a command".into(),
        (true, _) => format!("{verb} {}", if row.name.is_empty() || row.name == "tool" { "a step" } else { row.name.as_str() }),
        (false, "command") => format!("{verb} `{detail}`"),
        (false, _) => format!("{verb} {detail}"),
    }
}

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
        if parts.is_empty() && other == 1 && !last_name.is_empty() && last_name != "tool" {
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
    #[test]
    fn attached_files_become_chips_and_leave_the_typed_text_alone() {
        use super::split_attached_files as split;
        let note = crate::views::composer::attached_files_note(&[("/Users/max/My Docs/spec (v2).pdf".into(), 2_400_000), ("/tmp/data.csv".into(), 1500)]);
        let (text, files) = split(&format!("Summarise these\n\n{note}"));
        assert_eq!(text, "Summarise these");
        assert_eq!(files, [("/Users/max/My Docs/spec (v2).pdf".into(), "2.3 MB".to_string()), ("/tmp/data.csv".into(), "2 KB".to_string())]);
        // Files only, nothing typed.
        let (text, files) = split(&crate::views::composer::attached_files_note(&[("/tmp/a.txt".into(), 12)]));
        assert_eq!((text.as_str(), files.len()), ("", 1));
        // The person quoting the phrase mid-sentence, or lines that are not paths, stay as text.
        for plain in ["no files here", "I saw Attached file (open it from this path): earlier", "Attached file (open it from this path):\n- not a path"] {
            assert_eq!(split(plain), (plain.to_string(), Vec::new()), "{plain}");
        }
    }

    #[test]
    fn a_running_step_is_described_in_a_few_words() {
        use bomb_core::transcript::ToolRow;
        let row = |name: &str, args: &str| ToolRow { tool_id: "t".into(), name: name.into(), status: "running".into(), args: args.into(), result: None };
        assert_eq!(super::running_label(&row("Bash", r#"{"command":"npm test\nsecond line"}"#)), "Running `npm test`");
        assert_eq!(super::running_label(&row("Edit", r#"{"file_path":"src/app.ts","new_string":"x"}"#)), "Editing src/app.ts");
        assert_eq!(super::running_label(&row("Read", "src/lib.rs")), "Reading src/lib.rs");
        assert_eq!(super::running_label(&row("terminal", "")), "Running a command");
        assert_eq!(super::running_label(&row("tool", "")), "Working on a step");
        // A lone unnamed step no longer renders as the bare word "tool".
        assert_eq!(super::tool_group_summary([row("tool", "")].iter()), "1 other tool call");
        assert_eq!(super::tool_group_summary([row("Task", "")].iter()), "Task");
    }

    #[test]
    fn a_request_shows_its_command_once() {
        use super::approval_summary as a;
        assert_eq!(a(r#"ls node_modules: {"command":"ls node_modules"}"#, false), "ls node_modules");
        // Cut short by the agent, so the JSON no longer parses.
        assert_eq!(a(r#"ls node_modules | grep x: {"command":"ls node_modules | gre…"#, true), "ls node_modules | grep x");
        assert_eq!(a(r#"ls node_modules: {"command":"ls node_modules"}"#, true), "ls node_modules", "a pure echo is dropped even while waiting");
        let scoped = r#"Shell: {"command":"rm file","path":"/tmp"}"#;
        assert_eq!(a(scoped, true), scoped, "extra scope stays visible while waiting");
        assert_eq!(a(scoped, false), "rm file");
        assert_eq!(a(r#"Read: {"file_path":"/src/app.ts"}"#, false), "/src/app.ts");
        let edit = r#"Edit: {"file_path":"/src/app.ts","new_string":"x"}"#;
        assert_eq!(a(edit, false), "Edit /src/app.ts");
        assert_eq!(a(edit, true), edit, "a waiting edit shows everything being approved");
        assert_eq!(a("plain text", false), "plain text");
    }
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
        let rows = [
            row("Bash"),
            row("Bash"),
            row("Edit"),
            row("Read"),
            row("Sparkle"),
        ];
        assert_eq!(
            tool_group_summary(rows.iter()),
            "Ran 2 commands · Edited 1 file · Read 1 file · 1 other tool call"
        );
        let one = [row("Sparkle")];
        assert_eq!(tool_group_summary(one.iter()), "Sparkle");
    }
}

/// Keep the requested target readable; preserve the full request in Details.
/// A copy button in the corner of every code block: commands, output and diffs.
fn copy_code_button(code: SharedString) -> AnyElement {
    // Distinct per block so each button keeps its own "copied" tick.
    let id = { use std::hash::{Hash, Hasher}; let mut h = std::collections::hash_map::DefaultHasher::new(); code.hash(&mut h); h.finish() };
    gpui_kit::component::clipboard::Clipboard::new(("copy-code", id)).value(code).tooltip("Copy").into_any_element()
}

/// Chat prose is shaped without pair kerning or contextual forms so measured and drawn widths agree.
fn prose_font_features() -> FontFeatures {
    FontFeatures(Arc::new(vec![("kern".into(), 0), ("liga".into(), 0), ("calt".into(), 0)]))
}

/// What was asked, once. Agents send `<command>: {"command": "<command>"}`; show the command, not its echo.
/// A waiting edit keeps its full text in view (`waiting`), because that is what is being approved.
fn approval_summary(summary: &str, waiting: bool) -> String {
    if let Some(start) = summary.find('{') {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&summary[start..]) {
            if let Some(command) = value.get("command").and_then(|v| v.as_str()) {
                // While waiting, only drop the JSON when it is a pure echo: anything extra in it
                // (a working folder, say) is part of what is being approved.
                let lead = summary[..start].trim().trim_end_matches(':').trim();
                let echo = lead == command && value.as_object().is_some_and(|o| o.keys().all(|k| matches!(k.as_str(), "command" | "description" | "timeout" | "run_in_background")));
                return if waiting && !echo { summary.to_string() } else { command.to_string() };
            }
            if let Some(path) = value.get("file_path").or_else(|| value.get("path")).and_then(|v| v.as_str()) {
                // An edit's full text stays available under the chevron; the line names the file.
                let editing = value.get("new_string").is_some() || value.get("content").is_some();
                if editing && waiting { return summary.to_string(); }
                return if editing { format!("Edit {path}") } else { path.to_string() };
            }
            let lead = summary[..start].trim().trim_end_matches(':').trim();
            if !lead.is_empty() { return lead.to_string(); }
        }
    }
    // A summary cut short no longer parses; the text before its JSON echo is still the command.
    match summary.find(": {\"") {
        Some(cut) if !summary[..cut].trim().is_empty() => summary[..cut].trim().to_string(),
        _ => summary.to_string(),
    }
}

#[cfg(test)]
mod approval_display_tests {
    use super::approval_summary;
    #[test]
    fn read_target_is_concise_but_commands_and_invalid_payloads_remain_visible() {
        assert_eq!(
            approval_summary(r#"Read /tmp/a.png: {"file_path":"/tmp/a.png"}"#, true),
            "/tmp/a.png"
        );
        for text in [
            r#"Shell: {"command":"rm file","path":"/tmp"}"#,
            "Read: {broken",
            r#"Edit: {"path":"a","new_string":"b"}"#,
        ] {
            assert_eq!(approval_summary(text, true), text);
        }
    }
}

#[cfg(test)]
mod image_generation_tests {
    use super::{generating_image, generation_frame, ToolRow};
    #[test]
    fn only_active_generation_tools_get_placeholders() {
        let mut row = ToolRow {
            tool_id: "1".into(),
            name: "image_gen".into(),
            status: "running".into(),
            args: String::new(),
            result: None,
        };
        for name in [
            "image_gen",
            "image_edit",
            "generate_image",
            "functions.imagegen",
            "imagine: A cinematic arcade.",
        ] {
            row.name = name.into();
            assert!(generating_image(&row));
        }
        for status in ["completed", "failed", "denied", "cancelled", "restored"] {
            row.status = status.into();
            assert!(!generating_image(&row));
        }
        row.status = "running".into();
        for name in [
            "read_image",
            "view_image",
            "image_search",
            "Read /tmp/image.png",
        ] {
            row.name = name.into();
            assert!(!generating_image(&row));
        }
    }
    #[test]
    fn frame_uses_real_aspect_ratio_and_handles_partial_arguments() {
        assert_eq!(
            generation_frame(r#"{"aspect_ratio":"16:9","prompt":"Lake"}"#),
            (16. / 9., Some("Lake".into()))
        );
        for args in ["", "{partial", r#"{"aspect_ratio":"1:0"}"#] {
            assert_eq!(generation_frame(args), (1., None));
        }
    }
}

#[cfg(test)]
mod artifact_tests {
    use super::{generated_image_alias, local_images, ToolRow};
    #[test]
    fn resolves_session_relative_artifacts_and_rejects_traversal() {
        let dir = std::env::temp_dir().join(format!("bomb-artifact-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("1.jpg");
        std::fs::write(&file, b"test image coordinates").unwrap();
        let mut row = ToolRow {
            tool_id: "1".into(),
            name: "tool".into(),
            status: "completed".into(),
            args: String::new(),
            result: Some(
                serde_json::json!({"path":file,"filename":"1.jpg","session_folder":"images"})
                    .to_string(),
            ),
        };
        let (alias, path) = generated_image_alias(&row).unwrap();
        let aliases = std::collections::HashMap::from([(alias, path.clone())]);
        assert_eq!(
            local_images("![Arcade](images/1.jpg)", &dir, None, &aliases),
            vec![path]
        );
        row.result = Some(
            serde_json::json!({"path":file,"filename":"1.jpg","session_folder":"../images"})
                .to_string(),
        );
        assert!(generated_image_alias(&row).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod model_switch_tests {
    use super::{switch_identity, switch_parts};
    #[test]
    fn switch_notice_supports_saved_and_provider_qualified_models() {
        let (from, to, context) = switch_parts("Switched model: gpt-6-astra → grok · grok-4.6. Recent history carried over.").unwrap();
        assert_eq!(switch_identity(from), ("codex", "gpt-6-astra"));
        assert_eq!(switch_identity(to), ("grok", "grok-4.6"));
        assert_eq!(context, "Recent history carried over.");
        let (from, to, _) = switch_parts("Switched model: claude · sonnet → codex · gpt-6-astra. Context retained.").unwrap();
        assert_eq!(switch_identity(from), ("claude", "sonnet"));
        assert_eq!(switch_identity(to), ("codex", "gpt-6-astra"));
        assert!(switch_parts("An ordinary message").is_none());
    }
}
