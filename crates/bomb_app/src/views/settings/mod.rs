//! In-window Settings screen (⌘,): General · MCP · Memory · Worktrees · Permissions ·
//! Diagnostics. Built on gpui-kit's `Settings` pages; the data-heavy tabs are
//! custom-rendered items reading [`SettingsModel`] through its global handle.

pub mod model;

use crate::views::button::Button;
use gpui_kit::component::button::ButtonVariants;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Sizable, WindowExt};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use grok_config::SandboxProfile;
use grok_mcp::DoctorStatus;

use crate::models::app::AppModelHandle;
use crate::theme::{Layout, Ui};
pub use model::{SettingsHandle, SettingsModel};

pub struct SettingsView {
    model: Entity<SettingsModel>,
}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = cx.new(|cx| SettingsModel::new(window, cx));
        cx.set_global(SettingsHandle(model.clone()));
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        let app = cx.global::<AppModelHandle>().0.clone();
        cx.observe(&app, |_, _, cx| cx.notify()).detach();
        model.update(cx, |m, cx| m.load_all(window, cx));
        Self { model }
    }
}

fn settings(cx: &App) -> Entity<SettingsModel> {
    cx.global::<SettingsHandle>().0.clone()
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::theme::follow_system(window, cx);
        self.model.update(cx, |m, cx| m.sync_rule_inputs(window, cx));
        let ui = Ui::of(cx);
        div()
            .size_full()
            .text_color(ui.text)
            .child(
                Settings::new("bomb-settings")
                    .sidebar_width(px(Layout::SIDEBAR))
                    .sidebar_size_range(px(224.)..px(400.))
                    .sidebar_style(&StyleRefinement::default().bg(gpui_kit::transparent_black()))
                    .pages([
                        general_page(cx),
                        routing_page(),
                        mcp_page(),
                        memory_page(),
                        servers_page(),
                        permissions_page(cx),
                        advanced_page(),
                    ]),
            )
    }
}

// ── One pattern for every list in Settings ──────────────────────────────
// A group has a title and a one-line description. Inside it, each thing is a
// row: what it is on the left, its controls on the right, a hairline beneath.
// Actions for the whole list sit in a toolbar above; "add" inputs sit below.

/// A row in a settings list. Rows may wrap into a second line of detail.
fn list_row(ui: &Ui) -> Div {
    div().flex().flex_col().gap_1().w_full().min_h(px(40.)).py_2().justify_center().border_b_1().border_color(ui.hairline(0.06))
}

/// The main line of a row: name and facts on the left, controls pushed right by a `flex_1` spacer.
fn row_line() -> Div {
    div().flex().items_center().gap_2().w_full()
}

fn row_title(text: impl Into<SharedString>) -> Div {
    div().text_sm().font_weight(FontWeight::MEDIUM).child(text.into())
}

fn row_meta(text: impl Into<SharedString>, ui: &Ui) -> Div {
    div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child(text.into())
}

fn tag(text: impl Into<SharedString>, ui: &Ui) -> Div {
    div().px_1p5().rounded(px(4.)).text_size(px(crate::theme::Type::SMALL)).bg(ui.ink(0.06)).text_color(ui.text_muted).child(text.into())
}

/// Shown instead of rows when a list has nothing in it.
fn empty_list(text: impl Into<SharedString>, ui: &Ui) -> Div {
    div().py_3().text_sm().text_color(ui.text_faint).child(text.into())
}

/// Buttons that act on the whole list, above it.
fn toolbar() -> Div {
    div().flex().items_center().gap_2().pb_1()
}

/// Inputs for adding to a list, below it.
fn add_row() -> Div {
    div().flex().items_center().gap_2().pt_3()
}

// ── General ─────────────────────────────────────────────────────────────

