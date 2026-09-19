# Project features and parallel model work

Status: proposed plan for Max's review, 2026-09-19. This document does not authorize implementation, automatic task execution, or merging.

Updated direction: [Lightweight project workflow](project_workflow_lightweight.md) refines this proposal using Multi-Agent Max and current Prompt Foundry. In particular, it replaces contract-first startup with immediate independent work using explicit provisional assumptions, and defines portable repository documents alongside local runtime state.

## Product outcome

Capture ideas against a repository without interrupting active work. Turn an idea into a feature, split it into bounded tasks when useful, and run independent tasks concurrently with the same or different models. Show what needs attention and what is ready to integrate. Keep final integration understandable and reviewable.

The central unit is the feature. Threads are conversations supporting work; models are assignments that can change. Neither should be the user's only way to find or track a feature.

## Existing foundation and gaps

Verified in the current implementation:

- WorkspaceRecord stores project root, branch, path, base reference, and attached thread IDs. Multiple conversations can attach to a workspace.
- SessionRegistry supports multiple ACP sessions. Successful isolated turns can produce checkpoints.
- WorkspaceTurn and ensure_idle prevent overlapping turns/checkpoints and Git operations in the same working copy. Foundry also reserves its working directory.
- Existing workspace actions include updates from the project base, checkpoints, push, PR creation, and merging into the local default branch. Local merge does not itself publish.
- Smart Model Routing offers advisory, user-approved model changes for a prompt in one thread. It is not a scheduler or task-decomposition service.
- Foundry has stage bindings, dependencies, acceptance records, and review gates, but progresses through a current stage/cursor. It should not be presented as an existing parallel feature executor.
- DevServerManager currently holds one running server. Multiple independent previews require additional work.
- A global workspace gate protects start/send/Git paths. Some connection work is awaited under that gate: audit and narrow locks before claiming simultaneous starts across projects. Existing asynchronous prompt execution is not proof of all concurrency guarantees.

Missing: durable features/tasks, task assignment and dependencies, dependency-aware launching, task ownership/overlap visibility, feature integration workspaces, validation tied to exact revisions, a landing queue, and a consolidated project activity view.

Relevant code: crates/grok_persistence/src/workspaces.rs; crates/bomb_core/src/services/workspaces.rs; crates/bomb_core/src/services/mod.rs; crates/grok_control_core/src/registry.rs; crates/bomb_core/src/foundry.rs; crates/bomb_foundry/src/run.rs; crates/bomb_core/src/devserver.rs.

## User model

Project → Feature → Task → Attempts, conversations, and workspace.

A feature has a title, desired outcome, acceptance criteria, priority, target branch, status, and optional dependencies on other features. Creating an idea does not create a process or a worktree.

A task has a short brief, scope, acceptance checks, dependencies, and an assignment (provider, exact model ID, reasoning effort). One task normally owns one branch/worktree for its lifetime. It may have several attempts or sequential conversations, including a model handoff. Two models doing different work concurrently are two tasks with two worktrees. Two models trying alternative solutions are explicitly competing tasks: select one result, rather than automatically merging both.

For small features the default is a single task; the user should not have to construct a task graph.

Suggested feature board: Ideas → Ready → In progress → Review → Ready to merge → Done. Attention badges indicate blocked dependencies, failed checks, conflicts, rate limits, or a decision needed. Done means integrated into the configured target, not merely an agent saying it finished. If publishing is separate, display Not pushed / PR open independently.

Task states: Draft, Ready, Queued, Running, Needs input, Blocked, Review, Accepted, Integrated, Cancelled. Runtime attempt state is separate, so a retry does not erase the previous error or make the feature appear complete.

## Concrete example: leaderboard feature

1. Capture “Add a leaderboard with saved scores.”
2. Start UI exploration with provisional mock data while the backend task proposes storage and the API. Agree on the shared interface before real API wiring, rather than blocking all UI work.
3. Launch independent tasks as soon as their own required inputs are available:
   - Frontend: Fable, selected effort. Leaderboard view, interactions, loading/error states. Uses the agreed fixtures until the API is ready.
   - Backend: Astra, selected effort. Score persistence, endpoint, validation, tests. Owns database migrations.
