//! Center column: header · transcript (or welcome) · status line + meter ·
//! composer.

use std::time::Instant;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use uuid::Uuid;

use crate::models::app::AppModel;
use crate::models::thread::ThreadModel;
use crate::theme::{Layout, Ui};
use crate::views::composer::ComposerView;
use crate::views::meter::meter_bar;
use crate::views::motion::fade_in;
use crate::views::status_line::{status_line, StatusLineProps};
use crate::views::transcript::TranscriptView;

pub struct ThreadView {
    model: Entity<AppModel>,
    transcript: Option<(String, Entity<TranscriptView>)>,
    composer: Entity<ComposerView>,
    search_open: bool,
    search: Entity<InputState>,
}

impl ThreadView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |this, _, cx| {
            this.sync_transcript(cx);
            cx.notify()
        })
        .detach();
        let composer = cx.new(|cx| ComposerView::new(model.clone(), window, cx));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Find in conversation"));
        cx.subscribe(&search, |this, _, ev: &InputEvent, cx| match ev {
            InputEvent::Change => this.push_search(cx),
            InputEvent::PressEnter { shift, .. } => {
                let delta = if *shift { -1 } else { 1 };
                if let Some((_, t)) = &this.transcript {
                    t.update(cx, |t, cx| t.step_search(delta, cx));
                }
                cx.notify();
            }
            _ => {}
        })
        .detach();
        let mut this = Self {
            model,
            transcript: None,
            composer,
            search_open: false,
            search,
        };
        this.sync_transcript(cx);
        this
    }

    pub fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |c, cx| c.focus(window, cx));
    }

    /// ⌘F: open the find bar (or focus it); Esc / close hides it.
    pub fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_open {
            self.close_search(window, cx);
        } else {
            self.search_open = true;
            self.search.update(cx, |s, cx| s.focus(window, cx));
            self.push_search(cx);
            cx.notify();
        }
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_open = false;
        if let Some((_, t)) = &self.transcript {
            t.update(cx, |t, cx| t.set_search(None, cx));
        }
        self.focus_composer(window, cx);
        cx.notify();
    }

    fn push_search(&mut self, cx: &mut Context<Self>) {
        let q = self.search.read(cx).value().to_string();
        if let Some((_, t)) = &self.transcript {
            t.update(cx, |t, cx| t.set_search(Some(q), cx));
        }
        cx.notify();
    }

    fn find_bar(&self, ui: &Ui, cx: &mut Context<Self>) -> impl IntoElement {
        let status = self
            .transcript
            .as_ref()
            .and_then(|(_, t)| t.read(cx).search_status())
            .map(|(i, n)| if n == 0 { "no matches".to_string() } else { format!("{i} of {n}") })
            .unwrap_or_default();
        let hover = ui.hover;
        let icon_button = move |id: &'static str, icon: Lucide, muted: Hsla| {
            div()
                .id(id)
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .text_color(muted)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(div().size(px(13.)).child(Icon::from(icon)))
        };
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_1p5()
            .border_b_1()
            .border_color(ui.border)
            .child(div().w(px(280.)).child(Input::new(&self.search).cleanable(true)))
            .child(div().text_xs().text_color(ui.text_faint).child(status))
            .child(icon_button("find-prev", Lucide::ChevronUp, ui.text_muted).on_click(cx.listener(|this, _, _, cx| {
                if let Some((_, t)) = &this.transcript {
                    t.update(cx, |t, cx| t.step_search(-1, cx));
                }
            })))
            .child(icon_button("find-next", Lucide::ChevronDown, ui.text_muted).on_click(cx.listener(|this, _, _, cx| {
                if let Some((_, t)) = &this.transcript {
                    t.update(cx, |t, cx| t.step_search(1, cx));
                }
            })))
            .child(div().flex_1())
            .child(icon_button("find-close", Lucide::X, ui.text_muted).on_click(cx.listener(|this, _, window, cx| {
                this.close_search(window, cx);
            })))
    }

    fn sync_transcript(&mut self, cx: &mut Context<Self>) {
        let selected = self.model.read(cx).selected_thread();
        match selected {
            None => self.transcript = None,
            Some(t) => {
                let id = t.read(cx).id();
                let same = self
                    .transcript
                    .as_ref()
                    .map(|(cur, _)| cur == &id)
                    .unwrap_or(false);
                if !same {
                    let view = cx.new(|cx| TranscriptView::new(t.clone(), cx));
                    cx.observe(&t, |_, _, cx| cx.notify()).detach();
                    self.transcript = Some((id, view));
                }
            }
        }
    }

    /// Empty state: greeting, three ways in, composer front and center
    /// (assistant-ui). Builds top to bottom with staggered entrances.
    fn welcome(&self, ui: &Ui, cx: &mut Context<Self>) -> impl IntoElement {
        let project = self
            .model
            .read(cx)
            .active_project
            .as_deref()
            .map(crate::models::app::project_name);
        let suggestions: [(&str, &str, Lucide); 3] = [
            ("Explain this project", "Read the codebase and summarize how it's put together, where the entry points are, and what looks fragile.", Lucide::BookOpen),
            ("Fix a failing test", "Run the test suite, pick the first failure, find the root cause and fix it. Show me the diff before committing.", Lucide::Bug),
            ("Start a feature", "I want to build a new feature. Ask me what it should do, propose a plan, then implement it step by step.", Lucide::Sparkles),
        ];
        let composer = self.composer.clone();
        let hover = ui.hover;
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_6()
            .px_6()
            .child(fade_in(
                "empty-greeting",
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_2()
                    .child(div().size(px(22.)).text_color(ui.text_muted).child(Icon::from(Lucide::Bomb)))
                    .child(div().text_size(px(22.)).font_weight(FontWeight::MEDIUM).text_color(ui.text).child("How can I help?"))
                    .child(self.project_row(project, ui, cx)),
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap_2()
                    .max_w(px(640.))
                    .children(suggestions.iter().enumerate().map(|(i, (title, prompt, icon))| {
                        let composer = composer.clone();
                        let prompt = prompt.to_string();
                        crate::views::motion::fade_in(
                            ("empty-suggestion", i as u64),
                            div()
                                .id(("suggestion", i))
                                .flex()
                                .items_center()
                                .gap_2()
                                .h(px(34.))
                                .px_3()
                                .rounded(px(10.))
                                .border_1()
                                .border_color(ui.border)
                                .text_sm()
                                .text_color(ui.text_muted)
                                .cursor_pointer()
                                .hover(move |s| s.bg(hover))
                                .on_click(move |_, window, cx| {
                                    composer.update(cx, |c, cx| c.set_text(&prompt, window, cx));
                                })
                                .child(div().size(px(14.)).child(Icon::from(*icon)))
                                .child(*title),
                        )
                    })),
            )
    }

    /// "in <project ▾> · temporary chat" under the greeting.
    fn project_row(&self, project: Option<String>, ui: &Ui, cx: &mut Context<Self>) -> impl IntoElement {
        use gpui_kit::component::button::Button;
        use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
        use gpui_kit::component::Sizable;
        let projects = self.model.read(cx).projects.clone();
        let temporary = self.model.read(cx).prefs.temporary;
        let app = self.model.clone();
        let app2 = self.model.clone();
        div()
            .flex()
            .items_center()
            .gap_2()
            .text_sm()
            .text_color(ui.text_faint)
            .child("in")
            .child(
                Button::new("empty-project")
                    .outline()
                    .small()
                    .compact()
                    .label(project.unwrap_or_else(|| "choose a project".into()))
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut menu, _, _| {
                        for p in &projects {
                            let a = app.clone();
                            let root = p.clone();
                            menu = menu.item(PopupMenuItem::new(crate::models::app::project_name(p)).on_click(move |_, _, cx| {
                                a.update(cx, |m, cx| m.set_active_project(root.clone(), cx));
                            }));
                        }
                        menu = menu.separator();
                        let a = app.clone();
                        menu.item(PopupMenuItem::new("Open project…").on_click(move |_, _, cx| {
                            a.update(cx, |m, cx| m.open_project(cx));
                        }))
                    }),
            )
            .child("·")
            .child(
                div()
                    .id("temporary-chat")
                    .flex()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .text_color(if temporary { ui.text } else { ui.text_faint })
                    .on_click(move |_, _, cx| {
                        app2.update(cx, |m, cx| {
                            m.prefs.temporary = !m.prefs.temporary;
                            cx.notify();
                        })
                    })
                    .child(div().size(px(12.)).child(Icon::from(if temporary { Lucide::SquareCheck } else { Lucide::Square })))
                    .child("temporary chat (no worktree)"),
            )
    }

    fn header(&self, thread: &Entity<ThreadModel>, ui: &Ui, cx: &mut Context<Self>) -> impl IntoElement {
        let t = thread.read(cx);
        let id = Uuid::parse_str(&t.meta.id).ok();
        let backend = t.meta.backend.clone();
        let model = t.meta.model.clone();
        let has_worktree = t.meta.worktree.is_some();
        let branch = t
            .meta
            .worktree
            .as_deref()
            .and_then(|w| std::path::Path::new(w).file_name())
            .map(|s| s.to_string_lossy().to_string());
        let live = t.meta.live;
        let brain = t.meta.brain_mode.clone();
        let dev = self.model.read(cx).dev_server.clone();
        let dev_running = dev.as_ref().map(|d| d.running).unwrap_or(false);
        let dev_url = dev.as_ref().and_then(|d| d.url.clone());
        let app = self.model.clone();
        let hover = ui.hover;

        let chip = |id: &'static str, label: String, ui: &Ui| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .h(px(24.))
                .px_2()
                .rounded(px(6.))
                .text_xs()
                .text_color(ui.text_muted)
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(label)
        };

        div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(Layout::HEADER))
            .px_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(ui.text_faint)
                    .child(crate::views::brand::brand_mark(&backend, 13., true, ui))
                    .child(if model.is_empty() { backend.clone() } else { model })
                    .child("·")
                    .child(if live { brain.unwrap_or_else(|| "live".into()) } else { "saved".into() }),
            )
            .child(div().flex_1())
            .when(has_worktree, |el| {
                let app_land = app.clone();
                let app_sync = app.clone();
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .text_color(ui.text_faint)
                        .child(div().size(px(11.)).child(Icon::from(Lucide::GitBranch)))
                        .child(branch.clone().unwrap_or_default()),
                )
                .child(chip("sync", "Sync".into(), ui).on_click(move |_, _, cx| {
                    if let Some(id) = id {
                        app_sync.update(cx, |m, cx| m.sync_thread(id, cx));
                    }
                }))
                .child(chip("land", "Land".into(), ui).on_click(move |_, _, cx| {
                    if let Some(id) = id {
                        app_land.update(cx, |m, cx| m.land_thread(id, cx));
                    }
                }))
            })
            .child(
                chip(
                    "dev-preview",
                    if dev_running { "Preview".into() } else { "Dev server".into() },
                    ui,
                )
                .child(div().size(px(12.)).child(Icon::from(if dev_running { Lucide::AppWindow } else { Lucide::Play })))
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(crate::actions::ToggleDevPreview), cx)
                }),
            )
            .when_some(dev_url.filter(|_| dev_running), |el, url| {
                el.child(div().text_xs().font_family(ui.mono.clone()).text_color(ui.text_faint).child(url))
            })
    }
}