fn general_page(cx: &App) -> SettingPage {
    let backends: Vec<(SharedString, SharedString)> = {
        let app = cx.global::<AppModelHandle>().0.read(cx);
        let mut v: Vec<(SharedString, SharedString)> = app.backends.iter().map(|b| (b.id.clone().into(), b.display_name.clone().into())).collect();
        if v.is_empty() {
            v = vec![("grok".into(), "Grok".into()), ("claude".into(), "Claude".into()), ("codex".into(), "Codex".into())];
        }
        v
    };
    SettingPage::new("General")
        .description("Defaults for new threads and the narrator.")
        .group(
            SettingGroup::new()
                .title("Sessions")
                .item(
                    SettingItem::new(
                        "Default backend",
                        SettingField::dropdown(
                            backends.clone(),
                            |cx| settings(cx).read(cx).config.as_ref().map(|c| c.default_backend.key().to_string()).unwrap_or_default().into(),
                            |v, cx| settings(cx).update(cx, |m, cx| m.edit_config(|c| {
                                if let Some(b) = grok_config::Backend::from_key(&v) {
                                    c.default_backend = b;
                                }
                            }, cx)),
                        ),
                    )
                    .description("Pre-selected in the composer for new threads."),
                )
                .item(
                    SettingItem::new(
                        "Default approval mode",
                        SettingField::dropdown(
                            vec![
                                ("plan".into(), "plan · propose, no execution".into()),
                                ("ask".into(), "ask · confirm every tool".into()),
                                ("auto".into(), "auto · agent approval policy".into()),
                            ],
                            |cx| settings(cx).read(cx).config.as_ref().and_then(|c| c.approval_mode_default.clone()).filter(|m| m != "yolo").unwrap_or_else(|| "plan".into()).into(),
                            |v, cx| settings(cx).update(cx, |m, cx| m.edit_config(|c| c.approval_mode_default = Some(v.to_string()), cx)),
                        ),
                    )
                    .description("Full access requires explicit opt-in from a conversation’s mode menu. Deny rules always apply."),
                )
                .item(
                    SettingItem::new(
                        "Use isolated branches for changes",
                        SettingField::switch(
                            |cx| settings(cx).read(cx).config.as_ref().map(|c| c.worktree_isolation_default).unwrap_or(true),
                            |v, cx| settings(cx).update(cx, |m, cx| m.edit_config(|c| c.worktree_isolation_default = v, cx)),
                        ),
                    )
                    .description("Each thread can make changes on an isolated branch, keeping your project folder unchanged."),
                )
                .item(
                    SettingItem::new(
                        "Max concurrent sessions",
                        SettingField::input(
                            |cx| settings(cx).read(cx).config.as_ref().map(|c| c.max_concurrent_sessions.to_string()).unwrap_or_default().into(),
                            |v, cx| {
                                if let Ok(n) = v.trim().parse::<usize>() {
                                    settings(cx).update(cx, |m, cx| m.edit_config(|c| c.max_concurrent_sessions = n.max(1), cx));
                                }
                            },
                        ),
                    ),
                )
                .item(
                    SettingItem::new(
                        "Sandbox profile",
                        SettingField::dropdown(
                            vec![
                                ("workspace".into(), "Project files · read/write the project".into()),
                                ("read_only".into(), "read-only".into()),
                                ("strict".into(), "strict · minimal FS, no network".into()),
                                ("unrestricted".into(), "unrestricted".into()),
                            ],
                            |cx| settings(cx).read(cx).config.as_ref().map(|c| c.sandbox_profile.as_str().to_string()).unwrap_or_else(|| "workspace".into()).into(),
                            |v, cx| settings(cx).update(cx, |m, cx| m.edit_config(|c| c.sandbox_profile = SandboxProfile::from_str_lossy(&v), cx)),
                        ),
                    ),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Explainer")
                .description("A cheap side model narrates what the agent is doing, in plain English, under the status line.")
                .item(SettingItem::new(
                    "Enabled",
                    SettingField::switch(
                        |cx| settings(cx).read(cx).config.as_ref().map(|c| c.explainer_enabled).unwrap_or(false),
                        |v, cx| settings(cx).update(cx, |m, cx| m.set_explainer(v, cx)),
                    ),
                ))
                .item(SettingItem::new(
                    "Narrator backend",
                    SettingField::dropdown(
                        backends,
                        |cx| settings(cx).read(cx).config.as_ref().and_then(|c| c.explainer_backend.clone()).unwrap_or_else(|| "grok".into()).into(),
                        |v, cx| settings(cx).update(cx, |m, cx| {
                            let model = m.config.as_ref().and_then(|c| c.explainer_model.clone());
                            m.set_explainer_provider(Some(v.to_string()), model, cx)
                        }),
                    ),
                ))
                .item(SettingItem::new(
                    "Narrator model",
                    SettingField::input(
                        |cx| settings(cx).read(cx).config.as_ref().and_then(|c| c.explainer_model.clone()).unwrap_or_default().into(),
                        |v, cx| settings(cx).update(cx, |m, cx| {
                            let backend = m.config.as_ref().and_then(|c| c.explainer_backend.clone());
                            let model = Some(v.trim().to_string()).filter(|s| !s.is_empty());
                            m.set_explainer_provider(backend, model, cx)
                        }),
                    ),
                )),
        )
}

// ── MCP ─────────────────────────────────────────────────────────────────

fn mcp_page() -> SettingPage {
    SettingPage::new("Tools (MCP)")
        .description("Extra tools an agent can use, such as GitHub or a browser. Secrets are stored separately from these settings.")
        .group(
            SettingGroup::new()
                .title("Tool servers")
                .description("Turn one on to make it available when you start a thread.")
                .item(SettingItem::render(|_, _, cx| render_mcp_servers(cx))),
        )
        .group(
            SettingGroup::new()
                .title("Credentials")
                .description("Keys a tool server needs, saved by name (GITHUB_TOKEN, LINEAR_API_KEY, …).")
                .item(SettingItem::render(|_, _, cx| render_credentials(cx))),
        )
}

fn doctor_color(status: Option<&DoctorStatus>, ui: &Ui) -> Hsla {
    match status {
        Some(DoctorStatus::Ok) => ui.success,
        Some(DoctorStatus::Warn) => ui.warning,
        Some(DoctorStatus::Error) => ui.danger,
        _ => ui.text_faint,
    }
}

fn render_mcp_servers(cx: &mut App) -> AnyElement {
    let ui = Ui::of(cx);
    let model = settings(cx);
    let (servers, catalog, doctor, busy) = {
        let m = model.read(cx);
        (m.mcp.clone(), m.catalog.clone(), m.doctor.clone(), m.busy.clone())
    };
    let add_model = model.clone();
    let check_model = model.clone();
    div()
        .flex()
        .flex_col()
        .gap_2()
        .w_full()
        .child(
            toolbar()
                .child(
                    Button::new("mcp-add")
                        .outline()
                        .small()
                        .label("Add from catalog")
                        .dropdown_caret(true)
                        .dropdown_menu(move |mut menu, _, _| {
                            for e in &catalog {
                                let m = add_model.clone();
                                let entry = e.clone();
                                menu = menu.item(PopupMenuItem::new(format!("{} · {}", e.title, e.description)).on_click(move |_, _, cx| {
                                    m.update(cx, |s, cx| s.add_from_catalog(&entry, cx));
                                }));
                            }
                            menu
                        }),
                )
                .child(Button::new("mcp-check-all").ghost().small().label("Check all").on_click(move |_, _, cx| {
                    check_model.update(cx, |s, cx| s.doctor(None, cx));
                }))
                .when_some(busy, |el, b| el.child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child(b))),
        )
        .children(servers.into_iter().map(|s| {
            let name = s.name.clone();
            let report = doctor.get(&s.name);
            let dot = doctor_color(report.map(|r| &r.status), &ui);
            let msgs = report.map(|r| r.messages.join(" · ")).unwrap_or_default();
            let model_toggle = model.clone();
            let model_auto = model.clone();
            let model_check = model.clone();
            let model_remove = model.clone();
            let (n1, n2, n3, n4) = (name.clone(), name.clone(), name.clone(), name.clone());
            let mut badges: Vec<String> = Vec::new();
            if s.high_risk {
                badges.push("can change things outside the project".into());
            }
            if s.requires_approval {
                badges.push("asks before each use".into());
            }
            if s.read_only {
                badges.push("read only".into());
            }
            for k in &s.credential_keys {
                badges.push(format!("needs {k}"));
            }
            list_row(&ui)
                .child(
                    row_line()
                        .child(div().size(px(8.)).flex_shrink_0().rounded_full().bg(dot))
                        .child(row_title(s.name.clone()))
                        .child(row_meta(format!("{} · {}", s.kind, s.transport.as_str()), &ui))
                        .children(badges.into_iter().map(|b| tag(b, &ui)))
                        .child(div().flex_1())
                        .child(
                            Switch::new(SharedString::from(format!("auto-{name}")))
                                .label("auto-attach")
                                .checked(s.auto_attach)
                                .on_click(move |v, _, cx| model_auto.update(cx, |m, cx| m.set_auto_attach(n2.clone(), *v, cx))),
                        )
                        .child(
                            Switch::new(SharedString::from(format!("en-{name}")))
                                .checked(s.enabled)
                                .on_click(move |v, _, cx| model_toggle.update(cx, |m, cx| m.toggle_mcp(n1.clone(), *v, cx))),
                        )
                        .child(Button::new(SharedString::from(format!("chk-{name}"))).ghost().small().label("Check").on_click(move |_, _, cx| {
                            model_check.update(cx, |m, cx| m.doctor(Some(n3.clone()), cx));
                        }))
                        .child(Button::new(SharedString::from(format!("rm-{name}"))).ghost().small().label("Remove").on_click(move |_, window, cx| {
                            let m = model_remove.clone();
                            let n = n4.clone();
                            window.open_alert_dialog(cx, move |dlg, _, _| {
                                let m = m.clone();
                                let n = n.clone();
                                dlg.title(format!("Remove {n}?")).description("The server config is deleted; credentials stay.").on_ok(move |_, _, cx| {
                                    m.update(cx, |s, cx| s.remove_mcp(n.clone(), cx));
                                    true
                                })
                            });
                        })),
                )
                .when_some(s.description.clone(), |el, d| el.child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child(d)))
                .when(!msgs.is_empty(), |el| el.child(div().text_size(px(crate::theme::Type::SMALL)).text_color(dot).child(msgs)))
        }))
        .into_any_element()
}

