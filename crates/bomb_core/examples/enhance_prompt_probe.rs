//! Opt-in live probe: cargo run -p bomb_core --example enhance_prompt_probe
use std::sync::Arc;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let state = Arc::new(bomb_core::AppState::initialize().await?);
    let options = bomb_foundry::PromptOptions {
        depth: "fast-draft".into(), target: "grok-build".into(),
        work_type: "implementation".into(), autonomy: String::new(), sources: vec![],
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(180),
        bomb_core::foundry::generate_prompt(state, "Build a 2d tetris game".into(), "grok".into(), "grok-4.6".into(), options)).await??;
    assert!(result.contains("## GOAL"));
    println!("PASS: Grok generated a validated enhanced prompt ({} bytes).", result.len());
    Ok(())
}
