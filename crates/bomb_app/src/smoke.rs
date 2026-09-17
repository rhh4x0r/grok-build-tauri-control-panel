//! `BOMB_SMOKE=1`: drive one mock turn end-to-end and print the resulting
//! transcript to the log, then quit. Lets CI (and a headless developer)
//! verify the bridge → reducer → model path without a screen.

use std::time::Duration;

use gpui_kit::*;

use crate::models::app::AppModel;

pub fn maybe_run(model: Entity<AppModel>, cx: &mut App) {
    if std::env::var("BOMB_SMOKE").ok().as_deref() != Some("1") {
        return;
    }
    tracing::info!("smoke: starting mock session");
    cx.spawn(async move |cx| {
        model.update(cx, |m, cx| m.new_mock_session(cx));
        cx.background_executor().timer(Duration::from_millis(1500)).await;
        model.update(cx, |m, cx| {
            tracing::info!(selected = ?m.selected, threads = m.threads.len(), "smoke: sending prompt");
            m.send_prompt("hello from the smoke test".into(), Vec::new(), cx);
        });
        cx.background_executor().timer(Duration::from_millis(6000)).await;
        model.update(cx, |m, cx| {
            match m.selected_thread() {
                Some(t) => {
                    let t = t.read(cx);
                    tracing::info!(
                        phase = ?t.thread.presence.phase,
                        entries = t.thread.entries.len(),
                        label = ?t.thread.label,
                        "smoke: transcript"
                    );
                    for e in &t.thread.entries {
                        let body = match &e.body {
                            bomb_core::transcript::Body::Text(s) => s.chars().take(80).collect::<String>(),
                            other => format!("{other:?}").chars().take(80).collect(),
                        };
                        tracing::info!(role = ?e.role, streaming = e.streaming, %body, "smoke: entry");
                    }
                    tracing::info!(protocol = t.thread.protocol_log.len(), markdown_states = t.markdown.len(), "smoke: side state");
                }
                None => tracing::error!("smoke: no selected thread"),
            }
        });
        // Settings window: open it and report what loaded.
        cx.update(crate::views::settings::open_settings_window);
        cx.background_executor().timer(Duration::from_millis(2500)).await;
        cx.update(|cx| {
            match cx.try_global::<crate::views::settings::SettingsHandle>() {
                Some(h) => {
                    let m = h.0.read(cx);
                    tracing::info!(
                        config = m.config.is_some(),
                        mcp = m.mcp.len(),
                        catalog = m.catalog.len(),
                        credentials = m.credentials.len(),
                        memory = m.memory.len(),
                        worktree_repos = m.worktrees.len(),
                        runtime = m.runtime.is_some(),
                        presets = m.presets.len(),
                        "smoke: settings loaded"
                    );
                }
                None => tracing::error!("smoke: settings model missing"),
            }
        });
        cx.update(|cx| cx.quit());
    })
    .detach();
}