fn render_credentials(cx: &mut App) -> AnyElement {
    let ui = Ui::of(cx);
    let model = settings(cx);
    let (creds, key, value) = {
        let m = model.read(cx);
        (m.credentials.clone(), m.cred_key.clone(), m.cred_value.clone())
    };
    let save_model = model.clone();
    let none = creds.is_empty();
    div()
        .flex()
        .flex_col()
        .w_full()
        .when(none, |el| el.child(empty_list("No keys saved yet.", &ui)))
        .children(creds.into_iter().map(|c| {
            let m = model.clone();
            let k = c.key.clone();
            list_row(&ui).child(
                row_line()
                .child(row_title(c.key.clone()).font_family(ui.mono.clone()))
                .child(row_meta(c.masked.clone(), &ui))
                .child(div().flex_1())
                .child(Button::new(SharedString::from(format!("cred-rm-{}", c.key))).ghost().small().label("Remove").on_click(move |_, _, cx| {
                    m.update(cx, |s, cx| s.remove_credential(k.clone(), cx));
                })))
        }))
        .child(
            add_row()
                .child(div().w(px(220.)).child(Input::new(&key)))
                .child(div().flex_1().child(Input::new(&value).mask_toggle()))
                .child(Button::new("cred-save").small().label("Save").on_click(move |_, window, cx| {
                    save_model.update(cx, |s, cx| s.save_credential(window, cx));
                })),
        )
        .into_any_element()
}

// ── Memory ──────────────────────────────────────────────────────────────

fn memory_page() -> SettingPage {
    SettingPage::new("Memory")
        .description("Things every new thread should know. Notes for everything are always included; project notes only in that project.")
        .group(SettingGroup::new().title("Notes").description("Add, edit or remove what agents are told at the start of a thread.").item(SettingItem::render(|_, _, cx| render_memory(cx))))
}

