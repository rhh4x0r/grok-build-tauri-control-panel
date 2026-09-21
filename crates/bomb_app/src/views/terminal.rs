//! Docked, per-thread interactive terminals.
use crate::views::button::Button;
use crate::{runtime::spawn_service, theme::Ui};
use bomb_core::terminal::TerminalSession;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    button::ButtonVariants,
    Icon, Sizable,
};
use gpui_kit::{prelude::FluentBuilder as _, *};
use std::{path::PathBuf, sync::Arc, time::Duration};

struct Tab {
    id: u64,
    name: String,
    session: Arc<TerminalSession>,
}
pub struct TerminalPanel {
    cwd: PathBuf,
    tabs: Vec<Tab>,
    selected: Option<u64>,
    next_id: u64,
    error: Option<String>,
    loading: bool,
    focus: FocusHandle,
    _poll: Task<()>,
}
impl TerminalPanel {
    pub fn new(cwd: PathBuf, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        let poll = cx.spawn(async move |_, cx| {
            let mut last = None;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(80))
                    .await;
                if weak
                    .update(cx, |panel, cx| {
                        let revision = panel.active().map(|t| (t.id, t.session.revision()));
                        if revision != last {
                            last = revision;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut panel = Self {
            cwd,
            tabs: Vec::new(),
            selected: None,
            next_id: 1,
            error: None,
            loading: false,
            focus: cx.focus_handle(),
            _poll: poll,
        };
        panel.add(cx);
        panel
    }
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus.focus(window, cx);
    }
    fn active(&self) -> Option<&Tab> {
        self.tabs.iter().find(|t| Some(t.id) == self.selected)
    }
    fn add(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        self.error = None;
        let cwd = self.cwd.clone();
        let remote = crate::runtime::servers(cx).for_root(&cwd.to_string_lossy());
        let weak = cx.entity().downgrade();
        spawn_service(
            cx,
            async move {
                // A server thread's terminal is a shell on the server, streamed here.
                if let Some(remote) = remote {
                    return crate::remote::live::open_terminal(remote, &cwd.to_string_lossy()).await;
                }
                tokio::task::spawn_blocking(move || TerminalSession::spawn(&cwd))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r)
            },
            move |result, cx| {
                let _ = weak.update(cx, |panel, cx| {
                    panel.loading = false;
                    match result {
                        Ok(session) => {
                            let id = panel.next_id;
                            panel.next_id += 1;
                            panel.tabs.push(Tab {
                                id,
                                name: format!("Terminal {id}"),
                                session,
                            });
                            panel.selected = Some(id);
                        }
                        Err(error) => panel.error = Some(error),
                    }
                    cx.notify();
                });
            },
        );
    }
    fn close(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(index) = self.tabs.iter().position(|t| t.id == id) {
            let tab = self.tabs.remove(index);
            // Closing a PTY may wait briefly for the child to exit; keep that off the UI thread.
            std::thread::spawn(move || drop(tab));
        }
        if self.selected == Some(id) {
            self.selected = self.tabs.last().map(|t| t.id);
        }
        cx.notify();
    }
    fn input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if let Some(tab) = self.active() {
            if let Err(error) = tab.session.write(&bytes) {
                self.error = Some(error);
            }
        }
        cx.notify();
    }
}
impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::of(cx);
        let focused = self.focus.is_focused(window);
        let active = self.active().map(|t| t.session.clone());
        let tabs = self
            .tabs
            .iter()
            .map(|t| {
                let id = t.id;
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .rounded(px(5.))
                    .when(Some(id) == self.selected, |el| el.bg(ui.selected_bg()))
                    .child(
                        Button::new(SharedString::from(format!("terminal-tab-{id}")))
                            .ghost()
                            .small()
                            .label(format!(
                                "{}{}",
                                t.name,
                                if t.session.exited() { " · exited" } else { "" }
                            ))
                            .on_click(cx.listener(move |panel, _, window, cx| {
                                panel.selected = Some(id);
                                panel.focus(window, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("terminal-close-{id}")))
                            .ghost()
                            .small()
                            .label("×")
                            .tooltip("Close terminal and its shell")
                            .on_click(cx.listener(move |panel, _, _, cx| panel.close(id, cx))),
                    )
            })
            .collect::<Vec<_>>();
        let mut body = div()
            .id("terminal-screen")
            .relative()
            .w_full()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .track_focus(&self.focus)
            .key_context("BombTerminal")
            .cursor_text()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|panel, _, window, cx| panel.focus(window, cx)),
            )
            .capture_key_down(cx.listener(|panel, event: &KeyDownEvent, window, cx| {
                let key = &event.keystroke;
                if key.modifiers.platform && key.key == "v" {
                    if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        panel.input(text.into_bytes(), cx);
                    }
                } else if key.modifiers.platform && key.key == "c" {
                    if let Some(tab) = panel.active() {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            tab.session.screen().contents(),
                        ));
                    }
                } else if let Some(bytes) = terminal_key(key) {
                    panel.input(bytes, cx);
                } else {
                    return;
                }
                cx.stop_propagation();
                window.prevent_default();
            }))
            .on_scroll_wheel(cx.listener(|panel, event: &ScrollWheelEvent, _, cx| {
                if let Some(tab) = panel.active() {
                    let y = event.delta.pixel_delta(px(16.)).y;
                    tab.session.scroll(if y > px(0.) { 3 } else { -3 });
                    cx.notify();
                    cx.stop_propagation();
                }
            }));
        if let Some(session) = active {
            let screen = session.screen();
            let (rows, cols) = screen.size();
            let cursor = screen.cursor_position();
            let mut grid = div()
                .flex()
                .flex_col()
                .font_family(ui.mono.clone())
                .text_size(px(crate::theme::Type::SMALL))
                .line_height(px(18.));
            for row in 0..rows {
                let mut line = div().flex().h(px(16.)).flex_shrink_0();
                for col in 0..cols {
                    let Some(cell) = screen.cell(row, col) else {
                        continue;
                    };
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let mut fg = terminal_color(cell.fgcolor(), ui.text);
                    let mut bg = terminal_color(cell.bgcolor(), ui.bg);
                    if cell.inverse() {
                        std::mem::swap(&mut fg, &mut bg);
                    }
                    if focused
                        && !screen.hide_cursor()
                        && screen.scrollback() == 0
                        && cursor == (row, col)
                    {
                        std::mem::swap(&mut fg, &mut bg);
                    }
                    line = line.child(
                        div()
                            .w(px(if cell.is_wide() { 14.4 } else { 7.2 }))
                            .h(px(16.))
                            .flex_shrink_0()
                            .text_color(fg)
                            .bg(bg)
                            .when(cell.bold(), |el| el.font_weight(FontWeight::BOLD))
                            .child(if cell.contents().is_empty() {
                                " ".into()
                            } else {
                                cell.contents().to_string()
                            }),
                    );
                }
                grid = grid.child(line);
            }
            body = body.child(grid).child(
                canvas(
                    move |bounds, _, _| {
                        session.resize(
                            (bounds.size.height / px(16.)).floor() as u16,
                            (bounds.size.width / px(7.2)).floor() as u16,
                        );
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );
        } else {
            body = body.child(
                div()
                    .p_3()
                    .text_size(px(crate::theme::Type::SMALL))
                    .text_color(ui.text_muted)
                    .child(if self.loading {
                        "Opening shell…"
                    } else {
                        "No terminals. Click + to open one in this thread."
                    }),
            );
        }
        div()
            .h(px(280.))
            .w_full()
            .flex()
            .flex_col()
            .bg(ui.bg)
            .border_t_1()
            .border_color(ui.border)
            .px_3()
            .py_2()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(Icon::from(Lucide::Terminal).size(px(14.)))
                    .children(tabs)
                    .child(
                        Button::new("terminal-new")
                            .ghost()
                            .small()
                            .label("+")
                            .tooltip("New terminal in this thread")
                            .on_click(cx.listener(|panel, _, _, cx| panel.add(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(crate::theme::Type::CAPTION))
                            .text_color(ui.text_faint)
                            .child("⌘C copy screen · ⌘V paste · Ctrl+C interrupt"),
                    ),
            )
            .when_some(self.error.clone(), |el, error| {
                el.child(div().text_size(px(crate::theme::Type::CAPTION)).text_color(ui.danger).child(error))
            })
            .child(body)
    }
}
fn terminal_key(key: &Keystroke) -> Option<Vec<u8>> {
    if key.modifiers.platform {
        return None;
    }
    if key.modifiers.control {
        let ch = key.key.chars().next()?;
        if key.key.len() == 1 && ch.is_ascii() {
            return Some(vec![(ch.to_ascii_uppercase() as u8) & 0x1f]);
        }
    }
    let special = match key.key.as_str() {
        "enter" => "\r",
        "backspace" => "\x7f",
        "tab" => "\t",
        "escape" => "\x1b",
        "up" => "\x1b[A",
        "down" => "\x1b[B",
        "right" => "\x1b[C",
        "left" => "\x1b[D",
        "home" => "\x1b[H",
        "end" => "\x1b[F",
        "delete" => "\x1b[3~",
        "pageup" => "\x1b[5~",
        "pagedown" => "\x1b[6~",
        _ => return key.key_char.as_ref().map(|s| s.as_bytes().to_vec()),
    };
    Some(special.as_bytes().to_vec())
}
fn terminal_color(color: vt100::Color, default: Hsla) -> Hsla {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32).into(),
        vt100::Color::Idx(index) => {
            const BASIC: [u32; 16] = [
                0x181818, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
                0x5c6370, 0xff8a92, 0xb3e895, 0xffdb96, 0x8bc8ff, 0xe3a3ff, 0x80e2ec, 0xffffff,
            ];
            let value = if index < 16 {
                BASIC[index as usize]
            } else if index >= 232 {
                let v = 8 + 10 * (index as u32 - 232);
                (v << 16) | (v << 8) | v
            } else {
                let i = index as u32 - 16;
                let channel = |v| if v == 0 { 0 } else { 55 + v * 40 };
                (channel(i / 36) << 16) | (channel((i / 6) % 6) << 8) | channel(i % 6)
            };
            rgb(value).into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::terminal_key;
    use gpui_kit::Keystroke;

    #[test]
    fn shell_control_keys_are_forwarded() {
        for (key, bytes) in [("ctrl-c", b"\x03".as_slice()), ("enter", b"\r"), ("up", b"\x1b[A"), ("backspace", b"\x7f")] {
            assert_eq!(terminal_key(&Keystroke::parse(key).unwrap()).as_deref(), Some(bytes));
        }
        assert!(terminal_key(&Keystroke::parse("cmd-q").unwrap()).is_none());
    }
}
