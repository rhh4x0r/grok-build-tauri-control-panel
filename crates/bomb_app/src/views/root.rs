//! Main window: unified title bar over a resizable sidebar | thread view split.
//! Also owns app-level actions and drains toasts into the notification layer.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{h_resizable, resizable_panel, Icon, Root, TitleBar, WindowExt};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::{
    CycleApprovalMode, DeleteThread, LandThread, NewMockSession, NewThread, OpenProject, OpenSettings,
    FindInThread, OpenCommandPalette, RevealProject, StopTurn, SyncThread, ToggleDevPreview,
    ToggleExplainer,
};
use crate::views::preview::PreviewPanel;
use crate::models::app::{project_name, AppModel, ToastKind};
use crate::theme::{Layout, Ui};
use crate::views::sidebar::SidebarView;
use crate::views::thread_view::ThreadView;

pub struct RootView {
    model: Entity<AppModel>,
    sidebar: Entity<SidebarView>,
    thread: Entity<ThreadView>,
    preview: Entity<PreviewPanel>,
    review: Entity<crate::views::workspaces::ReviewPanel>,
    focus: FocusHandle,
    sidebar_open: bool,
    preview_open: bool,
    settings: Option<Entity<crate::views::settings::SettingsView>>,
    settings_open: bool,
    foundry: Option<Entity<crate::views::foundry::FoundryView>>,
    foundry_open: bool,
    features: Entity<crate::views::features::FeaturesView>,
    _mode_shortcut: Subscription,
}

impl RootView {
    pub fn new(model: Entity<AppModel>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |this, _, cx| {
            let request = this.model.update(cx, |m, _| m.file_reveal_request.take());
            if let Some(path) = request {
                this.preview_open = true;
                this.model.update(cx, |m, _| m.review_open = false);
                this.preview.update(cx, |panel, cx| panel.reveal_file(path, cx));
            }
            let close = this.model.update(cx, |m,_| std::mem::take(&mut m.foundry_close));
            if close { this.foundry_open = false; }
            cx.notify();
        }).detach();
        let features = cx.new(|cx| crate::views::features::FeaturesView::new(model.clone(), window, cx));
        let sidebar = cx.new(|cx| SidebarView::new(model.clone(), window, cx));
        let thread = cx.new(|cx| ThreadView::new(model.clone(), window, cx));
        let preview = cx.new(|cx| PreviewPanel::new(model.clone(), cx));
        let review = cx.new(|cx| crate::views::workspaces::ReviewPanel::new(model.clone(), cx));
        let window_id = window.window_handle().window_id();
        let view = cx.entity().downgrade();
        let mode_shortcut = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle().window_id() != window_id || !is_mode_shortcut(event) {
                return;
            }
            // Intercept before Root's TabPrev or Input's OutdentInline can move
            // focus/change text. Modal surfaces keep their own keyboard behavior.
            if window.has_active_dialog(cx) || window.has_active_sheet(cx)
                || gpui_kit::base::active_focus_trap(window, cx).is_some()
            {
                return;
            }
            let _ = view.update(cx, |this, cx| {
                if this.settings_open { return; }
                this.model.update(cx, |m, cx| m.cycle_mode(cx));
                window.prevent_default();
                cx.stop_propagation();
            });
        });
        Self {
            _mode_shortcut: mode_shortcut,
            review,
            model,
            sidebar,
            thread,
            preview,
            focus: cx.focus_handle(),
            sidebar_open: true,
            preview_open: false,
            settings: None,
            settings_open: false,
            foundry: None,
            foundry_open: false,
            features,
        }
    }

    fn open_features(&mut self, new: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.read(cx).active_project.is_none() {
            self.model.update(cx, |m, cx| { m.toast(ToastKind::Info, "Open a project to create or track features"); cx.notify(); });
            return;
        }
        self.settings_open = false;
        self.foundry_open = false;
        self.preview_open = false;
        self.model.update(cx, |m,cx| { m.features_open = true; m.review_open = false; cx.notify(); });
        self.features.update(cx, |v,cx| v.open(new,window,cx));
        cx.notify();
    }

    fn drain_toasts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let toasts: Vec<(ToastKind, String)> = self.model.update(cx, |m, _| m.toasts.drain(..).collect());
        for (kind, msg) in toasts {
            let note = match kind {
                ToastKind::Info => Notification::info(msg),
                ToastKind::Success => Notification::success(msg),
                ToastKind::Warning => Notification::warning(msg),
                ToastKind::Error => Notification::error(msg),
            };
            window.push_notification(note, cx);
        }
    }

    fn title_bar(&self, ui: &Ui, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let selected = model.selected_thread();
        let title = selected
            .as_ref()
            .map(|t| t.read(cx).title())
            .unwrap_or_else(|| "New thread".to_string());
        let title = if self.settings_open { "Settings".to_string() } else if model.features_open { "Features".to_string() } else { title };
        let project = model
            .active_project
            .as_deref()
            .map(project_name)
            .unwrap_or_else(|| "No project".into());
        let host = hostname();
        let hover = ui.hover;
        let muted = ui.text_muted;
        let icon_button = move |id: &'static str, icon: Lucide| {
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
                .child(div().size(px(14.)).child(Icon::from(icon)))
        };
        TitleBar::new().child(
            div()
                .flex()
                .items_center()
                .w_full()
                .h(px(Layout::TITLEBAR))
                .pl_1()
                .pr_3()
                .gap_1()
                .when(!self.settings_open, |el| el.child(icon_button("toggle-sidebar", Lucide::PanelLeft).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.sidebar_open = !this.sidebar_open;
                        cx.notify();
                    },
                ))))
                .child(icon_button("new-thread-tb", Lucide::Plus).on_click(cx.listener(
                    |_, _, window, cx| {
                        window.dispatch_action(Box::new(NewThread), cx);
                    },
                )))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .ml_3()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(ui.text_muted)
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(title),
                        )
                        .child(
                            div()
                                .id("project-chip")
                                .text_xs()
                                .text_color(ui.text_faint)
                                .cursor_pointer()
                                .hover(|s| s.opacity(0.8))
                                .on_click(cx.listener(|_, _, window, cx| {
                                    window.dispatch_action(Box::new(OpenProject), cx);
                                }))
                                .child(format!("{project} @ {host}")),
                        ),
                ),
        )
    }
}

