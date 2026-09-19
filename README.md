# Bomb Code

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](./LICENSE)
[![Rust](https://img.shields.io/badge/rust-edition%202021-orange.svg)](./Cargo.toml)
[![GPUI](https://img.shields.io/badge/ui-GPUI-purple.svg)](https://www.gpui.rs)

**Bomb Code** is an open-source **pure-Rust desktop app (GPUI + gpui-kit)**, a control panel for [Grok Build](https://x.ai) — multi-session agent orchestration with **ACP-first** integration, worktrees, permissions, MCP/skills, memory, scheduler, and crash recovery.

> **Requires** the [Grok Build CLI](https://x.ai) (`grok`) installed and authenticated. This panel does not ship the Grok binary.

## Features

- **ACP client** (`grok agent stdio`) — long-lived interactive sessions
- **Non-blocking prompts** — long coding turns stream via notifications (no ~120s UI timeout)
- **Headless CLI** fallback for batch / scheduled jobs
- **Multi-session registry** with concurrent `DashMap` access
- **Git worktree** isolation for parallel agents
- **Interactive tool approvals** (allow once / always / deny per request), deny rules enforced ahead of yolo, permission presets (safe / workspace / yolo) + sandbox profiles
- **Native GPUI app** — one window with projects and conversations in the sidebar, a conversation composer, and an in-window Settings screen (⌘,) with a Back button. Follows system light/dark appearance.
- **Guided welcome** — connect a provider, choose a project, and start a conversation. The sidebar has labeled Add project and New chat actions.
- **Isolated workspaces** — changes run on a separate `bomb/<prompt-slug>-<id>` branch with checkpoints. Multiple conversations can share a workspace. Ask a question keeps files unchanged. The Changes panel offers explicit review, push/PR, and local merge actions.
- **Project overview** — local branch map, recent commits, committed-file comparisons, workspace conversations, and GitHub PR status when GitHub CLI access is available. Plain folders offer Initialize Git repo; initialization does not commit or upload files.
- **MCP management** — catalog (filesystem, GitHub, Linear, X, Playwright, custom), doctor, credentials store, pre-spawn health checks, session attachment. Servers needing credentials (e.g. `GITHUB_TOKEN`, `LINEAR_API_KEY`, `X_API_BEARER`) are skipped with a visible reason until the secret is set.
- **Extensions backend** — skills/plugins services exist; a dedicated desktop management screen is not yet available.
- **Memory** — structured store + MEMORY.md flush/dream
- **Scheduler backend** — interval, cron, and one-shot routines with persistence and explicit working directories; a desktop scheduling screen is not yet available.
- **Persistence** — SQLite session/transcript recovery
- **Diff engine** — before/after capture and summaries
- **Dev sidebar** — preview and explicit Start server / Stop server controls, opened from the conversation toolbar.
- **macOS app** install under `/Applications/Bomb Code.app`

## Quick start

### Prerequisites

1. [Rust](https://rustup.rs/) (stable) + Xcode CLT on macOS
2. Grok Build CLI on `PATH` or at `~/.grok/bin/grok`
3. Grok auth (`grok` login or Sign in on the app’s welcome screen)

### Install from source (macOS)

```bash
git clone https://github.com/jedisherpa/grok-build-tauri-control-panel.git
cd grok-build-tauri-control-panel
./scripts/install.sh   # release build → /Applications/Bomb Code.app + open
```

Later launches:

```bash
./scripts/run.sh
# or
open "/Applications/Bomb Code.app"
```

See **[QUICKSTART.md](./QUICKSTART.md)** for first ACP session, MCP setup, and config paths.

### Develop

```bash
./scripts/run.sh --dev
# or
cargo run -p bomb_app          # dev build
./scripts/bundle.sh            # release .app under target/release/bundle/
```

The app discovers `~/.grok/bin/grok` even when launched from Finder (PATH is bootstrapped).

## Smart Model Routing

Open **Settings → Smart Model Routing**, choose **JEV direct** or **Vercel AI Gateway**, save your API key, and enable suggestions. The feature is off by default. Keys are stored in the app’s SQLite settings table; existing Keychain entries are migrated once and removed after a verified database write. `TYPESAFE_API_KEY` or `AI_GATEWAY_API_KEY` can also supply credentials. The **Test connection** button makes a small billed evaluation without project context.

Edit the starter model preferences to describe which connected models should handle which tasks. Before a text prompt is sent, JEV receives the prompt (up to 8,000 characters), a bounded excerpt of recent user/assistant messages, and the available model choices. An alternative with at least 50% selection probability and a 10-point lead opens an inline card with model and reasoning-effort selectors, **Switch & send**, and **Keep [current model]**. Only accepting switches models, using the existing conversation handoff. Using the current model suppresses that recommendation in the thread; the Smart Model Routing icon beside the model picker opens the thread toggle, which persists across restarts. Scores appear under **Why this suggestion**; the switch record includes the provider-reported reasoning effort. Completed review-loop results are available from the thread’s ellipsis menu → Review loop results; finished runs do not leave a persistent row in the conversation.

Only discovered models from signed-in, runnable providers are candidates. Uncertain responses, missing credentials, and requests taking more than fifteen seconds fall back to the current model. Attachment turns and slash commands bypass suggestions. This feature does not delegate work, change permissions, or move execution billing to JEV/Vercel. Model preferences and the external-evaluation opt-in are global settings; repository overlays cannot enable them.

## Config locations

| Path | Purpose |
|------|---------|
| `~/.grok/control-panel/config.toml` | Panel settings only |
| `~/.grok/config.toml` | Grok CLI config (**never overwritten** by panel) |
| `~/.grok/mcp_credentials.json` | MCP secrets (mode `0600`) |
| `~/.grok/control-panel/sessions/` | Panel SQLite recovery DB |

## Workspace layout

```
crates/           # backend libraries
crates/bomb_core/ # app core: state, services, transcript reducer
crates/bomb_app/  # GPUI desktop app
docs/plan/        # original multi-agent build plan
scripts/          # install / run helpers
```

## Security notes

- No API keys or credentials are committed to this repository.
- Secrets live under `~/.grok/` with restricted permissions.
- **Plan** is the initial default. **Full access** (`yolo`) requires explicit confirmation in the mode menu, is excluded from Shift+Tab cycling, and is not inherited by new conversations. Deny rules still apply.
- Do not log `XAI_API_KEY` or MCP tokens.

## Contributing

Issues and PRs welcome. Prefer conventional commits (`feat:`, `fix:`, `docs:`, …). Keep changes focused; run `cargo test` and `cargo clippy` before submitting.

## License

[MIT](./LICENSE) © 2026 Bomb Code contributors