impl Render for ThreadView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::of(cx);
        let thread = self.model.read(cx).selected_thread();
        let composer = self.composer.clone();

        let Some(thread) = thread else {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .child(div().flex_1().min_h_0().child(self.welcome(&ui, cx)))
                .child(composer)
                .into_any_element();
        };
        let Some((tid, transcript)) = self.transcript.clone() else {
            return div().size_full().into_any_element();
        };
        let now = Instant::now();
        let (presence, explain_open, explain_text, explain_pending, has_explanations) = {
            let t = thread.read(cx);
            (
                t.thread.presence.clone(),
                t.explain_open,
                t.thread.explanations.last().map(|e| e.text.clone()),
                t.thread.explain_pending,
                !t.thread.explanations.is_empty(),
            )
        };
        let meter = presence.meter(now);
        let show_status = presence.visible();
        let thread_for_toggle = thread.clone();

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(self.header(&thread, &ui, cx))
            .when(self.search_open, |el| el.child(self.find_bar(&ui, cx)))
            .child(fade_in(
                SharedString::from(format!("transcript-{tid}")),
                div().flex_1().min_h_0().child(transcript),
            ))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .px_6()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(Layout::COMPOSER_MAX))
                            .flex()
                            .flex_col()
                            .when(show_status, |el| {
                                el.child(status_line(
                                    StatusLineProps {
                                        presence: &presence,
                                        now,
                                        explain_open,
                                        explain_text,
                                        explain_pending,
                                        has_explanations,
                                    },
                                    move |_, _, cx| {
                                        thread_for_toggle.update(cx, |t, cx| {
                                            t.explain_open = !t.explain_open;
                                            cx.notify();
                                        });
                                    },
                                    &ui,
                                ))
                                .child(div().pb_2().child(meter_bar("meter", meter, &ui)))
                            })
                            .when(!show_status, |el| el.child(div().h(px(20.)))),
                    ),
            )
            .child(composer)
            .into_any_element()
    }
}
