//! Account-level usage limits per backend (the "5 hour / weekly" bars).
//!
//! Sources (same ones Zeron reads):
//! - Claude: `GET https://api.anthropic.com/api/oauth/usage` with the OAuth
//!   token Claude Code keeps in the macOS Keychain.
//! - Codex: `GET https://chatgpt.com/backend-api/wham/usage` with the token in
//!   `~/.codex/auth.json`.
//! - Grok: the CLI exposes no account-wide usage, only per-session tallies.
//!
//! Requests go through `curl` so we don't drag an HTTP stack into the build.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageWindow {
    /// "5h", "Weekly", …
    pub label: String,
    /// 0..=100
    pub used_pct: f32,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AccountUsage {
    pub backend: String,
    pub plan: Option<String>,
    pub windows: Vec<UsageWindow>,
    /// Why there are no windows, when we tried and couldn't get them.
    pub error: Option<String>,
}

/// Fetch usage for every backend that has a source. Never fails as a whole:
/// a backend without a token simply has no windows.
pub async fn all() -> Vec<AccountUsage> {
    let (claude, codex) = tokio::join!(claude(), codex());
    vec![claude, codex]
}

async fn curl_json(url: &str, headers: &[String]) -> Result<serde_json::Value, String> {
    let mut cmd = Command::new("curl");
    cmd.arg("-sS").arg("--max-time").arg("8").arg("-H").arg("accept: application/json");
    for h in headers {
        cmd.arg("-H").arg(h);
    }
    cmd.arg(url);
    let out = tokio::time::timeout(Duration::from_secs(10), cmd.output())
        .await
        .map_err(|_| "timed out".to_string())?
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("bad json: {e}"))
}

fn ts(v: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
    let v = v?;
    if let Some(s) = v.as_str() {
        return DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc));
    }
    let secs = v.as_i64()?;
    DateTime::from_timestamp(secs, 0)
}

// ── Claude ──────────────────────────────────────────────────────────────

async fn claude_token() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("security")
            .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
            .output()
            .await
            .ok()?;
        if out.status.success() {
            let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
            if let Some(t) = v.pointer("/claudeAiOauth/accessToken").and_then(|t| t.as_str()) {
                return Some(t.to_string());
            }
        }
    }
    let home = std::env::var("HOME").ok()?;
    let raw = tokio::fs::read(format!("{home}/.claude/.credentials.json")).await.ok()?;
    let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    v.pointer("/claudeAiOauth/accessToken").and_then(|t| t.as_str()).map(str::to_string)
}

async fn claude() -> AccountUsage {
    let mut u = AccountUsage { backend: "claude".into(), ..Default::default() };
    let Some(token) = claude_token().await else {
        return u;
    };
    match curl_json(
        "https://api.anthropic.com/api/oauth/usage",
        &[format!("authorization: Bearer {token}"), "anthropic-beta: oauth-2025-04-20".into()],
    )
    .await
    {
        Ok(v) => {
            for (key, label) in [("five_hour", "5h"), ("seven_day", "Weekly")] {
                if let Some(w) = v.get(key).filter(|w| !w.is_null()) {
                    u.windows.push(UsageWindow {
                        label: label.into(),
                        used_pct: w.get("utilization").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
                        resets_at: ts(w.get("resets_at")),
                    });
                }
            }
            if u.windows.is_empty() {
                u.error = v.get("error").map(|e| e.to_string()).or(Some("no usage windows".into()));
            }
        }
        Err(e) => u.error = Some(e),
    }
    u
}

// ── Codex ───────────────────────────────────────────────────────────────

async fn codex_auth() -> Option<(String, Option<String>)> {
    let home = std::env::var("HOME").ok()?;
    let raw = tokio::fs::read(format!("{home}/.codex/auth.json")).await.ok()?;
    let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    let token = v.pointer("/tokens/access_token")?.as_str()?.to_string();
    let account = v.pointer("/tokens/account_id").and_then(|a| a.as_str()).map(str::to_string);
    Some((token, account))
}

async fn codex() -> AccountUsage {
    let mut u = AccountUsage { backend: "codex".into(), ..Default::default() };
    let Some((token, account)) = codex_auth().await else {
        return u;
    };
    let mut headers = vec![format!("authorization: Bearer {token}")];
    if let Some(a) = account {
        headers.push(format!("chatgpt-account-id: {a}"));
    }
    match curl_json("https://chatgpt.com/backend-api/wham/usage", &headers).await {
        Ok(v) => {
            u.plan = v.get("plan_type").and_then(|p| p.as_str()).map(str::to_string);
            for key in ["primary_window", "secondary_window"] {
                let Some(w) = v.pointer(&format!("/rate_limit/{key}")).filter(|w| !w.is_null()) else {
                    continue;
                };
                let secs = w.get("limit_window_seconds").and_then(|s| s.as_i64()).unwrap_or(0);
                u.windows.push(UsageWindow {
                    label: window_label(secs),
                    used_pct: w.get("used_percent").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
                    resets_at: ts(w.get("reset_at")),
                });
            }
            if u.windows.is_empty() {
                u.error = Some("no usage windows".into());
            }
        }
        Err(e) => u.error = Some(e),
    }
    u
}

fn window_label(secs: i64) -> String {
    match secs {
        s if s <= 0 => "Limit".into(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s if s < 7 * 86_400 => format!("{}d", s / 86_400),
        s if s < 8 * 86_400 => "Weekly".into(),
        s => format!("{}d", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_follow_window_length() {
        assert_eq!(window_label(18_000), "5h");
        assert_eq!(window_label(604_800), "Weekly");
        assert_eq!(window_label(1_800), "30m");
    }

    #[test]
    fn timestamps_accept_epoch_and_rfc3339() {
        assert!(ts(Some(&serde_json::json!(1_700_000_000))).is_some());
        assert!(ts(Some(&serde_json::json!("2026-09-17T00:00:00Z"))).is_some());
        assert!(ts(Some(&serde_json::json!(null))).is_none());
    }
}
