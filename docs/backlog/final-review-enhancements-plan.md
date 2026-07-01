# Final Review Enhancements

- **Status:** Core shipped; Round 2 in backlog — every item under
  **Progress → Round 1** is implemented and merged. A second round of
  reviewer-workflow enhancements (**Round 2**, below) is captured but
  not yet started.
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
- [ ] Severity tags on comments / rejections (drive the GitHub review
      event + agent prioritization + a severity filter)
- [ ] Agent writes responses back into the feedback file
- [ ] Resolve / unresolve thread state across rounds
- [ ] Re-anchor comments across edits (context snippet + fuzzy
      re-locate; answers the first open question)
- [ ] Changeset overview + diff stats — manual / reviewer-triggered only
- [ ] Build / test gate before approve — per-project configurable check
      command
- [ ] File-level PR comments instead of body-dumping whole-file
      rejections (`subject_type: file`)
- [ ] Jump-by-hunk navigation in the diff
- [ ] Search within the diff
- [ ] Line comment auto-rejects its file (a commented file is implicitly
      "needs revision")

## Open questions

- Line comments: how to anchor a comment to a line that later moves
  (store file + line + a snippet of context, and best-effort re-locate
  on re-review)? — proposed answer captured as the **Round 2 →
  re-anchor comments** item above.
- Should multi-line feedback and the agent prompt move to a single
  composed "review summary" the reviewer edits before it's sent, rather
  than assembling the file from per-file inputs?
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
