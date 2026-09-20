# Project workflow UX audit and repair plan

Date: September 19, 2026 (America/Denver)  
Code reviewed: `gpui-rewrite`, including the conversational project workflow introduced in `d2cc218`.  
Status: proposal only; no application behavior or live project state changed.

## Main finding

The interface does not reliably answer three questions: **What is happening? Does it need me? What should I do next?** Several problems are functional, not merely visual. A stopped reviewer can look active, blocked work appears under Working, and a generic continuation action cannot explain or reliably recover every failure.

Keep the existing isolated worktrees, dependency scheduling, exact-revision approvals and independent review. Put a consistent, feature-centered interaction model over them, and fix state/recovery behavior before adding more automation.

## Evidence and limits

- **Observed:** read-only inspection of the `test-bombcode-game` workflow record and associated transcripts during this investigation. A second snapshot at `2026-09-20T02:19:35Z` still showed the mismatch below.
- **Source-confirmed:** traced project creation/opt-in, project conversation, plan approval, model assignment, board, task execution, permissions, review, preview, merge, Pause/Stop/Resume, restart recovery and their thread/sidebar entry points.
- **UX assessment:** judgments about wording, information density and navigation follow from these implementations and the user's screenshots. This was not a complete interactive native-app usability test. Layout, keyboard navigation, focus, tooltips, contrast and screen-reader behavior require the hands-on checks listed below. Previous Computer Use access was unavailable; it was not retried.
- Scope is the project-work experience and the connected chat, routing, review-loop, preview and Git surfaces. This is not an exhaustive audit of every unrelated Settings or MCP-management screen.

### The multiplayer incident

The foundation build was checkpointed. Its candidate checks passed, including 36 tests. A later independent review attempted external Convex account verification; after a network failure it requested additional permission. The app's read-only-session policy cancelled that request. The review therefore did not pass.

The persisted state then simultaneously said:

| Level | Recorded state | Meaning |
|---|---|---|
| Project | `Running` | Run intent remained enabled; this did not establish that useful work was active. |
| Foundation feature | `NeedsInput` | Review had stopped; no passing review was stored. |
| Foundation builder | `Checkpointed` | Implementation finished; this was not completion of the entire feature. |
| Latest reviewer session | `running` | Late tool events followed cancellation and wrote this status again. |
| Other three features | `Queued` | Their required feature integrations had not happened. |

The foundation note was only “The agent stopped without completing the task.” Two downstream features depended directly on foundation; the last depended on those two. Review-before-merge was enabled, with concurrency two and one automatic repair attempt. Waiting alone would not resolve this state. This was not a case of the user rejecting an approval.

## Prioritized findings

Priority meanings: **P0** = repair before relying on unattended runs; **P1** = required for the intended understandable, low-click workflow; **P2** = refinement after those foundations. Evidence tags are O (observed), S (source-confirmed), U (UX assessment/validation needed). Source references appear at the end.

### Status, execution and control

| ID | Priority / evidence | Current problem and user impact | Proposed fix |
|---|---|---|---|
| 01 | P0 · O/S | `NeedsInput` is grouped in Working. The project can remain Running after its driver has no runnable jobs. Waiting looks productive. | Derive the visible summary from actual activity and blockers. Add a distinct Needs you lane. Separate run intent from current activity; expose both when useful. [S1, S2, S3] |
| 02 | P0 · O/S | Late tool events write session status back to running after cancellation/completion. Thread, board and project disagree. | Correlate events with a run/turn attempt. Terminal outcomes stay terminal for that attempt; late output belongs in its log and cannot reactivate it. Reconcile session activity with the runner. [S4] |
| 03 | P0 · O/S | Read-only reviewers silently cancel non-exempt permission requests, including the observed external-verification request. The user sees an incomplete-task error without its cause. | Preserve a typed blocker: stage, operation, policy decision, evidence and next action. Distinguish policy refusal, user denial, missing credentials and network failure. Provide a supported verification route without granting the reviewer write access. [S5, S6] |
| 04 | P0 · S | Review/check errors enter the same automatic repair loop. A missing capability or network problem can consume a code-repair attempt. | Classify code defects separately from access, environment, provider and cancellation errors. Only code defects trigger code repair. Preserve valid checkpoints and evidence; retry the failed stage when its cause is resolved. [S6] |
| 05 | P0 · S | Pause lets active tasks finish, but the UI disables Stop whenever the project is not Running and not planning. Paused work may still need cancellation. | Keep Stop available while anything is active or cancelling. Show “Paused · 2 tasks finishing,” explain Pause's effect, and show acknowledgement when Stop finishes. [S3, S7] |
| 06 | P1 · S | Resume queues every saved Idea. “Save for later” becomes “start at next resume,” which is surprising. | Resume only previously started work. Add Start selected to backlog items and the plan card; explicitly add new scope to a run. [S7] |
| 07 | P1 · O/S | Queued cards do not explain whether they await a dependency, capacity, review, merge or Resume. Plan dependencies show internal IDs. | Show the immediate reason using feature titles: “Waiting for Foundation to merge.” Include a link and downstream impact. Show capacity separately; never imply a blocked job occupies an active agent slot. [S2, S8] |
| 08 | P1 · S | Reopening marks interrupted tasks for inspection; the header suggests Resume, but Resume does not itself reset those tasks. | Show a recovery card listing completed checkpoints and interrupted attempts, with Inspect / Continue interrupted task / Leave stopped. Resume becomes meaningful only for runnable work. Reconcile merges before retrying; do not replay completed prompts. [S7] |

