# Final Review Enhancements

- **Status:** Backlog
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

## Progress

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
- [ ] PR inline-comment integration
- [x] Review history (timestamped / append) — each round is a
      dated `## Review — <ts>` section prepended under a single
      title in `.claude/final-review-feedback.md`; the agent is
      prompted to address only the most recent round
- [ ] Re-review loop (changed-since-last-review)
- [x] File-list filters (undecided / rejected) — press `F` in the final
      review to cycle the file list through all / undecided / rejected;
      navigation (n/p/j/k, g/G) skips hidden files and decisions advance
      to the next visible file.

## Open questions

- Line comments: how to anchor a comment to a line that later moves
  (store file + line + a snippet of context, and best-effort re-locate
  on re-review)?
- Should multi-line feedback and the agent prompt move to a single
  composed "review summary" the reviewer edits before it's sent, rather
  than assembling the file from per-file inputs?
- For PR integration, how do whole-file vs line-level comments map onto
  GitHub review comments?

## Reasoning / when to build

Most impactful for the "review like a real developer" goal is
**line-level comments** — it's what whole-file feedback can't do. The
cheapest high-visibility wins are **markdown notes** and the
**multi-line feedback editor**, which pair naturally; a sensible first
slice is those two, then line-level comments as a larger follow-up.
