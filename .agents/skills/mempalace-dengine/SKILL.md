---
name: mempalace-dengine
description: Save cross-session d-engine ecosystem decisions/design records to the mempalace MCP — when to save, dedup check, wing/room naming conventions
---

# MemPalace (mempalace-dengine) Conversation Save Skillset

Scope: d-engine ecosystem repos (`d-engine`, `d-lmdb`, `d-stream`, `d-engine-product-design`,
`d-engine-jepsen`) using the `mempalace-dengine` MCP instance. This is the sibling doc to
`.claude/commands/save.md` (d-engine repo) — that skill writes ticket-scoped implementation
records to `d-engine-product-design/tickets|topics|decisions`; this skill writes cross-session,
searchable decision/design records to mempalace, for content that's more discussion-shaped
than doc-shaped (expert-consulted debugging, positioning pivots, root-cause findings).

## Before Saving: Dedup Check (MUST run first)

Before creating any drawer, check for an existing one on the same topic:

```
mempalace_check_duplicate({ content: "<the summary you're about to save>" })
mempalace_search({ query: "<topic keywords>" })   # if check_duplicate is inconclusive
```

- **Existing drawer, same topic still open** → this is an update/continuation. Add a new
  drawer but title it as a follow-up and link back: `(follow-up to drawer_id: ...)` —
  mempalace has no in-place append, so "update" in practice means "new drawer, explicitly linked."
- **Existing drawer, but this is a genuinely new decision/phase** → new drawer, still link
  the prior one under `## Related`.
- **Nothing found** → create new drawer.

Skipping this step is the main cause of duplicate/fragmented drawers — evidence of this
already exists in the palace: `d_engine`, `d-engine`, and `dengine` are three separate wings
today because of inconsistent naming, not three separate projects. Don't add a fourth variant.

## When to Save

Save when ANY of these occur AND the content has a concrete outcome (not just "we discussed X"):

| Trigger                              | Example from this project                                          |
| ------------------------------------- | -------------------------------------------------------------------- |
| Product/architecture positioning decided | d-stream pivot from broker to embedded pipeline (2026-06-07)     |
| Design decision after expert consultation | d-stream write-durability WAL design; d-lmdb pros/cons framing   |
| Root cause found for a real bug       | LMDB empty-key MDB_BAD_VALSIZE root cause                            |
| README / positioning rewrite recorded | d-lmdb "fault tolerance not scaling" reframing                       |
| Cross-project convention established  | "distributed fault tolerance, not distributed scaling" reused across d-lmdb and d-stream |
| Ticket implementation decision (if not already going through `save.md`) | Design choices made mid-debugging before a ticket exists |

Do not auto-save on keyword match — every candidate still goes through Dedup Check and
Dry-Run Check below.

## What NOT to Save

- Clarifying questions with no decision attached ("what does get_linearizable do?")
- Draft proposals that were rejected or superseded before being acted on
- Anything already fully captured in `d-engine-product-design/decisions/NNN-*.md` or
  `tickets/milestones/.../N.md` via the `save.md` skill — don't duplicate across both systems.
  Rule of thumb: **ticket-scoped implementation detail → `save.md` / decisions folder.
  Cross-session discussion/design reasoning that doesn't map to one ticket → mempalace.**
- Code-level detail fully recoverable by reading the current source (file paths, function
  signatures) — link to the file/line instead of pasting code that will drift

## Save Format

```typescript
mempalace_add_drawer({
  wing: "d-lmdb", // exact repo name, see Wing Convention below — never guess
  room: "decisions", // see Room Convention
  content: `
# {Topic} — {YYYY-MM-DD}

## Context
{1-2 sentences — why this came up}

## Conclusion / Decision
{the core outcome, phrased so it stands alone when searched}
- {point 1}
- {point 2}

## Honest Limits / Known Constraints
{if applicable — this project's norm is to state explicitly what a decision does NOT solve, don't omit it}

## Related
{linked drawer_id(s) or file paths, or "-" if none}
`,
});
```

## Wing Convention — canonical names only

| Wing                     | Repo                                    |
| ------------------------- | ---------------------------------------- |
| `d-engine`                | github.com/DEventLab/d-engine            |
| `d-lmdb`                  | github.com/DEventLab/d-lmdb              |
| `d-stream`                | github.com/DEventLab/d-stream            |
| `d-engine-product-design` | design/knowledge repo                    |
| `d-engine-jepsen`         | Jepsen test harness                      |

**Always use the hyphenated form matching the actual repo/directory name.** Do not use
underscores (`d_engine`) or no-separator forms (`dengine`) — those already exist as stray
wings from earlier inconsistent saves and should not be added to. If you're unsure which repo
a discussion belongs to, check the working directory path, not habit.

## Room Convention

| Room           | Use                                                             |
| -------------- | ---------------------------------------------------------------- |
| `decisions`    | Default. Almost everything goes here.                           |
| `architecture` | Only when the entire drawer is architecture/design, no decision outcome yet |

Don't over-split rooms — this project's palace already has legacy fragmentation
(`general`, `technical`, `tla`, per-crate room names from old imports). Going forward,
`decisions` is the default; only deviate when content is *exclusively* architecture-in-progress
with no decision reached yet.

**Searchability note**: room is a weak filter. Lead the drawer title/first line with the most
specific searchable noun — ticket number (`#422`), crate name (`d-lmdb`), technical term
(`MDB_BAD_VALSIZE`) — not buried in paragraph 2.

## Content Rules

1. **Skill file language vs. saved content language**: this document (the skill file itself)
   is English throughout. What actually gets *saved into mempalace* is different: body prose
   may stay in the original discussion language (Chinese, matching how this team actually
   communicates — see project CLAUDE.md) to preserve fidelity ("accurate record" was an
   explicit requirement from Joshua). Section headers/structure (Context, Decision, Related,
   etc.) stay in English for scannability across drawers. Keep ticket numbers, error strings,
   function/crate names verbatim regardless of language.
2. **Self-contained**: readable and actionable without the original conversation, 3+ months later.
3. **Concrete over comprehensive**: every section should contain at least one searchable
   entity (ticket #, error string, crate/function name, date). Cut sentences that only say
   "we discussed X" without landing on an outcome.
4. **One drawer per topic-conclusion**, not per message. A single debugging session that
   converges on one root cause = one drawer, even if it took many turns.
5. **Honest limits stay in**: this project's norm (established across d-lmdb/d-stream
   positioning work) is to record what a decision does *not* solve, not just what it solves.
   Don't strip that when compressing for mempalace.

## Session Boundaries

- Thread converges on a decision/root-cause → one drawer for the whole thread.
- Same topic resumed days later → new drawer, Dedup-Check-linked to the old one (see above;
  mempalace has no true append, so "continuation" = new drawer + explicit backlink).
- Genuinely new topic → new drawer, no link needed unless directly relevant.

## Dry-Run Check (run silently before every save)

1. Does this drawer lead with a concrete searchable noun (ticket #, crate name, error string)?
   If no → tighten the opening line.
2. Did I run Dedup Check and either link to an existing drawer or confirm this is new?
   If no → go back and check.
3. Searched 3+ months from now, will the first two lines surface the actual decision —
   not just "a discussion happened"? If no → revise.
4. Is the wing name one of the five canonical repo names above, not a variant? If no → fix it.

All four pass → save. Otherwise → revise or skip.