fn render_memory(cx: &mut App) -> AnyElement {
    let ui = Ui::of(cx);
    let model = settings(cx);
    let (scope, project_scope, search, add, tags, busy) = {
        let m = model.read(cx);
        (m.memory_scope.clone(), m.project_scope.clone(), m.mem_search.clone(), m.mem_add.clone(), m.mem_tags.clone(), m.busy.clone())
    };
    let entries = model.read(cx).visible_memory(cx);
    let scope_button = |label: &str, value: String, model: &Entity<SettingsModel>, current: &str| {
        let m = model.clone();
        let v = value.clone();
        Button::new(SharedString::from(format!("scope-{value}")))
            .small()
            .compact()
            .label(label.to_string())
            .map(|b| if current == value { b.primary() } else { b.outline() })
            .on_click(move |_, _, cx| {
                m.update(cx, |s, cx| {
                    s.memory_scope = v.clone();
                    cx.notify();
                })
            })
    };
    let digest_model = model.clone();
    let export_model = model.clone();
    let add_model = model.clone();
    div()
        .flex()
        .flex_col()
        .gap_3()
        .w_full()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(scope_button("Global", "global".into(), &model, &scope))
                .when_some(project_scope, |el, ps| el.child(scope_button("This project", ps, &model, &scope)))
                .child(div().flex_1())
                .child(div().w(px(240.)).child(Input::new(&search).cleanable(true)))
                .child(Button::new("mem-digest").ghost().small().label("Digest").on_click(move |_, window, cx| {
                    let m = digest_model.clone();
                    window.open_alert_dialog(cx, move |dlg, _, _| {
                        let m = m.clone();
                        dlg.title("Digest this scope?")
                            .description("A side model condenses the notes into one summary entry; the originals are kept.")
                            .on_ok(move |_, _, cx| {
                                m.update(cx, |s, cx| s.digest_memory(cx));
                                true
                            })
                    });
                }))
                .child(Button::new("mem-export").ghost().small().label("Export .md").on_click(move |_, _, cx| {
                    export_model.update(cx, |s, cx| s.export_memory(cx));
                }))
                .when_some(busy, |el, b| el.child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child(b))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .when(entries.is_empty(), |el| el.child(empty_list("No notes here yet.", &ui)))
                .children(entries.into_iter().map(|e| {
                    let m_edit = model.clone();
                    let m_rm = model.clone();
                    let entry = e.clone();
                    let id = e.id.clone();
                    list_row(&ui)
                        .child(div().text_sm().whitespace_normal().child(e.content.clone()))
                        .child(
                            row_line()
                                .children(e.tags.iter().map(|t| tag(t.clone(), &ui)))
                                .child(row_meta(e.updated_at.format("%b %-d, %Y").to_string(), &ui))
                                .child(div().flex_1())
                                .child(Button::new(SharedString::from(format!("mem-edit-{}", e.id))).ghost().small().label("Edit").on_click(move |_, window, cx| {
                                    m_edit.update(cx, |s, cx| s.edit_memory(&entry, window, cx));
                                }))
                                .child(Button::new(SharedString::from(format!("mem-rm-{}", e.id))).ghost().small().label("Delete").on_click(move |_, _, cx| {
                                    m_rm.update(cx, |s, cx| s.remove_memory(id.clone(), cx));
                                })),
                        )
                })),
        )
        .child(
            add_row()
                .child(div().flex_1().child(Input::new(&add)))
                .child(div().w(px(200.)).child(Input::new(&tags)))
                .child(Button::new("mem-add").small().label("Add").on_click(move |_, window, cx| {
                    add_model.update(cx, |s, cx| s.add_memory(window, cx));
                })),
        )
        .into_any_element()
}

// ── Servers ─────────────────────────────────────────────────────────────

fn servers_page() -> SettingPage {
    SettingPage::new("Servers")
        .description("Run projects on a server you own so threads keep working with your laptop closed. Any Mac you pair can pick them up.")
        .group(SettingGroup::new().item(SettingItem::render(|_, _, cx| render_servers(cx))))
}

