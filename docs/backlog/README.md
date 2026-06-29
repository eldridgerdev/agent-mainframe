# Backlog

Design docs for work that is **not in active development** — whether
planned but not yet started, or started, paused, and not yet finished.
Each file captures the reasoning, shape, and open questions for one
feature so the decision and context survive until someone picks it up
again — and so "why haven't we finished this" has a written answer.

For partially-built work, record **what's done and what's left** so the
next person can resume without re-deriving it. When a doc describes work
that is actively being implemented right now, keep the live plan next to
the code if you prefer, and trim the backlog entry to a pointer.

## Conventions

- One feature per file, named in kebab-case and **ending in `-plan`**
  (e.g. `remote-control-server-mode-plan.md`). The `plan` in the name
  matters: AMF's in-app Markdown viewer filters to files whose path
  contains "plan", so this is what makes a backlog doc show up there.
- Keep each doc **self-contained**: a reader should understand the
  proposal without opening other files, though linking to related plans
  or code is encouraged.
- Start each doc with a short status block (see the template).
- Add the doc to the Index below.

## Status values

- `Backlog` — captured, not scheduled, not started.
- `Designing` — actively being fleshed out.
- `Ready` — design settled, ready to implement.
- `Partial` — implementation started then paused; not finished. Record
  what's done and what's left in the doc.
- `In progress` — actively being implemented right now (consider keeping
  the live plan next to the code and trimming this entry to a pointer).
- `Dropped` — decided against; keep the doc with the reason.

## Template

Use a checklist under **Progress** so the state of each item is explicit
— `[x]` done, `[ ]` not done. Keep it current as work lands so a paused
doc always says exactly what remains.

```markdown
# <Feature name>

- **Status:** Backlog
- **Owner:** <who, or unassigned>
- **Relates to:** <links to shipped features, plans, code, issues>

## Why / problem

## Proposed design

## Progress

- [ ] <implementation item>
- [ ] <implementation item>

## Open questions

## Reasoning / when to build
```

## Index

- [Bug backlog](bug-backlog-plan.md) — _Backlog._ Running list of known
  bugs not yet scheduled for a fix, one section per bug. Currently
  tracks recently fixed dashboard, sidebar, and composer-pane regressions.
- [Prompt library](prompt-library-plan.md) — _In progress._ Save
  reusable prompts and inject them into a session (compose box when on,
  paste without sending when off). Phases 1–2 have shipped: SQLite-backed
  save & inject, a multi-source picker (`User` / `Project` / `Global` /
  `Worktree` with badges), export to `config.json`, fill-in
  `{{placeholders}}`, and select-menu slots authored inline as
  `{{name|a|b|c}}`. Tags/grouping, an `amf-add-prompt` skill, export/display
  location unification, and showing the on-disk destination path remain.
- [Startup performance](startup-performance-plan.md) — _In progress._
  Parser repair and sidebar concurrency fixes shipped in PR #308;
  indexed Codex transcript access and sequenced startup I/O are
  implemented, with provider consolidation and deeper instrumentation
  still planned.
- [Vim mode](vim-mode-plan.md) — _Partial._ Ranked checklist of vim
  features for the in-house editor (`src/editor.rs`). Tier 1 core editing
  largely shipped; change operators and Tiers 2-3 remain.
- [Remote Control — server mode](remote-control-server-mode-plan.md) —
  _Backlog._ Spawn new Claude sessions on demand from web/mobile, each
  in its own worktree. Deferred; AMF's role would shift from
  drive-the-agent to provision-and-review.
- [Remote Control — QR code overlay](remote-control-qr-overlay-plan.md) —
  _Backlog._ Render the Remote Control session URL as a scannable QR in
  a TUI overlay. Pays off most with server mode, which prints a stable
  URL.
- [Expanded keybindings](expanded-keybindings-plan.md) — _Backlog._
  Grow config wizard keybinding support beyond dashboard actions to
  leader commands and other scoped command surfaces.
- [Final review enhancements](final-review-enhancements-plan.md) —
  _Backlog._ Follow-ups for the shipped native final review: line-level
  comments, multi-line/markdown feedback and notes, on-demand
  walkthroughs, finish gating, resumable state, base-ref selection, and
  PR integration.
- [PR comment review](pr-comment-review-plan.md) — _Backlog._ Triage a
  GitHub PR's comments (inline, review summaries, conversation, bots)
  inside AMF and, per comment, inject a token-minimal fix into the live
  agent session, post an AI-drafted reply, or skip. Token efficiency is
  the core constraint: the TUI fetches/triages with zero agent tokens,
  fix prompts carry only comment + diff hunk + `file:line`, threads are
  cached by head SHA. Split into read-only MVP → fix injection →
  replies/resolution → throughput epics, with a planned Epic E adding an
  AI code review of the diff and a committed `review-memory.md` of common
  findings (bootstrapped from the last N PRs, grown one comment at a time).