### Describing work, planning and routing

| ID | Priority / evidence | Current problem and user impact | Proposed fix |
|---|---|---|---|
| 09 | P1 · S | Project and feature drafts clear before service acceptance; project switches and feature selection also clear inputs. Failure/navigation can lose unsent text. | Store drafts per project and feature. Clear only after an acknowledged submission; retain failed text with Retry/Edit. Async results must match project, feature and request generation. [S9] |
| 10 | P1 · S | A question returning no new features/updates sets the draft proposal to None. Asking about a plan can make its approval card disappear. | Keep the proposal until explicitly accepted, replaced or discarded. Treat questions as conversation, and show revisions as revisions of the same proposal. [S10] |
| 11 | P1 · S/U | The approval card expands all briefs, tasks, criteria and commands at once, but the effective merge policy and concurrency are elsewhere. | Start with a compact scope summary, what starts now, what waits, model/effort assignments and run policy. Expand acceptance criteria and exact commands in place. Add selection, edit and discard actions. [S8] |
| 12 | P1 · S | The project composer always invokes planning, even for status questions or an apparent command such as “pause.” Feature feedback uses a second textarea elsewhere. | Explicitly route new work, status questions, feature feedback and deterministic run controls. Show the destination above the composer. Resolve ambiguous feature references before changing work; “pause” must pause, not merely produce planner prose. [S9, S10] |
| 13 | P1 · S/U | Project routing uses global JEV enablement; its task suggestions lack visible provenance, and project controls differ from the thread routing icon. Saved role preferences are tucked into Manual features. | Reuse Smart Model Routing controls at project scope: inherit/on/off, role defaults, supported effort and “why this assignment.” Clearly distinguish requested, suggested and actually applied models. Explicit assignments should be structurally locked against advisory reassignment. Revalidate queued assignments before dispatch and offer Replace model when unavailable. [S10, S11] |
| 14 | P1 · S/U | Dependencies require feature integration before downstream features start. A planner can unnecessarily serialize UI behind backend work, even though fixture-based UI is supported inside a feature. | Show and challenge dependency reasons before Go. Prefer parallel UI/backend tasks plus a wiring task when appropriate. Keep genuinely shared scaffolding as a prerequisite. Do not remove integration guards simply to make the board look busy. [S8, S10, S12] |

### Board, threads and decisions

