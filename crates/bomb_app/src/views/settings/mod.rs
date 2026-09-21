//! In-window Settings screen (⌘,): General · MCP · Memory · Worktrees · Permissions ·
//! Diagnostics. Built on gpui-kit's `Settings` pages; the data-heavy tabs are
//! custom-rendered items reading [`SettingsModel`] through its global handle.

pub mod model;

use crate::views::button::Button;
use gpui_kit::component::button::ButtonVariants;
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
                        worktrees_page(),
                        permissions_page(cx),
                        diagnostics_page(),
                    ]),
            )
    }
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
    SettingPage::new("MCP")
        .description("Servers the agent can attach. Secrets live in ~/.grok/mcp_credentials.json, never in the config.")
        .group(
            SettingGroup::new()
                .title("Servers")
                .item(SettingItem::render(|_, _, cx| render_mcp_servers(cx))),
        )
        .group(
            SettingGroup::new()
                .title("Credentials")
                .description("Referenced by name from server templates (GITHUB_TOKEN, LINEAR_API_KEY, …).")
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
            div()
                .flex()
                .items_center()
                .gap_2()
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
                badges.push("high-risk".into());
            }
            if s.requires_approval {
                badges.push("needs approval".into());
            }
            if s.read_only {
                badges.push("read-only".into());
            }
            for k in &s.credential_keys {
                badges.push(format!("needs {k}"));
            }
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_3()
                .rounded(px(8.))
                .border_1()
                .border_color(ui.border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().size(px(8.)).rounded_full().bg(dot))
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(s.name.clone()))
                        .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child(format!("{} · {}", s.kind, s.transport.as_str())))
                        .children(badges.into_iter().map(|b| {
                            div().px_1p5().rounded(px(4.)).text_size(px(crate::theme::Type::SMALL)).bg(ui.ink(0.06)).text_color(ui.text_muted).child(b)
                        }))
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
    div()
        .flex()
        .flex_col()
        .gap_2()
        .w_full()
        .children(creds.into_iter().map(|c| {
            let m = model.clone();
            let k = c.key.clone();
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(div().text_sm().font_family(ui.mono.clone()).child(c.key.clone()))
                .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child(c.masked.clone()))
                .child(div().flex_1())
                .child(Button::new(SharedString::from(format!("cred-rm-{}", c.key))).ghost().small().label("Remove").on_click(move |_, _, cx| {
                    m.update(cx, |s, cx| s.remove_credential(k.clone(), cx));
                }))
        }))
        .child(
            div()
                .flex()
                .gap_2()
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
        .description("Notes injected into every new thread: global ones always, project ones for that project.")
        .group(SettingGroup::new().item(SettingItem::render(|_, _, cx| render_memory(cx))))
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
                .when(entries.is_empty(), |el| el.child(div().text_sm().text_color(ui.text_faint).py_2().child("No notes in this scope.")))
                .children(entries.into_iter().map(|e| {
                    let m_edit = model.clone();
                    let m_rm = model.clone();
                    let entry = e.clone();
                    let id = e.id.clone();
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_3()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(ui.border)
                        .child(div().text_sm().whitespace_normal().child(e.content.clone()))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .children(e.tags.iter().map(|t| {
                                    div().px_1p5().rounded(px(4.)).text_size(px(crate::theme::Type::SMALL)).bg(ui.ink(0.06)).text_color(ui.text_muted).child(t.clone())
                                }))
                                .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child(e.updated_at.format("%b %-d, %Y").to_string()))
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
            div()
                .flex()
                .gap_2()
                .child(div().flex_1().child(Input::new(&add)))
                .child(div().w(px(200.)).child(Input::new(&tags)))
                .child(Button::new("mem-add").small().label("Add").on_click(move |_, window, cx| {
                    add_model.update(cx, |s, cx| s.add_memory(window, cx));
                })),
        )
        .into_any_element()
}

// ── Worktrees ───────────────────────────────────────────────────────────

fn worktrees_page() -> SettingPage {
    SettingPage::new("Worktrees")
        .description("Every thread in a git project works in its own worktree under ~/.grok/worktrees.")
        .group(SettingGroup::new().item(SettingItem::render(|_, _, cx| render_worktrees(cx))))
}

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
            div().flex().items_center().gap_2().child(Button::new("wt-refresh").ghost().small().label("Refresh").on_click(move |_, _, cx| {
                refresh_model.update(cx, |s, cx| s.load_worktrees(cx));
            })),
        )
        .when(repos.is_empty(), |el| el.child(div().text_sm().text_color(ui.text_faint).child("No projects yet.")))
        .children(repos.into_iter().map(|(repo, list)| {
            let m_prune = model.clone();
            let m_create = model.clone();
            let (r1, r2) = (repo.clone(), repo.clone());
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(crate::models::app::project_name(&repo)))
                        .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child(repo.clone()))
                        .child(div().flex_1())
                        .child(Button::new(SharedString::from(format!("prune-{repo}"))).ghost().small().label("Prune").on_click(move |_, _, cx| {
                            m_prune.update(cx, |s, cx| s.prune_worktrees(r1.clone(), cx));
                        })),
                )
                .children(list.into_iter().map(|w| {
                    let m_diff = model.clone();
                    let m_rm = model.clone();
                    let path = w.path.display().to_string();
                    let (repo_rm, name_rm) = (repo.clone(), w.name.clone());
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .pl_3()
                        .h(px(28.))
                        .child(div().text_sm().font_family(ui.mono.clone()).child(w.name.clone()))
                        .when_some(w.branch.clone(), |el, b| el.child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child(b)))
                        .when(w.locked, |el| el.child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.warning).child("locked")))
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
                        }))
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
        .group(SettingGroup::new().title("Rules").item(SettingItem::render(|_, _, cx| render_rules(cx))))
}

