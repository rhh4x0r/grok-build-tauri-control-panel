# Server-first projects (run Bomb Code threads on my VPS)

## Context

Agents in Bomb Code are child processes of the desktop app, so closing the laptop or quitting kills any turn in flight (`kill_on_drop`; `shutdown_all` exists but nothing calls it on quit). Max wants to code from his laptop, close the lid or leave long jobs running overnight, and still be able to take work offline.

We looked at Zeron: every machine runs an engine, a session lives on the machine where it started, other devices only steer it, nothing moves. We adopt that, without Zeron's hosted account and cloud relay, and go one step further at Max's suggestion: **the server is the default home for projects, and going offline is a Git clone + sync, not a thread move.**

## Decisions

| Topic | Decision |
|---|---|
| Model | Each project has one home: **On my server** (default once a server is paired) or **On this Mac** (always available; required for macOS-only projects). Threads live where their project lives and never move. Any paired Mac can drive a server thread. |
| Closing the laptop / overnight | Nothing to do: server threads keep running. |
| Take it to go | "Download a copy to this Mac": a normal Git clone linked to the server project. Work offline in local threads. "Sync with server" pushes and pulls branches; never resets, conflicts are ordinary merges. |
| Git | The server copy is a real Git repo with worktrees per thread, exactly like local. Mac⇄server Git traffic goes through the app's paired connection (git bundles), not SSH, so invited people never need SSH. GitHub clone is also offered for getting a project onto the server. |
| Whose server | Bring your own Linux VPS. App installs over SSH once; afterwards Macs connect straight to one encrypted port. |
| Sign-in | Pairing link, no central accounts, per-device key in the macOS keychain, list/revoke devices. |
| Sharing | Shared VPS from day one, for people who trust each other. One Linux account per person; the installer is admin, is root, and invites people. Stated plainly in the invite screen. |
| AI logins | "Sign in on server" per provider. The Mac's login files are never copied. |
| Alerts | In-app only. |
| Merge target | The branch a thread started from (`WorkspaceRecord.base_ref`), not always the default branch. Fixes local behaviour too. |
| Clients v1 | Another Mac running Bomb Code. |
| Dropped | Moving a running thread between machines (handoff, bring-back, frozen copies, agent memory rebuild). |

## Architecture

- **`bombd` (new crate `bomb_server`, static Linux binary)**, two roles:
  - **Gateway**, one per VPS, unprivileged system user. One TLS 1.3 port with mutual TLS: server cert pinned by the pairing link; each device presents its own self-signed cert; fingerprint → device → user. Unregistered certs may only call `pair`. Pairing link `bomb://pair?h=…&fp=…&s=<128-bit>`: single use, 10 min, stored hashed, rate-limited. Always proxies to a per-user Unix socket.
  - **Core per person**: `bombd core` running as that person's Linux user (systemd template unit `bombd-core@<user>`, socket `/run/bombd/<user>.sock`). `GrokPaths` and the provider CLIs all derive from `$HOME`, so files, SQLite, worktrees and AI logins are isolated by the OS with no data-model change. A tiny root helper `bombd-admin` with fixed verbs (`create-user`, `lock-user`, `start-core`, `stop-core`) is the only privileged surface. Unit hardening: `PrivateTmp`, `ProtectSystem=strict`, `NoNewPrivileges`, `MemoryMax`/`CPUQuota`/`TasksMax`.
- **Protocol (new crate `bomb_proto`)**: length-delimited frames: JSON request / response / event, binary chunk, and TCP-forward data. `bomb_core::rpc::dispatch(&AppState, Request)` calls existing `services::*`.
- **Event journal (`bomb_core/src/journal.rs`)**: the single bus subscriber. Persists events (moved out of `bomb_app/src/runtime.rs:67-70`, with the Foundry child filtering), stamps a per-core `event_seq`, keeps a ring buffer, fans out to bounded per-client queues (slow client → `Resync`, never silent drops). `snapshot{thread}` returns rows + `as_of` + pending approvals (DB rows drop `request_id`/options); the client hydrates with `Thread::hydrate`, then applies events above `as_of`. `transcripts.seq` cannot be the cursor because `append_message_merged` rewrites the last row.
- **Client**: `enum Core { Local(Arc<AppState>), Remote(Arc<RemoteCore>) }` with `core_for_project(root)` / `core_for(thread)` in `bomb_app/src/runtime.rs`. Project- and thread-scoped calls route by the project's home; app-level calls (settings, local auth, usage) stay local. Remote events feed the same `AppModel::apply_events`. New `UserMessage` event so a second Mac sees prompts; approvals are first-writer-wins (`AlreadyResolved`). A stable `project_id` links a server project to its offline copy.
- **Still local-only**: MCP/memory editing, settings, generic `kv_*` (would expose the plaintext JEV keys).

## Phases (stop for Max's review after each)

**0 — Merge targets the thread's base.** `merge_target(w)` in `bomb_core/src/services/workspaces.rs` (strip `origin/`; fall back to `default_branch` for `HEAD`/missing). Use in `merge_request`, `is_merged`, `close_feature`, `git_ui.rs`; add `base_branch` to `WorkspaceReview`. Rework the merge prompt for a base checked out in another worktree (find via `worktrees.list`, `ensure_idle` it) or nowhere (`git fetch . {branch}:{base}` after merging base in). Update labels in `views/thread_view.rs`, `views/project.rs`, `views/workspaces.rs`. Extend the real-Git tests beside the existing merge/close test.

