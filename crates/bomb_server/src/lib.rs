//! `bombd`: the Bomb Code core without a window.
//!
//! [`core`] serves one person's core over a Unix socket. The gateway (added in
//! a later phase) authenticates devices and proxies them to that socket.

pub mod core;
