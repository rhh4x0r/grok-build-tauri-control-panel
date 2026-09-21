//! State behind the Settings window: config draft, MCP servers, memory,
//! worktrees, runtime status, plus the input entities the pages edit through.

use std::collections::HashMap;

use bomb_core::services::{self, RuntimeStatus};
use gpui_kit::component::input::{InputState, TextareaState};
use gpui_kit::*;
use grok_config::GrokConfig;
use grok_mcp::{AddMcpRequest, DoctorReport, McpCatalogEntry, McpCredential, McpServerConfigExt, UpdateMcpRequest};
use grok_memory::MemoryEntry;
use grok_permissions::PermissionPreset;
use grok_worktree::WorktreeInfo;

use crate::models::app::{AppModel, AppModelHandle, ToastKind};
use crate::runtime::{services as svc, spawn_service};

pub struct SettingsHandle(pub Entity<SettingsModel>);
impl Global for SettingsHandle {}

pub struct SettingsModel {
    pub config: Option<GrokConfig>,
    pub mcp: Vec<McpServerConfigExt>,
    pub catalog: Vec<McpCatalogEntry>,
    pub credentials: Vec<McpCredential>,
    pub doctor: HashMap<String, DoctorReport>,
    pub memory: Vec<MemoryEntry>,
    pub memory_scope: String,
    pub project_scope: Option<String>,
    pub worktrees: Vec<(String, Vec<WorktreeInfo>)>,
    pub diff_preview: Option<(String, String)>,
    pub runtime: Option<RuntimeStatus>,
    pub presets: Vec<PermissionPreset>,
    pub busy: Option<String>,

    pub mem_search: Entity<InputState>,
    pub mem_add: Entity<InputState>,
    pub mem_tags: Entity<InputState>,
    pub cred_key: Entity<InputState>,
    pub cred_value: Entity<InputState>,
    pub wt_name: Entity<InputState>,
    pub server_link: Entity<InputState>,
    pub server_name: Entity<InputState>,
    pub server_project: Entity<InputState>,
    pub server_invitee: Entity<InputState>,
    /// Devices per server id, as last listed.
    pub server_devices: std::collections::HashMap<String, Vec<serde_json::Value>>,
    /// People per server id (admin only), as last listed.
    pub server_people: std::collections::HashMap<String, Vec<serde_json::Value>>,
    /// A freshly made pairing link to show once, per server id.
    pub server_invite: Option<(String, String)>,
    pub allow_rules: Entity<TextareaState>,
    pub deny_rules: Entity<TextareaState>,
    rules_loaded: bool,
    pub routing_key: Entity<InputState>,
    pub routing_guidelines: Entity<TextareaState>,
    pub routing_status: String,
}

