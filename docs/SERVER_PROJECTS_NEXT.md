# Server projects: where it stands and what is next

Written 2026-09-21. Branch `server-projects` (pushed to the `fork` remote, not merged into `gpui-rewrite`).
The approved plan is `docs/plan/server_first_projects_plan.md`. The running record is `IMPLEMENTATION_LOG.md`.

## The idea in one paragraph

Agents are child processes of the desktop app, so closing the laptop kills a running turn. The fix follows Zeron's
model: a thread lives on the machine where its project lives and never moves. A project's home is either **this Mac**
or **a server you own**. Threads in a server project keep running with the laptop closed, and any paired Mac can drive
them. Going offline is ordinary Git: download a copy, work, sync back. There are no accounts and no relay service; a Mac
pairs with a server using a one-time link and keeps its own key.

## What is built

| Piece | Where |
|---|---|
| Merge goes to the branch a thread started from | `bomb_core/src/services/workspaces.rs` (`merge_target`) |
| Headless core: event journal with sequence numbers, RPC over existing services | `bomb_core/src/journal.rs`, `bomb_core/src/rpc.rs` |
| Wire protocol, client, upload/download/live streams | `crates/bomb_proto` |
| Device identities, pinned TLS 1.3 with client certs, pairing links | `crates/bomb_link` |
| `bombd`: per-person core, gateway (pairing, devices, revocation, invites), root helper | `crates/bomb_server` |
| App: paired servers, routing by project home, Settings → Servers | `bomb_app/src/remote/`, `bomb_app/src/core_router.rs` |
| Git sync as bundles (send, download a copy, sync, clone from GitHub on the server) | `bomb_core/src/services/project_sync.rs`, `bomb_app/src/remote/sync.rs` |
| Server terminal, file tree, dev preview through a forwarded port | `bomb_app/src/remote/live.rs`, `bomb_server/src/core.rs` |
| Several people on one server (Linux account each), invite and lock out | `bomb_server/src/admin.rs`, `deploy/server/` |
| SSH installer and provider sign-in on the server | `bomb_app/src/remote/install.rs` |

## What has and has not been checked

Checked:
- All suites pass on the Mac when run serially (`cargo test -p bomb_core -p bomb_app -p bomb_proto -p bomb_link -p bomb_server -- --test-threads=1`).
  This includes a test that runs the real app → gateway → core path on loopback: pair, create a project, run a thread,
  sync both ways, terminal, forwarded preview, device removal.
- The GitHub Actions workflow `linux-core` passed on the fork: the server code builds and its tests pass on Linux
  (`bomb_link` is not in that job yet), and it produced `bombd-linux-x86_64` and `bombd-linux-aarch64`.
- Those two binaries are in `~/.bombcode/server/` on Max's Mac, where the installer looks. They need glibc 2.35 or
  newer (Ubuntu 22.04+, Debian 12); not Alpine.

Not checked:
- Nothing has run on a real server.
- None of the new screens has been seen (Settings → Servers, the server mark in the sidebar, Add project menu,
  project-page sync buttons, the sign-in terminal dialog).
- `deploy/server/` (systemd units, sudoers line, tmpfiles entry, socket activation, `useradd` flags) is a first draft.

## Next steps, in order

1. **Look at the app.** Build and run from this branch, open Settings → Servers, and check the new screens read well
   and fit the type scale. Fix what looks off before going further.
2. **One-person trial on a throwaway VPS** (Ubuntu 22.04 or 24.04).
   - Settings → Servers → "Set up a new server": SSH login and `address:7443`, then "Install and pair".
   - If it fails, the log lines under the button say which step. The same steps by hand are in `deploy/server/README.md`
     (`bombd up --data ~/.bombd --listen 0.0.0.0:7443 --public <address>:7443`, then paste the link it prints).
   - Open TCP 7443 at the hosting provider's firewall as well as on the server.
   - Install git, Node and at least one agent CLI on the server, then use "Sign in" from a server project.
3. **Walk the real flow.** New project on the server, start a thread, close the laptop, reopen and confirm it kept
   going. Put a local project on the server, make a change there, Sync, check the Mac's copy. Try the terminal and the
   dev preview on a small web project. Pair a second Mac and confirm both see the same thread and that an approval
   answered on one clears on the other.
4. **Check each provider's sign-in on a headless server.** Claude and Grok should show a link or code in the dialog.
   Codex's default login uses a browser callback on localhost and may need a device-code option; if it cannot work,
   add an API-key field as the fallback.
5. **Shared-server trial.** Follow the "Shared" section of `deploy/server/README.md` on a fresh VPS. Confirm that one
   person cannot read another's home or connect to another's socket, that "Invite a person" creates the account and a
   working link, and that "Lock out" stops their core and disconnects their Macs. Expect to fix the units here.
6. **Decide on merging `server-projects` into `gpui-rewrite`** once steps 1–3 hold up.

## Known gaps

- The Changes panel (diff and staging) is hidden on server threads.
- Sync does not detect or warn about Git LFS or submodules.
- A `bombd` restart or server reboot stops running turns; threads come back resumable, not running.
- The model picker for a server project lists that server's models only after it connects.
- Events a session emits before its row is first saved are not written to history (predates this work).
- Earlier discussion settled on "merge only on the Mac"; in the final server-first design server threads can merge on
  the server, and the two copies meet through Sync or GitHub. Revisit if that causes confusion.
- The server's owner is root and can read everyone's data. The invite screen says so; keep it that way.

## Useful commands

    # everything that matters, serially (the usage-cache test is flaky in parallel)
    cargo test -p bomb_core -p bomb_app -p bomb_proto -p bomb_link -p bomb_server -- --test-threads=1

    # a local server to poke at, with a scratch home
    HOME=/tmp/bombd-home ./target/debug/bombd up --data /tmp/bombd-home/gateway --listen 127.0.0.1:17443 --public 127.0.0.1:17443

    # fresh server binaries from CI
    gh run list --repo rhh4x0r/grok-build-tauri-control-panel --workflow linux-core
    gh run download <run-id> --repo rhh4x0r/grok-build-tauri-control-panel --dir ~/.bombcode/server