fn render_servers(cx: &mut App) -> AnyElement {
    let ui = Ui::of(cx);
    let model = settings(cx);
    let app = cx.global::<AppModelHandle>().0.clone();
    let pairing = app.read(cx).pairing;
    let servers = crate::runtime::servers(cx).all();
    let (link, name, project, invitee, devices, invite, people) = {
        let m = model.read(cx);
        (m.server_link.clone(), m.server_name.clone(), m.server_project.clone(), m.server_invitee.clone(), m.server_devices.clone(), m.server_invite.clone(), m.server_people.clone())
    };
    let pair_model = model.clone();
    let caption = |text: &str| div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child(text.to_string());
    let mut page = div().flex().flex_col().gap_5().w_full();

    for server in servers {
        let id = server.config.id.clone();
        let state = server.state();
        let (dot, status) = match &state {
            crate::remote::LinkState::Connected => (ui.success, "Connected".to_string()),
            crate::remote::LinkState::Connecting => (ui.warning, "Connecting…".to_string()),
            crate::remote::LinkState::Offline(why) => (ui.danger, why.clone()),
        };
        let (m_project, m_devices, m_invite, id_project, id_devices, id_invite, id_unpair) = (model.clone(), model.clone(), model.clone(), id.clone(), id.clone(), id.clone(), id.clone());
        let unpair_app = app.clone();
        let server_name = server.config.name.clone();
        let mut card = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded(px(Layout::PANEL_RADIUS))
            .border_1()
            .border_color(ui.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().size(px(8.)).rounded_full().bg(dot))
                    .child(div().text_size(px(crate::theme::Type::TITLE)).font_weight(FontWeight::MEDIUM).child(server.config.name.clone()))
                    .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child(format!("{} · signed in as {}{}", server.config.host, server.config.user, if server.config.admin { " (admin)" } else { "" })))
                    .child(div().flex_1())
                    .child(Button::new(SharedString::from(format!("server-unpair-{id}"))).ghost().small().label("Unpair this Mac").on_click(move |_, window, cx| {
                        let (app, id, name) = (unpair_app.clone(), id_unpair.clone(), server_name.clone());
                        window.open_alert_dialog(cx, move |dlg, _, _| {
                            let (app, id) = (app.clone(), id.clone());
                            dlg.confirm()
                                .title(format!("Unpair from {name}?"))
                                .description("This Mac stops showing that server’s projects and is removed from its devices. Nothing on the server is deleted.")
                                .on_ok(move |_, _, cx| { app.update(cx, |m, cx| m.unpair_server(id.clone(), cx)); true })
                        });
                    })),
            )
            .child(caption(&status))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(260.)).child(Input::new(&project)))
                    .child(Button::new(SharedString::from(format!("server-project-{id}"))).outline().small().label("New project on this server").on_click(move |_, window, cx| {
                        m_project.update(cx, |s, cx| s.create_server_project(id_project.clone(), window, cx));
                    })),
            )
            .child(caption("Creates an empty Git project in ~/projects on the server and opens it here."));

        card = card.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(Button::new(SharedString::from(format!("server-invite-{id}"))).outline().small().label("Link for another Mac of mine").on_click(move |_, window, cx| {
                    m_invite.update(cx, |s, cx| s.invite_to_server(id_invite.clone(), false, window, cx));
                }))
                .child(Button::new(SharedString::from(format!("server-devices-{id}"))).ghost().small().label("Show devices").on_click(move |_, _, cx| {
                    m_devices.update(cx, |s, cx| s.load_server_devices(id_devices.clone(), cx));
                })),
        );
        if server.config.admin {
            let (m_person, id_person, m_people, id_people) = (model.clone(), id.clone(), model.clone(), id.clone());
            card = card
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(260.)).child(Input::new(&invitee)))
                        .child(Button::new(SharedString::from(format!("server-person-{id}"))).outline().small().label("Invite a person").on_click(move |_, window, cx| {
                            m_person.update(cx, |s, cx| s.invite_to_server(id_person.clone(), true, window, cx));
                        }))
                        .child(Button::new(SharedString::from(format!("server-people-{id}"))).ghost().small().label("Show people").on_click(move |_, _, cx| {
                            m_people.update(cx, |s, cx| s.load_server_people(id_people.clone(), cx));
                        })),
                )
                .child(caption("Each person gets their own account on the server, with their own projects, threads and AI sign-ins. You manage the server, so you can read anything stored on it, including theirs. Only invite people who are comfortable with that."));
            for person in people.get(&id).into_iter().flatten() {
                let name = person["name"].as_str().unwrap_or_default().to_string();
                let locked = person["locked"].as_bool().unwrap_or(false);
                let is_admin = person["admin"].as_bool().unwrap_or(false);
                let (m_lock, id_lock, lock_name) = (model.clone(), id.clone(), name.clone());
                card = card.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .h(px(28.))
                        .child(div().text_sm().font_family(ui.mono.clone()).child(name.clone()))
                        .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child(if is_admin { "admin" } else if locked { "locked" } else { "active" }))
                        .child(div().flex_1())
                        .when(!is_admin && !locked, |el| {
                            el.child(Button::new(SharedString::from(format!("server-lock-{name}"))).ghost().small().label("Lock out").on_click(move |_, window, cx| {
                                let (m, server, person) = (m_lock.clone(), id_lock.clone(), lock_name.clone());
                                window.open_alert_dialog(cx, move |dlg, _, _| {
                                    let (m, server, person) = (m.clone(), server.clone(), person.clone());
                                    dlg.confirm()
                                        .title(format!("Lock out {person}?"))
                                        .description("Their running threads stop, their Macs are disconnected and their account is locked. Their files stay on the server.")
                                        .on_ok(move |_, _, cx| { m.update(cx, |s, cx| s.lock_server_person(server.clone(), person.clone(), cx)); true })
                                });
                            }))
                        }),
                );
            }
        }
        if let Some((_, link_text)) = invite.as_ref().filter(|(for_server, _)| for_server == &id) {
            card = card.child(
                div()
                    .p_2()
                    .rounded(px(6.))
                    .bg(ui.ink(0.04))
                    .text_size(px(crate::theme::Type::SMALL))
                    .font_family(ui.mono.clone())
                    .child(link_text.clone()),
            )
            .child(caption("Paste this into Settings → Servers on the other Mac. It works once and expires in 10 minutes."));
        }
        for device in devices.get(&id).into_iter().flatten() {
            let device_id = device["id"].as_str().unwrap_or_default().to_string();
            let mine = device_id == server.config.device_id;
            let (m_revoke, id_revoke) = (model.clone(), id.clone());
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(28.))
                    .child(div().text_sm().child(device["label"].as_str().unwrap_or("Mac").to_string()))
                    .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child(format!("{}{}", device["user"].as_str().unwrap_or_default(), if mine { " · this Mac" } else { "" })))
                    .child(div().flex_1())
                    .when(!mine, |el| {
                        el.child(Button::new(SharedString::from(format!("server-revoke-{device_id}"))).ghost().small().label("Remove").on_click(move |_, _, cx| {
                            m_revoke.update(cx, |s, cx| s.revoke_server_device(id_revoke.clone(), device_id.clone(), cx));
                        }))
                    }),
            );
        }
        page = page.child(card);
    }

    let (install_target, install_public, install_log, installing) = {
        let m = model.read(cx);
        (m.install_target.clone(), m.install_public.clone(), m.install_log.clone(), m.installing)
    };
    let install_model = model.clone();
    page = page.child(
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("Set up a new server"))
            .child(caption("For a Linux server you can already reach with `ssh`. The app installs Bomb Code there for your login, starts it, and pairs this Mac. SSH is only used for this step."))
            .child(div().flex().items_center().gap_2().child(div().w(px(300.)).child(Input::new(&install_target))).child(div().w(px(300.)).child(Input::new(&install_public))))
            .child(div().flex().child(Button::new("server-install").outline().small().label(if installing { "Setting up…" } else { "Install and pair" }).disabled(installing).on_click(move |_, _, cx| {
                install_model.update(cx, |s, cx| s.install_server(cx));
            })))
            .children(install_log.into_iter().map(|line| div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child(line))),
    );
    page.child(
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("Pair with a server"))
            .child(caption("On the server, run `bombd up` (or `bombd invite`) and paste the link it prints. This Mac makes its own key for that server and keeps it in a private file, like an SSH key."))
            .child(Input::new(&link))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(260.)).child(Input::new(&name)))
                    .child(Button::new("server-pair").primary().small().label(if pairing { "Pairing…" } else { "Pair this Mac" }).disabled(pairing).on_click(move |_, window, cx| {
                        pair_model.update(cx, |s, cx| s.pair_server(window, cx));
                    })),
            ),
    )
    .into_any_element()
}