| ID | Priority / evidence | Current problem and user impact | Proposed fix |
|---|---|---|---|
| 15 | P1 · O/S/U | Technical workspaces such as `feature-candidate` appear as things the user must understand. Cards expose `Checkpointed` and other implementation states. | Make the feature the primary object. Nest Build, Combine, Checks, AI review and Merge activity beneath it. Use “Build finished · review blocked.” Keep worktree/branch IDs under Technical details. [S1, S2, S6, S13] |
| 16 | P1 · S | Needs you counts review/input features plus approvals, but Next needs you cycles only those features. Planning questions and final-project failures are not included. | Use one durable decision collection for plan questions, permissions, blockers, human review and final failures. The count, filter and Next action must operate on the same unresolved records. Group related decisions without hiding distinct approvals. [S3, S14] |
| 17 | P1 · S | Managed threads show a feature link but retain an ordinary composer and generic retry. Sending can be rejected because the workflow owns the workspace. | Render the same feature decision card in the thread and project. Label the composer “Feedback for Foundation”; dispatch through the feature service. A task retry must identify its stage and remain owned by the orchestrator. [S13, S15] |
| 18 | P1 · S/U | Inspect replaces the board; opening a related thread navigates away. Multiple lookalike text boxes and unmarked tabs make context easy to lose. | Use one inline inspector on wide layouts and a reversible single-pane detail on narrow layouts. Preserve board filter/scroll and draft. Clearly mark active tabs. One contextual composer, with a visible Add new work escape. [S2, S3, S9] |
| 19 | P2 · S/U | Five fixed 250px columns require substantial horizontal space; full-width expanded plans/logs compete with the work. | Responsive board with compact cards and a list fallback. Keep outcome, stage, reason and action visible; collapse successful checks and detailed tasks. Validate minimum supported window width and long feature names. [S2, S3] |
| 20 | P1 · S/U | Standalone review loops, project review and manual features offer overlapping concepts and entry points. A completed sub-run can be mistaken for feature completion. | Reuse the decision/result components and vocabulary across flows. Label completion by scope: “Build finished,” “Review passed,” “Merged locally.” Hide resolved gates from active attention; retain them in history. Move model defaults to project settings and reconcile manual/automatic feature records before consolidating entry points. [S3, S13, S16] |

### Testing, review, merge and completion

| ID | Priority / evidence | Current problem and user impact | Proposed fix |
|---|---|---|---|
| 21 | P1 · S | Preview silently returns when no candidate exists. Reopening the same candidate calls server start, which stops/restarts the server. Switching candidates requires hunting for Stop elsewhere. | Always show a useful Preview state: Not ready / Starting / Ready / Failed. Reuse an existing server for the same revision. Offer one explicit Switch preview action for another worktree. Scope async results to the selected feature/revision. [S9, S17] |
| 22 | P1 · S/U | Review is a long mix of handoffs, criteria numbers and command output; the diff is one code block. Users must infer what to test and whether the preview is real or fixture-backed. | Lead with what changed, how to test and what remains unverified. Show preview revision/environment, named acceptance criteria, evidence provenance, failed checks first and a file-based diff. Preserve earlier review attempts in a timeline. [S14] |
| 23 | P1 · S | “Approve & merge” only stores approval when paused. Dirty checkout/wrong branch failures become generic NeedsInput, whose continuation clears review and approval. | Label paused approval accurately. Add typed Merge blocked cards with relevant Git actions and Retry merge; retain exact-revision approval when still valid. If target/candidate changes, clearly explain revalidation and any required new approval. [S7, S14, S18] |
| 24 | P1 · S | Final assembled-project checks can fail after all features show Done. Failure pauses the project without becoming a first-class Needs you item. | Add a project-level result/decision card: “Features merged · final verification failed.” Offer Retry checks for transient failures or a bounded repair proposal for an integration defect. Announce completion only for the verified target revision. [S12, S14] |
| 25 | P1 · S/U | Optional workflow still dominates the project home when off; parallel manual/automatic screens and mixed documentation make adoption unclear. Portable files do not contain all local live evidence. | Give workflow-off projects a useful normal home and a small opt-in. Explain where approved records live and when they enter the repo. Link Tracking documents from the feature. Distinguish portable checkpoint history from local live state, and do not imply an imported clone reconstructs running sessions. [S3, S19] |
| 26 | P1 · U/S | The user reported tool text painting over the composer. Existing clipping/summary code addresses specific paths, but it is not evidence all content is contained. | Native visual regression pass across tool JSON, multiline commands, approvals, expanded errors, diffs and inline cards. Bound/scroll expanded content, wrap or truncate summaries, preserve copy/full details, and keep the composer hit area unobstructed. Verify keyboard focus and accessibility too. [S13, S20] |

## Proposed experience

### 1. Project home: talk, see progress, handle decisions

Keep Conversation and Board as views of the same work. No additional workflow-management window is needed. The header should answer immediately:

> **Waiting for you** · 1 blocked feature · 3 waiting on dependencies  
> No tasks executing. Automatic continuation is on. Review before merging to `main`.

If independent work is running, say “2 tasks running · 1 needs you” instead of declaring the entire project blocked. Show active tasks separately from checks/merges; do not label every scheduler job an agent.

Board lanes: **Backlog / Queued / In progress / Needs you / Done**. Cards in Needs you have a subtype: Ready to test, Access needed, Answer needed, Checks failed, Merge blocked or Interrupted. A completed feature means merged locally; project completion additionally requires final verification.

