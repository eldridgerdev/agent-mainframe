# Final Review Enhancements

- **Status:** Rounds 1–2 shipped; Round 3 in backlog — every item under
  **Progress → Round 1** and **Round 2** is implemented and merged
  (most recently Round 2's file-level PR comments). **Round 3** (captured
  2026-07-01) has started: first-class file-level comments, interdiff on
  re-review, the "fixes ready — re-review?" notification, local application of
  suggestion blocks, and a **Cost** batch have shipped. The Cost batch makes
  bounded headless passes honor `review_model`, caps
  `final-review-feedback.md` with an archive file, and batches REVIEW MODE's
  note instruction per turn. The rest of the
  loop-closing, viewer-ergonomics, AI co-review,
  and workflow items are not yet started. Three Cost follow-ups (per-action
  model overrides, capping `review-notes.md` the same way, and measuring the
  most token-efficient way to dispatch review fixes) are added below.
- **Owner:** unassigned
- **Relates to:** the shipped native final review
  (`src/app/review.rs`, `src/handlers/diff.rs`,
  `src/ui/dialogs/diff.rs`, `DiffViewerState` in `src/app/state.rs`);
  the per-file diff review (`src/handlers/diff_review.rs`); review
  mode (`CLAUDE.local.md` → `.claude/review-notes.md`).

## Why / problem

The final review was rewritten to use AMF's native diff UI (press `f`
in a feature view). It walks every file changed since the base ref,
shows the developer's reasoning from `.claude/review-notes.md` beside
each diff, lets the reviewer approve / reject (with feedback) / skip
each file plus add general feedback, writes
`.claude/final-review-feedback.md`, and prompts the feature's agent to
address it.

That covers the core "developer walks through their changes" loop, but
several gaps remain before it matches a real-world code review — most
notably, feedback is whole-file only (no line-level comments), the
feedback editors are single-line, and a few rough edges were knowingly
deferred when the feature shipped.

## Proposed design

Grouped roughly by impact. Each item is independent; they can land in
any order.

### Loose ends (small, deferred at ship time)

- **Paste-vs-submit toggle for the agent prompt.**
  `finish_final_review` pastes the prompt into the agent pane and sends
  Enter. Offer a config (or key) to paste without submitting so the
  reviewer can eyeball/edit it first.
- **Notes scroll precision.** `review_notes_scroll_bottom` clamps to the
  note's raw line count, so a long soft-wrapped note may not scroll
  fully to the visual bottom. Compute wrapped height against the panel
  width instead.
- **Multi-line feedback editor.** Both the per-file rejection editor and
  the general-feedback editor are single-line (`feedback_input`, capped
  at 2000 chars, no newlines). Reuse the vim-capable `TextEditor` (as
  `SteeringPromptState` does) so reviewers can write paragraphs/lists.

### High-value reviewer features

- **Line / hunk-level comments.** The defining feature of real code
  review and the one thing the whole-file model can't express. Let the
  reviewer move a cursor in the diff, mark a line/range, and attach a
  comment, written into the feedback file with file+line context (e.g.
  `### src/foo.rs:42`). `TextSelection` (in `ViewState`) and the patch
  renderer's line numbers already provide most of the plumbing.
- **Generate a walkthrough on demand for noteless files.** The per-file
  review can spawn a headless Claude explanation
  (`generate_diff_review_explanation` / `ClaudeLauncher::spawn_headless`).
  The final review only shows "No developer note." Add a key to generate
  an explanation for a file with no note so the walkthrough is never
  empty.
- **Render notes as markdown.** The Developer Notes panel uses a plain
  `Paragraph`. Render with `crate::markdown` (and the syntax highlighter)
  so multi-paragraph notes with headings/lists/code blocks read well.
- **Finish gating / jump-to-next-undecided.** Add a key to jump to the
  next file with no verdict, and at finish either warn about skipped
  files or require a verdict on each, so a half-done review isn't
  finished by accident.
- **Persist / resume review state.** Decisions and general feedback live
  only in `DiffViewerState`; `q`/`Esc` ends the review and discards them.
  Persist to `.claude/` so a long review can be paused and resumed.
- **See inline comments while browsing the diff.** Line comments are stored
  (`line_comments` in `DiffViewerState`) but only surface in the feedback file
  and while the line cursor is editing one — there's no marker in the rendered
  diff for a line that already has a comment, so a reviewer scrolling back
  can't tell which lines they annotated. Add a gutter marker (e.g. a dot /
  count) on commented lines and reveal the comment body when the cursor (or
  scroll) lands on/over it — at minimum a peek on hover-over. The unified
  renderer already maps each rendered row to `addressable_lines()`
  (`ui/dialogs/diff.rs`), so the anchor plumbing exists.
- **Multi-line / range line comments.** A `LineComment` anchors to a single
  `DiffLineLocation` today. Let the reviewer mark a start line, extend the
  selection to an end line, and attach one comment to the whole span — matching
  GitHub's `start_line`/`line` multi-line review comments. Touches the comment
  model (store a range), the cursor (anchor + extend), the feedback-file anchor
  format (`### src/foo.rs:42-48`), and `build_pr_review` (emit
  `start_line`/`start_side`).
- **Dispatch review fixes to a new agent / harness session.**
  `finish_final_review` pastes the "address the feedback" prompt into the
  feature's existing agent pane. Offer instead to spin up (or target) a fresh
  agent session — optionally a different harness (Claude / Codex / opencode) —
  so the fixes run in a clean context rather than the long-running review
  session. Reuses the session plumbing (`add_session`, the session picker) and
  the PR-comment-review "fix target" concept (a dedicated session, see
  `fix_session_index_prefers_dedicated_else_creates`).

### Nice-to-haves

- **Choose the base ref.** The diff auto-resolves against
  origin/HEAD → main → master. Let the reviewer pick a specific
  commit/branch (the viewer already supports refresh).
- **PR integration.** Rejections could optionally post as inline PR
  comments (see the `pr-*` skills and `code-review --comment`) instead
  of or alongside the feedback file.
- **Review history.** `final-review-feedback.md` is overwritten each
  run; timestamped entries or append mode would preserve a trail across
  rounds.
- **Re-review loop.** After the agent addresses feedback, flag which
  files changed since the last review so only those need re-checking.
- **File-list filters.** Show only undecided / only rejected files for
  large changesets.

### Round 2 — deeper review workflow

A second batch, captured after the core shipped. Roughly ordered by
impact. Each is independent and grounded in the plumbing it reuses.

