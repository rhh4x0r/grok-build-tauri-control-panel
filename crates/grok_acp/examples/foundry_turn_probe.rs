//! Opt-in, one-turn read-only smoke: cargo run -p grok_acp --example foundry_turn_probe -- /absolute/grok
use grok_acp::{AcpClient, AcpClientConfig, AcpSpawnOptions, ApprovalMode};
use grok_events::{ControlEvent, EventBus};
use std::{sync::Arc, time::Duration};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let program = std::env::args().nth(1).ok_or("Pass the Grok executable")?;
    let cwd = tempfile::tempdir()?;
    let mut config = AcpClientConfig::new(program, cwd.path());
    config.read_only = true;
    config.prompt_timeout = Duration::from_secs(90);
    let opts = AcpSpawnOptions {
        plan_mode: true,
        approval_mode: ApprovalMode::Plan,
        ..Default::default()
    };
    let bus = Arc::new(EventBus::new());
    let mut events = bus.subscribe();
    let client = AcpClient::connect(config, &opts, Some(bus), uuid::Uuid::new_v4()).await?;
    let token = uuid::Uuid::new_v4().to_string();
    client.send_prompt_correlated("This is a protocol smoke test. Do not use tools or read files. Return exactly: <foundry-result>{\"outcome\":\"passed\",\"summary\":\"Protocol response received\",\"criteria\":[],\"artifacts\":[]}</foundry-result>",&[],Some(token.clone())).await?;
    let result = tokio::time::timeout(Duration::from_secs(95), async {
        let mut text = String::new();
        loop {
            match events.recv().await? {
                ControlEvent::AgentMessage { text: chunk, .. } => text.push_str(&chunk),
                ControlEvent::Raw { payload, .. }
                    if payload["correlation"] == token && payload["turn_complete"] == true =>
                {
                    return Ok::<_, Box<dyn std::error::Error>>(text)
                }
                ControlEvent::ApprovalRequired { request_id, .. } => {
                    client.respond_approval(&request_id, None).await?;
                }
                ControlEvent::Error { message, .. } => return Err(message.into()),
                _ => {}
            }
        }
    })
    .await;
    client.shutdown().await?;
    let text = result??;
    let body = text
        .split_once("<foundry-result>")
        .and_then(|(_, s)| s.split_once("</foundry-result>"))
        .ok_or("Missing structured result")?
        .0;
    let value: serde_json::Value = serde_json::from_str(body)?;
    if value["outcome"] != "passed" {
        return Err("Unexpected outcome".into());
    }
    println!("PASS: correlated ACP completion and structured Foundry result received; no tools authorized.");
    Ok(())
}
