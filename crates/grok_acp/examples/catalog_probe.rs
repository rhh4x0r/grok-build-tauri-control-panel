//! Print the catalog advertised by an installed ACP adapter without sending a prompt.
//! cargo run -p grok_acp --example catalog_probe -- /absolute/program [adapter arguments]
use grok_acp::{AcpClient, AcpClientConfig};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let program = args.next().ok_or("Pass an absolute ACP adapter executable")?;
    let cwd = tempfile::tempdir()?;
    let mut config = AcpClientConfig::new(program, cwd.path());
    config.args = args.collect();
    config.env.push(("ANTHROPIC_API_KEY".into(), std::env::var("ANTHROPIC_API_KEY").unwrap_or_default()));
    config.startup_timeout = std::time::Duration::from_secs(20);
    config.request_timeout = std::time::Duration::from_secs(30);
    let catalog = AcpClient::discover_models(config).await?;
    println!("{}", serde_json::to_string_pretty(&catalog)?);
    Ok(())
}