- **AI co-reviewer first pass (pre-fill draft comments).** Instead of
  starting from a blank diff, run a headless pass (reuse the
  `generate_review_walkthrough` / `walkthrough_child` machinery and/or
  the `code-review` skill) that seeds `line_comments` as *draft*
  comments the human accepts / edits / dismisses (e.g. Tab through
  them). Shifts the reviewer from "find everything" to "adjudicate the
  AI's findings". **Token cost is the main constraint** — a
  whole-changeset headless pass is expensive, so it must be opt-in and
  bounded: per-file / on-demand triggering and/or a diff-size cap rather
  than an automatic sweep on open. Treat draft comments as clearly
  distinct from human ones until accepted.
- **Suggested-change blocks.** Let a line comment carry
  a *replacement* for the cursored line/span (reuses `comment_cursor` /
  `comment_anchor` and the range plumbing). Emit a GitHub fenced
  `suggestion` block so it's one-click-appliable on the PR, and feed
  the same exact replacement into the agent prompt as a verbatim patch
  rather than prose to interpret. Touches `build_pr_review` and the
  feedback-file rendering.
- **Severity tags on comments / rejections.** Tag each comment / file
  rejection with a severity (`blocker` / `suggestion` / `nit` /
  `question` / `praise`, conventional-comments style). Buys three things
  cheaply: (1) map the overall verdict to the GitHub review *event* —
  all-approve → `APPROVE`, any blocker → `REQUEST_CHANGES`, else
  `COMMENT` (today `create_review` is hardcoded to `COMMENT`; keep that
  as the default and only escalate when the reviewer is **not** the PR
  author, since GitHub forbids approving / requesting-changes on your
  own PR); (2) tell the agent in the prompt what's mandatory vs optional
  (it currently weights every item equally); (3) add a severity option
  to the existing `FileFilter` cycle ("blockers only").
