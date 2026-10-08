//! Pure reducers over [`grok_events::ControlEvent`] that turn the event stream
//! into a renderable thread model. No GPUI, no SQLite, no processes, so the
//! same code folds events on the Mac and in the iOS companion.
//!
//! - [`transcript`] folds events and saved rows into a [`transcript::Thread`].
//! - [`presence`] tracks what the current turn is doing.

pub mod presence;
pub mod transcript;
