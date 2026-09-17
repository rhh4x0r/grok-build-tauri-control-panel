# Projects, workspaces and git: a deliberate flow

**Status:** proposal for review (2026-09-17). Nothing here is implemented yet.

## 1. What's wrong today

- The sidebar is a flat list of *threads*. The project name is a caption on each row, so ten threads across three projects read as noise, and a project with no threads is invisible.
- A thread silently owns a git worktree and a `thread/<id>` branch. That's the right isolation, but the user never chose it, can't see it as a thing, and can't put two conversations in the same working copy.
- **Sync** and **Land** are git verbs with the direction hidden. Sync = "merge the project branch *into* this worktree"; Land = "merge this worktree's branch *into* the project branch". Nobody can tell which is which from the words.
- Nothing knows about the remote. No ahead/behind, no PRs, no "main moved under you", no push.
- The agent's edits pile up uncommitted in the worktree. There's no checkpoint to diff against, revert to, or turn into a PR.

## 2. Mental model

Three nouns, nested, each with one job:

| Noun | What it is | Git meaning |
|---|---|---|
| **Project** | A repository you've opened | the checkout at its root, its `origin`, its default branch |
| **Workspace** | One unit of work: a feature, a fix, an experiment | a worktree + a branch (`bomb/<slug>`), based on the default branch |
| **Thread** | One conversation with one agent | none; it just runs *in* a workspace |

Rules that make it predictable:

1. Agents only ever run inside a workspace. Never on `main` directly. (An "Inline" workspace that *is* the main checkout exists for quick questions, and it is read-only for edits unless the user opts in.)
2. A workspace can hold many threads, sequentially or in parallel (two agents in one worktree is allowed but flagged; the default is one at a time).
3. Every agent turn that changed files ends in a **checkpoint commit** on the workspace branch, written by the agent ("Add login form validation"). Checkpoints are what you diff, revert, squash and ship.
4. The user's verbs are about *work*, not git: **Start**, **Update**, **Review**, **Ship**, **Archive**. Each maps to one git operation and says so in its confirmation.

## 3. Sidebar

```
▾ sovereign-crm                     main · 2 behind origin
    ● Sovereign CRM image concept   bomb/crm-image · PR #41 ✓   1h
    ○ Usage statistics review       bomb/usage-stats · 3 files   2d
    · Inline (main)                                              4w
▸ rh-theatre                        main
▸ claude-test                       main · uncommitted changes
+ Open project…
```

- **Project rows** are collapsible sections. The right side shows the default branch and its remote state (`2 behind origin`, `dirty`). Clicking the project name opens the **Project page** (§4). The `+` on hover starts a new workspace.
- **Workspace rows** show: status dot (idle / working / needs input / failed), name (slug from the first prompt, renameable), branch, one fact that matters (PR state, or `3 files` changed, or `conflicts`), time ago. Clicking opens the workspace's most recent thread.
- **Threads** are not in the sidebar by default. A workspace with more than one thread shows a small `▸ 3 threads` under it; expanding lists them. This keeps the sidebar about work, not conversations.
- **Inline (main)** is the always-present workspace that runs in the checkout itself. It is where "what does this repo do?" questions go. Edits there prompt: "Turn this into a workspace?".
- Filter box at the top (project, branch, text). Archived workspaces live under a collapsed "Archived" group per project.

## 4. Project page (center, when a project is selected)

Header: project name, path, default branch, `Fetch` (last fetched 3m ago), `Open in editor`, `Reveal`.

Sections:

1. **Active workspaces** as cards: name, branch, agent activity, `+12 −4 · 3 files`, PR chip (`#41 · checks passing`), last message excerpt, buttons `Open`, `Ship`.
2. **Main branch**: ahead/behind origin, uncommitted changes in the checkout (with a warning if an agent is about to run there), `Pull`.
3. **Recent threads** across workspaces (the old flat list, demoted).
4. **Merged / archived** (collapsed).

Empty state: "No workspaces yet. Describe what you want to do below" with the composer, which creates a workspace on send.

## 5. Workspace page (center, when a workspace is selected)

The thread view you have now, with a header that finally says where you are:

```
sovereign-crm › bomb/crm-image      ↑2 ↓0 vs main · 3 files changed      [Update] [Review] [Ship ▾]
```

