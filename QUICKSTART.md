# Quickstart — Bomb Code + Grok Build

## Prerequisites

1. **Grok Build CLI** installed (`grok version` works).
2. Auth configured for Grok (login via `grok` once if needed).
3. Rust toolchain (only needed to rebuild).

Your CLI lives at e.g. `~/.grok/bin/grok` — the panel discovers this automatically, including when launched from Finder.

## Install & launch (recommended)

```bash
cd ~/grok-build-tauri-control-panel
./scripts/install.sh
```

This builds a release `.app` and copies it to **Applications**, then opens it.

Later launches:

```bash
./scripts/run.sh
# or
open "/Applications/Bomb Code.app"
```

## First coding session

1. Open the **Sessions** tab.
2. Set **Project directory** to an absolute path of a git repo you want to work in.
3. Leave **Plan mode** on (safer).
4. Click **Start ACP Session** (uses `grok agent stdio`).
5. Select the session in the list, type a prompt, **Send Prompt**.
6. Watch **Live Events** for tool calls / plan updates.

### Optional MCP

- **MCP** tab → pick `filesystem` or `github` → **Add**.
- For GitHub: save `GITHUB_TOKEN` under credentials first — without it the server is skipped at session start (the thread will say why). Linear needs `LINEAR_API_KEY`; X needs `X_API_BEARER`; stdio servers need Node/npx on PATH.
- On Sessions, set **MCP attach** to those names (e.g. `github`).

## Config locations (safe)

| File | Purpose |
|------|---------|
| `~/.grok/control-panel/config.toml` | Panel settings only |
| `~/.grok/config.toml` | Grok CLI config (**never overwritten** by panel) |
| `~/.grok/mcp_credentials.json` | MCP secrets (mode 0600) |
| `~/.grok/control-panel/sessions/` | Panel SQLite recovery DB |

## Dev loop (rebuild UI/backend)

```bash
./scripts/run.sh --dev
# or
cargo run -p bomb_app
```

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| Status: Grok not found | Ensure `~/.grok/bin/grok` exists; re-run install |
| ACP start fails | Run `grok agent stdio` once in a terminal; complete auth |
| Window opens but nothing loads | Run `RUST_LOG=debug cargo run -p bomb_app` and check the terminal for errors |
| MCP doctor warns | Install Node/npx for stdio servers; set credentials |

## Safety

- Default: **plan mode on**, **always-approve off**. With always-approve off, each tool permission shows an approval card in the thread (Allow once / Always / Deny).
- High-risk MCP (browser, grok-build, custom) requires explicit attach approval.
- Do not enable Always approve unless you trust the workspace.