The primary unit is a feature. Related agent threads and worktrees remain available under its activity, with clear titles such as “Multiplayer foundation · AI review.” A run should not fill the main sidebar with unexplained infrastructure names.

### 2. Plan approval: compact, editable and explicit

Show the proposed feature titles and a short outcome for each. Above Start selected:

- What can start immediately and what depends on another result, using names and reasons.
- Models and reasoning effort, with expand/edit controls per role or task.
- Effective parallelism, target branch, review gate and automatic-repair limit.
- Expandable acceptance criteria, exact commands, test instructions and assumptions.

Actions: **Start selected**, **Save to backlog**, **Revise**, **Discard**. If selected work requires an unselected prerequisite, resolve that choice before starting. Keep questions and failed planning attempts attached to the same draft. A status question must not destroy it.

Use existing inline card, model identity, picker, disclosure and approval components. Smart routing is advisory; the visible accepted assignments are authoritative. Enhancement should be an optional preparation step with draft preservation, not another mandatory wizard.

### 3. Decisions: the same card wherever the user is

For the observed failure, the proposed card would read:

```text
Multiplayer foundation                         Needs access
Build finished · Checks passed · Review blocked

The reviewer could not verify the Convex account because
its network permission request was blocked by review policy.
Your code and successful checks are saved.

Two features are waiting for this feature to merge.

[Resolve verification]  [View saved evidence]
Details: reviewer, attempted operation, policy, attempt history
```

“Resolve verification” must open a real supported remedy. It cannot promise a permission approval that the current adapter will silently cancel. Proposed remedies are a narrowly scoped verification operation where supported, or explicit human evidence for the particular criterion if the agreed review policy permits it. Never silently mark the review passed or make the reviewer a general writer.

A passing result uses the same card shell:

```text
Multiplayer foundation                         Ready to test
What changed · 3 short outcome bullets
Checks passed · AI review passed · Revision abc123

[Open preview]  [Changes]  [How to test]
[Approve & merge]  [Request changes]
```

Each action operates on a stable decision ID and expected revision. Resolving it in a thread resolves it on the board and in Needs you. Stale actions explain what changed and open the current result. Permissions show their actual operation and consequence; approving a plan, allowing a tool and accepting a result must remain distinct decisions.

### 4. Recovery actions match the problem

| Cause | Primary action | Work to preserve |
|---|---|---|
| Missing answer | Answer question | Existing valid checkpoints |
| Access or credentials | Resolve access, then Retry verification | Code and valid checks |
| Provider/model unavailable | Reconnect or Replace model for the unfinished task | Finished task outputs and explicit preferences |
| Failing implementation check | Repair this failure | Other completed tasks |
| Transient check failure | Retry checks | Candidate revision |
| Human requests changes | Request changes | Review history; invalidate approval for changed content |
| Dirty target checkout/wrong branch | Inspect Git, then Retry merge | Valid reviewed revision and approval |
| Target advanced | Recombine and recheck | Task checkpoints; explain new review requirement |
| Interrupted task | Inspect, then Continue task | Completed checkpoints; avoid duplicate dispatch |
| Final integration failure | Retry checks or propose integration repair | Already integrated features |

### 5. Fast navigation and durable records

Cmd+K should include Add project work, Open board, Next decision, Pause project and Stop project, with the target project visible. Notifications deep-link to the unresolved decision and stay deduplicated; the project retains a durable decision queue after a toast disappears. Cross-project attention can be a later compact aggregate of the same records.

Keep unsent drafts, board position and inspector selection per project. Load earlier conversation instead of stopping at the last 60 messages. Auto-scroll only when already at the bottom; otherwise show a New activity affordance.

Link the existing `plan/features`, `plan/runs`, `plan/tasks` and `plan/results` records. Document which evidence is local to Bomb Code and which has been committed. No imported repo should require workflow adoption, and turning it off should preserve normal threads and existing records.

## Implementation sequence

### Phase 1 — Make status and recovery trustworthy

Address 01–05 and the recovery foundation for 08/16/23/24. Introduce typed blockers and attempt-aware terminal transitions; derive effective project activity. Keep existing persisted feature stages where feasible, with additive/versioned data for blockers and decisions rather than a sweeping rewrite.

Separate three concepts: run intent (continue/paused/stopped), execution stage (build/check/review/merge), and attention (what needs the user). Build a single projection that the board, thread, project header and notifications consume.