// ── Worktrees ───────────────────────────────────────────────────────────


// ── Advanced: thread folders ────────────────────────────────────────────

fn render_worktrees(cx: &mut App) -> AnyElement {
    let ui = Ui::of(cx);
    let model = settings(cx);
    let (repos, wt_name, diff) = {
        let m = model.read(cx);
        (m.worktrees.clone(), m.wt_name.clone(), m.diff_preview.clone())
    };
    let refresh_model = model.clone();
    div()
        .flex()
        .flex_col()
        .gap_3()
        .w_full()
        .child(
            toolbar().child(Button::new("wt-refresh").ghost().small().label("Refresh").on_click(move |_, _, cx| {
                refresh_model.update(cx, |s, cx| s.load_worktrees(cx));
            })),
        )
        .when(repos.is_empty(), |el| el.child(empty_list("No projects yet.", &ui)))
        .children(repos.into_iter().map(|(repo, list)| {
            let m_prune = model.clone();
            let m_create = model.clone();
            let (r1, r2) = (repo.clone(), repo.clone());
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .child(
                    row_line()
                        .pt_2()
                        .child(row_title(crate::models::app::project_name(&repo)))
                        .child(row_meta(repo.clone(), &ui))
                        .child(div().flex_1())
                        .child(Button::new(SharedString::from(format!("prune-{repo}"))).ghost().small().label("Clear missing folders").tooltip("Forget folders that no longer exist on disk").on_click(move |_, _, cx| {
                            m_prune.update(cx, |s, cx| s.prune_worktrees(r1.clone(), cx));
                        })),
                )
                .children(list.into_iter().map(|w| {
                    let m_diff = model.clone();
                    let m_rm = model.clone();
                    let path = w.path.display().to_string();
                    let (repo_rm, name_rm) = (repo.clone(), w.name.clone());
                    list_row(&ui).pl_3().child(
                    row_line()
                        .child(row_title(w.name.clone()).font_family(ui.mono.clone()))
                        .when_some(w.branch.clone(), |el, b| el.child(row_meta(b, &ui)))
                        .when(w.locked, |el| el.child(tag("locked", &ui)))
                        .child(div().flex_1())
                        .child(Button::new(SharedString::from(format!("diff-{}", w.id))).ghost().small().label("Diff").on_click(move |_, _, cx| {
                            m_diff.update(cx, |s, cx| s.show_diff(path.clone(), cx));
                        }))
                        .child(Button::new(SharedString::from(format!("wt-rm-{}", w.id))).ghost().small().label("Remove").on_click(move |_, window, cx| {
                            let m = m_rm.clone();
                            let (r, n) = (repo_rm.clone(), name_rm.clone());
                            window.open_alert_dialog(cx, move |dlg, _, _| {
                                let m = m.clone();
                                let (r, n) = (r.clone(), n.clone());
                                dlg.title(format!("Remove worktree {n}?")).description("Uncommitted changes in it are lost.").on_ok(move |_, _, cx| {
                                    m.update(cx, |s, cx| s.remove_worktree(r.clone(), n.clone(), cx));
                                    true
                                })
                            });
                        })))
                }))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .pl_3()
                        .child(div().w(px(240.)).child(Input::new(&wt_name)))
                        .child(Button::new(SharedString::from(format!("wt-create-{repo}"))).outline().small().label("Create").on_click(move |_, window, cx| {
                            m_create.update(cx, |s, cx| s.create_worktree(r2.clone(), window, cx));
                        })),
                )
        }))
        .when_some(diff, |el, (path, text)| {
            let m = model.clone();
            el.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child(format!("diff · {path}")))
                            .child(div().flex_1())
                            .child(Button::new("diff-close").ghost().small().label("Close").on_click(move |_, _, cx| {
                                m.update(cx, |s, cx| {
                                    s.diff_preview = None;
                                    cx.notify();
                                })
                            })),
                    )
                    .child(
                        div()
                            .p_2()
                            .rounded(px(6.))
                            .bg(ui.ink(0.04))
                            .border_1()
                            .border_color(ui.border)
                            .max_h(px(360.))
                            .overflow_hidden()
                            .text_size(px(crate::theme::Type::SMALL))
                            .font_family(ui.mono.clone())
                            .whitespace_normal()
                            .child(text),
                    ),
            )
        })
        .into_any_element()
}