- **Agent writes responses back into the feedback file.**
  `REVIEW_FEEDBACK_PROMPT` tells the agent to *address* the latest round
  but never to *respond*. Extend it so the agent appends a structured
  `**Agent:** …` reply under each item ("fixed in X" / "disagree because
  Y"). The next review round can then render the agent's reasoning
  beside the original comment — turning fire-and-forget feedback into a
  threaded conversation. Pairs with thread state, below.
- **Resolve / unresolve thread state across rounds.** Each round in
  `final-review-feedback.md` is independent today; the re-review loop
  only flags *files* that changed (`changed_since_last`). Track
  per-comment resolved state (paired with the agent-response item) so a
  re-review can show "N unresolved threads" and filter to just those —
  GitHub-style resolvable conversations without leaving AMF.
- **Re-anchor comments across edits.** Store a small context snippet
  (the commented line + ~2 neighbours) alongside the `DiffLineLocation`.
  On re-review, if the exact line moved, fuzzy-match the snippet to
  re-locate it; if it can't be found, surface the comment as "anchor
  lost — possibly addressed" rather than silently dropping it. This is
  the concrete answer to the first open question and the prerequisite
  that makes thread state survive the agent actually editing the code.
- **Changeset overview + diff stats (manual only — never automatic).**
  Two parts: (a) a key to generate an on-demand whole-diff
  overview / risk summary via headless (same mechanism as the per-file
  walkthroughs, whole-diff scope); (b) `+/-` counts and risk markers
  (large / no developer note / no test coverage) on the file-list rows
  so attention goes where it matters (the snapshot already fingerprints
  every file in `save_review_snapshot`). **Must be reviewer-triggered**,
  not run automatically on open, to avoid surprise token spend.
- **Build / test gate before approve.** Offer a check command run on
  finish; surface pass/fail in the summary and optionally block an
  all-approve on failure — a final review that approves code that
  doesn't compile is a miss. The command must be **configurable
  per-project** (e.g. point it at the project's existing proof / CI
  script) rather than a hardcoded `cargo build`.
- **File-level PR comments instead of body-dumping.** Whole-file
  rejections currently collapse into the review *body* (no anchor).
  GitHub's API supports `subject_type: file` comments — anchor them to
  the file so they stay inline on the PR instead of in the summary.
  Touches `build_pr_review` and the `GhCli::create_review` payload.
- **Jump-by-hunk navigation.** A key to jump the patch cursor to the
  next / previous hunk for fast traversal of large diffs.
- **Search within the diff.** Incremental search over the current
  file's patch (and/or across files) to jump to a match.
- **Line comment auto-rejects its file.** Leaving a line comment on a file is
  itself a signal the file needs work, so a file with any (kept, non-draft) line
  comment should be treated as "needs revision" rather than requiring a separate
  explicit rejection. On storing the first such comment
  (`diff_review_submit_line_comment`), default the file's `decisions` entry to
  `ReviewDecision::Reject` (with empty feedback, since the line comments carry
  the specifics) unless the reviewer has already set an explicit verdict; clear
  it again if the last comment on the file is removed. Keep it overridable — an
  explicit approve/skip after commenting should win — and decide whether an
  accepted AI draft counts. Surfaces in the file-list markers and the
  approved/needs-work/skipped counts on finish.

### Round 3 — review-loop and viewer upgrades

A third batch, captured 2026-07-01 from a fresh pass over the shipped
feature (including the Round 2 items that have since landed: AI
co-review drafts, suggestion blocks, the `Changed` re-review filter).
Grouped by theme; each item is independent. Overlaps with existing
Round 2 items are cross-referenced rather than duplicated — in
particular, feedback-resolution tracking is already covered by
**Round 2 → resolve/unresolve thread state** + **re-anchor comments**,
and outcome-driven PR review events by **Round 2 → severity tags**.

#### Comment model

- **File-level comments (the missing third anchor).** Feedback has exactly two
  scopes today: **general** (the whole review) and **line** (a `LineComment` on a
  `DiffLineLocation` or span). A file has no comment of its own — the only way to
  say something about a file as a whole is to *reject* it, because file-scoped
  prose and the needs-work verdict are the same act
  (`ReviewDecision::Reject { feedback }`). So a reviewer can't leave a
  verdict-free observation ("fine, but this module wants splitting") without
  marking the file as needing work, and can't reject a file without inventing
  prose when the line comments already said it. Add a first-class file comment:
  a `file_comments` map on `DiffViewerState` beside `line_comments`, carrying the
  same `Severity` (so a `[nit]` file comment doesn't imply a blocker) and the same
  resolved/thread state, attachable independently of the verdict. It renders as a
  `### src/foo.rs` section (no line number) in
  `.claude/final-review-feedback.md`, gets its own file-list marker + `F` filter
  step, and posts to the PR through the **already-shipped**
  `GhCli::create_file_comment` / `subject_type: file` path — note this is *not* a
  duplicate of **Round 2 → file-level PR comments**, which only changed how an
  existing whole-file *rejection* is transported to GitHub and added no new
  comment kind. That transport being done is what makes this item cheap: it needs
  the model, the editor entry point, and the renderer, not the GitHub plumbing.
  Decide how it interacts with **Round 2 → line comment auto-rejects its file**
  (a file comment probably should *not* auto-reject — that's the point of it).

#### Closing the review→fix→re-review loop

- **Interdiff on re-review.** `final-review-snapshot.json` stores only
  per-file fingerprints, so a re-review can say *which* files changed
  but then shows the full branch diff again — the reviewer re-reads
  everything to find the fix. Store the per-file patch (or the
  base+head blob ids) in the snapshot so a changed file can offer a
  "since last review" view: the diff between what was reviewed last
  round and what's there now. Answers the actual re-review question
  directly; the single highest-leverage item in this round.
- **"Fixes ready — re-review?" notification.** After a finished review
  dispatches the fix prompt, watch the target agent session via the
  existing thinking-status sync; when it goes idle, raise a
  notification (existing notification plumbing) with a one-key jump
  straight back into the review — which, paired with interdiff, opens
  pre-filtered to changed files. Turns review rounds into a loop
  instead of the reviewer polling the pane.
- **Apply suggestions locally.** A suggestion is already a verbatim
  replacement with an exact line span — everything needed to patch the
  worktree directly. A key on a suggestion-carrying comment (or an
  "apply all suggestions" option at finish) skips the agent round-trip
  for mechanical fixes, reserving the agent for prose feedback. Guard
  with a dirty-file / span-still-matches check and report what was
  applied in the finish summary.
- **Finish summary screen.** `q` currently gates on undecided files and
  then immediately writes and dispatches. Insert an editable summary
  overlay first — every verdict, comment and suggestion in one list,
  Enter jumps back to any item to edit — one last look before the
  feedback becomes an agent prompt and possibly a PR review. Also fixes
  a smaller gap: editing feedback on an earlier file today means
  hunting it down in the list again. This is the concrete answer to the
  open question about a composed "review summary".

#### Viewer ergonomics

- **High priority: close / pause without finishing the review.** Review
  progress is persisted, but there is no intentional way to leave the viewer
  without completing the round: outside cursor/editor modes, both `q` and
  `Esc` run the finish flow, which can write feedback, post to the PR, dispatch
  fixes, clear progress, and replace the last-review snapshot. Add a distinct
  close/pause action that returns to the feature view while preserving
  `.claude/final-review-progress.json` exactly as-is and performs none of the
  finish side effects. Make the difference between **pause** and **finish**
  explicit in the footer/help and confirmation copy, and keep nested `Esc`
  behavior predictable (dismiss editor/cursor/modal first, then pause from the
  top-level viewer). Reopening Final Review must resume the same file,
  decisions, comments, filters where applicable, and general feedback.
- **Fix the layout toggle.** `v` should reliably switch the diff between
  unified and side-by-side layouts and the footer should always describe the
  layout actually being rendered. Today the binding can be shadowed by `v`'s
  range-selection meaning while the line cursor is active, and added/untracked
  files force unified layout by silently ignoring the toggle. Give layout and
  range selection unambiguous bindings or mode-specific hints, avoid a silent
  no-op for files that cannot render side-by-side, preserve the user's layout
  preference when moving between ordinary and new files, and add handler/UI
  coverage for toggling in final-review mode.
- **Review-round timeline and history browser.** Add a compact timeline strip
  such as `Round 1 ─ Round 2 ─ Current` so reviewers can move backward and
  forward through the conversation instead of seeing only the latest round and
  its replies. Left/right (or h/l) selects a round; the body is independently
  scrollable and shows that round's verdicts, file/line comments, suggestions,
  agent replies, check result, timestamp, and summary counts. Historical rounds
  are read-only, while `Current` returns to the live editable review. Make long
  histories horizontally scroll or window around the selected round, with a
  clear marker for the current round and unresolved threads that carried
  forward. Load older rounds lazily from
  `.claude/final-review-feedback-archive.md` so this view does not undo the
  existing live-file token cap; show an explicit limitation when an old round
  lacks enough snapshot data to reconstruct its original diff, rather than
  silently presenting today's diff as historical content.
- **Hierarchical file tree + shorter Developer Notes panel.** Replace the flat
  changed-file list with a directory-aware tree so files are grouped by path,
  directories can be expanded/collapsed, and keyboard navigation can move
  through the visible hierarchy without losing the existing verdict, comment,
  risk, and changed-since-last-review markers. Keep filters working against
  files while retaining the directories needed to show matching results. At
  the same time, reduce the Developer Notes panel's default share of the right
  column from about 40% to about 20% (half its current height), leaving more
  room for the diff; the existing expand-notes action should still make it
  full-height on demand.
- **Expand context around hunks.** `DiffFile` already carries
  `old_content`/`new_content`, so GitHub-style "expand N lines
  above/below" (or a whole-file toggle) is mostly a rendering change.
  Hunks alone often hide what's needed to judge a change — the
  enclosing function signature, the surrounding match.
- **Word-level intra-line diff highlighting.** Highlight which tokens
  changed within a modified line pair (the other half of diff
  readability alongside the existing syntax highlighting). Cheap and
  adjacent: an ignore-whitespace toggle (`git diff -w` semantics).
- **Global comment navigation + undo verdict.** Tab cycles AI drafts
  only within the current file; add next/prev *comment* navigation
  across all files. And an undo for the last verdict — an accidental
  `a` currently just advances and the file must be re-found manually.
- **Open at line in `$EDITOR`.** With the line cursor active, a key to
  suspend the TUI and open the file at the cursored line — sometimes
  the reviewer needs to poke around before writing the comment.
- **`?` help overlay for review mode.** The key surface is large now
  (a/r/s/f/c/v/S/C/A/w/e/u/F/t/b/n/p/[/]/Tab, plus cursor-mode
  overloads of a/d/Esc); the footer can't teach all of it. Reuse the
  dashboard's help-overlay pattern with review-specific groupings.
- **Mouse support in the diff viewer.** `handlers/mouse.rs` already
  handles the dashboard; add click-to-select in the file list,
  wheel-scroll in the patch, and click-to-place the comment cursor.

#### AI co-review upgrades

- **Whole-changeset co-review.** `A` runs per-file only. Add a variant
  that queues all non-binary, undecided files through the same per-file
  pass — one headless child at a time, a progress marker in the file
  list, drafts landing as each file finishes — a true first-pass
  reviewer while keeping token cost bounded and visible.
- **Severity + suggestions in AI findings.** Extend the output contract
  to `<line>|<severity>|<comment>` so drafts colour-code and filter by
  severity (skim blockers first — feeds the Round 2 severity-tags
  item), and let the model optionally attach a fenced replacement that
  lands as a *draft suggestion* — which, with apply-suggestions-locally
  above, becomes "AI proposes, human accepts, patch applies" end to
  end.
- **Cross-file context for the co-reviewer.** The prompt currently
  shows one file's diff in isolation, so it can't catch "renamed here
  but not there". Include the changeset's file list plus the other
  files' hunk headers (still bounded) to raise finding quality.
- **Ask the AI a question in-line, without leaving the review.** The
  co-reviewer (`A`) and walkthrough (`w`) are one-shot: the reviewer
  can't ask a follow-up ("why is this safe?", "does this handle the
  empty case?") while forming a verdict. Add a key to type a free-form
  question about the current file (or a `v`-selected span), fire it
  headless with the file's diff as context (reuse the
  `spawn_headless`/poll machinery and the diff-truncation from
  `build_walkthrough_prompt`), and show the answer **without rejecting
  the file or leaving the screen** — no losing your place in the diff.
  Two candidate presentations: (a) a modal answer dialog over the
  viewer (dismiss to return to exactly where you were), or (b) split
  the developer-notes panel into two boxes — the note/walkthrough on
  the left, the AI answer on the right — so question and diff stay
  visible together. Keep it reviewer-triggered and per-file so token
  cost stays bounded, and consider threading follow-ups (append to the
  same answer box) rather than one-shot. Pairs with the notes-panel
  plumbing that already renders markdown + the agent replies-back
  section (`draw_notes_panel`, `src/ui/dialogs/diff.rs`).

#### Workflow & entry points

- **Start a review from the dashboard.** `trigger_final_review`
  requires `AppMode::Viewing`; add a dashboard binding on the selected
  feature, plus a "review pending" badge on features whose agent went
  idle since the last review snapshot — making reviews visible as a
  queue instead of a hop through the feature view.
- **Reviewer-experience-level review notes.** Let the reviewer declare
  how familiar they are with the language/codebase — e.g. "experienced
  engineer familiar with the language" vs. "student who's never used
  this language before" — and calibrate how in-depth the agent's
  `.claude/review-notes.md` explanations end up being: skip
  language-idiom asides and boilerplate context for an expert, spell out
  *why* a pattern is used and define unfamiliar terms for a beginner. The
  natural landing spot is a new `final_review_notes_level` (or similar)
  config value woven into the REVIEW MODE block
  `ensure_review_claude_md` injects into `CLAUDE.local.md`
  (`src/app/setup.rs`) — an extra instruction line telling the agent who
  it's writing notes for. Likely wants a per-project setting (a
  student's familiarity doesn't change per feature) rather than global,
  and should default to today's level-agnostic wording so existing
  projects are unaffected.

## Progress

### Round 1 (shipped)


- [x] Paste-vs-submit toggle for the agent prompt
- [x] Notes scroll-to-bottom precision (wrapped height)
- [x] Multi-line feedback editor (reuse `TextEditor`)
- [x] Line / hunk-level comments (unified view; side-by-side
      navigates + stores comments but has no inline marker yet)
- [x] On-demand walkthrough for noteless files
- [x] Render Developer Notes as markdown
- [x] Finish gating + jump-to-next-undecided
- [x] Persist / resume review state (incremental save to
      `.claude/final-review-progress.json`; restored on a fresh open,
      cleared on finish)
- [x] Choose base ref (press `b` in the diff viewer / final review to
      diff against any branch, tag, or commit; blank reverts to auto)
- [x] PR inline-comment integration — opt-in via the
      `final_review_post_to_pr` config flag. On finishing a review (with the
      flag on), AMF resolves the branch's PR and posts a single GitHub review:
      line comments become inline comments (anchored `RIGHT` for a current-file
      line, `LEFT` for a deletion-only line); whole-file rejections and the
      general feedback — which have no single line to anchor to — become the
      review's summary body. The event is `COMMENT` (safe on a self-PR). The
      `GhCli` layer gained a feature-agnostic `create_review`; the final-review
      mapping lives in `build_pr_review` (`src/app/review.rs`). Best-effort: a
      missing PR or `gh` error is folded into the finish message and logged, and
      the local `.claude/final-review-feedback.md` is always written regardless.
- [x] Review history (timestamped / append) — each round is a
      dated `## Review — <ts>` section prepended under a single
      title in `.claude/final-review-feedback.md`; the agent is
      prompted to address only the most recent round
- [x] Re-review loop (changed-since-last-review) — finishing a review
      fingerprints the diff to `.claude/final-review-snapshot.json`; the next
      review compares against it, marks changed files with a `Δ` in the file
      list and a header count, and on a fresh re-review where only some files
      changed auto-applies a new `Changed` file-list filter (in the `F` cycle
      when a prior review exists) landing on the first changed file.
- [x] File-list filters (undecided / rejected) — press `F` in the final
      review to cycle the file list through all / undecided / rejected;
      navigation (n/p/j/k, g/G) skips hidden files and decisions advance
      to the next visible file.
- [x] See inline comments while browsing — commented lines carry a `●` gutter
      marker (unified view), and parking the line cursor on a commented line
      peeks the comment body in a bordered "comment on this line" box above the
      cursor hints (the review footer grows to fit, capped at 6 body rows; Enter
      still opens the full editor). Reuses the existing `comment_cursor` →
      `addressable_lines()` → `line_comments` lookup (`ui/dialogs/diff.rs`)
- [x] Multi-line / range line comments — with the line cursor active, press `v`
      to drop a selection anchor, extend it with j/k, and `Enter` attaches one
      comment to the whole span. The selection gutter is tinted while marking;
      a stored comment marks every line of its span with `●` and the peek box
      reads "comment on these lines". The model gained
      `LineComment.start: Option<DiffLineLocation>` (defaulted, so old progress
      files load), the feedback-file anchor renders `src/foo.rs:42-48`, and
      `build_pr_review` emits GitHub `start_line`/`start_side` so a ranged
      comment posts as a multi-line PR review comment. Re-opening a comment
      snaps the anchor/cursor onto its span so an edit preserves the range.
- [x] Dispatch review fixes to a new agent / harness session instead of the
      existing pane — press `t` in the final review to toggle the fix target
      between the feature's existing agent pane (default, unchanged) and a fresh
      dedicated "Final Review" session; the footer shows `t target: live` /
      `dedicated`. On finish with the dedicated target, an existing "Final
      Review" session is reused, or — when none exists — a harness picker
      (`AppMode::ReviewHarnessPick`) lets the reviewer choose which harness
      (Claude / Codex / opencode / …) runs the fixes before the session is spun
      up. The feedback file is always written first, so cancelling the picker
      just leaves it for later. Reuses the PR-review `FixTarget` toggle (now
      parameterized by session label) and `create_dedicated_review_session`
      (now accepting a label + optional harness override).

### Round 2 (planned)

- [x] AI co-reviewer first pass (pre-fill draft comments) — press `A` in
      the final review to run a headless Claude pass over the **current
      file only** (reviewer-triggered + per-file + diff truncated, so it's
      opt-in and bounded for token cost). The pass reports findings as
      `<line>|<comment>`, parsed onto `addressable_lines()` and seeded as
      *draft* line comments (`LineComment.draft`, serde-defaulted so old
      progress files load). Drafts render distinctly — a hollow `○` gutter
      marker in the warning colour vs a kept comment's filled `●`, and the
      peek box / footer label them and surface `a` accept · `d` dismiss ·
      Enter edit. With the line cursor active, `a` accepts the draft under
      the cursor (promote to a permanent comment), `d` dismisses it, and
      `Tab` jumps to the next draft ("Tab through them"); editing a draft
      and submitting also accepts it. Unaccepted drafts are excluded from
      the finished feedback file and the PR review, and persist across a
      pause/resume via the progress file. Reuses the
      `spawn_headless`/poll machinery (a second child slot alongside the
      walkthrough's) — `generate_co_review` / `poll_co_review` in
      `src/app/review.rs`.
- [x] Suggested-change blocks — with the line cursor active (optionally over a
      `v` range), press `S` to open a suggestion editor pre-filled with the
      span's current code (`DiffFile::addressable_line_texts`, diff-prefix
      stripped); the edited replacement is stored on the span's `LineComment` as
      `suggestion: Option<String>` (serde-defaulted, so old progress files load).
      A suggestion may accompany prose or stand alone (empty-prose comment);
      emptying a suggestion-only comment deletes it. On finish it renders as a
      fenced ```suggestion block in `.claude/final-review-feedback.md` (a
      verbatim patch for the agent, not prose) and, when posting to the PR,
      `build_pr_review` appends the same block to the inline comment body so it's
      one-click-appliable. The peek box and footer surface it (`S suggest`);
      `diff_review_start_suggestion` / `diff_review_submit_suggestion` in
      `src/app/review.rs`.
- [x] Severity tags on comments / rejections — each line comment and file
      rejection carries a conventional-comments severity (`blocker` /
      `suggestion` / `nit` / `question` / `praise`, `Severity` in
      `src/app/state.rs`, serde-defaulted so old progress files load). In the
      comment / rejection editor `Ctrl+E` cycles it (shown in the editor title
      and footer); a fresh line comment defaults to `suggestion`, an explicit
      file rejection to `blocker`. The finished feedback file tags every item
      (`#### src/foo.rs:42 — [blocker]`) and the agent prompt explains the tags
      so blockers are prioritized; a blocker line comment tints its `●` gutter
      marker danger and the cursor peek box leads with the severity. Posting to
      a PR prefixes each inline comment / summary line with the tag and maps the
      review to a GitHub *event* (`build_pr_review` / `severity_review_event`):
      any blocker → `REQUEST_CHANGES`, no rejection → `APPROVE`, else `COMMENT`
      — only escalating past `COMMENT` when a best-effort `GhCli::is_self_review`
      check confirms the reviewer isn't the PR author (GitHub forbids self
      approve / request-changes). A new `Blockers` step in the `F` file-filter
      cycle narrows the list to blocker-carrying files.
