//! Bomb Code application core: everything the desktop shell needs that is
//! not UI. Framework-free so the GPUI app (or a test harness) can drive it.
//!
//! - [`state::AppState`] wires the backend crates together.
//! - [`services`] exposes every user action as a plain async fn over `&AppState`.
//! - [`transcript`] and [`presence`] are pure reducers over [`grok_events::ControlEvent`]
//!   that turn the event stream into a renderable thread model.

pub mod devserver;
pub mod explainer;
pub mod presence;
pub mod services;
pub mod state;
pub mod transcript;
pub mod usage;

pub use state::AppState;
pub use grok_events::{ControlEvent, EventBus};
