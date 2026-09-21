//! The composer: a pill with a growing textarea, attachment tray, model and
//! mode pickers, attach, and a send button that morphs into stop while a
//! turn runs. Enter sends, Shift+Enter inserts a newline, Shift+Tab cycles
//! the approval mode.

use crate::views::button::Button;
use std::sync::Arc;

use base64::Engine;
use bomb_core::services::ImageInput;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::progress::ProgressCircle;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Disableable, Icon, Side, Sizable, Selectable};
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
    routing_serial: u64,
    routing_pending: Option<String>,
    routing_result: Option<(String, Result<bomb_core::services::model_suggestions::Evaluation, String>)>,
    routing_suggestion: Option<(String, bomb_core::services::model_suggestions::Suggestion)>,
    routing_feedback: Option<(Option<uuid::Uuid>, String)>,
    routing_bypass: bool,
    routing_choice: Option<bomb_core::services::model_suggestions::Candidate>,
    routing_effort: String,
    routing_details: bool,
    model: Entity<AppModel>,
    input: Entity<TextareaState>,
    attachments: Vec<Attachment>,
    drag_over: bool,
    model_menu_open: bool,
    slash_index: usize,
    slash_dismissed: bool,
    model_search: Entity<InputState>,
    provider_filter: Option<String>,
    speed: Entity<super::speed::SpeedSelector>,
    location: Entity<super::work_location::WorkLocation>,
    sent_draft: Option<(String, Vec<Attachment>, u64)>,
    failed_draft: Option<(String, Vec<Attachment>)>,
    destination_busy: bool,
    destination_message: Option<String>,
    destination_init: Option<String>,
    destination_ready: Option<(String,String)>,
    review_loop_busy: bool,
    review_loop_result: Option<(String, uuid::Uuid)>,
    foundry_busy: bool,
    foundry_setup: bool,
    foundry_more: bool,
    foundry_memory: Vec<grok_memory::MemoryEntry>,
    foundry_memory_open: bool,
    foundry_memory_loading: bool,
    foundry_memory_search: Entity<InputState>,
    foundry_pending_sources: Vec<String>,
    foundry_source_loading: bool,
    foundry_depth: String,
    foundry_target: String,
    foundry_work_type: String,
    foundry_autonomy: Entity<InputState>,
    foundry_sources: Vec<(Entity<InputState>, String)>,
    foundry_result: Option<(String, String, Option<uuid::Uuid>)>,
    foundry_undo: Option<(String, String, Option<uuid::Uuid>)>,
    foundry_message: Option<String>,
    placeholder: &'static str,
}