- [x] Agent writes responses back into the feedback file — `REVIEW_FEEDBACK_PROMPT`
      now asks the agent, after addressing each item, to append a `**Agent:** …`
      reply on its own line under that item (what it changed / why it disagrees /
      an answer to a `[question]`). On the next review round AMF parses the latest
      round's replies from `.claude/final-review-feedback.md`
      (`parse_agent_responses` in `src/app/review.rs`, grouped by file) and, in
      `load_prior_agent_responses`, files them onto
      `DiffViewerState.prior_agent_responses` for files still in the diff. The
      notes panel renders them as an "Agent replies (last round)" markdown section
      beneath the developer note / walkthrough, so re-reviewing a file shows what
      the agent said it did. Surfaced per file (anchor-free), so it ships without
      the thread-state / re-anchor machinery; per-comment threaded rendering
      beside each individual line comment still pairs with the two items below.
- [x] Resolve / unresolve thread state across rounds — each kept line comment
      is a thread with a `resolved` flag (`LineComment.resolved` in
      `src/app/state.rs`, serde-defaulted so old progress/snapshot files load
      as open). With the line cursor active, `R` toggles the comment under the
      cursor between resolved and reopened (`diff_review_toggle_resolved` in
      `src/app/review.rs`); the peek box and cursor-mode footer surface the key
      and label a settled thread "resolved thread". Resolving a file's last
      open thread clears its comment-implied rejection the same way removing
      the comment does (`diff_review_sync_auto_reject` now keys off
      `LineComment::is_open_thread`, i.e. kept and unresolved); reopening one
      re-applies it. On finish, a resolved thread is withheld from
      `.claude/final-review-feedback.md` and the PR review — only open threads
      reach the agent — so settling a conversation actually stops re-sending
      it. Threads carry across rounds: `save_review_snapshot` gained a
      `threads` map of every kept comment (any resolved state) tagged
      `carried`, and `restore_review_progress`'s fresh-round case seeds
      `line_comments` from it (dropping only threads that are both resolved
      and anchor-lost, which have nothing left to show). A carried comment
      that's still open renders in the feedback file tagged "(unresolved from
      a previous round)". A new `Unresolved` step in the `F` file-filter cycle
      (skipped when nothing is open, like `Changed` without a snapshot) narrows
      to files with an open thread, and `apply_review_snapshot_diff` reports
      the carried-over count in the re-review open message. Restoring carried
      threads makes `line_comments` non-empty on a fresh open, so the
      "pristine re-review" check that steers the auto-`Changed` filter and
      approval carry-over now asks `has_only_carried_comments()` instead of
      emptiness, and a file with an open thread is excluded from the
      stale-approval carry regardless of its last verdict.
