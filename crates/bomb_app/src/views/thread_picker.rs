//! Picking another thread, in any project, on this Mac or a server: the composer's @ mentions and
//! a reply's "Send to thread…".

use crate::models::app::{project_name, AppModel};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::component::{Icon, WindowExt};
use gpui_kit::*;
use uuid::Uuid;

/// A thread to pick, as the lists show it.
#[derive(Clone, Debug)]
pub struct ThreadChoice {
    pub id: Uuid,
    pub title: String,
    pub project: String,
    /// The server it runs on, when it's not this Mac.
    pub server: Option<String>,
    pub working: bool,
}

impl ThreadChoice {
    /// "Project · on Server · working".
    pub fn detail(&self) -> String {
        let mut parts = vec![self.project.clone()];
        if let Some(server) = &self.server { parts.push(format!("on {server}")); }
        if self.working { parts.push("working".into()); }
        parts.join(" · ")
    }
}

/// Every thread but `except`, most recent first.
pub fn choices(m: &AppModel, except: Option<Uuid>, cx: &App) -> Vec<ThreadChoice> {
    let servers = crate::runtime::servers(cx);
    m.thread_order.iter().filter(|id| Some(**id) != except).filter_map(|id| {
        let t = m.threads.get(id)?.read(cx);
        let root = t.meta.project_root.clone().unwrap_or_else(|| t.meta.cwd.clone());
        Some(ThreadChoice {
            id: *id,
            title: t.title(),
            project: project_name(&root),
            server: servers.for_root(&root).map(|s| s.config.name.clone()),
            working: t.thread.presence.turn_active(),
        })
    }).collect()
}

/// Threads whose title, project or server has every word of `query`.
pub fn matching(all: Vec<ThreadChoice>, query: &str, limit: usize) -> Vec<ThreadChoice> {
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_string).collect();
    all.into_iter()
        .filter(|c| { let hay = format!("{} {}", c.title, c.detail()).to_lowercase(); words.iter().all(|w| hay.contains(w)) })
        .take(limit)
        .collect()
}

/// "Send to thread…": pick a thread and `text` is posted there; it starts working on it.
pub fn open_send_to(model: Entity<AppModel>, from: Uuid, text: String, window: &mut Window, cx: &mut App) {
    let state = cx.new(|cx| CommandState::new(window, cx));
    let threads = choices(model.read(cx), Some(from), cx);
    let focus = state.clone();
    window.open_dialog(cx, move |dialog, window, cx| {
        let (state, threads, model, text) = (state.clone(), threads.clone(), model.clone(), text.clone());
        focus.update(cx, |s, cx| s.focus(window, cx));
        dialog
            .w(px(560.))
            .margin_top(px(96.))
            .close_button(false)
            .content(move |content, _, _| {
                let items = threads.iter().map(|c| {
                    CommandItem::new()
                        .label(format!("{}  ·  {}", c.title, c.detail()))
                        .icon(Icon::from(Lucide::MessageCircle))
                        .keywords([c.project.clone()])
                });
                let (threads, model, text) = (threads.clone(), model.clone(), text.clone());
                content.child(
                    Command::new(&state)
                        .placeholder("Send this reply to which thread?")
                        .bordered(false)
                        .max_h(px(420.))
                        .items(items)
                        .on_confirm(move |path, window, cx| {
                            if let Some(c) = threads.get(path.row) {
                                let (to, title, text) = (c.id, c.title.clone(), text.clone());
                                model.update(cx, |m, cx| m.forward_to_thread(from, to, title, text, cx));
                            }
                            window.close_dialog(cx);
                        })
                        .on_cancel(|window, cx| window.close_dialog(cx)),
                )
            })
    });
}
