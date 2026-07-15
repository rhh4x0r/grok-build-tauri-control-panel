//! Tauri application library — state, commands, and event bridge.

mod commands;
mod devserver;
mod explainer;
mod haven;
mod state;

use tauri::{Emitter, Manager};
use tracing::{info, warn};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_handle = app.handle().clone();
            let state = tauri::async_runtime::block_on(AppState::initialize())?;
            let bus = state.event_bus.clone();
            let bus_persist = state.event_bus.clone();
            let persistence = state.persistence.clone();
            let db_path = persistence.path().display().to_string();
            app.manage(state);

            // Forward backend events to the frontend
            tauri::async_runtime::spawn(async move {
                let mut rx = bus.subscribe();
                loop {
                    match rx.recv().await {
                        Ok(ev) => {
                            let _ = app_handle.emit("control-event", &ev);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(n, "event bus lagged");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });

            // Durable thread memory — survive reboot / app updates
            tauri::async_runtime::spawn(async move {
                let mut rx = bus_persist.subscribe();
                loop {
                    match rx.recv().await {
                        Ok(ev) => {
                            commands::persist_control_event(&persistence, &ev);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            // Leave a visible marker: silence here reads as a
                            // complete transcript when rows were dropped.
                            warn!(n, "persistence event bus lagged");
                            let _ = persistence.set_kv(
                                "last_transcript_gap",
                                &format!("{} events dropped at {}", n, chrono::Utc::now()),
                            );
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });

            info!(db = %db_path, "Bomb Code backend ready (SQLite thread memory)");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::discover_environment,
            commands::list_backends,
            commands::get_config,
            commands::save_config,
            commands::capture_baseline,
            commands::get_runtime_status,
            commands::set_last_cwd,
            commands::create_project_folder,
            commands::get_auth_status,
            commands::backend_auth_status,
            commands::open_backend_login,
            commands::start_grok_login,
            commands::start_grok_login_oauth,
            commands::grok_login_status,
            commands::submit_grok_login_code,
            commands::open_grok_login_url,
            commands::cancel_grok_login,
            commands::logout_grok,
            commands::start_session,
            commands::start_mock_session,
            commands::list_sessions,
            commands::list_threads,
            commands::get_session,
            commands::get_session_transcript,
            commands::send_prompt,
            commands::agent_supports_images,
            commands::cancel_session,
            commands::remove_session,
            commands::set_plan_mode,
            commands::set_always_approve,
            commands::set_approval_mode,
            commands::add_session_allow_rule,
            commands::explainer_focus,
            commands::set_explainer_enabled,
            commands::set_explainer_provider,
            commands::respond_approval,
            commands::rename_thread,
            commands::land_thread,
            commands::sync_thread,
            commands::list_projects,
            commands::add_project,
            commands::remove_project,
            commands::list_worktrees,
            commands::create_worktree,
            commands::remove_worktree,
            commands::worktree_diff,
            commands::prune_worktrees,
            commands::list_permission_presets,
            commands::evaluate_permission,
            commands::list_extensions,
            commands::add_mcp,
            commands::remove_mcp,
            commands::toggle_mcp,
            commands::list_mcp_servers,
            commands::get_mcp_server,
            commands::add_mcp_server,
            commands::update_mcp_server,
            commands::remove_mcp_server,
            commands::doctor_mcp_server,
            commands::list_mcp_tools,
            commands::list_mcp_catalog,
            commands::set_mcp_credential,
            commands::list_mcp_credentials,
            commands::remove_mcp_credential,
            commands::suggest_mcp_for_project,
            commands::preview_session_mcp,
            commands::add_skill,
            commands::remove_skill,
            commands::extensions_doctor,
            commands::memory_list,
            commands::memory_add,
            commands::memory_remove,
            commands::memory_flush,
            commands::memory_digest,
            commands::remember,
            commands::project_scope,
            commands::scheduler_list,
            commands::scheduler_add,
            commands::scheduler_cancel,
            commands::scheduler_pause,
            commands::scheduler_resume,
            commands::diff_current,
            commands::diff_capture_before,
            commands::diff_capture_after,
            commands::export_session_markdown,
            commands::list_persisted_sessions,
            commands::persistence_checkpoint,
            commands::shutdown_all,
            commands::detect_dev_server,
            commands::start_dev_server,
            commands::stop_dev_server,
            commands::dev_server_status,
            commands::open_dev_server,
            commands::reveal_project,
            commands::haven_status,
            commands::haven_get_config,
            commands::haven_set_config,
            commands::haven_list_jobs,
            commands::haven_start_shell,
            commands::haven_job_log,
            commands::haven_remove_job,
            commands::haven_list_files,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

pub use state::AppState;
