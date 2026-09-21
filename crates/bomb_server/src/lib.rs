//! `bombd`: the Bomb Code core without a window.
//!
//! [`core`] serves one person's core over a Unix socket. [`gateway`] is the one
//! encrypted port: it authenticates devices and relays them to that socket.

pub mod admin;
pub mod core;
pub mod gateway;
pub mod store;
