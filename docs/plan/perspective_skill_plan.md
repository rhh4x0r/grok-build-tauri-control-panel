# Perspective — a profile of how the user works with AI

Status: spec agreed, not built (2026-10-07).

**Goal:** after using Bomb Code for a while, the user runs one action that reads their threads
and writes a portable profile of how they think and work. They can hand it to any agent so it
works the way they would want. Sharing it is up to them: Bomb Code produces the file and
never installs or sends it anywhere.

## Decisions

| Question | Decision |
|---|---|
| Who runs the analysis | The user's logged-in agents (their subscriptions), not an API key |
| Voice | Third person ("Max prefers…") |
| Scope | Working style only, no project domain knowledge |
| Other people | **Never named.** Distilled into how the user works with and thinks about other people (see §2.8) |

## 1. Source material

All threads in Bomb Code's database: native threads and those imported from Claude Code and
Codex.

On 2026-10-07 that was about 270 threads and 5,336 user messages (≈2.3 MB, ≈600k tokens).

The strongest signal is the user's own messages, and how they react to the reply before each
one: what they push back on, approve, correct or ask for next. Approval and denial decisions,
model switches and the tools used add context.

## 2. Output: a skill folder

The folder is `perspective/`, written so Claude Code can load it as a skill and plain enough
for any agent.

- **`SKILL.md`**
  - Frontmatter: `name`, plus a `description` saying when to use it ("when acting on Max's
    behalf or deciding how to do work for him").
  - A short third-person profile, in the sections below.
- **`evidence.md`**
  - Each claim with 2–3 short, de-identified quotes, the thread date, and how often it showed
    up.
  - This makes the profile checkable rather than invented.

### Sections of `SKILL.md`

1. **Values and quality bar**
   - What "done" means (verified in the real app, not just tests).
   - What they protect: their data, unasked-for changes, secrets.
   - What they won't trade away.
2. **How they decide**
   - When they want a plan first versus "just build it".
   - Scope versus speed.
   - When they ask before committing, pushing or deleting.
   - How they choose between options.
3. **Questions they ask**
   - Recurring checks that an agent should anticipate, e.g. "is it pushed?", "what's it
     using?", "how does that compare?", "why did that happen?".
4. **How they communicate**
   - Message style (short, dictated, screenshots instead of descriptions).
   - What they want back (a short answer first, then the details).
   - How they phrase requests and corrections.
5. **Tools and stack**
   - Languages, frameworks, services.
   - Which models they use for which jobs.
   - Git habits and the environments they work in.
6. **Reactions**
   - What earns "way better" and what earns "it looks worse".
   - Recurring frustrations and red lines.
7. **How to work for them**
   - A concrete do / don't list derived from 1–6.
8. **Working with others**, with no names, ever. How they:
   - **Collaborate:** split ownership, wait for or build on others' work, coordinate before
     big design decisions.
   - **Think about other people's stake:** what they check with others versus decide alone,
     and how they treat work someone else owns.
   - **Communicate outward:** what they share, in what form (plans, summaries, links,
     docs), and how polished it is.
   - **Look for inputs:** feedback, reviews, references, examples, other people's code or
     research, and how those feed the next step.
   - **Handle dependencies:** parking work until someone else's piece lands, and handing
     work off.

   People appear only as roles ("a collaborator", "a client", "a teammate") or not at all. The
   section describes patterns, not relationships.

## 3. Pipeline

1. **Gather (local, no AI).** For each thread, collect:
   - the user's messages;
   - a short excerpt of the reply each one responds to;
   - project, models used, approvals and denials.

   Drop the noise:
   - mock/test threads;
   - automated prompts (recovery packs, Foundry stage prompts, "Implement the plan above");
   - leftover import wrapper text.
2. **Scrub before any AI sees it.**
   - Remove secrets with key and token patterns.
   - Replace personal names and email addresses with roles, using context ("my
     collaborator", "the client"), or `[person]`.
   - Remove phone numbers, addresses and account IDs.
3. **Per-thread notes (AI, in parallel).** Each thread, or a batch of small threads, becomes
   observations in a fixed JSON format: section, claim, quote, date, strength.
   - Runs headless through the user's logged-in agent: a mid-size model, read-only, no tools
     needed.
   - Cached per thread, keyed by thread id + last message.
4. **Merge (AI).**
   - Combine all notes into the profile with a strong model.
   - Patterns that recur count for more than one-offs.
   - Newer behavior wins conflicts, and contradictions are stated, not hidden.
5. **Final scrub.** Check the finished files for names, secrets and quotes that identify
   someone. Anything found is replaced, then re-checked.
6. **Review.** The user reads it in Bomb Code, edits or deletes lines, then exports.

**Incremental updates:** later runs only process new or changed threads, then re-merge.

## 4. Interface

A **Perspective** page in Settings:

- **Before the first run:**
  - "Build my perspective".
  - Scope picker: all projects, selected projects, or a date range; exclude specific threads.
  - The agent and model to use.
  - Estimated tokens and time.
- **While running:** "Analyzing threads 84 / 231". Background, cancellable; the user keeps
  working.
- **When done:**
  - The profile by section, with a "show evidence" link on each claim.
  - Inline editing and deleting.
  - **Export**: save as a Claude Code skill (user picks the location, e.g.
    `~/.claude/skills/perspective/`), copy as Markdown, or save to a folder.
- **Later:** "Last updated Oct 7 · 12 new threads since" with an **Update** button.

## 5. Estimates

- **Input:** ≈600k tokens of user text, plus reply excerpts. A full run is a few million tokens.
- **Time:** about 10–20 minutes for the first run with parallel notes. Updates take a minute or
  two.
- **Cost:** counts against the user's agent subscriptions. The estimate is shown before
  starting.

## 6. Build order

1. Gather + scrub (core, local), with tests on real-shaped fixtures. This includes name
   detection.
2. Per-thread notes through a headless agent run, with the observation format and per-thread
   cache.
3. Merge + final scrub. This writes `SKILL.md` and `evidence.md`.
4. Settings page: run, progress, review/edit, export.
5. Incremental update.

## 7. Risks

- **Name scrubbing is never perfect.** That's why there's a second pass on the output and a
  user review before export.
- **Flattering or generic output** ("Max values quality"). Prevent it by requiring evidence:
  a claim needs quotes from at least two threads, or it's dropped.
- **Over-fitting to one intense week.** Weight by how many threads a pattern appears in, not by
  message count.