impl ComposerView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Do anything…")
                .auto_grow(3, 14)
                .submit_on_enter(true)
        });
        cx.subscribe_in(
            &input,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => {
                    this.slash_index = 0;
                    this.slash_dismissed = false;
                    cx.notify();
                }
                InputEvent::PressEnter { shift: false, .. } => this.send(window, cx),
                _ => {}
            },
        )
        .detach();
        let memory_search=cx.new(|cx|InputState::new(window,cx).placeholder("Search saved memories…"));
        cx.observe(&memory_search,|_,_,cx|cx.notify()).detach();
        let speed = cx.new(|cx| super::speed::SpeedSelector::new(model.clone(), cx));
        let location=cx.new(|cx|super::work_location::WorkLocation::new(model.clone(),cx));
        Self {
            routing_serial: 0,
            routing_pending: None,
            routing_result: None,
            routing_suggestion: None,
            routing_feedback: None,
            routing_bypass: false,
            routing_choice: None,
            routing_effort: String::new(),
            routing_details: false,
            location,
            speed,
            model,
            input,
            attachments: Vec::new(),
            drag_over: false,
            model_menu_open: false,
            slash_index: 0,
            slash_dismissed: false,
            model_search: cx.new(|cx| InputState::new(window, cx).placeholder("Search models…")),
            provider_filter: None,
            sent_draft: None,
            failed_draft: None,
            destination_busy: false,
            destination_message: None,
            destination_init: None,
            destination_ready: None,
            review_loop_busy: false,
            review_loop_result: None,
            foundry_busy: false,
            foundry_setup: false,
            foundry_more: false,
            foundry_memory: Vec::new(),
            foundry_memory_open: false,
            foundry_memory_loading: false,
            foundry_memory_search: memory_search,
            foundry_pending_sources: Vec::new(),
            foundry_source_loading: false,
            foundry_depth: "fast-draft".into(),
            foundry_target: String::new(),
            foundry_work_type: String::new(),
            foundry_autonomy: cx.new(|cx|InputState::new(window,cx).placeholder("e.g. Build and test locally; ask before deploying")),
            foundry_sources: Vec::new(),
            foundry_result: None,
            foundry_undo: None,
            foundry_message: None,
            placeholder: "Do anything…",
        }
    }

    fn run_foundry(&mut self, cx: &mut Context<Self>) {
        let original = self.input.read(cx).value().to_string();
        if self.foundry_busy || original.trim().is_empty() { return; }
        self.foundry_target = match self.model.read(cx).prefs.backend.as_str() {"grok"=>"grok-build","codex"=>"openai-codex","claude"=>"claude-code",_=>"general-assistant"}.into();
        let options = bomb_foundry::PromptOptions {
            depth: self.foundry_depth.clone(), target: self.foundry_target.clone(), work_type: self.foundry_work_type.clone(),
            autonomy: self.foundry_autonomy.read(cx).value().to_string(),
            sources: self.foundry_sources.iter().map(|(input,role)|bomb_foundry::PromptSource {value:input.read(cx).value().to_string(),role:role.clone()}).collect(),
        };
        if let Err(error) = options.document(&original) { self.foundry_message=Some(error);cx.notify();return; }
        self.foundry_setup = false;
        let m = self.model.read(cx);
        let backend = m.prefs.backend.clone();
        let model = m.effective_model();
        let thread = m.selected;
        let state = crate::runtime::services(cx);
        self.foundry_busy = true;
        self.foundry_message = None;
        let weak = cx.entity().downgrade();
        crate::runtime::spawn_service(cx, async move {
            let result = bomb_core::foundry::generate_prompt(state, original.clone(), backend, model, options).await;
            (original, result)
        }, move |(original, result), cx| {
            let _ = weak.update(cx, |v,cx| {
                v.foundry_busy = false;
                match result {
                    Ok(prompt) => v.foundry_result = Some((original, prompt, thread)),
                    Err(error) => v.foundry_message = Some(format!("Couldn’t enhance prompt: {error}")),
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    fn destination_panel(&self, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let mut panel=div().w_full().mb_2().p_3().rounded_lg().bg(ui.bg).border_1().border_color(ui.border).flex().flex_col().gap_2()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Set up this chat"))
            .child(div().text_sm().child(self.destination_message.clone().unwrap_or_default()));
        if let Some(root)=self.destination_init.clone() {
            panel=panel.child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child(root.clone()))
                .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child("Initialize Git creates an empty initial commit. Existing files stay uncommitted and won’t appear in the isolated branch until you commit them."))
                .child(Button::new("destination-init").small().label("Initialize Git").disabled(self.destination_busy).on_click(cx.listener(move |v,_,_,cx| {
                    v.destination_busy=true;let root=root.clone();let weak=cx.entity().downgrade();
                    crate::runtime::spawn_service(cx,async move {bomb_core::services::thread_setup::initialize(&root).await},move |result,cx|{let _=weak.update(cx,|v,cx|{
                        v.destination_busy=false;v.destination_init=None;
                        v.destination_message=Some(match result {Ok(())=>"Git is ready. Press Send to start your thread.".into(),Err(e)=>e});cx.notify();
                    });});cx.notify();
                })));
        }
        panel=panel.when(self.failed_draft.is_some(),|el|el.child(Button::new("restore-unsent").ghost().small().label("Restore previous unsent prompt").on_click(cx.listener(|v,_,window,cx| {
            if let Some((text,images))=v.failed_draft.take() {let current=v.input.read(cx).value().to_string();let current_images=std::mem::replace(&mut v.attachments,images);v.input.update(cx,|s,cx|s.set_value(text,window,cx));if !current.is_empty() || !current_images.is_empty() {v.failed_draft=Some((current,current_images));}}cx.notify();
        }))));
        panel.child(div().flex().flex_wrap().gap_2()
            .child(Button::new("destination-folder").ghost().small().label("Choose existing folder…").disabled(self.destination_busy).on_click(cx.listener(|v,_,_,cx|{v.model.update(cx,|m,cx|m.open_project(cx));v.destination_init=None;v.destination_message=Some("Select your project folder, then press Send.".into());cx.notify();})))
            .child(Button::new("destination-create").ghost().small().label("Create new project…").disabled(self.destination_busy).on_click(cx.listener(|v,_,_,cx|{v.model.update(cx,|m,cx|m.create_project(cx));v.destination_init=None;v.destination_message=Some("Choose a name and location. A new folder and Git repository will be created, then press Send.".into());cx.notify();})))
            .child(Button::new("destination-temporary").ghost().small().label("Use temporary chat").disabled(self.destination_busy).on_click(cx.listener(|v,_,_,cx|{v.model.update(cx,|m,cx|m.temporary_chat(cx));v.destination_init=None;v.destination_message=Some("Preparing a private temporary folder. Press Send when it appears below.".into());cx.notify();})))
            .child(Button::new("destination-dismiss").ghost().small().label("Dismiss").on_click(cx.listener(|v,_,_,cx|{v.destination_message=None;cx.notify();}))))
            .into_any_element()
    }

    fn add_foundry_source(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.foundry_sources.len()>=20 {self.foundry_message=Some("You can attach up to 20 sources.".into());return;}
        if self.foundry_sources.iter().any(|(s,_)|s.read(cx).value().as_ref()==value) {return;}
        let input=cx.new(|cx| {let mut s=InputState::new(window,cx);s.set_value(value,window,cx);s});
        self.foundry_sources.push((input,"supporting-context".into()));cx.notify();
    }
    fn pick_foundry_sources(&mut self, cx: &mut Context<Self>) {
        let rx=cx.prompt_for_paths(PathPromptOptions{files:true,directories:false,multiple:true,prompt:Some("Attach sources".into())});
        cx.spawn(async move |weak,cx| {
            let Ok(Ok(Some(paths)))=rx.await else {return;};
            let _=weak.update(cx,|v,cx| {
                v.foundry_source_loading=true;cx.notify();
                let callback=weak.clone();
                crate::runtime::spawn_service(cx,async move {
                    let mut results=Vec::new();for path in paths.into_iter().take(20) {results.push(bomb_core::services::prompt_sources::read(&path).await);}results
                },move |results,cx| {let _=callback.update(cx,|v,cx| {v.foundry_source_loading=false;for result in results {match result {Ok(s)=>v.foundry_pending_sources.push(s),Err(e)=>v.foundry_message=Some(format!("Could not attach source: {e}"))}}cx.notify();});});
            });
        }).detach();
    }
    fn load_foundry_memory(&mut self, cx: &mut Context<Self>) {
        self.foundry_memory_open = !self.foundry_memory_open;
        if !self.foundry_memory_open {cx.notify();return;}
        self.foundry_memory_loading=true;let state=crate::runtime::services(cx);let weak=cx.entity().downgrade();
        crate::runtime::spawn_service(cx,async move {bomb_core::services::memory_list(&state,None).await},move |result,cx|{let _=weak.update(cx,|v,cx|{v.foundry_memory_loading=false;match result {Ok(entries)=>v.foundry_memory=entries,Err(e)=>v.foundry_message=Some(e)}cx.notify();});});cx.notify();
    }

    fn foundry_choice(&self, field: &'static str, selected: &str, options: &'static [(&'static str, &'static str)], cx: &Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        let current = selected.to_owned();
        let label = if !options.iter().any(|(id,_)| *id == current) { "Or select other…" } else { bomb_foundry::label(options,&current) };
        Button::new(field).outline().small().w_full().label(label.to_owned()).dropdown_caret(true)
            .dropdown_menu(move |mut menu,_,_| {
                for (id,label) in options {
                    let weak=weak.clone();let value=id.to_string();
                    menu=menu.item(PopupMenuItem::new(*label).checked(current==*id).on_click(move |_,_,cx| {
                        let _=weak.update(cx,|v,cx| { match field { "foundry-target"=>v.foundry_target=value.clone(), "foundry-work-type"=>v.foundry_work_type=value.clone(), _=>{} } cx.notify(); });
                    }));
                }
                menu
            }).into_any_element()
    }
    fn foundry_panel(&self, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let backend=self.model.read(cx).prefs.backend.clone();
        let target=match backend.as_str() {"grok"=>"Grok Build","claude"=>"Claude Code","codex"=>"OpenAI Codex",_=>"Current provider"};
        let mut work=div().flex().flex_wrap().gap_2();
        for (id,label,icon) in [("research-only","Research",Lucide::Search),("repository-audit","Audit",Lucide::ShieldCheck),("planning-docs","Planning",Lucide::ListTodo),("implementation","Implementation",Lucide::Code)] {
            work=work.child(Button::new(SharedString::from(format!("enhance-work-{id}"))).outline().small().icon(icon).label(label).selected(self.foundry_work_type==id).when(self.foundry_work_type==id,|b|b.primary()).on_click(cx.listener(move |v,_,_,cx|{v.foundry_work_type=id.into();cx.notify();})));
        }
        let other=self.foundry_choice("foundry-work-type",&self.foundry_work_type,&[
            ("implementation-plus-verification","Implementation + verification"),("production-readiness-audit","Production readiness"),("refactor-migration","Refactor / migration"),("content-creation","Content creation"),("mixed-workflow","Mixed workflow")],cx);
        let context_label = if self.foundry_more { "Additional context".to_owned() } else if !self.foundry_sources.is_empty() || !self.foundry_autonomy.read(cx).value().is_empty() { "Context added · Edit".to_owned() } else { "Add context · optional".to_owned() };
        let mut panel=div().id("foundry-intake-body").w_full().max_h(px(300.)).overflow_y_scroll().p_4().flex().flex_col().gap_3()
            .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child("How much detail?"))
            .child(div().flex().gap_2()
                .child(Button::new("foundry-fast").outline().small().flex_1().icon(Lucide::Zap).label("Fast Draft").when(self.foundry_depth=="fast-draft",|b|b.primary()).on_click(cx.listener(|v,_,_,cx|{v.foundry_depth="fast-draft".into();cx.notify();})))
                .child(Button::new("foundry-full").outline().small().flex_1().icon(Lucide::Layers).label("Full Project").when(self.foundry_depth=="full-project",|b|b.primary()).on_click(cx.listener(|v,_,_,cx|{v.foundry_depth="full-project".into();cx.notify();}))))
            .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child(if self.foundry_depth=="fast-draft" {"Focused instructions for a smaller task."} else {"Detailed phases, checks, and a clear handoff."}))
            .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child("What are you doing?"))
            .child(work).child(other)
            .child(div().flex().items_center().gap_2().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted)
                .child(crate::views::brand::brand_mark(&backend,14.,true,ui)).child(format!("For {target} · follows your selected provider")))
            .child(Button::new("foundry-more").ghost().small().icon(if self.foundry_more {Lucide::ChevronDown} else {Lucide::ChevronRight}).label(context_label).on_click(cx.listener(|v,_,_,cx|{v.foundry_more = !v.foundry_more;cx.notify();})));
        if self.foundry_more {
            panel=panel
            .child(div().text_sm().child("Approval notes (optional)"))
            .child(Input::new(&self.foundry_autonomy))
            .child(div().flex().items_center().gap_2().child(div().text_sm().child("Sources of truth (optional)"))
                .child(Button::new("foundry-add-source").ghost().small().label("+ Add source").disabled(self.foundry_sources.len()>=20).on_click(cx.listener(|v,_,window,cx| {
                    let input=cx.new(|cx|InputState::new(window,cx).placeholder("URL, file path, reference text or requirement"));
                    v.foundry_sources.push((input,"supporting-context".into()));cx.notify();
                }))));
            panel=panel.child(div().flex().flex_wrap().gap_2()
                .child(Button::new("foundry-file").outline().small().icon(Lucide::Paperclip).label(if self.foundry_source_loading {"Adding files…"} else {"Attach files"}).disabled(self.foundry_source_loading || self.foundry_sources.len()>=20).on_click(cx.listener(|v,_,_,cx|v.pick_foundry_sources(cx))))
                .child(Button::new("foundry-memory").outline().small().icon(Lucide::Brain).label("From memory").disabled(self.foundry_memory_loading).on_click(cx.listener(|v,_,_,cx|v.load_foundry_memory(cx)))));
            if self.foundry_memory_open {
                panel=panel.child(Input::new(&self.foundry_memory_search));
                let query=self.foundry_memory_search.read(cx).value().to_lowercase();
                let entries: Vec<_>=self.foundry_memory.iter().filter(|m|format!("{} {} {}",m.scope,m.content,m.tags.join(" ")).to_lowercase().contains(&query)).collect();
                if entries.is_empty() { panel=panel.child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child(if self.foundry_memory_loading {"Loading saved memories…"} else {"No matching saved memories."})); }
                let mut list=div().id("enhance-memory-list").max_h(px(150.)).overflow_y_scroll().flex().flex_col().gap_1();
                for entry in entries.iter().take(50) {
                    let value=format!("Saved Bomb Code memory [{}] (user-selected reference):\n{}",entry.scope,entry.content);
                    let preview=format!("{} · {}",entry.scope,entry.content.chars().take(90).collect::<String>());
                    list=list.child(Button::new(SharedString::from(format!("pick-memory-{}",entry.id))).ghost().small().icon(Lucide::Plus).label(preview).disabled(self.foundry_sources.len()>=20 || value.len()>20_000).on_click(cx.listener(move |v,_,window,cx|{v.add_foundry_source(value.clone(),window,cx);v.foundry_memory_open=false;cx.notify();})));
                }
                panel=panel.child(list);
            }
        for (i,(input,role)) in self.foundry_sources.iter().enumerate() {
            let value=input.read(cx).value().to_string();
            let snapshot=value.starts_with("File reference:") || value.starts_with("Saved Bomb Code memory");
            let source_view=if snapshot {
                let title=value.lines().next().unwrap_or("Source").to_string();
                let note=if value.contains("Contents not included:") {"File reference · contents not included".to_owned()} else {format!("Included · {} characters",value.chars().count())};
                div().flex().flex_col().gap_1().text_size(px(crate::theme::Type::SMALL)).child(div().overflow_hidden().text_ellipsis().whitespace_nowrap().child(title)).child(div().text_color(ui.text_faint).child(note)).into_any_element()
            } else {Input::new(input).into_any_element()};
            let weak=cx.entity().downgrade();let selected=role.clone();
            panel=panel.child(div().flex().items_center().gap_1()
                .child(div().flex_1().min_w_0().child(source_view))
                .child(Button::new(SharedString::from(format!("source-role-{i}"))).ghost().small().label(bomb_foundry::label(bomb_foundry::SOURCE_ROLES,role).to_owned()).dropdown_menu(move |mut menu,_,_| {
                    for (id,label) in bomb_foundry::SOURCE_ROLES {
                        let weak=weak.clone();let role=id.to_string();
                        menu=menu.item(PopupMenuItem::new(*label).checked(selected==*id).on_click(move |_,_,cx| {let _=weak.update(cx,|v,cx|{if let Some(source)=v.foundry_sources.get_mut(i) {source.1=role.clone();}cx.notify();});}));
                    } menu
                }))
                .child(Button::new(SharedString::from(format!("source-remove-{i}"))).ghost().small().label("Remove").on_click(cx.listener(move |v,_,_,cx| {if i<v.foundry_sources.len() {v.foundry_sources.remove(i);}cx.notify();}))));
        }
            panel=panel.child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child("Selected text files and memory are included. Binary or large files are linked for later inspection."));
        }
        let card=div().w_full().max_w(px(520.)).rounded(px(16.)).bg(linear_gradient(145., linear_color_stop(if ui.dark {hsla(0.64,0.10,0.15,0.98)} else {hsla(0.64,0.15,0.99,1.)},0.), linear_color_stop(if ui.dark {hsla(0.64,0.07,0.09,0.98)} else {hsla(0.64,0.12,0.95,1.)},1.))).border_1().border_color(ui.border).shadow_lg().overflow_hidden().flex().flex_col()
            .child(div().flex().items_center().gap_2().px_4().py_3().border_b_1().border_color(ui.border)
                .child(Icon::from(Lucide::Sparkles).size(px(16.)).text_color(ui.text_muted))
                .child(div().flex_1().text_sm().font_weight(FontWeight::SEMIBOLD).child("Enhance Prompt"))
                .child(Button::new("foundry-close").ghost().small().icon(Lucide::X).label("Close").on_click(cx.listener(|v,_,_,cx|{v.foundry_setup=false;cx.notify();}))))
            .child(panel)
            .child(div().flex().items_center().flex_wrap().gap_3().px_4().py_3().border_t_1().border_color(ui.border)
                .child(div().flex_1().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child(if self.foundry_work_type.is_empty() {"Choose a work type to continue."} else {"Review the result before sending."}))
                .child(Button::new("foundry-generate").primary().small().icon(Lucide::Sparkles).label("Enhance Prompt").disabled(self.foundry_source_loading || self.foundry_work_type.is_empty() || self.input.read(cx).value().trim().is_empty()).on_click(cx.listener(|v,_,_,cx|v.run_foundry(cx)))));
        div().w_full().max_w(px(Layout::COMPOSER_MAX)).flex().justify_end().mb_2().child(card).into_any_element()
    }

    fn slash_catalog(&self, cx: &App) -> serde_json::Value {
        let m = self.model.read(cx);
        m.selected_thread()
            .filter(|t| {
                let t = t.read(cx);
                t.meta.backend == m.prefs.backend
                    && t.provider_commands.get("backend").and_then(|v| v.as_str())
                        == Some(m.prefs.backend.as_str())
            })
            .map(|t| t.read(cx).provider_commands.clone())
            .or_else(|| m.backend_info(&m.prefs.backend).and_then(|b| b.commands.as_ref()).map(|commands| serde_json::json!({"backend":m.prefs.backend,"commands":commands})))
            .unwrap_or_default()
    }

    fn slash_matches(&self, cx: &App) -> Vec<SlashCommand> {
        if self.slash_dismissed {
            return Vec::new();
        }
        slash_matches(
            self.input.read(cx).value().as_ref(),
            &self.slash_catalog(cx),
        )
    }

    fn choose_slash(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |s, cx| s.set_value(format!("/{name} "), window, cx));
        self.slash_dismissed = true;
        self.focus(window, cx);
        cx.notify();
    }

    fn slash_menu(&self, ui: &Ui, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.slash_dismissed || slash_query(self.input.read(cx).value().as_ref()).is_none() {
            return None;
        }
        let catalog = self.slash_catalog(cx);
        let commands = self.slash_matches(cx);
        let selected = self.slash_index.min(commands.len().saturating_sub(1));
        let backend = self.model.read(cx).prefs.backend.clone();
        let mut menu = div()
            .id("provider-slash-menu")
            .w_full()
            .max_h(px(260.))
            .overflow_y_scroll()
            .mb_2()
            .p_2()
            .rounded(px(12.))
            .border_1()
            .border_color(ui.border)
            .bg(ui.bg)
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .text_size(px(crate::theme::Type::CAPTION))
                    .text_color(ui.text_faint)
                    .child(super::brand::brand_mark(&backend, 13., true, ui))
                    .child("Commands · ↑ ↓ to navigate · Tab or Enter to select"),
            );
        if commands.is_empty() {
            menu = menu.child(div().px_2().py_2().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted)
                .child(if catalog.is_null() { "Commands load when this provider connects. You can still type a command and send it." } else { "No matching commands" }));
        }
        for (index, command) in commands.into_iter().enumerate() {
            let name = command.name.clone();
            menu =
                menu.child(
                    div()
                        .id(SharedString::from(format!("slash-{}", command.name)))
                        .px_2()
                        .py_2()
                        .rounded(px(7.))
                        .cursor_pointer()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .when(index == selected, |el| el.bg(ui.selected_bg()))
                        .hover(|s| s.bg(ui.hover))
                        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.choose_slash(&name, window, cx)
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .text_size(px(crate::theme::Type::SMALL))
                                .text_color(ui.text)
                                .child(format!("/{}", command.name))
                                .when(index == selected, |el| el.child("↵")),
                        )
                        .child(
                            div()
                                .text_size(px(crate::theme::Type::CAPTION))
                                .text_color(ui.text_muted)
                                .child(command.description),
                        )
                        .when_some(command.hint, |el, hint| {
                            el.child(
                                div()
                                    .text_size(px(crate::theme::Type::CAPTION))
                                    .text_color(ui.text_faint)
                                    .child(hint),
                            )
                        }),
                );
        }
        Some(menu.into_any_element())
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| s.focus(window, cx));
    }

    /// Prefill the input (empty-state suggestions) and focus it.
    pub fn set_text(&self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |s, cx| s.set_value(text.to_string(), window, cx));
        self.focus(window, cx);
    }

    pub fn can_retry(&self, text: &str, cx: &App) -> bool {
        let draft = self.input.read(cx).value();
        (draft.trim().is_empty() || draft.trim() == text.trim()) && self.attachments.is_empty()
            && !self.model.read(cx).starting && !self.destination_busy && !self.review_loop_busy
    }

    pub fn retry_prompt(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_retry(text, cx) {
            self.model.update(cx, |m, cx| {
                m.toast(ToastKind::Warning, "Retry keeps your current draft safe. Clear it or send it first.");
                cx.notify();
            });
            return;
        }
        self.set_text(text, window, cx);
        self.send(window, cx);
    }

    /// Context usage ring: tokens used vs the model's window; red past 85%.
    fn context_ring(&self, ui: &Ui, cx: &mut Context<Self>) -> Option<AnyElement> {
        let m = self.model.read(cx);
        let t = m.selected_thread()?;
        let used = t.read(cx).thread.context_tokens?;
        let window_tokens = context_window(&m.prefs.backend, &m.effective_model());
        let frac = (used as f32 / window_tokens as f32).clamp(0.0, 1.0);
        let pct = (frac * 100.0).round() as u32;
        let color = if frac >= 0.9 {
            ui.danger
        } else if frac >= 0.75 {
            ui.warning
        } else {
            ui.text_muted
        };
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
                .text_size(px(crate::theme::Type::CAPTION))
                .text_color(color)
                .hover(move |s| s.bg(hover))
                .child(
                    div().size(px(16.)).child(
                        ProgressCircle::new("ctx-ring")
                            .value(pct as f32)
                            .color(color),
                    ),
                )
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

    fn run_review_loop(&mut self, cx: &mut Context<Self>) {
        if self.review_loop_busy || self.foundry_busy { return; }
        if !self.attachments.is_empty() {
            self.foundry_message=Some("Review loops use text instructions. Add file paths to your prompt, or send image attachments in a normal chat first.".into());
            cx.notify(); return;
        }
        let request=self.input.read(cx).value().to_string();
        if request.trim().is_empty() { return; }
        let m=self.model.read(cx);
        if m.starting || !m.model_ready() {return;}
        let thread=m.selected_thread();
        if thread.as_ref().is_some_and(|t|t.read(cx).thread.presence.turn_active()) {return;}
        let parent=thread.as_ref().map(|t|t.read(cx).meta.id.clone());
        let cwd=thread.map(|t|t.read(cx).meta.cwd.clone()).or_else(||m.active_project.clone());
        let backend=m.prefs.backend.clone();
        let model=m.effective_model();
        let approval=m.prefs.mode.clone();
        let location = grok_control_core::SpawnOptions {
            backend:grok_config::Backend::from_key(&backend).unwrap_or_default(),model:Some(model.clone()),
            approval_mode:serde_json::from_value(serde_json::json!(approval)).ok(),
            isolate_worktree:m.prefs.location=="new" && !m.prefs.temporary,
            edit_checkout:m.prefs.location=="checkout" && !m.prefs.temporary,
            checkout_branch:if m.prefs.location=="branch"{m.prefs.existing_branch.clone()}else{None},
            base_ref:m.prefs.base_branch.clone(),read_only:m.prefs.read_only,..Default::default()
        };
        let origin=m.selected;
        let target=match backend.as_str(){"grok"=>"grok-build","claude"=>"claude-code","codex"=>"openai-codex",_=>"general-assistant"};
        let options=bomb_foundry::PromptOptions{depth:"full-project".into(),target:target.into(),work_type:"implementation-plus-verification".into(),autonomy:self.foundry_autonomy.read(cx).value().to_string(),sources:self.foundry_sources.iter().map(|(input,role)|bomb_foundry::PromptSource{value:input.read(cx).value().to_string(),role:role.clone()}).collect()};
        let mut document=match options.document(&request){Ok(d)=>d,Err(e)=>{self.foundry_message=Some(e);cx.notify();return;}};
        document.graph=bomb_foundry::template("plan-build-review");
        let state=crate::runtime::services(cx);
        let weak=cx.entity().downgrade();
        self.review_loop_busy=true;self.foundry_setup=false;
        self.foundry_message=Some("Starting review loop · Plan → Build → Review".into());
        crate::runtime::spawn_service(cx,async move {
            let cwd=match cwd {Some(cwd)=>cwd,None=>{
                let home=std::env::var_os("HOME").ok_or("Home directory unavailable")?;
                bomb_core::services::scratch::create(&std::path::PathBuf::from(home).join(".bombcode/chats")).await?.to_string_lossy().into_owned()
            }};
            let run=bomb_core::foundry::FoundryService::start_at(state.clone(),document,cwd,backend,model,approval,parent,Some(location)).await?;
            let threads=bomb_core::services::list_threads(&state).await.ok();
            Ok::<_,String>((run,threads))
        },move |result,cx|{let _=weak.update(cx,|v,cx|{
            v.review_loop_busy=false;
            match result {
                Ok((run,threads))=>{
                    v.foundry_message=Some("Review loop started · Open Stages & controls to follow progress.".into());
                    if v.model.read(cx).selected==origin {
                        if let Some(id)=run.parent_thread.and_then(|id|uuid::Uuid::parse_str(&id).ok()) {
                            v.review_loop_result=Some((request,id));
                            v.model.update(cx,|m,cx|{if let Some(threads)=threads {m.set_threads(threads,cx);}m.refresh_threads(cx);m.select(Some(id),cx);});
                        }
                    }
                }
                Err(error)=>v.foundry_message=Some(format!("Couldn’t start review loop: {error}. Your draft is unchanged.")),
            }
            cx.notify();
        });});cx.notify();
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
                m.toast(
                    ToastKind::Warning,
                    "The agent is still working — stop it first.",
                );
                cx.notify();
            });
            return;
        }
        if !self.model.read(cx).model_ready() {
            self.model.update(cx, |m, cx| {
                m.toast(
                    ToastKind::Warning,
                    "Refresh provider models and choose an available model before sending.",
                );
                cx.notify();
            });
            return;
        }
        if self.destination_busy || self.review_loop_busy || self.model.read(cx).starting { return; }
        let m=self.model.read(cx);
        if m.selected_thread().is_none() {
            let Some(root)=m.active_project.clone() else {
                self.destination_message=Some("Choose where this chat should work. Your prompt is kept below.".into());
                self.destination_init=None;cx.notify();return;
            };
            if m.prefs.worktree && !m.prefs.temporary && self.destination_ready.as_ref() != Some(&(root.clone(),text.clone())) {
                self.destination_busy=true;
                self.destination_message=Some("Checking project folder…".into());
                let weak=cx.entity().downgrade();
                crate::runtime::spawn_service(cx,async move {(root.clone(),text, bomb_core::services::thread_setup::check(&root).await)},move |(root,text,result),cx| {
                    let _=weak.update(cx,|v,cx| {
                        v.destination_busy=false;
                        if v.model.read(cx).active_project.as_deref()!=Some(&root) || v.model.read(cx).selected.is_some() {v.destination_message=None;cx.notify();return;}
                        match result {
                            Ok(bomb_core::services::thread_setup::Readiness::Ready)=>{v.destination_ready=Some((root,text));v.destination_message=None;v.destination_init=None;}
                            Ok(bomb_core::services::thread_setup::Readiness::NeedsGit)=>{v.destination_message=Some("This folder needs Git before you can make changes in an isolated thread. Choose a destination below; your prompt is safe.".into());v.destination_init=Some(root);}
                            Ok(bomb_core::services::thread_setup::Readiness::NeedsCommit)=>{v.destination_message=Some("This repository has no commits yet. Create an empty initial commit to enable isolated threads.".into());v.destination_init=Some(root);}
                            Err(e)=>{v.destination_message=Some(e);v.destination_init=None;}
                        }cx.notify();
                    });
                });cx.notify();return;
            }
        }
        if !std::mem::take(&mut self.routing_bypass) && self.check_model_suggestion(&text, cx) { return; }
        self.routing_suggestion=None;
        self.destination_ready=None;self.destination_message=None;self.destination_init=None;
        if self.model.read(cx).selected.is_none() { self.sent_draft=Some((text.clone(),self.attachments.clone(),self.model.read(cx).start_failure_serial)); }
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
        self.model
            .update(cx, |m, cx| m.send_prompt(text, images, cx));
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
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
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
                m.toast(
                    ToastKind::Warning,
                    format!("Not an image: {}", path.display()),
                );
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
                m.toast(
                    ToastKind::Error,
                    format!("Could not read {}: {e}", path.display()),
                );
                cx.notify();
            }),
        }
    }

    fn attach_bytes(
        &mut self,
        name: String,
        format: ImageFormat,
        bytes: Vec<u8>,
        cx: &mut Context<Self>,
    ) {
        let total: usize = self
            .attachments
            .iter()
            .map(|a| a.bytes.len())
            .sum::<usize>()
            + bytes.len();
        if self.attachments.len() >= MAX_ATTACHMENTS || total > MAX_TOTAL_BYTES {
            self.model.update(cx, |m, cx| {
                m.toast(
                    ToastKind::Warning,
                    "Attachment limit: 8 images, 12 MB total.",
                );
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
        let Some(item) = cx.read_from_clipboard() else {
            return false;
        };
        let mut took = false;
        for entry in item.entries() {
            match entry {
                ClipboardEntry::Image(img) => {
                    let n = self.attachments.len() + 1;
                    self.attach_bytes(
                        format!("pasted-{n}.{}", ext_for(img.format())),
                        img.format(),
                        img.bytes().to_vec(),
                        cx,
                    );
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
                        .child(
                            img(a.image.clone())
                                .size_full()
                                .object_fit(ObjectFit::Cover),
                        )
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
                                .child(
                                    div()
                                        .size(px(10.))
                                        .text_color(ui.on_solid)
                                        .child(Icon::from(Lucide::X)),
                                ),
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
        let trigger_label = if model.is_empty() {
            backend.clone()
        } else {
            m.model_name(&backend, &model)
        };
        let (_, effort_applies) = crate::views::brand::effort_levels(&backend);
        let eff_label = if effort_applies {
            let label = crate::views::brand::effort_label(&effort);
            if label.is_empty() { if effort.is_empty() { "Default" } else { &effort } } else { label }
        } else {
            ""
        };
        let open = self.model_menu_open;
        let trigger = Button::new("model-selector").ghost().compact().child(
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
                        .text_size(px(crate::theme::Type::SMALL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if open {
                            ui.text
                        } else {
                            Ui::alpha(ui.text, 0.9)
                        })
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
                            .text_size(px(crate::theme::Type::SMALL))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(Ui::alpha(ui.text_muted, 0.7))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(eff_label.to_owned()),
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
                    (
                        m.backends.clone(),
                        m.prefs.backend.clone(),
                        m.effective_model(),
                        m.prefs.effort.clone(),
                    )
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
                let mut col = div()
                    .flex()
                    .flex_col()
                    .w(px(304.))
                    .text_size(px(crate::theme::Type::BODY))
                    .text_color(ui.text);

                // ── tab strip ────────────────────────────────────────
                let mut tabs = div()
                    .flex()
                    .items_center()
                    .h(px(40.))
                    .px(px(4.))
                    .gap(px(2.))
                    .border_b_1()
                    .border_color(hairline);
                let tab = |id: SharedString,
                           on: bool,
                           child: AnyElement,
                           this: Entity<Self>,
                           key: Option<String>| {
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
                            el.child(
                                div()
                                    .absolute()
                                    .bottom(px(-4.))
                                    .left(px(6.))
                                    .right(px(6.))
                                    .h(px(2.))
                                    .rounded(px(1.))
                                    .bg(ui.accent),
                            )
                        })
                };
                let all_on = viewed.is_none();
                tabs = tabs.child(tab(
                    "tab-all".into(),
                    all_on,
                    div()
                        .size(px(15.))
                        .text_color(if all_on { ui.text } else { ui.text_muted })
                        .child(Icon::from(Lucide::Star))
                        .into_any_element(),
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
                        tab(
                            SharedString::from(format!("tab-{}", b.id)),
                            on,
                            mark,
                            this.clone(),
                            Some(b.id.clone()),
                        )
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
                        .child(
                            div()
                                .size(px(14.))
                                .text_color(ui.text_muted)
                                .child(Icon::from(Lucide::Search)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(&search).appearance(false).bordered(false)),
                        ),
                );

                // ── model rows ───────────────────────────────────────
                let mut list = div()
                    .flex()
                    .flex_col()
                    .py(px(4.))
                    .px(px(4.))
                    .bg(ui.ink(0.02));
                let mut any = false;
                let mut ix = 0usize;
                let selected_bg = ui.selected_bg();
                for b in &backends {
                    if let Some(v) = &viewed {
                        if v != &b.id {
                            continue;
                        }
                    }
                    let models = b.models.clone();
                    let models: Vec<String> = models
                        .into_iter()
                        .filter(|md| {
                            query.is_empty()
                                || md.to_lowercase().contains(&query)
                                || b.model_names
                                    .get(md)
                                    .is_some_and(|name| name.to_lowercase().contains(&query))
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
                        let blurb = b
                            .model_descriptions
                            .get(&md)
                            .cloned()
                            .unwrap_or_else(|| md.clone());
                        let hint = if ix < 9 {
                            Some(format!("⌘{}", ix + 1))
                        } else {
                            None
                        };
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
                                    .when(!selected && available, |el| {
                                        el.hover(move |s| s.bg(ink05))
                                    })
                                    .when(!available, |el| el.opacity(0.4))
                                    .when(available, |el| {
                                        el.cursor_pointer().on_click(move |_, _, cx| {
                                            app.update(cx, |a, cx| {
                                                a.set_backend(&bid, Some(mdl.clone()), cx)
                                            });
                                            this.update(cx, |c, cx| {
                                                c.model_menu_open = false;
                                                cx.notify();
                                            });
                                        })
                                    })
                                    .when(viewed.is_none(), |el| {
                                        el.child(crate::views::brand::brand_mark(
                                            &b.id, 13., true, &ui,
                                        ))
                                    })
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
                                                    .text_size(px(crate::theme::Type::SMALL))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(ui.text)
                                                    .child(
                                                        b.model_names
                                                            .get(&md)
                                                            .cloned()
                                                            .unwrap_or_else(|| md.clone()),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .text_size(px(crate::theme::Type::CAPTION))
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
                                                .text_size(px(crate::theme::Type::CAPTION))
                                                .font_family(ui.mono.clone())
                                                .text_color(ui.text_muted)
                                                .child(h),
                                        )
                                    })
                                    .when(selected, |el| {
                                        el.child(
                                            div()
                                                .size(px(14.))
                                                .flex_shrink_0()
                                                .text_color(ui.text)
                                                .child(Icon::from(Lucide::Check)),
                                        )
                                    }),
                            ),
                        );
                    }
                }
                if !any {
                    let loading = app.read(cx).models_loading;
                    let message = if loading {
                        "Loading models from providers…".into()
                    } else if !query.is_empty() {
                        format!("No model matches \"{query}\"")
                    } else {
                        backends
                            .iter()
                            .filter(|b| viewed.as_ref().is_none_or(|id| id == &b.id))
                            .filter_map(|b| b.model_error.as_ref().or(b.reason.as_ref()))
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    list = list.child(
                        div()
                            .px_2()
                            .py_2()
                            .text_size(px(crate::theme::Type::SMALL))
                            .text_color(ui.text_muted)
                            .whitespace_normal()
                            .child(if message.is_empty() {
                                "No provider models available.".into()
                            } else {
                                message
                            }),
                    );
                }
                list = list.child(
                    Button::new("refresh-models")
                        .ghost()
                        .small()
                        .label(if app.read(cx).models_loading {
                            "Refreshing models…"
                        } else {
                            "Refresh provider models"
                        })
                        .on_click({
                            let app = app.clone();
                            move |_, _, cx| app.update(cx, |m, cx| m.refresh_backends(cx))
                        }),
                );
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
                                .text_size(px(crate::theme::Type::CAPTION))
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
                                .text_size(px(crate::theme::Type::BODY))
                                .text_color(if !applies {
                                    ui.text_faint
                                } else if on {
                                    ui.text
                                } else {
                                    Ui::alpha(ui.text, 0.9)
                                })
                                .when(on && applies, |el| el.bg(selected_bg))
                                .when(!on && applies, |el| el.hover(move |s| s.bg(selected_bg)))
                                .when(applies, |el| {
                                    el.cursor_pointer().on_click(move |_, _, cx| {
                                        app.update(cx, |a, cx| a.set_effort(e, cx))
                                    })
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(crate::views::brand::effort_label(e)),
                                )
                                .when(e == default_level, |el| {
                                    el.child(
                                        div()
                                            .flex_shrink_0()
                                            .text_size(px(crate::theme::Type::CAPTION))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(ui.text_muted)
                                            .child("Default"),
                                    )
                                })
                                .when(on, |el| {
                                    el.child(
                                        div()
                                            .size(px(14.))
                                            .flex_shrink_0()
                                            .text_color(ui.text)
                                            .child(Icon::from(Lucide::Check)),
                                    )
                                }),
                        );
                    }
                    if cur_backend != "claude" {
                        tray = tray.child(
                            div()
                                .px(px(8.))
                                .pt(px(2.))
                                .pb(px(4.))
                                .text_size(px(crate::theme::Type::CAPTION))
                                .text_color(ui.text_faint)
                                .child("Applies to new conversations"),
                        );
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
        let label = if chosen.is_empty() {
            "mcp".to_string()
        } else {
            format!("mcp: {}", chosen.len())
        };
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
                        .text_size(px(crate::theme::Type::SMALL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(Ui::alpha(ui.text_muted, 0.7))
                        .child(label),
                )
                .dropdown_menu(move |mut menu, _, _| {
                    menu = menu.label(
                        "Attach to the new thread (auto-attach servers are always included)",
                    );
                    for n in &names {
                        let app = app.clone();
                        let name = n.clone();
                        let on = chosen.contains(n);
                        menu = menu.item(PopupMenuItem::new(n.clone()).checked(on).on_click(
                            move |_, _, cx| {
                                app.update(cx, |a, cx| a.toggle_mcp_pref(&name, cx));
                            },
                        ));
                    }
                    menu
                })
                .into_any_element(),
        )
    }

    fn mode_picker(&self, ui: &Ui, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.model.read(cx).prefs.mode.clone();
        let app = self.model.clone();
        let options = self.model.read(cx).provider_mode_options(cx);
        let (label, icon, description) = provider_mode_presentation(&mode, &options)
            .unwrap_or_else(|| {
                let (label, icon, desc) = mode_presentation(&mode);
                (label.into(), icon, desc.into())
            });
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
                    .bg(Ui::alpha(
                        if mode == "yolo" { ui.warning } else { ui.text },
                        0.06,
                    ))
                    .text_size(px(crate::theme::Type::SMALL))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if mode == "yolo" {
                        ui.warning
                    } else {
                        ui.text_muted
                    })
                    .child(Icon::from(icon).size(px(14.)))
                    .child(label)
                    .child(Icon::from(Lucide::ChevronDown).size(px(12.))),
            )
            .dropdown_menu(move |mut menu, _, _| {
                menu = menu
                    .min_w(px(320.))
                    .max_w(px(340.))
                    .check_side(Side::Right)
                    .item(PopupMenuItem::label("Approval mode"));
                for m in APPROVAL_CYCLE {
                    if m == "yolo" {
                        menu = menu
                            .separator()
                            .item(PopupMenuItem::label("Advanced · explicit opt-in"));
                    }
                    let app = app.clone();
                    let Some((label, icon, description)) = provider_mode_presentation(m, &options)
                    else {
                        continue;
                    };
                    menu = menu.item(
                        PopupMenuItem::element(move |_, cx| {
                            let ui = Ui::of(cx);
                            div()
                                .flex()
                                .items_start()
                                .gap(px(10.))
                                .py(px(6.))
                                .w(px(260.))
                                .child(
                                    div()
                                        .mt(px(2.))
                                        .size(px(16.))
                                        .text_color(if m == "yolo" {
                                            ui.warning
                                        } else {
                                            ui.text_muted
                                        })
                                        .child(Icon::from(icon).size(px(16.))),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(px(3.))
                                        .child(
                                            div()
                                                .text_size(px(crate::theme::Type::SMALL))
                                                .line_height(px(18.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(if m == "yolo" {
                                                    ui.warning
                                                } else {
                                                    ui.text
                                                })
                                                .child(label.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(crate::theme::Type::CAPTION))
                                                .line_height(px(16.))
                                                .text_color(ui.text_muted)
                                                .whitespace_normal()
                                                .child(description.clone()),
                                        ),
                                )
                        })
                        .checked(m == mode)
                        .on_click(move |_, window, cx| {
                            super::approval_mode::request(app.clone(), m, window, cx);
                        }),
                    );
                }
                menu.separator()
                    .item(PopupMenuItem::label("Shift+Tab cycles available modes"))
            })
            .into_any_element()
    }
}

/// Keep host approval enforcement while showing the connected provider's actual mode names.
pub(crate) fn provider_mode_presentation(
    mode: &str,
    catalog: &serde_json::Value,
) -> Option<(String, Lucide, String)> {
    let (label, icon, description) = mode_presentation(mode);
    let Some(options) = catalog.get("options").and_then(|v| v.as_array()) else {
        return Some((
            label.into(),
            icon,
            format!("{description} Provider mode is resolved on connection."),
        ));
    };
    let candidates: &[&str] = match mode {
        "plan" => &["plan", "planning", "read-only", "readonly"],
        "auto" => &["auto", "acceptEdits", "accept_edits", "agent"],
        "yolo" => &[
            "always_approve",
            "alwaysallow",
            "always_allow",
            "bypassPermissions",
            "yolo",
            "dontAsk",
            "agent-full-access",
        ],
        _ => &["default", "normal", "ask", "code", "agent"],
    };
    let find = |id: &str| {
        options.iter().find(|o| {
            o.get("value")
                .and_then(|v| v.as_str())
                .is_some_and(|v| v.eq_ignore_ascii_case(id))
        })
    };
    let current = if mode == "auto" {
        catalog.get("auto_value").or_else(|| catalog.get("current"))
    } else {
        catalog.get("current")
    }
    .and_then(|v| v.as_str());
    let option = current
        .filter(|id| candidates.iter().any(|c| c.eq_ignore_ascii_case(id)))
        .and_then(find)
        .or_else(|| candidates.iter().find_map(|id| find(id)))?;
    let native = option.get("name").and_then(|v| v.as_str()).unwrap_or(label);
    let desc = option
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or(description);
    // Full access remains an explicit opt-in regardless of native terminology.
    let value = option
        .get("value")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let display = if mode == "yolo" {
        "Full access"
    } else if mode == "ask" && value == "agent" {
        "Ask first"
    } else if mode == "plan" {
        "Plan"
    } else {
        native
    };
    Some((display.into(), icon, format!("{desc} {description}")))
}

/// UI names are separate from the backend's stable approval-mode identifiers.
fn mode_presentation(mode: &str) -> (&'static str, Lucide, &'static str) {
    match mode {
        "plan" => (
            "Plan",
            Lucide::BookOpen,
            "Investigate and propose changes before execution.",
        ),
        "auto" => (
            "Auto",
            Lucide::Zap,
            "Use the agent’s automatic approval policy.",
        ),
        "yolo" => (
            "Full access",
            Lucide::ShieldAlert,
            "Approve tools automatically. Deny rules still apply.",
        ),
        _ => (
            "Ask first",
            Lucide::ShieldCheck,
            "Request approval before running tools.",
        ),
    }
}

impl Render for ComposerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.apply_routing_result(window, cx);
        // Say what sending will do: on the project page a message starts a new thread.
        let placeholder = {
            let m = self.model.read(cx);
            if m.selected.is_none() && m.active_project.is_some() && !m.prefs.temporary {
                "Describe a feature or fix — sending starts a new thread on its own copy of the project…"
            } else {
                "Do anything…"
            }
        };
        if self.placeholder != placeholder {
            self.placeholder = placeholder;
            self.input.update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
        }
        if let Some((request,thread))=self.review_loop_result.take() {
            if self.model.read(cx).selected==Some(thread) && self.input.read(cx).value().as_ref()==request {
                self.input.update(cx,|s,cx|s.set_value("",window,cx));
            }
        }
        if let Some((original, prompt, thread)) = self.foundry_result.take() {
            if self.model.read(cx).selected == thread && self.input.read(cx).value().as_ref() == original {
                self.input.update(cx, |s,cx| s.set_value(prompt.clone(), window, cx));
                self.foundry_undo = Some((original, prompt, thread));
                self.foundry_message = Some("Prompt enhanced · Review before sending".into());
            } else {
                self.foundry_message = Some("Draft changed while Foundry was working; your edits were kept. Run it again when ready.".into());
            }
        }
        if self.foundry_undo.as_ref().is_some_and(|(_,generated,thread)| *thread != self.model.read(cx).selected || self.input.read(cx).value().as_ref() != generated) {
            self.foundry_undo = None;
            self.foundry_message = None;
        }
        for value in std::mem::take(&mut self.foundry_pending_sources) {self.add_foundry_source(value,window,cx);}
        if self.sent_draft.as_ref().is_some_and(|(_,_,serial)|*serial!=self.model.read(cx).start_failure_serial) {
            if let Some((text,images,_))=self.sent_draft.take() { self.failed_draft=Some((text,images)); }
            self.destination_message=Some("The thread could not start. Your unsent prompt was kept; choose a destination or retry Send.".into());
        }
        if self.failed_draft.is_some() && self.input.read(cx).value().is_empty() && self.attachments.is_empty() {
            if let Some((text,images))=self.failed_draft.take() {self.input.update(cx,|s,cx|s.set_value(text,window,cx));self.attachments=images;}
        }
        if self.model.read(cx).selected.is_some() {self.sent_draft=None;}
        if let Some((root,text))=self.destination_ready.clone() {
            let current=self.input.read(cx).value().trim().to_string();
            if self.model.read(cx).selected.is_none() && self.model.read(cx).active_project.as_deref()==Some(&root) && current==text {
                self.send(window,cx);
            } else {self.destination_ready=None;}
        }
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
            (
                busy,
                m.starting,
                branch,
                m.prefs.worktree && !m.prefs.temporary,
                t.is_some(),
            )
        };
        let new_target = self.model.read(cx).active_workspace.is_none();
        let location_label = self
            .model
            .read(cx)
            .active_workspace
            .as_deref()
            .and_then(|id| self.model.read(cx).workspaces.iter().find(|w| w.id == id))
            .map(|w| {
                if w.inline {
                    "Questions · files unchanged".into()
                } else {
                    w.name.clone()
                }
            })
            .unwrap_or_else(|| "New conversation".into());
        let has_text =
            !self.input.read(cx).value().trim().is_empty() || !self.attachments.is_empty();
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
                .child(
                    div()
                        .size(px(14.))
                        .text_color(on_solid)
                        .child(Icon::from(Lucide::ArrowUp)),
                )
                .into_any_element()
        };

        let drag_border = if self.drag_over {
            ui.accent
        } else {
            ui.pill_border()
        };
        let slash_menu = self.slash_menu(&ui, cx);
        let destination=self.destination_message.as_ref().map(|_|self.destination_panel(&ui,cx));
        let setup = self.foundry_setup.then(||self.foundry_panel(&ui,cx));
        let tray = self.tray(&ui, cx);
        let model_picker = self.model_selector(&ui, cx);
        let routing_card = self.routing_card(cx);
        let routing_control = self.routing_control(cx);
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
            .when_some(destination, |el,panel|el.child(panel))
            .when_some(setup, |el,panel|el.child(panel))
            .when(self.foundry_busy || self.foundry_message.is_some() || self.foundry_undo.is_some(), |el|el.child(div().w_full().max_w(px(Layout::COMPOSER_MAX)).flex().items_center().flex_wrap().gap_2().pb_2()
                .when(self.foundry_busy, |el|el.child(super::motion::breathe("enhance-working",0.45,div().flex().items_center().gap_2().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child(Icon::from(Lucide::Sparkles).size(px(12.))).child("Enhancing your prompt… Your draft is safe."))))
                .when_some(self.foundry_message.clone(), |el,message|el.child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child(message)))
                .when(self.foundry_undo.is_some(), |el|el.child(Button::new("undo-foundry").ghost().small().label("Undo").on_click(cx.listener(|v,_,window,cx| {
                    if let Some((original,_,_)) = v.foundry_undo.take() { v.input.update(cx,|s,cx|s.set_value(original,window,cx)); }
                    v.foundry_message = None; cx.notify();
                }))))))
            .on_action(cx.listener(|this, _: &CycleApprovalMode, _, cx| {
                this.model.update(cx, |m, cx| m.cycle_mode(cx));
                cx.stop_propagation();
            }))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if !this.input.read(cx).focus_handle(cx).is_focused(window)
                    || ev.keystroke.modifiers.shift
                {
                    return;
                }
                let commands = this.slash_matches(cx);
                if commands.is_empty() {
                    return;
                }
                match ev.keystroke.key.as_str() {
                    "down" => this.slash_index = (this.slash_index + 1) % commands.len(),
                    "up" => {
                        this.slash_index = (this.slash_index + commands.len() - 1) % commands.len()
                    }
                    "enter" | "tab" => {
                        let command = &commands[this.slash_index.min(commands.len() - 1)];
                        this.choose_slash(&command.name, window, cx);
                    }
                    "escape" => this.slash_dismissed = true,
                    _ => return,
                }
                cx.stop_propagation();
                window.prevent_default();
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" && this.foundry_setup {this.foundry_setup=false;cx.stop_propagation();cx.notify();return;}
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
                    .when_some(slash_menu, |el, menu| el.child(menu))
                    .child(div().flex().justify_center().child(
                        div().w_full().max_w(px(Layout::CONTENT_MAX)).px_6().child(routing_card)
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            // Context bar: where this message will work, attached to the top of the pill.
                            .min_h(px(38.)).flex_wrap()
                            .gap(px(Layout::SPACE_XS))
                            .mx(px(14.))
                            .px(px(8.))
                            .py(px(4.))
                            .rounded_t(px(14.))
                            .border_1()
                            .border_color(ui.pill_border())
                            .bg(ui.ink(0.035))
                            .child(footer_label(Lucide::MessageCircle, location_label, &ui))
                            .when(worktree_on, |el| {
                                el.child(footer_label(
                                    Lucide::GitBranch,
                                    if self.model.read(cx).active_workspace.as_ref().and_then(|id|self.model.read(cx).workspaces.iter().find(|w|&w.id==id)).is_some_and(|w|w.shared_checkout){"Shared checkout"}else{"Isolated branch"}.into(),
                                    &ui,
                                ))
                            })
                            .when_some(branch, |el, b| {
                                el.child(footer_label(Lucide::GitBranch, b, &ui))
                            })
                            .when(!has_thread && new_target, |el| el.child(self.location.clone()))
                            .child(div().flex_1())
                            .when(starting, |el| {
                                el.child(
                                    div()
                                        .text_size(px(crate::theme::Type::CAPTION))
                                        .text_color(ui.text_faint)
                                        .child("starting agent…"),
                                )
                            })
                            .children(context_ring),
                    )
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
                                // Message on top at full width, controls underneath.
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(6.))
                                    .pl(px(18.))
                                    .pr(px(10.))
                                    .pt(px(14.))
                                    .pb(px(8.))
                                    .child(
                                        div()
                                            .id("composer-text")
                                            .w_full()
                                            .min_w_0()
                                            .min_h(px(76.))
                                            .cursor_text()
                                            .on_click(cx.listener(|this, _, window, cx| this.focus(window, cx)))
                                            .text_size(px(Layout::BODY_SIZE))
                                            .line_height(px(Layout::BODY_LINE))
                                            .child(
                                                Textarea::new(&self.input)
                                                    .text_size(px(Layout::BODY_SIZE))
                                                    .line_height(px(Layout::BODY_LINE))
                                                    .appearance(false)
                                                    .bordered(false),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .w_full()
                                            .flex_wrap()
                                            .gap(px(4.))
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
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.pick_files(cx)
                                                    }))
                                                    .child(
                                                        div()
                                                            .size(px(16.))
                                                            .child(Icon::from(Lucide::Paperclip)),
                                                    ),
                                            )
                                            .child(mode_picker)
                                            .children(mcp_picker)
                .child(Button::new("run-foundry").ghost().small().icon(Lucide::Sparkles).rounded_full().selected(self.foundry_setup)
                    .tooltip("Rewrite this message into a clearer prompt before sending")
                    .label(if self.foundry_busy { "Enhancing…" } else { "Enhance" })
                    .disabled(self.review_loop_busy || self.foundry_busy || busy || starting || self.input.read(cx).value().trim().is_empty() || !self.model.read(cx).model_ready())
                    .on_click(cx.listener(|v,_,_,cx| {
                        if v.foundry_target.is_empty() { v.foundry_target = match v.model.read(cx).prefs.backend.as_str() { "grok"=>"grok-build", "codex"=>"openai-codex", "claude"=>"claude-code", _=>"general-assistant" }.into(); }
                        v.foundry_setup = !v.foundry_setup;v.foundry_message=None;cx.notify();
                    })))
                .child(Button::new("run-review-loop").ghost().small().icon(Lucide::Repeat).rounded_full()
                    .label(if self.review_loop_busy {"Starting loop…"} else {"Review loop"})
                    .disabled(self.review_loop_busy || self.foundry_busy || self.foundry_source_loading || busy || starting || self.input.read(cx).value().trim().is_empty() || !self.model.read(cx).model_ready())
                    .tooltip("Plan, build, independently review, and revise. Uses your current approval mode; pauses for final approval.")
                    .on_click(cx.listener(|v,_,_,cx|v.run_review_loop(cx))))
                                            .child(div().flex_1().min_w(px(8.)))
                                            .child(model_picker)
                                            .child(routing_control)
                                            .child(self.speed.clone())
                                            .child(div().w(px(6.)))
                                            .child(send_button),
                                    ),
                            ),
                    )
            )
    }
}

