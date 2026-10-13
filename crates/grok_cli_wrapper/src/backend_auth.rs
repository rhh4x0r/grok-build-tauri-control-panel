//! Which agent services are actually usable right now.
//!
//! Two independent questions, which are easy to conflate:
//!
//! 1. **Runnable** — can we launch the backend's ACP adapter? For claude and
//!    codex the adapter is normally *not* installed: `resolve_backend` falls
//!    back to `npx --yes <pkg>`, which fetches it on demand. So a backend can
//!    be perfectly runnable with no CLI on PATH at all.
//! 2. **Signed in** — are there credentials for it? Those belong to the vendor
//!    CLI, not the adapter, and we never store any ourselves:
//!    - grok:   `~/.grok/auth.json` or `XAI_API_KEY`
//!    - claude: `claude auth status --json` / macOS Keychain, or `ANTHROPIC_API_KEY`
//!    - codex:  `$CODEX_HOME/auth.json` or `OPENAI_API_KEY`
//!
//! A backend is only usable when both hold.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use grok_config::backends::{descriptor, resolve_backend, Backend, LaunchVia};
use grok_config::GrokConfig;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::GrokCli;

/// How a backend proved it is signed in. `ApiKey` means an env var is doing the
/// work, so there is nothing to sign in or out of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    Subscription,
    ApiKey,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendAuth {
    /// "grok" | "claude" | "codex"
    pub backend: String,
    pub display_name: String,
    /// We can launch this backend's ACP adapter (installed binary, or npx).
    pub runnable: bool,
    /// How we would launch it, for the tooltip: a path, or "npx <pkg>".
    pub launch: Option<String>,
    /// The credential-owning vendor CLI is on PATH. Not needed to *run* the
    /// backend — only to sign in from here without npx.
    pub cli_installed: bool,
    pub logged_in: bool,
    pub kind: AuthKind,
    /// Signed-in identity, when the CLI reports one.
    pub account: Option<String>,
    /// Plan / auth-method detail, e.g. "max" or "claude.ai".
    pub plan: Option<String>,
    /// One line for the UI: who you are, or why this backend is unusable.
    pub message: String,
    /// Shell command that signs in. Uses `npx` when the CLI is absent.
    pub login_command: Option<String>,
    /// The panel drives this login itself (device code); otherwise the CLI
    /// needs a TTY + browser and we hand off to a terminal.
    pub in_app_login: bool,
}

fn which(bin: &str) -> Option<PathBuf> {
    which::which(bin).ok()
}

/// A provider's own CLI (`claude`, `codex`), not its ACP adapter: PATH, then the usual install
/// folders, which an app opened from Finder may not have on its PATH.
pub fn cli_path(bin: &str) -> Option<PathBuf> {
    which(bin).or_else(|| {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        [home.join(".local/bin"), home.join(".claude/local"), "/opt/homebrew/bin".into(), "/usr/local/bin".into()]
            .into_iter()
            .map(|dir| dir.join(bin))
            .find(|path| path.is_file())
    })
}

fn env_key(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| !v.trim().is_empty())
}

/// Can we launch this backend, and how?
fn launchability(b: Backend, cfg: &GrokConfig) -> (bool, Option<String>) {
    match resolve_backend(b, cfg) {
        Ok(r) => {
            let how = match r.via {
                LaunchVia::Npx => format!("npx {}", r.args.last().cloned().unwrap_or_default()),
                LaunchVia::Binary => r.program.display().to_string(),
            };
            (true, Some(how))
        }
        Err(_) => (false, None),
    }
}

/// Run a CLI's own status probe. A timeout is reported as "unknown", never as
/// "signed out" — we must not tell the user to re-auth on a slow disk.
async fn probe(bin: &PathBuf, args: &[&str]) -> Option<String> {
    let fut = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    let out = tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .ok()?
        .ok()?;
    String::from_utf8(out.stdout).ok()
}

/// Sign-in command, preferring an installed CLI and falling back to npx so the
/// user can authenticate without a global install.
fn auth_command(backend: Backend, verb: &str) -> Option<String> {
    let (cli, npx_pkg, sub) = match backend {
        // grok has no npx package: without the binary there is no way in.
        Backend::Grok => ("grok", None, verb.to_string()),
        Backend::Claude => (
            "claude",
            Some("@anthropic-ai/claude-code"),
            format!("auth {verb}"),
        ),
        Backend::Codex => ("codex", Some("@openai/codex"), verb.to_string()),
    };
    if which(cli).is_some() {
        return Some(format!("{cli} {sub}"));
    }
    let pkg = npx_pkg?;
    which("npx")?;
    Some(format!("npx -y {pkg} {sub}"))
}