**1 — Headless core over a local socket.** `bomb_proto`, `journal.rs`, `rpc.rs`, `bomb_server` with `bombd core --socket`. Persistence moves into the journal; `start_bridge` becomes a journal client. SIGTERM and app quit call `shutdown_all`. `UserMessage` event, pending-approvals snapshot. Linux CI build of everything except `bomb_app` (cfg gates already exist; audit `presence.rs`, `explainer.rs`, `default_projects_dir`, `open_login_url` at runtime). Dep: `tokio-util`.

**2 — Gateway, single user.** `bomb_server/src/gateway/{tls,pairing,devices,proxy}.rs`. Deps: `tokio-rustls`, `rcgen`, `sha2`, `rand`, `subtle`. Frame-size cap, pre-auth limits, idle timeouts.

**3 — Server projects in the app (chat).** `bomb_app/src/remote/{mod,connection,keychain}.rs`, `Core` routing, Settings → Servers (pair, list, revoke), a server/Mac marker on sidebar projects, reconnect with backoff, board and thread header working against a server project (`list_workspaces`, `review_workspace`, `project_overview::load`, `merge_request`, `is_merged`, `close_feature` over RPC), "New project" / "Add project" asks **On my server** or **On this Mac**.

**4 — Getting projects on and off the server (Git sync).** "Add project to server": GitHub clone, or one-time upload of a local repo. "Download a copy to this Mac" and "Sync with server": incremental `git bundle` both ways over the channel (chunked, sha256, resumable); incoming refs land in `refs/bomb/incoming/*` then fast-forward or become an ordinary merge; never reset; warn on LFS/submodules. Layout `~/projects/<project_id>/repo`. New `bomb_core/src/services/project_sync.rs` built on `grok_worktree::run_git`.

**5 — Server projects feel local.** File tree + read for server projects (`views/file_tree.rs`, Changes panel diff read-only), terminal as a streamed PTY (`bomb_core/src/terminal.rs` already uses portable-pty + vt100), dev-server preview through a TCP forward over the channel so `views/preview.rs` loads a local forwarded port (`devserver.rs` already reports the URL it serves).

**6 — Multi-user.** `bombd-admin`, template unit + hardening, invites (creates Linux user `bc-xxxx` + a pairing link bound to it), admin role checks.

**7 — Installer and server sign-in.** Wizard shelling out to system `ssh`/`scp` (inherits agent, `~/.ssh/config`, known_hosts): detect arch, upload sha256-checked `bombd`, install units, open the port (ufw/firewalld or instruct), check/install git, Node and provider CLIs, print the admin pairing link. Grok login reuses `LoginManager` (`grok_cli_wrapper/src/auth.rs`); Claude/Codex use a PTY login session with URL/code extraction (`auth.rs:549-600`); API-key field as fallback. Verify on a real VPS that Codex offers a device flow.

Until phase 7, the server is installed by hand from a documented command so phases 2–6 are testable.

## Reuse

`AppState::initialize_with_paths`; `services::{start_session, send_prompt, resume_saved_session, respond_approval, cancel_session, wait_until_idle, shutdown_all, start_mock_session}`; `workspaces::{ensure_idle, list_workspaces, review_workspace, merge_request, is_merged, close_feature}`; `project_overview::{load, describe_relation, describe_relation_short}`; `Thread::hydrate`; `grok_worktree::{run_git, WorktreeManager}`; `LoginManager`; `bomb_core/src/terminal.rs`; `views/button.rs`; the `Type` scale in `theme.rs`.

## Verification

- Phase 0: `cargo test -p bomb_core` real-Git cases: local base, `origin/main`, `HEAD`, deleted base, base checked out in another worktree.
- Phase 1: temp-`HOME` integration test driving `start_mock_session` through the socket; kill and reconnect with `since`, assert no missing or duplicate deltas; forced `Resync`; Linux CI green.
- Phase 2: loopback gateway tests: pair ok / expired / reused / rate-limited, wrong pin refused, revoked device dropped mid-stream, unpaired cert limited to `pair`, malformed and oversized frames.
- Phase 3: run the app against a local `bombd` with a second `HOME`; two app instances on one thread (prompt mirroring, approval first-writer-wins); board, merge and close on a server project.
- Phase 4: bundle round-trips both directions, incremental bundle, diverged branches produce a merge not a reset, interrupted upload resumes.
- Phase 5: open a file, run a terminal command, and load a Vite preview from a server project through the forward.
- Phase 6: systemd VM with two users: cross-user file and socket access denied; revoking a user stops their core.
- Phase 7: manual VPS checklist (Ubuntu 22.04/24.04, Debian 12; x86_64 + arm64; fresh install, upgrade, reboot; each provider login; real Node and Rust projects; second Mac; revoke).
- Each phase adds an `IMPLEMENTATION_LOG.md` entry, per `AGENTS.md`.

## Known limits to state in the UI

Server projects need a connection; offline work needs a downloaded copy. A `bombd` upgrade or reboot interrupts running turns (they return resumable, not running). The server admin can read everyone's data. macOS-only projects must live on the Mac. Running a personal AI subscription on a server is subject to each provider's terms.
