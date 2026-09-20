# Conversational project work

Select a project to open **Project / Board / Git**. The workflow is optional for each repository. **New thread** still opens a normal conversation, whether project work is enabled or not. Importing a repository does not rewrite its instructions or adopt this workflow automatically.

## Start with a description

1. Choose **New project** or **Open project**, or select an existing project in the unchanged left sidebar. The project opens in the main pane. If no agent is connected, **Connect an agent** opens the existing settings. This is a provider connection, not a separate Bomb Code account.
2. Select **Plan this project** and describe what you want in the project composer. Enter sends; Shift+Enter adds a line. The same composer plans a new feature in an existing project.
3. The planning model inspects the repository read-only and guides a short conversation. It can ask one focused question, with suggested answers or free text, while retaining the evolving plan. **Help me decide** asks it for guidance. Clicking a suggestion preserves an unsent composer draft for review rather than replacing it.
4. **Your plan** shows outcomes, decisions and assumptions. Expand the plan to inspect full scope, dependencies, task/model assignments, reasoning effort, acceptance criteria, check commands and test steps. Reply to revise the plan; no workers start during planning. Pending questions must be answered before acceptance.
5. Choose **Start building**, or **Save for later**. Select prerequisites along with dependent features; the app checks the selection. Acceptance is bound to the exact displayed plan and selection. Resume continues previously started work; saved features need an explicit start. **Keep planning** returns to the conversation; **Discard** removes the proposal.

**Agents & settings** exposes role defaults, routing, concurrency and merge policy. Smart Model Routing requires JEV in settings. Saved role preferences and explicit instructions guide planning. JEV suggestions require explicit selection; they do not overwrite approved assignments or start work.
**Cmd+K → Describe project work / new feature**, or **Cmd+Option+N**, returns to the project composer. **Cmd+K → Project board** opens the board. **Cmd+K → Next project decision**, **Pause project**, and **Stop project** handle work without navigating between threads. Workflow settings exposes role defaults, reasoning effort, routing preferences, concurrency and merge policy. You can also send an exact command such as “pause,” “stop,” “resume,” or “status” in the project composer; these do not invoke a model.

Verification commands run exactly as shown in the accepted plan. They run from a fresh worktree, so the plan should include reproducible dependency preparation where needed. Use terminating commands, not watch mode or development servers.

## Work, review, continue

The project overview and board group outcomes into **Saved for later / Queued / In progress / Ready to review / Questions / Blocked / Added**. Compact cards are the default; older conversation and operational details expand on demand. Each feature shows its stage and a concrete waiting reason, including dependency names or execution capacity. The header distinguishes active work from waiting for decisions, paused work and stopped work. The attention count and Next decision include feature blockers, human review, tool permissions, planner questions and final-project check failures.

Independent tasks run in separate Git worktrees. The default limit is two; Workflow settings supports one to four. Task dependencies wait for committed checkpoints. Feature dependencies wait for local integration. A UI task can start with fixtures while an API task proceeds; a wiring task can wait for both.

The runner combines task commits in a separate candidate worktree, runs the accepted commands, then asks a read-only agent to review the result against every acceptance criterion. A passing review requires structured per-criterion evidence and an observed completed tool call. A code defect permits one automatic repair by default, then asks for help. Access, network, provider and interruption failures ask for the appropriate recovery action without consuming code-repair attempts. Other independent work can continue.

Select a feature to inspect it in place:

- **Result:** requested outcome, test instructions and the shared decision card. Open the preview here, or expand completed checks and review evidence. Failures and missing evidence remain visible. A question or blocker appears before testing details when it needs an answer.
- **Changes:** the candidate’s diff against its integration base, grouped by file.
- **Activity:** implementation, repair and review conversations, actual branches and checkpoints, including previous attempts. Older agent attempts remain nested under the feature in the sidebar.
- **Details:** agents and assignments for unfinished work, plus approved briefs and portable record paths.

The inline preview uses the combined candidate’s files and dev server. Only one app-managed server runs at a time. Reopening the same preview reuses it; **Switch preview** explicitly replaces another workspace’s server. Not-ready and failed states explain what is missing. Preview and diff results are discarded if the inspected revision changes. A local preview does not prove external-service connectivity.

Choose **Approve & add** after testing, or **Request changes**. The card names the local destination; approval does not push or publish. A paused project instead offers **Approve for later**, followed by **Resume project**. Approval applies to the exact displayed reviewed commit and brief; changed decisions are rejected. For a task question, use **Answer question**. Project results and managed threads share the same decision card. Unsent project descriptions and feature feedback are saved separately and survive navigation or submission failure.

Recovery actions name the stopped stage: **Continue task**, **Retry combining**, **Retry checks**, **Retry review**, **Retry repair**, or **Retry merge**. Retry and start actions explain when they resume the entire project, allowing other ready work to continue. Completed checkpoints stay saved. An unchanged candidate reuses passing checks when retrying its review. **Provide verification evidence** supplies claims for the independent reviewer; it does not bypass acceptance checks. An interrupted repair retains its original scope. Stale or duplicate decisions are rejected.

Project messages and in-app notices announce results or blockers. Finishing all started features triggers a final pass of the accepted check commands against the assembled target. “Complete” requires this pass. A final failure creates a project decision with Retry checks or a repair proposal; previously merged features remain Added. Added cards distinguish local integration from the final project checks still required.

## Git and run controls

Review before merging is the default. **Automatic local merging** is an explicit option in Workflow settings; pause before changing it. It still requires checks and independent review. Neither mode pushes or deploys.

Landing is serialized and fast-forwards the selected local target to the reviewed candidate. If the target advances, the candidate is combined with the new target and checked/reviewed again. Unrelated untracked files (including older planning records) stay in place and do not block landing. Tracked or staged edits still block with the affected paths listed. Git also refuses to overwrite colliding untracked or ignored files. The app does not silently commit unrelated work. The project checkout must be on the target branch.

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
