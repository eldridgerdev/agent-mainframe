# Design Review

Implementation notes for agents and contributors. Source is authoritative
when it disagrees; update this file in the same change. The design and its
phases are in [the plan](../../backlog/design-review-plan.md).

Design Review has two halves. The **Decision Log** (Phase 1, built) has the
agent record the choices a reviewer would question while it works. The
**review room** lays those records out as an agenda. Phase 3 built the room
shell (agenda, content, thread, Usage tab); the packet builder (Phase 2b), the
question ladder above L0 and refinement (Phase 4) come later.

```text
src/design_review/          # pure, no App
├── support.rs   # design_review_supported(), the "unavailable" wording
├── record.rs    # DecisionKind, field caps, DecisionRecord, OutboxLine
├── outbox.rs    # .amf/design-review/outbox.jsonl: append, read_from
└── capture.rs   # `amf decision add` (git-derived head and paths)
src/design_review/agenda.rs # pure: agenda derivation, merge, item states
src/design_review/usage.rs  # pure: Usage tab aggregation and Markdown copy
src/app/design_review/      # App glue: ingest_design_review_outbox()
├── room.rs      # AppMode::DesignReview state, open/close, agenda storage
├── actions.rs   # accept / note / request change / ask, open files
├── usage_tab.rs # ledger reload, grouping, drill-down, copy
└── snapshot.rs  # base/head commit, diff fingerprint, changed files
src/handlers/design_review.rs, src/ui/dialogs/design_review.rs  # keys, draw
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
- **One owner per outbox.** The outbox is per checkout but decisions belong
  to one feature. `ensure_feature_review_instructions` calls `outbox::bind`
  for an active Design Review feature before its agent starts; it records the
  feature id in `.amf/design-review/owner` and, when another feature (or
  nobody) owned the outbox, deletes it first, so the CLI numbers from `d1`
  again. Ingest skips an outbox the feature does not own. Two Design Review
  features on one checkout therefore take turns: lines the previous owner
  wrote after its last ingest are dropped when the other binds.
- **Truncate, never reject.** Caps live in `record.rs`: title 80 chars, why
  400, rejected 3 × 160 (extras dropped), ask 200, paths 40. Text is cut on a
  char boundary and ends in `…`.
- **Paths are per decision.** Each record carries a capture-only `dirty` map
  (path to `git hash-object` blob id or `deleted`, 200 entries, not stored in
  the DB). A later record lists files committed in `prev_head..HEAD` unless
  they were dirty with that same blob, plus dirty files whose blob differs
  from the previous snapshot. The first record uses the merge-base. Path
  lists come from `-z` output, so non-ASCII names are not quoted, and blob
  ids are batched (`ls-tree` with literal pathspecs, `hash-object
  --stdin-paths`), so git runs a fixed number of times per record.
- **Idempotent ingest by uid.** `App::ingest_design_review_outbox` reads
  complete lines from the stored cursor and inserts with `INSERT OR IGNORE`
  on `uid`. The cursor is a byte offset plus the SHA-256 of up to 4 KiB
  before it (`OutboxCursor`); when those bytes changed or the file is
  shorter, the outbox was replaced and reading restarts at 0. A trailing
  partial line waits for the next read; a malformed line is logged
  (`design-review`) and skipped. The cursor is not advanced when an insert
  fails, so the next turn retries.
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
  - Presets resolve through `resolve_preset_review` on every TUI surface (the
    New Feature dialog and the compact companion setup), and the GUI form
    mirrors it: Review Mode wins when a preset sets both, and
    `design_review: true` on an unsupported harness or container project is
    dropped. Either way a warning toast (GUI: inline notice) says so.
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
    harness is unsupported and toasts "Design Review turned off: <reason>",
    with the reason from `design_review_unavailable_reason`.
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
`design_review_outbox_offsets`. `MIGRATION_050` adds the cursor's
`tail_sha256`; a NULL fingerprint re-reads the outbox from 0. Each table
carries `project_id` for cleanup. Ledger token counts are nullable: absent
means unavailable, not zero.

## Review room

`AppMode::DesignReview(Box<DesignReviewRoom>)`, opened with leader `y` from a
session view (`Ctrl+Space`, `y`). `y` was the only unused letter in the leader
menu that was not a scroll key (`j`/`k`) and not a Learning/diff key; the
menu entry is in `ui/pane.rs` and the handler is in `handlers/view.rs`. Esc
or `q` returns to the session view. The room creates no session row.

- **Open gate.** `open_design_review` asks the one gate
  (`design_review_unavailable_reason`) and toasts `Design Review is <reason>`;
  a feature without the flag toasts `Design Review is off for this feature`;
  no database also refuses. Nothing is created on a refusal.
- **Open sequence.** Ingest the outbox, read the git snapshot, find or create
  round 1 (`design_review_rounds`: base and head commit, diff fingerprint),
  call `request_design_review_packet(pi, fi, round)` (a no-op that Phase 2b
  fills), then derive and store the agenda.
- **Agenda derivation is pure** (`agenda::derive_agenda`): brief,
  architecture, key flow, fit, each non-superseded decision (`d<seq> <title>`),
  the `unexplained` artifact (only when stored), each uncovered detector hit,
  each decision's `--ask` (`d<seq>? <ask>`), plan drift. Items are matched on
  reopen by `(kind, ref_key)` (section key, decision uid, `<detector>:<path>`)
  in `merge_agenda`: a match keeps id, state and thread and takes the new
  position, title and paths; a new item is `pending`; an item the inputs no
  longer produce stays, after the derived ones, flagged stale.
- **Possibly stale** is a decision item whose recorded paths have all left the
  current diff (`possibly_stale`). It is unknown (false) without a loadable
  diff and for a decision with no paths.
- **States** (`ItemState`, stored as text in `design_review_items.state`):
  `pending`, `question-open`, `change-requested`, `re-review`, `accepted`.
  `agenda::transition` is the one table: accept and note from pending or
  re-review go to accepted, request change goes to change-requested, ask goes
  to question-open and the answer returns it to pending. Change-requested and
  accepted accept no action until Phase 4 brings replies (`re-review`). Every
  action writes the database first; the room's copy changes only after the
  write succeeded.
- **Threads** are `design_review_turns` (`reviewer`, `record`; the ladder's
  presenter and developer roles are later). Note, request-change and ask text
  is a reviewer turn.
- **Ask is L0 only** (`actions::answer_question`): it quotes the decision's
  stored why and rejected options as a `record` turn at level `L0`, or says no
  decision is recorded. L1 and L2 branch from that function once the staff
  runner is merged.
- **Content** is Markdown through the shared renderer with Mermaid: a packet
  section shows the newest `design_review_artifacts` row for its key by id
  (`latest_design_review_artifact`) and reads "Not prepared yet" when there is
  none; a decision, ask or detector hit is rendered from its record. Content
  and thread are re-read on every selection.
- **Open files** (`o`/Enter) opens the ordinary diff viewer on the item's first
  path (`DiffViewerState::focus_path`, which also matches a directory) with
  `return_to` set to the room, so closing the viewer lands back in it.
- **Usage tab** (`Tab` or `u`): aggregation is pure
  (`design_review::usage`). Groupings cycle with `g` (phase, task, agent, model,
  item, round); `In` is input plus cache read plus cache write; `Eff` is shown
  only for a group with one model (`-` otherwise, and never for the total);
  `~` marks estimated rows, `*` an assumed price, `-` an unavailable figure.
  The footer is `Premium $x · Staff $y · Saved by cache $z (n sections
  reused)`, premium being agent `developer`. Enter opens a group's calls, `y`
  copies the table as Markdown. The header shows the feature's dollar total
  (`—` when the ledger has none). The Usage tab prices each call at its own
  model's rate, so it is the accurate figure and may disagree with the sidebar
  (#863); help and the tab say so.
