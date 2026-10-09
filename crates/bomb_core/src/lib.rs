//! Bomb Code application core: everything the desktop shell needs that is
//! not UI. Framework-free so the GPUI app (or a test harness) can drive it.
//!
//! - [`state::AppState`] wires the backend crates together.
//! - [`services`] exposes every user action as a plain async fn over `&AppState`.
//! - [`transcript`] and [`presence`] are pure reducers over [`grok_events::ControlEvent`]
//!   that turn the event stream into a renderable thread model. They live in
//!   `bomb_transcript` so the iOS companion can share them.

pub mod devserver;
pub mod explainer;
pub mod journal;
pub mod rpc;
pub mod services;
pub mod state;
pub mod usage;

pub use bomb_transcript::{presence, speech as transcript_speech, summary, transcript};
pub use grok_events::{ControlEvent, EventBus};
pub use state::AppState;

pub mod terminal;

pub mod foundry;
pub mod helpers;
pub mod helpers_mcp;
