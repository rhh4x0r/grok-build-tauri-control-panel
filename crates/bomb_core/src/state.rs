//! Shared application state wired from all backend crates.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::RwLock;
use tracing::{info, warn};

use grok_cli_wrapper::{GrokCli, LoginManager};
use grok_config::{GrokConfig, GrokPaths, discover_environment};
use grok_control_core::SessionRegistry;
use grok_events::{EventBus, shared_bus};
use grok_extensions::ExtensionsService;
use grok_mcp::McpManager;
use grok_memory::MemoryService;
use grok_persistence::Persistence;
use grok_scheduler::{JobHandler, ScheduledJob, Scheduler};
use grok_worktree::WorktreeManager;

use crate::devserver::DevServerManager;
use crate::explainer::ExplainerService;

pub struct AppState {
    /// Saves and numbers every event; the UI and remote devices attach here.
    pub journal: Arc<crate::journal::Journal>,
    pub foundry: Arc<crate::foundry::FoundryService>,
    pub workspace_turns: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    pub workspace_gate: Arc<tokio::sync::Mutex<()>>,
    pub paths: GrokPaths,
    pub config: Arc<RwLock<GrokConfig>>,
    pub event_bus: Arc<EventBus>,
    pub grok_cli: Arc<GrokCli>,
    pub registry: Arc<SessionRegistry>,
    pub worktrees: Arc<WorktreeManager>,
    pub extensions: Arc<ExtensionsService>,
    pub mcp: Arc<McpManager>,
    pub memory: Arc<MemoryService>,
    pub scheduler: Arc<Scheduler>,
    pub persistence: Arc<Persistence>,
    pub dev_server: Arc<DevServerManager>,
    pub login: Arc<LoginManager>,
    pub explainer: Arc<ExplainerService>,
}

impl AppState {
    pub async fn initialize() -> Result<Self> {
        // Critical for macOS .app launches from Finder/Dock.
        grok_config::bootstrap_process_env();

        let paths = if std::env::var("BOMB_SMOKE").ok().as_deref() == Some("1") {
            // Smoke runs must never load/recover the user's live workflow database.
            let home =
                std::env::temp_dir().join(format!("bomb-code-smoke-{}", uuid::Uuid::new_v4()));
            let grok = home.join("grok");
            let panel = grok.join("panel");
            info!(path=%home.display(),"isolated smoke storage");
            GrokPaths {
                home_dir: home.clone(),
                grok_dir: grok.clone(),
                config_file: panel.join("config.toml"),
                grok_cli_config_file: grok.join("config.toml"),
                worktrees_dir: home.join("worktrees"),
                memory_dir: panel.join("memory"),
                sessions_dir: panel.join("sessions"),
                panel_dir: panel,
                project_config_file: None,
                project_root: None,
            }
        } else {
            GrokPaths::discover(std::env::current_dir().ok().as_deref())
                .context("path discovery")?
        };
        Self::initialize_with_paths(paths).await
    }