**Exit gate:** replay the multiplayer incident. It must show review blocked, preserve build/check evidence, explain the cause, and never claim a cancelled reviewer is active. Pause followed by Stop must work. An infrastructure blocker must not consume a code-repair attempt.

### Phase 2 — Make the existing UI understandable

Address 07 and 15–20, with the result presentation in 22. Add the Needs you lane/queue, human-readable dependency reasons, feature-owned thread titles and the shared inline decision card. Use existing GPUI components. Keep one contextual composer and preserve navigation context.

**Exit gate:** from either the project or its agent thread, identify the blocker and reach its real next action without searching another window. Resolve a decision in either location and verify both update.

### Phase 3 — Make planning and testing predictable

Address 06, 09–14, 21 and 23. Preserve drafts/proposals; separate Resume from starting backlog. Add compact editable plan review, visible routing/defaults and precise preview states. Implement retry-by-stage and paused approval wording.

**Exit gate:** save an idea, pause/resume another run, and prove the saved idea stays unstarted. Ask a question about a plan without losing it. Retry a transient check without rebuilding. Test and approve the exact previewed result.

### Phase 4 — Complete recovery, optional use and visual QA

Finish 08/24–26, history/notification navigation and documentation. Reopen during a build, during review and after Git has landed but before state persistence. Reconcile rather than blindly replay. Preserve unrelated dirty checkouts and ordinary threads. Exercise narrow windows and expanded content in the native app.

**Exit gate:** a complete plan → parallel work → blocker → recovery → test → approval → local merge → final verification journey, including restart, has no silent controls, lost drafts or contradictory status. Check implementation with focused reducer/orchestrator tests plus actual native UI walkthroughs; passing Rust tests alone is insufficient.

## Acceptance scenarios

These are proposed pass criteria, not tests claimed to have run in this audit.

1. **Actual multiplayer incident:** completed builder + passed checks + policy-blocked reviewer → Needs access; no false running session. Direct dependents explain Foundation; resolving verification advances only the correct stage. Human merge approval remains required by current policy.
2. **Mixed progress:** one blocked feature and one unrelated running feature → both visible; unrelated work continues within capacity.
3. **Parallel UI/API:** fixtures permit UI work alongside backend; wiring waits for both checkpoints. The preview clearly identifies fixture versus connected behavior.
4. **Gate distinction:** agent text saying “completed” cannot override failing checks/review or imply local merge. Approval, review passage, merge and final verification remain distinguishable.
5. **Pause/Stop:** pause two running tasks, then stop them; both controls explain their effects and cancellation eventually has a terminal outcome.
6. **Backlog scope:** Resume cannot dispatch saved-but-unstarted features; Start selected validates prerequisite selection.
7. **Draft and plan safety:** disconnected provider, validation failure, project switch, feature switch and a question about an existing proposal preserve relevant drafts and accepted scope.
8. **Model change:** a queued model disappears; choose a replacement with supported reasoning effort, visible scope and no changes to completed work.
9. **Retry correctness:** access, check, review and merge retries have different actions; repeated clicks do not create duplicate runs.
10. **Preview:** no candidate has an informative state; same-candidate tab selection does not restart; switching candidates is explicit; delayed A-preview output cannot overwrite B's inspector.
11. **Merge correctness:** paused approval is visibly queued; dirty main is preserved; cleanup allows merge retry without needless rebuilding. Changed target invalidates evidence as required and explains why.
12. **Restart:** interrupted attempts are actionable; late events cannot revive them; an already-landed commit is recognized without duplicate merge/dispatch.
13. **Final failure:** merged features remain merged while a final-check failure appears in Needs you with a real remedy; Complete requires a verified target commit.
14. **One decision everywhere:** thread, board, Next decision and notification resolve the same record, with matching counts and no stale enabled approvals.
15. **Optional workflow:** fresh/imported repositories can use ordinary threads without adopting tracking; enabling/disabling does not rewrite unrelated instructions or discard work.
16. **Layout/accessibility:** very long unbroken JSON, commands, titles, paths, errors and diffs never paint over or intercept the composer. Verify resizing, scrolling, visible focus, keyboard-only operation, active-tab indication, non-color status cues and accessible labels.
17. **History:** older messages/review attempts remain reachable; new events do not pull a reader away from older content.

Usability targets for the native walkthrough: understand whether work is progressing or needs help within five seconds; open a feature result in one action from its card; reach the relevant recovery control in at most two actions. These targets are not permission to skip required review or authorization.

## Source map

Line numbers refer to the audited checkout; links open the source files.

