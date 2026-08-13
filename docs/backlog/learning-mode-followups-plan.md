# Learning Mode follow-ups

- **Status:** Anchor drift is done; navigable references and the
  alternative actionable mechanisms are still backlog.
- **Owner:** unassigned
- **Relates to:** [Learning Mode](learning-mode-plan.md) (shipped in
  `v0.36.0`), `src/app/learning.rs`, `src/ui/dialogs/learning.rs`,
  `src/db/learning.rs`, Final Review comment re-anchoring
  (`src/app/review.rs` — `reanchor_line_comments`), Feature TODOs
  (`feature-todos-plan.md`)

Three pieces of work that Learning Mode's plan deferred rather than
rejected. They are collected here — one `## ` section each, in the style
of `bug-backlog-plan.md` — because each is a small extension to one
shipped feature rather than a feature of its own, and because they were
deferred as a set. Each section is self-contained; pick up any one of
them without the others.

The fourth deferral from that plan is **not** Learning Mode's and lives
in `bug-backlog-plan.md` instead: *"Toasts raised while landing in the
composer are never drawn"*.

## Anchor drift: a stored `path:line-range` silently goes stale

- **Status:** Done. Content matching against the stored `selection_text`,
  run once per overlay open, with the three outcomes marked in the Q&A
  history, stated in the answer pane, and carried into `S` and `a`. The
  section below is kept as the record of what was decided and why.
- **Priority:** highest of the three. This is the only one that makes
  Learning Mode *wrong* rather than merely less useful.

### Why / problem

A `learning_qa` row stores `file_path`, `line_start`, `line_end`, and
`selection_text` with no drift protection (`MIGRATION_019`). Editing the
file afterwards — even reformatting it — leaves every earlier entry
pointing at whatever now occupies those line numbers.

This bites Learning Mode's primary use case first. Explanatory answers
are exactly the entries meant to be long-lived notes, and a newcomer is
the user least equipped to notice: a stale anchor does not look like a
stale anchor, it looks like an answer that was always wrong. That
undermines the one thing the mode is selling.