4. Both run in separate worktrees from a recorded base. The UI task isolates its sample-data adapter. When the shared contract is accepted, pin that revision for API wiring and reconcile the UI and backend against it.
5. Combine accepted checkpoint commits in a feature integration worktree. Run the real frontend against the real backend, check the acceptance criteria, and preview the combined feature.
6. Show the combined diff and evidence to Max. After approval, land one reviewed feature on the target branch.

Changing the contract during implementation creates a new contract revision and flags affected tasks for review. Agents should not silently improvise incompatible APIs. Tasks that need the backend's actual implementation rather than its contract remain dependent and run later.

## Concurrency rules

- One writer per worktree at a time, regardless of model or number of chat tabs. Sequential handoffs can share a workspace; simultaneous writers cannot.
- Separate worktrees protect uncommitted files, but do not eliminate merge conflicts or incompatible behavior. Concurrent edits to the same file are possible in separate worktrees; show an overlap warning and decide whether to sequence or integrate them deliberately.
- Declare expected edit areas as planning guidance. File-scope notes are not security enforcement. Actual write restrictions require enforcement in the existing permissions/tool layer; never claim a prompt instruction is a hard boundary.
- Give shared contracts, lockfiles, generated clients, migrations, global styles, and central routing/configuration an explicit task owner. Broad refactors should be sequenced before or after dependent features.
- Reads may inspect dependencies at pinned commits. Do not make a task depend on another agent's moving, dirty working copy.
- Assign separate preview ports and test database namespaces where needed. Git does not isolate ports, databases, remote accounts, environment files, or shared generated artifacts. Initially expose one selected preview rather than pretend concurrent previews exist.
- Start with a configurable project limit of two active coding tasks. Later add global/provider limits and separate limits for expensive builds/tests. More simultaneous agents can increase merge and review work.
- Never expand tool permissions when switching model or starting a task. Existing deny rules and approval modes apply.

## Git workflow

Use short-lived feature/task branches against a chosen target branch. Do not require an additional permanent develop branch.

For a one-task feature: create one worktree/branch from a recorded target commit; checkpoint; review; validate against the current target; merge with approval.

For a multi-task feature:

- Record target commit T. Create a feature integration branch/worktree at T.
- Commit any approved shared contract at C on that feature branch. Create each task branch/worktree at C (or T when no shared code contract is needed).
- Workers commit only in their own branches. Record candidate head SHAs for review. A checkpoint is recoverable progress, not task acceptance.
- Freeze accepted task heads for the integration attempt. Merge those exact commits one at a time into the integration worktree. Use normal merges initially; do not rewrite active worker history.
- Resolve conflicts only in the integration worktree. Record resolutions and test the combined result. Keep task worktrees intact for follow-up.
- Store validation with the integration head, target head, brief/contract revision, and accepted input SHAs. Any relevant change invalidates Ready to merge.
- Land features serially per repository/target. If another feature advances the target, update the next integration branch, rerun affected checks, and review the new result before merging. A clean Git merge does not prove the combined behavior works.
- MVP landing should preserve merge ancestry. A later optional squash policy must also define how continuing task branches are retired/rebased; do not mix ad hoc squash and cherry-pick flows.
- Push, PR creation, final merge, and deployment remain distinct actions with clear destinations. Cleanup happens after integration is verified and no uncommitted work would be lost.

Branches can be named bomb/feature/<feature-id> and bomb/task/<task-id>-<slug>. Models do not belong in branch identities: switching models should not split the feature's history.

Git worktrees provide multiple working trees sharing one repository, with separate working-tree state: https://git-scm.com/docs/git-worktree. Git merge combines histories and can leave conflicts requiring resolution: https://git-scm.com/docs/git-merge. Worktree administrative locks are not agent write locks; preserve application-level ownership.

## UI proposal

Project overview has Features and Activity. A quick “Add idea” field stores an idea immediately. Feature cards show task progress, running models, attention needed, and review/merge readiness; they do not require opening each conversation.

Feature detail includes:

- Outcome and acceptance criteria.
- Tasks with assignment, reasoning effort, dependencies, branch/worktree, and current activity.
- All associated conversations, including existing threads attached by the user.
- Combined Changes, Preview, and Checks once integration is prepared.
- One decision list for questions, blocked tasks, review requests, and conflicts.