| Ref | Evidence location |
|---|---|
| S1 | [types.rs](../../crates/bomb_core/src/services/project_work/types.rs), lines 260–300: feature stages, column mapping and labels. |
| S2 | [project_work_render.rs](../../crates/bomb_app/src/views/project_work_render.rs), lines 138–261: card contents and board columns. |
| S3 | [project_work_render.rs](../../crates/bomb_app/src/views/project_work_render.rs), lines 654–989: drafts, counts, header, controls, settings, history and composer. |
| S4 | [services/mod.rs](../../crates/bomb_core/src/services/mod.rs), lines 2360–2396: tool events and session-status persistence. |
| S5 | [client.rs](../../crates/grok_acp/src/client.rs), lines 1891–1916: read-only permission cancellation. |
| S6 | [integration.rs](../../crates/bomb_core/src/services/project_work/integration.rs), candidate creation and lines 347–397: review success and generic repair path; [execution.rs](../../crates/bomb_core/src/services/project_work/execution.rs), lines 124–131: generic stopped-turn error. |
| S7 | [project_work/mod.rs](../../crates/bomb_core/src/services/project_work/mod.rs), lines 37–80 and 233–388: reopen, Go/Resume, Pause/Stop, approval and continuation. |
| S8 | [project_work_render.rs](../../crates/bomb_app/src/views/project_work_render.rs), lines 5–136: proposal card and actions. |
| S9 | [project_work.rs](../../crates/bomb_app/src/views/project_work.rs), lines 104–120, 222–242 and 432–501: context/draft changes, preview and feedback. |
| S10 | [planning.rs](../../crates/bomb_core/src/services/project_work/planning.rs), planner instructions and lines 75–103: routing and proposal replacement. |
| S11 | [model_suggestions.rs](../../crates/bomb_app/src/views/model_suggestions.rs): thread routing controls, result invalidation, model/effort card and send-current escape. |
| S12 | [execution.rs](../../crates/bomb_core/src/services/project_work/execution.rs), lines 315–370, 432–480 and 485–568: readiness, idle driver exit, failures and final verification. |
| S13 | [thread_view.rs](../../crates/bomb_app/src/views/thread_view.rs), lines 648–726: managed-feature link, retry, status and review-loop visibility; [sidebar.rs](../../crates/bomb_app/src/views/sidebar.rs), workspace/thread naming. |
| S14 | [project_work_render.rs](../../crates/bomb_app/src/views/project_work_render.rs), lines 263–606: permissions, result tabs, checks, approval, Next needs you and feedback. |
| S15 | [services/mod.rs](../../crates/bomb_core/src/services/mod.rs), lines 704–720, and [workspaces.rs](../../crates/bomb_core/src/services/workspaces.rs), lines 145–156: ordinary-send ownership guard. |
| S16 | [foundry.rs](../../crates/bomb_app/src/views/foundry.rs), [features.rs](../../crates/bomb_app/src/views/features.rs), [palette.rs](../../crates/bomb_app/src/views/palette.rs): standalone flows and command entry points. |
| S17 | [devserver.rs](../../crates/bomb_core/src/devserver.rs), lines 369–371: start stops the existing managed server. |
| S18 | [integration.rs](../../crates/bomb_core/src/services/project_work/integration.rs), lines 428–501: exact review, advanced target, branch and dirty-checkout guards. |
| S19 | [PROJECT_WORKFLOW.md](../PROJECT_WORKFLOW.md), [planning.rs](../../crates/bomb_core/src/services/project_work/planning.rs), accepted-plan records; [project_run_ux.md](project_run_ux.md), stale “implementation deferred” footer. |
| S20 | [status_line.rs](../../crates/bomb_app/src/views/status_line.rs): existing summary and expanded-detail containment. |

## Research informing the recommendations

The audit's functional findings come from Bomb Code's state and source. External references inform the interaction recommendations:

- Visible status, familiar language, user control and recognition over recall support explicit blocker reasons and consistent actions: [Nielsen Norman Group, usability heuristics](https://www.nngroup.com/articles/ten-usability-heuristics/).
- Errors should identify the specific problem and support recovery where it occurs: [Nielsen Norman Group, error-message guidelines](https://www.nngroup.com/articles/error-message-guidelines/).
- Preserving context while inspecting a work item is an established pattern; the proposed inline inspector adapts that principle to Bomb Code's existing components: [Linear, Peek preview](https://linear.app/docs/peek).

No app fixes, session retries, approvals, merges or project-state changes were performed as part of this audit.
