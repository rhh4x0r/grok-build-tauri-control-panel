# Bomb Code for iPhone: design

## Context

Max wants an iOS companion to Bomb Code: open the phone, see every thread, read along live, answer approvals, send follow-ups, and start new threads, the same as on the Mac.

Most of what this needs is already built. The `server-projects` work (now on `main`) added three things:
- a headless core, `bombd`;
- a wire protocol, `bomb_proto`: JSON in length-prefixed frames, with `seq`-based catch-up;
- device pairing, `bomb_link`: pinned TLS 1.3 with client certificates and `bomb://pair?h=&fp=&s=` links.

`bomb_core::rpc::dispatch` already serves `list_threads`, `snapshot`, `start_session`, `send_prompt` (which resumes saved threads), `cancel_session`, `respond_approval` (first answer wins, others get `already_resolved`), `list_projects`, `list_backends`, `set_session_effort`, `rename_thread`, `start_mock_session` and more. To the server, **the phone is just another paired device, like a second Mac.**

Four gaps, found while exploring:
1. **The Mac app hosts nothing.** Its core runs in-process through `journal.attach_local()`, so a phone can't reach threads homed "On this Mac", which today is most of them.
2. **No push.** iOS drops sockets when the app is in the background, and alerts are in-app only.
3. **The transcript reducer can't be serialized.** `bomb_core/src/transcript.rs` and `presence.rs` (about 2.3k lines) produce `Entry`/`Body`, which have no serde. A Swift client would have to re-implement them, or reuse them from Rust.
4. **Phone-unfriendly protocol details.** Every client gets every event for every thread. `snapshot` has no paging. The gateway drops a client after 15 minutes of silence from its side.

## Recommended architecture

```
 iPhone                                      Mac (Bomb Code app)  or  VPS (bombd)
┌────────────────────────────┐              ┌───────────────────────────────────┐
│ SwiftUI views              │              │ gateway (pairing, devices, TLS)   │
│   ▲ view models (records)  │  mTLS 1.3    │   ▼ relays frames                 │
│ BombMobile.xcframework     │◄────────────►│ core: rpc::dispatch + journal     │
│  (Rust via UniFFI):        │  bomb_proto  │   ▼                               │
│  bomb_link + bomb_proto +  │              │ AppState → agent CLIs (ACP)       │
│  bomb_transcript reducer   │              │ push: APNs (HTTP/2, your .p8 key) │
└────────────────────────────┘              └───────────────────────────────────┘
```

**SwiftUI for the UI, a shared Rust core for everything below it.**
- GPUI doesn't run on iOS, so the UI is native SwiftUI.
- Pairing, TLS, the protocol, catch-up and the transcript reducer come from the existing Rust crates, compiled for iOS and exposed through UniFFI.
- The phone and the Mac then fold events the same way: one reducer, no Swift copy that drifts.
- Swift never sees wire JSON. It gets typed records and change callbacks.

### Where threads run (the two "homes")
1. **Server projects (`bombd`):** works today with no server changes. The phone pairs with the same `bomb://pair` link, scanned as a QR code.
2. **"On this Mac" projects:** a new **Phone access** switch in Settings. The Mac app hosts the existing gateway around its own `AppState`:
   - start `bomb_server::core` serving on `~/.bombcode/host/core.sock`, using the app's `AppState` and journal (`attach_remote` already exists);
   - point a `Gateway` at that socket.
   
   Phone-started turns go through the same event bus, so the Mac's UI mirrors them live, and the reverse holds too. While a turn runs, a power assertion stops idle sleep; this is a setting, on by default. A closed lid still stops things. That is what servers are for.

### Getting there off home Wi-Fi
- **Recommended: Tailscale.** No code needed. Put the tailnet name or IP in the pairing link. The Mac host listens only on the LAN or tailnet interface by default, never `0.0.0.0` unless chosen.
- A public VPS on TCP 7443 already works.
- No relay service. This keeps the "no accounts, no relay" principle from `server_first_projects_plan.md`.

## Work, in phases

### Phase 0: make the reducer portable
- New crate **`crates/bomb_transcript`**:
  - Move `bomb_core/src/transcript.rs` and `presence.rs` into it unchanged.
  - Depends only on `grok_events`, `chrono`, `serde_json`.
  - `bomb_core` re-exports `pub use bomb_transcript::{transcript, presence};`, so `bomb_app` doesn't change.
