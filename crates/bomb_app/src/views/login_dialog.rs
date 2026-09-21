//! Grok device-code sign-in, as a dialog: confirm code, open the browser,
//! paste the code back if the CLI asks for it.

use crate::views::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable, Sizable, WindowExt};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use grok_cli_wrapper::LoginPhase;

use crate::models::app::AppModel;
use crate::theme::Ui;

pub fn open_login_dialog(model: Entity<AppModel>, window: &mut Window, cx: &mut App) {
    let code_input = cx.new(|cx| InputState::new(window, cx).placeholder("Paste the code from the browser"));
    model.update(cx, |m, cx| m.start_grok_login(cx));
    window.open_dialog(cx, move |dialog: Dialog, _window, _cx| {
        let model = model.clone();
        let code_input = code_input.clone();
        let model_close = model.clone();
        dialog
            .title("Sign in to Grok")
            .w(px(440.))
            .on_close(move |_, _, cx| {
                model_close.update(cx, |m, cx| m.close_login(cx));
            })
            .content(move |content, _window, cx| {
                let ui = Ui::of(cx);
                let login = model.read(cx).login.clone();
                let mono = ui.mono.clone();
                let Some(login) = login else {
                    let _ = model.read(cx).login_starting;
                    return content.child(div().text_sm().text_color(ui.text_muted).child("Starting…"));
                };
                let phase = login.phase;
                let model_open = model.clone();
                let model_cancel = model.clone();
                let model_submit = model.clone();
                let code_for_submit = code_input.clone();
                content.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(div().text_sm().text_color(ui.text_muted).child(login.instructions.clone()))
                        .when_some(login.confirm_code.clone(), |el, code| {
                            el.child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .py_3()
                                    .rounded(px(10.))
                                    .bg(ui.ink(0.05))
                                    .text_xl()
                                    .font_family(mono.clone())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ui.text)
                                    .child(code),
                            )
                        })
                        .when(matches!(phase, LoginPhase::AwaitingBrowser | LoginPhase::Starting), |el| {
                            el.child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(
                                        Button::new("open-login-url")
                                            .primary()
                                            .small()
                                            .label("Open login page")
                                            .disabled(login.login_url.is_none())
                                            .on_click(move |_, _, cx| {
                                                model_open.update(cx, |m, cx| m.open_login_url(cx));
                                            }),
                                    )
                                    .child(Button::new("cancel-login").outline().small().label("Cancel").on_click(
                                        move |_, window, cx| {
                                            model_cancel.update(cx, |m, cx| m.cancel_login(cx));
                                            window.close_dialog(cx);
                                        },
                                    )),
                            )
                        })
                        .when(login.needs_paste && phase == LoginPhase::AwaitingBrowser, |el| {
                            el.child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(div().flex_1().child(Input::new(&code_input)))
                                    .child(Button::new("submit-code").small().label("Submit").on_click(
                                        move |_, _, cx| {
                                            let code = code_for_submit.read(cx).value().to_string();
                                            if !code.trim().is_empty() {
                                                model_submit.update(cx, |m, cx| m.submit_login_code(code, cx));
                                            }
                                        },
                                    )),
                            )
                        })
                        .when(phase == LoginPhase::Completed, |el| {
                            el.child(div().text_sm().text_color(ui.success).child(format!(
                                "Signed in{}",
                                login.status.email.as_deref().map(|e| format!(" as {e}")).unwrap_or_default()
                            )))
                        })
                        .when(phase == LoginPhase::Failed, |el| {
                            el.child(div().text_sm().text_color(ui.danger).child(if login.output_tail.is_empty() {
                                "Sign-in failed.".to_string()
                            } else {
                                login.output_tail.clone()
                            }))
                        }),
                )
            })
    });
}
