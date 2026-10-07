# Routines and workflows — plan

Status: proposal, not started (2026-10-07). Parked until Paul's related pieces land; agree the
workflow-engine base with him before building.

- **Routines** decide *when* something runs: a schedule, a project, an agent, an access level.
- **Workflows** decide *how*: a saved recipe of steps, each possibly on a different model, with
  handoffs and checkpoints between them.
- A routine triggers either a single prompt or a workflow.

---

## 1. What exists today

### Scheduler (both codebases, same core)
- `crates/grok_scheduler` (~470 lines): jobs with name, prompt, folder, schedule
  (`Interval{secs}` · `Cron{expr}` in **UTC** · `Once{delay_secs}`), optional `max_runs`,
  pause/cancel, last/next run, run count.
- Persisted in kv `scheduler_jobs` and reloaded at startup (`bomb_core/src/state.rs:205-222`).
  Empty on Max's Mac: nothing has ever been scheduled.
- Firing (`state.rs:165-205`) spawns a **headless one-shot via the `grok` CLI wrapper**, always
  `plan_mode: true`. It skips jobs without a folder and records the run id or the error in kv.
- Service functions `scheduler_list/add/cancel/pause` (`bomb_core/src/services/mod.rs:2090-2140`).
  Paul's Tauri app registers the same four commands.

Gaps:
- No UI in either codebase.
- Not in the app↔server API (`bomb_core/src/rpc.rs`).
- Grok only.
- Read-only.
- Results aren't surfaced.
- Cron is UTC, not the user's time zone.

### Workflow engines (two, overlapping)

**Paul's reviewed builds — `grok_workflows`, only on `origin/main`**
- Fixed roles: Planner → plan approval → Implementer → Auditor → Verifier. Each role has its own
  backend/model (`RoleRoutes`).
- Spec: project root, objective, a **write set** (1–256 allowed paths), `max_repairs` ≤ 10.
- Plan approval is bound to a **digest of task + plan + checkout**. If any of them changes,
  the approval goes stale (`StaleApproval`).
- States run from Planning to ReadyForReview, then Accepted / NeedsChanges. Stalled, Cancelled,
  Failed and Interrupted are also states, and a restart marks a run Interrupted rather than
  resuming it silently.
- `coordination.rs` handles several builds at once:
  - a dependency graph;
  - refusing tasks whose **write sets overlap**;
  - a parallelism limit;
  - reservations the host persists before spawning.
- The crate holds only the state machine: "the host owns execution… this crate only makes
  transitions explicit". That makes it portable into the rewrite.

**This rewrite's Foundry — `bomb_foundry`, `bomb_core/src/foundry.rs`, `views/foundry.rs`**
- A flexible `SkillGraph` of steps of three kinds: **stage / review / gate**. Each step has
  a prompt and exit criteria, and optionally its own backend/model (`StageBinding`).
- Each attempt is a fresh child ACP session with no inherited chat.
- Handoff is structured: a step ends with `<foundry-result>` JSON (outcome, summary, artifact
  paths, criteria evidence), which is passed on as an "Explicit handoff".
- Has return edges (loops), human gates, templates and a UI.

### Other relevant pieces
- **Server projects** (`bombd`, branch `server-projects` = `main`): the server runs the same core,
  so its scheduler already ticks while the Mac sleeps.
  - Projects are homed "On my server" or "On this Mac".
  - A Mac project can have a linked server copy (send-to-server, Sync via git bundles).
- **Unseen marker** (sidebar green dot) and the planned notifications / menu bar icon: how
  results reach the user.
- **Agent-calls-agent tools** (`generate_image`, `ask_model` exposed over a built-in MCP server):
  separate idea, waiting on Paul's work. It complements workflows; it doesn't replace them.

---

## 2. Routines

### Where a routine runs
Threads live where their project lives; routines follow the same rule.

| Project | "Run on" | Behaviour |
|---|---|---|
| On the server | server | Runs on the server. |
| Mac only | this Mac | Runs only while the Mac is awake. Each routine has a missed-run rule: *run when I wake* or *skip*. |
| Mac only | server | Bomb Code sets up a linked server copy first (existing send-to-server), and the routine runs against that copy. |

