# Conversational project workspace, board, and review workflow

Research and design proposal for Max, 2026-09-19. No kanban, planner, or automatic project-run implementation is included. This extends the optional Features workflow in [FEATURES.md](../FEATURES.md).

## Intended experience

Describe the project → inspect an editable plan → Go → watch features progress → handle the decisions that need you → receive a checked, integrated result.

The project's primary entry point is a persistent conversation: “Let's do this, fix that bug, and then add this feature.” The app translates that conversation into tracked parallel work. The board is another view of the same work, not a form the user must fill in first.

The interface should answer three questions immediately: What is moving? What needs me? How do I inspect and accept the result? Success means getting through those decisions with little navigation and enough evidence to make them confidently. Few clicks alone are not proof of usable review.

## Research findings

These are documented interaction patterns and design inferences, not user-testing results for Bomb Code.

| Reference | Observed pattern | Application to Bomb Code |
|---|---|---|
| [Linear board layout](https://linear.app/docs/board-layout) | Boards and lists offer similar actions and keyboard navigation; grouping and hidden columns reduce visible clutter. | Default to a compact feature board; retain a list option for dense projects and smaller windows. |
| [Linear Peek](https://linear.app/docs/peek) | Space previews the focused issue; arrows move between issues while the preview stays open. | Keep a persistent inspection pane that changes with the selected card, preserving the board's position. Make the same action visible for mouse users. |
| [Linear Triage](https://linear.app/docs/triage) | A focused inbox supports explicit accept, decline, and snooze decisions with shortcuts. | Offer a Needs you queue with a concrete question and action on each item, instead of requiring users to find blocked threads. |
| [Vibe Kanban board](https://vibekanban.com/docs/cloud/kanban-board) | Status columns, issue cards, a right-side issue panel, and hidden backlog/cancelled states. | Use the board for outcomes and an in-place panel for work inspection. Keep inactive ideas and old results out of the active viewport by default. |
| [Vibe Kanban review](https://vibekanban.com/docs/reviewing-code) | Diff comments are collected and sent together to the agent. | Let the user inspect several issues and submit one Keep working instruction with the comments attached. |
| [Vibe Kanban browser testing](https://vibekanban.com/docs/browser-testing) | An embedded browser supports app testing and selecting UI elements as chat context. | Show the actual selected task/feature candidate with concise test instructions; visual feedback should point to that candidate. |
| [Nielsen Norman Group: progressive disclosure](https://www.nngroup.com/articles/progressive-disclosure/) | Keep the primary interface focused while revealing less common controls when needed. | Start with result, evidence, and decisions. Put full logs, individual attempts, and Git internals behind detail controls. |

Vibe Kanban's [workspace interface](https://vibekanban.com/docs/workspaces/interface) uses four panels and separate context modes. For Bomb Code, the proposal is a simpler default with two content areas: board/queue and inspection. Full thread and technical detail views remain available when needed.

## Talk to the project

Selecting a project opens its persistent project conversation, with **Conversation / Board** views and a shared Needs you count. The composer can capture a whole project, several independent requests, a bug report, or feedback on existing work. Regular model threads remain available for direct work.

Example message: “Polish the HUD, fix scores resetting, and add a leaderboard. Use Fable for the visuals and Codex for the backend.”

The response is concise, actionable, and linked to real work:

```text
I'll start HUD polish and the score-reset fix in parallel.
Leaderboard wiring will wait for the score fix.

[HUD polish · Fable · Medium · Ready]
[Score reset · Codex · High · Ready]
[Leaderboard · UI + backend · Waiting on score fix]

[Go]  [Adjust]
```

For the first run, Go accepts the visible plan and execution policy. Within an already authorized run, routine additions can queue or start under that policy with an immediate visible acknowledgement. Avoid a second approval for every sentence. Changed scope, new external effects, or exceeding the run's limits should be surfaced as concrete decisions. If the user has not enabled automatic dispatch, show the queued cards with one Start ready work action.

Interpret messages as additions, amendments, questions, or pause/cancel instructions. Link follow-ups to the existing feature/task when clear, and ask a short inline disambiguation question when “fix that” could target multiple results. Maintain the original request and links to the resulting tasks. A project-level response must report actual launch state, not claim work is running merely because a plan was generated.

Words such as “then” are not sufficient on their own to serialize every idea. Respect explicit ordering like “after the API is finished”; propose useful independent work in parallel and show the intended dependency in the acknowledgement. A new idea should not regenerate all running briefs or reset completed work.

Finished results return to the project conversation as review cards. Each card offers Test it, the appropriate acceptance action, and Keep working. The project knows which feature/worktree the result belongs to, attaches feedback to it, and routes work to the right task. Users can inspect individual agent threads, but normal delegation and acceptance do not require that navigation.

The project conversation, board, attention queue, and task thread are views of shared records and decisions. Resolving a decision in one place resolves it everywhere; they must not create four independent approval requests. Routine progress updates consolidate within the work card instead of flooding the conversation with every tool event.

## One project home

Top bar: project name, **Conversation / Board**, **Go / Pause**, and **Needs you (count)**. Plan project and New feature remain available through Command K or inline actions, but the project composer is sufficient to get started. Ordinary threads remain accessible. Projects that have not enabled Features keep their existing experience.

Default columns:

| Ideas | Queued | Working | Review | Done |
|---|---|---|---|---|
| Captured, not part of a run | Accepted work, ready or waiting on a named dependency | Building, checking, AI reviewing, repairing, or landing | A result is ready for the human acceptance gate | Accepted result integrated into the run's selected target |

Waiting, Failed, and Needs you are explicit badges/filters rather than several extra columns. A feature can still be Working while one of its tasks needs a decision. AI review remains a Working substatus; Review means there is something ready for the user to inspect. Show the exact reason for waiting, not a generic spinner.

Use one feature card per user-visible outcome. Inside it show a short task summary such as “UI working · API checking · Wiring waiting,” with model identities and reasoning effort available on the task rows. A feature can contain multiple task threads and attempts without creating duplicate top-level cards for every session. A Tasks toggle can expand this detail when coordinating parallel work.

Cards move automatically from actual run events. Dragging is useful for ordering queued work; dragging to Done opens the acceptance action and cannot bypass checks or fabricate integration. Preserve selection and scroll when cards update. Avoid constantly reordering a queue while the user reads it.

## Inspect without opening another window

Select a card once to open an in-place inspection pane. Selecting another updates the same pane. On narrow windows, expand the inspection area within the same center view, with a return action restoring the board position. Do not squeeze a full kanban, thread, diff, preview, and terminal into tiny simultaneous panels.

The default inspection content is a result card:

- **Requested:** the outcome and a short acceptance checklist.
- **Changed:** a few factual bullets tied to the candidate's diff.
- **Check it:** a Preview action and two or three concrete steps to try, or a suitable backend/API check when there is no visual UI.
- **Evidence:** checks that passed, failed, or were not run; reviewer findings; known limitations. Expand any item for its actual output.
- **Next decision:** explicit buttons and a compact feedback input.

Tabs within the pane: **Result · Preview · Changes · Thread**. Opening a task starts with its result or blocker, not the first line of its conversation history. The normal thread shows the same decision card linked to the same state; accepting from either view resolves it everywhere.

A preview identifies its task/feature and candidate revision, and whether it uses fixtures or a real backend. A frontend preview with mocks is not a demonstration of a completed integrated feature. Switching cards must switch to the correct worktree/server or clearly offer to start it. The current single-server manager needs explicit candidate binding or an extension before this experience can be promised for concurrent tasks.

## Decisions with clear consequences

| Situation | Primary actions | Meaning |
|---|---|---|
| Draft plan | **Approve plan & Go**, **Edit plan**, **Save for later** | Accept the displayed scope, assignments, gates, concurrency, and landing policy. |
| Tool permission | **Allow once**, **Deny** plus existing scoped permission choices | Authorize that operation; does not accept the feature. |
| Missing product decision | Suggested answers plus free text | Answer the named question and unblock its affected work. |
| Task checkpoint requiring acceptance | **Accept task**, **Keep working** | Accept this task's output for its dependents, not the whole feature. |
| Integrated feature ready for manual landing | **Approve & merge**, **Keep working** | Accept and queue the reviewed candidate for the displayed local target, or send feedback for another pass. |
| Failed check or exhausted repair limit | **View failure**, **Retry / continue with feedback** | Explain what failed and what another attempt would do. |

Never use a universal “Approve” button for all of these. Label the object and consequence in the card. Show “Merged into local main” only after landing succeeds; pending/conflicting landing stays visible. Approval applies to the reviewed revision and is invalidated by material changes to that candidate.

Keep working submits the user's text plus attached code/preview comments to the appropriate task. It preserves the feature and its history; the user does not need to start a new thread. If feedback expands scope, show the proposed plan change before adding it to the current run.

For speed, default human acceptance to a combined feature result, not every worker turn. Allow task-specific gates for visual direction or decisions that would otherwise waste downstream work.

## Needs you queue

The count includes tool requests, product questions, review decisions, and recoverable failures, each visibly typed. Select it to use the existing center pane as a compact queue plus the same inspection pane. Offer Project / All projects scope without opening another window.

Order by work unblocked and age, with the reason visible, such as “Choosing score type unblocks two tasks.” Independent tasks continue. Do not sound an alert for every tool result; notify when user action becomes necessary, the run fails, or the run finishes.

After a decision, **Next item** moves to the next pending item without returning to the board. Allow an opt-in automatic next-item mode, but preserve drafts and never reuse a button press or typed approval on the next item. Arrow keys navigate cards; Space peeks; Escape returns to the board. Command K exposes New feature, Plan project, Needs you, and Pause project. Exact shortcuts must be checked against existing bindings before implementation.

Completed decision cards collapse to a short history record, rather than staying above the composer. Pending decisions remain visible until resolved; a previously dismissed old result must not hide a new request.

## Go and automatic merging

The approved plan shows the assignments and execution policy inline. Two landing modes make the user's intent explicit:

- **Review features before merging:** automated task dispatch, checks, AI review, and bounded repair; one human decision for the combined feature.
- **Auto-merge passing features:** passing feature candidates land automatically under the accepted gates. Human questions and specified acceptance gates still pause affected work.

Both modes should use the same board and review card. Auto-merge removes unnecessary approval stops; it does not manufacture evidence or turn a worker's “done” message into a passing review. Go does not broaden the chosen tool permissions. Local integration, remote publishing, and deployment are separate actions/policies.

The app handles branch/worktree creation, dependency combination, checkpointing, and a serialized target merge queue. A moved target or changed candidate refreshes affected checks/review. Pause stops new dispatch and landing, with active isolated work allowed to finish; Stop requests cancellation and preserves outputs. Recovery shows interrupted runs and reconciles actual sessions and Git state before Resume.

Planning produces the feature/task records directly, with an editable preview before acceptance. Shared tracking documents are generated at checkpoints by one writer. The user should not have to copy a plan out of a conversation, make every card manually, or maintain matching Markdown and UI status.

## Example session

“Build a leaderboard and polish the game's HUD. Fable for UI, Codex for backend.”

1. Plan project proposes Foundation, HUD polish, and Leaderboard. UI fixture work and API work can overlap after the foundation.
2. The user edits one requirement and chooses Go. Cards advance as work runs.
3. A badge says “Needs you: weekly or all-time scores?” The user answers All-time in the inspection pane; HUD work continues.
4. HUD reaches Review. Its Result tab shows the changed controls, preview, and test evidence. The user tries it, enters “Make the score larger on mobile,” and clicks Keep working.
5. The revised card shows what changed since the last review. Approve & merge queues the candidate; Next item opens the leaderboard result.
6. Done contains integrated features. The project completes only after its planned final checks and acceptance gates pass.

## Validate the design before building the larger workflow

Prototype these tasks with Max: describe an app, alter one proposed assignment, inspect a UI result, send feedback, approve a revision, answer an API question, pause a run, and return to ordinary chat.

Measure time to find the next action, time to understand a result, navigation/backtracking, and mistaken approval scope. Initial interaction targets: one selection from board to evidence; one action from an inspected result to acceptance; one feedback submission to continue; no required external window for supported previews/diffs. These are design targets, not measured performance claims.

Test busy and failure states too: many simultaneous tasks, a missing provider, stale preview, denied tool, conflicting merge, changed plan, restart, and a small window. Pay particular attention to whether the user can distinguish “agent finished,” “checks passed,” “I accepted,” and “merged.”

Implementation of this workflow remains deferred at Max's request. The separately reported status-text overlap is a rendering bug and is handled independently.
