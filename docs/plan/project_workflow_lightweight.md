# Lightweight project workflow for Bomb Code

Proposal for Max, 2026-09-19. Refines the earlier parallel-features plan; no application changes or project migrations are authorized by this document.

## Direction

Keep every repository understandable through a small, consistent set of Markdown records. Capture an idea, start useful work immediately, run independent tasks in isolated worktrees, and review their combined result before merging. Preparation should expand only when the work needs it.

An API contract is not a prerequisite for visual exploration. Dependencies belong to the specific work that consumes them: UI layout can use fixtures now; real API wiring needs the interface later. The initial proposal's contract-first example was too restrictive as a default.

## References inspected

- [Multi-Agent Max README](https://github.com/Transformation-Agency/multi-agent-max/blob/ee7ab8fdc28e0ff194b7ec3ab650258f8e8df791/README.md), [protocol](https://github.com/Transformation-Agency/multi-agent-max/blob/ee7ab8fdc28e0ff194b7ec3ab650258f8e8df791/PROTOCOL.md), [ticket template](https://github.com/Transformation-Agency/multi-agent-max/blob/ee7ab8fdc28e0ff194b7ec3ab650258f8e8df791/tickets/TEMPLATE.md), and planner/state documents. Revision ee7ab8f.
- [Prompt Foundry README](https://github.com/jedisherpa/prompt-foundry/blob/9cb4e5e6c14d13b1e1ce14db6ec60ef683c9a9b3/README.md), [current contract schema](https://github.com/jedisherpa/prompt-foundry/blob/9cb4e5e6c14d13b1e1ce14db6ec60ef683c9a9b3/src/lib/schema/project-contract.ts), [template loops](https://github.com/jedisherpa/prompt-foundry/blob/9cb4e5e6c14d13b1e1ce14db6ec60ef683c9a9b3/docs/TEMPLATE_LOOPS.md), compiler and graph projection. Revision 9cb4e5e.
- Bomb Code's current Foundry docs, contract/intake types, stage graph, and composer integration.

Multi-Agent Max already recommends a minimal scaffold, local ticket checks, combined milestone checks, cached dependencies, and clean release verification only at release. Its scripts/prompts do not themselves schedule agents. Keep these efficiency improvements; do not copy its full role/stage/ledger system into the default experience.

Prompt Foundry provides a typed brief, Fast Draft versus Full Project, scope/verification boundaries, and explicit review/revision links. Its upstream graph canvas describes/exports workflows; it does not execute them. Bomb Code already adds sequential execution and actual approval handling.

Compatibility finding: upstream docs/SCHEMA.md describes an older vocabulary, while current src/lib/schema/project-contract.ts has different target agents, operating modes, source roles, assumption fields, and required fields. Both still report 1.0.0. Bomb Code uses the older vocabulary. Import/export must inspect supported structure and record its source format/revision, with explicit conversion or an actionable incompatibility message. Never treat matching version strings as proof of compatibility, or map an upstream autonomy label into broader tool permissions.

## Keep, simplify, defer

| Source concept | Lightweight treatment |
|---|---|
| SPEC + ARCHITECTURE | One short PROJECT.md with purpose, map, commands, conventions, and links to existing docs |
| Tickets and acceptance criteria | One feature brief; separate task briefs only when there is real parallel work or a handoff |
| WHITEBOARD | Generated STATUS.md snapshot plus the live project board in the app |
| Decisions, debt, lessons | Relevant bullets in the affected feature; promote a durable cross-project lesson into PROJECT.md only when earned |
| Planner / Worker / Verifier | User-approved task assignment; worker by default; independent reviewer at a useful boundary, without mandatory planner/reviewer calls per prompt |
| Ticket / milestone / release verification | Task check / combined feature check / release check; use project-configured commands and existing CI |
| Foundry contract | A short useful work brief; full contract remains an advanced option |
| Skill graph | Hidden execution detail for simple review loops; advanced canvas remains optional |
| Human approval | Keep actual approval actions; a document or generated gate is never approval by itself |

Do not introduce mandatory five-stage project progression, separate append-only Markdown ledgers for every action, a permanent planner agent, a full-suite run after every small patch, or infrastructure setup before the first useful feature. This does not remove meaningful checks for auth, data integrity, or other consequential behavior as it is built.

## Shared repository shape

Adopt this shape across projects without requiring identical application source folders:

```text
AGENTS.md                    existing rules + short pointer to project records
plan/
  PROJECT.md                 stable project map and working conventions
  STATUS.md                  generated, timestamped checkpoint summary
  features/
    F-012-leaderboard.md      outcome, tasks, assumptions, decisions, evidence
  tasks/                     created only when tasks need independent ownership
    F-012-ui.md
    F-012-api.md
```

Only PROJECT.md is needed at adoption; STATUS.md is generated from actual records, and feature/task files appear on demand. Preserve existing AGENTS.md, CLAUDE.md, specs, tests, and architecture docs. Link them instead of duplicating them. Installation shows a diff and never overwrites unrelated files. If a repository already has plan/ content, adopt alongside it or map existing paths explicitly.

PROJECT.md stays short: product purpose; source-directory map; run/check commands; default target branch; shared interfaces and existing docs; project constraints; model role preferences if desired. Machine/provider-specific paths, credentials, and live session handles stay out of committed documents.

A feature brief is one page when possible:

```markdown
---
format: bomb-feature/1
id: F-012
title: Leaderboard
status: in-progress
---
# Outcome
Players can compare their best scores.

# Done when
- Scores persist and sort correctly.
- Loading, empty, and error states work.
- The UI works with the real score API.

# Work
- UI: Fable; layout and interactions with sample data. Ready now.
- API: Astra; storage and score endpoint. Ready now.
- Wiring: connect the UI to the API. Waits for the agreed response shape.

# Assumptions / open decisions
- Mock rows have rank, player name, score. Provisional presentation data.
- Weekly versus all-time ranking needs a product decision before real scoring ships.

# Scope
Include score entry, list, and states. Exclude accounts and public sharing.

# Checks / handoff
Record the tested commit, relevant checks, preview evidence, and remaining work.
```

Each independently owned task adds only outcome, expected edit area, dependencies, assignment, focused check, and short handoff/evidence. Do not make a model copy the complete project plan into every task.

## Documents and live state without conflicting copies

Repository documents are the portable record of intent, assumptions, decisions, and checkpointed progress. SQLite keeps live execution: process IDs, queues, retries, worktree ownership, timestamps, detailed tool events, and runtime evidence. The app combines them into a live board; another checkout can understand the project without possessing the original machine's database.

Define field ownership rather than two competing sources of truth:

- Markdown owns the feature brief, acceptance criteria, dependencies, and durable decisions. The app edits those fields through file updates; its DB holds a revision/hash cache, not a competing brief.
- SQLite owns current execution status and attempt records. An agent-authored status line cannot mark a task integrated or grant approval.
- STATUS.md is a snapshot at a named timestamp/revision, never a promise that its processes are currently running. Refresh it at review/integration/checkpoint boundaries, not on every tool call.
- Each task worker writes only its own task document alongside its code checkpoint. Workers propose changes to shared feature/project documents; one app-controlled writer applies those changes in the designated feature integration workspace. For a single-task feature that workspace is simply its task worktree.
- Shared snapshots are regenerated at integration or explicit Save status in the chosen clean checkout, not background-written into the user's main working copy. Repo-wide STATUS.md is generated from records present on that branch, so unmerged tasks from another worktree are not falsely presented as landed.
- The live app can show work across all task branches. On another machine, importing docs restores planned/checkpointed work; active runtime claims become unconfirmed until reconciled. Export can include portable task summaries when sharing work before merge.
- External edits trigger a content-hash comparison and review/re-import. Never silently overwrite an edited brief. Changed briefs flag affected task assumptions/checks, rather than restarting every agent automatically.

This avoids multiple agents racing to rewrite one WHITEBOARD while keeping useful project records in Git. Raw logs/screenshots can remain local with a small committed summary; portable evidence must use repository-relative files or accessible links and identify unavailable local evidence honestly.

## Fast start and dependencies

Offer two task relationships:

- Can start now: enough information exists for a useful, reversible result.
- Waits for: a specific artifact/decision is genuinely required for the next action.

Example, all at once:

- Fable explores leaderboard composition using local mock data behind a small data adapter.
- Astra implements storage and proposes the endpoint/response shape.
- A separate task can improve an unrelated settings page in another worktree.

Once the response shape is agreed, wire the leaderboard UI to it and verify the combined flow. UI exploration did not need to wait; integrated behavior still needs verification. If UI requirements reveal an API need, capture that as a decision/request rather than allowing both workers to redefine the shared API independently.

Do not pretend mocks prove backend behavior. Show UI with mock data, API checked, and end-to-end checked as distinct evidence. Avoid mandatory extra task cards for tiny steps: API wiring can be a follow-up in the UI task once its dependency becomes available.

## Foundry in the everyday flow

The default flow is: capture request → short feature/task brief → Start. Creating a feature has no mandatory model call or intake interview; use a local template and existing project context. Ask only for decisions that block the intended work. Let Max mark ideas as just ideas without allocating a worktree.

Enhance Prompt remains optional. At a feature it improves outcome, scope, assumptions, checks, and a proposed split. At a task it improves only that task's brief. It should produce an editable update in place, preserve the original, and allow Undo. It should not create an unrelated second mega-prompt or silently launch work.

Run with review loop should have a short default:

1. Build and run focused checks.
2. Independent review of the requested result at an identified revision.
3. Fix concrete findings and recheck the changed behavior if needed.
4. Present the result and unresolved limitations.

No separate Plan stage by default when a usable brief already exists. Existing Full Project / graph templates can opt into more stages. Start with one automatic repair return, then stop for a decision if it still fails; expose retry limits. Scope/taste changes return to Max, not an endless debate between models.

For multiple task branches, ordinary task checks happen locally; one feature review examines the combined candidate. Higher-risk tasks can receive earlier independent review. Reviewers receive criteria, code/diff, and relevant evidence, not the author's private reasoning. A new review context does not require reinstalling dependencies or repeating unchanged valid checks.

Model choice and review depth remain separate. JEV can recommend a connected model; it does not schedule work, choose permissions, or certify correctness. Accepted task assignments are stable unless changed deliberately. Use the existing provider/effort and actual-applied-value handling.

## Verification that stays fast

During editing, run the smallest meaningful check. At task submission, record what ran and what remains untested. At feature integration, validate the real combined flow and necessary broader checks. Before release, use the project's release checks and human acceptance.

Preserve caches and dependency installations. Reuse evidence only for matching code, relevant config/dependencies/environment. Changed target/inputs invalidate integration readiness. Shared-folder writes, preview ports, and test databases still need isolation; starting work early does not relax those boundaries.

A page with mocks can be Ready for visual review while its feature is In progress. A worker saying finished is not feature acceptance. Merging is not deployment. Keep these distinctions visible without requiring separate process documents.

## Build order

1. Project records + feature capture + attach existing threads. Build the small Markdown format/importer and a single record-writing service first. Existing repositories remain untouched until adoption is requested.
2. Start feature tasks in isolated worktrees with accepted model/effort assignments. Allow independent tasks immediately; add only explicit waiting dependencies. Show one board with attention, progress, and checkpoint evidence.
3. Connect Enhance Prompt to editing those briefs and add the short optional review loop. Keep old Foundry envelopes supported; use an explicit adapter for the current upstream shape.
4. Add feature integration/preview and revision-bound checks; record final progress snapshots. Keep the existing per-worktree writer guard and serialize landing per target branch.
5. Later, let an approved feature automatically launch ready tasks within configured concurrency/retry limits. Reuse the same documents and services; automation does not need another planning framework.

The first product slice is a feature inbox and standardized briefs linked to the threads/worktrees already present. The proposed full workflow still needs integration support; tracking alone should not be described as a safe automatic multi-agent runtime.