- **Update** = bring the default branch into this workspace (`git merge main` by default, `rebase` as a per-project setting). If it conflicts, the workspace goes into *needs attention* and the composer gets a one-click "Ask the agent to resolve the conflicts" chip. This replaces **Sync**.
- **Review** opens the right pane on the **Changes** tab (§6).
- **Ship ▾** replaces **Land** with explicit choices:
  - *Merge into main locally* (what Land does today; only offered when there's no `origin`)
  - *Push and open pull request* (`git push -u origin bomb/crm-image`, `gh pr create --fill`; needs `gh`)
  - *Push only*
  - The menu shows what will happen ("Merge 4 checkpoints into main, then delete the workspace? You can squash first.").
- Thread tabs (or a dropdown) when the workspace has more than one thread; `+` starts another thread in the same worktree.

## 6. Right pane: Changes · Preview · Terminal

A tabbed pane docked right (the preview pane exists already):

- **Changes**: files changed vs the base branch, grouped by checkpoint. Click a file → diff with syntax highlighting. Buttons: `Revert file`, `Revert to checkpoint`, `Squash checkpoints`, `Commit message…`. Inline comments become the next prompt ("Fix the review comments in Changes").
- **Preview**: the dev server webview (done).
- **Terminal**: a shell in the workspace directory (later).

## 7. Git operations, precisely

Local:

- **Create workspace**: `git worktree add ~/.grok/worktrees/<project>/<slug> -b bomb/<slug> <base>`; base defaults to the fetched `origin/<default>` if a remote exists, else local `<default>`.
- **Checkpoint**: after a turn that touched files: `git add -A && git commit -m "<agent summary>"` with a `Bomb-Thread: <id>` trailer, so history says which conversation did what. Off by default for the Inline workspace.
- **Update**: `git fetch`, then `merge` (or `rebase`) `origin/<default>` into the workspace branch. Conflicts leave markers; the agent gets a prompt with the conflicted file list.
- **Ship → merge locally**: fast-forward or `--no-ff` merge into `<default>` only if the main checkout is clean; otherwise refuse with the reason.
- **Archive**: `git worktree remove` + keep the branch (deleting the branch is a second, explicit action; PR-merged branches offer "delete branch" automatically).

Remote:

- Detect `origin` and the default branch (`git symbolic-ref refs/remotes/origin/HEAD`, fallback `main`/`master`).
- Background `git fetch --prune` per project every few minutes and on focus; never pull automatically.
- Per workspace: ahead/behind vs `origin/<default>` and vs its own upstream; a PR chip via `gh pr view --json number,state,statusCheckRollup,reviewDecision` when `gh` is installed and authenticated (otherwise the chip says "install gh to track PRs").
- **Push and open PR** runs `gh pr create --fill --head bomb/<slug>`; the PR body is drafted by the agent from the checkpoints and shown for editing before submit.
- After the PR merges (poll or on focus), the workspace shows "Merged · archive?" and one click removes the worktree.

Safety:

- Never run destructive git without a confirm dialog that states the exact command.
- Never touch the user's main checkout while it has uncommitted changes.
- Every operation posts a system line into the thread ("Updated from origin/main (fast-forward, 12 files)").

## 8. Composer behaviour

- With a **project** selected and no workspace: sending creates a workspace named from the prompt and starts the thread there. A small `in: new workspace ▾` picker next to the model lets you choose *Inline (main)* or an existing workspace instead.
- With a **workspace** selected: sending adds a turn to the current thread; `⌘⇧N` starts a new thread in the same workspace.
- The mode/effort/model pickers stay as they are.

## 9. Data model changes

- New `workspaces` table: `id, project_root, name, slug, branch, path, base_ref, created_at, archived_at, pr_number, pr_url, last_checkpoint`.
- `sessions.workspace_id` (nullable for the Inline workspace).
- Migration: every existing thread with a worktree becomes a workspace named after the thread; threads without a worktree map to their project's Inline workspace.
- `projects` grows `default_branch`, `remote_url`, `merge_strategy` (merge|rebase), `checkpoint_commits` (bool), `last_fetch_at`.
- `grok_worktree` gains: `fetch`, `ahead_behind(base, branch)`, `default_branch`, `push(branch, set_upstream)`, `changed_files(base)`, `commit_all(message, trailers)`; a new `grok_git_remote` module wraps `gh`.

## 10. Rollout

1. **Vocabulary + header** (small): rename Sync→Update, Land→Ship menu, show branch + ahead/behind + changed-file count in the header. No schema change.
2. **Workspaces** (medium): table + migration; sidebar becomes project sections with workspace rows; project page.
3. **Checkpoints + Changes pane** (medium): auto-commit per turn, diff viewer, revert.
4. **Remote** (medium): fetch loop, PR chips, Push/PR via `gh`, merged-detection and archive.
5. **Terminal tab** (later).

Open questions for you:

- Should checkpoint commits be on by default? (Recommended yes; they make Review and Revert possible.)
- Merge or rebase for **Update** by default? (Recommended merge; agents handle merge conflicts more reliably than rebase sequences.)
- Do you want the Inline (main) workspace to allow edits at all?
