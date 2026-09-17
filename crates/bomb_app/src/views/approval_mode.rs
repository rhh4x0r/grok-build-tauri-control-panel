//! Explicit opt-in for automatic approval of all non-denied tools.
use crate::models::app::AppModel;
use gpui_kit::component::{WindowExt, dialog::DialogButtonProps};
use gpui_kit::{App, Entity, Window};

pub fn request(model: Entity<AppModel>, mode: &'static str, window: &mut Window, cx: &mut App) {
    if mode != "yolo" {
        model.update(cx, |m, cx| m.set_mode(mode, cx));
        return;
    }
    let m = model.read(cx);
    if m.prefs.mode == "yolo" {
        return;
    }
    if m.prefs.temporary {
        model.update(cx, |m, cx| m.set_mode(mode, cx));
        return;
    }
    let target = (
        m.selected,
        m.active_project.clone(),
        m.active_workspace.clone(),
    );
    // Wait until the mode menu has closed before presenting the confirmation.
    window.defer(cx, move |window, cx| {
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let model = model.clone();
            let target = target.clone();
            dialog.button_props(DialogButtonProps::default().show_cancel(true).ok_text("Enable Full access")).title("Enable Full access for this conversation?")
                .description("Tools will run without individual approval, including commands that can delete or overwrite files. Deny rules still apply. This does not become the default for new conversations.")
                .on_ok(move |_, _, cx| {
                    model.update(cx, |m, cx| {
                        if (m.selected, m.active_project.clone(), m.active_workspace.clone()) == target {
                            m.set_mode("yolo", cx);
                        }
                    });
                    true
                })
        });
    });
}