- Move the `TranscriptEntry` struct (`grok_persistence/src/lib.rs:63`) into `grok_events` and re-export it from `grok_persistence`. This keeps rusqlite out of the iOS build.
- Add `aarch64-apple-ios` and `aarch64-apple-ios-sim` to `targets` in `rust-toolchain.toml`.
- Check `cargo build --target aarch64-apple-ios -p bomb_proto -p bomb_link -p bomb_transcript`. ring and rustls build for iOS.

### Phase 1: protocol additions (back-compatible, still `PROTOCOL_VERSION = 1`)
All new fields are optional with `#[serde(default)]`, so old Macs and servers keep working.
- **Watch filter:** a `watch{threads:[id…]}` RPC, handled per connection in `bomb_server/src/core.rs`.
  - *Summary* events always go through: `session_created`, `session_status_changed`, `session_completed`, `session_cancelled`, `approval_required`, `approval_resolved`, `user_message`, `error`.
  - *Streaming* events (`agent_message`, `tool_call`, `plan_update`, `raw`) go through only for watched threads.
  - Without a `watch` call the connection gets everything, as today.
- **Snapshot paging:** `snapshot{id, limit?, before_seq?}` returns the last N rows plus `has_more`. Changes go in `bomb_core/src/rpc.rs` and the `grok_persistence` query.
- **Keepalive:** the phone sends `ping` every 60 s while in the foreground. The gateway's idle timer already counts client traffic, so no server change is needed.
- **New RPCs the phone needs:** `account_usage`, read-only `git_changes` / `file_diff` (later phase), `register_push` / `unregister_push`.

### Phase 2: `crates/bomb_mobile` (UniFFI, staticlib)
- Owns a tokio runtime.
- `MobileClient`:
  - `pair(link, label)`: makes a P-256 identity, calls `gateway.pair`, returns `ServerRecord`. Swift stores the key in the Keychain (`AfterFirstUnlockThisDeviceOnly`).
  - `connect(server)`: reuses `bomb_proto::client::connect`, adds reconnect with backoff (port `bomb_app/src/remote/mod.rs:174-206`), and keeps the cursor. On `resync`, it re-snapshots open threads.
  - `list_projects`, `list_threads`, `list_backends`, `open_thread(id)`, `close_thread(id)`, `send_prompt(...)`, `start_session(...)`, `respond_approval(...)`, `cancel(...)`, `rename(...)`, `upload_image(...)`.
- Callback interface `Listener`: `on_connection(state)`, `on_threads(list)`, `on_thread_changes(id, changes)`.
- FFI records mirror `Entry` / `Body` / `ToolRow` / `ApprovalCard` / `PlanDoc` / presence label. They are converted from the Rust reducer's `Change` stream. The tool and thought folding ("Thought · Ran N commands") reuses the reducer's output.
- Build script `scripts/build-ios.sh`: cargo for both iOS targets → `uniffi-bindgen generate --language swift` → `xcodebuild -create-xcframework` → `ios/BombMobile.xcframework`.
- Test `crates/bomb_mobile/tests/loopback.rs` runs pair → `start_mock_session` → `send_prompt` → approval → resync against an in-process gateway and core, the same way the existing loopback test in the server-projects suite does.

### Phase 3: iOS app MVP (`ios/BombCode/`, SwiftUI, iOS 17+)
Styling follows the Zeron preference: system light/dark, neutral panels, bubbles, disclosures.
- **Machines:** paired Macs and servers with connection dots. Pair by scanning a QR code (or opening a `bomb://` link; the app registers the scheme).
- **Threads:** grouped by project. Status: running, needs approval, done, error. Search. Swipe to rename or stop. Pull to refresh.
- **Thread view:**
  - transcript: user bubbles, agent markdown, collapsible thought/tool disclosures, plan card;
  - **approval cards** with the option buttons;
  - status line (phase, elapsed time, last tool).
- **Composer:** text (dictation comes from the keyboard), photo attach (PhotosPicker, downscaled to `ImageInput`), backend/model and effort pickers from `list_backends`, approval mode, Stop.
- **Security on the phone:**
  - new threads default to **plan** mode;
  - yolo/always-approve isn't offered on the phone;
  - the server-side deny rules apply as they always have.
- **New thread sheet:** project, backend/model, work location (worktree or inline), first prompt.
- Out of scope on the phone: Settings, MCP, memory, the terminal, dev preview.

### Phase 4: "Phone access" on the Mac
- `bomb_app/src/remote/host.rs`:
  - start and stop the core socket and the `Gateway`;
  - data in `~/.bombcode/host/` (`identity.json`, `registry.json`); **never under `~/.grok`**;
  - add the power assertion while turns run.
