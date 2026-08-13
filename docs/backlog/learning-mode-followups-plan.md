# Learning Mode follow-ups

- **Status:** Backlog
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

- [ ] Decide between content-match and SHA+snippet (or both), and
      whether `reanchor_file_comments` can be shared rather than
      duplicated.
- [ ] Re-anchor on history load, next to `reconcile_interrupted_qa`,
      which is already the place where a stored row is reconciled with
      the current world.
- [ ] Render the three outcomes distinctly in the Q&A history and the
      answer pane; a lost anchor keeps its question and answer, since
      those are still the only copy of what someone asked.
- [ ] Tests: an anchor whose code moved down, an anchor whose file was
      deleted, an anchor whose text now appears twice (ambiguous — treat
      as lost rather than guessing), and one that did not move at all
      (must not be reported as re-anchored).

### Open questions

- Ambiguous matches: if `selection_text` occurs more than once, is the
  nearest-to-original the right pick, or is "lost" more honest? Leaning
  lost.
- Whether drift is checked eagerly on load (simple, costs a read per
  distinct file) or lazily when an entry is selected.

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

1. **Anchor drift**, because it is the only correctness problem in the
   list and it degrades the feature's primary use case over time.
2. **Navigable references**, because it is mostly wiring over machinery
   that already exists, and because it partially mitigates the
   fabricated-reference problem that has been observed in real runs.
3. **Alternative actionable mechanisms**, last and only if real use
   shows the TODO path is not enough — the cheaper fixes to that path
   (a better seeded title) should be tried first.