fn format_for_path(p: &std::path::Path) -> Option<ImageFormat> {
    match p
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase()
        .as_str()
    {
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
        .h(px(28.))
        .max_w(px(200.))
        .min_w_0()
        .gap(px(6.))
        .px(px(8.))
        .text_size(px(crate::theme::Type::BODY))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .child(div().size(px(14.)).flex_shrink_0().child(Icon::from(icon)))
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(text),
        )
        .into_any_element()
}

#[cfg(test)]
mod provider_mode_tests {
    use super::provider_mode_presentation;
    #[test]
    fn shows_provider_fallback_and_omits_unavailable_modes() {
        let options = serde_json::json!({"current":"default","auto_value":"acceptEdits","options":[
            {"value":"default","name":"Manual"}, {"value":"acceptEdits","name":"Accept edits"}, {"value":"plan","name":"Plan"}
        ]});
        assert_eq!(
            provider_mode_presentation("auto", &options).unwrap().0,
            "Accept edits"
        );
        assert_eq!(
            provider_mode_presentation("ask", &options).unwrap().0,
            "Manual"
        );
        assert!(provider_mode_presentation("yolo", &options).is_none());
    }
}

#[derive(Clone)]
struct SlashCommand {
    name: String,
    description: String,
    hint: Option<String>,
}
fn slash_query(text: &str) -> Option<&str> {
    let query = text.strip_prefix('/')?;
    (!query.chars().any(char::is_whitespace)).then_some(query)
}
fn slash_matches(text: &str, catalog: &serde_json::Value) -> Vec<SlashCommand> {
    let Some(query) = slash_query(text).map(str::to_lowercase) else {
        return Vec::new();
    };
    let mut matches: Vec<_> = catalog
        .get("commands")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| {
            let name = v.get("name")?.as_str()?.trim_start_matches('/');
            if name.is_empty() || name.chars().any(char::is_whitespace) {
                return None;
            }
            let description = v.get("description").and_then(|v| v.as_str()).unwrap_or("");
            let normalized = name.to_lowercase();
            let rank = if query.is_empty() || normalized == query {
                0
            } else if normalized.starts_with(&query) {
                1
            } else if normalized.rsplit(':').next().is_some_and(|part| part.starts_with(&query)) {
                2
            } else if normalized.split(['-', '_', ':']).any(|part| part.starts_with(&query)) {
                3
            } else if normalized.contains(&query) {
                4
            } else if description.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|word| word.starts_with(&query)) {
                5
            } else {
                return None;
            };
            Some((rank, SlashCommand {
                name: name.into(),
                description: description.into(),
                hint: v.pointer("/input/hint").and_then(|v| v.as_str()).map(str::to_string),
            }))
        })
        .collect();
    // Descriptions are a fallback for discovery, never noise beside name matches.
    if matches.iter().any(|(rank, _)| *rank < 5) {
        matches.retain(|(rank, _)| *rank < 5);
    }
    matches.sort_by_key(|(rank, _)| *rank);
    matches.into_iter().map(|(_, command)| command).collect()
}

