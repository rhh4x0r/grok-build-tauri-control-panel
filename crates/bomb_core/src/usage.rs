//! Account-level usage limits per backend (the "5 hour / weekly" bars).
//!
//! Sources (same ones Zeron reads):
//! - Claude: `GET https://api.anthropic.com/api/oauth/usage` with the OAuth
//!   token Claude Code keeps in the macOS Keychain.
//! - Codex: `GET https://chatgpt.com/backend-api/wham/usage` with the token in
//!   `~/.codex/auth.json`.
//! - Grok: `GET <cli-chat-proxy>/billing?format=credits`, the call behind the
//!   TUI's `/usage` command, with the OIDC key from `~/.grok/auth.json`.
//!
//! Requests go through `curl` so we don't drag an HTTP stack into the build.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

#[path = "usage_cache.rs"]
mod cache;

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
    let (claude, codex, grok) = tokio::join!(claude(), codex(), grok());
    vec![grok, claude, codex]
}

async fn curl_json(url: &str, headers: &[String]) -> Result<serde_json::Value, String> {
    let mut cmd = Command::new("curl");
    cmd.arg("-sS").arg("--max-time").arg("8").arg("-H").arg("accept: application/json");
    for h in headers {
        cmd.arg("-H").arg(h);
    }
    cmd.arg("--write-out").arg("\n%{http_code}").arg(url);
    let out = tokio::time::timeout(Duration::from_secs(10), cmd.output())
        .await
        .map_err(|_| "timed out".to_string())?
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    parse_usage_response(&String::from_utf8_lossy(&out.stdout))
}

fn parse_usage_response(response: &str) -> Result<serde_json::Value, String> {
    let (body, status) = response.rsplit_once('\n').ok_or("Invalid usage response")?;
    match status.trim() {
        "200" => serde_json::from_str(body).map_err(|_| "Invalid usage response".into()),
        "429" => Err("Usage rate limited; retrying later".into()),
        "401" | "403" => Err("Usage access unavailable; check provider sign-in".into()),
        code => Err(format!("Usage request failed (HTTP {code})")),
    }
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
        u.error = Some("Usage unavailable; Claude subscription credentials were not found".into());
        return u;
    };
    cache::claude(&token, || fetch_claude(&token)).await
}

async fn fetch_claude(token: &str) -> AccountUsage {
    let mut u = AccountUsage { backend: "claude".into(), ..Default::default() };
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

// ── Grok ────────────────────────────────────────────────────────────────

/// The newest non-expired entry in `~/.grok/auth.json` (keyed by issuer).
async fn grok_key() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let raw = tokio::fs::read(format!("{home}/.grok/auth.json")).await.ok()?;
    let v: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    let mut best: Option<(DateTime<Utc>, String)> = None;
    for entry in v.as_object()?.values() {
        let Some(key) = entry.get("key").and_then(|k| k.as_str()) else { continue };
        let exp = ts(entry.get("expires_at")).unwrap_or(DateTime::<Utc>::MIN_UTC);
        if best.as_ref().is_none_or(|(e, _)| exp > *e) {
            best = Some((exp, key.to_string()));
        }
    }
    best.map(|(_, k)| k)
}

/// Depth-first search for a numeric field, since the billing payload nests
/// its period summary differently per plan.
fn find_num(v: &serde_json::Value, key: &str) -> Option<f64> {
    match v {
        serde_json::Value::Object(m) => {
            if let Some(x) = m.get(key) {
                // Grok wraps money-like numbers as `{ "val": 12.5 }`.
                if let Some(n) = x.as_f64().or_else(|| x.get("val").and_then(|v| v.as_f64())) {
                    return Some(n);
                }
            }
            m.values().find_map(|x| find_num(x, key))
        }
        serde_json::Value::Array(a) => a.iter().find_map(|x| find_num(x, key)),
        _ => None,
    }
}

fn find_val<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    match v {
        serde_json::Value::Object(m) => m.get(key).or_else(|| m.values().find_map(|x| find_val(x, key))),
        serde_json::Value::Array(a) => a.iter().find_map(|x| find_val(x, key)),
        _ => None,
    }
}

