//! Command palette (⌘K): app actions grouped by context, plus every thread
//! so you can jump to one by name.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::component::{Icon, WindowExt};
use gpui_kit::*;
use uuid::Uuid;

use crate::actions::{
    CycleApprovalMode, DeleteThread, FindInThread, LandThread, NewThread, OpenProject, OpenSettings,
    RevealProject, StopTurn, SyncThread, ToggleDevPreview, ToggleExplainer,
};
use crate::models::app::AppModel;

struct Entry {
    label: &'static str,
    keywords: &'static [&'static str],
    icon: Lucide,
    action: Box<dyn Action>,
}

fn entries() -> Vec<Entry> {
    vec![
        Entry { label: "New thread", keywords: &["chat", "start"], icon: Lucide::Plus, action: Box::new(NewThread) },
        Entry { label: "Open project…", keywords: &["folder", "repo"], icon: Lucide::FolderOpen, action: Box::new(OpenProject) },
        Entry { label: "Find in conversation", keywords: &["search"], icon: Lucide::Search, action: Box::new(FindInThread) },
        Entry { label: "Stop current turn", keywords: &["cancel", "interrupt"], icon: Lucide::Square, action: Box::new(StopTurn) },
        Entry { label: "Cycle approval mode", keywords: &["plan", "ask", "auto", "yolo"], icon: Lucide::ShieldCheck, action: Box::new(CycleApprovalMode) },
        Entry { label: "Toggle dev-server preview", keywords: &["browser", "webview"], icon: Lucide::AppWindow, action: Box::new(ToggleDevPreview) },
        Entry { label: "Toggle explainer", keywords: &["narrator"], icon: Lucide::MessageSquare, action: Box::new(ToggleExplainer) },
        Entry { label: "Sync from project branch", keywords: &["git", "update", "merge"], icon: Lucide::GitMerge, action: Box::new(SyncThread) },
        Entry { label: "Land into project branch", keywords: &["git", "ship"], icon: Lucide::GitPullRequestArrow, action: Box::new(LandThread) },
        Entry { label: "Reveal project in Finder", keywords: &["open", "folder"], icon: Lucide::Folder, action: Box::new(RevealProject) },
        Entry { label: "Delete thread…", keywords: &["remove"], icon: Lucide::Trash, action: Box::new(DeleteThread) },
        Entry { label: "Settings…", keywords: &["preferences", "mcp", "memory"], icon: Lucide::Settings, action: Box::new(OpenSettings) },
    ]
}

pub fn open_palette(model: Entity<AppModel>, window: &mut Window, cx: &mut App) {
    let state = cx.new(|cx| CommandState::new(window, cx));
    let threads: Vec<(Uuid, String, String)> = {
        let m = model.read(cx);
        m.thread_order
            .iter()
            .filter_map(|id| {
                let t = m.threads.get(id)?.read(cx);
                let project = crate::models::app::project_name(t.meta.project_root.as_deref().unwrap_or(&t.meta.cwd));
                Some((*id, t.title(), project))
            })
            .collect()
    };
    let action_count = entries().len();
    let focus_state = state.clone();
    window.open_dialog(cx, move |dialog, window, cx| {
        let state = state.clone();
        let threads = threads.clone();
        let model = model.clone();
        focus_state.update(cx, |s, cx| s.focus(window, cx));
        dialog
            .w(px(560.))
            .margin_top(px(96.))
            .close_button(false)
            .content(move |content, _window, _cx| {
                let items = entries().into_iter().map(|e| {
                    CommandItem::new()
                        .label(e.label)
                        .icon(Icon::from(e.icon))
                        .keywords(e.keywords.iter().map(|k| k.to_string()))
                        .action(e.action)
                });
                let thread_items = threads.iter().map(|(_, title, project)| {
                    CommandItem::new()
                        .label(format!("{title}  ·  {project}"))
                        .icon(Icon::from(Lucide::MessageCircle))
                        .keywords([project.clone()])
                });
                let model = model.clone();
                let threads = threads.clone();
                content.child(
                    Command::new(&state)
                        .placeholder("Type a command or thread name…")
                        .bordered(false)
                        .max_h(px(420.))
                        .items(items.chain(thread_items))
                        .on_confirm(move |path, window, cx| {
                            let row = path.row;
                            if row >= action_count {
                                if let Some((id, _, _)) = threads.get(row - action_count) {
                                    let id = *id;
                                    model.update(cx, |m, cx| m.select(Some(id), cx));
                                }
                            }
                            window.close_dialog(cx);
                        })
                        .on_cancel(|window, cx| window.close_dialog(cx)),
                )
            })
    });
}
