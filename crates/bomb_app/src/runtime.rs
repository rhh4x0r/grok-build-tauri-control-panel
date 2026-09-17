//! tokio ↔ GPUI bridge.
//!
//! Every backend crate is tokio; GPUI runs its own executor on the main
//! thread. Two rules keep them apart safely:
//!
//! 1. Backend work runs on the tokio [`Handle`] stored in the [`Tokio`]
//!    global. Results cross back over an `async_channel` that a GPUI task
//!    awaits, then land on the UI thread through `cx.update`.
//! 2. The UI never blocks on tokio. `spawn_service` is the only way a view
//!    calls into [`bomb_core`].

use std::future::Future;
use std::sync::Arc;

use bomb_core::{AppState, ControlEvent};
use gpui_kit::*;
use tokio::runtime::Handle;
use tracing::{debug, warn};

use crate::models::app::AppModel;

pub struct Tokio(pub Handle);
impl Global for Tokio {}

pub struct Services(pub Arc<AppState>);
impl Global for Services {}

/// The shared backend, for views that need to hand an `Arc` to a future.
pub fn services(cx: &App) -> Arc<AppState> {
    cx.global::<Services>().0.clone()
}

/// Run `fut` on tokio; call `on_done` with its output on the GPUI thread.
pub fn spawn_service<T, Fut, F>(cx: &mut App, fut: Fut, on_done: F)
where
    T: Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
    F: FnOnce(T, &mut App) + 'static,
{
    let handle = cx.global::<Tokio>().0.clone();
    let (tx, rx) = async_channel::bounded::<T>(1);
    handle.spawn(async move {
        let _ = tx.send(fut.await).await;
    });
    cx.spawn(async move |cx| {
        if let Ok(v) = rx.recv().await {
            cx.update(|cx| on_done(v, cx));
        }
    })
    .detach();
}

/// Forward the backend event bus into [`AppModel`], batching bursts so a
/// long stream costs one re-render per frame instead of one per chunk.
pub fn start_bridge(cx: &mut App, model: WeakEntity<AppModel>) {
    let bus = cx.global::<Services>().0.event_bus.clone();
    let (tx, rx) = async_channel::unbounded::<ControlEvent>();
    cx.global::<Tokio>().0.spawn(async move {
        let mut sub = bus.subscribe();
        loop {
            match sub.recv().await {
                Ok(ev) => {
                    if tx.send(ev).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!(n, "ui event bridge lagged");
                    let _ = tx
                        .send(ControlEvent::Error {
                            session_id: None,
                            message: format!("{n} events dropped (UI fell behind)"),
                            at: chrono::Utc::now(),
                        })
                        .await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
        debug!("ui event bridge closed");
    });
    cx.spawn(async move |cx| {
        while let Ok(first) = rx.recv().await {
            let mut batch = vec![first];
            while let Ok(ev) = rx.try_recv() {
                batch.push(ev);
                if batch.len() >= 512 {
                    break;
                }
            }
            if model
                .update(cx, |m, cx| m.apply_events(batch, cx))
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
}