- `bomb_server` moves from a dev-dependency to a dependency of `bomb_app`. If that pulls in too much, split the `gateway` and `core::handle` pieces behind a `lib` feature.
- **Settings → Phone:**
  - on/off switch;
  - interface choice (LAN or Tailscale);
  - QR code holding a fresh one-time link for this Mac plus one per paired server (`gateway.create_invite`; TTL 600 s);
  - paired-device list with Remove (`revoke_device`).

### Phase 5: notifications
- New crate **`crates/bomb_push`**: an APNs HTTP/2 client with an ES256 JWT from your own `.p8` key, configured in `~/.bombcode/config.toml` (Mac) or bombd's data directory. No third-party relay.
- The core keeps device tokens per person (`register_push` RPC, stored in core kv).
- A journal subscriber sends pushes on `approval_required`, `session_completed` and `error`.
- Payload: thread label and kind only, never transcript text.
- Swift side:
  - suppress the banner if that thread is open on screen;
  - **Approve / Deny notification actions** open a short background connection and call `respond_approval`.

### Later
- Read-only Changes/diff view.
- Usage bars.
- Face ID app lock.
- Live Activity / Dynamic Island for a running turn.
- iPad split view.
- Optionally a Secure Enclave key, through a custom rustls `SigningKey` that calls `SecKeyCreateSignature`.

## Critical files
- Reuse as is: `crates/bomb_proto/src/{lib.rs,client.rs}`, `crates/bomb_link/src/lib.rs`, `crates/bomb_server/src/{gateway.rs,core.rs}`, `crates/bomb_core/src/{rpc.rs,journal.rs}`.
- Move: `crates/bomb_core/src/{transcript.rs,presence.rs}` → `crates/bomb_transcript`; `TranscriptEntry` → `grok_events`.
- Change: `bomb_core/src/rpc.rs` (paging, watch, usage, push), `bomb_server/src/core.rs` (per-connection filter), `bomb_app/src/remote/` (+`host.rs`), `bomb_app` Settings (Phone page).
- New: `crates/bomb_mobile`, `crates/bomb_push`, `ios/BombCode/`, `scripts/build-ios.sh`.
- Template for the client API: `bomb_app/src/core_router.rs` (`Core::Remote` arms) and `bomb_app/src/remote/mod.rs` (reconnect and resync handling).

## Verification
- `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and the serial suite: `cargo test -p bomb_core -p bomb_app -p bomb_proto -p bomb_link -p bomb_server -p bomb_mobile -- --test-threads=1`.
- `cargo build --target aarch64-apple-ios-sim -p bomb_mobile`, then `xcodebuild test` on an iPhone simulator. Swift snapshot tests use recorded `ControlEvent` fixtures.
- **End-to-end on the Mac without spending tokens:**
  1. `HOME=/tmp/bombd-home ./target/debug/bombd up --listen 127.0.0.1:17443 ...`
  2. Paste the printed link into the simulator.
  3. `start_mock_session`, then check: transcript streams, the approval card resolves, and backgrounding then foregrounding catches up through `since`.
- **Phase 4:** turn on Phone access, pair a real iPhone over Tailscale, start a thread from the phone, and watch it appear live in the Mac's sidebar. Answer an approval on the Mac and check that the phone's card clears.
- `BOMB_SMOKE=1 cargo run -p bomb_app` still passes after the reducer move.
- Record each phase in `IMPLEMENTATION_LOG.md`. With approval, also save this design as `docs/plan/ios_companion_plan.md`.

## Decisions (settled 2026-10-07)
1. **Both homes in v1.** The phone pairs directly with the personal Mac (Phone access, phase 4) *and* with each linked `bombd` server. It shows one combined thread list, with a machine badge on each thread. Phase 4 is part of the MVP, not a follow-up.
   - **One scan pairs everything.** The Settings → Phone QR code holds the Mac's own one-time link plus a fresh `gateway.create_invite` link for each server the Mac is paired with. The Mac can already request these as a known device of the same person. The phone pairs with each one directly, with its own key per machine. The Mac never relays server traffic, so server threads still work while the Mac is asleep.
   - The QR code holds several links, so it's encoded as `bomb://pair-bundle?l=<link>&l=<link>…`, and `bomb_mobile::pair_bundle` pairs them all. If one fails (for example the server isn't reachable from the phone), the others still pair and the failure is shown.
2. **Tailscale** (or something like it) for reaching the Mac and servers off home Wi-Fi. No relay.
3. **There is an Apple Developer account**, so APNs push (phase 5), TestFlight and long-lived device installs are in scope.
4. **Minimum iOS version: 17.**

Build order: phases 0 → 1 → 2 → 3 (built against a local `bombd` as the test rig) → 4 (Mac Phone access plus the combined QR code) → 5 (push).
