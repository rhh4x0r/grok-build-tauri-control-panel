# Optional project features

For conversational planning, the kanban board and automatic project execution, see [PROJECT_WORKFLOW.md](PROJECT_WORKFLOW.md). The manual workflow below remains available under Workflow settings.

Open a project, then use **⌘K → New feature** (or **⌘⌥N**). **Project features** in the command palette and **Features** on the project overview open the board. This is a native center pane using the same card frame/provider identities as Smart Model Routing; it does not open another window.

Feature tracking starts **off for every project**. Importing a repo or opening the board creates no records or model sessions. Enable it explicitly. Ordinary threads always remain available. Turning tracking off preserves documents and existing threads, including running work.

## Quick workflow

1. Enable tracking and enter a feature name and desired outcome. Include what done means and any scope limits. Save an idea without tasks, or keep one Build task for a small feature.
2. Add Frontend, Backend, Review, or other task cards as needed. Each card has a scope, model, and reasoning effort. The Frontend starter explicitly allows provisional data so layout work need not wait for an API.
3. Enter optional routing preferences directly in the editor, such as “Fable for frontend, Codex for backend, inexpensive Grok for review.” **Smart Model Routing** compares connected models using these preferences, saved role assignments, and global routing guidelines. It respects the source thread's routing toggle. Proposals require **Use suggestion**; they never launch tasks. Manual choices work without JEV.
4. **Use assignments as project defaults** saves the chosen model/effort for each role plus the routing preferences. Different tasks with the same role must agree before a role default can be saved. Existing assignments are not changed by updating defaults.
5. **Save feature**, then **Start task** on independent tasks. Each gets a separate branch/worktree and an ordinary thread with Ask permissions. Starts are explicit; the app does not automatically run the entire feature. You can start another task while the first model is working. **Attach existing thread** tracks an existing thread without changing its model, permissions, or workspace.
6. Open task threads for approvals, follow-up prompts, model switches, the existing review-loop controls, and Git actions. After checking a clean checkpoint, choose **Ready for review** on the board. This records an exact commit and brief revision, not an approval to merge.
7. A task with explicit dependencies requires fresh Ready for review checkpoints. On Start, their recorded commits are merged into the new task worktree. Conflicts retain that workspace/thread and stop prompt dispatch. Resolve them in the thread; the source branches and target checkout are untouched.
8. A **Review** task needs one selected review target. It starts a read-only worktree at that checkpoint using its chosen reviewer model and effort. It does not certify the combined feature unless its selected target is already the combined implementation. A moved target aborts dispatch rather than reviewing a different revision silently.

A task's accepted launch prompt/model/effort is retained even if the feature brief changes while its provider connects. Failed startup retains the prompt for retry. Changes to an already-linked task's brief do not resend instructions; continue in its thread to apply the revision. The board labels its launch assignment; the thread composer follows provider-reported reasoning when selected, including saved values across restarts.

Feature **Open / Done / Archived** labels are user-controlled tracking labels. They do not merge, grant approval, deploy, or stop running tasks. The board defaults to Open features; switch the filter to see everything.

## Project records

```text
plan/
  PROJECT.md                       optional starter guide
  features/F-<id>.md                saved feature and task intent
  tasks/F-<id>-T-<id>.md            handoff in each writer's task branch
  STATUS.md                        optional checkpoint snapshot
```

- **Save feature** writes only its Markdown file in the selected project checkout. It does not commit the file. Include project records in your normal Git workflow when sharing the project.
- **Create project guide**, under the board's settings icon, creates PROJECT.md only when absent. Fill in the product purpose, source map, existing instruction links, target branch and check commands. Existing PROJECT.md, AGENTS.md, CLAUDE.md and unrelated records are preserved.
- A writer's task document is created in its isolated worktree before sending the prompt. It contains the brief and space for checks/assumptions/handoff. Normal thread checkpoints include it alongside the code. Read-only reviews do not create dirty task documents.
- **Save status snapshot** explicitly generates STATUS.md with its timestamp, user tracking labels and revalidated local checkpoint references. It distinguishes local work from merged/deployed code and refuses to overwrite an unrelated STATUS.md.
- Feature Markdown owns the brief and task intent. A versioned HTML comment carries stable IDs, role/model/effort metadata and dependencies; visible `#` / `##` headings own the feature/task titles. The prose following each task marker is its editable brief. Preserve the markers when editing outside the app. Reload imports changes. An optimistic revision check prevents Save from overwriting an externally edited brief; discard the local draft and reload to adopt the disk version.
- SQLite stores project enablement/preferences, thread links, accepted launch prompts, reported launch assignments, local tracking labels and exact Ready for review evidence. Importing committed feature files elsewhere restores the planned work, not imaginary live processes or approval.

No agent is assigned to maintain a shared status file while others work. Shared file writes are explicit app actions and respect the existing workspace ownership guard. Drafts are retained across project navigation during the current app session; save the feature to keep it across app restarts.

## Prompt Foundry and current limits

**Enhance brief** uses the existing Foundry Fast Draft generator and currently selected provider. It shows a proposed replacement inline, requires an explicit Apply, and supports Undo. Feature creation does not require this call or a planning interview. The existing Run with review loop remains available in each task's thread; the new Review task provides an explicit independent reviewer assignment.

This release provides manual feature/task execution, isolated worktrees, dependency checkpoint combination, and selected-checkpoint review. It does not add an automatic scheduler, background task decomposition, a feature-wide merge queue, deployment automation, or multiple concurrent dev-server previews. The existing single-server preview and Git shipping controls retain their existing scope. Review/test the combined behavior before using those shipping controls. Upstream Foundry format conversion is not added; the existing supported Bomb Code Foundry format remains in use.
