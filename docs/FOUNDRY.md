# Foundry

For everyday use, type your request and click **Enhance Prompt**. The sparkle button sits immediately to the right of attachments. A compact panel asks for **Fast Draft / Full Project**, **work type**; the target agent follows the currently selected provider. Work type requires an explicit choice rather than a silent guess. Expand **Add context** to add approval notes and sources classified as source of truth, supporting context, historical plan, reference implementation, or unverified assumption.

Click **Enhance Prompt** to create a structured contract with the selected provider/model. The intended target updates automatically when you change providers in the composer. Review the contract in the composer, edit, and Send when ready; **Undo** restores your original. Chat attachments stay attached but are not inspected by prompt generation; files selected through the panel's source picker follow the source inclusion rules below. Errors preserve the draft, and late results never replace a changed draft or another thread's input.

The choices and contract headings follow the website's [intake schema](https://github.com/jedisherpa/prompt-foundry/blob/main/src/lib/schema/project-contract.ts). Bomb Code generates through the selected ACP model; it does not call the website or promise identical wording to its deterministic local compiler. Confirmed target, depth, work type, original request, source roles and approval notes are retained in the generated output.

The review-loop panel’s **Advanced details** action opens Foundry for advanced contract, graph and saved-skill editing. Everyday actions live in the composer; there is no standalone Foundry sidebar button.

## Run directly from the composer

**Run with review loop**, beside Enhance Prompt, uses the current draft as the request and starts Plan → Build and verify → Independent review → Approve result. Review findings return to Build and verify within the existing retry limits. The selected provider/model and approval mode are preserved; Plan mode remains read-only. Planning and independent review sessions are read-only regardless of the selected mode.

The action uses the current thread, creates an isolated thread for a selected project, or creates a temporary chat when no project is selected. Use the loop panel above the composer to inspect stages, pause, stop, resume, approve, or request changes. **Advanced details** opens the full run inspector. Startup errors preserve the draft. Image attachments are not passed to loop stages; the UI asks you to send them in a normal chat or include file paths instead. Explicit Foundry sources and approval notes are included.

## Author a contract

Enter a request and choose Fast Draft or Full Project. The local compiler works without a provider call. Choose the operating mode, edit individual sections, and Apply changes. Structured sections (sources, assumptions, roles and phases) use JSON; ordinary lists use one item per line. Source entries preserve their upstream roles and metadata; adding a source does not fetch or verify it.

Optional provider refinement uses the selected provider/model in a fresh, read-only session. Review the before/after text and accept or discard the result. It does not execute the contract. Unsaved drafts and refinement results stay out of Foundry's durable library until Save or Run. Once saved, applied edits create local revisions; restoring an old revision creates a new revision.

**Use prompt** inserts the compiled contract into the composer for review. It never sends automatically.

## Build a skill loop

Use the built-in templates or seed stages from contract phases. The canvas supports dragging, panning and zooming; the ordered view provides a keyboard-accessible alternative. Moving a card changes only its position. Reorder controls change execution order.

Select a stage to edit its instructions and exit criteria, change its kind/role, choose a provider/model override, and add dependencies or explicit return edges. Models come from the app's discovered provider catalog. If a stage has several return edges, select which one `needs_revision` should follow. Advanced bindings, links and limits can also be edited as JSON.

## Run and review

Run uses the current thread's folder, or creates a thread in an isolated branch for the selected project. Without a project it uses the temporary-chat folder. A folder has one active Foundry run at a time. Each stage gets a fresh ACP session; approvals appear in the parent thread. Independent reviews use read-only sessions with explicit artifact references and app-observed tool events, without the author's summary. Existing tool permissions still apply.

Stages must return the documented structured result with evidence for every exit criterion. These are agent-reported findings, not proof that a test passed; the run inspector separately shows observed tool events. Invalid results block the run. A revision result follows its configured return edge and invalidates downstream accepted work. Defaults are two returns per edge and three attempts per node across the run.

Human gates pause for explicit approval of the displayed criteria. Pause lets an active stage finish but prevents the next from starting. Stop cancels the active session. Resume/retry is explicit, including after restart. Extending limits requires a separate user action. The thread's **Advanced details** button opens Runs.

## Files and compatibility

Import ProjectContract 1.0.0, SkillGraph 0.1.0, or Bomb Code's versioned Foundry envelope. Export creates a new package directory containing the canonical JSON, compiled Markdown, instructions, stage references, limitations and provenance. Existing files are never overwritten. Local library/revisions and run state live in `foundry.db` under the app sessions directory.

This release runs stages sequentially. Provider-folder installation, scheduled/parallel loops, and Word/PDF exports are not included. Templates are generic adaptations of [Prompt Foundry](https://github.com/jedisherpa/prompt-foundry); original provider metadata is preserved separately from Bomb Code refinement attribution.

## Choosing where to send the generated prompt

Submitting a new thread checks the destination before clearing the composer. If setup is missing, choose an existing folder, create a new project with the name/location picker, or use a temporary chat. Initialize Git is available for an existing folder; it creates an empty first commit without committing existing files. Existing uncommitted files must be committed separately before they appear in an isolated branch. After setup, press Send again. A thread-start failure restores the unsent prompt and attachments.

## Sources for enhancement

Under Add context, choose **Attach files**, **From memory**, or **Add source** for a manual reference. The memory picker searches saved Bomb Code notes across scopes; only entries you select are included. UTF-8 text files up to 18 KB are included as bounded snapshots. Larger files and binary formats are labeled as path references with their contents explicitly not included. Source rows show whether content was included, retain authority-role controls, and can be removed before generation. Generation is disabled while selected files are being added.

## Review loops in the conversation

The thread keeps a stage strip above the composer with explicit Working, Awaiting your approval, Paused, Needs attention, Stopped, and Completed states. Select a stage for its provider/model, duration, findings, evidence and file links. Completed stage replies collapse into readable cards; `<foundry-result>` envelopes are hidden during streaming and on restored conversations, and remain available under Technical details.

At a human gate, **Approve result** accepts the displayed run revision. **Request changes** sends your feedback back to the most recent build/revision stage and the subsequent independent review, invalidating downstream acceptance. The same attempt limits apply; exhausted limits require explicit extension. Stale approvals are rejected. The completed card records approval, keeps review limitations visible, and offers Preview and View changes. Approval does not commit, merge, or deploy.

Visual patterns adapt assistant-ui's Task card, Tool timeline and Approval card into native GPUI controls; no embedded React runtime is required.