impl Render for RootView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::theme::follow_system(window, cx);
        self.drain_toasts(window, cx);
        let ui = Ui::of(cx);
        if let Some(text) = self.model.update(cx, |m,_| m.foundry_insert.take()) {
            self.foundry_open = false;
            self.thread.update(cx, |t,cx|t.set_foundry_prompt(text,window,cx));
        }
        let model = self.model.clone();
        let m2 = self.model.clone();
        let m3 = self.model.clone();
        let m4 = self.model.clone();
        let m5 = self.model.clone();
        div()
            .id("root")
            .track_focus(&self.focus)
            .on_action(cx.listener(move |_, _: &NewMockSession, _, cx| {
                model.update(cx, |m, cx| m.new_mock_session(cx));
            }))
            .on_action(cx.listener(|this, _: &crate::actions::NewWorkspaceConversation, window, cx| {
                this.settings_open = false;
                this.foundry_open = false;
                this.model.update(cx, |m, cx| m.new_workspace_thread(cx));
                this.thread.update(cx, |t, cx| t.focus_composer(window, cx));
            }))
            .on_action(cx.listener(|this, _: &crate::actions::OpenHome, window, cx| {
                this.settings_open = false;
                this.foundry_open = false;
                this.preview_open = false;
                this.model.update(cx, |m, cx| m.open_home(cx));
                this.thread.update(cx, |t, cx| t.focus_composer(window, cx));
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &NewThread, window, cx| {
                this.settings_open = false;
                this.foundry_open = false;
                m2.update(cx, |m, cx| m.new_thread(cx));
                this.thread.update(cx, |t, cx| t.focus_composer(window, cx));
            }))
            .on_action(cx.listener(move |_, _: &OpenProject, _, cx| {
                m3.update(cx, |m, cx| m.open_project(cx));
            }))
            .on_action(cx.listener(move |_, _: &RevealProject, _, cx| {
                m4.update(cx, |m, cx| m.reveal_project(cx));
            }))
            .on_action(cx.listener(move |_, _: &CycleApprovalMode, _, cx| {
                m5.update(cx, |m, cx| m.cycle_mode(cx));
            }))
            .on_action(cx.listener(|this, _: &StopTurn, _, cx| {
                this.model.update(cx, |m, cx| m.cancel_selected(cx));
            }))
            .on_action(cx.listener(|this, _: &crate::actions::OpenFoundry, window, cx| {
                if this.foundry.is_none() { this.foundry = Some(cx.new(|cx| crate::views::foundry::FoundryView::new(this.model.clone(), window, cx))); }
                if let Some(text) = this.model.update(cx, |m,_|m.foundry_request.take()) {
                    if let Some(v)=&this.foundry {v.update(cx,|v,cx|v.seed(text,window,cx));}
                }
                if this.model.update(cx, |m,_| std::mem::take(&mut m.foundry_show_runs)) {
                    if let Some(v)=&this.foundry {v.update(cx,|v,cx|v.show_runs(cx));}
                }
                this.model.update(cx, |m,_| m.features_open = false);
                this.foundry_open = true; this.settings_open = false; cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                if this.settings.is_none() {
                    this.settings = Some(cx.new(|cx| crate::views::settings::SettingsView::new(window, cx)));
                }
                this.settings_open = true;
                this.focus.focus(window, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &crate::actions::NewFeature, window, cx| {
                this.open_features(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::actions::OpenFeatures, window, cx| {
                this.open_features(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenCommandPalette, window, cx| {
                crate::views::palette::open_palette(this.model.clone(), window, cx);
            }))
            .on_action(cx.listener(|this, _: &FindInThread, window, cx| {
                this.thread.update(cx, |t, cx| t.toggle_search(window, cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleDevPreview, _, cx| {
                let from_review = this.model.read(cx).review_open;
                this.model.update(cx, |m, cx| { m.review_open = false; cx.notify(); });
                if from_review { this.preview_open = false; }
                this.preview_open = !this.preview_open;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleExplainer, _, cx| {
                if let Some(t) = this.model.read(cx).selected_thread() {
                    t.update(cx, |t, cx| {
                        t.explain_open = !t.explain_open;
                        cx.notify();
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &LandThread, _, cx| {
                let sel = this.model.read(cx).selected;
                if let Some(id) = sel {
                    this.model.update(cx, |m, cx| m.land_thread(id, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &SyncThread, _, cx| {
                let sel = this.model.read(cx).selected;
                if let Some(id) = sel {
                    this.model.update(cx, |m, cx| m.sync_thread(id, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &DeleteThread, window, cx| {
                if this.settings_open {
                    return;
                }
                let sel = this.model.read(cx).selected;
                if let Some(id) = sel {
                    let m = this.model.clone();
                    window.open_alert_dialog(cx, move |dlg, _, _| {
                        let m = m.clone();
                        dlg.confirm().title("Delete this thread?")
                            .description("Its transcript is removed. The branch and files are kept. This cannot be undone.")
                            .on_ok(move |_, _, cx| {
                                m.update(cx, |a, cx| a.remove_thread(id, cx));
                                true
                            })
                    });
                }
            }))
            .size_full()
            .relative()
            .bg(ui.bg)
            .text_color(ui.text)
            .when(ui.dark, |el| {
                el.child(
                    img("assets/bg.png")
                        .absolute()
                        .inset_0()
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
            })
            .child(
            div()
            .size_full()
            .flex()
            .flex_col()
            .bg(ui.glass)
            .child(self.title_bar(&ui, cx))
            .when(self.settings_open, |el| {
                el.child(
                    div().flex().flex_col().flex_1().min_h_0()
                        .child(
                            div().h(px(Layout::HEADER)).flex_shrink_0().flex().items_center()
                                .px(px(Layout::SPACE_SM)).border_b_1().border_color(ui.border)
                                .child(
                                    div().id("settings-back").flex().items_center().gap_2()
                                        .px_2().py_1().rounded(px(6.)).cursor_pointer()
                                        .text_sm().text_color(ui.text_muted)
                                        .hover(move |s| s.bg(ui.hover).text_color(ui.text))
                                        .child(Icon::from(Lucide::ArrowLeft).size(px(14.)))
                                        .child("Back")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.settings_open = false;
                                            this.focus.focus(window, cx);
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(div().flex_1().min_h_0().children(self.settings.clone())),
                )
            })
            .when(!self.settings_open, |el| el.child(
                // The split is 100% tall internally; give it only the space below the title bar.
                div().flex_1().min_h_0().w_full().overflow_hidden().child(
                h_resizable("main-split")
                    .when(self.sidebar_open, |el| {
                        el.child(
                            resizable_panel()
                                .size(px(Layout::SIDEBAR))
                                .size_range(px(224.)..px(400.))
                                .child(self.sidebar.clone()),
                        )
                    })
                    .child(resizable_panel().child(if self.model.read(cx).features_open { self.features.clone().into_any_element() } else if self.foundry_open { self.foundry.clone().unwrap().into_any_element() } else { self.thread.clone().into_any_element() }))
                    .when(self.model.read(cx).review_open, |el| {
                        el.child(resizable_panel().size(px(640.)).size_range(px(400.)..px(1100.)).child(self.review.clone()))
                    })
                    .when(self.preview_open && !self.model.read(cx).review_open, |el| {
                        el.child(
                            resizable_panel()
                                .size(px(520.))
                                .size_range(px(360.)..px(1100.))
                                .child(self.preview.clone()),
                        )
                    }),
            ))),
            )
            // Root owns overlay state, but the application view must render it.
            .children(Root::render_sheet_layer(window, cx))
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}

pub fn open_main_window(model: Entity<AppModel>, cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1380.), px(900.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(960.), px(640.))),
        window_background: crate::theme::window_background(cx),
        ..TitleBar::window_options()
    };
    cx.spawn(async move |cx| {
        let result = cx.open_window(options, |window, cx| {
            let view = cx.new(|cx| RootView::new(model.clone(), window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        });
        if let Err(e) = result {
            tracing::error!(error = %e, "failed to open main window");
        }
    })
    .detach();
}

fn hostname() -> String {
    std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "local".into())
}

/// Only the conversation surface and its composer own this shortcut. Other
/// inputs and overlay controls must retain normal backwards tab navigation.
fn is_mode_shortcut(event: &KeystrokeEvent) -> bool {
    let key = &event.keystroke;
    if key.key != "tab" || !key.modifiers.shift
        || key.modifiers.platform || key.modifiers.control || key.modifiers.alt || key.modifiers.function
    {
        return false;
    }
    let has = |name| event.context_stack.iter().any(|context| context.contains(name));
    !has("PopupMenu") && !has("Popover") && !has("Dialog")
        && (!has("Input") || has("BombComposer"))
}

#[cfg(test)]
mod shortcut_tests {
    use super::is_mode_shortcut;
    use gpui_kit::{KeyContext, Keystroke, KeystrokeEvent};

    fn event(key: &str, contexts: &[&str]) -> KeystrokeEvent {
        KeystrokeEvent {
            keystroke: Keystroke::parse(key).unwrap(),
            action: None,
            context_stack: contexts.iter().map(|c| KeyContext::parse(c).unwrap()).collect(),
        }
    }

    #[test]
    fn cycles_before_focus_enters_composer_and_while_typing() {
        for contexts in [vec![], vec!["Root"], vec!["Root", "BombComposer"], vec!["Root", "BombComposer", "Input"]] {
            assert!(is_mode_shortcut(&event("shift-tab", &contexts)));
        }
    }

    #[test]
    fn leaves_other_keys_inputs_and_overlays_alone() {
        for key in ["tab", "ctrl-shift-tab", "cmd-shift-tab", "alt-shift-tab"] {
            assert!(!is_mode_shortcut(&event(key, &["Root", "BombComposer", "Input"])));
        }
        for contexts in [vec!["Root", "Input"], vec!["Root", "BombComposer", "PopupMenu"], vec!["Root", "BombComposer", "Popover", "Input"], vec!["Root", "Dialog"]] {
            assert!(!is_mode_shortcut(&event("shift-tab", &contexts)));
        }
    }
}