The plan accepted this for v1 explicitly ("v1 stores the raw path + line
range and **accepts staleness**"), so this is a known debt being paid,
not a regression.

### Proposed design

AMF already solves the analogous problem once, for Final Review's line
comments: `App::reanchor_line_comments` (`src/app/review.rs:340`) walks
each file and calls `reanchor_file_comments`, reporting how many moved
and how many lost their anchor entirely. Model this on that rather than
inventing a second mechanism, and lift the shared part if the shapes
converge.

Two candidate strategies, not mutually exclusive:

1. **Content match.** `selection_text` is already stored. On load,
   search the current file for it; if it is found at a different offset,
   move the anchor and say so. This needs no schema change and handles
   the common case (code moved down as lines were added above it).
2. **Commit SHA + snippet.** Add the commit the anchor was captured at,
   so a drifted anchor can be diffed against its original rather than
   fuzzy-matched. Costs a migration and only helps in a git project.

Start with (1); it is cheap and covers most drift.

Whatever is chosen, **the three outcomes must be distinguishable in the
UI**: anchored as stored, re-anchored to a new range, or lost. A silently
re-anchored entry is a smaller version of the same honesty problem —
Final Review's message ("re-anchored N comment(s)" / "N comment(s) lost
their anchor — possibly addressed") is the right register.

### Progress

- [x] **Content match, and no sharing with `reanchor_file_comments`.**
      Strategy (1), as the plan recommended; the SHA+snippet option was not
      needed and stays unbuilt. The Final Review helper could not be reused:
      it relocates a `DiffLineLocation` inside a diff using a captured
      single line plus its neighbours (`CommentAnchorContext`), and a
      `learning_qa` row has neither — it has plain 1-based file lines and
      the *whole* selection. Matching that block is both simpler and
      stronger evidence than line-plus-neighbours, so `expected_block` +
      `locate_block` + `check_anchor_drift` are their own pure functions in
      `src/app/learning.rs`. Comparison is **trimmed and blank-line-free**,
      so a `rustfmt` pass is not reported as movement — re-indentation is
      named in the problem statement and is the most common way a range
      moves without the code changing at all.
- [x] Re-anchored on history load: `App::learning_check_anchor_drift`, run
      from `open_learning_mode` right after the content loads. Not literally
      inside `load_learning_qa` — it needs the workdir, which is only
      assembled once the overlay state exists — but it is the same job as
      `reconcile_interrupted_qa` (that one reconciles the *runs*, this one
      the *code* they were about), and the doc comment says so. Eager, per
      the open question below. One read per distinct file, deduped.
- [x] Three outcomes, distinct in both places. `LearningAnchorDrift::{
      Reanchored, Lost(FileGone | NotFound | Ambiguous)}` lives in a side
      table (`LearningViewState::anchor_drift`) keyed by row id rather than
      as a field on `LearningQa` — a verdict is a judgment about the working
      directory right now, not something the row carries, and keeping them
      apart is what stops it being written back over the range the question
      was actually asked at. The history row gains `⚠ moved` / `⚠ anchor
      lost` (ahead of `→ TODO` / `→ session`, since the headline truncates
      from the right and "these line numbers can't be trusted" outranks
      "you already acted on this"), the answer pane gains a sentence under
      its header quoting the stored range and the new one, and the overlay
      raises a one-line summary on open. Three decisions came out of
      building it:
      - **The stored range is not overwritten, and neither outcome is
        persisted.** `selection_text` is the evidence, so the verdict is
        re-derived every open — which costs nothing and means a re-anchor
        that was really a branch switch un-reports itself when the branch
        switches back. Overwriting `line_start`/`line_end` would trade a
        recoverable answer for an unrecoverable one, and would lose the
        historical fact of where the question was asked.
        (`re_anchoring_does_not_rewrite_the_range_it_was_asked_at`.)
      - **"We didn't look" is not one of the three outcomes.** A file that
        is present but unreadable, a row with no captured selection, and a
        whole-file anchor whose file still exists all report *nothing* —
        a marker meaning "unchecked" would be worse than no marker, because
        the reader cannot tell it apart from "checked and fine".
        (`a_file_that_could_not_be_read_claims_nothing`,
        `a_row_with_no_captured_selection_is_left_alone`.)
      - **A diff-sourced selection can be lost but never re-anchored.** Its
        stored range comes from `new_line.or(old_line)` (`anchor_for_cursor`),
        so a selection opening on a removed line is already numbered off the
        base side — precise enough to point a reader at, but not a baseline
        to measure movement against. "That code is no longer in the file"
        survives that; "it moved to line 61" does not. `expected_block`
        strips the markers and drops the removed rows, so what is searched
        for is what could still be there.
        (`a_diff_anchor_is_reported_lost_but_never_moved`.)
      The verdict also travels: `escalation_seed` and `todo_body` take it as
      a parameter and state it. Both hand a `path:start-end` locator to
      something that will go and read it, and the seed quotes the *original*
      excerpt underneath — so a silent stale locator there is the exact route
      from a stale anchor to a confidently wrong answer, which is the failure
      this item exists to close.
- [x] Tests. Pure: `an_anchor_that_did_not_move_is_not_reported`,
      `re_indenting_the_code_is_not_movement`,
      `code_that_moved_down_is_found_again`,
      `a_moved_range_reports_both_of_its_ends`,
      `code_that_was_rewritten_loses_its_anchor`,
      `code_that_now_appears_twice_is_lost_rather_than_guessed_at`,
      `a_copy_made_elsewhere_does_not_unanchor_the_original`,
      `a_deleted_file_takes_every_anchor_in_it`,
      `a_file_that_could_not_be_read_claims_nothing`,
      `a_whole_file_anchor_only_notices_the_file_going_away`,
      `the_project_anchor_never_drifts`,
      `a_diff_selection_is_matched_on_what_survived_it`,
      `a_diff_anchor_is_reported_lost_but_never_moved`,
      `a_row_with_no_captured_selection_is_left_alone`. Overlay level, all
      DB-backed and editing the file behind the overlay's back:
      `a_question_reloads_marked_when_its_code_moved`,
      `re_anchoring_does_not_rewrite_the_range_it_was_asked_at`,
      `a_question_whose_file_was_deleted_reloads_marked_lost`,
      `an_untouched_project_reloads_with_nothing_marked`. Hand-off:
      `a_drifted_answer_is_handed_over_saying_where_the_code_went`,
      `a_drifted_answer_is_kept_saying_where_the_code_went`. Render:
      `a_drifted_anchor_is_marked_in_the_history`,
      `the_drift_marker_survives_beside_the_acted_on_markers`,
      `the_answer_pane_says_where_the_code_went`,
      `a_lost_anchor_says_the_answer_is_still_there`,
      `a_narrow_terminal_still_finishes_the_drift_sentence`,
      `an_answer_that_did_not_drift_gives_up_no_space_to_saying_so`.
      Suite: 1955 passing / 0 failing, `cargo clippy --all-targets` clean,
      no `println!`/`eprintln!` introduced.
      **Verified against real Claude**, driving the built binary in a 140×44
      tmux against a throwaway XDG root and a seeded demo repo: asked about
      `lines 3-6 of src/main.rs` (the `load` function), closed the overlay,
      added five lines above it, and reopened. The row came back
      `? explain  answered  ⚠ moved`, the banner read *"The project changed
      since some of these were asked: 1 moved with the code (the answer still
      fits)"*, and the answer pane kept its `lines 3-6` title over *"it was
      lines 3-6, it is now lines 7-10"* with the real markdown answer intact
      below. Renaming the file away then reloaded it as `⚠ anchor lost` with
      the file-gone sentence, and `S` on that row landed in a composer whose
      seed carried the warning directly under *"Where I was reading:
      src/main.rs:3-6"*. Two fixes came out of reading the rendered output
      rather than the code, neither of which any unit test had:
      - **"1 no longer point at code that is there."** The summary pluralised
        one branch and not the other. Guarded now by asserting the
        single-entry wording, not just the phrase.
      - **The drift sentence was clipped at narrow widths.** The block was a
        fixed two rows, which fits the longest of the three sentences at 140
        columns and not at 80 — and the clause that falls off the end is
        *"The question and answer below are unchanged"*, the one that stops
        the marker reading as "this entry is broken". The block is now sized
        to what the text takes at the pane's real width.
        (`a_narrow_terminal_still_finishes_the_drift_sentence`.)
      **Captured** as ten frames in `docs/screenshots/learning-mode-anchor-drift/`
      (scenario `scripts/dev/screenshot/scenarios/learning-mode-anchor-drift.txt`),
      driven against a throwaway instance seeded with a small demo repo, with one
      real headless Claude run. This is the first Learning Mode scenario to use
      `run:` steps as *content* rather than setup: drift only exists across a
      close, an edit, and a reopen, so the two file edits have to happen between
      the shots. One capture-harness gotcha worth recording for the next
      scenario that starts a session: the scratch instance uses the **real**
      AMF tmux socket (`~/.local/state/amf/tmux.sock`, logged at startup), not
      an isolated one, so a feature session left behind by an earlier run
      survives into the next — and a second `S` then lands on a duplicate
      `claude-2` window name and fails with `tmux send-keys failed`. Three runs
      were lost to a leftover `amf-notes-demo-feature` before that was spotted.
      Kill the feature session between runs, or don't pass `--keep` to a
      scenario that escalates.

### Open questions

- ~~Ambiguous matches~~ — **settled as lost.** Two candidates means there
  is no honest way to say which copy the question was about, and guessing
  is a smaller version of the problem the item exists to remove. Note the
  qualifier that fell out of building it: a duplicate is only ambiguous if
  the *original* also moved. Code still sitting where it was stored stays
  anchored however many copies have since been made elsewhere, or else
  extracting a repeated idiom would break every note about the original.
- ~~Eager or lazy~~ — **eager, on load.** The marker's whole job is to be
  there before the reader believes the row's line numbers, which a
  check-on-select cannot do.
- **A drifted answer is still an answer written about the old code.** The
  anchor is re-pointed; the prose is not re-checked. An answer that quotes
  a variable which has since been renamed is now marked "moved" and reads
  as freshly correct. `D` (re-ask with the repo open) is the existing
  mitigation and the marker at least makes the staleness visible, but a
  "this answer may describe an older version" register is not attempted.
- **The check only runs on open.** Editing a file from another AMF session
  while the overlay is up will not re-mark anything until it is reopened.
  A cheap re-check on scope toggle (`s`, which already re-reads) would
  cover most of it if this turns out to matter.

## Alternative mechanisms for making an answer actionable

### Why / problem

v1 ships one way to act on an answer without leaving read-only: `a`
keeps it as a project TODO. That was chosen because it reused the most
existing machinery (`src/db/todos.rs`, the `SessionKind::Todos` session,
and the spawn-agent-from-item flow), not because it is the best fit.

What the plan settled is the **shape** — actionability is an optional,
explicit gesture on an answer, never the terminal step of every Q&A. The
mechanism behind that gesture stayed open, and two candidates were named
and deferred:

1. **A composer seeded and scoped to the file/range.** Closer to `S`
   (escalate) than to `a`, but narrower: instead of handing the agent
   the whole conversation, hand it the anchor and the proposed change,
   scoped to the lines in question.
2. **An inline suggested patch**, in the shape Final Review's suggestion
   blocks already use — the answer proposes a diff, and applying it is a
   keypress.

(2) is the one that would change Learning Mode's read-only invariant,
which the plan is explicit about: relaxing it "should be a conscious
decision rather than a convenience patch". If suggested patches land,
the honest framing is that applying one is the *second* exception
alongside `S`, and the UI has to say so.

### Progress

- [ ] Decide whether either mechanism is wanted before doing more; the
      TODO path may simply be sufficient. Judge it against real use, not
      in the abstract.
- [ ] If the seeded-and-scoped composer wins: it is mostly `S` with a
      narrower seed, so cost is low.
- [ ] If suggested patches win: decide the read-only exception
      explicitly, and state it in the `?` overlay and README alongside
      the existing `S` carve-out.

### Open questions

- Learning-originated TODOs land in the same one-per-project list as
  hand-written ones. Whether they need visual distinction, or a separate
  list, is undecided — and is a reason to prefer a non-TODO mechanism if
  the noise turns out to matter.
- Seeded titles from `Explain` answers are known-poor (an explanation
  has no lead line to use, so the first real run seeded *"The short
  version"*). Fixing that — seed from the question, or ask the agent for
  a title as part of the answer — is cheaper than replacing the whole
  mechanism, and should probably be tried first.

## Make "Where to look next" references navigable

### Why / problem

Every `Newcomer`-level answer ends with a **Where to look next** list of
specific files and symbols — that is required by `level_instructions`,
and it is the part of the answer that is supposed to keep a newcomer
moving. Today it is plain text: reading `src/app/state.rs` there and
then opening it means leaving the answer, changing pane focus, and
finding the file in the list by hand.

Learning Mode already knows the repo's file list and can already load
any of it. Turning those references into jumps is mostly wiring.

### Proposed design

Parse file-path-looking references out of a rendered answer, and offer a
key that jumps the file list and content pane to one — without closing
the answer pane, since the reference only makes sense next to the
sentence that made it.

There is a second, sharper reason to build this. A `NoTools` answer
**invents references**: verified real runs pointed at `src/app/state.rs:812`
and a `LearningState` symbol, neither of which exists, and one run
fabricated an entire `Bash: ls -la` transcript with a plausible fake
result. A jump that fails is at least an honest signal — it turns an
invisible fabrication into a visible one, which is worth more here than
the navigation convenience. That argues for resolving references
**eagerly** (on answer load, marking each as resolvable or not) rather
than only on keypress, so a fabricated path is marked before the user
trusts it.

### Progress

- [ ] Extract references from the answer text. Scope it: repo-relative
      paths with a known extension, optionally `:line`. Symbol names are
      a much harder problem — leave them alone for v1.
- [ ] Resolve each against the loaded file list at render time; mark
      unresolvable ones visibly.
- [ ] A key that jumps to the selected reference with the answer pane
      still open, and a way to get back to where the answer was being
      read.
- [ ] Check the answer footer still fits. It is already two lines and
      has truncated a hint off the end more than once; this adds another.
- [ ] Tests: extraction from a realistic newcomer answer, a fabricated
      path marked unresolvable, a `path:line` jump landing on the line,
      and the answer pane surviving the jump.

### Open questions

- Whether an unresolvable reference should be marked quietly or say
  outright that the agent may have made it up. The latter is more
  honest and matches the mode's stated register, but it is a strong
  claim to render automatically — a path can also be unresolvable
  because it is outside the workdir or was legitimately deleted.
- Whether resolution should also cover the `NoTools`-fabrication problem
  more directly (detecting simulated tool transcripts in an answer), or
  whether that is its own item. Probably its own.

## Reasoning / when to build

None of these blocks anything, and Learning Mode is usable end to end
without them. Suggested order if they are picked up:

1. ~~**Anchor drift**~~ — **done**, taken first because it was the only
   correctness problem in the list and it degraded the feature's primary
   use case over time.
2. **Navigable references**, because it is mostly wiring over machinery
   that already exists, and because it partially mitigates the
   fabricated-reference problem that has been observed in real runs.
3. **Alternative actionable mechanisms**, last and only if real use
   shows the TODO path is not enough — the cheaper fixes to that path
   (a better seeded title) should be tried first.
