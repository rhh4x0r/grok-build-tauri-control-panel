# Cross-agent subagents ("helpers")

**Status:** plan, not started. Builds on native subagent support (commits `6aa7d0c`, `79070dc`).

## Goal

From any thread, a person can say:

> Spin up two subagents, one on Codex with Astra and one on Claude with Opus 5.5, and have them tell me what's in this repo.

and get exactly that: two helpers running at once on different agents and models, each visible
and openable like any subagent, whose reports go back to the agent that asked, which then answers.

Today a thread's subagents always run on the thread's own agent: Claude's on Claude models,
Codex's on OpenAI models, Grok's on Grok models. Claude Code can pick a Claude model per
subagent, but it can't run Astra, and Codex can't run Opus. Bomb Code already runs all
three agents, so Bomb Code is where cross-agent work has to happen.

## How it works

1. **Bomb Code gives every top-level thread's agent a small MCP server, `bomb`.** It is attached
   when the session starts, the same way other MCP servers are (`grok_mcp` injection). Its
   tools act on behalf of that one thread.
2. **The agent calls those tools when asked.** Nothing about ACP changes: the person's words reach
   the agent as usual, and the agent decides to call `start_helper` the way it would call any tool.
3. **A helper is a Bomb Code thread on the requested agent and model**, saved as a subagent of the
   asking thread (the `parentThread` link and `subagent` metadata that native subagents use, plus
   `kind: "helper"`, `agent` and `model`). Its prompt is the task.
4. **Results go back to the asking agent** through `wait_helpers`, which returns each helper's
   final message (its report), and **to the person** through the same UI as native subagents.

### Tools on the `bomb` MCP server

| Tool | What it does |
|---|---|
| `list_agents()` | Agents and models this machine can run right now: logged-in backends only, with each model's id and display name (so "Astra" or "Opus 5.5" resolve). |
| `start_helper({ agent, model, task, name? })` | Starts a helper and returns its id at once. `model` accepts an id, alias or display name. The helper runs in its parent's mode (see Rules). |
| `wait_helpers({ ids, timeout_secs? })` | Waits until those helpers finish or the timeout passes (default 120 s, capped below the agents' MCP tool timeouts). Returns each one's status and, once finished, its report. Call again to keep waiting. |
| `helper_status({ ids })` | Status, step count, and the latest line of each, without waiting. |
| `message_helper({ id, text })` | A follow-up to a finished helper (the same as writing to a thread). |
| `stop_helper({ id })` | Stops one. |

Starting returns immediately and waiting is separate, so a long helper never holds one tool
call open past an agent's timeout, and the parent can start several helpers before waiting on
all of them.

### Where the MCP server runs

A small stdio binary, `bomb-mcp`, shipped next to `bomb_app` and `bombd` (the way `bomb-dictate`
is). The core starts it per session with the thread id and a short-lived token. It connects to
the core over the existing core socket (`bomb_server::core`) and calls new core methods
(`helper_start`, `helper_wait`, …). The token ties every call to one parent thread, so one
thread's agent can't start or read another thread's helpers. The same binary works for threads
on a paired server, where `bombd` starts it.

## Rules

- **Same mode as the thread.** A helper runs in its parent's approval mode at the time it starts
  (Plan, Ask or Auto), never a more permissive one. Always-approve is never chosen for a helper
  by Bomb Code; it carries over only when the person put the parent in it themselves.
- **Edits happen on a branch.** In a parent that can't edit (Plan), helpers read the parent's
  folder. In one that can, each helper gets its own worktree (the existing thread isolation), so
  helpers can't overwrite each other or the parent; its work comes back as a branch to review
  and merge, like any thread's.
- **One level only.** Helpers don't get the `bomb` server, so a helper can't start helpers.
- **Limits:** at most 4 helpers running per thread and 8 per thread in total, both configurable.
  `start_helper` explains the limit instead of failing silently.
- **Stopping cascades.** Stopping the parent's turn stops its running helpers.
- **Questions reach the person.** A helper's approval card shows in the helper's transcript, and
  the parent's sidebar status and the Subagents tab show "a helper needs you" until it's answered.
- **Only logged-in agents.** `list_agents` and `start_helper` refuse agents that aren't set up,
  with the sign-in hint the Services panel shows.

## What the person sees

The same places as native subagents (the right panel's Subagents tab, the step in the parent's
transcript, the phone's cards and read-only transcripts), plus:

- each helper's **agent mark and model** ("Codex · Astra", "Claude · Opus 5.5");
- **"needs you"** when a helper waits on an approval;
- each helper's **usage on its card** ("Codex · Astra · 42k tokens"), where the agent reports it.

Native subagents should show their model too. Claude's adapter doesn't send it today (the
`subagent_spawned` update has no model, and the Agent tool call that carries it is consumed by
the adapter). Ask upstream to add it to `subagent_spawned`, or read it from the subagent's own
updates if it appears there.

## Phases

0. **Spike (half a day).** Confirm each agent (Claude, Codex, Grok) calls a stdio MCP server that
   Bomb Code attaches through ACP `session/new`, with a toy `echo` tool. Measure each agent's MCP
   tool timeout. Confirm `list_agents` model names match what people say ("Astra", "Opus 5.5").
1. **Core helpers.** `helper_start/wait/status/message/stop` in `bomb_core`, built on
   `start_session` and `send_prompt`. Records use the subagent metadata with `kind: "helper"`,
   `agent` and `model`. Includes mode capping, limits, and cascade-stop. Tests with the mock
   agent: start two helpers, wait, read their reports, check the parent link.
2. **`bomb-mcp`.** The stdio server, tokens, and attaching it to every top-level session (never to
   helpers). Tests: a mock agent calling the tools over MCP.
3. **UI.** Agent mark and model on subagent rows, the Subagents tab and phone cards; "needs you";
   usage.
4. **Paired servers.** `bombd` starts `bomb-mcp` for server threads; helpers run on the server.
5. **Polish.** Settings for the limits, a short "helpers" line in each agent's instructions so they
   know the tools exist, and a /help entry with example prompts.

## Decisions (2026-10-08)

1. **Mode:** helpers follow the parent thread's mode (not read-only by default).
2. **Edits:** an editing helper works on its own branch, reviewed and merged like a thread.
3. **Cost:** each helper's usage shows on its card.
4. **Grok's native subagents:** shown as subagent cards now, from the `spawn_subagent` tool call
   alone (status, step count, duration from its `<subagent_meta>`). They can't be opened, since
   Grok doesn't send their steps; switch to real subagent sessions if xAI adopts the ACP extension.