- [x] Re-anchor comments across edits — a line comment anchors to an exact
      `DiffLineLocation`, so when the diff reloads underneath it and the line has
      moved, the anchor no longer resolves and the comment silently vanishes from
      the gutter. Each comment now carries a context snippet (the commented line
      plus up to `ANCHOR_CONTEXT_RADIUS` = 2 neighbours on each side, from
      `addressable_line_texts()`): `CommentAnchorContext` in `src/app/state.rs`,
      with `anchor_context` / `start_anchor_context` / `anchor_lost` on
      `LineComment` (all serde-defaulted, so existing progress files load — and a
      snippet-less legacy comment simply reads as anchor-lost rather than being
      dropped). Snippets are captured centrally in `recapture_anchor_contexts`,
      called from `persist_review_progress` — the choke point every review
      mutation already funnels through — so no creation site has to remember.
      On every diff reload `reanchor_line_comments` (wired into
      `complete_diff_viewer_loading`) re-locates each comment whose anchor went
      stale: `CommentAnchorContext::best_match` matches the anchor line on
      trimmed text (tolerating reindentation) and disambiguates repeated lines by
      neighbour agreement, refusing to guess on a tie or a blank anchor line. A
      range whose `start` can't be re-found degrades to a single-line comment
      rather than inverting the span; a comment that can't be located at all is
      flagged `anchor_lost` and surfaced — "N comment(s) lost their anchor —
      possibly addressed" — instead of disappearing. A lost comment's heading in
      the feedback file drops its (now stale) line number
      (`src/foo.rs (anchor lost — possibly addressed)`, still resolved back to
      its file by `anchor_file_path`), and `build_pr_review` skips it for inline
      PR posting rather than pinning it to a wrong line. Policy lives in the pure
      `reanchor_file_comments` (`src/app/review.rs`). **Scope:** comments are
      still cleared when a review finishes, so today this fires on the in-session
      reload paths — an `r` refresh after the agent edits code, and a base-ref
      change. It becomes the cross-round mechanism the plan describes as soon as
      thread state (above) carries comments between rounds; this item is that
      item's stated prerequisite.