// ── Permissions ─────────────────────────────────────────────────────────

fn permissions_page(cx: &App) -> SettingPage {
    let presets: Vec<(SharedString, SharedString)> = settings(cx)
        .read(cx)
        .presets
        .iter()
        .map(|p| (p.name.clone().into(), format!("{} · {}", p.name, p.description).into()))
        .collect();
    SettingPage::new("Permissions")
        .description("Allow rules skip the approval prompt; deny rules block even in yolo.")
        .group(
            SettingGroup::new()
                .title("Defaults")
                .item(SettingItem::new(
                    "Trust repositories by default",
                    SettingField::switch(
                        |cx| settings(cx).read(cx).config.as_ref().map(|c| c.permissions.trust_repo).unwrap_or(false),
                        |v, cx| settings(cx).update(cx, |m, cx| m.edit_config(|c| c.permissions.trust_repo = v, cx)),
                    ),
                ))
                .item(
                    SettingItem::new(
                        "Apply a preset",
                        SettingField::dropdown(
                            presets,
                            |_| SharedString::default(),
                            |v, cx| settings(cx).update(cx, |m, cx| {
                                let preset = m.presets.iter().find(|p| p.name == v.as_ref()).cloned();
                                if let Some(p) = preset {
                                    let allow: Vec<String> = p.rules.iter().filter(|r| format!("{:?}", r.decision).to_lowercase().contains("allow")).map(|r| r.pattern.clone()).collect();
                                    let deny: Vec<String> = p.rules.iter().filter(|r| format!("{:?}", r.decision).to_lowercase().contains("deny")).map(|r| r.pattern.clone()).collect();
                                    m.edit_config(|c| {
                                        c.permissions.allow = allow;
                                        c.permissions.deny = deny;
                                        c.sandbox_profile = p.sandbox;
                                    }, cx);
                                    m.load_config_rules_again();
                                }
                            }),
                        ),
                    )
                    .description("Replaces the rule lists below with the preset's rules."),
                ),
        )
        .group(SettingGroup::new().title("Rules").description("One per line. A matching Allow rule skips the question; a matching Deny rule refuses without asking.").item(SettingItem::render(|_, _, cx| render_rules(cx))))
}

impl SettingsModel {
    /// Force the rule textareas to re-sync from config on the next frame.
    pub fn load_config_rules_again(&mut self) {
        self.mark_rules_dirty();
    }
}

fn render_rules(cx: &mut App) -> AnyElement {
    let model = settings(cx);
    let (allow, deny) = {
        let m = model.read(cx);
        (m.allow_rules.clone(), m.deny_rules.clone())
    };
    let save_model = model.clone();
    div()
        .flex()
        .flex_col()
        .gap_2()
        .w_full()
        .child(row_title("Allow without asking"))
        .child(Textarea::new(&allow))
        .child(row_title("Never allow").pt_2())
        .child(Textarea::new(&deny))
        .child(div().flex().child(Button::new("rules-save").small().label("Save rules").on_click(move |_, _, cx| {
            save_model.update(cx, |s, cx| s.save_rules(cx));
        })))
        .into_any_element()
}

// ── Advanced ────────────────────────────────────────────────────────────

fn advanced_page() -> SettingPage {
    SettingPage::new("Advanced")
        .description("For troubleshooting. You rarely need anything here.")
        .group(
            SettingGroup::new()
                .title("Thread folders")
                .description("Each thread works in its own copy of the project. Closing a feature removes its folder; clear leftovers here.")
                .item(SettingItem::render(|_, _, cx| render_worktrees(cx))),
        )
        .group(
            SettingGroup::new()
                .title("This app")
                .description("What is installed and where things are stored.")
                .item(SettingItem::render(|_, _, cx| render_runtime(cx))),
        )
        .group(
            SettingGroup::new()
                .title("Connection log")
                .description("Raw messages between the app and the agent for the thread you have open.")
                .item(SettingItem::render(|_, _, cx| render_protocol_log(cx))),
        )
}

fn render_runtime(cx: &mut App) -> AnyElement {
    let ui = Ui::of(cx);
    let model = settings(cx);
    let runtime = model.read(cx).runtime.clone();
    let m_ck = model.clone();
    let m_sd = model.clone();
    let m_rf = model.clone();
    let row = |k: &str, v: String, ui: &Ui| {
        list_row(ui).child(
            row_line()
                .child(div().w(px(180.)).flex_shrink_0().child(row_title(k.to_string())))
                .child(div().min_w_0().text_size(px(crate::theme::Type::SMALL)).font_family(ui.mono.clone()).text_color(ui.text_muted).whitespace_normal().child(v)),
        )
    };
    div()
        .flex()
        .flex_col()
        .w_full()
        .when_some(runtime, |el, r| {
            el.child(row("Status", r.message.clone(), &ui))
                .child(row("Grok program", format!("{}{}", r.grok_binary, if r.grok_binary_exists { "" } else { " (missing)" }), &ui))
                .child(row("Grok version", r.grok_version.clone().unwrap_or_else(|| "?".into()), &ui))
                .child(row("Settings file", r.config_path.clone(), &ui))
                .child(row("Thread folders", r.worktrees_dir.clone(), &ui))
                .child(row("App data", r.home_dir.clone(), &ui))
                .child(row("Agents running now", r.session_count.to_string(), &ui))
                .child(row("Tool servers", r.mcp_count.to_string(), &ui))
                .child(row("xAI API key", if r.xai_api_key_present { "set".into() } else { "not set".into() }, &ui))
        })
        .child(
            div()
                .flex()
                .gap_2()
                .pt_2()
                .child(Button::new("rt-refresh").ghost().small().label("Refresh").on_click(move |_, _, cx| {
                    m_rf.update(cx, |s, cx| s.load_runtime(cx));
                }))
                .child(Button::new("rt-checkpoint").outline().small().label("Checkpoint database").on_click(move |_, _, cx| {
                    m_ck.update(cx, |s, cx| s.checkpoint(cx));
                }))
                .child(Button::new("rt-shutdown").danger().small().label("Shut down all agents").on_click(move |_, window, cx| {
                    let m = m_sd.clone();
                    window.open_alert_dialog(cx, move |dlg, _, _| {
                        let m = m.clone();
                        dlg.title("Shut down all agents?").description("Every live session is cancelled; transcripts are kept.").on_ok(move |_, _, cx| {
                            m.update(cx, |s, cx| s.shutdown_all(cx));
                            true
                        })
                    });
                })),
        )
        .into_any_element()
}

