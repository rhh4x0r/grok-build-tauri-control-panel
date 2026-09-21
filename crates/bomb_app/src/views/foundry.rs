//! Native Foundry authoring, library and run inspection.
use crate::views::button::Button;
use crate::{
    models::app::AppModel,
    runtime::{services, spawn_service},
    theme::Ui,
};
use bomb_foundry::{Document, Run, RunStatus, SkillEdge, StageBinding};
use gpui_kit::component::{
    button::ButtonVariants,
    input::{Input, InputState, Textarea, TextareaState},
    Disableable, Sizable,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use std::time::Duration;

pub struct FoundryView {
    model: Entity<AppModel>,
    document: Document,
    baseline: Document,
    library: Vec<Document>,
    runs: Vec<Run>,
    tab: usize,
    section: String,
    selected: usize,
    editor: Entity<TextareaState>,
    request: Entity<TextareaState>,
    search: Entity<InputState>,
    message: Option<String>,
    busy: bool,
    saved: bool,
    list_view: bool,
    zoom: f32,
    pan: Point<Pixels>,
    drag: Option<(String, Point<Pixels>, bomb_foundry::Position)>,
    pending: Option<Document>,
    needs_sync: bool,
    history: Vec<Document>,
    run_selected: Option<String>,
    expanded_attempt: Option<String>,
    _poll: Task<()>,
}
impl FoundryView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let document = Document::new("Describe what you want to accomplish");
        let editor = cx.new(|cx| TextareaState::new(window, cx).auto_grow(8, 28));
        let request = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Describe your idea, task, or project…")
                .auto_grow(2, 6)
        });
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search saved skills…"));
        let weak = cx.entity().downgrade();
        let poll = cx.spawn(async move |_, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(750))
                .await;
            if weak
                .update(cx, |v, cx| {
                    v.runs = services(cx).foundry.runs();
                    cx.notify();
                })
                .is_err()
            {
                break;
            }
        });
        let mut v = Self {
            model,
            baseline: document.clone(),
            document,
            library: vec![],
            runs: vec![],
            tab: 0,
            section: "goal".into(),
            selected: 0,
            editor,
            request,
            search,
            message: None,
            busy: false,
            saved: false,
            list_view: false,
            zoom: 1.,
            pan: point(px(0.), px(0.)),
            drag: None,
            pending: None,
            needs_sync: false,
            history: vec![],
            run_selected: None,
            expanded_attempt: None,
            _poll: poll,
        };
        v.reload(cx);
        v.sync_editor(window, cx);
        v
    }
    pub fn show_runs(&mut self, cx: &mut Context<Self>) {
        self.tab = 2;
        cx.notify();
    }
    pub fn seed(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if !text.trim().is_empty() {
            self.document = Document::new(&text);
            self.baseline = self.document.clone();
            self.saved = false;
            self.request
                .update(cx, |s, cx| s.set_value(text, window, cx));
            self.tab = 0;
            self.sync_editor(window, cx);
        }
        cx.notify();
    }
    fn reload(&mut self, cx: &mut Context<Self>) {
        let service = services(cx);
        self.library = service.foundry.store.documents().unwrap_or_default();
        self.runs = service.foundry.runs();
    }
    fn sync_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = if self.tab == 0 {
            self.document
                .contract
                .0
                .get(&self.section)
                .cloned()
                .unwrap_or_default()
        } else if self.section == "binding" {
            serde_json::to_value(
                self.document
                    .graph
                    .nodes
                    .get(self.selected)
                    .and_then(|n| self.document.bindings.get(&n.id))
                    .cloned()
                    .unwrap_or_default(),
            )
            .unwrap()
        } else if self.section == "links" {
            serde_json::to_value(&self.document.graph.edges).unwrap()
        } else if self.section == "policy" {
            serde_json::to_value(&self.document.policy).unwrap()
        } else {
            self.document
                .graph
                .nodes
                .get(self.selected)
                .and_then(|n| serde_json::to_value(n).ok())
                .and_then(|n| n.get(&self.section).cloned())
                .unwrap_or_default()
        };
        let text = if let Some(s) = value.as_str() {
            s.to_string()
        } else if value
            .as_array()
            .is_some_and(|a| a.iter().all(serde_json::Value::is_string))
            && !["sources", "agentRoles", "phases", "assumptions", "links"]
                .contains(&self.section.as_str())
        {
            value
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            serde_json::to_string_pretty(&value).unwrap_or_default()
        };
        self.editor
            .update(cx, |s, cx| s.set_value(text, window, cx));
        cx.notify();
    }
    fn apply_edit(&mut self, cx: &mut Context<Self>) -> bool {
        let text = self.editor.read(cx).value().to_string();
        let result: Result<(), String> = (|| {
            if self.tab == 0 {
                let current = &self.document.contract.0[&self.section];
                let value = if current.is_string() {
                    serde_json::Value::String(text)
                } else if current
                    .as_array()
                    .is_some_and(|a| a.iter().all(serde_json::Value::is_string))
                    && !["sources", "agentRoles", "phases", "assumptions"]
                        .contains(&self.section.as_str())
                {
                    serde_json::json!(text
                        .lines()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>())
                } else {
                    serde_json::from_str(&text).map_err(|e| e.to_string())?
                };
                let mut next = self.document.contract.clone();
                next.0[&self.section] = value;
                next.validate()?;
                self.document.contract = next;
            } else if self.tab == 1 {
                match self.section.as_str() {
                    "binding" => {
                        let b: StageBinding =
                            serde_json::from_str(&text).map_err(|e| e.to_string())?;
                        let n = self
                            .document
                            .graph
                            .nodes
                            .get(self.selected)
                            .ok_or("Select a stage")?;
                        self.document.bindings.insert(n.id.clone(), b);
                    }
                    "links" => {
                        let edges: Vec<SkillEdge> =
                            serde_json::from_str(&text).map_err(|e| e.to_string())?;
                        let mut g = self.document.graph.clone();
                        g.edges = edges;
                        g.validate(false)?;
                        self.document.graph = g;
                    }
                    "policy" => {
                        self.document.policy =
                            serde_json::from_str(&text).map_err(|e| e.to_string())?
                    }
                    field => {
                        let n = self
                            .document
                            .graph
                            .nodes
                            .get_mut(self.selected)
                            .ok_or("Select a stage")?;
                        match field {
                            "title" => n.title = text,
                            "purpose" => n.purpose = text,
                            "prompt" => n.prompt = text,
                            "exitCriteria" => {
                                n.exit_criteria = text
                                    .lines()
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                    .map(str::to_owned)
                                    .collect()
                            }
                            _ => return Err("Unknown field".into()),
                        }
                    }
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.message = Some("Changes applied".into());
                self.autosave(cx);
                true
            }
            Err(e) => {
                self.message = Some(e);
                cx.notify();
                false
            }
        }
    }
    fn autosave(&mut self, cx: &mut Context<Self>) {
        if self.saved {
            self.save(cx);
        }
        cx.notify();
    }
    fn save(&mut self, cx: &mut Context<Self>) {
        self.document.name = self.document.contract.0["title"]
            .as_str()
            .unwrap_or("Untitled skill")
            .into();
        match services(cx).foundry.store.save(&mut self.document) {
            Ok(()) => {
                self.saved = true;
                self.message = Some(format!(
                    "Saved on this device · revision {}",
                    self.document.revision
                ));
                self.reload(cx);
            }
            Err(e) => self.message = Some(e),
        }
        cx.notify();
    }
    fn draft(&mut self, full: bool, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.request.read(cx).value().to_string();
        if text.trim().is_empty() {
            self.message = Some("Describe the task first".into());
            return;
        }
        let target = match self.model.read(cx).prefs.backend.as_str() {
            "codex" => "openai-codex",
            "claude" => "claude-code",
            _ => "grok-build",
        };
        let mode = self.document.contract.0["operatingMode"]
            .as_str()
            .unwrap_or("planning-docs")
            .to_string();
        self.document = Document::new(&text);
        self.document.contract = bomb_foundry::draft(
            &text,
            if full { "full-project" } else { "fast-draft" },
            &mode,
            target,
        );
        self.baseline = self.document.clone();
        self.saved = false;
        self.section = "goal".into();
        self.sync_editor(window, cx);
    }
    fn refine(&mut self, section: bool, cx: &mut Context<Self>) {
        if self.busy || !self.apply_edit(cx) {
            return;
        }
        self.busy = true;
        let state = services(cx);
        let d = self.document.clone();
        let backend = self.model.read(cx).prefs.backend.clone();
        let model = self.model.read(cx).effective_model();
        let key = section.then(|| self.section.clone());
        let weak = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { bomb_core::foundry::refine(state, d, backend, model, key).await },
            move |result, cx| {
                let _ = weak.update(cx, |v, cx| {
                    v.busy = false;
                    match result {
                        Ok(d) => {
                            v.pending = Some(d);
                            v.message = Some(
                                "Review the proposed change below, then accept or discard".into(),
                            );
                        }
                        Err(e) => {
                            v.message = Some(format!("Refinement failed; original preserved: {e}"))
                        }
                    }
                    cx.notify();
                });
            },
        );
    }
    fn handoff(&mut self, skill: bool, cx: &mut Context<Self>) {
        if !self.apply_edit(cx) {
            return;
        }
        let text = if skill {
            match self.document.graph.markdown() {
                Ok(s) => s,
                Err(e) => {
                    self.message = Some(e);
                    return;
                }
            }
        } else {
            self.document.contract.markdown()
        };
        self.model.update(cx, |m, cx| {
            m.foundry_insert = Some(text);
            cx.notify();
        });
    }
    fn start_run(&mut self, cx: &mut Context<Self>) {
        if self.busy || !self.apply_edit(cx) {
            return;
        }
        let m = self.model.read(cx);
        let thread = m.selected_thread();
        let parent = thread.as_ref().map(|t| t.read(cx).meta.id.clone());
        let cwd = thread
            .map(|t| t.read(cx).meta.cwd.clone())
            .or_else(|| m.active_project.clone());
        let backend = m.prefs.backend.clone();
        let model = m.effective_model();
        let approval = m.prefs.mode.clone();
        let document = self.document.clone();
        let state = services(cx);
        let weak = cx.entity().downgrade();
        self.busy = true;
        spawn_service(
            cx,
            async move {
                let cwd = match cwd {
                    Some(cwd) => cwd,
                    None => {
                        let home = std::env::var_os("HOME").ok_or("Home directory unavailable")?;
                        bomb_core::services::scratch::create(
                            &std::path::PathBuf::from(home).join(".bombcode/chats"),
                        )
                        .await?
                        .to_string_lossy()
                        .into_owned()
                    }
                };
                bomb_core::foundry::FoundryService::start(
                    state, document, cwd, backend, model, approval, parent,
                )
                .await
            },
            move |res, cx| {
                let _ = weak.update(cx, |v, cx| {
                    v.busy = false;
                    match res {
                        Ok(run) => {
                            v.document = run.document.clone();
                            v.saved = true;
                            v.run_selected = Some(run.id);
                            v.tab = 2;
                            v.reload(cx);
                            v.model.update(cx, |m, cx| m.refresh_threads(cx));
                        }
                        Err(e) => v.message = Some(e),
                    }
                    cx.notify();
                });
            },
        );
    }
    fn command(
        &mut self,
        id: String,
        command: &'static str,
        gate_token: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let state = services(cx);
        let weak = cx.entity().downgrade();
        spawn_service(
            cx,
            async move { bomb_core::foundry::FoundryService::command(state, id, command, gate_token) },
            move |res, cx| {
                let _ = weak.update(cx, |v, cx| {
                    if let Err(e) = res {
                        v.message = Some(e);
                    }
                    v.reload(cx);
                    cx.notify();
                });
            },
        );
    }
    fn import(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import Foundry JSON".into()),
        });
        cx.spawn(async move |weak, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(path) = paths.first() {
                    let result = std::fs::read_to_string(path)
                        .map_err(|e| e.to_string())
                        .and_then(|s| Document::import(&s));
                    let _ = weak.update(cx, |v, cx| {
                        match result {
                            Ok(d) => {
                                v.document = d;
                                v.baseline = v.document.clone();
                                v.saved = false;
                                v.needs_sync = true;
                                v.message = Some(
                                    "Imported. Select a section to edit; save to keep it locally."
                                        .into(),
                                );
                            }
                            Err(e) => v.message = Some(e),
                        }
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
    fn export(&mut self, cx: &mut Context<Self>) {
        if !self.apply_edit(cx) {
            return;
        }
        let d = self.document.clone();
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Export package here".into()),
        });
        cx.spawn(async move |weak, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                if let Some(path) = paths.first() {
                    let dir = path.join(format!("foundry-{}", bomb_foundry::id()));
                    let res = bomb_foundry::export_package(&d, &dir);
                    let _ = weak.update(cx, |v, cx| {
                        v.message = Some(match res {
                            Ok(()) => format!("Exported {}", dir.display()),
                            Err(e) => e,
                        });
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
    fn contract(&self, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let mut sections = div().w(px(190.)).flex_shrink_0().flex().flex_col().gap_1();
        for key in ["title", "sources"]
            .into_iter()
            .chain(bomb_foundry::SECTIONS.iter().copied())
        {
            let k = key.to_string();
            sections = sections.child(
                Button::new(SharedString::from(format!("section-{key}")))
                    .ghost()
                    .small()
                    .label(key)
                    .on_click(cx.listener(move |v, _, window, cx| {
                        if v.apply_edit(cx) {
                            v.section = k.clone();
                            v.sync_editor(window, cx);
                        }
                    })),
            );
        }
        let mut modes = div().flex().flex_wrap().gap_1();
        for mode in bomb_foundry::MODES {
            let mode = *mode;
            let active = self.document.contract.0["operatingMode"] == mode;
            modes = modes.child(
                Button::new(SharedString::from(format!("mode-{mode}")))
                    .ghost()
                    .small()
                    .label(format!(
                        "{}{}",
                        if active { "✓ " } else { "" },
                        mode.replace('-', " ")
                    ))
                    .on_click(cx.listener(move |v, _, w, cx| {
                        if v.apply_edit(cx) {
                            v.document.contract.0["operatingMode"] = serde_json::json!(mode);
                            v.sync_editor(w, cx);
                            v.autosave(cx);
                        }
                    })),
            );
        }
        div().flex().flex_col().gap_3().child(modes).child(Textarea::new(&self.request)).child(div().flex().gap_2()
            .child(Button::new("draft-fast").label("Fast Draft").on_click(cx.listener(|v,_,w,cx|v.draft(false,w,cx))))
            .child(Button::new("draft-full").label("Full Project").on_click(cx.listener(|v,_,w,cx|v.draft(true,w,cx))))
            .child(Button::new("refine").ghost().label("Refine with selected model").disabled(self.busy).on_click(cx.listener(|v,_,_,cx|v.refine(false,cx)))))
            .child(div().text_sm().text_color(ui.text_muted).child("Drafts are local. Refine sends the contract to your selected provider. References are not automatically verified."))
            .child(div().flex().gap_4().child(sections).child(div().flex_1().min_w_0().flex().flex_col().gap_2()
                .child(div().text_lg().child(self.section.clone())).child(Textarea::new(&self.editor))
                .child(div().flex().gap_2().child(Button::new("apply-section").label("Apply edits").on_click(cx.listener(|v,_,_,cx|{v.apply_edit(cx);})))
                    .child(Button::new("regen-section").ghost().label("Regenerate section").disabled(self.busy).on_click(cx.listener(|v,_,_,cx|v.refine(true,cx))))
                    .child(Button::new("restore-section").ghost().label("Restore section").on_click(cx.listener(|v,_,w,cx|{v.document.contract.0[&v.section]=v.baseline.contract.0[&v.section].clone();v.sync_editor(w,cx);v.autosave(cx);}))))
                .child(Button::new("seed-graph").ghost().label("Open contract as graph →").on_click(cx.listener(|v,_,w,cx|{if v.apply_edit(cx){v.document.graph=v.document.contract.graph();v.tab=1;v.selected=0;v.section="prompt".into();v.sync_editor(w,cx);v.autosave(cx);}})))
                .child(Button::new("use-contract").label("Use prompt in composer").on_click(cx.listener(|v,_,_,cx|v.handoff(false,cx))))
                .child(div().text_sm().text_color(ui.text_muted).child(self.document.contract.explain())))).into_any_element()
    }
    fn graph(&self, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let mut presets = div().flex().flex_wrap().gap_1();
        for name in bomb_foundry::TEMPLATES {
            let name = *name;
            presets = presets.child(
                Button::new(SharedString::from(format!("template-{name}")))
                    .ghost()
                    .small()
                    .label(name)
                    .on_click(cx.listener(move |v, _, w, cx| {
                        v.document.graph = bomb_foundry::template(name);
                        v.selected = 0;
                        v.section = "prompt".into();
                        v.sync_editor(w, cx);
                        v.autosave(cx);
                    })),
            );
        }
        let mut cards = div()
            .id("foundry-canvas")
            .relative()
            .h(px(480.))
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .bg(ui.bg)
            .border_1()
            .border_color(ui.border)
            .rounded(px(10.));
        if self.list_view {
            cards = cards.overflow_y_scroll();
        }
        if !self.list_view {
            let graph = self.document.graph.clone();
            let zoom = self.zoom;
            let pan = self.pan;
            let normal = ui.text_faint;
            let returning = ui.warning;
            cards = cards.child(
                canvas(
                    move |bounds, _, _| bounds,
                    move |_, bounds, window, _| {
                        for (i, e) in graph.edges.iter().enumerate() {
                            let Some(a) = graph.nodes.iter().find(|n| n.id == e.source) else {
                                continue;
                            };
                            let Some(b) = graph.nodes.iter().find(|n| n.id == e.target) else {
                                continue;
                            };
                            let origin = bounds.origin + pan;
                            let mut path = PathBuilder::stroke(px(1.5));
                            let end;
                            if e.kind == "sequence" {
                                let start = origin
                                    + point(
                                        px((a.position.x + 125.) * zoom),
                                        px((a.position.y + 102.) * zoom),
                                    );
                                end = origin
                                    + point(
                                        px((b.position.x + 125.) * zoom),
                                        px(b.position.y * zoom),
                                    );
                                path.move_to(start);
                                path.line_to(end);
                            } else {
                                let start = origin
                                    + point(
                                        px((a.position.x + 250.) * zoom),
                                        px((a.position.y + 40.) * zoom),
                                    );
                                end = origin
                                    + point(
                                        px((b.position.x + 250.) * zoom),
                                        px((b.position.y + 40.) * zoom),
                                    );
                                let bend = origin.x
                                    + px((a.position.x.max(b.position.x) + 290. + i as f32 * 8.)
                                        * zoom);
                                path.move_to(start);
                                path.line_to(point(bend, start.y));
                                path.line_to(point(bend, end.y));
                                path.line_to(end);
                            }
                            path.move_to(end + point(px(5.), px(-4.)));
                            path.line_to(end);
                            path.line_to(end + point(px(5.), px(4.)));
                            if let Ok(path) = path.build() {
                                window.paint_path(
                                    path,
                                    if e.kind == "loop" { returning } else { normal },
                                );
                            }
                        }
                    },
                )
                .absolute()
                .size_full(),
            );
        }
        for (i, n) in self.document.graph.nodes.iter().enumerate() {
            let node_id = n.id.clone();
            let position = n.position;
            let mut card = div()
                .id(SharedString::from(format!("stage-{}", n.id)))
                .p_3()
                .w(px(250. * self.zoom))
                .rounded(px(8.))
                .border_1()
                .border_color(if i == self.selected {
                    ui.text_muted
                } else {
                    ui.border
                })
                .bg(ui.bg)
                .cursor_pointer()
                .child(
                    Button::new(SharedString::from(format!("select-stage-{}", n.id)))
                        .ghost()
                        .small()
                        .label(format!("{}. {}", n.sequence_index + 1, n.title))
                        .on_click(cx.listener(move |v, _, w, cx| {
                            v.selected = i;
                            v.section = "prompt".into();
                            v.sync_editor(w, cx);
                        })),
                )
                .child(
                    div()
                        .text_size(px(crate::theme::Type::CAPTION))
                        .text_color(ui.text_muted)
                        .child(format!("{} · {}", n.kind, n.role)),
                )
                .child(
                    div()
                        .text_size(px(crate::theme::Type::CAPTION))
                        .text_color(ui.text_faint)
                        .child(n.purpose.clone()),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                        v.selected = i;
                        v.section = "prompt".into();
                        v.sync_editor(w, cx);
                        if !v.list_view {
                            v.drag = Some((node_id.clone(), e.position, position));
                        }
                    }),
                );
            if self.list_view {
                card = card.relative().mb_2().w_full();
            } else {
                card = card
                    .absolute()
                    .left(px(n.position.x * self.zoom) + self.pan.x)
                    .top(px(n.position.y * self.zoom) + self.pan.y);
            }
            cards = cards.child(card);
        }
        cards = cards
            .on_mouse_move(cx.listener(|v, e: &MouseMoveEvent, _, cx| {
                if let Some((id, start, position)) = &v.drag {
                    if let Some(n) = v.document.graph.nodes.iter_mut().find(|n| &n.id == id) {
                        n.position.x = position.x + (e.position.x - start.x) / px(v.zoom);
                        n.position.y = position.y + (e.position.y - start.y) / px(v.zoom);
                        cx.notify();
                    }
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|v, _, _, cx| {
                    if v.drag.take().is_some() {
                        v.autosave(cx);
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|v, _, _, cx| {
                    if v.drag.take().is_some() {
                        v.autosave(cx);
                    }
                }),
            )
            .on_scroll_wheel(cx.listener(|v, e: &ScrollWheelEvent, _, cx| {
                if !v.list_view {
                    let d = e.delta.pixel_delta(px(20.));
                    v.pan.x += d.x;
                    v.pan.y += d.y;
                    cx.stop_propagation();
                    cx.notify();
                }
            }));
        cards = cards.on_key_down(cx.listener(|v, e: &KeyDownEvent, w, cx| {
            if e.keystroke.modifiers.alt && ["up", "down"].contains(&e.keystroke.key.as_str()) {
                v.move_stage(if e.keystroke.key == "up" { -1 } else { 1 }, w, cx);
                cx.stop_propagation();
            }
        }));
        let mut link_controls = div().flex().flex_wrap().gap_1();
        if let Some(source) = self.document.graph.nodes.get(self.selected) {
            for target in self
                .document
                .graph
                .nodes
                .iter()
                .filter(|n| n.sequence_index < source.sequence_index)
            {
                for kind in ["loop", "depends-on"] {
                    let from = source.id.clone();
                    let to = target.id.clone();
                    let label = format!(
                        "{} {}",
                        if kind == "loop" {
                            "Return to"
                        } else {
                            "Depends on"
                        },
                        target.title
                    );
                    link_controls =
                        link_controls.child(
                            Button::new(SharedString::from(format!("add-{kind}-{to}")))
                                .ghost()
                                .small()
                                .label(label)
                                .on_click(cx.listener(move |v, _, w, cx| {
                                    if !v.document.graph.edges.iter().any(|e| {
                                        e.kind == kind && e.source == from && e.target == to
                                    }) {
                                        v.document.graph.edges.push(SkillEdge {
                                            id: bomb_foundry::id(),
                                            kind: kind.into(),
                                            source: from.clone(),
                                            target: to.clone(),
                                            label: Some(
                                                if kind == "loop" {
                                                    "Revise on failure"
                                                } else {
                                                    "Requires acceptance"
                                                }
                                                .into(),
                                            ),
                                            purpose: (kind == "loop").then(|| "revise".into()),
                                        });
                                        v.section = "links".into();
                                        v.sync_editor(w, cx);
                                        v.autosave(cx);
                                    }
                                })),
                        );
                }
            }
            for edge in self
                .document
                .graph
                .edges
                .iter()
                .filter(|e| e.source == source.id && e.kind != "sequence")
            {
                let id = edge.id.clone();
                let target = self
                    .document
                    .graph
                    .nodes
                    .iter()
                    .find(|n| n.id == edge.target)
                    .map(|n| n.title.as_str())
                    .unwrap_or(&edge.target);
                link_controls = link_controls.child(
                    Button::new(SharedString::from(format!("remove-link-{id}")))
                        .ghost()
                        .small()
                        .label(format!("× {} to {}", edge.kind, target))
                        .on_click(cx.listener(move |v, _, w, cx| {
                            v.document.graph.edges.retain(|e| e.id != id);
                            v.sync_editor(w, cx);
                            v.autosave(cx);
                        })),
                );
                if edge.kind == "loop" {
                    let id = edge.id.clone();
                    let source = source.id.clone();
                    link_controls = link_controls.child(
                        Button::new(SharedString::from(format!("route-link-{id}")))
                            .ghost()
                            .small()
                            .label(format!("On failure → {target}"))
                            .on_click(cx.listener(move |v, _, w, cx| {
                                v.document
                                    .bindings
                                    .entry(source.clone())
                                    .or_default()
                                    .return_edge = Some(id.clone());
                                v.section = "binding".into();
                                v.sync_editor(w, cx);
                                v.autosave(cx);
                            })),
                    );
                }
            }
        }
        let links = self
            .document
            .graph
            .edges
            .iter()
            .map(|e| {
                format!(
                    "{} {} → {}{}",
                    e.kind,
                    e.source,
                    e.target,
                    e.purpose
                        .as_ref()
                        .map(|p| format!(" · {p}"))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut fields = div().flex().flex_wrap().gap_1();
        for key in [
            "title",
            "purpose",
            "prompt",
            "exitCriteria",
            "binding",
            "links",
            "policy",
        ] {
            fields = fields.child(
                Button::new(SharedString::from(format!("node-field-{key}")))
                    .ghost()
                    .small()
                    .label(key)
                    .on_click(cx.listener(move |v, _, w, cx| {
                        if v.apply_edit(cx) {
                            v.section = key.into();
                            v.sync_editor(w, cx);
                        }
                    })),
            );
        }
        let node = self.document.graph.nodes.get(self.selected);
        let mut choices = div().flex().flex_wrap().gap_1();
        for kind in ["stage", "review", "gate"] {
            choices = choices.child(
                Button::new(SharedString::from(format!("kind-{kind}")))
                    .ghost()
                    .small()
                    .label(kind)
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if let Some(n) = v.document.graph.nodes.get_mut(v.selected) {
                            n.kind = kind.into();
                            v.autosave(cx);
                        }
                    })),
            );
        }
        let mut roles = div().flex().flex_wrap().gap_1();
        for role in [
            "intake",
            "research",
            "draft",
            "revise",
            "independent-review",
            "verify",
            "other",
        ] {
            roles = roles.child(
                Button::new(SharedString::from(format!("role-{role}")))
                    .ghost()
                    .small()
                    .label(role)
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if let Some(n) = v.document.graph.nodes.get_mut(v.selected) {
                            n.role = role.into();
                            v.autosave(cx);
                        }
                    })),
            );
        }
        let mut models = div()
            .id("stage-models")
            .max_h(px(100.))
            .overflow_y_scroll()
            .flex()
            .flex_wrap()
            .gap_1();
        for b in &self.model.read(cx).backends {
            for model in &b.models {
                let backend = b.id.clone();
                let model = model.clone();
                let label = format!("{} / {}", b.id, b.model_names.get(&model).unwrap_or(&model));
                models = models.child(
                    Button::new(SharedString::from(format!("stage-model-{backend}-{model}")))
                        .ghost()
                        .small()
                        .label(label)
                        .on_click(cx.listener(move |v, _, w, cx| {
                            if let Some(n) = v.document.graph.nodes.get(v.selected) {
                                let binding = v.document.bindings.entry(n.id.clone()).or_default();
                                binding.backend = Some(backend.clone());
                                binding.model = Some(model.clone());
                                v.section = "binding".into();
                                v.sync_editor(w, cx);
                                v.autosave(cx);
                            }
                        })),
                );
            }
        }
        div().flex().flex_col().gap_3().child(presets)
            .child(div().flex().flex_wrap().gap_1()
                .child(Button::new("add-stage").label("+ Stage").on_click(cx.listener(|v,_,w,cx|{v.document.graph.add_stage();v.selected=v.document.graph.nodes.len()-1;v.section="prompt".into();v.sync_editor(w,cx);v.autosave(cx);})))
                .child(Button::new("stage-up").ghost().label("Move up").on_click(cx.listener(|v,_,w,cx|v.move_stage(-1,w,cx))))
                .child(Button::new("stage-down").ghost().label("Move down").on_click(cx.listener(|v,_,w,cx|v.move_stage(1,w,cx))))
                .child(Button::new("stage-delete").ghost().label("Remove stage").on_click(cx.listener(|v,_,w,cx|{if let Some(n)=v.document.graph.nodes.get(v.selected){let id=n.id.clone();v.document.graph.remove_stage(&id);v.selected=0;v.sync_editor(w,cx);v.autosave(cx);}})))
                .child(Button::new("stage-view").ghost().label(if self.list_view{"Canvas"}else{"Ordered list"}).on_click(cx.listener(|v,_,_,cx|{v.list_view = !v.list_view;cx.notify();})))
                .child(Button::new("zoom-out").ghost().label("−").on_click(cx.listener(|v,_,_,cx|{v.zoom=(v.zoom-0.1).max(0.5);cx.notify();})))
                .child(Button::new("zoom-in").ghost().label("+").on_click(cx.listener(|v,_,_,cx|{v.zoom=(v.zoom+0.1).min(2.);cx.notify();})))
                .child(Button::new("rebuild-sequence").ghost().label("Rebuild sequence").on_click(cx.listener(|v,_,_,cx|{v.document.graph.rebuild_sequence();v.autosave(cx);}))))
            .child(div().flex().gap_3().child(cards).child(div().w(px(390.)).flex_shrink_0().flex().flex_col().gap_2()
                .child(div().text_lg().child(node.map(|n|n.title.clone()).unwrap_or("Select stage".into())))
                .child(choices).child(roles).child(fields).child(div().text_sm().text_color(ui.text_muted).child(self.section.clone())).child(Textarea::new(&self.editor))
                .child(Button::new("apply-stage").label("Apply edits").on_click(cx.listener(|v,_,_,cx|{v.apply_edit(cx);})))
                .child(div().text_sm().child("Model override · inherit by default")).child(models)
                .child(Button::new("inherit-model").ghost().small().label("Use run model").on_click(cx.listener(|v,_,w,cx|{if let Some(n)=v.document.graph.nodes.get(v.selected){v.document.bindings.remove(&n.id);v.sync_editor(w,cx);v.autosave(cx);}})))))
            .child(div().text_sm().text_color(ui.text_muted).child("Stage linkages · sequence follows stage order; return links repeat earlier work."))
            .child(link_controls)
            .child(div().text_size(px(crate::theme::Type::CAPTION)).child(links))
            .child(div().flex().gap_2().child(Button::new("insert-skill").ghost().label("Insert instructions").on_click(cx.listener(|v,_,_,cx|v.handoff(true,cx))))
                .child(Button::new("run-loop").label("Run loop").disabled(self.busy).on_click(cx.listener(|v,_,_,cx|v.start_run(cx)))))
            .child(div().text_sm().text_color(ui.text_muted).child("Run uses the current thread folder or creates an isolated branch for the selected project. Two returns per edge; total attempts default to 3 × stages. Human gates always pause."))
            .into_any_element()
    }
    fn move_stage(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(n) = self.document.graph.nodes.get(self.selected) {
            let id = n.id.clone();
            self.document.graph.move_stage(&id, delta);
            self.selected = self
                .document
                .graph
                .nodes
                .iter()
                .position(|n| n.id == id)
                .unwrap_or(0);
            self.sync_editor(window, cx);
            self.autosave(cx);
        }
    }
    fn runs_view(&self, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let mut view = div().flex().flex_col().gap_3();
        for r in &self.runs {
            let mut card = div()
                .p_4()
                .border_1()
                .border_color(ui.border)
                .rounded(px(10.))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_lg()
                        .child(format!("{} · {:?}", r.document.name, r.status)),
                )
                .child(div().text_sm().text_color(ui.text_muted).child(format!(
                    "Stage {} / {} · {} attempts · {}",
                    (r.cursor + 1).min(r.document.graph.nodes.len()),
                    r.document.graph.nodes.len(),
                    r.attempts.len(),
                    r.cwd
                )))
                .child(if r.status == bomb_foundry::RunStatus::Completed { "Completed".into() } else { r.note.clone() });
            let mut actions = div().flex().flex_wrap().gap_1();
            for (label, command, enabled) in [
                (
                    "Pause",
                    "pause",
                    matches!(r.status, RunStatus::Running | RunStatus::Ready),
                ),
                (
                    "Resume / retry",
                    "resume",
                    matches!(
                        r.status,
                        RunStatus::Paused | RunStatus::Blocked | RunStatus::Interrupted
                    ),
                ),
                (
                    "Approve gate",
                    "approve",
                    r.status == RunStatus::WaitingGate,
                ),
                (
                    "Extend limits",
                    "extend",
                    matches!(r.status, RunStatus::Paused | RunStatus::Blocked),
                ),
                (
                    "Stop",
                    "stop",
                    !matches!(r.status, RunStatus::Stopped | RunStatus::Completed),
                ),
            ] {
                let id = r.id.clone();
                let gate_token = (command == "approve").then(|| r.gate_token());
                actions = actions.child(
                    Button::new(SharedString::from(format!("run-{id}-{command}")))
                        .ghost()
                        .small()
                        .label(label)
                        .disabled(!enabled)
                        .on_click(cx.listener(move |v, _, _, cx| {
                            v.command(id.clone(), command, gate_token.clone(), cx)
                        })),
                );
            }
            if let Some(parent) = r.parent_thread.clone() {
                actions = actions.child(
                    Button::new(SharedString::from(format!("open-run-{}", r.id)))
                        .ghost()
                        .small()
                        .label("Open thread / approvals")
                        .on_click(cx.listener(move |v, _, _, cx| {
                            v.model.update(cx, |m, cx| {
                                m.select(uuid::Uuid::parse_str(&parent).ok(), cx);
                                m.foundry_close = true;
                                cx.notify();
                            });
                        })),
                );
            }
            if r.status == RunStatus::WaitingGate {
                if let Some(gate) = r.current() {
                    card = card.child(div().p_3().border_1().border_color(ui.border).child(
                        format!(
                            "{} — {}\nApprove only after checking:\n{}",
                            gate.title,
                            gate.purpose,
                            gate.exit_criteria.join("\n")
                        ),
                    ));
                }
            }
            card = card.child(actions);
            for a in &r.attempts {
                card = card.child(
                    div()
                        .border_t_1()
                        .border_color(ui.border)
                        .pt_2()
                        .child(format!(
                            "{} · {} / {}{}",
                            a.node_id,
                            a.backend,
                            a.model,
                            if a.invalidated { " · superseded" } else { "" }
                        ))
                        .child(
                            div().text_sm().text_color(ui.text_muted).child(
                                a.error
                                    .clone()
                                    .or_else(|| {
                                        a.result
                                            .as_ref()
                                            .map(|r| format!("{}: {}", r.outcome, r.summary))
                                    })
                                    .unwrap_or("Running…".into()),
                            ),
                        )
                        .child(
                            Button::new(SharedString::from(format!("attempt-{}", a.id)))
                                .ghost()
                                .small()
                                .label(if self.expanded_attempt.as_deref() == Some(&a.id) {
                                    "Hide output and evidence"
                                } else {
                                    "Show output and evidence"
                                })
                                .on_click({
                                    let id = a.id.clone();
                                    cx.listener(move |v, _, _, cx| {
                                        v.expanded_attempt =
                                            if v.expanded_attempt.as_ref() == Some(&id) {
                                                None
                                            } else {
                                                Some(id.clone())
                                            };
                                        cx.notify();
                                    })
                                }),
                        )
                        .when(self.expanded_attempt.as_deref() == Some(&a.id), |el| {
                            el.child(div().text_sm().child(a.output.clone())).child(
                                div()
                                    .text_size(px(crate::theme::Type::CAPTION))
                                    .text_color(ui.text_muted)
                                    .child(format!(
                                        "App-observed tool events:\n{}",
                                        a.observed_evidence.join("\n")
                                    )),
                            )
                        }),
                );
            }
            view = view.child(card);
        }
        view.into_any_element()
    }
}
impl Render for FoundryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.needs_sync {
            self.needs_sync = false;
            self.sync_editor(window, cx);
        }
        let ui = Ui::of(cx);
        let mut tabs = div().flex().items_center().gap_2().child(
            div()
                .text_xl()
                .font_weight(FontWeight::SEMIBOLD)
                .child("Foundry"),
        );
        for (i, label) in ["Contract", "Skill Loop", "Runs"].iter().enumerate() {
            tabs = tabs.child(
                Button::new(SharedString::from(format!("foundry-tab-{i}")))
                    .ghost()
                    .label(*label)
                    .on_click(cx.listener(move |v, _, w, cx| {
                        if v.tab == 2 || v.apply_edit(cx) {
                            v.tab = i;
                            v.section = if i == 0 { "goal" } else { "prompt" }.into();
                            v.sync_editor(w, cx);
                        }
                    })),
            );
        }
        tabs = tabs.child(div().flex_1()).child(
            Button::new("foundry-back")
                .ghost()
                .label("← Back")
                .on_click(cx.listener(|v, _, _, cx| {
                    v.model.update(cx, |m, cx| {
                        m.foundry_close = true;
                        cx.notify();
                    });
                })),
        );
        let mut library = div()
            .w(px(200.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_sm().child("Saved on this device"))
            .child(Input::new(&self.search));
        let q = self.search.read(cx).value().to_lowercase();
        for d in self
            .library
            .iter()
            .filter(|d| d.name.to_lowercase().contains(&q))
        {
            let d = d.clone();
            library = library.child(
                Button::new(SharedString::from(format!("doc-{}", d.id)))
                    .ghost()
                    .small()
                    .label(d.name.clone())
                    .on_click(cx.listener(move |v, _, w, cx| {
                        v.document = d.clone();
                        v.baseline = d.clone();
                        v.saved = true;
                        v.selected = 0;
                        v.sync_editor(w, cx);
                    })),
            );
        }
        library = library
            .child(
                Button::new("new-foundry")
                    .ghost()
                    .label("+ New draft")
                    .on_click(cx.listener(|v, _, w, cx| v.seed("New project".into(), w, cx))),
            )
            .child(
                Button::new("save-foundry")
                    .label(if self.saved {
                        "Save revision"
                    } else {
                        "Save on this device"
                    })
                    .on_click(cx.listener(|v, _, _, cx| {
                        if v.apply_edit(cx) {
                            v.save(cx);
                        }
                    })),
            )
            .child(
                Button::new("import-foundry")
                    .ghost()
                    .label("Import JSON")
                    .on_click(cx.listener(|v, _, _, cx| v.import(cx))),
            )
            .child(
                Button::new("export-foundry")
                    .ghost()
                    .label("Export package")
                    .on_click(cx.listener(|v, _, _, cx| v.export(cx))),
            )
            .child(
                Button::new("history-foundry")
                    .ghost()
                    .label("Revision history")
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.history = services(cx)
                            .foundry
                            .store
                            .revisions(&v.document.id)
                            .unwrap_or_default();
                        cx.notify();
                    })),
            );
        for d in &self.history {
            let d = d.clone();
            library = library.child(
                Button::new(SharedString::from(format!("revision-{}", d.revision)))
                    .ghost()
                    .small()
                    .label(format!("Restore revision {}", d.revision))
                    .on_click(cx.listener(move |v, _, w, cx| {
                        let rev = v.document.revision;
                        v.document = d.clone();
                        v.document.revision = rev;
                        v.sync_editor(w, cx);
                        v.save(cx);
                    })),
            );
        }
        let content = match self.tab {
            0 => self.contract(&ui, cx),
            1 => self.graph(&ui, cx),
            _ => self.runs_view(&ui, cx),
        };
        let mut body = div()
            .id("foundry-content")
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_3()
            .child(content);
        if let Some(pending) = &self.pending {
            body = body.child(
                div()
                    .p_4()
                    .border_1()
                    .border_color(ui.border)
                    .child("Proposed contract · current version is unchanged")
                    .child(
                        div()
                            .flex()
                            .gap_4()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .child("Current")
                                    .child(self.document.contract.markdown()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .child("Proposed")
                                    .child(pending.contract.markdown()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("accept-refinement")
                                    .label("Accept changes")
                                    .on_click(cx.listener(|v, _, w, cx| {
                                        if let Some(d) = v.pending.take() {
                                            v.document = d;
                                            v.sync_editor(w, cx);
                                            v.autosave(cx);
                                        }
                                    })),
                            )
                            .child(
                                Button::new("discard-refinement")
                                    .ghost()
                                    .label("Discard")
                                    .on_click(cx.listener(|v, _, _, cx| {
                                        v.pending = None;
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        }
        div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .p_4()
            .gap_3()
            .bg(ui.bg)
            .text_color(ui.text)
            .child(tabs)
            .when_some(self.message.clone(), |el, msg| {
                el.child(div().text_sm().text_color(ui.text_muted).child(msg))
            })
            .when(self.busy, |el| {
                el.child("Working with provider… original draft remains available")
            })
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap_4()
                    .child(library)
                    .child(body),
            )
    }
}