- [x] Changeset overview + diff stats — press `O` in the final review to run a
      headless Claude pass over **every file's diff at once** (reviewer-triggered,
      capped at 30 files with a per-file patch budget, so a huge changeset can't
      produce an unbounded request) and show the result — a short overview plus a
      "Risk factors" list — in a scrollable modal (`j`/`k`/PageUp/PageDown/g/G,
      `O` again regenerates, `q`/`Esc` closes; the modal takes full key
      precedence while open, mirroring the search prompt). The result is cached
      on `DiffViewerState.changeset_overview` so reopening the modal never
      re-spawns a headless pass on its own — only an explicit regenerate does.
      Reuses the exact `spawn_headless`/poll pattern as the per-file walkthrough
      (`generate_changeset_overview` / `poll_changeset_overview` in
      `src/app/review.rs`) and the notes panel's markdown-render-and-scroll
      approach for the body. Separately, the file list now carries a
      `[L,N,T]`-style risk-marker span per row (review mode only): `L` when a
      file's total added+removed lines cross a "large change" threshold, `N`
      when it has neither a developer note nor a generated walkthrough, and `T`
      when it looks like non-test source code and the *whole changeset*
      contains no test-looking file at all — the closest proxy for "no test
      coverage" available, since no per-file test-mapping convention exists
      anywhere in this codebase. `file_risk_marker` in
      `src/ui/dialogs/diff.rs`.
- [x] Build / test gate before approve — a per-project
      `final_review_check_command` (`ExtensionConfig` in `src/extension.rs`,
      merged from `~/.config/amf/config.json` / `{repo}/.amf/config.json`
      exactly like `lifecycle_hooks`, project overrides global) is run via
      `bash -c` in the feature's workdir when finishing a review. Unset
      (the default) skips it entirely — zero behavior change for projects
      that don't opt in. The command runs in the background and is polled
      to completion (`finish_check_child` / `finish_check_command` on
      `DiffViewerState`, spawned in `finish_final_review` and polled by
      `poll_final_review_check` each main-loop tick, mirroring
      `changeset_overview_child`) so a slow `cargo build` or test suite
      doesn't freeze the UI. Pass/fail is folded into the finish summary
      message and a `### `/`**Check:**` section in
      `.claude/final-review-feedback.md` (with captured output, capped at
      `CHECK_OUTPUT_MAX_CHARS`, on failure). A failing check blocks the
      "all files approved, nothing to write" fast path — the round is
      always written and dispatched to the agent when the check fails,
      even with zero rejections, which is the concrete answer to "block an
      all-approve on failure". `complete_final_review` in
      `src/app/review.rs`.
- [x] File-level PR comments instead of body-dumping whole-file
      rejections — a rejected file with no line comments now posts as its
      own `subject_type: file` review comment attached to that file,
      instead of a paragraph in the review's summary body. GitHub's batch
      `create_review` endpoint has no file-level comment support in its
      `comments` array (checked the REST docs: `subject_type` only exists
      on the single-comment endpoint, and there's no way to attach a
      standalone comment to an already-created review), so this is a
      second round of best-effort `gh api` calls — one per rejected file —
      made only after the batch review itself posts successfully.
      `GhCli::create_file_comment` / `PrFileComment` (`src/github.rs`);
      `build_pr_review` returns the new file-comment list alongside the
      existing summary body and inline comments (`src/app/review.rs`).
- [x] Jump-by-hunk navigation in the diff — press `]` / `[` in the final
      review to jump the line cursor to the next / previous hunk's first
      line (activating the cursor if it's off; `]` lands on the first
      hunk, `[` on the last). From mid-hunk, `[` first snaps to the
      current hunk's start, vim-style. The patch follows via the
      existing cursor sync, the selection anchor is kept so a `v` range
      can span hunks, and the cursor-mode footer hints the keys.
      `DiffFile::hunk_start_indices` (`src/diff.rs`) anchors the jumps;
      `diff_review_jump_hunk` in `src/app/review.rs`.
- [x] Search within the diff — press `/` in the final review to open an
      incremental, case-insensitive search over the **current file's** diff
      (matched against `addressable_line_texts()` so it hits added / removed /
      context lines alike). Typing jumps the line cursor to the first match at
      or after its position; on commit (`Enter`) the query sticks and `n` / `N`
      cycle matches with wraparound (shadowing file navigation only while a
      search is active), while `Esc` clears it. Every hit carries a hollow `▷`
      gutter marker (the current match is the cursor's solid `▶`) and the footer
      shows `search: <q> (i/N)`. Reuses the `comment_cursor` +
      `cursor_sync_to_view` plumbing for the jump/scroll; `compute_search_matches`
      / `diff_search_*` in `src/app/review.rs`, `search_*` on `DiffViewerState`.
      Cross-file search deferred.
- [x] Line comment auto-rejects its file — storing a file's first kept
      (non-draft) line comment or suggestion defaults its verdict to
      `Reject` with empty feedback (the comments carry the specifics);
      deleting the file's last kept comment clears a verdict that was
      auto-set this way. Explicit verdicts win: an existing decision is
      never overwritten, and approve / skip / typed-rejection drop the
      file from the auto set so the reviewer's call sticks (after a
      skip, only a fresh comment mutation re-defaults it). An accepted
      AI draft counts (it's a human-affirmed finding); a dismissed one
      doesn't. The implicit/explicit distinction persists across
      pause/resume (`auto_rejected` in the progress file, serde-defaulted
      so old files load), auto-rejected files surface in the existing
      rejected file-list marker / filter / finish counts, and the
      feedback file renders "(Needs revision — see this file's line
      comments below)" instead of "no feedback provided".
      `diff_review_sync_auto_reject` in `src/app/review.rs`.

