//! App-wide actions, key bindings and the native menu bar.

use gpui_kit::*;

gpui_kit::actions!(
    bomb,
    [
        Quit,
        OpenSettings,
        OpenFoundry,
        OpenHome,
        NewThread,
        NewFeature,
        OpenFeatures,
        NewWorkspaceConversation,
        NewMockSession,
        OpenProject,
        RevealProject,
        ToggleExplainer,
        CycleApprovalMode,
        SendPrompt,
        StopTurn,
        ToggleDevPreview,
        FindInThread,
        OpenCommandPalette,
        LandThread,
        SyncThread,
        DeleteThread,
    ]
);

pub fn init(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-n", NewThread, None),
        KeyBinding::new("cmd-shift-n", NewWorkspaceConversation, None),
        KeyBinding::new("cmd-o", OpenProject, None),
        KeyBinding::new("cmd-shift-e", ToggleExplainer, None),
        KeyBinding::new("cmd-.", StopTurn, None),
        KeyBinding::new("cmd-f", FindInThread, None),
        KeyBinding::new("cmd-k", OpenCommandPalette, None),
        KeyBinding::new("cmd-alt-n", NewFeature, None),
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
                MenuItem::action("New Workspace", NewThread),
                MenuItem::action("New Feature", NewFeature),
                MenuItem::action("Project Features", OpenFeatures),
                MenuItem::action("New Conversation in Workspace", NewWorkspaceConversation),
                MenuItem::action("Open Project…", OpenProject),
                MenuItem::action("Reveal Project in Finder", RevealProject),
            ],
            disabled: false,
        },
        Menu {
            name: "Thread".into(),
            items: vec![
                MenuItem::action("Stop Turn", StopTurn),
                MenuItem::action("Cycle Approval Mode", CycleApprovalMode),
                MenuItem::separator(),
                MenuItem::action("Update Workspace from Main", SyncThread),
                MenuItem::action("Review / Ship Workspace", LandThread),
                MenuItem::separator(),
                MenuItem::action("Delete Thread…", DeleteThread),
            ],
            disabled: false,
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Command Palette…", OpenCommandPalette),
                MenuItem::action("Find in Conversation", FindInThread),
                MenuItem::separator(),
                MenuItem::action("Toggle Explainer", ToggleExplainer),
                MenuItem::action("Toggle Dev Preview", ToggleDevPreview),
            ],
            disabled: false,
        },
    ]);
}