#[cfg(test)]
mod slash_tests {
    use super::slash_matches;
    #[test]
    fn prioritizes_command_names_and_uses_descriptions_only_as_fallback() {
        let catalog = serde_json::json!({"commands":[
            {"name":"claude-api","description":"Extra guidance for API use"},
            {"name":"run-extract","description":"Run extraction"},
            {"name":"bundled:extract","description":"Read documents"},
            {"name":"extract-pages","description":"Read PDF pages"},
            {"name":"extra","description":"Additional tools"}
        ]});
        let names = |query| slash_matches(query, &catalog).into_iter().map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(names("/EXTRA"), ["extra", "extract-pages", "bundled:extract", "run-extract"]);
        assert_eq!(names("/guidance"), ["claude-api"]);
        assert!(names("/dance").is_empty());
        assert_eq!(names("/")[0], "claude-api");
    }

    #[test]
    fn filters_only_provider_commands_at_start_of_draft() {
        let catalog = serde_json::json!({"commands":[{"name":"compact","description":"Summarize context","input":{"hint":"Optional focus"}}, {"name":"review","description":"Review changes"}]});
        assert_eq!(slash_matches("/COM", &catalog)[0].name, "compact");
        assert_eq!(slash_matches("/", &catalog).len(), 2);
        assert_eq!(
            slash_matches("/com", &catalog)[0].hint.as_deref(),
            Some("Optional focus")
        );
        assert!(slash_matches("explain /compact", &catalog).is_empty());
        assert!(slash_matches("/compact focus", &catalog).is_empty());
        assert!(slash_matches("/", &serde_json::Value::Null).is_empty());
    }
}

#[path = "model_suggestions.rs"]
mod model_suggestions;