Task actions: Start, Open thread, Pause/stop, Retry, Reassign model, Review result. A model change waits for the current turn/checkpoint to finish. Clicking Start clearly shows the branch/worktree and sends only that task's approved brief and relevant context.

Capture an unrelated idea during a task with “Save as feature” or “Add follow-up task.” This should not redirect the currently running agent unless Max explicitly changes its scope.

Show model defaults by task type (frontend/backend/testing) with per-task overrides. JEV may suggest an assignment using those preferences and connected provider catalogs. Persist the accepted assignment; do not reroute every attempt unpredictably. Display model/effort actually applied by the provider.

## Data and implementation boundaries

Use the existing SQLite persistence layer for operational records. Prefer stable project IDs mapped to repository roots so moving a project does not orphan its features.

Proposed records:

- features: project, title, brief revision, acceptance criteria, target ref, priority, lifecycle state.
- feature_tasks: feature, title, scope, acceptance criteria, assignment, workspace ID, lifecycle state.
- task_dependencies: prerequisite and dependent task IDs, required acceptance revision. Reject cycles and invalid cross-project dependencies.
- task_attempts: task, session/thread, actual provider/model/effort, start/finish, outcome, base/head SHAs, diagnostic summary.
- feature_integrations: feature, workspace, target SHA, accepted task heads, integration SHA, validation/approval references, status.
- feature_events: durable transitions and user decisions for recovery/audit.

Reuse workspace/session services; avoid a second Git manager or a separate agent-launch pipeline. Put coordination in a bomb_core feature service. GPUI views render persisted state and service events. Do not place orchestration logic in the composer or JEV client.

Specs may be exported to repository Markdown, but automated mutable status should live in SQLite initially to avoid task agents constantly conflicting over a shared tracker file. Each attempt receives an immutable brief/contract revision.

For later automated scheduling: reserve task + workspace ownership before launching; use an attempt ID as the idempotency key; reconcile interrupted attempts on restart rather than blindly replaying prompts. Use short repository locks for shared ref mutations and workspace leases for the full turn/checkpoint. Do not hold a global Git lock during model/network execution.

## Delivery sequence

1. Feature tracking and manual assignments: project board, idea capture, feature briefs, tasks, attach existing threads, model/effort assignment, explicit Start in isolated worktree, visible statuses. Reuse current execution and checkpoints. No automatic decomposition or merge queue yet.
2. Reliable parallel tasks: dependencies, guarded launch of ready tasks, concurrency limits, overlap visibility, durable attempts/recovery, and project activity/decision inbox. Audit the global lock and validate two real provider sessions concurrently.
3. Feature integration: integration worktree, frozen task inputs, conflict handling, combined preview/checks, target-aware validation and manual serialized landing. This completes the multi-part frontend/backend workflow.
4. Opt-in automation: propose a task split, approve the plan, then automatically launch only ready tasks within limits; schedule reviewer runs and prepare integrations. Keep final merge/deploy decisions explicit until Max defines a more specific policy.

First useful release is step 1. The complete safe parallel-feature workflow needs steps 1–3. Do not label tracking alone as automatic orchestration.

## Acceptance checks for implementation

- Two tasks in one project can run concurrently in different worktrees with the same provider or different providers.
- A second writer cannot enter an owned worktree, including during checkpointing and retry.
- Failed startup can be retried without losing task history or sending duplicate prompts.
- Dependency cycles are rejected; a revised contract invalidates dependent acceptance.
- A model/effort adjustment is visible without changing task identity or permissions.
- Restarting the app restores features, workspaces, and attempt outcomes without assuming interrupted work succeeded.
- Integration uses recorded task SHAs; later worker commits do not silently enter a reviewed result.
- Conflicts leave worker branches intact and cannot advance the target automatically.
- Target movement invalidates stale merge readiness; checks run on the combined candidate.
- Completed agent turns/checkpoints cannot mark a feature Done before it is integrated.
- Preview environments do not silently replace another task's server or share mutable test data.

## Practical workflow before this is built

Create separate isolated threads for independent work, name them consistently by feature/task, give each a clear scope and shared contract, and run them independently. Review checkpoints and integrate one at a time using existing controls. For unfinished multi-part features, the integration-branch step is currently a manual Git workflow; the existing Merge action targets the project's default branch, so do not use it as if it already combined a private feature. Avoid putting concurrent editing agents in conversations attached to the same workspace.
