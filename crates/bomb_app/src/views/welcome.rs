//! One next step for connecting an agent, choosing a project and starting a chat.
use crate::views::button::Button;
use crate::{
    models::app::{project_name, AppModel},
    theme::Ui,
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::*;

#[derive(Debug, PartialEq)]
pub enum Step {
    Connect,
    Project,
    Conversation,
}
pub fn next_step(ready: bool, has_project: bool) -> Step {
    if !ready {
        Step::Connect
    } else if !has_project {
        Step::Project
    } else {
        Step::Conversation
    }
}

pub fn setup(model: Entity<AppModel>, ui: &Ui, cx: &App) -> AnyElement {
    let m = model.read(cx);
    let backend = m.prefs.backend.clone();
    let auth = m.auth.iter().find(|a| a.backend == backend);
    let ready = auth.is_some_and(|a| a.logged_in && a.runnable);
    let runnable = auth.is_some_and(|a| a.runnable);
    let provider = auth
        .map(|a| a.display_name.clone())
        .unwrap_or_else(|| backend.clone());
    let step = next_step(ready, m.active_project.is_some());
    let accounts = m.auth.clone();
    let projects = m.projects.clone();
    let picker = model.clone();
    let action = model.clone();
    let (title, detail) = match step {
        Step::Connect if auth.is_none() => ("Check your connections".into(), "Refresh to detect installed providers and sign-in status.".into()),
        Step::Connect if runnable => (format!("Connect {provider}"), "Sign in to start a conversation with your coding agent.".to_string()),
        Step::Connect => ("Connect a coding agent".into(), format!("Install the {provider} CLI, then refresh connections. You can also choose another provider.")),
        Step::Project => ("Choose a project".into(), "Select an existing project or add a folder to work in.".into()),
        Step::Conversation => ("Ready to start".into(), format!("Start a conversation in {}. Describe what you want to do in the composer.", m.active_project.as_deref().map(project_name).unwrap_or_default())),
    };
    let mut content = div()
        .id("guided-welcome")
        .size_full()
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_5()
        .p_6()
        .child(
            div().flex().gap_4().text_size(px(crate::theme::Type::CAPTION)).children(
                [
                    ("1  Connect", Step::Connect),
                    ("2  Choose project", Step::Project),
                    ("3  Start conversation", Step::Conversation),
                ]
                .into_iter()
                .map(|(label, s)| {
                    div()
                        .text_color(if s == step { ui.text } else { ui.text_faint })
                        .child(label)
                }),
            ),
        )
        .child(
            Icon::from(Lucide::MessageCircle)
                .size(px(24.))
                .text_color(ui.text_muted),
        )
        .child(
            div()
                .text_size(px(crate::theme::Type::DISPLAY))
                .font_weight(FontWeight::MEDIUM)
                .child(title),
        )
        .child(
            div()
                .max_w(px(440.))
                .text_center()
                .text_size(px(crate::theme::Type::BODY))
                .text_color(ui.text_muted)
                .child(detail),
        );
    let temporary = model.clone();
    match step {
        Step::Connect => {
            content = content
                .child(
                    Button::new("welcome-provider")
                        .ghost()
                        .small()
                        .label(provider)
                        .dropdown_caret(true)
                        .dropdown_menu(move |mut menu, _, _| {
                            for account in &accounts {
                                let app = picker.clone();
                                let backend = account.backend.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(account.display_name.clone()).on_click(
                                        move |_, _, cx| {
                                            app.update(cx, |m, cx| {
                                                m.set_backend(&backend, None, cx)
                                            });
                                        },
                                    ),
                                );
                            }
                            menu
                        }),
                )
                .child(
                    Button::new("welcome-connect")
                        .primary()
                        .label(if runnable {
                            "Sign in"
                        } else {
                            "Refresh connections"
                        })
                        .on_click(move |_, window, cx| {
                            if !runnable {
                                action.update(cx, |m, cx| {
                                    m.refresh_services(cx);
                                    m.refresh_backends(cx);
                                });
                            } else if backend == "grok" {
                                super::login_dialog::open_login_dialog(action.clone(), window, cx);
                            } else {
                                action.update(cx, |m, cx| m.sign_in(&backend, cx));
                            }
                        }),
                );
        }
        Step::Project => {
            content = content.child(
                Button::new("welcome-project")
                    .primary()
                    .label("Choose project")
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut menu, _, _| {
                        for project in &projects {
                            let app = action.clone();
                            let path = project.clone();
                            menu = menu.item(PopupMenuItem::new(project_name(project)).on_click(
                                move |_, _, cx| {
                                    app.update(cx, |m, cx| m.set_active_project(path.clone(), cx));
                                },
                            ));
                        }
                        let app = action.clone();
                        menu.separator().item(
                            PopupMenuItem::new("Add project…").on_click(move |_, _, cx| {
                                app.update(cx, |m, cx| m.open_project(cx))
                            }),
                        )
                    }),
            );
        }
        Step::Conversation => {
            content = content.child(
                Button::new("welcome-start")
                    .primary()
                    .label("Start conversation")
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::actions::NewThread), cx)
                    }),
            );
        }
    }
    content = content.child(Button::new("welcome-temporary").ghost().label("Temporary chat")
        .on_click(move |_, _, cx| temporary.update(cx, |m, cx| m.temporary_chat(cx))))
        .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child("No project needed · saved in ~/.bombcode/chats"));
    content.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{next_step, Step};
    #[test]
    fn setup_prioritizes_connection_then_project_then_conversation() {
        assert_eq!(next_step(false, false), Step::Connect);
        assert_eq!(next_step(false, true), Step::Connect);
        assert_eq!(next_step(true, false), Step::Project);
        assert_eq!(next_step(true, true), Step::Conversation);
    }
}
