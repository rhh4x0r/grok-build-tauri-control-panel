//! `bombd`: run Bomb Code on a server.
//!
//!   bombd up       --data <dir> --listen <addr> --public <host:port>   one person: core + gateway in one process
//!   bombd core     --socket <path>                                     one person's core (multi-user installs)
//!   bombd gateway  --data <dir> --listen <addr> --public <host:port>   the encrypted port (multi-user installs)
//!   bombd add-user --data <dir> --name <name> --socket <path> [--admin]
//!   bombd invite   --data <dir> --public <host:port> --user <name>     print a one-time pairing link
//!   bombd admin    <verb> <account>                                    root-only helper the shared gateway calls through sudo

use std::path::PathBuf;
use std::sync::Arc;

use bomb_server::gateway::Gateway;

fn usage() -> ! {
    eprintln!("usage:\n  bombd up --data <dir> --listen <addr> --public <host:port>\n  bombd core --socket <path>\n  bombd gateway --data <dir> --listen <addr> --public <host:port>\n  bombd add-user --data <dir> --name <name> --socket <path> [--admin]\n  bombd invite --data <dir> --public <host:port> --user <name>");
    std::process::exit(2);
}

struct Args(Vec<String>);
impl Args {
    fn value(&self, flag: &str) -> Option<String> {
        self.0.iter().position(|a| a == flag).and_then(|i| self.0.get(i + 1)).cloned()
    }
    fn required(&self, flag: &str) -> String {
        self.value(flag).unwrap_or_else(|| usage())
    }
    fn has(&self, flag: &str) -> bool {
        self.0.iter().any(|a| a == flag)
    }
}

async fn until_stopped() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal handler");
    tokio::select! { _ = term.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into())).init();
    let args = Args(std::env::args().skip(1).collect());
    match args.0.first().map(String::as_str) {
        Some("core") => {
            let socket = PathBuf::from(args.required("--socket"));
            let state = Arc::new(bomb_core::AppState::initialize().await?);
            bomb_server::core::serve(state.clone(), &socket, until_stopped()).await?;
            // Stop agents cleanly and flush history before exiting.
            let _ = bomb_core::services::shutdown_all(&state).await;
        }
        Some("admin") => {
            // Two fixed arguments, both validated inside; nothing else is accepted.
            let (Some(verb), Some(account), None) = (args.0.get(1), args.0.get(2), args.0.get(3)) else { usage() };
            bomb_server::admin::run(verb, account).map_err(anyhow::Error::msg)?;
        }
        Some("gateway") => {
            // `--shared` lets the admin invite people; each gets a Linux account through the sudo helper.
            let privileged: Box<dyn bomb_server::admin::Privileged> = if args.has("--shared") { Box::new(bomb_server::admin::Sudo) } else { Box::new(bomb_server::admin::Unavailable) };
            let gateway = Gateway::open_with(&PathBuf::from(args.required("--data")), &args.required("--public"), privileged).map_err(anyhow::Error::msg)?;
            let listener = tokio::net::TcpListener::bind(args.required("--listen")).await?;
            gateway.serve(listener, until_stopped()).await.map_err(anyhow::Error::msg)?;
        }
        Some("up") => {
            let data = PathBuf::from(args.required("--data"));
            let gateway = Gateway::open(&data, &args.required("--public")).map_err(anyhow::Error::msg)?;
            let socket = data.join("core.sock");
            let owner = std::env::var("USER").ok().filter(|u| bomb_server::gateway::valid_user_name(u)).unwrap_or_else(|| "owner".into());
            gateway.add_user(&owner, &socket, true).map_err(anyhow::Error::msg)?;
            if gateway.store().read()?.devices.is_empty() {
                println!("\nPair your Mac with this link (works once, for 10 minutes):\n\n  {}\n", gateway.create_invite(&owner).map_err(anyhow::Error::msg)?);
            }
            let state = Arc::new(bomb_core::AppState::initialize().await?);
            let listener = tokio::net::TcpListener::bind(args.required("--listen")).await?;
            let (stop_core, core_stopped) = tokio::sync::oneshot::channel::<()>();
            let core = tokio::spawn({
                let state = state.clone();
                async move { bomb_server::core::serve(state, &socket, async { let _ = core_stopped.await; }).await }
            });
            gateway.serve(listener, until_stopped()).await.map_err(anyhow::Error::msg)?;
            let _ = stop_core.send(());
            let _ = core.await;
            let _ = bomb_core::services::shutdown_all(&state).await;
        }
        Some("add-user") => {
            let gateway = Gateway::open(&PathBuf::from(args.required("--data")), "unused:0").map_err(anyhow::Error::msg)?;
            gateway.add_user(&args.required("--name"), &PathBuf::from(args.required("--socket")), args.has("--admin")).map_err(anyhow::Error::msg)?;
        }
        Some("invite") => {
            let gateway = Gateway::open(&PathBuf::from(args.required("--data")), &args.required("--public")).map_err(anyhow::Error::msg)?;
            println!("{}", gateway.create_invite(&args.required("--user")).map_err(anyhow::Error::msg)?);
        }
        _ => usage(),
    }
    Ok(())
}
