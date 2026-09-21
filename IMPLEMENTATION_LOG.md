# IMPLEMENTATION LOG — Grok Build Tauri Control Panel

**Orchestrator:** Grok Build (Central Orchestrator Agent)  
**Plan source:** `docs/plan/` (from `grok_build_tauri_multi_agent_plan.zip`)  
**Date:** 2026-07-10  
**Repo:** `grok-build-tauri-control-panel`

---

## Process

Each phase ran Planning → Implementation → Audit → Revise loops until **zero Critical/High** issues.  
Verification gates: `cargo check --workspace`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`.

---

## Phase 0 — Foundation & Discovery

### Planning wave
- Cargo workspace members defined (`grok_config`, `grok_cli_wrapper`, …, `src-tauri`).
- Path discovery (`~/.grok`), config TOML, sandbox profiles, CLI wrapper for `version`/`inspect`.
- Security default: `always_approve_default = false`, `plan_mode_default = true`.

### Implementation wave
- `crates/grok_config` — paths, TOML load/save, MCP/skill/plugin maps, discovery report.
- `crates/grok_cli_wrapper` — async typed CLI, headless spawn opts, baseline snapshot.
- Tauri 2 skeleton (`src-tauri`, capabilities, static `frontend/`).
- `AGENTS.md`, `README.md`, plan docs under `docs/plan/`.

### Audit wave
| Auditor | Findings | Severity |
|---------|----------|----------|
| Completeness | Plan sketches incomplete (empty dirs) — expanded in later phases | Low |
| Security | Always-approve default correctly false | Pass |
| Maintainability | Config crate focused | Pass |

### Revise wave
- Atomic config write (tmp + rename).
- Input validation stubs for names/cwd/prompt.

### Gate
- `cargo check` (after Phase 1+ crates present): PASS  
- **Status:** Phase 0 complete

---

## Phase 1 — Core ACP & Single-Session Engine

### Planning wave
- ACP JSON-RPC 2.0 NDJSON transport; initialize → authenticate → session/new → prompt.
- Event bus with broadcast fan-out.
- SessionRegistry + AgentHandle (ACP preferred).

### Implementation wave
- `grok_events` — `ControlEvent`, tool/plan/status events.
- `grok_acp` — `NdjsonTransport`, `AcpClient` with background notification loop.
- `grok_control_core` — DashMap registry, mock sessions, plan mode, approvals.
- Tauri commands: `start_session`, `start_mock_session`, `send_prompt`, `cancel_session`, etc.

### Audit wave
| Auditor | Findings | Severity |
|---------|----------|----------|
| Correctness | Mock cancel used transport → `SessionNotReady` | High |
| Concurrency | Arc + DashMap appropriate | Pass |
| Integration | Headless requires prompt | Pass |

### Revise wave (loop 1)
- Mock/offline ACP paths for cancel/prompt/set_mode/approval without transport.
- Unit test `mock_session_lifecycle` fixed.

### Gate
- Tests PASS including mock lifecycle  
- **Status:** Phase 1 complete — zero Critical/High

---

## Phase 2 — Multi-Session Orchestration & Worktrees

### Planning wave
- Concurrent session map already via DashMap; max concurrent from config.
- Worktree manager: git porcelain + grok CLI fallback.
- Permission engine with presets and deny-first evaluation.

### Implementation wave
- `grok_worktree` — create/list/remove/prune/diff/status.
- `grok_permissions` — safe/workspace/yolo presets, glob matcher.
- Commands: worktree CRUD, permission presets/evaluate.

### Audit wave
| Auditor | Findings | Severity |
|---------|----------|----------|
| Security | YOLO preset explicit; not default | Pass |
| Performance | DashMap avoids global write lock on list | Pass |
| Concurrency | Per-session isolation via worktrees | Pass |

### Revise wave
- Name validation on worktrees; force remove flag.

### Gate
- Unit tests for porcelain parse + deny `rm -rf`  
- **Status:** Phase 2 complete

---

## Phase 3 — Extensions, MCP, Skills, Memory & Scheduler

### Planning wave
- ExtensionsService mutates config + optional CLI wrap.
- MemoryService JSON + flush/dream.
- Scheduler interval/cron/once with rate limit + handler for headless spawn.

### Implementation wave
- `grok_extensions`, `grok_memory`, `grok_scheduler`.
- Scheduler job handler spawns headless agents (or records error if binary missing).
- Event bus emits MCP/memory/scheduler events.

### Audit wave
| Auditor | Findings | Severity |
|---------|----------|----------|
| Clippy | `type_complexity` on JobHandler | Med |
| Correctness | Cron delay error type mismatch | High |
| Security | Extension name validation | Pass |

### Revise wave
- Type aliases for JobHandler.
- `this_fail` uses `e.to_string()`.
- Scheduler add request DTO (too-many-args).

### Gate
- Scheduler interval test fires ≥1  
- **Status:** Phase 3 complete

---

## Phase 4 — Polish, Integrations & Finalization

### Planning wave
- Diff capture, SQLite persistence, export markdown, checkpoint, shutdown_all.
- Frontend tabs for all major surfaces.
- Final global audit.

### Implementation wave
- `grok_diff` — before/after + unified summary.
- `grok_persistence` — sessions, transcripts, kv, export.
- Full Tauri invoke surface + control-event bridge.
- Minimal dark UI (`frontend/`).

### Audit wave (global)
| Auditor | Findings | Severity |
|---------|----------|----------|
| Completeness | All report areas mapped to crates/commands | Pass |
| Clippy | `-D warnings` clean | Pass |
| Tests | All crate unit tests green | Pass |
| Security | No always-approve default; secrets not logged | Pass |
| Correctness | cargo check workspace green | Pass |

### Revise wave
- Clippy field-reassign-with-default in config tests.
- Partial-move fix in `persist_session`.
- Command name clash with `discover_environment` import resolved.

### Gate
```
cargo check --workspace          → PASS
cargo test --workspace           → PASS (all crates)
cargo clippy --workspace --all-targets -- -D warnings → PASS
```

**Final auditor consensus:** **ALL PASS — zero Critical/High remaining.**

---

## Wave summary

| Phase | Impl waves | Audit loops | Critical fixed | High fixed |
|-------|------------|-------------|----------------|------------|
| 0 | 1 | 1 | 0 | 0 |
| 1 | 1 | 1 | 0 | 1 (mock cancel) |
| 2 | 1 | 1 | 0 | 0 |
| 3 | 1 | 1 | 0 | 1 (cron error type) |
| 4 | 1 | 1 | 0 | 0 |

---

## Deliverables checklist

- [x] Multi-crate Cargo workspace  
- [x] ACP-first session engine  
- [x] Multi-session + worktrees + permissions  
- [x] MCP/skills/plugins, memory, scheduler  
- [x] Full MCP manager + 7-server catalog + session injection  
- [x] MCP plans under `docs/mcp_plans/` + `examples/mcp_setup.md`  
- [x] Diff + SQLite recovery + export  
- [x] Tauri 2 host + frontend shell  
- [x] AGENTS.md  
- [x] IMPLEMENTATION_LOG.md  
- [x] Plan artifacts in `docs/plan/`  
- [x] Public GitHub repository  

---

## Notes for operators

1. Real ACP requires `grok` on PATH and valid `XAI_API_KEY`.  
2. Use **Start Mock Session** without a binary.  
3. Prefer plan mode; avoid yolo preset except trusted repos.  
4. Frontend is intentional thin shell — backend is the production surface.

---

## MCP Integration Wave (post Phase 4) — 2026-07-10

**Plan source:** `docs/mcp_plans/` (`mcp_server_build_plans.zip`)  
**Orchestrator:** `docs/mcp_plans/mcp_build_plans/integrator/orchestrator_prompt.md`

### Planning wave
- Shared infrastructure first: `McpManager`, extended config, CLI wrappers, credentials, security, session injection.
- Then catalog for all 7 servers in order: filesystem → github → linear → x → browser → grok_build → custom.

### Implementation wave
- New crate: `crates/grok_mcp`
  - `types` — `McpServerConfigExt`, transports, scopes, add/update DTOs
  - `catalog` — 7 built-in templates with tools + risk flags
  - `security` — path denylist, URL HTTPS rules, command validation
  - `credentials` — `~/.grok/mcp_credentials.json` (0600), `${VAR}` resolve, masking
  - `injection` — attachment policy (high-risk requires approval), ACP payload builder, Linear ID detect
  - `manager` — list/add/update/remove/doctor/tools/suggest/session_mcp_payload
- `grok_cli_wrapper`: `mcp_add_http`, `mcp_doctor`, `mcp_tools`
- `SpawnOptions`: `mcp_server_names`, `approved_high_risk_mcp`, `include_auto_mcp`
- `SessionMetadata.mcp_servers` records attachments
- Tauri commands: `list_mcp_servers`, `add_mcp_server`, `update_mcp_server`, `remove_mcp_server`, `doctor_mcp_server`, `list_mcp_tools`, `list_mcp_catalog`, credentials, suggest, preview
- Frontend **MCP** tab: catalog CRUD, doctor, tools, credentials, session payload preview
- Docs: `examples/mcp_setup.md`, plans under `docs/mcp_plans/`

### Audit wave
| Auditor | Findings | Severity | Resolution |
|---------|----------|----------|------------|
| Security | High-risk auto-attach blocked without approval | Pass | by design |
| Security | `/` and `~/.ssh` filesystem paths denied | Pass | tests |
| Correctness | moved value in list_tools | High | fixed clone order |
| Clippy | unused HashMap import | Low | removed |
| Completeness | all 7 catalog entries | Pass | unit test |

### Revise wave
- Compile fix for tool description format after move.
- Prefer typed `mcp_add_http` in CLI add path.

### Gate
```
cargo check --workspace          → PASS
cargo test --workspace           → PASS (incl. 14 grok_mcp tests)
cargo clippy --workspace --all-targets -- -D warnings → PASS
```

**MCP auditor consensus: ALL PASS — zero Critical/High remaining.**

## Codex ACP startup fix — 2026-09-17

- Removed native `codex` from ACP discovery: it has no `acp` subcommand and exits when launched with piped stdio. Discovery now uses `codex-acp` or the existing `@agentclientprotocol/codex-acp` npx fallback.
- Reject explicit native Codex binary overrides (including symlink targets) with an actionable configuration error.
- Removed the obsolete native `codex acp` reasoning-flag branch.
- Validation: all 14 grok_config tests passed, including adapter discovery and override regression coverage; workspace check and strict all-target Clippy passed.
- Cached codex-acp 1.12.0 successfully completed ACP initialize using an isolated temporary CODEX_HOME. Normal-home handshake was blocked by the task filesystem sandbox; no authenticated model turn was sent.

## Projects and workspaces flow — 2026-09-17

Approved defaults: automatic checkpoint commits, merge-based Update, and read-only Inline conversations with an explicit transition into an isolated workspace.

### Implementation
- Added durable workspace records and session membership, with idempotent migration of existing conversations. Conversation deletion keeps shared workspace files and history.
- Added project sections, collapsible workspace conversation groups, project overview, workspace naming, and Cmd-Shift-N for a new conversation in the same workspace.
- New workspaces use a `bomb/<prompt-slug>-<id>` branch based on the repository's default branch (remote tracking ref when available). Failed isolation no longer falls back to editing the project checkout.
- Added automatic checkpoints after confirmed successful ACP turns, serialized workspace turns/Git operations, and explicit checkpoint failure reporting. Cancelled responses are invalidated; transport timeouts no longer report an active editing stream as idle.
- Inline sessions deny writes, terminal creation, and permission escalation at the ACP host, use planning mode, and omit MCP attachments. Moving to a workspace carries conversation context forward.
- Added Update, Changes/Preview navigation, branch/ahead/behind summaries, checkpoint restore, file revert, review comments, unpublished checkpoint squash with backup history, editable PR bodies, push/local merge, and archive that keeps branches and transcripts.
- Added project remote status, manual fetch/pull, periodic fetch, and PR/check status through `gh`. Pull and local merge require a clean default-branch checkout; push/PR requires saved changes.
- Kept the standalone Terminal tab as the plan's explicitly later item.

### Audit and validation
- Added persistence coverage for shared conversations, restart, archive, and deletion; Git default-branch and conflict-stage protection tests; read-only write denial even under an elevated approval stance; and workspace turn exclusion tests.
- End-to-end local lifecycle test uses a mock agent and temporary repository: create workspace → checkpoint → reject overlapping work → share a second conversation → delete first conversation without deleting files → enforce Inline read-only → review → archive while retaining branch.
- Lifecycle testing found and fixed macOS `/var` vs `/private/var` worktree path normalization during archive.
- Visually checked the native development app: migrated project/workspace sidebar and project overview. Corrected non-Git folder labels and header wrapping.
- Real remote push/PR creation was not exercised; no user repository was pushed or merged during verification.
- Final gates: `cargo test --workspace` passed (122 tests, 2 existing opt-in tests ignored); `cargo check --workspace`, strict all-target Clippy, and development app build passed. Final UI-only layout edits were rechecked with check/Clippy/build.

## 2026-09-16 — Workspace UX and saved image follow-up
- Restored provider logos on workspace rows and project overview cards; workspace tooltips list the models represented by their conversations.
- Replaced the repeated empty Inline/read-only rows with actual question conversations. Added paired Ask a question / Make changes actions and a new-conversation intent picker; existing workspace conversations retain their workspace context.
- Updated question-to-workspace wording and synchronized intent when selecting conversations.
- Fixed persistence role mapping for generated images: image rows now reach the image hydration renderer after reopening. Existing image files need no migration or regeneration.
- Added a regression test through image-event persistence, database reopen, and transcript hydration, checking the preserved bytes and attachment metadata.
- Validation: workspace check and strict all-target Clippy passed; full workspace tests passed (123 passed, 2 existing opt-in tests ignored). The first run timed out in the existing workspace lifecycle test; a full rerun passed. Development build passed.
- Relaunched Bomb Code Dev and verified provider logos, model tooltip, the paired intent controls, and the restored potato image in the saved conversation. The screenshot capture still clips the right side of the conversation, so this does not establish full-width visual layout correctness.

## 2026-09-16 — Remove sidebar Questions category
- Removed the confusing repeated Questions label and the extra header row for conversations nested beneath projects. Provider logo, title, and time/status now share one row.
- Workspace check, strict all-target Clippy, and development build passed. Relaunched Bomb Code Dev and visually verified the labels are absent.

## 2026-09-17 — Restore thread deletion confirmation
- Fixed invisible delete confirmations: the main application view now renders GPUI Root's dialog layer, plus its sheet and notification layers. Root stores their state but does not render them automatically.
- Added Cancel buttons to the sidebar's two delete confirmations and the Delete Thread action confirmation.
- Validation: workspace check, strict all-target Clippy, development build, and the existing workspace deletion/restart persistence test passed. Live click verification was unavailable because Computer Use denied access to Bomb Code Dev.

## 2026-09-17 — In-window Settings and opaque dialogs
- Replaced the separate Settings window with a lazily created screen inside the main window. It shares the app's title bar, artwork, tint, and Geist typography, uses a matching-width settings navigation sidebar, and includes a Back button that preserves the existing thread and panels.
- New-thread actions return to the conversation screen. Delete Thread is disabled while Settings hides the conversation; the thread-sidebar toggle is hidden there.
- Made the dark component background opaque so confirmation dialogs no longer show underlying content through their surface. The app's artwork and glass tint remain independently rendered.
- Updated the smoke workflow to open Settings through its normal action instead of creating another window.
- Validation: workspace check, strict all-target Clippy, development build, and diff whitespace checks passed. Live visual verification remains unavailable following the Computer Use access denial; the smoke workflow was updated but not executed.

## 2026-09-17 — Sidebar brand and Home navigation
- Moved the Bomb Code wordmark from the title bar to a clickable row above the sidebar's selected project folder.
- Clicking the wordmark returns to the welcome screen, deselecting the active project/conversation and closing review/preview panels without removing saved content.
- Validation: workspace check, strict all-target Clippy, development build, and diff whitespace checks passed. Live UI verification remains unavailable after the earlier Computer Use access denial.

## 2026-09-17 — Simplify conversation toolbar
- Unified toolbar actions with 12px labels, 28px controls, and compact icons. Removed redundant saved/live text, the inline branch/commit counters, and the dev URL from the conversation header.
- Renamed Conversation to New chat and Review / Ship to Changes, with a nonzero file count and branch details in a tooltip. Moved Update into the overflow menu with an explicit default-branch merge label.
- Renamed the preview control to Dev sidebar with a right-panel icon. Opening it now only toggles the panel; Start server / Stop server lives inside the panel, and its empty-state instructions match.
- Validation: workspace check, strict all-target Clippy, development build, and diff whitespace checks passed. Live visual verification remains unavailable following the earlier Computer Use access denial.

## 2026-09-17 — Composer context icon and approval-mode picker
- Replaced the folder glyph beside the conversation context with a chat glyph.
- Added distinct mode icons, readable Plan / Ask first / Auto / Full access labels, a dropdown indicator, and an amber Full access treatment. The menu separates titles from descriptions, marks the current selection, and explains the keyboard shortcut and permission-rule precedence.
- Preserved the existing backend mode identifiers and approval behavior. Auto's description now refers to the agent's policy rather than promising all automatically approved operations are safe.
- Reviewed project overview data and proposed a branch/workspace map, selected-branch details, and repository-wide PR cards; left the project screen unchanged pending design direction.
- Validation: workspace check, strict all-target Clippy, development build, and diff whitespace checks passed. Live visual verification remains unavailable after the earlier Computer Use access denial.

## 2026-09-17 — Visual project map, mode styling, and Shift+Tab
- Replaced the project overview with selectable local branch lanes, default/current branch indicators, ahead/behind comparisons, committed-file counts, workspace/chat links, recent commits, and selected-branch file details. Comparisons explicitly use the local default branch, not an inferred ancestry graph.
- Added repository-wide open GitHub PR cards with head/base branches, draft/review state, checks, mergeability, and browser links. Loading, no-remote, unavailable/authentication, empty, and 100-result-limit states are explicit; reads use a bounded, kill-on-drop GitHub CLI subprocess. No remote write is performed by loading the overview.
- Kept refresh, fetch, reveal, ask, new-workspace, and confirmed default-branch pull controls. Non-Git folders retain the question flow. Overview results are cached by project with duplicate-load protection.
- Made dark popovers opaque and tightened approval menu widths, icon alignment, text sizes, and line heights.
- Fixed Shift+Tab with composer-scoped bindings, including a more-specific input binding that overrides outdent. Replaced the composer's no-op mode action handler and stopped propagation to avoid cycling twice.
- Mode changes update thread metadata immediately, report/roll back failures, and persist for saved threads without requiring a live process. Existing read-only restrictions and backend permission semantics remain intact.
- Validation: workspace check, strict all-target Clippy, development build, and diff whitespace checks passed. All bomb_core tests passed (37 passed, 2 existing opt-in tests ignored), including new branch/PR-check tests and live/saved approval-mode coverage in the workspace lifecycle test. Live visual/keyboard verification remains unavailable after Computer Use denied app access; real GitHub PR retrieval was not exercised during automated tests.

## 2026-09-17 — Consume Shift+Tab before focus navigation
- Replaced composer-only Shift+Tab bindings with a main-window keystroke interceptor that cycles the current approval mode before GPUI can dispatch backward focus navigation or input outdent. It does not move focus or change composer text/selection.
- The interceptor is scoped to its owning window and subscription lifetime. Settings, dialogs, sheets, focus traps, popup menus/popovers, unrelated inputs, ordinary Tab, and extra-modifier combinations retain normal keyboard handling.
- Added routing regression coverage for initial/non-composer focus, composer typing, other inputs, overlays, and unrelated key combinations.
- Validation: workspace check, strict all-target Clippy, both shortcut routing tests, development build, and diff whitespace checks passed. Live focus/cursor verification remains unavailable after the earlier Computer Use access denial.

## 2026-09-17 — Git initialization empty state and UX findings review
- Replaced the ordinary non-Git-folder error card with “Git not detected,” concise guidance, and an “Initialize Git repo” action. Removed the Ask about this folder fallback from the error card.
- Added explicit repository detection so Git execution/access failures remain errors rather than incorrectly offering initialization. The action initializes a local main branch only on click, blocks duplicate submissions, refreshes the overview/status, and reports failures.
- Initialization stages no files, creates no commit, adds no remote, and recognizes parent repositories/linked worktrees rather than nesting a repository inside them.
- Reviewed TRA-90 through TRA-97 against current code. Prioritized Full access opt-in across menu/keyboard entry points, staged first-run authentication/project guidance, labeled create actions, accurate empty-state language, and docs cleanup. The old multipurpose Activity rail no longer matches the current screen; scheduler/extensions claims still lack matching settings screens. These recommendations were reviewed, not implemented in this change.
- Validation: workspace check, strict all-target Clippy, development build, three project overview tests, and diff whitespace checks passed. New initialization coverage checks untracked-file preservation, no initial commit, repeat calls, parent-repository detection, and relative-path rejection. No user project was initialized during verification; live UI verification remains unavailable after the earlier Computer Use denial.

## 2026-09-17 — Guided welcome and explicit Full access
- Added a central Connect → Choose project → Start conversation welcome flow with provider selection, existing login flows, connection refresh, and project selection. Explicit New chat now opens the composer rather than returning to the project overview. Git setup remains available for plain/empty repositories.
- Moved signed-out connection guidance into Home; the sidebar footer reports status. Added labeled Add project / New chat controls and distinct no-project/no-conversation messages, plus an Isolated workspace composer label.
- Separated Full access into an Advanced menu section with a Cancel / Enable Full access confirmation. Confirmation is scoped to the original conversation/project/workspace. Shift+Tab cycles only Plan → Ask first → Auto; new conversations, project switches, and empty workspaces do not inherit Full access. Removed Full access from the default-mode dropdown.
- Kept existing permission enforcement/read-only restrictions. Question intent now consistently sets worktree isolation off for the draft and opens the composer.
- Updated README and Quickstart for current navigation, modes, workspace behavior, Git setup, MCP attachment, and dev sidebar. Scheduler and Extensions are explicitly labeled backend capabilities without desktop management screens.
- Validation: workspace check, strict all-target Clippy, all six bomb_app tests, development build, and diff whitespace checks passed. Added tests for onboarding step order and keyboard exclusion of Full access. Live visual/authentication/confirmation checks remain unavailable after the earlier Computer Use access denial; no account was signed in or user configuration rewritten during verification.

## 2026-09-17 — Image actions, file browser, scratch chats and agent speed
- Added image context menus for user attachments, agent attachments and local generated images: native Copy image, Reveal in Finder, and Reveal in file tree. Embedded attachments are materialized in an app cache on reveal; failures show a toast.
- Added Dev server / File tree tabs to the dev sidebar. File browsing loads directories asynchronously, expands and scrolls to the requested image, highlights selection, handles external image directories, and resets with the active workspace. Symlink entries are not traversed as directories. Switching to files drops the native preview so it cannot cover the tree.
- Simplified approval summaries, moved raw request details behind a disclosure, used option labels for resolved requests, and removed the historical “restored” label without re-enabling old approvals. Command/edit details remain visible.
- Added Temporary chat on the welcome screen. It creates a private empty Git project under ~/.bombcode/chats and uses normal isolated workspaces and approval modes, with no project selection required. Chats persist; this is not an auto-delete/incognito feature. Bootstrap commits contain no files and do not use user hooks, signing or Git identity.
- Added a composer Speed selector for running agents advertising a speed/fast-mode/service-tier config option. It reflects the agent's current value, sends the exact advertised option/value, and reports failures. Unsupported agents and not-yet-started threads do not show a nonfunctional toggle. Reasoning effort is unchanged.
- Investigated actual persisted skill failures: reads of ~/.grok/bundled/skills/imagine/SKILL.md fail with “path outside workspace” in the host ACP filesystem handler. Recommended canonicalized, read-only installed-skill roots with symlink-escape coverage; no filesystem permission policy was changed in this phase.
- Validation: workspace check, strict all-target Clippy, full workspace tests (including new approval-summary, speed-capability and empty scratch repository tests), development build and whitespace checks passed. Two existing opt-in tests remain ignored. Native clipboard/Finder/menu interactions and provider-specific fast mode were not exercised live; Computer Use access was denied earlier in this session.

## 2026-09-17 — Recognize Codex fast mode
- Inspected the installed @agentclientprotocol/codex-acp 1.12.0 adapter: its actual config ID is `fast-mode`, with `off`/`on` select values when boolean config support is not advertised. The speed selector previously omitted this hyphenated ID.
- Recognize the real ID and display Standard / Fast labels. Capture config-option session updates and refresh the selector as app state changes instead of permanently caching the initial capability result.
- Validation: workspace check, strict all-target Clippy, grok_acp and bomb_app tests, development build and whitespace checks passed. Regression tests cover the adapter's actual payload, live option removal/reappearance and readable labels. Native UI interaction remains unverified after the earlier Computer Use denial.

## 2026-09-17 — Visible image generation, model history and pre-send speed controls
- Promoted image-generation placeholders from hidden tool details to their own inline rows, independent of activity collapse. Adapted the assistant-ui image-generation reference: staggered 8×8 dots, a subtle decorative gradient, a pulsing generation label, prompt caption, and requested aspect ratio when available. Pending calls show a static preparing state; terminal calls remove the placeholder and normal returned-image rendering supplies the result. Image reads/searches do not trigger placeholders.
- Preserved original tool identity/arguments on sparse ACP status updates so an image tool does not become an anonymous tool and lose its placeholder mid-generation.
- Fixed Fast visibility on saved threads and drafts: Codex always shows Fast · Default/Off/On beside the model selector. Choices apply to the next prompt after connection/resumption; unsupported Fast requests fail visibly before sending rather than silently using standard speed. Agent-advertised values are used without guessing protocol values. Model/thread changes reset draft speed overrides.
- Found that model selection had relied on an extra session/new/load model field which adapters can ignore. Now explicitly apply the advertised model config after new/load/resume, reject unavailable model IDs, and use Codex's reasoning_effort option (with existing effort fallback). The composer's reasoning choice is reapplied before the next prompt, including resumed threads.
- Added durable per-thread model identity history and overlapping smaller provider logos beneath branch labels, with exact model names in tooltips and overflow counts. Workspace rows keep the current/latest provider as the main mark. History is recorded from known session metadata going forward; older unrecorded identities are not inferred. Deleting a thread removes its history key.
- Model switches now add a visible, persisted in-chat notice after successful reconnection, including whether native session continuity or text-history fallback was used. Failed switches do not announce success. Existing fallback remains bounded to 40 saved entries, 2,000 characters per entry and about 24 KB total, plus project memory; it does not transfer internal model state or replay old binary attachments.
- Validation: workspace check, strict all-target Clippy, all workspace tests and development build passed; two existing opt-in tests remain ignored. Added coverage for generation tool detection, partial/aspect-ratio arguments, sparse tool updates, live model notices, persisted/deduplicated/deleted model history, exact speed values and advertised model/reasoning configuration. Live native UI/provider execution was not exercised because Computer Use access was denied earlier; no paid image generation was triggered for verification.

## 2026-09-17 — Provider-owned model catalogs and terminal timer freeze
- Replaced the desktop picker's hardcoded/configured model catalog fallback with live provider discovery: ACP model config options (including grouped options) or legacy availableModels, and `grok models` for Grok. Preserve exact provider IDs, names, descriptions and current model; no guessed model choices or static blurbs are offered when discovery fails.
- Query catalogs at launch, after detected authentication changes, and via Refresh provider models. Discovery uses an isolated read-only metadata session, no prompt, no login flow, and no Bomb Code thread; active ACP catalogs can be reused. Failures remain visible instead of falling back to invented IDs. Sending before a valid catalog/model is available preserves the composer draft and shows guidance.
- Fixed selected-default dispatch so existing threads send the actual displayed provider model. Provider-advertised `default` is now a valid selectable alias, and legacy ACP model selection uses session/set_model when config options are unavailable.
- Verified installed Claude and Codex adapters with the metadata-only catalog_probe diagnostic. Claude returned Default, Opus 5 (`opus[1m]`), Fable 5.1 (`claude-fable-5-1[1m]`), Sonnet 5 (`sonnet`), and Haiku 4.5 (`haiku`). Codex returned Astra, Sol, Terra, Luna and 5.5. Neither probe sent a prompt or generated billable content.
- Added a latched completion timestamp to presence. Failed/cancelled/completed turns stop accumulating elapsed time, duplicate terminal signals do not extend it, late idle notifications preserve failure, and a new prompt resets the clock. Local send errors also end the turn and settle running tool rows.
- Validation: workspace check, strict all-target Clippy, full workspace tests, development build and whitespace checks passed. Added exact-alias/default/grouped/legacy catalog coverage and timer tests for failure/cancellation/completion, duplicate signals, local errors and retry reset. Two existing opt-in tests remain ignored. Live native UI interaction remains unverified after the earlier Computer Use denial.

## 2026-09-17 — Generated image paths, provider modes and session startup
- Traced the missing Arcade image to a successful Grok image_gen result: the JPEG exists in Grok's session directory, while the reply uses `images/1.jpg`. Resolve those relative references from preceding explicit artifact metadata, including hydrated transcripts, rather than guessing session paths. Preserve artifact coordinates when shortening tool results. Recognize Grok's `imagine:` title during generation, and forward ACP agent-message image blocks as well as tool-result image blocks.
- Fixed false Failed status after provider switches: retire the old idle client without emitting a cancellation into the new turn. Explicit Stop/delete cancellation behavior is retained.
- Fixed new-thread startup race: Idle can arrive before the connected ACP client is installed. First-send readiness now requires the installed client as well as the session status, preventing early model/effort operations from reporting session not found.
- Surface connected-provider approval labels/descriptions and current-mode updates. Hide unsupported approval intents and skip them in keyboard cycling; retain host enforcement and Full access confirmation. Remember provider-reported Auto → Accept edits fallback for the connection, including when switching away and back. Native mode rejection now propagates instead of falsely reporting success. Before connection, host approval choices remain visible with a provider-resolution explanation.
- Validation: full workspace tests passed (two existing opt-in tests ignored), including regressions for artifact lookup/traversal, inline ACP images, generation title changes, ready-vs-idle race, retirement without cancellation, and provider fallback presentation. Final workspace check, strict all-target Clippy, targeted ACP/UI tests and development build also passed. Native UI interaction remains unverified after the prior Computer Use denial; no generation prompt or paid provider request was sent for these checks.

## 2026-09-17 — Restore thread Archive/Delete actions
- Found that single-conversation workspace rows replaced ordinary thread rows but only exposed Rename workspace. Restored Archive thread / Delete thread on those rows and added the shared actions to the open thread's overflow menu, including conversations without a worktree. Existing archived-thread rows retain Unarchive / Delete.
- Hide sidebar workspace rows when they have no unarchived conversations, so archiving/deleting the only thread actually removes its apparent duplicate. Empty workspaces and their files remain available through the project overview. Existing deletion confirmations and delayed dialog opening are preserved.
- Validation: workspace check and strict all-target Clippy passed; development build checked before launch. No real user thread was archived/deleted for testing. Native UI interaction remains unverified after the prior Computer Use denial.

## 2026-09-17 — Thread terminology and Fast opt-in
- Standardized user-facing creation, rename, project actions, review messages and settings on Thread terminology. The single-thread sidebar row now uses the actual conversation title and Rename thread edits that title rather than the underlying grouping name. Old saved Workspace created notices display as Thread created without modifying stored history. Git isolation is described as an isolated branch; internal worktree/storage identifiers remain unchanged.
- Fast mode now defaults explicitly to Off, including new chats and thread/model switches, and the selector offers Off / On instead of inheriting an agent default. The next prompt applies the explicit standard-speed choice when the provider exposes a speed option.
- Validation: workspace check and strict all-target Clippy passed; development build verified before relaunch. Native UI interaction remains unverified after the prior Computer Use denial.

## 2026-09-17 — Claude usage rate-limit handling
- Diagnosed the missing Services bars with a read-only usage request: Claude's credential was present and unexpired; the endpoint returned HTTP 429/rate_limit_error. Found 16 running development app instances, each using the old independent two-minute polling loop. No credentials were printed or changed.
- Added a shared Claude usage cache with nonblocking process-safe file locking, two-minute success caching and exponential failure backoff (five minutes up to one hour). Preserve last successful bars with an explicit stale/error message. Credential fingerprints keep cached usage separated across sign-in changes; cache files contain usage metadata only and are created owner-only.
- Services now shows usage errors rather than filtering failed fetches out. HTTP failures get concise status messages without including response bodies.
- Validation: workspace check, strict all-target Clippy and all five usage tests passed, covering HTTP failures, cache reuse, failure backoff, account change and concurrent instances. Development bundle updated after successful build. Did not start another app instance or terminate potentially active user turns; old copies must be quit before they stop polling. Native UI remains unverified after the earlier Computer Use denial.

## 2026-09-17 — Provider support reads, command menus and thread terminals
- Added read-only access to the active Grok, Claude or Codex provider's skill/plugin/memory locations, including configured provider homes and shared agent skills. Canonical containment blocks symlink escapes, credential files outside those roots remain inaccessible, explicit deny rules still win, and write boundaries are unchanged. Eligible support-file approval requests are automatically answered.
- Completed turns no longer restart on delayed usage, Running or approval-resolution events. ACP prompt completion explicitly settles the transcript; usage remains available as metadata.
- Model-switch notices show the previous and new provider logos with an arrow and model labels, including historical notices. Composer slash suggestions use only the connected selected provider/model's advertised ACP commands, with filtering, descriptions, argument hints and keyboard selection. Commands are inserted for review before sending.
- Added a Terminal action beside the thread header controls. Each thread retains its own bottom panel and multiple real PTY shell tabs, opened in its working directory. Tabs can be selected/closed; hiding the panel preserves shells. Includes ANSI colors/cursor rendering, terminal resize, scrollback, clipboard paste/copy-screen and control keys. Removing a thread releases its terminal sessions.
- Added regression coverage for provider support/credential/write/deny boundaries, symlink escapes, late completion metadata, command filtering, provider-switch rendering and terminal shell input/cwd/resize/exit/control keys.
- Validation: workspace check, strict all-target Clippy, workspace unit tests (two existing opt-in tests ignored), all 14 app tests and development build passed. An overlapping Cargo check initially invalidated a documentation-test artifact; the complete workspace documentation suite passed when rerun sequentially. Whitespace checks passed. Development bundle updated without launching another copy; native UI interaction remains unverified after the prior Computer Use denial.

## 2026-09-17 — Restore slash commands after app restart
- Found that the slash menu only read ephemeral per-thread command notifications. Restored threads had no command catalog, and Grok's CLI model discovery skipped ACP command discovery entirely.
- Metadata discovery now drains ACP command notifications without sending a prompt, caches the advertised commands alongside each provider's model metadata, and exposes them to the composer before an interactive thread connects. Live thread catalogs take precedence. Model metadata updates preserve the command cache; provider commands are no longer incorrectly gated on an exact model-string match.
- Verified against the installed Grok ACP adapter: it returned real built-in and skill commands, including compact, context, goal, review and bundled:imagine. No generation prompt was sent. All 40 ACP tests, workspace check and strict all-target Clippy passed, including command event/cache preservation coverage.

## 2026-09-17 — Rank slash-command search by command name
- Replaced provider-order substring filtering with stable relevance ranking: exact name, name prefix, namespaced command prefix, name segment prefix, then other name substrings. Description word-prefix matches are offered only when no command name matches, avoiding unrelated results such as claude-api displacing extra commands.
- Added regression coverage for exact/prefix/namespaced ranking, case-insensitivity, description fallback, rejection of mid-word description matches and preserving provider order for an empty query. Workspace check, strict all-target Clippy and both slash search tests passed.

## 2026-09-17 — Native Prompt Foundry and Services sizing
- Added the `bomb_foundry` domain crate for versioned contracts, graphs, five adapted loop templates, deterministic compilation, package import/export, SQLite revisions and a bounded run state machine. Graph placement is independent from execution order; malformed dependencies/return routes are rejected. Human gate receipts bind to the run revision/cursor/attempt, and revision returns invalidate downstream acceptance.
- Added the native Foundry screen in the existing shell, with Contract / Skill Loop / Runs, library search, section editing/refinement/restore, canvas and ordered graph views, provider/model overrides, reviewed composer insertion, and run controls. Draft refinement is transient until explicit Save/Run; applied edits to saved documents create revisions.
- Integrated fresh ACP stage sessions, correlated completion, parent-thread transcript/approvals, artifact-focused independent reviews, explicit restart recovery, pause/resume/stop and bounded retries. Added a read-only opt-in ACP probe. Evidence claims remain agent-reported and are shown alongside separately captured tool events.
- Fixed Services footer clipping by preventing flex shrink and limiting its height with internal scrolling, while allowing the thread list to shrink.
- Added user documentation in docs/FOUNDRY.md. Structured source/role/phase editing currently uses JSON; provider-folder installation, scheduled/parallel loops and Word/PDF export remain deferred as planned.
- Validation: workspace tests, workspace check and strict all-target Clippy passed before the final UI navigation adjustment; final rerun and development build recorded below. A live Grok probe passed correlated completion and structured-result parsing with no tools authorized. Native visual/interaction QA remains unverified after the earlier Computer Use denial.
- Final validation: all workspace tests passed (175 passed, two existing opt-in tests ignored), workspace check and strict all-target Clippy passed, and the development app built successfully. Run output/evidence is expandable to keep the inspector readable. Updated the development bundle for relaunch.

## 2026-09-17 — Foundry directly in the composer
- Replaced the composer’s editor-launching Improve prompt / Use skill actions with a single Run through Foundry button. It rewrites the typed request using the selected provider/model and puts the resulting prompt directly into the composer for review; no screen navigation, contract setup, or automatic send.
- Added working/error feedback and Undo. Attachments remain in place. Results are inserted only when the originating thread and original text still match, preserving concurrent edits and drafts in other threads. Blank requests and duplicate generation are disabled.
- Reused Foundry’s transient read-only ACP generation path; prompt-writing instructions preserve intent and scale detail to the request instead of requiring every task to become a plan. The advanced editor remains optional in the sidebar.
- Validation: two output-parser tests passed (clean extraction, missing/empty/incomplete output), workspace check and strict all-target Clippy passed. Native interaction remains unverified after the prior Computer Use denial.
- Development build passed; updated the app bundle and reopened it with the simplified composer action.

## 2026-09-17 — Foundry intake and structured contracts
- Compared the website’s current intake/schema/compiler through its repository. Replaced the immediate short rewrite with an inline composer setup: Fast Draft / Full Project, five target agents, nine explicitly selected work types, optional approval notes and classified sources. Generation stays in the composer; review, Undo and manual Send remain.
- Added validated PromptOptions and generation of a structured ProjectContract through the selected ACP model. Confirmed request, depth, target, work type and source roles are pinned; user approval notes survive provider changes. Malformed responses preserve the draft. The target agent is separate from the generator model and does not alter chat permissions.
- Rendered contracts with readable website-style headings, phases, responsibilities, bullet lists and original request instead of raw JSON section bodies. Source references are included without being fetched. This aligns intake/format, not byte-for-byte output from the website’s deterministic compiler.
- Validation: 179 workspace tests passed (two existing opt-in tests ignored), workspace check and strict all-target Clippy passed. Added Tetris intake, missing work-type, pinned choice/approval and malformed response coverage. Native visual interaction remains unverified after the earlier Computer Use denial.
- Development build passed; updated and reopened the dev app with the new intake flow.

## 2026-09-17 — Actionable destination setup before submit
- Added a read-only project/Git/initial-commit check before clearing or submitting a new-thread draft. Missing setup now presents an inline card with Choose existing folder, Create new project, Use temporary chat, and Initialize Git when appropriate; the generated prompt stays in the composer.
- New-project creation uses a native name/location picker, creates a new directory without overwriting existing folders, initializes its own Git repository and creates an empty first commit. Initializing an existing folder explicitly leaves staged/untracked files out of the commit; the card explains that existing uncommitted files do not appear in an isolated branch.
- Setup actions do not send the prompt. The user presses Send again after selecting the destination. If subsequent thread creation fails, the composer restores the unsent draft and attachments; newer edits are preserved with an explicit restore action.
- Validation: both setup tests passed (staged/untracked preservation and nested new-project isolation/no overwrite), workspace check and strict all-target Clippy passed. Native visual/file-picker interaction remains unverified after the prior Computer Use denial.
- Development build passed; updated the dev bundle and reopened it for the new setup flow.

## 2026-09-17 — Constrain the sidebar to the visible window
- Found the remaining footer clipping above the sidebar: the resizable main split renders at 100% height while it is a sibling below the title bar. Wrapped it in a flex-growing, zero-minimum-height viewport so its percentage height resolves against the remaining space instead of extending below the window.
- Constrained the sidebar overflow to that viewport and prevented service blocks from shrinking. The existing Services scroll region remains available on short windows; its bottom is now inside the main viewport.
- Native visual verification remains unavailable after the earlier Computer Use denial. Validation uses workspace check, strict all-target Clippy and development build.
- Workspace check, strict all-target Clippy and development build passed. Updated the dev bundle for relaunch.

## 2026-09-17 — Native Enhance Prompt composer controls
- Renamed the action to Enhance Prompt and moved its sparkle button immediately right of the attachment control, before Send. Removed the detached full-width action row; the trigger retains selected styling while open and displays Enhancing during generation.
- Replaced the full-width form with a compact, right-aligned composer panel: native selected depth buttons, labeled work-type/target dropdowns, progressive disclosure for optional context, close/Escape dismissal, and a clear primary action. Existing input values and source-role controls are retained when optional fields are collapsed.
- Used assistant-ui's composer and elicitation-form patterns as design guidance (https://www.assistant-ui.com/elements/composer and https://www.assistant-ui.com/elements/elicitation-form), implemented in GPUI using the app's own surfaces, typography, buttons and motion. The toolbar wraps on narrow widths; optional content scrolls independently of the panel header/footer. Added a subtle activity pulse and kept review/Undo behavior.
- Workspace check and strict all-target Clippy passed. Native visual/interaction verification remains pending after the prior Computer Use denial.
- Development build passed; updated and reopened the dev bundle.

## 2026-09-17 — Enhance Prompt quick choices and source pickers
- Replaced the flat-black panel with a subtle opaque charcoal/blue-gray gradient (and a light-theme variant). Added icon buttons for Research, Audit, Planning and Implementation, with an Other menu for the remaining work types.
- Removed manual target selection from the main flow. Both the visible provider badge and generation target follow the currently selected composer provider, including switches while the panel is open.
- Added native multi-file selection and a searchable saved-memory picker using Bomb Code's existing memory service. Only explicitly selected memory entries are included. UTF-8 text files up to 18 KB become bounded source snapshots; binary/larger files are explicitly marked as references for later inspection. Existing source roles/removal remain, duplicate snapshots are ignored, and generation waits for selected files to finish loading.
- Added a regression test for actual text inclusion and explicit binary/large-file reference behavior. Native visual/picker verification remains pending after the prior Computer Use denial.
- Validation: source-handling regression test, workspace check, strict all-target Clippy and development build passed. Updated and reopened the dev bundle.

## 2026-09-17 — Grok Enhance Prompt cancellation and response parsing
- Separated transient contract generation from execution-stage instructions. Enhancement no longer asks the provider to inspect artifacts or perform the requested project; read-only permissions remain enforced.
- Accept literal JSON string control characters and fenced contract JSON, skipping quoted placeholder tags while retaining contract validation and user-confirmed fields. Cancellation keeps the original draft and now includes a retry explanation.
- Five intake regression tests passed. Live Grok ACP test successfully enhanced “Build a 2d tetris game” into a validated contract. Added an opt-in full-path probe for future provider checks.
- Workspace check, strict all-target Clippy and development build passed; updated and reopened the dev bundle.

## 2026-09-17 — Composer Run with review loop
- Added a native repeat-icon action next to Enhance Prompt. It starts a bounded Plan → Build and verify → Independent review → Approve result workflow from the current draft, provider/model and approval mode.
- Review failures return to build; existing return/attempt limits and human approval remain. Planning and independent review use read-only sessions. Current thread/project or temporary-chat fallback uses the existing Foundry orchestration and thread controls.
- Prevented duplicate startup, kept drafts on failure, hydrated the new thread before selecting it, and only cleared an unchanged submitted draft. Image attachments receive an explicit unsupported-source message rather than being dropped.
- All 15 Foundry tests, workspace check and strict all-target Clippy passed, including a new regression covering build/review revision and final human approval. Native visual verification remains pending.
- Development build passed; updated and reopened the dev bundle.

## 2026-09-17 — Thread-native review loop progress and decisions
- Replaced the debug-style Foundry header with a composer-adjacent stage strip, provider/model and frozen completed durations, expandable findings/evidence/file links, and explicit running/waiting/paused/blocked/stopped/completed states. Added inline approve, change requests, pause/resume/stop, explicit retry-limit extension, Preview and View changes; advanced run editing remains available.
- Completed stage replies collapse to readable summaries. Raw foundry-result envelopes are hidden during streaming (including split tags) and when replaying saved conversations; Technical details preserves raw output. Provider graphics and elapsed times identify stage replies.
- Added token-validated feedback transitions back to build and independent review, invalidating downstream acceptance and retaining limits. Approval clears stale waiting notes; older completed runs suppress the stale note in both thread and advanced views. Generic turn status resumes for subsequent ordinary prompts rather than displaying stale loop timers.
- Adapted assistant-ui Task card, Tool timeline and Approval card patterns to native GPUI: https://www.assistant-ui.com/elements/task-card, https://www.assistant-ui.com/elements/tool-timeline, https://www.assistant-ui.com/elements/approval-card.
- All 18 Foundry tests pass. Core suite: 57 passed, two ignored, one usage-cache assertion failed under the full suite and passed in isolation; no usage-cache changes made. Native visual/interaction verification remains unverified after the prior Computer Use denial.
- Final workspace check, strict all-target Clippy and development build passed; updated and reopened the dev bundle.

## 2026-09-17 — Remove standalone Foundry sidebar entry
- Removed the out-of-place Foundry sidebar button. Enhance Prompt, Run with review loop, and the loop panel’s Advanced details action remain the entry points.
- Workspace check, strict all-target Clippy and development build passed; updated and reopened the dev bundle.

## 2026-09-17 — Thread locations, branch state, Changes redesign and loop dismissal
- Added Close/Show details to the review-loop panel and linked completed loops with uncommitted files to the commit dialog.
- Added Work in (new branch with base selector, current checkout, existing local branch) and independent read-only access. Existing branches reuse or acquire a working copy without switching the main checkout. Shared locations retain explicit permission records, block overlapping turns and cannot be removed/rewritten through archive/restore/squash.
- Migrated the old unique-path workspace schema transactionally so read-only and writable conversations can share a checkout without inheriting permissions. Preserved ids, associations and legacy defaults; covered migration and foreign-key integrity in tests.
- Added a header branch menu with local-base and upstream comparisons, working folder access and a direct-checkout badge. The active sidebar branch reflects live Git state and dirty status.
- Replaced the Changes command collection with Uncommitted/Branch changes tabs, file statistics, explicit commit selection, per-file diff views and a stable action footer. Commit dialogs preserve unselected staged changes; PR dialogs expose source/target/title/body. History/restore, update and squash move behind More.
- Local merge now works with remotes and remains separate from pushing the default branch. Dynamic default-branch names and no-force pushes are used. Added Git-action progress and periodic local review refresh while Changes is open.
- Validation: selected-file commit test (including unselected staged changes), workspace integration tests covering direct checkout permissions, existing branches without switching main, archive protection, merge with a local bare remote and separate push; persistence migration tests; Foundry regression suite. Native visual verification remains pending after the prior Computer Use denial.
- Final workspace check, strict all-target Clippy and development build passed. Updated and reopened the dev bundle.

## 2026-09-17 — Remove redundant sidebar project selector
- Removed the selected-project dropdown beneath the Bomb Code logo. Project groups, search, Add project and New chat remain available.
- Workspace check, strict all-target Clippy and development build passed. Native visual verification remains pending.

## 2026-09-19 — Optional JEV model suggestions within a thread
- Added global-only, default-off model suggestion settings: JEV direct or Vercel AI Gateway, editable starter preferences, masked key entry, macOS Keychain storage/removal, and an explicit connection test with no project context. Environment-key fallback is supported; keys are never saved in project config or passed in process arguments.
- Added bounded text evaluation via the documented TypeSafe `/v1/systemone` and Vercel `/v1/evaluate` APIs. Only discovered models from signed-in runnable providers are offered. Conservative probability/margin gates reject uncertain, malformed, unavailable, or unchanged selections; timeouts and errors send with the current model.
- Composer evaluates on send and offers Switch & send / Use current. Acceptance uses the existing thread handoff and preserves approval settings. Dismissed targets and per-thread opt-outs persist. Exact draft/destination/preferences checks and request generations discard stale/cancelled results. Attachment turns and slash commands bypass evaluation.
- Added regression coverage for uncertain/invalid recommendations, bounded payloads, curl config escaping, durable isolated thread preferences, and default-off/global-only config behavior. Workspace tests passed with `--test-threads=1`; the parallel suite reproduced the previously documented unrelated usage-cache assertion failure. Workspace check, strict all-target Clippy, and development build passed.
- Opened the updated development binary as Bomb Code Preview. Computer Use access was not approved, so native visual verification remains unconfirmed. Authenticated JEV/Vercel evaluation was not exercised; the explicit Test saved connection action is available after the user supplies a key.

## 2026-09-19 — Diagnose invisible JEV results and allow Keychain startup time
- User confirmed Preview with JEV enabled but no suggestion for a frontend prompt. Saved configuration selected Vercel; Keychain metadata confirmed the key exists without exposing its value.
- Verified the saved connection with a synthetic live evaluation. A second synthetic frontend-routing request returned the frontend candidate at 0.98 probability / 0.97 confidence and passed the real decoder. These checks took about 9–10 seconds including Keychain access, exceeding the original eight-second total send-time budget. Raised that budget to fifteen seconds; the explicit connection test permits thirty seconds for Keychain prompts.
- Added persistent composer feedback for a current-model decision, uncertain/invalid recommendations, skipped evaluation (missing discovered alternatives, dismissed targets, or unsupported attachments/commands), and errors. HTTP failures now report actionable status categories without exposing response bodies or secrets; Keychain access errors no longer masquerade as missing keys.
- Moved Test connection into a dedicated Settings group with an outlined button and persistent status. Added explicit opt-in live tests using only synthetic data.
- Validation: saved-key connection and live frontend-routing tests passed. Native visual verification remains unavailable because Computer Use access to Preview was not approved.

## 2026-09-19 — SQLite credentials, measured latency, and advisory confidence
- Split timing established the bottleneck: 9,621 ms in macOS Keychain lookup versus 409 ms for the HTTP subprocess, network, and JEV response. At the user's request, moved routing keys into the existing SQLite settings table. Request-time reads never access Keychain. Added one-time verified migration; the user's Vercel key was copied and verified, and its legacy Keychain entry was removed using the system credential tool after the native deletion attempt failed.
- A live synthetic routing check with the migrated key measured 1 ms for SQLite credential lookup and 773 ms for HTTP + JEV, completing in 0.78 seconds. Secrets were never printed. Credential saves/migration restrict the database and existing WAL/SHM files to owner access on Unix.
- Made task-specific preferences take priority over the general stay-on-current/follow-up preference. Advisory suggestions now require a 50% selection probability and a 10-point lead, with no second veto from the separate confidence metric; user acceptance is still mandatory. The composer shows selected/current/runner-up scores and the exact unmet threshold. Invalid or incomplete responses have separate error messages. The last evaluation summary is retained per thread without prompts or credentials.
- Added SQLite credential reopen/replace/remove and file-mode tests, plus advisory-threshold and explicit-score tests. Workspace tests passed serially. Native UI access remains unapproved; behavior was verified through unit tests and live synthetic API calls.

## 2026-09-19 — Smart Model Routing composer and review presentation
- Implemented Max's accepted UI proposal: renamed the feature Smart Model Routing, moved the per-thread toggle and diagnostic feedback into a composer icon/popover, and added a disabled-state tooltip pointing to JEV settings. Removed the permanent JEV status row and accepted-message residue.
- Inline recommendations retain the draft and offer connected model selection, compatible reasoning-level selection, Switch & send, Keep current, and an expandable explanation with evaluation scores. Choices remain separate from composer preferences until accepted; existing stale-draft and provider-availability checks still apply. New drafts use global routing settings; the persistent thread toggle becomes available once the thread exists.
- Model-switch transcript records include the provider-reported reasoning effort and explain adjustments or unavailable reporting. The picker follows reported adjustments. ACP config updates now preserve returned currentValue instead of overwriting provider normalization; failed effort application records the completed model switch while preventing prompt dispatch.
- Completed/stopped review loops live in the scrollable conversation and default to collapsed. Presentation state persists in SQLite per run, status, and decision token, so hiding an older gate cannot hide a new approval request. Active reviews remain next to the composer. Failed text turns expose Details and Retry through the normal send/routing path; an existing draft disables Retry.
- Added regression coverage for destination effort compatibility and provider-adjusted/acknowledged effort values. Full serial workspace tests passed (199 passed, 4 intentionally ignored). Final core verification passed (65 passed, 4 ignored), as did workspace check, strict Clippy, and the Preview build. Native visual verification remains unavailable because Computer Use access to Preview was previously denied; no additional external JEV evaluation was needed for these UI changes.

## 2026-09-19 — Inline routing recommendation styling
- Restyled the recommendation using the completed model-switch card from Max's screenshot as the reference. Proposed and completed handoffs now share a card frame and provider/model identity component: tinted provider marks, current → destination, quiet background, restrained typography, and rounded border.
- Embedded the destination selector in the handoff row, with a compact reasoning selector, continuity description, and action row. Removed the redundant Suggested/Current headings and outlined form controls; aligned the recommendation with the transcript content width.
- Workspace check and strict Clippy passed. This is a presentation-only change; existing acceptance, draft, and routing behavior remains intact. Native visual verification remains unavailable following the earlier Computer Use denial.

## 2026-09-19 — Saved Claude context variant mismatch and stale review results
- Investigated Max's failed Fable switch. Metadata-only live adapter probes reproduced the mismatch: fresh sessions advertised `claude-fable-5-1[1m]`, while the affected saved session advertised `claude-fable-5-1`. Setting the saved session to its advertised concrete ID succeeded without sending a prompt.
- ACP selection now permits the narrow Claude concrete-ID `[1m]` → same concrete ID fallback when the explicit context variant is absent. Exact choices still win; other versions, providers, and family aliases remain rejected. The live session record and composer track the applied variant, and the switch transcript explicitly explains the adjustment.
- Removed completed/stopped review-loop rows from the normal transcript entirely. Results remain accessible through the thread's ellipsis menu → Review loop results. Active reviews and pending approvals remain visible.
- Added regression coverage for exact-match precedence and refusal to substitute different model versions, providers, or family aliases. Workspace check and strict Clippy passed. Full serial workspace tests passed (200 passed, 4 intentionally ignored), and the final registry tests passed (4 passed). Native visual verification remains unavailable following the earlier Computer Use denial.

## 2026-09-19 — Retry failed ACP startup
- Found that a failed ACP handshake leaves a registry entry without an installed client; send treated that entry as live and skipped reconnection. Resending now retires and resumes failed connection placeholders before applying options or dispatching a prompt. The last durable ACP session ID/history is preserved rather than overwritten by the failed placeholder. In-progress handshakes and failed turns on usable clients are not retired by this check.
- Renamed Retry to Retry last prompt with a tooltip describing resend/reconnect behavior. Retry accepts an empty composer or the same restored failed prompt, protects a different draft, and updates its enabled state when the composer changes. A stale click now explains why it is blocked instead of silently returning.
- Added registry regression coverage for failed placeholders versus live failed turns and in-progress startup. Workspace check, strict Clippy, and all five registry tests passed. Native end-to-end clicking remains unverified because Computer Use access was previously denied.

## 2026-09-19 — Plan project feature tracking and parallel model work
- At Max's request, inspected the current workspace/session, Git, Foundry, and preview services and drafted docs/plan/project_features_parallel_work.md. The proposal covers a project/feature/task model, isolated parallel execution, contract dependencies, integration workspaces, validation pinned to revisions, manual landing, and staged automation.
- Identified existing foundations and explicit gaps, including the single-server preview manager, default-branch merge action, stage-based Foundry execution, and global locking that needs a concurrency audit.
- Planning documentation only; no runtime behavior changed or automatic work started. Checked the diff for whitespace errors; compile/test gates were not rerun for prose-only changes.

## 2026-09-19 — Simplify feature workflow using Multi-Agent Max and Prompt Foundry
- Inspected temporary read-only reference checkouts of Transformation-Agency/multi-agent-max at ee7ab8f and jedisherpa/prompt-foundry at 9cb4e5e. Confirmed the latter is the upstream referenced by Bomb Code.
- Drafted docs/plan/project_workflow_lightweight.md: portable project/feature/task Markdown, explicit document/runtime ownership, immediate UI exploration with provisional mocks, specific waiting dependencies, optional short prompt enhancement/review, focused task checks and combined feature verification. Updated the earlier plan to remove a universal contract-first startup assumption.
- Noted upstream Foundry schema/documentation drift: current code and Bomb Code's older shape both use version 1.0.0 with incompatible fields. Proposed explicit format adaptation instead of assuming interchangeability. No upstream scripts executed, no runtime changes, no project scaffolding installed. Prose-only validation: git diff --check.

## 2026-09-19 — Optional project features and explicit parallel task workflow
- Implemented the authorized first feature workflow in a native center pane: ⌘K New feature / Project features, ⌘⌥N, and a project-overview entry. Reused the routing handoff card frame, provider marks, toolkit inputs, menus and buttons; no extra windows. Tracking is off per project, including imports. Enable/disable alone creates no files or model sessions, and ordinary threads stay available.
- Added versioned Markdown feature records, editable task briefs, explicit project-guide/status generation, local Open/Done/Archived tracking, existing-thread attachment, and optimistic external-edit protection. SQLite holds local preferences/runtime links and immutable accepted launch prompts. Records are only written by explicit actions; existing instructions/unrelated files are preserved, symlink paths rejected, and project/workspace write guards reused. Task handoff files are created only in the new writer worktree.
- Added manual connected model + effort assignments, project role defaults and inline JEV proposals with acceptance, provider/draft/config staleness checks, source-thread opt-out, and project/global routing preferences. Accepted assignments do not modify permissions; linked threads retain their launch choice and use their own composer for later switching. Provider-reported effort is retained and restored on thread selection to avoid displaying another thread's previous effort.
- Independent tasks launch through existing session/worktree services with Ask permissions. Explicit dependencies validate exact Ready-for-review commits and brief revisions, then combine pinned commits in a new task worktree. Conflicts retain a recoverable thread and prevent prompt dispatch. Review tasks use a selected checkpoint in a separate read-only worktree; target movement during creation blocks dispatch. Task readiness and manual feature labels are not merge/deployment approval.
- Reused Foundry Fast Draft for optional inline brief enhancement with proposed replacement and Undo. Existing per-thread review-loop controls remain available; no automatic scheduler, upstream schema conversion, merge queue or concurrent preview-server manager is introduced. Documented the workflow and boundaries in docs/FEATURES.md.
- Validation: full serial workspace suite passed (210 passed, 4 intentionally ignored), workspace check and strict Clippy passed, and Preview built successfully. Nine new tests cover document round trips/external edits, optional enablement, two isolated writers, stale-checkpoint invalidation, real dependency code combination, read-only snapshot review, recoverable conflicts and retry prompt persistence. Model sessions in these tests are mocks; no live JEV/provider prompts were sent. Native visual verification remains unavailable after the earlier Computer Use denial. Updated the Preview bundle without interrupting its running process; the session database reports running/waiting sessions, so a restart requires a user decision.

## 2026-09-19 — Status text containment and conversational project UX research
- Investigated Max's screenshot of shell text painting over transcript rows and the composer. Provider tool titles can contain entire multiline commands; the fixed-height status row previously rendered them without clipping. Status summaries now use the first nonempty line, normalize whitespace, and truncate long Unicode titles while preserving the original tool data. The status row shrinks/ellipsizes within its width, its explanation has bounded scrolling, and the transcript area clips painting at its boundary.
- Validation: all 14 presence tests passed, including new multiline-command and long-Unicode regressions. Workspace check, strict Clippy, and the Preview build passed. Updated the Preview bundle atomically and verified its executable hash against the build. The running app was not restarted. Native visual verification remains unavailable following the prior Computer Use denial; no live provider calls were needed.
- Researched Linear board/Peek/Triage, Vibe Kanban board/review/browser workflows, and NN/g progressive disclosure. Recorded the proposal and primary sources in docs/plan/project_run_ux.md: project conversation as the primary input, linked feature kanban, one inspection pane, typed Needs you decisions, candidate-bound previews/evidence, and clear Test/Approve/Keep working actions. Incorporated Max's clarification that ordinary speech should create and route concurrent work. These are proposed interactions, not measured usability findings.
- At Max's request, no planner, kanban, project scheduler, or automatic merge workflow was implemented. Only the separately requested rendering bug changed application behavior.

## 2026-09-19 — Stop long-running ACP terminals
- Located the leaderboard worktree's shell → npm preview → node processes. The exit waiter held the child mutex throughout process execution, while Stop/kill/release waited for that same mutex. Terminated only the verified leftover preview processes; Max had closed Preview, and subsequent process inspection confirmed they and the old app/agent were gone.
- Replaced the shared child lock with a process-owning task that selects between exit and a cancellation signal. Kill, release, and session cancellation signal it without blocking on the waiter. Unix commands receive separate process groups, so cancellation also stops shell/npm descendants without touching another terminal's group. Output drains before completion is published, with a bounded drain wait.
- Validation: all six terminal tests passed, including cancellation during an active wait, release, multiple terminals, other-session isolation, and a live shell-descendant heartbeat. Workspace check, strict Clippy, and Preview build passed; updated the Preview bundle for the next launch. No other project sessions were intentionally stopped and no native UI access was used.
- Max subsequently authorized implementing the conversational project workflow described in docs/plan/project_run_ux.md after this fix. That implementation follows as a separate phase.

## 2026-09-19 — Conversational project workflow and reviewed parallel execution
- Implemented Max's explicit follow-up authorization after the Stop fix. Project selection now opens a native Conversation / Board / Git surface, with optional per-project enablement, normal threads still available, and Cmd+K / Cmd+Option+N entry points. Existing manual feature authoring and saved model defaults remain accessible.
- Added read-only repository planning with explicit proposal DTOs, blocking questions, feature/task dependencies, human test instructions and exact verification commands. Inline cards expose connected model and reasoning selectors for builders and reviewers; JEV suggestions use project/global/user preferences when enabled. Approve plan & Go is the dispatch boundary; Save for later parks ideas without injecting them into an existing run.
- Added durable SQLite coordination, pinned plan/task commits, bounded parallel ACP workers in isolated worktrees, committed Markdown briefs/plan indexes/task/result handoffs, and dependency scheduling. Successive plans share document history to avoid competing initial guides. Existing project instructions and user-owned tracking files are preserved; plan records first live on isolated branches.
- Combined candidates run actual bounded/cancellable command checks and independent read-only Foundry-format reviews with criterion/tool evidence. Failed checks/reviews have a bounded repair loop. Human approval is the default; automatic local merging is explicit. Landing is serialized, preserves dirty checkouts, requires the exact reviewed commit/document, and invalidates evidence when the target advances. Final assembled-project checks gate completion; saved ideas do not block finishing the active work.
- Added five-column kanban, Needs you filtering and inline provider permission cards, one result inspector with Result / Changes / Thread / candidate-bound Preview, actual check output, test instructions, Approve & merge, Keep working, and next-decision navigation. Agent threads link to the same feature result. In-app messages/notices report blockers and results. The preview's start/stop/open actions stay scoped to the selected candidate and refuse to replace another workspace's server silently.
- Pause/Stop preserve work and prevent new dispatch; reopening marks active/interrupted work for inspection without replaying prompts or merges. Project ownership guards exclude unrelated direct workspace writes while coordination owns the candidate. Explicit feedback can recover a preserved dependency conflict; unresolved or unstaged conflicts cannot become successful checkpoints.
- Validation: the workspace test suite passed, and all 14 project-workflow regressions passed after the final conflict-recovery changes. Coverage includes real Git isolation/combinations/conflicts, exact-commit landing, target advancement, dirty-root preservation, restart and saved-plan behavior, actual failed command output/cancellation, final completion checks and correlated mock ACP turns. Workspace check and strict Clippy passed; Preview rebuilt and its bundle executable replaced atomically, with matching SHA-256 hashes. The app was not launched or restarted. Native visual QA remains unavailable after the earlier Computer Use denial; no live-provider or live-JEV end-to-end run was performed. See docs/PROJECT_WORKFLOW.md for use and boundaries.

## 2026-09-19 — Project workflow UX repair, phase 1

- Added structured stage/blocker records, runtime activity projection, named dependency reasons, and explicit retry/start-backlog services.
- Kept saved ideas out of Resume; made Stop available during paused active work; preserved passing checks when retrying review of the unchanged candidate.
- Read-only policy refusals now report their cause; infrastructure failures no longer automatically consume code-repair attempts. User verification evidence remains a claim for independent review, never an automatic pass.
- Late tool output no longer rewrites persisted session activity or reopens a finished transcript turn. Review attempts retain history and human-readable thread names.
- Validation: workspace check and strict all-target Clippy passed; bomb_core tests passed (95 passed, 4 intentionally ignored live/fixture checks). Native UI validation remains for the UI phases.

## 2026-09-19 — Project workflow UX repair, phase 2

- Added the shared inline feature decision card to project results and managed threads, with revision-aware approve/retry actions, saved feedback, verification evidence, and explicit new-work navigation.
- Added Needs you grouping, matching attention counts/next-decision navigation, named dependencies, feature-first sidebar navigation, compact/narrow board layouts, an adjacent inspector, active tabs, earlier history and new-activity navigation.
- Planning now preserves proposals across questions, saves drafts across navigation/failure, supports selective start/backlog/discard and compact acceptance details, and exposes project routing/defaults. JEV recommendations no longer replace explicit assignments; users choose them in the plan.
- Added unavailable-model replacement for unfinished work, prior review attempts, named criteria, file-based diffs, portable-record explanations, actionable final-check failures and clickable project notices.
- Preview switching is explicit and serialized; reopening the same preview reuses it. Expanded tool/check output is contained and scrollable. Manual records redirect to their existing managed result rather than showing a separate tracking state.
- Validation: workspace check and strict all-target Clippy passed; bomb_core tests passed (100 passed, 4 intentionally ignored) with four test threads. An earlier unconstrained parallel run timed out in the existing workspace lifecycle test; its isolated rerun and the bounded full core run passed.

## 2026-09-19 — Project workflow UX repair, phase 3

- Completed recovery coverage for interrupted repairs: original repair scope persists across failure/restart; Retry repair resumes that scope, and Models exposes a replacement repair writer without changing finished builds. Task questions get an Answer question action; merge blockers remain visible even when approval is retained.
- Deterministic project commands work without a planning model, including while planning is busy. Older task/review/repair threads retain feature ownership and are discoverable under the feature. Preview explicitly identifies local/unreviewed context, and static preview ownership stays with the requested workspace even when serving a subdirectory.
- Added regression coverage for review retries reusing unchanged passing checks, approval-preserving merge retry after checkout cleanup, stale retry rejection, interrupted repair recovery/model replacement, Stop after Pause, command interpretation and preview reuse/failed-switch preservation.
- Added an isolated BOMB_SMOKE fixture with a temporary repository/database and a timeout, covering blocked/ready/queued features and a proposed plan. The attempted native run stalled on unavailable macOS UI services before rendering; stopped only that test process. Native visual/keyboard/accessibility QA and live-provider/JEV end-to-end verification remain uncompleted. Previous Computer Use access was not retried. No live user project was resumed, retried, approved or merged.
- Validation: complete workspace suite passed (243 passed, 4 intentionally ignored). Final workspace check and strict all-target Clippy passed; Preview build passed. After the last preview ownership change, focused dev-server checks passed (7 passed, 2 existing fixture-dependent tests ignored). Updated the audit coverage and current workflow guide.
- Updated the Preview bundle atomically; its executable matched the build SHA-256 before ad-hoc bundle signing. Corrected the bundle resource signature and verified it with codesign --verify --strict. The user's running Preview app was not restarted; reopen it to load the fixes.


## 2026-09-20 — Guided project planning and simpler multi-agent UI

- Implemented Max's approved workflow redesign from the reviewed design brief. Used Michelangelo's scoped implementation and independent review approach; this was a direct implementation authorized by Max, not a formal completed Brunelleschi/Michelangelo pipeline run.
- Preserved the existing left sidebar. New/Open project now routes into its project pane, which offers planning and an existing-provider connection action when needed. Ordinary threads remain available; no separate Bomb Code account or fictional sample application features were introduced.
- Added persistent guided planning questions, suggested responses, decisions and assumptions with backward-compatible defaults. Follow-up questions retain feature scope, rejected assumptions can be removed, and unresolved questions prevent Start/Save. The compact plan expands to full task assignments, dependencies and verification commands before explicit acceptance.
- Replaced default operational detail with compact outcome groups, clear questions/blockers, optional conversation history, and an in-place Result/Changes/Activity inspector. Preview and detailed evidence expand inside Result. Added outcomes distinguish integration from final project completion.
- Simplified the shared decision card: Approve & add identifies the local destination; paused approval is saved for later; project resume and recovery actions explain their scope. Exact rendered plan selections and feature decisions are validated before acting. Preview/diff callbacks discard results for a changed candidate.
- Independent source review identified and closed stale-click acceptance, clearing rejected planning assumptions, and first-project connection visibility issues. Follow-up review found no remaining important issue in these fixes. Sidebar files are unchanged.
- Validation: complete serial workspace suite passed (258 passed, 4 existing tests intentionally ignored), workspace check and strict all-target Clippy passed, and git diff --check passed. The first parallel suite encountered an unrelated usage-cache assertion failure; the serial suite passed it without changing cache code. Tests cover guided planning persistence/validation, unresolved-question guards, assumption removal, action policy, acknowledgements, and stale rendered decisions, alongside existing Git/approval/recovery regressions.
- Native validation limit: the isolated smoke fixture initialized successfully, but macOS UI services reported connection errors and the existing 35-second watchdog exited before rendering. Computer Use also denied access to Bomb Code Preview. No visual, keyboard/accessibility or live-provider end-to-end success is claimed.
- Built the updated app and atomically refreshed the existing workspace Preview bundle, verified the executable matched the build before signing, and passed strict ad-hoc signature verification. The running user app was not restarted; reopen Preview to load this build.


## 2026-09-20 — Merge retry blocked by unrelated planning records

- Investigated Max's live merge report read-only. The target checkout had only two untracked older feature-planning records, neither present in the approved candidate. The blanket clean-checkout requirement unnecessarily blocked this merge; Retry repeated the same check.
- Landing now permits unrelated untracked files without committing, stashing or moving them. It still blocks tracked/staged edits and lists their paths. Fast-forward integration uses --no-overwrite-ignore so Git rejects colliding ignored files as well as ordinary untracked files. Candidate cleanliness, exact review/approval, target revision and idle-workspace gates remain in force.
- All 36 project-workflow tests passed. Added real-Git coverage for preserving unrelated planning records with the exact approved HEAD, colliding untracked and ignored files, file/directory collisions and staged edits. Existing dirty-checkout and retry tests now explicitly use tracked local changes. Workspace check and strict all-target Clippy passed.
- Built and refreshed the workspace Preview bundle, verified its executable before signing, and passed strict signature verification. The live project and its files were not mutated during diagnosis. Preview must be reopened to load the fix; no live merge was performed. The current app was not terminated because persisted sessions include running/waiting-approval states.


## 2026-09-20 — simple-app branch: remove the project planning workflow

- On the new `simple-app` branch only (gpui-rewrite keeps everything), removed the project workflow surface: Project/Board/Git tabs, Attention, Next decision, Pause/Stop, feature board/editor/decision views, and the `project_work`, `features` and `feature_documents` core services.
- Root, sidebar, palette, actions, welcome, project page and smoke fixture return to their pre-workflow (95795a4) form; AppModel, thread view, AppState and workspace idle checks drop their workflow hooks. Unrelated fixes from the same window (ACP terminal cancellation, late tool updates, effort persistence, scrollable tool output, dev server repairs) are kept.
- Validation: `cargo check -p bomb_app` clean and the dev build launches. The test suite and BOMB_SMOKE run were not executed on the final state.


## 2026-09-20 — simple-app: plain-language project page, Merge to main, Close feature

- Project page is now one "finished work" card for the default branch plus "In progress" and "Already in main" lists. Each row states its relation to the default branch in plain language (`describe_relation`: saved changes not in main yet / newer updates not picked up / nothing to merge) instead of ahead/behind counts. Details expand per row; the right-hand branch panel, header badges, Fetch and "Ask a question" are gone; the PR panel only appears when PRs exist.
- Thread header replaces the Changes button with the same plain-language standing plus Merge to main (or Close feature when nothing is left to merge). Changes & history stays reachable from the branch menu.
- `merge_to_main` saves loose thread work, merges the default branch into the thread, then merges the thread into the default branch. Conflicts stay in the thread's working copy and the app sends the thread's agent a prompt to resolve them; the user presses Merge again afterwards. Untracked files in the project folder no longer block a merge; tracked edits or a non-default checkout do, with a plain explanation.
- `close_feature` saves loose work, archives the thread and its chats, and deletes the branch only when `git branch -d` confirms it is merged.
- The composer placeholder on the project page now says that sending starts a new thread.
- Validation: real-Git test covers save-before-merge, conflict hand-back, re-merge, untracked-file tolerance, tracked-edit block and merged-branch removal; relation wording test passes; `cargo check -p bomb_app` clean; dev build launched. Full suite and BOMB_SMOKE not run; the new UI has not been visually verified by me.

- Follow-up the same day, on Max's direction: the project lists became a three-column board (Working · Ready to merge · In main) whose cards show the agent model, the plain-language standing and the actions. Merging is no longer an app-side Git command: Merge to main sends the thread's own agent a request in its chat (commit, merge the default branch in, resolve conflicts, run checks, then merge into the default branch from the project folder; stop rather than stash or discard; never push). Merge & close sends the same request and closes the feature after the turn ends only when `is_merged` confirms the default branch contains the branch and nothing is unsaved; a cancelled or failed turn clears the pending close. Close stays a separate confirmed action. The earlier app-side `merge_to_main` core function was removed; the real-Git test now covers the request text, `is_merged` for unsaved/unmerged/merged work and merged-branch removal on close. Not visually verified by me; no live agent merge was run.

- UI consistency pass (simple-app): added one type scale in `theme.rs` (`Type::CAPTION 12 · SMALL 13 · BODY 14 · TITLE 17 · DISPLAY 24`; transcript and composer text 15/24) and migrated every ad hoc `text_size(px(..))` and `text_xs` in the views onto it, so nothing renders below 12px. Fixed line heights and sidebar row heights were scaled to match; header 48, sidebar 272. All buttons now come from `views::button::Button`, which adds a pointer cursor and a 28px minimum hit area to the kit button; the collapsible tool-group header also gets a pointer. The composer stacks a full-width, three-line-minimum message area above its controls and focuses on click anywhere in that area. Board column "Ready to merge" is now "In progress". Smart Model Routing uses the Route icon instead of the Git branch icon. Compile-checked only: screen capture from the agent shell has no Screen Recording permission, so none of this was visually verified.

- Root cause of the "too small and not cohesive" text, found from Max's screenshot: `themes/bomb.json` set `font.size` 13, which the kit uses as the window rem size, so every kit button, input and `text_sm` label rendered at about 11px next to explicit 13–14px labels. Base size is now 16 (mono 13.5): kit small controls are 14 and captions 12, matching `Type`. All `xsmall` buttons became `small`, composer footer labels match the footer buttons (14, 28px tall), the composer textarea sets 15/24 text explicitly (same as the transcript), and the usage-bar label no longer wraps "Weekly". Still compile-checked only by me; Max is verifying by eye.

- Composer alignment (simple-app): the context labels and work-location controls moved from under the pill into a context bar attached above it; inside the pill the message sits on top and one control row sits below with input tools on the left (attach, approval mode, MCP, Enhance, Review loop) and the model, routing, speed and send on the right. "Enhance Prompt" and "Run with review loop" were shortened, with tooltips. The Git-not-detected panel became a centered hero with one primary action in plain language. Compile-checked; Max verifies by eye.

- Thread header decluttered to a single non-wrapping row: agent mark, then one status dropdown whose label is the short standing (`describe_relation_short`, e.g. "8 unsaved edits · 1 change to merge") with the full sentence, branch names, remote state, changes & history and the working folder inside its menu; on the right one primary action (Merge to main, or Close feature when nothing is left to merge) and icon-only New chat, Terminal, Dev sidebar and More with tooltips and accessible names. Merge & close and Close without merging moved into the More menu. Wording test extended. Compile-checked; Max verifies by eye.

- New projects default to `~/Documents/BombCode` (created on demand) instead of opening the save dialog in the home folder; the unused core `create_project_folder` default follows the same `default_projects_dir`. Adding the home folder itself, or a filesystem root, as a project is refused with a plain explanation. Existing projects are untouched. Compile-checked; not exercised in the running app by me.

- Sidebar sorting and pinning: a sort menu (filter icon beside search) orders projects and the threads inside them by Most recent (default; a project is as recent as its most recent thread), Time created, or Alphabetical; rows with no activity go last. Projects can be pinned from their header and appear in a Pinned section above Projects. Both choices persist in the kv store (`sidebar_sort`, `pinned_projects`). Ordering is a pure function with a unit test. Compile-checked; Max verifies the UI by eye.

- Sidebar projects show only their three most recently changed open threads; the rest sit behind "View N more threads" / "Show fewer threads". The thread the user is on always stays visible, and the visible rows keep the chosen sort order. Archived shelves are unchanged. The selection rule is a pure function with a unit test. Compile-checked; Max verifies the UI by eye.


## 2026-09-21 — Quieter threads, sidebar, project page and Settings (gpui-rewrite)

- Permission requests: a waiting request keeps its full card. Once answered it becomes one line (shield, Allowed / Always allowed / Denied / Cancelled, the command, a chevron for the rest); answered requests next to each other fold into "3 requests allowed". On reopening a thread, the saved "approval granted/cancelled" line is attached to its request instead of showing as its own row (`saved_approval_outcome`, `approval_outcome_label`). Each command shows once: a JSON echo of it is dropped, also when the agent cut the summary short. A waiting request still shows everything it would approve: the full text of an edit, and any extra scope such as a working folder.
- Tool calls: groups stay closed whether running or finished. A running group's header shows a breathing dot, what it is doing now ("Running `npm test`", "Editing src/app.ts") and the step count; a group with a failed step opens itself. A lone unnamed step reads "1 other tool call" instead of the bare word "tool".
- Sidebar: only the project in view is open by default; the chevron overrides either way, and arriving in a project opens it. A closed project shows an amber dot when a thread is waiting for permission and an accent dot when one is working. The "main · uncommitted" line under project names is gone.
- Project page: the explanation on the finished-work card has a "Got it" that is remembered, leaving one line. "In main" shows five cards with "Show all", and has a confirmed "Clean up" backed by `workspaces::cleanup_merged`, which closes idle threads and deletes branches only when the default branch already contains them and they are not checked out.
- Settings: one list pattern (`list_row`, `row_line`, `row_title`, `row_meta`, `tag`, `empty_list`, `toolbar`, `add_row`) replaces bordered cards and bare rows in tool servers, keys, notes, thread folders and app facts; every group has a title and a one-line description in plain words. "MCP" is "Tools (MCP)". Worktrees and Diagnostics became "Advanced" (Thread folders, This app, Connection log).
- Validation: new tests for saved outcomes and labels, the command-once rules (including a truncated summary and a waiting edit), running-step wording and the unnamed-step label, and a real-Git clean-up test (merged removed, unmerged and checked-out kept, second run a no-op). Serial suites: bomb_app 20, bomb_core 73 (+4 ignored), all passing. Not seen on screen by me.

- Follow-up from Max's screenshot. Overlapping words beside inline code: the kit lays a line out as boxes sized to the shaped text, then wraps each box again by summing single-character widths; with kerning the sum is slightly wider, so a word drops a line and lands on other text. Chat prose now turns off `kern`, `liga` and `calt` so both measurements agree (a workaround; the kit is a registry crate). Code colors: the theme file has no syntax section and the kit fell back to its light palette in both modes; `apply_mode` now picks the dark or light highlight theme. Every code block in chats (messages, tool output, diffs) has a copy button. The sidebar hides itself below 860px of window width and returns when there is room; opening it by hand while narrow is respected until the window is wide again. Compile-checked and tests passing; the overlap fix and colors need Max's eyes.

- Attachments accept any file. Images still go to the agent as images. Anything else is attached by reference: the composer shows a named chip, and on send the message gains "Attached file(s) (open from these paths)" with each absolute path and size, because the agent runs on this machine and can open the file itself. No size or type limit applies to references; folders are refused; duplicates are ignored; the 8-attachment cap covers both kinds. Picker, drag-and-drop and the send path all share `attach_path`. Test covers the note's wording. Reading a path outside the thread folder may prompt for permission, depending on approval mode. On the `server-projects` branch this will need a different route, since a Mac path means nothing to an agent on a server.

- Thread folders moved into the project. New threads get their working copy at `<project>/.bombcode/threads/<name>-<id>` instead of `~/.grok/worktrees` (a name that made no sense for Claude or Codex threads, and a place nobody looked). The folder is ignored through the clone's own `.git/info/exclude`, so the project's status stays clean and its `.gitignore` is untouched; the rule is added once. `WorktreeManager::is_managed` recognises both the new location and the old shared root, and thread deletion uses it. Existing threads stay where they are; `outside_projects()` keeps the old layout available. Real-Git test covers location, clean status, no duplicate rule, a dirty thread not dirtying the project, removal, and the old layout.
- Files: a file link in a reply, a file chip and a file in the dev sidebar's tree now show the file in Finder (`open -R`); folders in the tree still expand in place, and generated images still open. Files copied in Finder can be pasted into the composer (previously only pasted images were taken).
- Attachments in sent messages: the agent still receives the paths, but the bubble shows what was typed plus a chip per file (icon, name, size; hover for the full path, click to show in Finder). `split_attached_files` only treats a well-formed note at the very end of a message as attachments, so quoting the phrase or a malformed list stays as text; this also tidies messages already in history. Tests cover paths with spaces and parentheses, files-only messages and the non-matches. Serial suites passing: bomb_app 22, bomb_core 73 (+4 ignored), grok_worktree 6. Not seen on screen by me.

- Pasting a screenshot from CleanShot typed its path into the composer: that app puts the file on the clipboard as a plain-text path, which the paste handler ignored. A pasted string is now an attachment when every line of it is an absolute path or `file://` URL of an existing file (up to 8); prose that merely contains a path, a missing file, a folder or a relative path is still pasted as text. Images attach as thumbnails, other files as chips. Test covers a name with spaces, several paths, a percent-encoded file URL and the non-matches.

