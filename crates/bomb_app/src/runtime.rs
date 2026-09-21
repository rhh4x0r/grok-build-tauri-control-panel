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
use tracing::debug;

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

/// Forward the core's event journal into [`AppModel`], batching bursts so a
/// long stream costs one re-render per frame instead of one per chunk. The
/// journal saves history and hides Foundry's helper sessions before we see them.
pub fn start_bridge(cx: &mut App, model: WeakEntity<AppModel>) {
    let journal = cx.global::<Services>().0.journal.clone();
    let (tx, rx) = async_channel::unbounded::<ControlEvent>();
    cx.global::<Tokio>().0.spawn(async move {
        let mut events = journal.attach_local().events;
        while let Some((_, ev)) = events.recv().await {
            if tx.send(ev).await.is_err() {
                break;
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
