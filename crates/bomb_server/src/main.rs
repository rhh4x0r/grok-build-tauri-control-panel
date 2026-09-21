//! `bombd core --socket <path>`: run this user's Bomb Code core headless.

use std::path::PathBuf;
use std::sync::Arc;

fn usage() -> ! {
    eprintln!("usage: bombd core --socket <path>");
    std::process::exit(2);
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("core") => {
            let socket = args.iter().position(|a| a == "--socket").and_then(|i| args.get(i + 1)).map(PathBuf::from).unwrap_or_else(|| usage());
            let state = Arc::new(bomb_core::AppState::initialize().await?);
            let stopping = state.clone();
            bomb_server::core::serve(state, &socket, async {
                let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal handler");
                tokio::select! { _ = term.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
            }).await?;
            // Stop agents cleanly and flush history before exiting.
            let _ = bomb_core::services::shutdown_all(&stopping).await;
            Ok(())
        }
        _ => usage(),
    }
}
