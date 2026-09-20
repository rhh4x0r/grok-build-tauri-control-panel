# Conversational project work

Select a project to open **Conversation / Board / Git**. The workflow is optional for each repository. **New thread** still opens a normal conversation, whether project work is enabled or not. Importing a repository does not rewrite its instructions or adopt this workflow automatically.

## Start with a description

1. Select **Enable project workflow**.
2. Describe what you want in the project composer. For example: “Build a leaderboard, improve its UI, and fix score sorting. Use my frontend model for the UI, Codex for the backend, and a cheaper connected model for review.” Enter sends; Shift+Enter adds a line.
3. The selected planning model inspects the repository read-only. It proposes features, bounded tasks, real dependencies, acceptance criteria, explicit verification commands, human test steps and an independent reviewer. Blocking questions stay in the conversation.
4. Inspect the inline plan. Model and reasoning selectors are editable for each task and reviewer. Reply in the composer to refine scope, order or checks. Smart Model Routing has project inherit/on/off controls and requires JEV in settings. Saved role preferences and explicit instructions guide planning. JEV suggestions show their reason and require an explicit selection; they do not overwrite approved assignments or start work.
5. Choose **Start selected**, or **Save to backlog**. Select prerequisites along with dependent features; the app checks the selection. Resume only continues previously started work. Backlog features start through an explicit **Start this feature** action. Questions retain the current proposal; **Discard** explicitly removes it.

**Cmd+K → Describe project work / new feature**, or **Cmd+Option+N**, returns to the project composer. **Cmd+K → Project board** opens the board. **Cmd+K → Next project decision**, **Pause project**, and **Stop project** handle work without navigating between threads. Workflow settings exposes role defaults, reasoning effort, routing preferences, concurrency and merge policy. You can also send an exact command such as “pause,” “stop,” “resume,” or “status” in the project composer; these do not invoke a model.

Verification commands run exactly as shown in the accepted plan. They run from a fresh worktree, so the plan should include reproducible dependency preparation where needed. Use terminating commands, not watch mode or development servers.

## Work, review, continue

The board groups features into **Backlog / Queued / In progress / Needs you / Done**. Each feature shows its stage and a concrete waiting reason, including dependency names or execution capacity. The header distinguishes active work from waiting for decisions, paused work and stopped work. Needs you and Next decision include feature blockers, human review, tool permissions, planner questions and final-project check failures.

Independent tasks run in separate Git worktrees. The default limit is two; Workflow settings supports one to four. Task dependencies wait for committed checkpoints. Feature dependencies wait for local integration. A UI task can start with fixtures while an API task proceeds; a wiring task can wait for both.

The runner combines task commits in a separate candidate worktree, runs the accepted commands, then asks a read-only agent to review the result against every acceptance criterion. A passing review requires structured per-criterion evidence and an observed completed tool call. A code defect permits one automatic repair by default, then asks for help. Access, network, provider and interruption failures ask for the appropriate recovery action without consuming code-repair attempts. Other independent work can continue.

Select a feature to inspect it in place:

- **Result:** requested outcome, worker handoffs, instructions to test, actual check exits/output, and review evidence.
- **Changes:** the candidate’s diff against its integration base, grouped by file.
- **Thread:** the related implementation, repair and review conversations, including previous review attempts. Older agent attempts are nested under the feature in the sidebar.
- **Records:** the approved brief and portable record paths.
- **Models:** replacements for unfinished tasks, the reviewer and the repair writer. Completed outputs retain their original assignments.
- **Preview:** the combined candidate’s files and dev server. Only one app-managed server runs at a time. Reopening the same preview reuses it; **Switch preview** explicitly replaces another workspace’s server. Not-ready and failed states explain what is missing. The preview identifies the reviewed revision when available; a local preview does not prove external-service connectivity.

Choose **Approve & merge** after testing, or **Request changes**. For a task question, use **Answer question**. Approval applies to the exact reviewed commit and brief; when paused the button explicitly says merging waits for Resume. Project results and managed threads share the same inline decision card. Unsent project descriptions and feature feedback are saved separately and survive navigation or submission failure.

Recovery actions name the stopped stage: **Continue task**, **Retry combining**, **Retry checks**, **Retry review**, **Retry repair**, or **Retry merge**. Completed checkpoints stay saved. An unchanged candidate reuses passing checks when retrying its review. **Provide verification evidence** supplies claims for the independent reviewer; it does not bypass acceptance checks. An interrupted repair retains its original scope. Stale or duplicate decisions are rejected.

Project messages and in-app notices announce results or blockers. Finishing all started features triggers a final pass of the accepted check commands against the assembled target. “Complete” requires this pass. A final failure creates a project decision with Retry checks or a repair proposal; previously merged features remain Done.

## Git and run controls

Review before merging is the default. **Automatic local merging** is an explicit option in Workflow settings; pause before changing it. It still requires checks and independent review. Neither mode pushes or deploys.

Landing is serialized and fast-forwards the selected local target to the reviewed candidate. If the target advances, the candidate is combined with the new target and checked/reviewed again. Dirty project files are preserved and block landing. Switch to the target branch and commit/move your edits before continuing.

- **Pause:** no new tasks or merges; active tasks may finish. Stop remains available during those tasks.
- **Stop:** cancel active agents/checks; preserve worktrees, transcripts and checkpoints.
- **Reopen:** interrupted work is shown for inspection. The app does not automatically resend prompts or repeat merges. Use the stage-specific recovery card for interrupted work. Resume alone does not replay those attempts or start saved backlog. Valid merge approval is retained for Retry merge after checkout cleanup; a changed candidate or target still requires revalidation.

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

Current boundaries: no remote push/deployment automation, no simultaneous app-managed preview servers, and no automatic replay after interruption. Automated checks exercise temporary Git worktrees/processes and mock ACP turns. The isolated `BOMB_SMOKE=1` fixture covers the board, blocked/ready decisions and proposal render paths; it uses a fresh temporary database and repository. It does not establish visual/accessibility correctness or live-provider/JEV behavior. See the implementation log for executed validation and limitations.
