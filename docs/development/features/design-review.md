# Design Review

Implementation notes for agents and contributors. Source is authoritative
when it disagrees; update this file in the same change. The design and its
phases are in [the plan](../../backlog/design-review-plan.md).

Design Review has two halves. The **Decision Log** (Phase 1, built) has the
agent record the choices a reviewer would question while it works. The
**review room** lays those records out as an agenda, beside the **packet**
(Phase 2, built) that staff models prepare. Phase 3 built the room shell
(agenda, content, thread, Usage tab). Phase 4 added refinement: the brief,
developer replies, re-review and approval. Wiring the room to the packet,
the question ladder above L0 and round-two packet regeneration come later.

```text
src/design_review/          # pure, no App
├── support.rs   # design_review_supported(), the "unavailable" wording
├── record.rs    # DecisionKind, field caps, DecisionRecord, OutboxLine
├── outbox.rs    # .amf/design-review/outbox.jsonl: append, read_from
└── capture.rs   # `amf decision add` (git-derived head and paths)
src/design_review/agenda.rs # pure: agenda derivation, merge, item states
src/design_review/brief.rs  # pure: labels, brief text, inline-or-file delivery
src/design_review/usage.rs  # pure: Usage tab aggregation and Markdown copy
src/design_packet/          # pure: staff roles, context builders, input
                            # hash, Mermaid vetting, detectors
src/app/design_review/      # App glue
├── mod.rs           # ingest_design_review_outbox()
├── packet.rs        # prepare_design_review_packet(), poll, status
├── packet_inputs.rs # background git gather, ledger token counts
├── room.rs      # AppMode::DesignReview state, open/close, agenda storage
├── actions.rs   # accept / note / request change / ask, open files
├── usage_tab.rs # ledger reload, grouping, drill-down, copy
├── finish.rs    # `f`: brief + destination + dispatch, or approval
├── replies.rs   # ingest of `amf review reply` lines
└── snapshot.rs  # base/head commit, diff fingerprint, changed files
src/handlers/design_review.rs, src/ui/dialogs/design_review.rs  # keys, draw
src/db/design_review.rs     # decisions, outbox offsets, room rows, artifacts,
                            # ledger, project cleanup
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
- **Detectors read code, not diff markers.** `DiffLine::text` from
  `parse_unified_diff` keeps its ` `/`+`/`-` marker; `detectors::code`
  strips it before any detector looks at a line.
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

## Packet

`App::prepare_design_review_packet(pi, fi, round)` is the room's entry; the
room reads progress from `design_review_packet_status(feature_id)` and each
section with `AmfDb::latest_design_review_artifact(feature_id, section)`.

- **Section keys** are `token_cost::ledger::Task` strings: `brief`,
  `architecture`, `key-flow`, `fit`, `unexplained`, `plan-drift`. Every
  artifact's `format` is `markdown`. A diagram section stores exactly one
  fenced ```` ```mermaid ```` block (source, never rendered text, without the
  model's trailing prose); its list fallback is plain Markdown.
- **Off the UI thread.** Preparation ingests the outbox, reads decisions
  (not `superseded`) on the UI thread, then gathers on a worker thread:
  `diff::load_snapshot` against the resolved base, commit subjects since it,
  `git ls-tree -d` of the base at the root and `src/` (for the new-module
  detector) and up to 48 KiB of `AMF_PLAN.md`. `poll_design_review_bg` in
  `cli.rs` collects it and every staff result.
- **Cache by input hash, template included.** Each section's key is
  `design_packet::context::input_hash` over the built context, the resolved
  template text (so a feature, project or global override invalidates only
  its section), the renderer limits and the staff harness, model and
  reasoning. A stored artifact with that hash is reused; nothing runs.
- **One ledger row per call.** Every staff call, each diagram attempt and
  every failed or stale call, writes one `design_review_usage` row: phase
  `prepare`, task = section, agent = role, the harness, model and reasoning
  that ran (a level the harness cannot express is recorded empty), round,
  duration. Staff runs use `HeadlessRunner::run_with_usage` (the same tool
  policy as `run`/`run_read_only`, in the structured event mode), so
  reported usage is `measured`: Claude's input excludes its cache reads,
  which go to `cache_read`; Codex's cached share is taken out of input.
  Cache writes are not parsed and stay unavailable. Only a run that reports
  nothing is `estimated` at bytes/4 of prompt and reply. A failed call has
  every count unavailable. A cache hit
  writes a `LedgerRow::cache_hit` row whose `saved_usd` is the cost of the
  row that produced the artifact (`usage_id`), or empty when unknown.
- **Staff runs.** Roles resolve through `design_packet::staff::resolve`
  from `AppConfig::design_review`; no model for the harness marks the
  section `Skipped`. Cartographer sections (`architecture`, `key-flow`,
  `fit`) run read-only, scribe sections run restricted with no tools.
  Each run is a thread; `HeadlessRunner` holds a
  headless lease for it. At most `MAX_PARALLEL_RUNS` (3) run at once, fewer
  when the agent concurrency limit, less harness sessions and other
  headless runs, leaves less room, but never fewer than one. A prompt over
  its role's input cap (only possible with an override) fails the section.
- **Plan drift** waits for the brief (cached or new) and is built from it;
  without `AMF_PLAN.md` it is `Skipped`, and it fails when the brief does.
- **Diagram retry policy.** A diagram reply that fails `diagram::check` is
  retried once with `diagram::retry_prompt`; `diagram::accept` then keeps
  valid source or falls back to a list. A harness error on the first
  attempt fails the section; on the retry, the first reply decides.
- **Stale results are dropped.** One packet is active at a time, with a
  request id. Preparing again (any round or feature) or
  `cancel_design_review_packet` supersedes it; a result with another id
  still writes its ledger row (the spend happened) but never becomes an
  artifact or changes the status. Superseded runs are not killed; they
  finish and release their leases.
- **Failures** log `log_warn("design-review", ..)`, leave
  `SectionState::Failed(reason)` and no artifact, so the next preparation
  retries them.
- **Tests** install a fake runner with
  `PacketRuntime::set_runner_for_test`; the default runner refuses under
  `cfg(test)`, so no test can launch a paid harness.

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
- **Open sequence.** Leader `y` runs the gate, then reads the git snapshot
  (`load_room_snapshot`) on a background thread; `poll_design_review_opening`
  opens the room when it arrives, and only if the reviewer is still viewing
  that feature. Opening then ingests the outbox, finds or creates
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
  longer produce stays, after the derived ones, marked `Retired` ("off
  agenda"). The whole agenda is written in one transaction
  (`AmfDb::design_review_transaction`), so a failed open leaves the stored
  order as it was.
- **Staleness** (`agenda::Staleness`, stored in `design_review_items.stale` as
  0 current, 1 possibly stale, 2 retired). Retired and possibly stale have
  separate tags and banners.
- **Possibly stale** is a decision item whose recorded paths have all left the
  current diff (`possibly_stale`). It is unknown (false) without a loadable
  diff and for a decision with no paths.
- **States** (`ItemState`, stored as text in `design_review_items.state`):
  `pending`, `question-open`, `change-requested`, `re-review`, `accepted`.
  `agenda::transition` is the one table: accept and note from pending or
  re-review go to accepted, request change goes to change-requested, ask goes
  to question-open and the answer (`Answered { back_to }`) returns it to the
  state it was asked from, pending or re-review; the caller keeps that state
  while the question is open. `DeveloperReplied` takes change-requested to
  re-review (for `done` and for `pushback` alike); from re-review the
  reviewer accepts (accepted) or requests a change again (change-requested).
  Change-requested accepts no reviewer action, since it waits on the
  developer, and accepted accepts none. Every action writes the database
  first, as one transaction; the room's copy changes only after the commit.
  The room resumes at the first item that `awaits_reviewer` (pending or
  re-review).
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

## Refinement (Phase 4)

- **Finish (`f`).** Items in `change-requested` and `question-open` (off-agenda
  items excepted) make the brief; the verdict is `changes-requested`. With
  none and every item accepted, `f` confirms approval instead. Undecided
  (pending or re-review) items with nothing to send refuse with a toast.
- **Labels.** `brief::build_brief` numbers changes `C1..` and questions
  `Q1..` in agenda order and keeps a label an item already carries, so
  a second finish of the same round does not renumber. They are stored in
  `design_review_items.label`, which also holds the agenda's `d4` labels.
  `merge_agenda` keeps a stored `C<n>`/`Q<n>` label across re-derivation
  (`brief::is_review_label`); nothing reads the `d<n>` value, since the brief
  derives its tag from the item's decision.
- **Brief shape** is plan section 5: a counts line, one line per item
  (`C1 [d4 <title>] <request>`, `C2 [unexplained: <summary>] ...`,
  `Q1 [d7] <question>`; the request is the item's newest reviewer turn, on one
  line), then the reply instructions and the `--supersedes dN` line. Under
  2_000 estimated tokens (`estimate_tokens_from_bytes`, so 7_996 bytes is
  inline and 8_000 is not) the text is pasted; otherwise it is written to
  `<workdir>/.amf/design-review/round-N.md` (beside the outbox, so the same
  `*` `.gitignore` covers it) and the paste is the counts line plus an
  absolute path to read.
- **Dispatch reuses Final Review.** The room builds the destination rows with
  `App::review_destination_rows` (extracted from
  `open_review_destination_picker`, minus the new-feature row, whose setup is a
  diff-viewer overlay) and pastes through `dispatch_review_feedback`, which
  gained a `prompt_override` (and `ReviewHarnessPickState::prompt`, for the
  "no dedicated session yet" harness pick). Routing, the submit setting
  (`final_review_submit_prompt`) and the "fixes ready" watch are Final
  Review's. Confirmation comes first; the labels, brief, destination
  (`live`, `dedicated:<harness>`, `feature:<id>`), verdict and `finished_at` are
  stored in one transaction before the paste, and a brief-file write failure
  aborts before anything is stored. The dispatch returns to the session view.
- **A round is not split yet.** Finishing again on the same round overwrites
  the brief and keeps the labels. Round two (incremental packet, delta agenda)
  is open.
- **`amf review reply <LABEL> (--done|--pushback|--answer) <text>`** appends
  `{"type":"reply","uid","label","kind","text","session_id","ts"}` to the
  outbox, never opens `amf.db`, prints `C1 reply recorded` (plus
  ` (truncated)` over 400 chars) or one error line with a non-zero exit (not
  `C<n>`/`Q<n>`, or a kind that does not fit the label). The CLI does not know
  the round, so the line carries none. `decision_add` counts decisions only, so
  replies never shift `d<N>` numbers, and an outbox of decisions alone still
  parses.
- **Reply ingest** (`replies::apply_reply`, one transaction per reply) finds the
  item by label in the newest round that has it, stores
  `reply_kind`/`reply`, appends a `developer` turn whose id is the reply's
  uid (that is the idempotency check: a re-read is a no-op, even after the
  reviewer has moved the item on) and moves the item per the state machine.
  An `answer` also inserts a decision with `source = 'review'`, kind
  `tradeoff`, title the reviewer's question (80 chars), why the answer, uid
  `review:<reply uid>`, `seq = 0` (the CLI numbers `d<N>` from the outbox, so
  the review's records stay out of that sequence), and returns the question
  to pending. `derive_agenda` skips `review`-source decisions: they are memory,
  not review items. An unknown label, or a kind that does not fit it, is
  logged and skipped.
- **Supersedes.** Ingest marks the agent decision numbered by a later record's
  `supersedes` (`d4`, `D4` or `4`, below the record's own `seq`) as
  `superseded`; the next agenda derivation retires its item.
- **Approval** sets the round's verdict `approved` and `finished_at` and marks
  every `open` decision of the feature `accepted` (superseded ones are left),
  in one transaction. The header shows the verdict; an approved room is
  read-only (accept, note, change, ask and finish refuse with a toast) when it
  is reopened. Decisions stay in `amf.db` as project memory.
- **Still open:** L3 escalation with a cost preview, developer ledger rows,
  round-two regeneration, ADR export.
