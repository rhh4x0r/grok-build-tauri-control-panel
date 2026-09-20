//! Choose a project first, then connect an agent and plan the work together.
use crate::{
    models::app::{project_name, AppModel},
    theme::Ui,
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants};
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
    if !has_project {
        Step::Project
    } else if !ready {
        Step::Connect
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
        Step::Connect if runnable => (format!("Connect {provider}"), "Your project is selected. Sign in to plan with your coding agent.".to_string()),
        Step::Connect => ("Connect a coding agent".into(), format!("Install the {provider} CLI, then refresh connections. You can also choose another provider.")),
        Step::Project => ("What would you like to work on?".into(), "Create a project or open a folder. You can choose your project before connecting an agent.".into()),
        Step::Conversation => ("Let’s plan your project".into(), format!("Describe what you want to build in {}. We’ll work through the questions and plan together before you start building.", m.active_project.as_deref().map(project_name).unwrap_or_default())),
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
            div().flex().gap_4().text_size(px(11.)).children(
                [
                    ("1  Choose project", Step::Project),
                    ("2  Connect agent", Step::Connect),
                    ("3  Plan together", Step::Conversation),
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
                .text_size(px(22.))
                .font_weight(FontWeight::MEDIUM)
                .child(title),
        )
        .child(
            div()
                .max_w(px(440.))
                .text_center()
                .text_size(px(13.))
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
            let create = model.clone();
            let open = model.clone();
            content = content.child(
                div()
                    .flex()
                    .gap_3()
                    .child(
                        Button::new("welcome-new-project")
                            .primary()
                            .label("New project")
                            .on_click(move |_, _, cx| {
                                create.update(cx, |m, cx| m.create_project(cx));
                            }),
                    )
                    .child(
                        Button::new("welcome-open-project")
                            .primary()
                            .label("Open project")
                            .on_click(move |_, _, cx| {
                                open.update(cx, |m, cx| m.open_project(cx));
                            }),
                    ),
            );
        }
        Step::Conversation => {
            content = content.child(
                Button::new("welcome-start")
                    .primary()
                    .label("Plan project work")
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::actions::NewFeature), cx)
                    }),
            );
        }
    }
    // Recent projects remain available without requiring an agent connection.
    if !projects.is_empty() {
        let recent = model.clone();
        content = content.child(
            Button::new("welcome-recent-projects")
                .ghost()
                .label("Recent projects")
                .dropdown_caret(true)
                .dropdown_menu(move |mut menu, _, _| {
                    for project in &projects {
                        let app = recent.clone();
                        let path = project.clone();
                        menu = menu.item(PopupMenuItem::new(project_name(project)).on_click(
                            move |_, _, cx| {
                                app.update(cx, |m, cx| m.set_active_project(path.clone(), cx));
                            },
                        ));
                    }
                    menu
                }),
        );
    }
    let mut alternatives = div().flex().gap_3();
    if step == Step::Conversation {
        alternatives = alternatives.child(
            Button::new("welcome-chat")
                .ghost()
                .label("Ordinary chat")
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(crate::actions::NewThread), cx);
                }),
        );
    }
    content = content
        .child(
            alternatives
                .child(
                    Button::new("welcome-temporary")
                        .ghost()
                        .label("Temporary chat")
                        .on_click(move |_, _, cx| {
                            temporary.update(cx, |m, cx| m.temporary_chat(cx));
                        }),
                )
                .child(
                    Button::new("welcome-settings")
                        .ghost()
                        .label("Connections & settings")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(crate::actions::OpenSettings), cx);
                        }),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(ui.text_faint)
                .child("Temporary chats are saved in ~/.bombcode/chats"),
        );
    content.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{next_step, Step};
    #[test]
    fn setup_allows_project_selection_before_connection_then_planning() {
        assert_eq!(next_step(false, false), Step::Project);
        assert_eq!(next_step(false, true), Step::Connect);
        assert_eq!(next_step(true, false), Step::Project);
        assert_eq!(next_step(true, true), Step::Conversation);
    }
}