### Round 3 (planned)

Comments:

- [x] File-level comments — press `m` in the final review to edit one
      verdict-free comment anchored to the current file; unlike a rejection or
      an open line comment, it never auto-rejects the file, so observations,
      questions, nits and praise can coexist with an approve/skip verdict. The
      comment carries the same conventional `Severity` and resolved/thread
      state as line comments (`M` resolves/reopens it), persists in
      `.claude/final-review-progress.json`, and is carried between finished
      rounds in the review snapshot with an "unresolved from a previous round"
      tag. File-list rows show `◆` for an open file thread and `◇` for a
      resolved one; the `F` cycle gained a `File comments` step, while the
      existing `Blockers` and `Unresolved` filters/counts include open file
      threads. Finishing writes a dedicated `### File Comments` section with a
      `#### src/foo.rs — [severity]` anchor and posts each open comment through
      the already-shipped `GhCli::create_file_comment` /
      `subject_type: file` path. Blocker file comments participate in PR event
      escalation; resolved ones are retained for reopening but withheld from
      feedback and PR posting. `FileComment` / `file_comments` in
      `src/app/state.rs`; editor, persistence, finish and PR mapping in
      `src/app/review.rs`. The shared review footer now expands for this editor
      too, so pressing `m` visibly opens the same multi-line edit box used by
      line comments and rejection feedback.

Loop:

- [x] Interdiff on re-review — `.claude/final-review-snapshot.json` gained a
      `content` map (file path -> its `new_content` when the round finished,
      skipped for binary files and deletions, `#[serde(default)]` so older
      snapshots just load with nothing to diff against yet). Press `I` on a
      `Δ`-flagged file to open a read-only modal with the diff between that
      saved content and the file's current content — computed on demand via
      a single local `git diff --no-index` (`build_interdiff` in
      `src/app/review.rs`, materializing both blobs to `NamedTempFile`s and
      reusing `crate::diff::load_review_file`, the same plumbing the
      config-wizard confirm dialog and the Claude-hook diff-review prompt
      already use), so unlike the AI-generated overview it needs no
      child-process/poll machinery. A no-op with a message when there's
      nothing to show: no prior review, the file has no saved content from
      last round, or the diff comes back empty (the fingerprint can move for
      reasons other than the file's own content, e.g. the base ref shifted).
      The modal (`draw_interdiff_modal`, `src/ui/dialogs/diff.rs`) reuses
      `draw_patch_panel` — the same syntax-highlighted unified-diff renderer
      as the main patch panel — and takes full key precedence while open,
      mirroring the changeset-overview modal exactly (`j`/`k`/PageUp/
      PageDown/`g`/`G` scroll, `q`/`Esc` close). `App::open_interdiff` /
      `close_interdiff` / `interdiff_scroll_*` in `src/app/review.rs`.
- [x] "Fixes ready — re-review?" notification — dispatching the feedback
      prompt (either target: the feature's existing agent pane or a
      dedicated review session) now registers the tmux session in
      `App::awaiting_review_fixes`, only once the prompt is actually
      submitted (a paste-only, not-yet-sent prompt has nothing to watch
      for). The existing agent-agnostic thinking-status sync
      (`sync_thinking_status`) flips a `started_thinking` flag on that entry
      the next time it observes the session thinking, then — the next time
      it goes idle — raises a distinctly-labeled `review-ready` pending
      input ("Fixes ready — re-review?") instead of the generic "waiting for
      input" one, and clears the watch. Requiring an observed
      thinking-then-idle round trip (rather than just "went idle") avoids
      firing immediately off whatever idle/thinking state happened to
      precede the dispatch. Selecting the notification (`handle_notification_select`
      in `src/app/notifications.rs`) jumps into the feature view and calls
      `trigger_final_review` directly — landing in the diff viewer rather
      than just the pane — where the existing re-review snapshot machinery
      auto-filters to `Changed` files, so this composes with **interdiff**
      above for free. `AwaitingReviewFix` in `src/app/state.rs`.
- [x] Apply suggestions locally — with the line cursor on a kept suggestion,
      press `x` to apply that exact replacement to the worktree; press `X` from
      the viewer to opt into applying every remaining open suggestion when the
      review finishes (before the configured build/test gate runs). Application
      is deliberately conservative: the target must be a regular file inside
      the worktree, its full content must still equal the snapshot loaded into
      the diff viewer, the comment must cover a contiguous current-side span
      (not a deletion-only/mixed-side range), and the anchored source lines must
      still match. Multi-suggestion batches validate once per file and patch
      bottom-up so replacements that add/remove lines do not shift later
      anchors; existing LF/CRLF and EOF-newline shape is preserved. A successful
      application consumes the suggestion, settles its thread, clears a
      suggestion-implied rejection when appropriate, persists progress, and
      refreshes the diff. A stale/dirty or otherwise unsafe suggestion is left
      open for the fixing agent instead of being overwritten. The finish
      message and feedback round report the anchors applied locally plus the
      count/reasons for anything skipped. Core plumbing is
      `apply_suggestions_to_file` / `apply_review_suggestion_jobs` in
      `src/app/review.rs`; key handling and discoverable footer hints live in
      `src/handlers/diff.rs` and `src/ui/dialogs/diff.rs`.
- [ ] Finish summary screen (editable overview of all verdicts /
      comments / suggestions before write + dispatch)

Cost:

- [x] Model selection for the bounded headless passes — `ClaudeLauncher::
      spawn_headless` (`src/claude.rs`) gained an `Option<&str>` model
      override (`--model <name>`), previously only threaded through
      `HeadlessRunner` for AI PR review. Wired `AppConfig::review_model`
      into the walkthrough (`w`), AI co-review (`A`), changeset overview
      (`O`), and the config-wizard diff explain — all four used to always
      run on the CLI's default model with no way to point them at a
      cheaper one.
- [x] Capped `.claude/final-review-feedback.md` growth — rounds were
      prepended and kept forever, but `REVIEW_FEEDBACK_PROMPT` and
      `parse_agent_responses` only ever consume the newest round, so a
      long-lived feature's re-review loop paid to re-read the whole
      history every round for nothing. `split_rounds` /
      `split_overflow_rounds` (`src/app/review.rs`) now keep only the
      newest 2 rounds live and move the rest to a new
      `.claude/final-review-feedback-archive.md` (gitignored in
      `setup.rs`) — full history stays on disk, live-file read cost stays
      flat.
- [x] Batched REVIEW MODE's `.claude/review-notes.md` instruction —
      `ensure_review_claude_md` (`src/app/setup.rs`) told the agent to
      append a note before *every* Edit/Write, doubling the write
      operations for any multi-file task. Reworded to write one note per
      touched file at the end of each logical batch of changes (skipping
      a file that already has a note with nothing new to add), instead of
      one per individual edit. Output format (and `parse_review_notes`)
      unchanged.
- [ ] Per-action model overrides. `review_model` is one setting shared
      across six call sites with different cost/quality needs — PR
      Triage's AI review, the review-memory lookback bootstrap, and (as of
      the item above) final review's walkthrough / AI co-review /
      changeset overview / diff explain. A whole-changeset overview (`O`)
      reading every file's diff at once benefits from a stronger model; a
      single-file walkthrough is a much smaller ask and could run on
      something cheap. Extend `review_model` from one string to a map
      keyed by action, falling back to a single default when unset.
      Explicitly flagged as still-open in the shipped item's own
      CHANGELOG entry.
