# Foundry

For everyday use, type your request in the chat composer and click **Run through Foundry**. The selected provider/model rewrites it automatically and places the improved prompt back in the composer. Review, edit, and Send when ready; **Undo** restores your original. Attachments stay attached. Errors leave your draft intact, and a late result never replaces a changed draft or another thread's input.

The sidebar **Foundry** screen remains available for advanced contract, graph and saved-skill editing. You do not need to open it to improve a prompt.

## Author a contract

Enter a request and choose Fast Draft or Full Project. The local compiler works without a provider call. Choose the operating mode, edit individual sections, and Apply changes. Structured sections (sources, assumptions, roles and phases) use JSON; ordinary lists use one item per line. Source entries preserve their upstream roles and metadata; adding a source does not fetch or verify it.

Optional provider refinement uses the selected provider/model in a fresh, read-only session. Review the before/after text and accept or discard the result. It does not execute the contract. Unsaved drafts and refinement results stay out of Foundry's durable library until Save or Run. Once saved, applied edits create local revisions; restoring an old revision creates a new revision.

**Use prompt** inserts the compiled contract into the composer for review. It never sends automatically.

## Build a skill loop

Use the five adapted templates or seed stages from contract phases. The canvas supports dragging, panning and zooming; the ordered view provides a keyboard-accessible alternative. Moving a card changes only its position. Reorder controls change execution order.

Select a stage to edit its instructions and exit criteria, change its kind/role, choose a provider/model override, and add dependencies or explicit return edges. Models come from the app's discovered provider catalog. If a stage has several return edges, select which one `needs_revision` should follow. Advanced bindings, links and limits can also be edited as JSON.

## Run and review

Run uses the current thread's folder, or creates a thread in an isolated branch for the selected project. Without a project it uses the temporary-chat folder. A folder has one active Foundry run at a time. Each stage gets a fresh ACP session; approvals appear in the parent thread. Independent reviews use read-only sessions with explicit artifact references and app-observed tool events, without the author's summary. Existing tool permissions still apply.

Stages must return the documented structured result with evidence for every exit criterion. These are agent-reported findings, not proof that a test passed; the run inspector separately shows observed tool events. Invalid results block the run. A revision result follows its configured return edge and invalidates downstream accepted work. Defaults are two returns per edge and three attempts per node across the run.

Human gates pause for explicit approval of the displayed criteria. Pause lets an active stage finish but prevents the next from starting. Stop cancels the active session. Resume/retry is explicit, including after restart. Extending limits requires a separate user action. The thread's **Stages & controls** button opens Runs.

## Files and compatibility

Import ProjectContract 1.0.0, SkillGraph 0.1.0, or Bomb Code's versioned Foundry envelope. Export creates a new package directory containing the canonical JSON, compiled Markdown, instructions, stage references, limitations and provenance. Existing files are never overwritten. Local library/revisions and run state live in `foundry.db` under the app sessions directory.

This release runs stages sequentially. Provider-folder installation, scheduled/parallel loops, and Word/PDF exports are not included. Templates are generic adaptations of [Prompt Foundry](https://github.com/jedisherpa/prompt-foundry); original provider metadata is preserved separately from Bomb Code refinement attribution.
