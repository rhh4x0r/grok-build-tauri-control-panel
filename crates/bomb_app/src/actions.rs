//! App-wide actions, key bindings and the native menu bar.

use gpui_kit::*;

gpui_kit::actions!(
    bomb,
    [
        Quit,
        OpenSettings,
        NewThread,
        NewMockSession,
        OpenProject,
        RevealProject,
        ToggleExplainer,
        CycleApprovalMode,
        SendPrompt,
        StopTurn,
    ]
);

pub fn init(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-n", NewThread, None),
        KeyBinding::new("cmd-o", OpenProject, None),
        KeyBinding::new("cmd-shift-e", ToggleExplainer, None),
    ]);
    cx.set_menus(vec![
        Menu {
            name: "Bomb Code".into(),
            items: vec![
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Quit Bomb Code", Quit),
            ],
            disabled: false,
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Thread", NewThread),
                MenuItem::action("Open Project…", OpenProject),
                MenuItem::action("Reveal Project in Finder", RevealProject),
                MenuItem::separator(),
                MenuItem::action("New Mock Session (debug)", NewMockSession),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![MenuItem::action("Toggle Explainer", ToggleExplainer)],
            disabled: false,
        },
    ]);
}
