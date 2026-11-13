# Design Review

Implementation notes for agents and contributors. Source is authoritative
when it disagrees; update this file in the same change. The design and its
phases are in [the plan](../../backlog/design-review-plan.md).

Design Review has two halves. The **Decision Log** (Phase 1, built) has the
agent record the choices a reviewer would question while it works. The
**review room** (Phases 2–4, not built yet) lays those records out as an
agenda.

```text
src/design_review/          # pure, no App
├── support.rs   # design_review_supported(), the "unavailable" wording
├── record.rs    # DecisionKind, field caps, DecisionRecord, OutboxLine
├── outbox.rs    # .amf/design-review/outbox.jsonl: append, read_from
└── capture.rs   # `amf decision add` (git-derived head and paths)
src/app/design_review/      # App glue: ingest_design_review_outbox()
src/db/design_review.rs     # decisions, outbox offsets, project cleanup
src/app/setup.rs            # ensure_design_review_instructions()
```

## Invariants

- **Write-only capture.** The agent records a decision with
  `amf decision add` and never reads a review artifact. The CLI prints
  exactly one line (`d<N> recorded`, plus ` (truncated)` when a field was
  cut) and, on error, one line on stderr with a non-zero exit.
- **Outbox, not database, from the agent.** The CLI appends one JSON line to
  `<checkout>/.amf/design-review/outbox.jsonl` with a single `O_APPEND` write
  and never opens `amf.db`, so it works with AMF closed. The directory holds
  its own `*` `.gitignore`, and `.amf/` is AMF-managed in diffs.
- **Truncate, never reject.** Caps live in `record.rs`: title 80 chars, why
  400, rejected 3 × 160 (extras dropped), ask 200, paths 40. Text is cut on a
  char boundary and ends in `…`.
- **Paths are per decision.** Each record carries a capture-only `dirty` map
  (path to `git hash-object` blob id or `deleted`, 200 entries, not stored in
  the DB). A later record lists files committed in `prev_head..HEAD` unless
  they were dirty with that same blob, plus dirty files whose blob differs
  from the previous snapshot. The first record uses the merge-base.
- **Idempotent ingest by uid.** `App::ingest_design_review_outbox` reads
  complete lines from the stored byte offset and inserts with
  `INSERT OR IGNORE` on `uid`. A trailing partial line waits for the next
  read; a file shorter than the offset restarts at 0; a malformed line is
  logged (`design-review`) and skipped. The offset is not advanced when an
  insert fails, so the next turn retries.
- **Ingest triggers.** At agent turn end (`stop`, `thinking-stop`,
  `input-request`), beside Review Mode's note archiving in
  `notifications.rs`, only for features with `design_review` on. The review
  room must call it again on open, which also covers harnesses without
  turn-end hooks.
- **Bounded always-on text.** `DESIGN_REVIEW_BLOCK` in `setup.rs`, markers
  included, is at most 640 bytes (test). It is written between
  `AMF:design-review` markers, listed in `MANAGED_BLOCKS`, and stripped when
  the mode is off or unsupported.
- **One gate.** `design_review_supported(agent, containerized)` is true only
  for Claude in a non-containerized project. Every surface asks it, and
  `design_review_unavailable_reason` returns the wording to show
  (`DESIGN_REVIEW_CLAUDE_ONLY`, `DESIGN_REVIEW_CONTAINER_UNAVAILABLE`).
  Container projects are gated off because the sandbox image does not ship
  `amf`.
- **One review mode per feature.** `Feature::set_design_review` and
  `Feature::set_review_mode` clear the other flag. Injection writes Review
  Mode's notes block only when `Feature::review_notes_enabled()`, so Design
  Review wins if both are set. Every call site goes through
  `ensure_feature_review_instructions`.
- **Where the mode is offered.** Every surface shows the Claude-only limit
  instead of a toggle that does nothing:
  - The TUI New Feature form has a last row, `[ ] design review (Claude only
    for now)` (`mode_max_focus()`: 7 on Claude, 5 elsewhere). For another
    harness or a container project the row reads `Design Review: <reason>`
    and cannot be toggled; changing to an unsupported harness unchecks it.
    `CreateFeatureState::toggle_review` / `toggle_design_review` keep the two
    modes exclusive. The compact companion setup (`FeatureSetupState`, PR
    Triage, Final Review, issue fixer) has the same row.
  - A preset's `design_review: true` on an unsupported harness is ignored and
    a warning toast says so (`apply_preset_review`).
  - The GUI New Feature form has a disabled "Design review" switch for any
    harness but Claude (`creationCopy.ts` mirrors the wording). Container
    projects are refused by the server, not pre-checked in the form.
  - Automation `design_review` is refused with an error (nothing created) on
    an unsupported harness, in a container project, or together with
    `review`.
  - `App::effective_design_review` is the one place a creation path turns a
    request into a stored flag (`finish_feature_launch`, hook-pending upsert,
    fork, companion registration), so an unsupported feature is never stored
    with the flag on and never gets the instruction block.
  - `change_feature_harness` (TUI and GUI) clears the flag when the new
    harness is unsupported and toasts "Design Review turned off: Claude only
    for now".
  - Batch creation and the GUI's TODO/issue forms do not offer it yet.
- **Decisions are project memory.** The tables have no FK to `projects` or
  `features`, since the store's full-replace save would cascade them away.
  Deleting a feature keeps its decisions. Deleting a project removes every
  Design Review row (`AmfDb::delete_design_review_for_project`).

## Persistence

`MIGRATION_049` adds `features.design_review` and every table the later
phases need: `design_decisions`, `design_review_rounds`,
`design_review_items`, `design_review_turns`, `design_review_artifacts`,
`design_review_usage` (the token ledger) and
`design_review_outbox_offsets`. Each table carries `project_id` for
cleanup. Ledger token counts are nullable: absent means unavailable, not
zero.