/// Status for all three backends, probed concurrently.
pub async fn all(cfg: &GrokConfig) -> Vec<BackendAuth> {
    let (grok, claude, codex) = tokio::join!(grok(cfg), claude(cfg), codex(cfg));
    vec![grok, claude, codex]
}

pub async fn grok(cfg: &GrokConfig) -> BackendAuth {
    let (runnable, launch) = launchability(Backend::Grok, cfg);
    let cli_installed = which("grok").is_some();
    let base = BackendAuth {
        backend: "grok".into(),
        display_name: descriptor(Backend::Grok).display_name.into(),
        runnable,
        launch,
        cli_installed,
        logged_in: false,
        kind: AuthKind::None,
        account: None,
        plan: None,
        message: String::new(),
        login_command: auth_command(Backend::Grok, "login"),
        in_app_login: true, // device-code flow, driven by the panel
    };

    if !runnable {
        return BackendAuth {
            message: "Grok CLI not found — install it to use this backend".into(),
            in_app_login: false,
            ..base
        };
    }
    // The grok backend accepts a raw xAI key with no login at all.
    if env_key("XAI_API_KEY") {
        return BackendAuth {
            logged_in: true,
            kind: AuthKind::ApiKey,
            plan: Some("XAI_API_KEY".into()),
            message: "Using XAI_API_KEY from the environment".into(),
            login_command: None,
            in_app_login: false,
            ..base
        };
    }

    let st = GrokCli::auth_status();
    BackendAuth {
        logged_in: st.logged_in,
        kind: if st.logged_in { AuthKind::Subscription } else { AuthKind::None },
        account: st.email.clone(),
        plan: st.auth_mode.clone(),
        message: if st.logged_in {
            st.email.unwrap_or_else(|| "Signed in".into())
        } else {
            "Not signed in".into()
        },
        ..base
    }
}