async fn grok() -> AccountUsage {
    let mut u = AccountUsage { backend: "grok".into(), ..Default::default() };
    let Some(key) = grok_key().await else {
        return u;
    };
    let base = std::env::var("GROK_CLI_CHAT_PROXY_BASE_URL")
        .ok()
        .filter(|b| !b.trim().is_empty())
        .unwrap_or_else(|| "https://cli-chat-proxy.grok.com/v1".into());
    let url = format!("{}/billing?format=credits", base.trim_end_matches('/'));
    match curl_json(&url, &[format!("authorization: Bearer {key}")]).await {
        Ok(v) => {
            if let Some(keys) = v.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()) {
                tracing::debug!(?keys, "grok billing payload");
            }
            u.plan = find_val(&v, "tier")
                .or_else(|| find_val(&v, "subscriptionTier"))
                .or_else(|| find_val(&v, "plan"))
                .and_then(|t| t.as_str())
                .map(str::to_string);
            let period_end = find_val(&v, "billingPeriodEnd").and_then(|x| ts(Some(x)));
            let period_start = find_val(&v, "billingPeriodStart").and_then(|x| ts(Some(x)));
            let cycle = find_val(&v, "billingCycle")
                .or_else(|| v.pointer("/config/currentPeriod/type"))
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let label = if cycle.contains("week") {
                "Weekly".to_string()
            } else if cycle.contains("month") {
                "Monthly".to_string()
            } else if let (Some(a), Some(b)) = (period_start, period_end) {
                window_label((b - a).num_seconds())
            } else {
                "Period".to_string()
            };
            if let Some(cfg) = v.get("config").and_then(|c| c.as_object()) {
                let keys: Vec<String> = cfg
                    .iter()
                    .map(|(k, x)| match x.as_object() {
                        Some(o) => format!("{k}{{{}}}", o.keys().cloned().collect::<Vec<_>>().join(",")),
                        None => k.clone(),
                    })
                    .collect();
                tracing::debug!(?keys, "grok billing config shape");
            }
            if let Some(pct) = find_num(&v, "creditUsagePercent") {
                u.windows.push(UsageWindow { label: label.clone(), used_pct: pct as f32, resets_at: period_end });
            } else if let (Some(used), Some(limit)) = (
                find_num(&v, "includedUsed").or_else(|| find_num(&v, "totalUsed")),
                find_num(&v, "monthlyLimit"),
            ) {
                if limit > 0.0 {
                    u.windows.push(UsageWindow {
                        label: label.clone(),
                        used_pct: (used / limit * 100.0) as f32,
                        resets_at: period_end,
                    });
                }
            }
            if let (Some(used), Some(cap)) = (find_num(&v, "onDemandUsed"), find_num(&v, "onDemandCap")) {
                if cap > 0.0 {
                    u.windows.push(UsageWindow { label: "Extra".into(), used_pct: (used / cap * 100.0) as f32, resets_at: period_end });
                }
            }
            if u.windows.is_empty() {
                u.error = find_val(&v, "error")
                    .or_else(|| find_val(&v, "message"))
                    .map(|e| e.to_string())
                    .or(Some("no usage windows".into()));
                tracing::warn!(body = %v, "grok billing: nothing usable");
            }
            tracing::debug!(plan = ?u.plan, windows = ?u.windows, "grok billing parsed");
        }
        Err(e) => {
            tracing::warn!(%e, "grok billing request failed");
            u.error = Some(e);
        }
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

#[cfg(test)]
mod response_tests {
    use super::parse_usage_response;
    #[test]
    fn usage_http_failures_are_explicit_and_do_not_expose_response_bodies() {
        assert!(parse_usage_response("{\"five_hour\":{}}\n200").is_ok());
        assert_eq!(parse_usage_response("private response\n429").unwrap_err(), "Usage rate limited; retrying later");
        assert!(parse_usage_response("private response\n401").unwrap_err().contains("sign-in"));
        assert_eq!(parse_usage_response("private response\n200").unwrap_err(), "Invalid usage response");
    }
}