fn render_protocol_log(cx: &mut App) -> AnyElement {
    let ui = Ui::of(cx);
    let app = cx.global::<AppModelHandle>().0.clone();
    let lines: Vec<String> = app
        .read(cx)
        .selected_thread()
        .map(|t| t.read(cx).thread.protocol_log.iter().rev().take(200).cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    div()
        .w_full()
        .p_2()
        .rounded(px(6.))
        .bg(ui.ink(0.04))
        .border_1()
        .border_color(ui.border)
        .max_h(px(320.))
        .overflow_hidden()
        .text_size(px(crate::theme::Type::SMALL))
        .font_family(ui.mono.clone())
        .text_color(ui.text_muted)
        .whitespace_normal()
        .when(lines.is_empty(), |el| el.child("Open a thread in the main window to see its messages here."))
        .children(lines.into_iter().rev().map(|l| div().child(l)))
        .into_any_element()
}

fn routing_page() -> SettingPage {
    SettingPage::new("Smart Model Routing")
        .description("JEV can suggest a different connected model before sending. Switching always needs your click. Enabling sends your prompt and a short recent conversation excerpt to the selected routing provider; evaluation calls are billed by that provider.")
        .group(SettingGroup::new().title("Smart Model Routing")
            .item(SettingItem::new("Enable Smart Model Routing", SettingField::switch(
                |cx|settings(cx).read(cx).config.as_ref().is_some_and(|c|c.model_suggestions.enabled),
                |v,cx|settings(cx).update(cx,|m,cx|m.edit_config(|c|c.model_suggestions.enabled=v,cx)))))
            .item(SettingItem::new("Connection",SettingField::dropdown(
                vec![("typesafe".into(),"JEV direct · TypeSafe".into()),("vercel".into(),"Vercel AI Gateway".into())],
                |cx|settings(cx).read(cx).config.as_ref().map(|c|c.model_suggestions.connection.clone()).unwrap_or_else(||"typesafe".into()).into(),
                |v,cx|settings(cx).update(cx,|m,cx|{m.routing_status.clear();m.edit_config(|c|c.model_suggestions.connection=v.to_string(),cx)}))))
            .item(SettingItem::render(|_,_,cx| {
                let model=settings(cx);let m=model.read(cx);let input=m.routing_key.clone();let status=m.routing_status.clone();
                div().w_full().flex().flex_col().gap_2().child(Input::new(&input))
                    .child(div().flex().gap_2()
                        .child(Button::new("routing-save-key").label("Save key").on_click({let model=model.clone();move |_,w,cx|model.update(cx,|m,cx|m.save_routing_key(false,w,cx))}))
                        .child(Button::new("routing-remove-key").ghost().label("Remove saved key").on_click(move |_,w,cx|model.update(cx,|m,cx|m.save_routing_key(true,w,cx)))))

                    .child(div().text_size(px(crate::theme::Type::SMALL)).child(status)).into_any_element()
            })))
        .group(SettingGroup::new().title("Test connection")
            .description("After saving your key, verify access to JEV. This sends a small billed test request without project context.")
            .item(SettingItem::render(|_,_,cx| {
                let status=settings(cx).read(cx).routing_status.clone();
                div().w_full().flex().flex_col().gap_2()
                    .child(Button::new("routing-test").outline().label("Test connection").on_click(|_,_,cx|settings(cx).update(cx,|m,cx|m.test_routing_connection(cx))))
                    .child(div().text_sm().whitespace_normal().child(status)).into_any_element()
            })))
        .group(SettingGroup::new().title("Model preferences")
            .description("Editable starter preferences. Only signed-in providers and their discovered models are considered. Specific task preferences take priority over staying with the current model. Uncertain or failed evaluations use the current model.")
            .item(SettingItem::render(|_,_,cx| {
                let model=settings(cx);let input=model.read(cx).routing_guidelines.clone();
                div().flex().flex_col().gap_2().child(Textarea::new(&input))
                    .child(Button::new("routing-save-preferences").label("Save preferences").on_click(move |_,_,cx|model.update(cx,|m,cx| {
                        let text=m.routing_guidelines.read(cx).value().to_string();
                        m.edit_config(|c|c.model_suggestions.guidelines=text,cx);
                    }))).into_any_element()
            })))
}