impl SettingsModel {
    /// Force the rule textareas to re-sync from config on the next frame.
    pub fn load_config_rules_again(&mut self) {
        self.mark_rules_dirty();
    }
}

fn render_rules(cx: &mut App) -> AnyElement {
    let ui = Ui::of(cx);
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
        .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child("Allow"))
        .child(Textarea::new(&allow))
        .child(div().text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_muted).child("Deny"))
        .child(Textarea::new(&deny))
        .child(div().flex().child(Button::new("rules-save").small().label("Save rules").on_click(move |_, _, cx| {
            save_model.update(cx, |s, cx| s.save_rules(cx));
        })))
        .into_any_element()
}

// ── Diagnostics ─────────────────────────────────────────────────────────

fn diagnostics_page() -> SettingPage {
    SettingPage::new("Diagnostics")
        .group(SettingGroup::new().title("Runtime").item(SettingItem::render(|_, _, cx| render_runtime(cx))))
        .group(
            SettingGroup::new()
                .title("Protocol log")
                .description("Raw ACP and stderr lines from the selected thread.")
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
        div()
            .flex()
            .gap_3()
            .h(px(22.))
            .items_center()
            .child(div().w(px(140.)).text_size(px(crate::theme::Type::SMALL)).text_color(ui.text_faint).child(k.to_string()))
            .child(div().text_size(px(crate::theme::Type::SMALL)).font_family(ui.mono.clone()).text_color(ui.text).child(v))
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .w_full()
        .when_some(runtime, |el, r| {
            el.child(row("status", r.message.clone(), &ui))
                .child(row("grok binary", format!("{}{}", r.grok_binary, if r.grok_binary_exists { "" } else { " (missing)" }), &ui))
                .child(row("grok version", r.grok_version.clone().unwrap_or_else(|| "?".into()), &ui))
                .child(row("config", r.config_path.clone(), &ui))
                .child(row("worktrees", r.worktrees_dir.clone(), &ui))
                .child(row("home", r.home_dir.clone(), &ui))
                .child(row("live sessions", r.session_count.to_string(), &ui))
                .child(row("MCP servers", r.mcp_count.to_string(), &ui))
                .child(row("XAI_API_KEY", if r.xai_api_key_present { "present".into() } else { "not set".into() }, &ui))
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
        .when(lines.is_empty(), |el| el.child("Select a thread in the main window to see its protocol log."))
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