impl SettingsModel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mk = |ph: &str, window: &mut Window, cx: &mut Context<Self>| {
            let ph = ph.to_string();
            cx.new(|cx| InputState::new(window, cx).placeholder(ph))
        };
        let this = Self {
            config: None,
            mcp: Vec::new(),
            catalog: Vec::new(),
            credentials: Vec::new(),
            doctor: HashMap::new(),
            memory: Vec::new(),
            memory_scope: "global".into(),
            project_scope: None,
            worktrees: Vec::new(),
            diff_preview: None,
            runtime: None,
            presets: Vec::new(),
            busy: None,
            mem_search: mk("Search notes…", window, cx),
            mem_add: mk("Add a note the agent should always know…", window, cx),
            mem_tags: mk("tags, comma separated", window, cx),
            cred_key: mk("KEY (e.g. GITHUB_TOKEN)", window, cx),
            cred_value: mk("secret value", window, cx),
            wt_name: mk("new worktree name", window, cx),
            server_link: mk("bomb://pair?…  (pairing link from your server)", window, cx),
            server_name: mk("Name for this server, e.g. My VPS", window, cx),
            server_project: mk("new project name", window, cx),
            server_invitee: mk("Who is this for? e.g. Sam", window, cx),
            server_devices: Default::default(),
            server_people: Default::default(),
            server_invite: None,
            allow_rules: cx.new(|cx| TextareaState::new(window, cx).placeholder("one pattern per line, e.g. Bash(cargo test *)").auto_grow(3, 8)),
            deny_rules: cx.new(|cx| TextareaState::new(window, cx).placeholder("one pattern per line, e.g. Bash(rm -rf *)").auto_grow(3, 8)),
            rules_loaded: false,
            routing_key: cx.new(|cx| InputState::new(window,cx).masked(true).placeholder("Paste API key")),
            routing_guidelines: cx.new(|cx| TextareaState::new(window,cx).placeholder("Which models should handle which tasks?").auto_grow(3,8)),
            routing_status: String::new(),
        };
        // Any edit in an input repaints the window.
        for e in [&this.mem_search, &this.mem_add, &this.mem_tags, &this.cred_key, &this.cred_value, &this.wt_name] {
            cx.observe(e, |_, _, cx| cx.notify()).detach();
        }
        this
    }

    fn app(cx: &App) -> Entity<AppModel> {
        cx.global::<AppModelHandle>().0.clone()
    }

    fn toast(&self, kind: ToastKind, msg: impl Into<String>, cx: &mut Context<Self>) {
        let app = Self::app(cx);
        let msg = msg.into();
        app.update(cx, |m, cx| {
            m.toast(kind, msg);
            cx.notify();
        });
    }

    pub fn load_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_config(window, cx);
        self.load_mcp(cx);
        self.load_memory(cx);
        self.load_worktrees(cx);
        self.load_runtime(cx);
        self.load_presets(cx);
    }

    // ── config ──────────────────────────────────────────────────────────

    pub fn load_config(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::get_config(&state).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                if let Ok(cfg) = res {
                    m.config = Some(cfg);
                    m.rules_loaded = false;
                    cx.notify();
                }
            });
        });
    }

    /// Push the current permission rule text into the textareas once per load.
    pub fn pair_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let link = self.server_link.read(cx).value().trim().to_string();
        let name = self.server_name.read(cx).value().trim().to_string();
        if link.is_empty() { return; }
        self.server_link.update(cx, |s, cx| s.set_value("", window, cx));
        self.server_name.update(cx, |s, cx| s.set_value("", window, cx));
        let app = cx.global::<crate::models::app::AppModelHandle>().0.clone();
        app.update(cx, |m, cx| m.pair_server(link, name, cx));
    }

    pub fn create_server_project(&mut self, server: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.server_project.read(cx).value().trim().to_string();
        if name.is_empty() { return; }
        self.server_project.update(cx, |s, cx| s.set_value("", window, cx));
        let app = cx.global::<crate::models::app::AppModelHandle>().0.clone();
        app.update(cx, |m, cx| m.create_server_project(server, name, cx));
    }

    pub fn load_server_devices(&mut self, server: String, cx: &mut Context<Self>) {
        let Some(remote) = crate::runtime::servers(cx).get(&server) else { return; };
        let weak = cx.entity().downgrade();
        crate::runtime::spawn_service(cx, async move { remote.gateway("gateway.list_devices", serde_json::Value::Null).await }, move |res, cx| {
            let _ = weak.update(cx, |s, cx| {
                match res {
                    Ok(list) => { s.server_devices.insert(server, list.as_array().cloned().unwrap_or_default()); }
                    Err(e) => s.toast(ToastKind::Error, e, cx),
                }
                cx.notify();
            });
        });
    }

    pub fn revoke_server_device(&mut self, server: String, device: String, cx: &mut Context<Self>) {
        let Some(remote) = crate::runtime::servers(cx).get(&server) else { return; };
        let weak = cx.entity().downgrade();
        crate::runtime::spawn_service(cx, async move { remote.gateway("gateway.revoke_device", serde_json::json!({ "id": device })).await }, move |res, cx| {
            let _ = weak.update(cx, |s, cx| {
                if let Err(e) = res { s.toast(ToastKind::Error, e, cx); }
                s.load_server_devices(server, cx);
            });
        });
    }

    /// A one-time link for another Mac of mine (`person` false), or, as admin, a new person with their own account.
    pub fn invite_to_server(&mut self, server: String, person: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(remote) = crate::runtime::servers(cx).get(&server) else { return; };
        let label = self.server_invitee.read(cx).value().trim().to_string();
        if person { self.server_invitee.update(cx, |s, cx| s.set_value("", window, cx)); }
        let (method, params) = if person { ("gateway.invite_person", serde_json::json!({ "label": label })) } else { ("gateway.create_invite", serde_json::json!({})) };
        let weak = cx.entity().downgrade();
        crate::runtime::spawn_service(cx, async move { remote.gateway(method, params).await }, move |res, cx| {
            let _ = weak.update(cx, |s, cx| {
                match res {
                    Ok(v) => { s.server_invite = Some((server.clone(), v["link"].as_str().unwrap_or_default().to_string())); if person { s.load_server_people(server, cx); } }
                    Err(e) => s.toast(ToastKind::Error, e, cx),
                }
                cx.notify();
            });
        });
    }

    pub fn load_server_people(&mut self, server: String, cx: &mut Context<Self>) {
        let Some(remote) = crate::runtime::servers(cx).get(&server) else { return; };
        let weak = cx.entity().downgrade();
        crate::runtime::spawn_service(cx, async move { remote.gateway("gateway.list_users", serde_json::Value::Null).await }, move |res, cx| {
            let _ = weak.update(cx, |s, cx| {
                match res {
                    Ok(list) => { s.server_people.insert(server, list.as_array().cloned().unwrap_or_default()); }
                    Err(e) => s.toast(ToastKind::Error, e, cx),
                }
                cx.notify();
            });
        });
    }

    pub fn lock_server_person(&mut self, server: String, person: String, cx: &mut Context<Self>) {
        let Some(remote) = crate::runtime::servers(cx).get(&server) else { return; };
        let weak = cx.entity().downgrade();
        crate::runtime::spawn_service(cx, async move { remote.gateway("gateway.lock_person", serde_json::json!({ "user": person })).await }, move |res, cx| {
            let _ = weak.update(cx, |s, cx| {
                if let Err(e) = res { s.toast(ToastKind::Error, e, cx); }
                s.load_server_people(server, cx);
            });
        });
    }

    pub fn sync_rule_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rules_loaded {
            return;
        }
        let Some(cfg) = &self.config else { return };
        let allow = cfg.permissions.allow.join("\n");
        let deny = cfg.permissions.deny.join("\n");
        self.allow_rules.update(cx, |s, cx| s.set_value(allow, window, cx));
        self.deny_rules.update(cx, |s, cx| s.set_value(deny, window, cx));
        self.routing_guidelines.update(cx, |s,cx| s.set_value(cfg.model_suggestions.guidelines.clone(),window,cx));
        self.rules_loaded = true;
    }

    pub fn save_routing_key(&mut self, remove: bool, window: &mut Window, cx: &mut Context<Self>) {
        let connection = self.config.as_ref().map(|c|c.model_suggestions.connection.clone()).unwrap_or_default();
        let db=svc(cx).persistence.clone();
        let key = self.routing_key.read(cx).value().to_string();
        self.routing_key.update(cx,|s,cx|s.set_value("",window,cx));
        self.routing_status = "Updating saved key…".into();
        let weak=cx.entity().downgrade();
        spawn_service(cx,async move {
            if remove { services::model_suggestions::remove_key(db,connection).await }
            else { services::model_suggestions::save_key(db,connection,key).await }
        },move |result,cx| {let _=weak.update(cx,|m,cx| {
            m.routing_status=match result {Ok(())=>if remove {"Saved key removed.".into()}else{"API key saved in the settings database.".into()},Err(e)=>e};cx.notify();
        });});cx.notify();
    }

    pub fn test_routing_connection(&mut self, cx: &mut Context<Self>) {
        let connection=self.config.as_ref().map(|c|c.model_suggestions.connection.clone()).unwrap_or_default();
        let db=svc(cx).persistence.clone();
        self.routing_status="Testing saved key…".into();
        let weak=cx.entity().downgrade();
        spawn_service(cx,async move {services::model_suggestions::test_connection(db,connection.clone()).await.map(|()|connection)},move |result,cx| {
            let _=weak.update(cx,|m,cx| {
                m.routing_status=match result {Ok(provider)=>format!("Connected to {provider}."),Err(e)=>e};cx.notify();
            });
        });cx.notify();
    }

    pub fn mark_rules_dirty(&mut self) {
        self.rules_loaded = false;
    }

    pub fn edit_config(&mut self, f: impl FnOnce(&mut GrokConfig), cx: &mut Context<Self>) {
        let Some(cfg) = self.config.as_mut() else { return };
        f(cfg);
        let cfg = cfg.clone();
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::save_config(&state, cfg).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                if let Err(e) = res {
                    m.toast(ToastKind::Error, format!("Could not save settings: {e}"), cx);
                }
            });
        });
        cx.notify();
    }

    pub fn save_rules(&mut self, cx: &mut Context<Self>) {
        let allow: Vec<String> = self.allow_rules.read(cx).value().lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
        let deny: Vec<String> = self.deny_rules.read(cx).value().lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
        self.edit_config(|c| {
            c.permissions.allow = allow;
            c.permissions.deny = deny;
        }, cx);
        self.toast(ToastKind::Success, "Permission rules saved", cx);
    }

    pub fn set_explainer(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let state = svc(cx);
        spawn_service(cx, async move { services::set_explainer_enabled(&state, enabled).await }, |_, _| {});
        self.edit_config(|c| c.explainer_enabled = enabled, cx);
    }

    pub fn set_explainer_provider(&mut self, backend: Option<String>, model: Option<String>, cx: &mut Context<Self>) {
        let state = svc(cx);
        let (b, m) = (backend.clone(), model.clone());
        spawn_service(cx, async move { services::set_explainer_provider(&state, b, m).await }, |_, _| {});
        self.edit_config(|c| {
            c.explainer_backend = backend;
            c.explainer_model = model;
        }, cx);
    }

    // ── mcp ─────────────────────────────────────────────────────────────

    pub fn load_mcp(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move {
                let servers = services::list_mcp_servers(&state).await;
                let creds = services::list_mcp_credentials(&state).await;
                let catalog = services::list_mcp_catalog().await;
                (servers, creds, catalog)
            },
            move |(servers, creds, catalog), cx| {
                let _ = this.update(cx, |m, cx| {
                    if let Ok(s) = servers {
                        m.mcp = s;
                    }
                    if let Ok(c) = creds {
                        m.credentials = c;
                    }
                    if let Ok(c) = catalog {
                        m.catalog = c;
                    }
                    cx.notify();
                });
            },
        );
    }

    pub fn toggle_mcp(&mut self, name: String, enabled: bool, cx: &mut Context<Self>) {
        if let Some(s) = self.mcp.iter_mut().find(|s| s.name == name) {
            s.enabled = enabled;
        }
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::toggle_mcp(&state, name, enabled).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                if let Err(e) = res {
                    m.toast(ToastKind::Error, e, cx);
                }
                m.load_mcp(cx);
            });
        });
        cx.notify();
    }

    pub fn remove_mcp(&mut self, name: String, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::remove_mcp_server(&state, name).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                if let Err(e) = res {
                    m.toast(ToastKind::Error, e, cx);
                }
                m.load_mcp(cx);
            });
        });
    }

    pub fn doctor(&mut self, name: Option<String>, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        self.busy = Some("checking…".into());
        cx.notify();
        spawn_service(cx, async move { services::doctor_mcp_server(&state, name).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                m.busy = None;
                match res {
                    Ok(reports) => {
                        for r in reports {
                            m.doctor.insert(r.name.clone(), r);
                        }
                    }
                    Err(e) => m.toast(ToastKind::Error, e, cx),
                }
                cx.notify();
            });
        });
    }

    pub fn add_from_catalog(&mut self, entry: &McpCatalogEntry, cx: &mut Context<Self>) {
        let t = &entry.template;
        let req = AddMcpRequest {
            name: entry.default_name.clone(),
            kind: Some(entry.id.clone()),
            transport: Some(entry.transport.as_str().to_string()),
            command: t.command.clone(),
            args: Some(t.args.clone()),
            url: t.url.clone(),
            env: Some(t.env.clone()),
            enabled: Some(true),
            scope: None,
            description: Some(entry.description.clone()),
            allowed_paths: None,
            read_only: Some(t.read_only),
            auto_attach: Some(t.auto_attach),
            requires_approval: Some(entry.requires_approval),
            ..Default::default()
        };
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::add_mcp_server(&state, req).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                match res {
                    Ok(s) => m.toast(ToastKind::Success, format!("Added {}", s.name), cx),
                    Err(e) => m.toast(ToastKind::Error, e, cx),
                }
                m.load_mcp(cx);
            });
        });
    }

    pub fn set_auto_attach(&mut self, name: String, auto: bool, cx: &mut Context<Self>) {
        let req = UpdateMcpRequest {
            name,
            auto_attach: Some(auto),
            ..Default::default()
        };
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::update_mcp_server(&state, req).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                if let Err(e) = res {
                    m.toast(ToastKind::Error, e, cx);
                }
                m.load_mcp(cx);
            });
        });
    }

    pub fn save_credential(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.cred_key.read(cx).value().trim().to_string();
        let value = self.cred_value.read(cx).value().trim().to_string();
        if key.is_empty() || value.is_empty() {
            return;
        }
        self.cred_key.update(cx, |s, cx| s.set_value("", window, cx));
        self.cred_value.update(cx, |s, cx| s.set_value("", window, cx));
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::set_mcp_credential(&state, key, value).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                match res {
                    Ok(()) => m.toast(ToastKind::Success, "Credential saved", cx),
                    Err(e) => m.toast(ToastKind::Error, e, cx),
                }
                m.load_mcp(cx);
            });
        });
    }

    pub fn remove_credential(&mut self, key: String, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::remove_mcp_credential(&state, key).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                if let Err(e) = res {
                    m.toast(ToastKind::Error, e, cx);
                }
                m.load_mcp(cx);
            });
        });
    }

    // ── memory ──────────────────────────────────────────────────────────

    pub fn load_memory(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        let project = Self::app(cx).read(cx).active_project.clone();
        spawn_service(
            cx,
            async move {
                let scope = match project {
                    Some(p) => services::project_scope(p).await.ok(),
                    None => None,
                };
                let entries = services::memory_list(&state, None).await;
                (scope, entries)
            },
            move |(scope, entries), cx| {
                let _ = this.update(cx, |m, cx| {
                    m.project_scope = scope;
                    if let Ok(e) = entries {
                        m.memory = e;
                    }
                    cx.notify();
                });
            },
        );
    }

    pub fn visible_memory(&self, cx: &App) -> Vec<MemoryEntry> {
        let q = self.mem_search.read(cx).value().to_lowercase();
        let mut v: Vec<MemoryEntry> = self
            .memory
            .iter()
            .filter(|e| e.scope == self.memory_scope)
            .filter(|e| q.is_empty() || e.content.to_lowercase().contains(&q) || e.tags.iter().any(|t| t.to_lowercase().contains(&q)))
            .cloned()
            .collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.updated_at));
        v
    }

    pub fn add_memory(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let content = self.mem_add.read(cx).value().trim().to_string();
        if content.is_empty() {
            return;
        }
        let tags: Vec<String> = self.mem_tags.read(cx).value().split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
        self.mem_add.update(cx, |s, cx| s.set_value("", window, cx));
        self.mem_tags.update(cx, |s, cx| s.set_value("", window, cx));
        let scope = self.memory_scope.clone();
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::memory_add(&state, scope, content, tags).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                if let Err(e) = res {
                    m.toast(ToastKind::Error, e, cx);
                }
                m.load_memory(cx);
            });
        });
    }

    pub fn remove_memory(&mut self, id: String, cx: &mut Context<Self>) {
        self.memory.retain(|e| e.id != id);
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::memory_remove(&state, id).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                if let Err(e) = res {
                    m.toast(ToastKind::Error, e, cx);
                }
                m.load_memory(cx);
            });
        });
        cx.notify();
    }

    /// Edit = load the note into the add row and drop the original.
    pub fn edit_memory(&mut self, entry: &MemoryEntry, window: &mut Window, cx: &mut Context<Self>) {
        let content = entry.content.clone();
        let tags = entry.tags.join(", ");
        self.mem_add.update(cx, |s, cx| s.set_value(content, window, cx));
        self.mem_tags.update(cx, |s, cx| s.set_value(tags, window, cx));
        self.remove_memory(entry.id.clone(), cx);
    }

    pub fn digest_memory(&mut self, cx: &mut Context<Self>) {
        let scope = self.memory_scope.clone();
        let state = svc(cx);
        let this = cx.entity().downgrade();
        self.busy = Some("digesting…".into());
        cx.notify();
        spawn_service(cx, async move { services::memory_digest(&state, scope).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                m.busy = None;
                match res {
                    Ok(_) => m.toast(ToastKind::Success, "Digest added", cx),
                    Err(e) => m.toast(ToastKind::Error, e, cx),
                }
                m.load_memory(cx);
            });
        });
    }

    pub fn export_memory(&mut self, cx: &mut Context<Self>) {
        let scope = self.memory_scope.clone();
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::memory_flush(&state, scope).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| match res {
                Ok(path) => m.toast(ToastKind::Success, format!("Exported to {path}"), cx),
                Err(e) => m.toast(ToastKind::Error, e, cx),
            });
        });
    }

    // ── worktrees ───────────────────────────────────────────────────────

    pub fn load_worktrees(&mut self, cx: &mut Context<Self>) {
        let repos = Self::app(cx).read(cx).projects.clone();
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(
            cx,
            async move {
                let mut out = Vec::new();
                for r in repos {
                    if let Ok(list) = services::list_worktrees(&state, r.clone()).await {
                        out.push((r, list));
                    }
                }
                out
            },
            move |list, cx| {
                let _ = this.update(cx, |m, cx| {
                    m.worktrees = list;
                    cx.notify();
                });
            },
        );
    }

    pub fn create_worktree(&mut self, repo: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.wt_name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        self.wt_name.update(cx, |s, cx| s.set_value("", window, cx));
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::create_worktree(&state, repo, name, None).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                if let Err(e) = res {
                    m.toast(ToastKind::Error, e, cx);
                }
                m.load_worktrees(cx);
            });
        });
    }

    pub fn remove_worktree(&mut self, repo: String, name: String, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::remove_worktree(&state, repo, name, true).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                if let Err(e) = res {
                    m.toast(ToastKind::Error, e, cx);
                }
                m.load_worktrees(cx);
            });
        });
    }

    pub fn prune_worktrees(&mut self, repo: String, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::prune_worktrees(&state, repo).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                match res {
                    Ok(out) => m.toast(ToastKind::Info, if out.trim().is_empty() { "Nothing to prune".into() } else { out }, cx),
                    Err(e) => m.toast(ToastKind::Error, e, cx),
                }
                m.load_worktrees(cx);
            });
        });
    }

    pub fn show_diff(&mut self, path: String, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        let p = path.clone();
        spawn_service(cx, async move { services::worktree_diff(&state, p).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                match res {
                    Ok(d) => m.diff_preview = Some((path.clone(), if d.trim().is_empty() { "(no changes)".into() } else { d })),
                    Err(e) => m.toast(ToastKind::Error, e, cx),
                }
                cx.notify();
            });
        });
    }

    // ── diagnostics ─────────────────────────────────────────────────────

    pub fn load_runtime(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::get_runtime_status(&state).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                if let Ok(r) = res {
                    m.runtime = Some(r);
                    cx.notify();
                }
            });
        });
    }

    pub fn load_presets(&mut self, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::list_permission_presets().await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                if let Ok(p) = res {
                    m.presets = p;
                    cx.notify();
                }
            });
        });
    }

    pub fn checkpoint(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::persistence_checkpoint(&state).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| match res {
                Ok(()) => m.toast(ToastKind::Success, "Database checkpointed", cx),
                Err(e) => m.toast(ToastKind::Error, e, cx),
            });
        });
    }

    pub fn shutdown_all(&mut self, cx: &mut Context<Self>) {
        let state = svc(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move { services::shutdown_all(&state).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| {
                match res {
                    Ok(()) => m.toast(ToastKind::Success, "All agents shut down", cx),
                    Err(e) => m.toast(ToastKind::Error, e, cx),
                }
                Self::app(cx).update(cx, |a, cx| a.refresh_threads(cx));
                m.load_runtime(cx);
            });
        });
    }
}