- [ ] Cap/archive `.claude/review-notes.md` the same way
      `final-review-feedback.md` was capped above. The batching fix
      reduces how *often* the agent writes to this file, but not how
      large it gets — nothing trims it, and
      `generate_review_walkthrough`'s "does this file already have a
      note" check (`state.review_notes.contains_key`, `src/app/review.rs`)
      means the agent — per the REVIEW MODE instructions
      `ensure_review_claude_md` writes into `CLAUDE.local.md` — has to
      read the whole file every batch to decide what still needs a note.
      Same unbounded-history-reread shape the feedback file had, on a
      file that's read even more often (every batch, not just per review
      round). Likely the same shape as `split_overflow_rounds`: keep the
      newest N files' notes (or notes from the last N review rounds) live,
      move the rest to a gitignored `.claude/review-notes-archive.md`.
- [ ] Investigate and implement the most token-efficient strategy for
      dispatching review comments to fixing agents. Benchmark at least four
      shapes on small, medium, and very large review rounds: one fresh agent per
      comment, one agent per file, batches of related comments, and one agent
      for the whole round. Measure total input/output tokens, repeated
      repository/bootstrap context, wall-clock time, fix quality, and edit
      conflicts — a separate agent for every comment may parallelize well, but
      can waste tokens by rebuilding the same context and can race when several
      comments touch the same file or behavior. The likely default to validate
      is adaptive: batch comments on the same file or dependency together,
      parallelize only independent batches, and cap concurrency.

      Do not require every fixing agent to read a potentially huge
      `.claude/final-review-feedback.md`. AMF already parses review state, so it
      should be able to construct a minimal task packet containing only the
      latest unresolved comment(s), severity, file/line or range, bounded diff
      hunk and nearby context, relevant developer note/agent reply, and any
      explicitly related comments. Compare that against reading the complete
      latest round, including the effect of provider prompt caching. Preserve a
      central round manifest so replies, resolutions, retries, and partial
      failures still reconcile into the review file without each worker loading
      or rewriting the whole document. Define size/token thresholds that switch
      strategies automatically, while allowing the reviewer to override the
      choice for unusually coupled changes.

Viewer:

- [ ] **High priority:** close / pause Final Review without finishing — retain
      persisted progress and return to the feature view without writing or
      dispatching feedback, posting to the PR, clearing progress, or updating
      the finished-review snapshot
- [ ] Fix the `v` layout toggle so unified/side-by-side switching is reliable,
      discoverable, and accurately labeled in review mode (including cursor
      binding conflicts and added/untracked-file fallback behavior)
- [ ] Review-round timeline/history browser (`Round 1 ─ Round 2 ─ Current`)
      with round selection, a scrollable read-only summary, carried-thread
      markers, and lazy access to archived rounds
- [ ] Hierarchical, collapsible file tree for easier path-based navigation +
      reduce the Developer Notes panel's default height from ~40% to ~20%
      (preserve full-height expansion)
- [ ] Expand context around hunks (old/new content already loaded)
- [ ] Word-level intra-line diff highlighting + ignore-whitespace toggle
- [ ] Global comment navigation across files + undo last verdict
- [ ] Open at cursored line in `$EDITOR`
- [ ] `?` help overlay for review mode
- [ ] Mouse support in the diff viewer (file list, patch scroll,
      comment cursor)

AI co-review:

- [ ] Whole-changeset co-review (queued per-file passes, progress in
      the file list)
- [ ] Severity + draft suggestions in AI findings
      (`<line>|<severity>|<comment>` + optional fenced replacement)
- [ ] Cross-file context for the co-reviewer (changeset file list +
      hunk headers in the prompt)
- [ ] Ask the AI a question in-line without leaving the review —
      free-form follow-up on the current file / span, answered headless
      and shown in a modal dialog or a second notes box (AI answer on
      the right); reviewer-triggered + per-file for bounded cost

Workflow:

- [ ] Start a review from the dashboard + "review pending" badge
- [ ] Customizable review-notes depth by reviewer experience level (e.g.
      "experienced engineer familiar with the language" vs. "student
      who's never used this language before") — a config setting woven
      into the REVIEW MODE block `ensure_review_claude_md` writes to
      `CLAUDE.local.md`, calibrating how in-depth the agent's
      `.claude/review-notes.md` explanations are

## Open questions

- ~~Line comments: how to anchor a comment to a line that later moves
  (store file + line + a snippet of context, and best-effort re-locate
  on re-review)?~~ Resolved and shipped (**Round 2 → re-anchor
  comments**): each comment stores the commented line plus two
  neighbours on each side; on reload a stale anchor is re-located by
  matching that snippet on trimmed text, disambiguated by neighbour
  agreement, refusing to guess on a tie. What can't be re-located is
  flagged "anchor lost — possibly addressed" rather than dropped, and is
  withheld from inline PR posting so a stale line is never annotated.
- Should multi-line feedback and the agent prompt move to a single
  composed "review summary" the reviewer edits before it's sent, rather
  than assembling the file from per-file inputs? — proposed answer
  captured as the **Round 3 → finish summary screen** item above.
- ~~Dispatching fixes to a new session: default to the same harness as the
  feature, or prompt for one each time? And should the existing pane stay an
  option (e.g. a toggle / picker at finish) rather than being replaced?~~
  Resolved: the existing pane stays the default and is kept as a `t` toggle; the
  dedicated target prompts for a harness each time a fresh session must be
  created (an existing "Final Review" session is reused without prompting).
- ~~For PR integration, how do whole-file vs line-level comments map onto
  GitHub review comments?~~ Resolved: line comments post inline (RIGHT/LEFT by
  side); whole-file rejections and general feedback go in the review summary
  body, since they have no single line to anchor to.

## Reasoning / when to build

Most impactful for the "review like a real developer" goal is
**line-level comments** — it's what whole-file feedback can't do. The
cheapest high-visibility wins are **markdown notes** and the
**multi-line feedback editor**, which pair naturally; a sensible first
slice is those two, then line-level comments as a larger follow-up.
