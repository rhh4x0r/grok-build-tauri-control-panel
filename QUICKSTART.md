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

## First conversation

1. Follow the center welcome screen to **Sign in** to your chosen provider. If its CLI is missing, install it and use **Refresh connections**. The sidebar footer shows connection status.
2. Use **Choose project** or **Add project** to select a folder. If Git is missing from that folder, **Initialize Git repo** creates a local repository without committing or uploading files. A new repository needs an initial commit before you can create an isolated workspace.
3. Choose **Start conversation** or the sidebar's **New chat**. The composer opens with a visible **Isolated workspace** label for change-making conversations. **Ask a question** uses a read-only conversation instead.
4. Keep **Plan** to investigate and propose changes, or choose **Ask first** to request tool approvals. Type your request and press Enter or the send button; Shift+Enter adds a newline.
5. Responses, tool activity, and approval cards appear in the conversation. **Changes** opens file review. **Dev sidebar** opens the preview and its Start/Stop server controls.

Click **Bomb Code** above the folder selector to return Home. Clicking a project opens its Git overview; clicking a conversation reopens it. **Settings** (⌘,) opens in the main window, and **Back** returns to your work.

### Approval modes

- **Plan** is the initial default. **Ask first** requests approval for tools, subject to your permission rules. **Auto** uses the agent's automatic approval policy.
- **Shift+Tab** cycles Plan → Ask first → Auto without moving focus in the conversation screen. Normal keyboard navigation remains available in Settings, menus, dialogs, and other text fields.
- **Full access** is under Advanced in the mode menu and requires explicit confirmation. It allows automatic tool execution, including destructive commands, but deny rules still apply. It is excluded from keyboard cycling and is not inherited by new conversations.

### Optional MCP

- Open **Settings → MCP** to add servers from the catalog and manage credentials; use **Back** to return.
- Servers that need credentials are skipped until configured. For example, GitHub uses `GITHUB_TOKEN`; Linear uses `LINEAR_API_KEY`; X uses `X_API_BEARER`. Stdio servers may require Node/npx.
- Before sending a new conversation's first message, use the composer's **mcp** picker to choose additional servers. Auto-attach servers are included automatically.

Scheduler and Extensions currently have backend services but no dedicated desktop screens.

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

- Plan is the initial default, and Full access requires explicit opt-in. Approval cards offer Allow once / Always / Deny according to the selected mode and permission rules.
- Deny rules take precedence even under Full access. Read-only question conversations cannot escalate into editing; choose Make changes to start an isolated workspace.
- High-risk MCP servers require explicit attachment approval.
