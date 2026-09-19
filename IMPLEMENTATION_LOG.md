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