Keeping a server copy current while the laptop is closed:
- Before each run, the server pulls from the project's git remote when there is one.
- Otherwise it runs on the last synced state, and the run says so ("Ran on the version synced
  Oct 6, 6pm").
- Nothing is pushed back automatically; results come back through Sync when the Mac wakes.

### What a run is
- A **normal thread** started by the scheduler, not a headless one-shot.
  - Any backend/model (Claude, Codex, Grok) through the same ACP sessions as interactive threads.
  - Labelled e.g. "Routine · Nightly tests · Oct 8".
  - Listed under its project in a folded "Routine runs" group.
  - Gets the unseen dot (and later a notification).
- **Access levels:**

| Level | Can do | Where work goes |
|---|---|---|
| Report (default) | Read, read-only commands, write a summary | Nothing changes |
| Make changes | Edit and run commands, auto-approved within deny rules | An isolated branch `routine/<slug>/<date>`, never the main checkout |

- **Unattended approvals:** nobody is there to approve during a scheduled run.
  - "Make changes" is an explicit per-routine opt-in to auto-approval, confined to the routine's
    own branch. Deny rules still win, so this stays within AGENTS.md ("never default to
    always-approve").
  - Anything outside that scope parks the run as *waiting for you*; it never guesses.
- **Outcome**, one of:
  - *No changes needed*
  - *Changes ready: N files on `routine/…` · Review · Merge* (existing review/merge flow)
  - *Failed / needs you*

  An optional GitHub PR comes later.

### UI
- **Sidebar:** a top-level "Routines" entry next to Home, covering all routines across projects
  and servers. Each shows next run and last result, with Run now / Pause.
- **Project page:** a Routines section for that project.
- **New routine sheet** (one screen):
  - **What:** a template (*Nightly tests & fix*, *Dependency updates*, *Morning summary*,
    *Custom*) plus the prompt, or a workflow.
  - **When:** presets ("Every weekday at 9am", "Nightly at 2am", "Hourly") or a custom
    schedule, always in the user's time zone.
  - **Where:** the project, plus *Run on: this Mac / <server>* ("Runs while your laptop is
    closed").
  - **Agent:** backend and model.
  - **Access:** Report / Make changes.

---

## 3. Workflows

### Direction: one engine
- Use Foundry's flexible step graph and structured handoffs as the base.
- Adopt from Paul's reviewed builds:
  - approvals bound to a digest;
  - declared write sets and overlap refusal (coordination);
  - Interrupted-not-resumed on restart;
  - the repair-loop cap.
- Ship Planner → Implementer → Auditor → Verifier as a **built-in template**, not a parallel
  system.
- Decide with Paul which codebase's engine is the base before writing code.

### UX
- **A workflow is a saved recipe** (per project or global). Examples:
  - *Design from an image:* Astra generates a reference image → Opus builds the page →
    Fable reviews the visuals.
  - *Reviewed build:* plan → approve → build → audit → verify.
  - *Bug hunt:* reproduce → fix → test.
- **Started from:**
  - the composer ("Run workflow ▾", or `/design-from-image …`);
  - a routine;
  - later, a single sentence that Bomb Code turns into proposed steps for the user to confirm.
- **Run view:**
  - a step strip in the thread (Plan ✓ → Build ● → Audit ○ → Verify ○);
  - each step is its own thread;
  - handoffs show as files and summaries;
  - gates show as Approve / Request changes cards.
- **Unattended runs** (from a routine) pause at human gates and wait.
  - A "Make changes" routine may skip the plan gate, never the final merge.

Limit: each step is a fresh session that gets only the handoff. That's deliberate, for clean
reviews, but a long-running session can't call another model mid-task. That's the
agent-as-tool idea in §1.

---

## 4. Build order

1. **Core routines:**
   - Extend jobs with project, server, backend/model, access, time zone and missed-run rule.
   - Runs spawn real ACP sessions in the right workspace: read-only inline, or an isolated
     routine branch.
   - Add a concurrency limit.
   - Replace the Grok-only headless path.
2. **Server API:** routine calls in `rpc.rs`, so the Mac lists and manages server routines
   (combined like `server_lists`).
3. **UI:** Routines list, New routine sheet, project section, "Routine runs" grouping.
4. **Mac projects on the server:**
   - set-up-on-server step;
   - git pull before each run;
   - results back through Sync;
   - "ran on version from …" note.
5. **Workflow engine merge** (after agreeing the base with Paul), the reviewed-build template, and
   the workflow-as-routine trigger.
6. **Later:**
   - notifications and a menu bar icon (needs `.app` packaging; GPUI logs "system
     notifications disabled: not running from an app bundle");
   - GitHub PRs;
   - sentence → workflow proposals.

## 5. Risks

- **Agent logins on the server.** Codex on a headless box may need its device-code login;
  Claude can use an API key.
- **Auto-approval in unattended runs** is the main safety surface. Keep it confined to an
  isolated branch with deny rules enforced, and audit-log every auto-approved call.
- **Server copy drift** for Mac projects without a git remote.

## 6. Open questions

1. A Mac project with no server copy and "Run on server": create the copy automatically, or ask?
2. "Make changes" results: review/merge in Bomb Code only, or also open a GitHub PR when there's a
   remote?
3. Sidebar: top-level "Routines" entry, per-project only, or both?
4. Which engine is the workflow base: Foundry or `grok_workflows`?