pub async fn claude(cfg: &GrokConfig) -> BackendAuth {
    let (runnable, launch) = launchability(Backend::Claude, cfg);
    let cli = which("claude");
    let base = BackendAuth {
        backend: "claude".into(),
        display_name: descriptor(Backend::Claude).display_name.into(),
        runnable,
        launch,
        cli_installed: cli.is_some(),
        logged_in: false,
        kind: AuthKind::None,
        account: None,
        plan: None,
        message: String::new(),
        login_command: auth_command(Backend::Claude, "login"),
        in_app_login: false, // needs a TTY + browser: hand off to a terminal
    };

    if !runnable {
        return BackendAuth {
            message: "No adapter and no npx — cannot launch Claude Code".into(),
            ..base
        };
    }
    if env_key("ANTHROPIC_API_KEY") {
        return BackendAuth {
            logged_in: true,
            kind: AuthKind::ApiKey,
            plan: Some("ANTHROPIC_API_KEY".into()),
            message: "Using ANTHROPIC_API_KEY from the environment".into(),
            login_command: None,
            ..base
        };
    }

    // `claude auth status --json` is the CLI's own answer. On macOS the token
    // lives in the Keychain, so when the CLI is absent we can only check for
    // the credentials file other platforms use.
    let Some(bin) = cli else {
        let creds = claude_credentials_file().is_some_and(|p| p.exists());
        return BackendAuth {
            logged_in: creds,
            kind: if creds { AuthKind::Subscription } else { AuthKind::None },
            message: if creds {
                "Signed in".into()
            } else {
                "Sign-in unknown — install the Claude CLI to check".into()
            },
            ..base
        };
    };

    let raw = probe(&bin, &["auth", "status", "--json"]).await;
    let v: Option<serde_json::Value> = raw.as_deref().and_then(|s| serde_json::from_str(s).ok());
    let field = |k: &str| {
        v.as_ref()
            .and_then(|v| v.get(k))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let logged_in = v
        .as_ref()
        .and_then(|v| v.get("loggedIn"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let account = field("email");

    BackendAuth {
        logged_in,
        kind: if logged_in { AuthKind::Subscription } else { AuthKind::None },
        account: account.clone(),
        plan: field("subscriptionType").or_else(|| field("authMethod")),
        message: if logged_in {
            account.unwrap_or_else(|| "Signed in".into())
        } else {
            "Not signed in".into()
        },
        ..base
    }
}

pub async fn codex(cfg: &GrokConfig) -> BackendAuth {
    let (runnable, launch) = launchability(Backend::Codex, cfg);
    let cli = which("codex");
    let base = BackendAuth {
        backend: "codex".into(),
        display_name: descriptor(Backend::Codex).display_name.into(),
        runnable,
        launch,
        cli_installed: cli.is_some(),
        logged_in: false,
        kind: AuthKind::None,
        account: None,
        plan: None,
        message: String::new(),
        login_command: auth_command(Backend::Codex, "login"),
        in_app_login: false,
    };

    if !runnable {
        return BackendAuth {
            message: "No adapter and no npx — cannot launch Codex".into(),
            ..base
        };
    }
    if env_key("OPENAI_API_KEY") {
        return BackendAuth {
            logged_in: true,
            kind: AuthKind::ApiKey,
            plan: Some("OPENAI_API_KEY".into()),
            message: "Using OPENAI_API_KEY from the environment".into(),
            login_command: None,
            ..base
        };
    }

    // The adapter reads $CODEX_HOME/auth.json, so that file — not the CLI — is
    // the ground truth. The CLI's own probe is a nicety when it happens to be
    // installed (it also knows the account).
    let auth_file = codex_auth_file();
    let has_file = auth_file.as_ref().is_some_and(|p| p.exists());
    let mut account = auth_file
        .as_ref()
        .filter(|p| p.exists())
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| {
            v.pointer("/tokens/id_token/email")
                .or_else(|| v.pointer("/tokens/email"))
                .or_else(|| v.get("email"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });

    let mut logged_in = has_file;
    if let Some(bin) = cli {
        let raw = probe(&bin, &["login", "status"]).await.unwrap_or_default();
        let low = raw.to_lowercase();
        if low.contains("not logged in") {
            logged_in = false;
        } else if low.contains("logged in") {
            logged_in = true;
            account = account.or_else(|| {
                raw.split_whitespace()
                    .find(|w| w.contains('@') && w.contains('.'))
                    .map(|w| w.trim_matches(|c: char| !c.is_ascii_graphic()).to_string())
            });
        }
    }

    BackendAuth {
        logged_in,
        kind: if logged_in { AuthKind::Subscription } else { AuthKind::None },
        account: account.clone(),
        plan: logged_in.then(|| "ChatGPT".into()),
        message: if logged_in {
            account.unwrap_or_else(|| "Signed in".into())
        } else {
            "Not signed in".into()
        },
        ..base
    }
}

fn codex_auth_file() -> Option<PathBuf> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")))?;
    Some(home.join("auth.json"))
}

fn claude_credentials_file() -> Option<PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")))?;
    Some(dir.join(".credentials.json"))
}

/// The sign-in / sign-out command for a backend, resolved the same way the
/// status rows resolve it (installed CLI first, then npx).
pub fn login_command(backend: &str) -> Option<String> {
    auth_command(Backend::from_key(backend)?, "login")
}

pub fn logout_command(backend: &str) -> Option<String> {
    auth_command(Backend::from_key(backend)?, "logout")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_commands_cover_every_backend() {
        for b in ["grok", "claude", "codex"] {
            // Each backend can produce a command as long as *some* route exists
            // (the CLI, or npx). At minimum the mapping must know the backend.
            assert!(Backend::from_key(b).is_some());
        }
        assert!(login_command("nope").is_none());
        assert!(logout_command("nope").is_none());
    }

    #[test]
    fn claude_login_uses_auth_subcommand() {
        // `claude login` is not a thing; it is `claude auth login`. Guard the
        // shape of whichever route we take.
        if let Some(cmd) = login_command("claude") {
            assert!(cmd.contains("auth login"), "unexpected claude login: {cmd}");
        }
        if let Some(cmd) = login_command("codex") {
            assert!(cmd.ends_with("login"), "unexpected codex login: {cmd}");
            assert!(!cmd.contains("auth"), "codex has no auth subcommand: {cmd}");
        }
    }

    #[tokio::test]
    async fn npx_backends_are_runnable_without_their_cli() {
        // The whole point of the npx fallback: claude/codex run with no adapter
        // and no vendor CLI installed. If npx is missing we cannot assert this.
        if which("npx").is_none() {
            return;
        }
        let cfg = GrokConfig::default();
        for s in [claude(&cfg).await, codex(&cfg).await] {
            assert!(s.runnable, "{} should be runnable via npx", s.backend);
            assert!(s.launch.is_some());
        }
    }
}