- Paste never reached the attachment code: the text box binds ⌘V to its own `Paste` action, and actions are dispatched before the composer's key listener, so pasted images and files were always handled (or ignored) by the box. The composer now takes attachments in a `capture_action::<Paste>` handler and stops the action only when it attached something; plain text still goes to the box. This is why a copied image did nothing and a CleanShot path was typed. Compile-checked; the paste itself needs Max to try, since the agent shell cannot drive the window.


## 2026-09-21 — Text overlap fixed at the source; transcript virtualised

- Overlap beside inline code: the cause is in `gpui-base` 0.6.1. A paragraph with a styled span is laid out as line fragments whose boxes are sized by shaping, then each fragment is drawn by a text element that wraps using per-character advances; when the two disagree by a fraction of a pixel the last word drops onto the next line. `vendor/gpui-base` is a copy of the crate with one change (`whitespace_nowrap` on each fragment's element: a fragment is already exactly one line) wired in through `[patch.crates-io]`; `vendor/gpui-base/BOMB_PATCH.md` explains it. The earlier kerning workaround did nothing and was removed.
- Typing lag: a 3 s sample of the app during a turn showed the main thread re-laying out the whole thread every animation frame (text wrapping, selection geometry, flex layout), so frame cost grew with thread length and keystrokes waited behind frames. The transcript now renders through gpui's `list`: only visible rows are laid out, heights are remembered, new rows are spliced in, the last three rows are re-measured when the tail changes, and the list's own Tail follow mode replaces the hand-rolled pinning (scroll up to stop following, back to the bottom to resume). Find-in-thread pauses following and reveals the row. Rows that were already present when the thread opened no longer fade in, since rows re-enter the screen while scrolling. The breathing dot and other animations stay.
- Idle CPU after launch: 0.3%. Not yet measured during a turn on a long thread; that is the case to check. Serial `bomb_app` tests: 23 passing.
## 2026-09-21 — Server-first projects, phase 0: merge targets the branch a thread started from

- Plan approved by Max: `~/.claude/plans/ok-great-now-what-valiant-blanket.md` (threads live where their project lives; the server is the default project home; offline work is a Git clone plus sync; no thread handoff). Work is on branch `server-projects`.
- `workspaces::merge_target` resolves a thread's target from `WorkspaceRecord.base_ref` (local name, migrated `origin/<name>`, or unusable values such as `HEAD`, empty, deleted or the thread's own branch, which fall back to the default branch). Standing (`review_workspace().base`), `merge_request`, `is_merged`, `close_feature`, "Bring in the latest", the legacy Changes merge and squash all use it. Push-to-origin and PR flows still use the repository default branch.
- The merge request now sends the agent to wherever the target branch is checked out (project folder or another thread's folder, refusing while that thread is busy) or, when it is checked out nowhere, to fast-forward it with `git fetch . <branch>:<target>` after merging the target in. Close deletes the branch only after `merge-base --is-ancestor` confirms the target contains it (`-D`, because `-d` compares with whatever the project folder has checked out).
- The project board compares each thread's branch with its own base (`project_overview::load_for_project`, `Branch.base`); header, board and Changes labels say "Merge to <that branch>" and "Started from".
- Validation: new real-Git tests cover target resolution, both landing instructions, merged-into-wrong-branch, and close with the project folder on another branch. Serial `cargo test -p bomb_core -p bomb_app`: 91 passed, 4 ignored. `cargo check -p bomb_app` clean. Not exercised in the running app.


## 2026-09-21 — Server-first projects, phase 1: headless core over a local socket

- New `bomb_core::journal`: the single event-bus subscriber. It saves history (moved out of `bomb_app/src/runtime.rs`, together with the Foundry helper-session filtering), numbers events under one lock, keeps a 10k backlog, tracks open approvals with their request ids and options, and fans out to clients. The desktop UI attaches as a never-dropped local client; remote clients have a bounded queue and are cut off with a resync rather than losing events silently. `snapshot` returns rows, open approvals and an `as_of` sequence that is consistent with them.
- New `bomb_core::rpc::dispatch`: a curated method table over existing `services::*` (threads, prompts, approvals, modes, projects, workspaces, standing, merge request, close, backends). No settings, MCP, memory or raw key-value access. Prompts sent through it emit the new `ControlEvent::UserMessage { origin }` so other devices on a thread see them; an already-answered approval is reported as such, not as an error.
- New crates: `bomb_proto` (length-delimited frames: JSON messages and numbered binary chunks, 8 MB frame cap, plus a client with request correlation) and `bomb_server` (`bombd core --socket <path>`, socket mode 0660, SIGTERM/Ctrl-C run `shutdown_all`). The app's Quit now also runs `shutdown_all` with a 3 s cap. `AppState::initialize_with_paths` is public. `ProjectOverview`, `Branch`, `PullRequest`, `ProjectStatus` and `ImageInput` gained serde derives.
- `.github/workflows/linux-core.yml` builds `bombd` and runs the proto, core and server tests on Ubuntu. It has not run yet; Docker was not available locally to try a Linux build.
- Validation: `bomb_server/tests/core_socket.rs` drives a core with a temp home through its socket with no UI: history is saved, sequence numbers are gapless, a dropped client resumes from its cursor and gets exactly the missed event, an unknown cursor demands a resync, a second client sees the first one's prompt, credentials are unreachable, and the socket is removed on shutdown; a second test overflows a slow client. Serial suites: bomb_app 18, bomb_core 73 (+4 ignored), bomb_proto 2, bomb_server 2, all passing. `cargo check -p bomb_app` clean. The desktop app was not launched on this build.
- Known and unchanged: events a session emits before its row is first saved are not written to history (the mock session shows this); this predates the journal.
- `crates/bomb_core/src/state.rs` also carries an import reordering that was already uncommitted in the checkout before this work.


## 2026-09-21 — Server-first projects, phase 2: the gateway

- New crate `bomb_link`: self-signed device and server identities (rcgen), TLS 1.3 only with client certificates (rustls + ring). The Mac pins the server's certificate fingerprint from the pairing link; the server lets any device finish the handshake only if it proves possession of its key, then decides what it may do from its fingerprint. Session resumption and tickets are off so a revoked device cannot return on a cached session. Pairing links are `bomb://pair?h=<host:port>&fp=<sha256>&s=<128-bit secret>`.
- `bomb_server::gateway`: one port. Unknown devices may only call `gateway.pair`; links are single use, expire after 10 minutes, are stored as SHA-256 hashes compared in constant time, and wrong guesses are throttled per address (5/min) and globally (50/min). Known devices either ask the gateway (`whoami`, `list_devices`, `revoke_device`, `create_invite`, admin-only `list_users`) or send `Hello` and are relayed frame by frame to their person's Unix socket. Revoking a device also cuts its open connections. 10 s handshake window, 15 min idle limit, 8 MB frame cap. The registry is one JSON file rewritten atomically under a file lock, so the `bombd` command line and the running gateway can both change it.
- `bombd` commands: `up` (one person: core + gateway in one process, prints a pairing link on first run), `core`, `gateway`, `add-user`, `invite`.
- Fixed in `bomb_proto::client`: a request made after the connection had ended waited forever; it now fails immediately.
- Validation on loopback: pinned server accepted and an impostor identity refused; pairing works once and never again; the secret is not in the registry file; an unpaired device can neither reach a core nor list devices; a paired device gets a normal core session through the gateway; revocation cuts the live session and later connections; one person cannot see, remove or invite for another; a person whose core is down never reaches someone else's; guessing is throttled even for a valid link; expired links, unknown users and raw junk are refused while the gateway keeps serving. bomb_link 2, bomb_proto 2, bomb_server 5 tests passing. Not run on a real server yet.
