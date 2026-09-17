# Branches and Changes

The thread header shows the checked-out branch, uncommitted file count, and a Direct checkout badge for shared locations. Its menu separates comparison against the local default branch from synchronization with the upstream remote branch, and links to Changes and the working folder. The active sidebar row mirrors the actual branch and indicates uncommitted files.

For a new thread, **Work in** offers:
- **New branch**: a separate working copy, with an explicit local base branch selector.
- **Current checkout**: edit the project's currently checked-out branch directly. The shared-working-copy hint shows existing conversations.
- **Existing branch**: use its already checked-out working copy, or attach it to a new working copy without switching the project's current branch.

**Read-only / Can edit files** is independent of location. Approval mode remains a separate control. Shared locations are guarded against overlapping conversations and cannot be removed via Archive or rewritten via restore/squash. Existing read-only conversations retain their permissions when a writable conversation uses the same folder. Shared checkouts do not automatically commit after turns.

Changes refreshes while visible. **Uncommitted** lists edits since HEAD; **Branch changes** compares the working copy with the merge base of the local default branch. Select files in Uncommitted, inspect a filename's diff, then Commit selected files with an editable message. Unselected staged changes are preserved. New text files have inline previews; binary and large new files are explicitly identified instead.

The primary action follows the current state: Commit selected files, Push branch, Create pull request, or local merge for a project without a remote. A PR dialog shows its source, editable target, title and description before publishing. **More** contains local merge, push-default-branch, update, commit history, and unpublished-commit squash. History contains explicit restore confirmations for isolated editable threads.

Local merge is available with or without a remote, requires both working copies to be clean, and uses the actual default branch name. It never pushes automatically. After merge, the default branch's unpublished commits are shown separately, with a Push action. No force-push action is exposed.

The native GPUI file list, diff view and explicit action confirmations adapt assistant-ui File tree, Code diff and Approval card patterns; they do not embed a web runtime.
