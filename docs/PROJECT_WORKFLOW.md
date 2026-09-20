# Conversational project work

Select a project to open **Conversation / Board / Git**. The workflow is optional for each repository. **New thread** still opens a normal conversation, whether project work is enabled or not. Importing a repository does not rewrite its instructions or adopt this workflow automatically.

## Start with a description

1. Select **Enable project workflow**.
2. Describe what you want in the project composer. For example: “Build a leaderboard, improve its UI, and fix score sorting. Use my frontend model for the UI, Codex for the backend, and a cheaper connected model for review.” Enter sends; Shift+Enter adds a line.
3. The selected planning model inspects the repository read-only. It proposes features, bounded tasks, real dependencies, acceptance criteria, explicit verification commands, human test steps and an independent reviewer. Blocking questions stay in the conversation.
4. Inspect the inline plan. Model and reasoning selectors are editable for each task and reviewer. Reply in the composer to refine scope, order or checks. Smart Model Routing uses JEV when globally enabled; saved project role preferences and explicit instructions are supplied to planning/routing. Suggestions never start work by themselves.
5. Choose **Approve plan & Go**, or **Save for later**. Saved features appear in Ideas and are not dispatched by a run already in progress. **Go/Resume** queues saved ideas.

**Cmd+K → Describe project work / new feature**, or **Cmd+Option+N**, returns to the project composer. **Cmd+K → Project board** opens the board. Manual feature authoring and saved model defaults remain available in Workflow settings.

Verification commands run exactly as shown in the accepted plan. They run from a fresh worktree, so the plan should include reproducible dependency preparation where needed. Use terminating commands, not watch mode or development servers.

## Work, review, continue

The board groups features into **Ideas / Queued / Working / Review / Done**. Each feature shows its tasks, assigned/applied model and reasoning effort. Needs you filters features waiting for a decision; pending tool-permission cards are also shown in the project without opening their threads.

Independent tasks run in separate Git worktrees. The default limit is two; Workflow settings supports one to four. Task dependencies wait for committed checkpoints. Feature dependencies wait for local integration. A UI task can start with fixtures while an API task proceeds; a wiring task can wait for both.

The runner combines task commits in a separate candidate worktree, runs the accepted commands, then asks a read-only agent to review the result against every acceptance criterion. A passing review requires structured per-criterion evidence and an observed completed tool call. A failed check/review permits one automatic repair by default, then asks for help. Other independent work can continue.

Select a feature to inspect it in place:

- **Result:** requested outcome, worker handoffs, instructions to test, actual check exits/output, and review evidence.
- **Changes:** the candidate’s diff against its integration base.
- **Thread:** the related implementation, repair and review conversations.
- **Preview:** the combined candidate’s files and dev server. Only one app-managed server runs at a time; another workspace’s server must be stopped before switching.

Choose **Approve & merge** after testing, or describe the changes in **Keep working**. Approval applies to the exact reviewed commit and brief. A paused project retains approval until Resume. **Next needs you** moves to the next decision. Agent threads contain a direct link to their feature’s result and approval/feedback card.

Project messages and in-app notices announce results or blockers. Finishing all features triggers a final pass of the accepted check commands against the assembled target. “Complete” requires this pass.

## Git and run controls

Review before merging is the default. **Automatic local merging** is an explicit option in Workflow settings; pause before changing it. It still requires checks and independent review. Neither mode pushes or deploys.

Landing is serialized and fast-forwards the selected local target to the reviewed candidate. If the target advances, the candidate is combined with the new target and checked/reviewed again. Dirty project files are preserved and block landing. Switch to the target branch and commit/move your edits before continuing.

- **Pause:** no new tasks or merges; active tasks may finish.
- **Stop:** cancel active agents/checks; preserve worktrees, transcripts and checkpoints.
- **Reopen:** interrupted work is shown for inspection. The app does not automatically resend prompts or repeat merges. Inspect affected feature threads, provide Keep working feedback, and Resume.

Ordinary direct edits/Git actions are blocked on app-owned in-progress workspaces; use feature feedback so the checked candidate cannot silently change. Existing unrelated threads remain usable.

## Repository records and local evidence

Plans are first committed in isolated plan worktrees. They enter the project through feature integration, keeping an imported or dirty checkout untouched during planning.

```text
plan/
  PROJECT.md          # starter guide, only when absent
  STATUS.md           # tracking index, only when absent
  features/F-*.md     # approved briefs, task/model choices, dependencies, criteria
  runs/W-*.md         # accepted plan indexes
  tasks/F-*-T-*.md    # task handoffs on worker branches
  results/F-*.md      # combined handoffs and task checkpoint commits
```

Existing PROJECT.md, STATUS.md and repository instructions are preserved. Feature documents remain compatible with the earlier manual workflow; version 2 adds cross-feature dependencies and verification plans.

The app’s SQLite store owns live run state, conversations, exact checkpoint/approval references, actual check output and review evidence. Markdown handoffs are portable context, not proof that a particular commit passed or was merged. Importing the portable records does not reconstruct another machine’s running agents or approvals.

Current boundaries: no remote push/deployment automation, no simultaneous app-managed preview servers, and no automatic replay after interruption. Native visual QA and live-provider end-to-end testing were not performed in this implementation session; automated tests exercise real temporary Git worktrees/processes and mock ACP turns.
