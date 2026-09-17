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
        let _ = model.update(cx, |m, cx| m.new_mock_session(cx));
        cx.background_executor().timer(Duration::from_millis(1500)).await;
        let _ = model.update(cx, |m, cx| {
            tracing::info!(selected = ?m.selected, threads = m.threads.len(), "smoke: sending prompt");
            m.send_prompt("hello from the smoke test".into(), Vec::new(), cx);
        });
        cx.background_executor().timer(Duration::from_millis(6000)).await;
        let _ = model.update(cx, |m, cx| {
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
        let _ = cx.update(|cx| cx.quit());
    })
    .detach();
}