    pub async fn initialize_with_paths(paths: GrokPaths) -> Result<Self> {
        let _ = paths.ensure_dirs();

        // Resolve the binary against the BASE (global-only) config and save
        // that — saving the overlay-merged view would silently promote
        // project-scoped settings into the user's global config.
        let resolved_binary = match grok_config::discover_grok_binary() {
            Ok(bin) => {
                info!(binary = %bin.display(), "resolved grok binary");
                Some(bin)
            }
            Err(e) => {
                warn!(error = %e, "grok binary not found — install Grok Build CLI");
                None
            }
        };
        {
            let mut base = GrokConfig::load_base(&paths).unwrap_or_default();
            if resolved_binary.is_some() {
                base.grok_binary = resolved_binary.clone();
            }
            let _ = base.save(&paths.config_file);
        }

        // Runtime config: global + project overlay.
        let mut config = GrokConfig::load(&paths).unwrap_or_default();
        if resolved_binary.is_some() {
            config.grok_binary = resolved_binary;
        }

        let binary = config
            .resolve_grok_binary()
            .unwrap_or_else(|_| PathBuf::from("grok"));

        let config = Arc::new(RwLock::new(config));
        let event_bus = shared_bus();
        let grok_cli = Arc::new(GrokCli::new(binary));

        let registry = SessionRegistry::new(event_bus.clone(), config.clone(), grok_cli.clone());

        let worktrees_root = {
            let cfg = config.read().await;
            cfg.worktrees_root
                .clone()
                .unwrap_or_else(|| paths.worktrees_dir.clone())
        };
        let worktrees = Arc::new(WorktreeManager::new(grok_cli.clone(), worktrees_root));
        let _ = worktrees.ensure_root().await;

        let extensions = Arc::new(ExtensionsService::new(
            config.clone(),
            paths.clone(),
            grok_cli.clone(),
            event_bus.clone(),
        ));

        let mcp = McpManager::new(
            config.clone(),
            paths.clone(),
            grok_cli.clone(),
            event_bus.clone(),
        )
        .context("mcp manager")?;

        let memory = MemoryService::open(paths.memory_dir.clone(), event_bus.clone())
            .await
            .context("memory service")?;

        let persistence_path = paths.sessions_dir.join("control_panel.db");
        let persistence =
            Arc::new(Persistence::open(persistence_path).context("persistence open")?);

        // One-time compatibility migration; ordinary evaluations read SQLite only.
        let routing = config.read().await.model_suggestions.clone();
        if routing.enabled {
            if let Err(error) = crate::services::model_suggestions::migrate_legacy_key(
                persistence.clone(),
                routing.connection,
            )
            .await
            {
                warn!(%error, "routing credential migration incomplete");
            }
        }

        let scheduler = Scheduler::new(event_bus.clone());
        let registry_for_jobs = registry.clone();
        let persistence_for_jobs = persistence.clone();
        scheduler
            .set_handler(JobHandler::new(move |job: ScheduledJob| {
                let registry = registry_for_jobs.clone();
                let persistence = persistence_for_jobs.clone();
                async move {
                    info!(job_id = %job.id, name = %job.name, "scheduler firing job");
                    // A Finder-launched app's current_dir is `/` — running an
                    // agent from filesystem root is never what anyone wants.
                    let Some(cwd) = job.cwd.clone().filter(|c| !c.trim().is_empty()) else {
                        warn!(job_id = %job.id, "scheduled job has no cwd; skipping run");
                        let _ = persistence.set_kv(
                            &format!("last_job_error_{}", job.id),
                            "job has no cwd configured",
                        );
                        return;
                    };

                    // Prefer headless one-shot for scheduled routines
                    let opts = grok_control_core::SpawnOptions {
                        mode: grok_control_core::AgentMode::Headless,
                        prompt: Some(job.prompt.clone()),
                        plan_mode: true,
                        always_approve: false,
                        ..Default::default()
                    };

                    match registry.spawn_agent(&cwd, opts).await {
                        Ok(id) => {
                            let _ = persistence
                                .set_kv(&format!("last_job_{}", job.id), &id.to_string());
                        }
                        Err(e) => {
                            // Offline / no binary: record intent only
                            warn!(error = %e, "scheduler could not spawn agent");
                            let _ = persistence
                                .set_kv(&format!("last_job_error_{}", job.id), &e.to_string());
                        }
                    }
                }
            }))
            .await;

        // Durable routines: persist the job list on every change and reload
        // it at startup (jobs previously lived only in memory).
        {
            let persistence_for_sched = persistence.clone();
            scheduler
                .set_change_hook(move |jobs| {
                    if let Ok(json) = serde_json::to_string(&jobs) {
                        let _ = persistence_for_sched.set_kv("scheduler_jobs", &json);
                    }
                })
                .await;
            if let Ok(Some(json)) = persistence.get_kv("scheduler_jobs") {
                if let Ok(jobs) = serde_json::from_str::<Vec<ScheduledJob>>(&json) {
                    scheduler.restore_jobs(jobs).await;
                }
            }
        }

        // Discovery log (Phase 0)
        match discover_environment() {
            Ok(report) => info!(?report, "environment discovery"),
            Err(e) => warn!(error = %e, "environment discovery failed"),
        }

        let dev_server = DevServerManager::new();
        let login = LoginManager::new(grok_cli.grok_path.clone());

        // ELI12 narrator for the right panel (selected-thread side LLM calls).
        let explainer = {
            let cfg = config.read().await;
            ExplainerService::start(
                grok_cli.clone(),
                config.clone(),
                event_bus.clone(),
                cfg.explainer_enabled,
                cfg.explainer_backend.clone(),
                cfg.explainer_model.clone(),
            )
        };

        let foundry = Arc::new(
            crate::foundry::FoundryService::open(&paths.sessions_dir.join("foundry.db"))
                .map_err(anyhow::Error::msg)?,
        );
        let journal = crate::journal::Journal::start(&event_bus, persistence.clone(), foundry.clone());
        Ok(Self {
            journal,
            foundry,
            workspace_turns: Arc::new(std::sync::Mutex::new(Default::default())),
            workspace_gate: Arc::new(tokio::sync::Mutex::new(())),
            paths,
            config,
            event_bus,
            grok_cli,
            registry,
            worktrees,
            extensions,
            mcp,
            memory,
            scheduler,
            persistence,
            dev_server,
            login,
            explainer,
        })
    }
}
