# Changelog

All notable changes to AMF are documented in this file.

This changelog follows a Keep a Changelog style layout. Use
`## [Unreleased]` for pending work, then add a dated release section
when cutting a version. Major and minor releases are expected to
document user-facing changes and any migration notes here before they
are tagged.

## [Unreleased]

### Added

- **Combined batch fix in the interactive PR review — "fix all of these, then
  I'll come back."** Mark a set of comments with `space`, then press `B` to
  build **one** numbered prompt covering all of them and inject it into the
  dedicated review session in a single shot. Where `F` queues each marked
  comment as its own prompt to watch through one at a time, `B` is
  send-and-leave: one shared preamble plus a `Comment N:` entry per comment —
  each with its `file:line` pointer, comment text, and diff hunk, and (as with
  a single fix) no file contents — so a big set is the cheapest path in tokens
  and the agent works the whole list while you're away. It reuses the familiar
  fix dialog, so you still get the `~N tokens` preview, editing, and vim keys
  before sending. Everything included is marked `Fixing` and the marks clear,
  so the next refresh reconciles what actually got resolved. Very large batches
  raise a warning toast but still go through.
- **Search within the final-review diff.** Press `/` in the final review to
  incrementally search the current file's diff (case-insensitive). The line
  cursor jumps to the first match as you type; after `Enter`, `n` / `N` cycle
  matches (wrapping) and `Esc` clears the search. Every hit is flagged with a
  `▷` gutter marker and the footer shows the match count.
- **AMF is now licensed under the MIT License.** The repository ships a
  `LICENSE` file, so you can use, modify, and redistribute AMF under
  standard MIT terms. No migration is required.
- **TODOs keybindings in the help overlay.** The `?` help now has an "In the
  TODOs view" section documenting every key in a TODO list — navigate, add,
  edit title/notes/scratchpad, toggle done, cycle priority, reorder, delete,
  and spawn an agent — so they're discoverable without leaving the app.
- **Deleting a TODO list's host feature no longer loses your TODOs.** A
  project's TODO list lives under whichever feature you created it in. When you
  delete that host feature but the project still has other features, AMF now
  prompts you to **re-home** the list onto a surviving feature or **delete** it
  outright — `Esc` keeps the list by moving it to the first surviving feature.
  If no features remain, the now-orphaned list is cleaned up automatically.

### Changed

- **Codex SuperVibe now skips Codex permission prompts.** Codex-backed
  SuperVibe sessions launch with full-access, no-approval permissions, so the
  mode now behaves consistently with its no-prompt warning.
- **Next / previous feature navigation is now opt-in.** The leader `n` / `p`
  shortcuts no longer jump between features by default, and they no longer show
  up in the leader menu unless you bind them. To get them back, bind the
  `next_feature` and `prev_feature` actions in your config (or via the config
  wizard) — they'll reappear in the menu using whatever keys you choose.
- **Freed-up shortcuts moved to lowercase.** With `n` / `p` no longer taken by
  feature navigation, the prompt library (leader menu) and the syntax parser
  picker (dashboard) both moved from `P` to `p`.
- **Building from source is now warning-free.** `cargo build` and
  `cargo clippy` complete without a single warning, so installing AMF from
  source no longer scrolls lint noise past you. CI now enforces this
  (`clippy -D warnings` plus a formatting check), so it stays that way.
  Nothing about AMF's behavior changes.
- **The README caught up with the app.** It now covers the Pi harness, the
  first-run harness setup wizard, the final-review workflow (including the AI
  co-reviewer and suggested changes), PR comment review, per-project TODO
  lists, and the usage/cost meters — and the keybinding tables match the
  in-app help overlay again. Config examples were corrected too: agent and
  mode values in `config.json` must be lowercase (`"claude"`, `"vibe"`), and
  the `feature_presets` docs now list the real field set. Nothing about AMF's
  behavior changes.
- **Review and Plan are no longer labeled "(experimental)".** Both features
  are stable and fully documented in the README, so the feature-wizard
  toggles, the batch-creation dialog, feature-list and view-mode badges, and
  the leader-key help all drop the caveat. Steering keeps its experimental
  label for now.
- **`amf --help` explains itself.** The top-level help text now describes
  what AMF is, and `-V, --version` has a one-line description instead of
  showing up blank.

### Fixed

- **File-level PR review comments no longer inject the whole file as a diff
  hunk.** A comment left on the file as a whole (GitHub's `subject_type:
  "file"`) carries the entire file diff as its `diff_hunk` — fixing it used to
  paste that whole diff into the fix prompt. The fix prompt now references
  `File: path` instead and omits the hunk, with the same backstop for any
  line-anchored comment whose hunk is pathologically large (over 150 lines,
  well clear of the ~90-line hunks ordinary line comments run to).
- **Final-review line comments no longer disappear when the diff changes.**
  Refreshing the diff — or switching the base ref — used to drop your line
  comments out of the gutter, because each one was pinned to an exact line
  number. Comments now follow their code when it moves, including multi-line
  ones. If a comment's line is genuinely gone (the agent already fixed it, say),
  AMF tells you it was "possibly addressed" instead of silently discarding it,
  keeps it in the feedback file, and won't post it to a pull request where it
  would land on the wrong line.
- **Markdown tables show their header rows again.** The in-app Markdown viewer
  now renders table headers and the header/body divider, so plan and review
  docs with tables are readable instead of starting at the first body row.
- **Markdown table alignment is now guarded against regressions.** The in-app
  Markdown viewer keeps left, centered, and right-aligned columns stable,
  including when narrow panes force table cells to truncate.
- **Markdown tables keep uneven and empty cells in place.** Short rows are
  padded to the table's declared column count and empty cells still occupy
  visible grid space, so later cells no longer appear to shift left in the
  in-app Markdown viewer.
- **Markdown tables keep their quote and list indentation.** Tables rendered
  inside blockquotes or list items now preserve the surrounding prefix on every
  border and row, and narrow prefixed tables still clamp to the available
  viewer width.
- **Markdown tables keep cell styling and stay readable when cells are long.**
  Inline code, emphasis, strong text, strikethrough, and links now keep their
  styling inside table cells, and oversized cells truncate with an ellipsis
  without breaking the table borders.
- **Markdown footnotes are readable in the in-app viewer.** Footnote
  references now show as compact inline labels, and definitions render as
  labeled footnote blocks instead of exposing raw `[^label]:` syntax.
- **Markdown math is visible in the in-app viewer.** Inline and display
  formulas now render as styled `$...$` / `$$...$$` text, including inside
  table cells, so plans and notes with formulas no longer look like ordinary
  prose or lose math-specific context.
- **New agent sessions hide their startup command echo.** When AMF starts a
  fresh Claude, Codex, opencode, or Pi session, the embedded pane now shows a
  loading screen until the harness is ready instead of flashing the long tmux
  launch command and environment setup. Existing running sessions open normally.
- **Terminal sessions no longer add surprise blank lines on macOS.** AMF now
  uses the direct tmux input path on macOS, so typing in an embedded terminal
  inserts the intended characters instead of turning each keypress into a
  newline.
- **Dialog footers no longer promise the wrong key.** The first-run harness
  wizard said `c confirm  Esc confirm` — Esc actually confirmed, not
  canceled — so it now reads `c/Esc done`. The New Project dialog said
  `Enter confirm` on every field, but Enter only confirms after the last
  one; it now reads `Enter next` on the Name and Repo path fields and
  `Enter confirm` only once you're on the harness field.
- **Empty project list is no longer flush against the left border.** The
  "No projects yet." message now has the same left padding as every other
  row.

### Migration

- No action required for comment re-anchoring. A review you paused before
  upgrading still resumes; its comments simply can't follow moved code until you
  re-add them, and any that no longer match are flagged rather than dropped.
- No action required for the Markdown table alignment coverage.
- No action required for the Markdown table header fix.
- No action required for the Markdown table styling, table truncation, or
  footnote rendering fixes.
- No action required for the startup loading-screen fix.
- No action required for the macOS terminal input fix.
- No action required for the Review/Plan experimental-label graduation,
  the `--help` text, or the dialog footer and empty-state fixes.
- If you relied on `n` / `p` to move between features in a session, add
  `"next_feature"` / `"prev_feature"` keybindings to your config to restore
  them.

## [v0.29.0] - 2026-07-01

### Added

- **AI co-reviewer first pass in a final review.** Press `A` to have Claude do
  a first pass over the file you're looking at and suggest line comments, so you
  start from a draft instead of a blank diff. Suggestions show up as *draft*
  comments — a hollow `○` in the gutter (vs the filled `●` of a comment you've
  kept) — and you adjudicate each one: with the line cursor active (`c`), `a`
  accepts the draft under the cursor, `d` dismisses it, `Tab` jumps to the next
  one, and `⏎` opens it to edit (editing also accepts). Drafts you don't accept
  are ignored — they never reach the feedback file or a posted PR review — but
  they do survive pausing and resuming a review. It runs only when you ask and
  only on the current file (with the diff capped), so it stays cheap on tokens.
- **Suggested changes in a final review.** With the line cursor active (`c`),
  press `S` to propose a concrete replacement for the line (or `v` range) you're
  on — the editor opens pre-filled with the current code so you tweak it rather
  than retype it. A suggestion can stand alone or ride along with a comment on
  the same line(s). It's written into the feedback file as a fenced
  ` ```suggestion ` block — a verbatim patch the agent can apply directly
  instead of interpreting prose — and, when you have PR posting on, it's
  appended to the inline PR comment as a GitHub suggestion so it's
  one-click-appliable on the pull request.
- **Jump between hunks while commenting in a final review.** With the line
  cursor active (`c`), press `]` / `[` to jump straight to the next / previous
  hunk instead of holding `j`/`k` through a long file — handy for skimming a big
  diff change-by-change. If the cursor is off, the first press turns it on (`]`
  on the first hunk, `[` on the last), and from the middle of a hunk `[` snaps to
  that hunk's top first. A range you're marking with `v` carries across the jump,
  so you can still select a span that spans hunks.
- **A line comment now flags its file as needing work.** Leaving a comment (or a
  suggested change) on a file in a final review is itself a signal the file
  isn't done, so AMF now marks that file "needs revision" for you — no separate
  reject step, and the comments themselves stand in as the reason. Your own call
  always wins: explicitly approving, skipping, or rejecting a file overrides the
  automatic mark and sticks, and clearing a file's last comment clears an
  automatic mark again. Accepting an AI draft comment counts the same as writing
  one; dismissing a draft doesn't. The distinction is remembered if you pause and
  resume a review.
- **Per-project TODO lists.** Add a `TODOs` session from the session picker
  (`s`) to keep a running to-do list for a project — somewhere to jot what's
  left and where you left off instead of holding it in your head. It opens a
  native full-screen list (no tmux pane) with a free-form **scratchpad** note
  at the top (for context, links, or where you left off), and each project gets
  at most one list, shared across all its features. Inside the list: `a` add,
  `e` edit a title, `o` edit longer notes on an item, `space` toggle done
  (completed items get a strikethrough and sink to the bottom, never
  auto-cleared), `p` cycle priority, `J`/`K` reorder, `d` delete (with confirm),
  `b` edit the scratchpad note, and `j`/`k` to move. Press `g` (or `Enter`) on
  an item to **launch an agent for it**: AMF opens a fresh agent session in the
  list's feature — using that feature's harness and mode — and pre-fills the
  composer with a prompt built from the item's title and notes, left editable so
  you review it before sending. Launched items show a marker, and pressing `g`
  again jumps back to the same session (adding onto it rather than spawning a
  second). You can also **quick-capture** a TODO without leaving whatever session
  you're in: from a session view, leader → `N` opens a one-line input that
  appends to the project's list (auto-creating the list if there isn't one yet).
  The list is saved per checkout, so it survives restarts.
- **Fix several PR comments in one pass.** While reviewing PR comments, press
  `space` to mark comments (a `●` flags them, and the footer shows how many),
  then `F` to queue a scoped fix for every marked comment into the review
  session at once — without leaving the pane. The harness works through them
  one after another while you keep triaging, sharing the session's warm file
  context, and each marked comment is flagged `fixing`. Already-resolved marks
  are skipped. Start the review session first with a single `f` (the batch
  queues into that warm session rather than spinning up a cold one).

### Fixed

- **Notification hook scripts are written atomically.** AMF rewrites its
  helper hook scripts (`tool-start.sh`, `notify.sh`, and friends) under
  `~/.config/amf` on startup. Those writes happened in place, so if a script
  changed (e.g. after an upgrade) at the moment the agent harness was executing
  it, the shell could read a half-rewritten file and fail with a spurious
  syntax error — which surfaced as a hook error blocking a tool call. AMF now
  stages each script in a temp file and renames it into place, so a running
  hook always sees a complete file (old or new, never a mix).

- **PR-comment triage marks no longer vanish after the agent pushes a fix.**
  When reviewing PR comments, marking a comment done/skipped, injecting a fix,
  or posting a reply records local triage state — but that state was keyed by
  the PR's head commit, so as soon as a fix was pushed and you reopened the
  review (`G`), the new head SHA meant none of your marks showed up. Triage is
  now keyed by the comment itself and survives a push, so done/skipped/fixing
  marks and skip notes persist across commits and re-opens. Existing triage is
  migrated automatically (any per-commit duplicates collapse to your latest
  mark).

- **Agent usage now binds to the right session more reliably.** New Claude,
  Codex, and opencode panes now pass enough session identity through their
  local hooks/plugins for AMF to attach token usage to the matching dashboard
  session instead of guessing from the newest transcript in the worktree.
- **Inferred usage is safer for older sessions.** When AMF has to fall back to
  workdir/timestamp matching, it no longer assigns the same inferred provider
  session to multiple same-harness panes in one feature; unmatched sessions stay
  unbound until a better match or exact provider event appears.
- **New Codex panes no longer inherit old usage.** A newly created Codex pane
  could briefly show a large cost from an older Codex thread in the same
  worktree before Codex emitted its exact session identity. AMF now ignores
  stale inferred Codex sources and clears any older bad inferred binding on the
  next refresh, so new panes stay blank until their own usage is known.

### Changed

- **Readable comment rows in the interactive PR review.** Each row now leads
  with the **reviewer's name** — bold and in the accent color — so you can scan
  who left each comment at a glance. Long file paths no longer hide the part
  that matters: the location is truncated from the *left* with a leading `…`, so
  the filename and line number stay visible even when the path is too long for
  the pane (e.g. `@reviewer  …/dialogs/pr_review.rs:123`).
- **Feature rows now show total agent usage.** The dashboard feature row shows a
  compact feature-level usage total, while each agent session row and sidebar
  keeps showing only that specific pane's usage. Terminal, editor, custom, and
  unsupported Pi sessions are excluded from the feature total.
- **A real editor for the PR-comment fix prompt.** The confirm dialog that
  shows the scoped fix before it's injected (press `f` while reviewing PR
  comments, then `e` to edit) is now a full editor instead of a plain text
  box. Toggle **vim keys** with `Ctrl+T` — the choice sticks for the rest of
  the PR, so reopening the dialog for the next comment keeps your keymap, and
  the title shows `· vim insert` / `· vim normal` so you know whether `Esc`
  leaves the dialog or just drops to normal mode. Long prompts now **scroll**
  (`Ctrl+J`/`Ctrl+K`, `PgUp`/`PgDn`, with a scrollbar) and follow the cursor as
  you type, and you get undo/redo and word motions for free. **`Tab` injects**
  the prompt straight from edit mode (where `Enter` makes a newline); `Enter`
  still injects from the confirm view.
- **Consistent vim-toggle key across every editor.** All the multi-line editor
  dialogs now toggle vim mode with the same key, **`Ctrl+T`**. Previously the
  steering prompt, the diff-review feedback editor, and the PR-comment fix
  prompt used `Ctrl+V` while the compose box, prompt library, and fill-in
  fields used `Ctrl+T`. They're unified on `Ctrl+T` — which also frees `Ctrl+V`
  from clashing with paste. Footers, the status bar, and the help screen all
  reflect the new key.

### Migration

- No migration is required. Existing inferred usage sources can be replaced
  automatically when the next exact provider event arrives, and stale inferred
  Codex usage is cleared on refresh.

## [v0.28.0] - 2026-06-29

### Added

- **Run final-review fixes in a fresh session.** Press `t` in a final review to
  choose where the "address this feedback" prompt goes when you finish: the
  feature's existing agent pane (the default, as before) or a brand-new
  dedicated review session — the footer shows `t target: live` / `dedicated`.
  Pick the dedicated target and, when finishing needs to spin up that session,
  AMF asks which harness should run it (Claude / Codex / opencode / …) so the
  fixes run in a clean context instead of the long-running review conversation.
  A dedicated "Final Review" session is reused on later rounds without asking
  again. The feedback file is always written first, so skipping the harness
  picker just leaves it for later.
- **Multi-line comments in a final review.** A line comment can now cover a range
  of lines, not just one. With the line cursor active (`c`), press `v` to start a
  selection, extend it with `j`/`k`, then `⏎` to attach a single comment to the
  whole span. The selection is highlighted as you mark it, every line of a saved
  range carries the `●` gutter marker, and the anchor is recorded as
  `src/foo.rs:42-48` in `.claude/final-review-feedback.md`. When posting to a
  GitHub PR, a ranged comment becomes a proper multi-line review comment. Parking
  the cursor on any line of a range peeks the comment and re-opening it keeps the
  range so you can just edit the text.
- **Peek a line comment while browsing the diff.** In a final review, lines you've
  commented on already show a `●` marker in the gutter — now parking the line
  cursor on one also pops the comment's text into a "comment on this line" box
  above the footer hints, so you can re-read what you wrote without reopening the
  editor. Press `⏎` to edit it as before; long comments are previewed up to a few
  lines.
- **Final review can post to your GitHub PR.** Turn on
  `final_review_post_to_pr` and finishing a final review also posts the
  feedback to the branch's PR as a single GitHub review: your line comments
  land as inline comments on the diff, and whole-file rejections plus any
  general feedback go in the review summary. It posts as a plain `COMMENT`
  review (so it works on your own PR) and is best-effort — if there's no PR for
  the branch or `gh` isn't set up, AMF just says so and carries on. The local
  `.claude/final-review-feedback.md` is still written every time, so nothing is
  lost whether or not the PR post succeeds. Off by default; needs an
  authenticated `gh` CLI.
- **Final review flags what changed since last time.** Finishing a final review
  now records a fingerprint of the diff, so the next time you review the same
  feature AMF marks the files that changed since your last pass with a `Δ` in
  the file list (and a `Δ N changed` count in the header). On a fresh re-review
  where only some files changed, the file list automatically narrows to just
  those files and lands on the first one — press `F` to cycle the filter
  (all → undecided → rejected → changed) and see everything again. This closes
  the loop after an agent addresses feedback: you only re-check what it touched.
- **Reply to PR comments from inside AMF.** In the PR comment-review pane you
  can now answer a comment without leaving the tool, with two purpose-built
  replies: press `R` to post a "Done in `<sha>`." reply auto-filled from your
  latest commit (and mark the comment done), or `n` to write why a fix isn't
  needed (and mark it skipped, keeping your note). Both open an editable draft
  and only post when you press `⏎` — replies to inline review comments land in
  the right thread, while conversation comments and review summaries post to the
  PR conversation. The first time you post, if your `gh` login lacks write
  access AMF tells you to run `gh auth refresh -s repo` rather than failing
  cryptically.
- **Resolve PR review threads from the pane.** Press `x` on an inline review
  comment to resolve its thread on GitHub (or reopen one that's already
  resolved) without leaving AMF — the `✓` marker updates immediately. Resolving
  is independent of replying, so you can close out a thread with or without a
  comment. After you post a reply AMF also re-checks thread resolution, so the
  list stays in sync if a comment got resolved meanwhile. Conversation comments
  and review summaries have no thread to resolve and say so.
- **PR review triage state sticks.** In the PR comment-review pane you can now
  mark a comment done (`m`) or skip it (`s`), and injecting a fix (`f`)
  automatically flags the comment as "fixing". These states persist across
  re-opening the PR and restarting AMF, so a long review picks up where you left
  off. The comment list shows a per-comment checkbox (`[ ]`/`[~]`/`[x]`/`[-]`)
  and the detail header a `[fixing]`/`[done]`/`[skipped]` chip — distinct from
  GitHub's own `✓` thread-resolution marker. Mark-done and skip are manual with
  no auto-advance: the selection stays put so you can watch the agent work.
- **The PR review pane is much easier to read.** The comment detail is no longer
  a flat wall of text: it's split into clear sections (header, diff hunk, body,
  and any local note) separated by subtle dividers, with author/role/kind/
  resolution/triage shown as compact chips and a marker legend in the footer.
  Comment bodies render as Markdown — headings, lists, code blocks, and inline
  code instead of raw text — and the diff hunk is colored like a diff (added
  green, removed red) **and syntax-highlighted** for the comment's language
  using the same tree-sitter highlighter as the diff viewer. When a language's
  parser isn't installed it falls back gracefully to plain coloring.
- **Install syntax highlighting without leaving the PR review pane.** When a
  comment's diff hunk falls back to plain coloring because its language parser
  isn't installed, the pane now shows an inline `<Lang> highlighting not
  installed — press i` hint. Press `i` to open the syntax-language picker for the
  selected comment's file — install the parser and you're dropped right back into
  the same comment, now highlighted. This matches the `i` shortcut already in the
  diff viewer. The pane footer and the keybindings help (which gained a dedicated
  "While reviewing PR comments" section) list the new key.
- **Pick a PR to review from a list.** When you press `G` on a feature whose
  branch has no detectable PR — or press `g` inside the review pane to switch PRs
  — AMF now shows a scrollable list of the repo's pull requests (number · title ·
  author · branch, newest first) so you can open one without knowing its number.
  Press `a` to include closed/merged PRs, `#` to type a number instead (the old
  prompt is still one keypress away), and `⏎` to open the highlighted PR. The list
  is fetched in Rust via `gh pr list`, so it spends zero agent tokens.
- **Choose which harness runs your PR-review fixes.** The first time you press
  `f` to fix a comment in a PR, AMF now asks which agent harness the dedicated
  review session should run — so you can triage on a cheaper or faster harness
  than the one your feature is being built with. It highlights the project's
  preferred agent by default; pick one with `j/k` and `⏎` (or `esc` to back
  out). Your choice is remembered for the rest of that PR, so every later fix
  reuses the same warm session without asking again. Only the dedicated review
  session prompts — fixing in your existing live session keeps using whatever
  harness it's already running.
- Added the full Gruvbox Material UI theme family to the theme picker and
  config: dark/light, hard/medium/soft contrast, and material/mix/original
  foreground palettes. Use names like `gruvbox-material-dark-medium` or
  `gruvbox-material-original-light-soft` in `~/.config/amf/config.json`.

### Changed

- Embedded-view auto refresh is now configurable with `view_auto_refresh` and
  defaults to off. Manual refresh and event-driven pane updates still work; turn
  the setting on only if you still need periodic Claude/tmux visual repairs.

### Fixed

- AMF now cleans up the bundled `amf-gruvbox` Opencode theme along with the
  other AMF-managed Opencode themes when removing local Opencode integration
  files.
- Claude panes now render immediately behind the composer when an agent session
  opens with composer input enabled. Previously the composer could appear over a
  blank pane until you closed it or sent input.
- **Composer pastes now show progress.** Pressing `Ctrl+V` in the composer
  immediately shows `[Pasting...]` while AMF reads the clipboard, so slow text
  or image pastes no longer look like the app ignored the keypress.

### Migration

- No action required. AMF adds a `pr_comment_triage` table to its SQLite store
  on first launch; triage state is keyed by PR number, comment id, and head
  commit, and stale rows are pruned automatically after a week. The new
  Gruvbox Material themes are available immediately after upgrade.

## [v0.27.0] - 2026-06-25

### Added

- **Final review reads like a real code review.** Developer notes beside each
  diff now render as Markdown, so headings, lists, and code blocks in your
  `.claude/review-notes.md` show up formatted instead of as raw text.
- **Multi-paragraph review feedback.** The rejection and general-feedback
  editors in the final review are now full multi-line editors: press `Enter`
  for a new line and write proper paragraphs or lists, then `Tab` to submit.
  `Ctrl+V` toggles vim keybindings and `Esc` (or `Ctrl+Q`) cancels.
- **Paste-vs-submit toggle for the final-review prompt.** The new
  `final_review_submit_prompt` config (default `true`) controls whether
  finishing a review with feedback auto-submits the "address the feedback"
  prompt to the feature's agent. Set it to `false` to paste the prompt without
  sending Enter, so you can eyeball or edit it before submitting.
- **Line-level comments in the final review.** Press `c` in the patch to turn
  on a per-line cursor, move it with `j`/`k` (`g`/`G` jump to the first/last
  changed line), then `Enter` to attach a comment to that exact line — the
  cursored row and any commented lines are marked in the gutter (`▶`/`●`).
  Comments are written into `.claude/final-review-feedback.md` under a
  `## Line Comments` section anchored as `### <file>:<line>`, so the agent gets
  precise, location-specific feedback that whole-file rejection can't express.
  `Esc` exits the cursor; `q` still finishes the review. (Inline markers are
  shown in the unified diff layout.)
- **On-demand AI walkthrough for noteless files.** When a file in the final
  review has no developer note in `.claude/review-notes.md`, press `w` to
  generate a concise walkthrough of that file's diff with a headless Claude
  run. The Developer Notes panel shows "Generating walkthrough…" while it works
  and then renders the result as markdown (titled "AI Walkthrough"), cached per
  file so navigating away and back doesn't re-run it — so the notes panel is
  never empty even when the developer didn't leave a note.
- **Finish gating + jump-to-next-undecided in the final review.** Finishing a
  review (`q`/`Esc`) while one or more files still have no verdict now asks for
  confirmation instead of ending silently — press `q`/`y` to finish anyway,
  `Esc` to keep reviewing, or `u` to jump straight to the next file with no
  verdict (wrapping). The footer shows how many files are still undecided, so a
  half-done review isn't finished by accident.
- **Pause and resume a final review.** Your verdicts, line comments, and
  general feedback are now saved to `.claude/final-review-progress.json` as you
  go, so a long review survives quitting AMF (or a crash). Reopening the final
  review for the same feature restores everything where you left off; finishing
  the review clears the saved progress so the next one starts fresh.
- **Choose the diff base ref.** Press `b` in the branch diff viewer or final
  review to compare against any branch, tag, or commit instead of the
  auto-resolved base (`origin/HEAD` → `main` → `master`). The header shows a
  `(manual)` marker while an override is active; submitting a blank entry
  reverts to automatic resolution.
- **Final review keeps a history.** `.claude/final-review-feedback.md` is no
  longer overwritten each round — every review is now appended as a dated
  `## Review — <timestamp>` section, with the newest round first under a single
  title, so you keep a trail across rounds. The agent is prompted to address
  only the most recent round, so accumulated history doesn't make it re-do
  already-fixed items.
- **Filter the final-review file list.** Press `F` in the final review to cycle
  the file list through all → undecided → rejected → all, so a large changeset
  can be narrowed to just the files still needing attention. The Files panel
  title shows the active filter and visible/total counts, navigation
  (`n`/`p`, `j`/`k`, `g`/`G`) skips hidden files, and approving/rejecting a file
  advances to the next visible one.

- **Insert an agent skill into a prompt.** While editing a prompt body (or
  filling a text placeholder), press `Ctrl+K` to open a search-as-you-type
  picker of the agent skills available in your workspace — the same
  `~/.claude/skills` and project `.claude/skills` the compose `/command` popup
  draws from. Selecting one inserts its `/skill-name` invocation at the cursor,
  so the agent expands the skill when the prompt is delivered.

- **Vim editing in the prompt library.** The New/Edit Prompt body editor now
  supports a vim keymap you can toggle with `Ctrl+T` (just like the compose
  box), and the box title shows `[Vim Insert]` / `[Vim Normal]` so you always
  know which mode you're in. Multi-line placeholder fill fields gain the same
  `Ctrl+T` toggle, and the choice carries across slots while you fill them in.

- **Prompt library shows where each template lives.** The picker preview pane
  now displays the resolved source file for the selected entry — the local
  SQLite store for your own (`User`) templates, or the `.amf/config.json` path
  for `Project` / `Worktree` / `Global` templates. The export prompt (`x`) now
  names each target's exact path before you choose — e.g.
  `(g) ~/.config/amf/config.json   (p) <repo>/.amf/config.json` — so you can
  confirm which repo a template lands in. The New/Edit dialog adds a hint
  showing where a save will be written, calling out that `User` templates live
  in the local store and aren't version-controlled.

- **`/amf:add-prompt` skill.** A new skill (parallel to `/amf:add-preset`) that
  adds a declarative prompt template to your workspace without hand-writing
  JSON. It writes a `prompt_templates` entry to `.amf/config.json` (project) or
  the global `~/.config/amf/config.json`, and documents the template schema —
  including tags, inline `{{slot}}` / `{{name|opt1|opt2}}` syntax, and explicit
  text / multi-line / select placeholders. The template then shows up in the
  prompt library picker as a read-only `Project`/`Global` entry.

- **Tags for prompt templates.** The New/Edit Prompt dialog now has a `Tags`
  field (Tab cycles Name → Tags → Body); enter a comma- or space-separated
  list and AMF stores it with the template. Tags show as `#chips` in the
  picker's preview pane. In the picker's search (`/`), the query now also
  matches tags, and a `#`-prefixed query filters by tag only — `#frontend`
  narrows to templates tagged `frontend`, and a bare `#` lists every tagged
  template.

- The vim-mode editor now accepts numeric count prefixes, so you can repeat
  motions and edits the way you would in vim: `3w`, `5j`, `2dd`, `d3w`, and
  combined counts like `2d3w`. A leading `0` still jumps to the start of the
  line unless you are already typing a count (`10w`).

- **Review a PR's comments inside AMF.** Press `G` on a feature in the
  dashboard to open a full-screen pane listing the open pull request's
  comments for that branch — inline review comments, review summaries, and
  conversation comments together, each showing its file/line, author, a
  one-line preview, and whether the thread is resolved. Navigate with
  `j`/`k` to read the full comment and its diff context, scroll long comments
  with `Ctrl+D`/`Ctrl+U` (or `PgDn`/`PgUp`), and press `h` to hide comments
  already resolved on GitHub so you can focus on what's still open (a counter
  shows how many are hidden); `esc`/`q` closes. If the branch has no open PR,
  AMF prompts for a PR number instead of giving up, and you can press `g` from
  inside the pane any time to pull up a different PR's comments by number (a bad
  number reports the error inline so you can correct it). Everything is fetched
  with the GitHub CLI and stays read-only for now — acting on comments (sending
  a fix to the agent, replying) is coming in a later release. Requires the `gh`
  CLI installed and authenticated. Re-opening a PR whose latest commit hasn't
  changed is now instant and makes no network calls — comments are cached
  locally per PR and head commit — and pressing `r` in the pane refreshes from
  GitHub (picking up new comments and any commits you've pushed) on demand.

- **Send a review comment to an agent as a fix.** In the PR-review pane, press
  `f` on a comment to turn it into a tightly-scoped fix prompt for an agent
  session. `f` now opens a confirm dialog first that shows the exact text that
  will be sent — the comment, its `file:line`, and GitHub's diff hunk, with no
  file contents — alongside a `~N tokens` estimate and the target session, so
  nothing reaches the agent until you approve it. Press `e` to edit the prompt
  in place before sending (`Esc` back to the confirmation), `Enter` to inject,
  or `Esc` to cancel. On inject AMF drops you into the session to watch it
  work; the prompt is pasted without auto-sending so you can still eyeball it
  there. By default the fix goes to a dedicated "PR Review" session that AMF
  spins up once and reuses for every fix on that PR (so the per-session
  overhead is paid once); press `t` to switch the target to the feature's
  existing live session for warm in-progress context. The footer shows the
  current target (`f fix→dedicated`).

### Changed

- **Clearer feature setup options.** The new-feature mode step now shows a
  distinct details callout when you hover or focus optional settings like
  Review, Plan, Chrome, Remote Control, and Steering, making the tradeoffs
  easier to understand without crowding the mode picker.

- **New/Edit Prompt dialog redesign.** Each field (Name, Tags, Prompt body)
  now sits in its own labelled box with clear spacing instead of stacking
  flush against each other. The focused field's border highlights, the
  placeholder-syntax legend moved onto the body box's bottom border, the key
  hints moved onto the dialog's bottom border, and the "where this saves"
  hint sits at the bottom — so the form is much easier to scan.

- The AMF prompt composer now works in Codex, OpenCode, and Pi sessions with
  the same workflow and keybindings as Claude Code. Start typing to compose,
  use `Ctrl+E` or `leader+e` to switch between compose and direct input, and
  use drafts, prompt templates, multiline input, clipboard paste, and image
  attachments without changing workflows between agent harnesses. Terminal,
  editor, and custom sessions continue to use direct input.
- Opening an agent session now shows the composer immediately when composer
  input is enabled, so you can start drafting without typing a throwaway first
  character. Sessions switched to direct input still open directly into the
  pane.

### Fixed

- Claude sidebars now find todo lists that Claude stores under a separate
  task-store ID instead of the visible session ID, so the sidebar's `Todos`
  panel shows the current checklist again.
- Pasting now works in the prompt library editor. Bracketed-paste events
  were dropped in the New/Edit Prompt dialog and the placeholder fill flow,
  so clipboard text never landed in the name, tags, body, or fill fields.
  Pastes now route to the focused single-line field (newlines collapsed) or
  the multi-line body/fill editor.

- Dialogs opened from a feature now keep a compact project/feature/session
  context bar visible, so you can still tell which workspace you are acting on
  while reviewing diffs, editing prompts, or using other overlays.

- Pasting images into the prompt composer now works on Windows/WSL. The
  composer reads screenshots and copied images from the Windows clipboard,
  and when you send a prompt the image now reliably arrives with your text
  instead of being left behind in the agent's input box.
- Claude panes now keep their periodic automatic repaint when direct tmux
  transport is active, preventing the embedded UI from becoming mangled on
  Linux even though control mode is disabled by default.

- Adding a session to a stopped feature now starts the feature automatically,
  so the new session opens immediately and the dashboard returns to a green
  status. If AMF cannot start the feature or create the session, it now shows
  an error toast instead of appearing to do nothing.

- Scrolling to the bottom of a developer note in the final review now reaches
  the true visual bottom. Long notes that soft-wrap or expand under Markdown
  rendering were clamped to the raw line count, so the last lines stayed out
  of view; the scroll limit now tracks the rendered (wrapped) height.

### Migration

- No migration is required. The composer is enabled by default for all
  built-in agent harness sessions; use `leader+e` per session to opt into
  direct input.

## [v0.26.0] - 2026-06-22

### Added

- **Fill-in placeholders for prompt templates.** Injecting a template that
  contains `{{slot}}` markers now walks you through one field at a time,
  collecting a value for each slot before the prompt is delivered. A `2/3`
  counter shows your progress; `Tab`/`Enter` move to the next field,
  `Shift+Tab` goes back, and `Ctrl+S` injects at any point. Slots are filled in
  the order they appear in the prompt, pre-seeded with any configured defaults,
  and required slots block injection until filled. Templates without slots
  inject immediately as before.

- **Menu (pick-from-a-list) placeholders.** A slot can offer a fixed set of
  choices instead of free text: when you reach it in the fill-in flow, you pick
  from a list with `↑`/`↓` (or `j`/`k`) and `Tab` to confirm. Define one right
  in the prompt body by listing the choices with `|` — `{{a|b|c}}` makes every
  segment a selectable option, for example `Deploy to {{dev|staging|prod}}`. Add
  an optional heading with a leading `label:` — `{{env: dev|staging|prod}}` shows
  "env" above the same three choices. Plain `{{name}}` stays a free-text slot.
  The New/Edit Prompt editor now shows a short, always-visible help line
  explaining all three forms (it no longer disappears once you start typing).

- **Save a past prompt to the library.** In the recent-prompts menu
  (`leader+L`), press `s` to save the highlighted prompt as a new reusable
  template — it opens the editor pre-filled so you can name and tweak it.

- **Worktree export target for prompt templates.** Pressing `x` → `w` in the
  prompt library exports a template to the active feature's worktree
  `.amf/config.json` instead of the main project config. Worktree templates
  appear with a distinct yellow `[Worktree]` badge so you can tell them apart
  from `[Project]` templates. This lets you commit a prompt on a feature branch
  and promote it to the main repo via git when it's ready.

- **Edit prompts from any source.** Project, Global, and Worktree templates can
  now be opened in the prompt editor with `e`. Saving writes back to the config
  file they came from — SQLite for `[User]` templates, the appropriate
  `.amf/config.json` for config-file sources. Renaming a config-source template
  removes the old entry so stale names don't accumulate.

- A running bug backlog in `docs/backlog/bug-backlog-plan.md`, viewable
  in AMF's in-app Markdown viewer alongside the existing plans, so known
  issues are tracked in one place. First entry: pressing `A` on the
  dashboard does not open harness setup.

### Changed

- All four template sources (User, Worktree, Project, Global) are now shown
  independently in the prompt library with no cross-source deduplication.
  Previously, a Worktree or Project template with the same name would silently
  hide the Global copy, making freshly-exported templates appear to vanish.
  Each scope's copy is now always listed so you can see exactly where a prompt
  lives and manage them individually.

- The library list updates immediately after every mutation — export, create,
  edit, or duplicate — and the cursor jumps straight to the affected entry.
  Previously the list reset to position 0 and newly-exported entries could be
  invisible until you closed and reopened the picker.

- Global templates are re-read from disk each time the library opens, so
  changes made while AMF is running (including exports from the same session)
  take effect without restarting the app.

- Export failures now surface as a toast instead of doing nothing silently.
  A corrupt or unwriteable config file shows an explicit error message.

- Only `[User]` templates show the `d` delete hint. Config-source templates
  (`Project`, `Global`, `Worktree`) show `e edit` but not `d del`, since
  deleting those requires editing the config file directly. Pressing `d` on a
  config template now immediately explains this instead of arming a confirm
  dialog that then refuses to act.

- **Direct tmux transport is now the default.** AMF no longer attaches a
  persistent control-mode (`-CC`) client to drive agent panes; it talks to
  tmux directly, the same way it always has on Windows. This is what fixes the
  long-standing garbled-pane corruption (see Fixed). Control mode remains
  available as an opt-in via `tmux_control_mode: true` in `config.json` (or the
  `AMF_TMUX_INPUT_TRANSPORT` / `AMF_EXPERIMENTAL_PERSISTENT_TMUX_INPUT` env
  vars). The periodic automatic pane refresh and the SIGWINCH re-anchor bounce
  now run only in control mode, where they're needed — direct mode re-captures
  the full pane each frame and self-heals, so it no longer flickers from those.

- The diff-review popup now holds for **1.5s** by default instead of 3s.

  **Migration:** on first launch after upgrading, AMF rewrites `config.json`
  once to apply the new defaults — a persisted `tmux_control_mode: true` is set
  to `false` and a `diff_review_popup_hold_secs` of `3.0` becomes `1.5`. Values
  you previously customized away from those old defaults are left untouched, and
  the file is stamped with a `config_version` so the migration never runs twice
  (re-enable control mode afterward and it stays enabled).

- **Retired the legacy vimdiff diff-review viewer.** The native in-app AMF diff
  viewer is now the only reviewer; the neovim/vimdiff popup path and its bundled
  `plugins/diff-review` scripts have been removed. Existing configs that still
  set `diff_review_viewer = "nvim"` (or the old `"legacy"` alias) keep working —
  the value now deserializes to the AMF viewer instead of erroring.

### Maintenance

- Repo cleanup pass: removed dead code across the codebase (unreferenced tmux
  helpers, orphaned methods/constants, and a never-run test) and dropped the
  crate-wide `#![allow(dead_code)]` so new dead code is caught going forward.
  Items intentionally retained (write-only fields, serde/API types, and
  test-only helpers) now carry targeted `#[allow(dead_code)]` annotations. No
  behavior change; all tests pass.

### Fixed

- **Garbled / mangled agent panes on Linux and macOS.** The persistent
  control-mode tmux clients AMF attached to each session could desync Claude
  Code's incremental renderer, leaving stale cells in the pane grid (the input
  box bleeding into the divider above it). Windows was never affected because
  that transport is Unix-only. Defaulting to the direct transport removes the
  extra clients and the corruption with them.

- Diff/review mode no longer shows AMF-generated runtime files as if they
  were branch changes. The change-tracker log (`.amf/change-history.json`)
  and the opencode TUI state (`tui.json`) are now gitignored and untracked,
  so the review surface reflects only the real changes on your branch.

- Pressing `A` on the dashboard now opens the agent-harness setup picker as
  expected. The key was only wired into the `Ctrl+Space` leader chord, so a
  bare `A` did nothing. It is now a top-level dashboard action (and listed in
  the `?` help overlay) alongside the other capital-letter shortcuts.

## [v0.25.0] - 2026-06-19

### Added

- Prompt library (phase 1): save reusable prompts and inject them into a
  session on demand. Open the picker with `leader+P` while viewing a
  session, or `L` from the dashboard. `Enter` injects the selected
  prompt — into the compose box when compose is on (so you can review
  and edit before sending), or pasted straight into the agent window
  without sending when compose is off. Manage entries from the picker
  (`n` new, `e` edit, `d` delete, `y` duplicate, `/` search) or export
  one to declarative config with `x` → `g` (global
  `~/.config/amf/config.json`) or `p` (project `{repo}/.amf/config.json`)
  so it can be version-controlled and shared. From the compose box,
  `Ctrl+P` opens the library to inject a saved prompt (mirroring
  `leader+P`) and `Ctrl+S` saves the current buffer as a new template.
  Templates are
  stored in the AMF SQLite database. Fill-in `{{placeholder}}` slots and
  declarative/team templates are planned for later phases.
- Vim normal mode now supports the change operator: `c` with a motion
  (`cw`, `cb`, `c$`, …) deletes and drops into insert mode, `cc`/`S`
  changes a whole line (leaving it empty rather than removing it), and
  `C` changes to the end of the line. The delete and the text typed
  afterwards undo together as one step.

### Documentation

- Optimized the README for LLM/AI-assistant retrieval (c7score):
  merged install and upgrade steps into copy-paste-ready blocks,
  removed screenshot placeholders, and trimmed the duplicated `src/`
  file tree in favor of a pointer to `CLAUDE.md`.
- Fixed README issues found during the cleanup: a Catppuccin theme
  list and a `cargo build` snippet that had drifted into the wrong
  sections, and a dead link to a non-existent
  `docs/docker-screenshots.md`.
- Removed the `zai` (Z.AI usage limits) configuration section from the
  README. The feature is not reliable enough to document yet; the
  config key still exists for future use.
- Added a top-level `llms.txt` navigation file so AI tools can quickly
  locate the README, architecture docs, and automation guides.

### Changed

- Final review (press `f` in a feature view) now runs entirely inside
  AMF instead of opening tmux/vim popups. You step through every changed
  file with syntax-highlighted diffs, read the agent's own per-file
  reasoning beside each change (captured by review mode in
  `.claude/review-notes.md`), and approve, reject with feedback, or skip
  each file — plus leave general feedback that isn't tied to a file. On
  finish, AMF writes `.claude/final-review-feedback.md` and prompts the
  feature's agent to read it and address every item, so a review round
  flows straight back to the agent without copy-pasting.
- The per-file change review now finds a developer's note even when the
  heading includes a title (e.g. `## path — summary`), not only a bare
  path heading, so your review-mode notes show up more reliably.
- Feature view now highlights the active worktree name with a stronger
  accent color, making labels like `/visual-updates` easier to spot in
  the header.
- Renamed session bookmark internals and UI copy to remove legacy
  external-editor terminology.

### Fixed

- Final review no longer crashes with a tmux `open terminal failed: not
  a terminal` error. The previous version tried to attach a separate
  terminal session for its vim/popup walkthrough, which doesn't work
  with AMF's embedded tmux; the review now stays inside AMF.
- The Ctrl+Space leader menu now lists the bookmark shortcuts for
  opening bookmarks, adding or removing the current session, and jumping
  to saved bookmark slots, so the on-screen help matches the available
  commands.

### Migration

- The AMF database gains a `prompt_templates` table (schema migration
  `007`), applied automatically on first launch. No user action is
  required.

## [v0.24.0] - 2026-06-16

### Added

- Vim mode in the compose and steering-prompt inputs now supports undo
  and redo: press `u` in normal mode to undo and `Ctrl+R` to redo.
  Everything typed during a single insert session is undone in one step.
- Vim normal mode now supports operator + motion editing: `d` combined
  with a motion (`dw`, `db`, `de`, `d$`, `d0`, `dh`, `dl`), line
  deletes (`dd`, and `dj`/`dk` for multiple lines), `D` to delete to
  end of line, and the `e` motion to jump to the end of a word.
- Vim normal mode now supports yank and paste: `y` with a motion
  (`yw`, `y$`, …), `yy`/`Y` to yank a line, and `p`/`P` to paste after
  or before the cursor (charwise or linewise). Deletes and `x` feed the
  same register, so `ddp` and `xp` work as in vim.
- Remote Control: drive a feature's local Claude session from
  claude.ai/code or the Claude mobile app while the agent keeps running
  on your machine in tmux. Turn it on with the new "Remote Control"
  toggle when creating a Claude feature, or set it as the default for
  all new Claude features in config. The toggle disables itself with a
  reason when it can't be used — on z.ai or other third-party providers,
  or with a Claude Code older than v2.1.51 — so you never start a session
  that can't connect.
- A `[remote ●]` badge appears in the session view when Remote Control
  is connected, and RC-enabled features are marked `[remote]` in the
  project list. From a Claude view, the Ctrl+Space menu adds: `c` to
  copy the session URL, `O` to open it in your browser, and `C` to
  toggle Remote Control on or off at runtime. When the link only lives
  in Claude Code's footer (and can't be read back), AMF tells you to use
  that footer link rather than failing silently.

### Changed

- Review mode, final review, PR comment review, plan mode, and the steering
  coach now show an experimental label in the UI, so users can tell these
  workflows are still being refined before they opt into them.

### Fixed

- Opening the branch diff viewer and Markdown viewer now shows a
  loading indicator while AMF gathers files or reads content, so slower
  startups and large worktrees no longer look like the app ignored the
  command.

### Migration

- No migration is required.

## [v0.23.0] - 2026-06-15

### Added

- New compose input for Claude Code sessions that sidesteps Claude
  Code's input-box rendering glitches. Start typing in a Claude view
  and an AMF-drawn input opens over Claude Code's own input box; press
  Enter to send the finished text in one shot (Alt+Enter inserts a
  newline). Claude Code's output stays visible and live above the box
  while you type.
- Typing `/` in the compose input opens a slash-command menu listing
  Claude Code built-ins, your global and project custom commands, and
  skills, with descriptions. Arrows or Ctrl+P/N select, Tab completes,
  Enter runs. Commands that open Claude Code's own dialogs (such as
  `/model` or `/config`) automatically hand control back so you can
  drive them directly.
- Images can be pasted into the compose input with Ctrl+V. They show
  as `[Image 1]` placeholders and are delivered to Claude Code as real
  image attachments on send, so the agent can see them. Text on the
  clipboard pastes as usual.
- A direct-input escape hatch for when you want keys to go straight to
  Claude Code again: press Ctrl+E in the composer (or `leader+e` in
  the view) to disable the compose input per session, shown with a
  `[direct input]` badge; `leader+e` — also listed in the Ctrl+Space
  menu — turns it back on. Ctrl+Space inside the composer opens the
  leader menu directly.
- Unsent compose drafts (text and attached images) survive closing the
  box; the next keystroke in that session restores them. Submissions
  also clear any leftover text in Claude Code's input first, so stray
  typed characters can no longer merge into your prompt.

### Fixed

- The toast shown when the composer switches a session to direct input
  (after running an interactive command like `/model`, or via
  `leader+e`/Ctrl+E) was too long and got truncated, hiding the part
  that told you how to get back. It now clearly reads "Composer off —
  leader+e to re-enable" as a warning so the state change stands out.
- Claude Code panes could garble in the embedded view: the input box
  drifted up a row and bled its text into the divider above it, and a
  repaint (leader-R) only cleared it until the next update. The garble
  is in the real tmux grid — Claude Code's incremental renderer draws
  its input box at a stale anchor row and leaves the vacated cells
  behind — so AMF was faithfully showing corrupted pane content rather
  than mis-rendering. AMF now re-anchors a live Claude pane every few
  seconds with a one-row SIGWINCH bounce, forcing Claude Code to fully
  repaint and clear the stale cells. The bounce is hidden: the display
  holds its last good frame while the shrink/restore and repaint happen
  off-screen, so a clean pane shows no wobble and a garbled one just
  resolves in place. Other harnesses fully repaint on their own and are
  left untouched.
- The control-mode view worker also re-captures the full pane on a
  ~250ms self-heal floor instead of only on detected output, so any
  frame the change-notifier misses no longer lingers until the 3s drift
  reseed.

### Migration

- The compose input is on by default for Claude Code sessions, so
  typing in a Claude view now opens the AMF composer instead of going
  straight to Claude Code. If you prefer the old behavior for a
  session, press `leader+e` (or Ctrl+E inside the composer) to switch
  that session to direct input.

## [v0.22.0] - 2026-06-12

### Added

- The startup loading screen now shows short tips for easy-to-miss
  commands, including the view refresh shortcut for fixing visual
  glitches in embedded sessions.
- Leader → Shift-R in view mode now repaints the agent's screen on
  demand. If an agent's display ever desyncs mid-turn (text appearing
  on the wrong line while it streams — a Claude Code rendering bug,
  not an AMF one), one keystroke forces a full redraw instead of
  waiting for it to fix itself.
- Agent sidebars now show the active model when AMF can determine it
  for Claude, Codex, and OpenCode sessions, making it easier to confirm
  which model a running agent is using.

### Changed

- Pending input requests can now surface sooner when AMF starts in an
  embedded session view. The default input-request startup wait is now
  1.5 seconds, and you can tune it with
  `input_request_wait_seconds` in `~/.config/amf/config.json`.
- Diff-review prompts now only open automatically while you are viewing
  the feature that requested the review. From the dashboard or another
  feature's view, the review is added to the pending input requests and
  announced with a toast instead of stealing focus; open it from the
  input picker, by entering the feature view, or with `V`.

### Fixed

- The embedded view no longer corrupts permanently when something else
  resizes an agent's pane behind AMF's back (a second AMF instance,
  attaching the session directly in another terminal). AMF now
  notices the size drift within a few seconds and restores it, and it
  backs off instead of fighting if another instance keeps resizing.
- The embedded pane now fills the full view area (it was always two
  rows short), and typing echo lands on the correct row. A subtle
  capture quirk shifted the whole frame up one line whenever the
  screen was exactly full, leaving the cursor floating below the text.
- Typing into an agent no longer turns sluggish while it is streaming
  output. Screen updates from fast agent output are now paced so
  keystrokes keep priority, and keystroke echo is never delayed behind
  a backlog of redraws.
- Dragging or re-tiling the AMF window now resizes the agent's pane
  once, after the size settles, instead of once per animation frame.
  Repeated mid-stream resizes were the main way garbled "ghost" rows
  got baked into an agent's scrollback.
- AMF now detects when its tmux server was started by a different AMF
  build with a different bundled tmux version. Previously this
  mismatch silently broke the fast view path and session-event
  updates; both now fall back cleanly, with a clear note in the debug
  log.
- Session status events now arrive from the tmux server AMF actually
  uses. They were being watched on the wrong server, so status changes
  silently fell back to slow polling for everyone.
- Codex sidebar model details now remain visible after usage and
  activity lines appear by letting the Status section grow to fit its
  contents.
- The vibeless diff-review popup now appears on macOS. The hook's
  `amf notify-wait` call bound its reply socket under `$TMPDIR`, whose
  long per-user path on macOS exceeds the Unix socket path limit, so
  IPC delivery silently failed on every review. Reply sockets now live
  in the short state directory used by the main AMF socket.
- Fallback notification files (written when IPC delivery fails) now
  open the diff-review popup automatically instead of waiting for a
  manual `V` refresh. The existing filesystem watcher picks them up
  the moment they are written; no polling was added.
- The Terminal option is back in the new-session picker. It was
  accidentally dropped in v0.20.0 alongside an intentional change to
  the new-feature dialog, so adding a plain terminal to a running
  feature was impossible. The new-feature dialog is unchanged.

### Migration

- No migration is required. To override the new default, set
  `"input_request_wait_seconds": 1.5` in
  `~/.config/amf/config.json` and adjust the value as needed.

## [v0.21.0] - 2026-06-11

### Changed

- AMF now stays fast after running for hours or days. Usage and token
  statistics are computed in the background and re-read only what
  changed since the last check, so input no longer stalls as the day's
  agent transcripts grow.
- View mode now uses far less CPU while idle. The embedded pane is
  updated when output actually arrives instead of being re-captured
  many times per second, with no change to typing echo or streaming
  responsiveness.
- Hook and agent notifications now wake AMF immediately instead of
  waiting for the next poll tick, so toasts and pending-input alerts
  appear without delay.
- The debug log (`~/.local/state/amf/debug.log`) is now capped at
  10 MB with one rotated generation kept, so it no longer grows
  without bound. Routine per-message IPC chatter is summarized once
  per 5 seconds instead of logged line by line.
- The dashboard now reacts immediately when agent sessions start or
  stop: AMF listens for tmux session events instead of polling every
  5 seconds, so a feature that finishes or dies shows its new status
  right away.
- Agent activity indicators and sidebar prompts now update from
  filesystem events instead of timed scans. Thinking status appears
  as soon as an agent reports it, and an idle AMF does close to zero
  background work regardless of how many features exist.
- AMF now keeps a small hidden tmux session (`_amf-observer`) while
  running so it can receive those session events. It never appears in
  AMF's pickers and is removed when AMF exits.

### Fixed

- Fixed periodic input stalls in view mode caused by usage statistics
  being recalculated on the main thread while agents were streaming.
- An idle embedded pane no longer spawns background tmux processes
  several times per second.

### Migration

- No migration is required. The token-usage cache database is upgraded
  automatically on first launch.

## [v0.20.0] - 2026-06-03

### Added

- New feature creation now includes a session naming step before launch,
  prefilled with the default harness name such as `Claude 1` or
  `Codex 1`, so you can rename the initial agent session before it
  starts.
- Starting an additional session now asks for a session name after you
  choose the session type, with the current automatic name filled in by
  default.
- Existing-worktree feature creation now supports `/` search in the
  worktree picker, so large worktree lists can be filtered before
  selecting one.

### Changed

- The session picker no longer offers `Terminal` as a built-in session
  type.
- Feature creation now scopes feature session and worktree names by
  project, so separate projects can both use names like `main` or `tt`
  without attaching to the wrong session or colliding on the same
  worktree path.

### Fixed

- The new feature form now shows duplicate or invalid feature names
  inline on the `Name` field and lets you correct the name before
  continuing.
- Fixed slow typing in the new feature form by avoiding repeated config
  lookups while the dialog redraws.
- Fixed Vibeless diff reviews that could get stuck on Claude's file
  update step when Claude reported a working directory that did not
  match AMF's stored feature path. AMF now identifies the waiting
  review by its managed tmux session first, so the review dialog opens
  immediately instead of requiring the manual `V` recovery shortcut.
- AMF now skips broken Claude Code auto-update binaries when launching
  Claude sessions or headless Claude commands. If the newest installed
  Claude binary fails `--version`, AMF tries the next installed version
  before falling back to `claude` on `PATH`, so a bad Claude update no
  longer prevents AMF-managed Claude sessions from starting.

### Migration

- No migration is required.

## [v0.19.7] - 2026-05-14

### Changed

- Pending diff reviews now show up in `Work -> state` for Claude,
  Codex, and Opencode sessions, with a `leader V` hint when the
  review popup is not appearing.

### Fixed

- Pending Claude diff reviews now stay visible in the sidebar `Work`
  section while they are waiting, and clear from the sidebar after the
  review is submitted.
- Fixed vibeless diff-review not appearing when AMF was on the
  dashboard. Previously, a diff-review arriving while the dashboard was
  open would be silently queued and never shown — the review request
  would time out after 120 seconds and the agent would stall waiting
  for a response that never came. This was most noticeable on macOS.
  The review dialog now opens immediately regardless of which screen
  you are on.

### Migration

- No migration is required.

## [v0.19.6] - 2026-05-14

### Added

- Vibeless diff reviews now have a recovery shortcut, so you can check
  for a pending review from the dashboard or embedded view and open it
  manually when the normal popup flow gets stuck. This is available as
  `V` on the dashboard for feature/session rows and as `Ctrl+Space`
  then `V` while viewing a session.

### Changed

- Opencode sidebar updates now use AMF's IPC path when available and
  keep fallback file checks off the UI thread, so sidebar refreshes no
  longer risk making the dashboard or embedded view feel stuck. That
  keeps the view responsive while sidebar state is loading or updating.

### Migration

- No migration is required.

## [v0.19.5] - 2026-05-11

### Fixed

- Fixed the remaining sources of startup latency that persisted through
  v0.19.4. Three changes ship together:
  - **Prompt cache tail-read**: `read_prompts_from_claude_sessions` now
    reads only the last 64 KB of the most-recently-modified session file
    per feature instead of loading all `.jsonl` bytes across every session
    file. For features with a long Claude history this reduces prompt-cache
    time from seconds to microseconds.
  - **Token-count off the hot path**: the today-token calculation
    (`calculate_claude_today_tokens`) previously blocked the startup
    usage-refresh task by reading every `.jsonl` file modified today
    across all `~/.claude/projects/` subdirectories. It now runs in a
    dedicated background thread; the usage display updates once the count
    arrives without delaying the dashboard.
  - **Loading gate trimmed**: the session-status background thread
    (`session_status_bg`) no longer holds the "Loading AMF..." screen
    open. Token-usage counts are cosmetic; the dashboard now appears as
    soon as the other startup tasks finish and the counts fill in
    asynchronously.

### Migration

- No migration is required.

## [v0.19.4] - 2026-05-11

### Fixed

- Fixed the dominant cause of the "Loading AMF..." stall that persisted
  through v0.19.3. `App::new` was synchronously reading every Claude
  session `.jsonl` file (potentially megabytes per feature) and every
  `PLAN.md` to pre-populate the prompt and plan caches before the first
  frame could draw. Both caches now start empty and are filled by the
  background sidebar-load tasks that run immediately after the dashboard
  appears, so startup is fast regardless of session history size.

### Migration

- No migration is required.

## [v0.19.3] - 2026-05-11

### Fixed

- Fixed slow startup (stall on "Loading AMF..." or "Refreshing Claude
  hooks...") that became noticeable after the v0.19.0 global store
  migration. With a large feature count, `ensure_notify_scripts` and
  `ensure_amf_skills` were writing tens of files per feature on every
  launch. The hook and plugin refresh passes now record a version stamp
  after completing; subsequent startups on the same binary skip both
  passes entirely. Individual script and skill writes are also guarded
  by a content check so they are no-ops when already up to date.
- Eliminated a 50ms idle gap between each startup task; the event loop
  now spins without delay while startup tasks are pending, so the loading
  screen clears as fast as the tasks complete.

### Migration

- No migration is required.

## [v0.19.2] - 2026-05-11

### Fixed

- View mode now stays responsive while typing again, but still refreshes
  periodically when an agent harness is working, so live output keeps
  moving without waiting for the next keypress.
- Control-mode view input now uses the cheaper burst path again, which
  removes the extra redraw work that made repeated typing feel slower.
- Sidebar metadata and worktree sidebar updates now trigger redraws as
  soon as they arrive, so harness-side status changes appear without an
  extra keystroke.
- Fixed slow startup ("Loading AMF..." screen stall) introduced in v0.19.0
  by the global project store migration. `ensure_notify_scripts` and
  `ensure_amf_skills` now skip disk writes when the on-disk content is
  already up to date, so startup I/O scales to a few cheap reads per
  feature instead of tens of unconditional writes.

### Migration

- No migration is required.

## [v0.19.1] - 2026-05-09

### Fixed

- Release automation now waits for the same `cargo test --locked`
  preflight used by CI before it tags a version, so a failing test suite
  stops the release earlier instead of creating a broken release object.
- Fixed the view snapshot test harness so it no longer expects a return
  value from `send(...)` after the channel sender change.

## [v0.19.0] - 2026-05-09

### Added

- Debug log overlay now supports `p` to hide perf entries when you want
  to focus on non-performance messages.
- Feature create and delete actions now write richer audit entries to
  the debug log so a removed feature is easier to reconstruct later.
- View mode now shows a real scrollbar while you are in scroll/copy mode, so it is easier to tell that the pane is being scrolled instead of forwarded directly to tmux.
- Scroll/copy mode now supports fast movement with `Ctrl+j`, `Ctrl+k`, `Ctrl+Up`, and `Ctrl+Down`, matching the faster scrolling behavior used in other viewers.

### Changed

- The dashboard header now shows the AMF version next to the app title.
- AMF now uses one global project database at `~/.config/amf/amf.db`
  no matter which checkout you launch it from, so your project list stays
  consistent across directories.
- If you already had separate per-worktree project stores, AMF now merges
  them into the global database automatically the next time you start it.
  That means the first launch after upgrading may bring in projects from
  other checkouts instead of keeping them isolated.
- Scroll/copy mode now preserves the pane's ANSI coloring instead of flattening everything into plain text, so syntax highlighting and terminal colors remain visible while scrolling.
- The scroll-mode header now makes the active mode more explicit for users who are reading the status line.
- View mode now wakes immediately when new snapshots arrive, using a
  self-pipe wakeup and condvar-assisted worker polling to reduce input
  lag.
- GitHub releases now publish their notes from the matching changelog
  section, with a direct link back to the source entry in `CHANGELOG.md`.

### Migration

- No manual migration is required. AMF will fold legacy worktree-local
  project data into the global database on startup.

### Fixed

- Drag-to-copy selection now highlights correctly while you are in scroll/copy mode and still copies the selected text from the scrolled view.
- AMF now validates installed syntax highlighters at startup and repairs stale parser bundles automatically, so release builds should stop silently dropping syntax coloring.

## [v0.18.4] - 2026-05-08

### Fixed

- Eliminated the remaining source of input lag in view mode on all
  platforms, but most noticeably on macOS. The control-mode view worker
  was calling `reseed_control_view_parser` (two `tmux` subprocesses:
  `capture-pane` + `display-message`) on every keypress burst *and* on
  every control-protocol pane update, completely negating the benefit of
  having an event-driven control-mode view. The worker now:
  - On keypress burst: sends the current vt100 parser state immediately
    (zero subprocesses) so the display responds instantly; the actual
    pane update arrives shortly via the control protocol.
  - On control-protocol update (`parser_changed`): sends the
    incrementally-updated parser state directly (zero subprocesses)
    instead of re-capturing from tmux.
  - Periodic `NORMAL` reseeds and structural changes (pane swap, mode
    change, pause) still do a full reseed to correct any parser drift.

### Migration

- No migration is required.

## [v0.18.3] - 2026-05-08

### Fixed

- Fixed persistent "warning: could not set up terminal" on macOS even after
  the `xterm-256color` fix in v0.18.2. The root cause was that a user-set
  `TERMINFO` or `TERMINFO_DIRS` env var (e.g. a Homebrew ncurses path) was
  inherited by AMF's control-mode tmux clients, overriding the system
  terminfo lookup and preventing any terminal type — including `dumb` —
  from being found. AMF now strips `TERMINFO` and `TERMINFO_DIRS` from the
  environment of all spawned control-mode clients so they fall back to the
  compiled-in system terminfo paths where `xterm-256color` is reliably
  present.

### Migration

- No migration is required.

## [v0.18.2] - 2026-05-08

### Fixed

- Fixed noticeable input lag on macOS caused by the wrong terminal type
  (`screen-256color`) being used for AMF-managed tmux sessions. macOS's
  system terminfo does not include `screen-256color`, which caused the
  tmux control-mode clients to fail initialisation and fall back to
  spawning a `tmux send-keys` subprocess per keypress (~20–50 ms each).
  AMF now uses `xterm-256color` on macOS, which is present in the system
  terminfo, and explicitly overrides `TERM` when spawning control-mode
  clients so they are not affected by an inherited broken terminal type.
  This also eliminates the "warning: could not set up terminal" message
  that appeared when opening a terminal inside an AMF session on macOS.

### Migration

- No migration is required.

## [v0.18.1] - 2026-05-08

### Fixed

- Linux `amf upgrade` now skips unsupported packaged file types inside the
  bundled `tmux-root` tree, which prevents failures when copying release
  assets that contain special entries such as package docs.

### Migration

- No migration is required.

## [v0.18.0] - 2026-05-08

### Added

- Toast notifications now surface input requests and other transient
  prompts directly in the dashboard.
- AMF skills can now be injected into feature workspaces when a feature
  starts.
- Mouse-wheel scrolling now works in the Markdown viewer and help
  dialog.

### Changed

- Project storage, token caching, debug logging, and session status
  tracking now use SQLite-backed persistence.
- Codex notification hooks now use the updated `codex_config` flow, and
  Codex settings overrides are merged into the local workspace config.
- The tmux viewing stack now has a fallback path for environments where
  control-mode is unavailable.
- Startup now shows a loading screen while AMF initializes.
- Embedded overlays now keep the tmux cursor hidden behind dialogs and
  other UI surfaces.

### Fixed

- Embedded tmux view updates now reseed from tmux when control-mode
  output arrives, which prevents stale whitespace from lingering until
  the next manual input or view refresh.
- Toasts now render correctly in Viewing mode.
- macOS control-mode space rendering now works correctly.
- `amf upgrade` now handles symlinked release paths correctly.
- Harness setup can now be dismissed cleanly.

### Migration

- Existing stores migrate in place to the SQLite-backed schema; no
  manual migration is required.

## [v0.17.1] - 2026-04-21

### Fixed

- Managed tmux control-mode sessions now bootstrap with a temporary hidden
  session before applying the global `default-terminal` setting, avoiding the
  macOS startup failure where tmux could not connect to the managed socket.
- tmux startup on macOS now handles the dedicated managed socket without
  relying on `tmux start-server`, which could fail with `server exited
  unexpectedly`.

### Migration

- No store migration is required.

## [v0.17.1] - 2026-04-21

### Fixed

- macOS cross-compilation now skips the PTY termios setup that is not
  available on that target, resolving the build failure in
  `src/tmux.rs`.

### Migration

- No store migration is required.

## [v0.17.0] - 2026-04-20

### Added

- Embedded tmux sessions now use a full tmux control-mode view by
  default, streaming pane output directly into AMF for much more
  responsive typing and rendering in view mode.
- Added `tmux_control_mode` to `~/.config/amf/config.json`. It defaults
  to `true`; set it to `false` to return to the legacy ambient tmux
  socket and direct `tmux send-keys` fallback path.
- Help dialogs now support scrolling so longer keybinding and workflow
  reference text remains readable inside smaller terminals.

### Changed

- AMF now uses a dedicated managed tmux socket for control-mode sessions
  instead of inheriting a potentially polluted ambient tmux server.
- View-mode input no longer relies on per-key `tmux send-keys`
  subprocesses in the default path, reducing input latency and avoiding
  stale control-client buildup on long-running tmux servers.
- Diff-review prompts now include a short hold delay to avoid accidental
  keystrokes being interpreted immediately after the review popup opens.

### Fixed

- Control-mode view reseeding now restores the parser cursor to tmux's
  real pane cursor before applying incremental output, fixing misplaced
  cursor and stray text artifacts during shell/readline redraws.
- Session selection redraws now update correctly after switching
  sessions.
- Control-mode clients now perform readiness checks and fall back safely
  if startup fails.

### Migration

- No store migration is required.
- Existing tmux sessions on the previous ambient socket are not moved to
  the new managed control-mode socket. Restart those sessions from AMF,
  or temporarily set `"tmux_control_mode": false` in
  `~/.config/amf/config.json` if you need to keep using the legacy tmux
  server.

## [v0.16.0] - 2026-04-20

### Added

- Claude and Opencode sidebars now show task/todo progress with a
  compact progress bar, checkbox-style status markers, and a focused
  window around active work.
- Debug log navigation now supports `PageUp`/`PageDown`, `g`/`G` for
  top/bottom jumps, mouse wheel scrolling, and an explicit end-of-log
  marker.

### Changed

- Startup session-status sync now runs in the background instead of
  blocking the main event loop, improving first-open responsiveness for
  large session histories.
- VS Code availability detection now runs asynchronously during startup
  rather than blocking `App::new()`.
- The sidebar prompt section is more compact: the `leader l` hint moved
  into the border title, prompt text renders directly without a
  `Preview:` prefix, and prompt copy uses the primary text color.
- Persistent tmux control-mode input is now guarded behind
  `AMF_EXPERIMENTAL_PERSISTENT_TMUX_INPUT`, with direct `send-keys`
  remaining the default path.

### Fixed

- macOS key release events from crossterm are now ignored at top-level
  key dispatch, preventing actions from firing twice for a single
  keystroke.
- Recursive markdown, slash-command, usage, and session metadata scans
  no longer follow symlinked directories, avoiding UI stalls caused by
  symlink cycles or unexpectedly large linked trees.
- tmux control-mode input fallback now waits for client readiness,
  detects dead persistent clients, respawns them when needed, and falls
  back to direct `send-keys` on failure.

### Migration

- No store migration is required.

## [v0.15.0] - 2026-04-13

### Added

- Agent harness configuration and setup flow. AMF now lets you choose
  which harnesses are enabled, persists that selection in
  `projects.json`, and can prompt for setup on startup when no
  harnesses are configured.
- Pi support as a fourth harness/session type alongside Claude,
  Opencode, and Codex.

### Changed

- UI language now refers to user-selectable agents as "harnesses" in
  dialogs, help text, and picker flows.
- Feature creation can now skip the default terminal session and skip
  steering prompt setup when those extras are not needed.
- Feature creation, session pickers, and related config flows now only
  show harnesses that are currently enabled.
- Dashboard activity indicators are now animated, making background
  work and harness checks easier to spot.

### Fixed

- `amf upgrade` now streams release downloads to disk instead of
  buffering the full archive in memory first, improving reliability for
  larger bundles and lower-memory systems.
- Diff syntax highlighting now refreshes its cache correctly, reducing
  stale or incorrect highlighting in the diff viewer. Added multi-file
  syntax fixtures to make regressions easier to catch.

### Migration

- Existing stores migrate in place to keep using project store version
  5 while adding the new `available_harnesses` field.
- After upgrading, AMF may ask you to configure at least one harness
  before feature creation or session picker flows are available.

## [v0.14.1] - 2026-04-07

### Changed

- Dashboard status syncing now scales better with large project lists by
  using cached sidebar state for Opencode thinking detection and by
  reducing repeated visible-item and tmux-session scans.

### Fixed

- Embedded dashboard performance no longer degrades as sharply on
  machines with many projects, features, and open tmux panes due to
  repeated background `tmux capture-pane` fallbacks and redundant
  session-list work.

## [v0.14.0] - 2026-04-03

### Added

- Embedded view now supports `Ctrl+Space` then `R` to refresh tmux pane
  sizing on demand after terminal or layout changes.

### Changed

- Linked git worktrees now keep branch-local AMF state in
  `.amf/projects.json`, seeded from the primary checkout on first
  launch, so project and feature changes in one checkout no longer leak
  into another.
- Embedded tmux view refresh was reworked for better responsiveness,
  reducing idle overhead and making pane updates feel faster while you
  type, submit prompts, and interact with sessions.

### Migration

- No manual migration is required.
- The primary checkout still uses `~/.config/amf/projects.json`.
- The first AMF launch inside a linked worktree creates a local
  `.amf/projects.json`, initialized from the primary store when one
  exists.

## [v0.13.1] - 2026-03-31

### Fixed

- `amf upgrade` now replaces bundled release directories recursively,
  preventing partial installs that could leave the tmux wrapper present
  without its neighboring `tmux-real` binary or bundled support files.

## [v0.13.0] - 2026-03-26

### Added

- Opencode sidebar with work section, todos list, and sidecar state
  tracking — shows task activity, todo items, and LSP metadata
  alongside other session details.
- Per-session Codex prompt history and preview in sidebar — prompts are
  now session-specific rather than shared across features.
- Codex sidebar session metadata display including thread information,
  usage source confidence, and reasoning token counts.
- Local command actions in command picker — focused access to AMF-level
  actions without mixing in session-specific commands.
- Claude session resume picker on `S` now works for Claude sessions as
  well as Opencode, with session titles pulled from the first user
  prompt in each saved conversation.
- The steering prompt coach now supports scrolling for longer prompts
  without leaving the feature-creation flow.

### Changed

- Sidebar layout refinements across Codex and Opencode sessions for
  improved visual hierarchy and compactness.
- Codex sidebar summary and prompt sections reorganized to prioritize
  active work and plan context.
- Session pickers now show cleaner titles and relative ages for saved
  Claude, Codex, and Opencode sessions.
- Sidebar background refresh work now pauses while the sidebar is
  hidden, reducing unnecessary polling and improving view responsiveness.
- Sidebar, token usage, and usage refresh paths were reworked for lower
  overhead background updates.

### Fixed

- Stale worktree delete failures are now handled gracefully without
  blocking feature deletion.
- tmux session launches no longer leak AMF-managed `PATH` overrides into
  child sessions.
- `amf upgrade` now resolves the actual release asset from GitHub's
  release metadata instead of hardcoding a guessed download URL, so
  future packaging changes do not regress into `404` download failures.
- macOS `x86_64` upgrade detection now only selects the Apple Silicon
  bundle when AMF is running under Rosetta on Apple Silicon. Native
  Intel Macs now get a clear unsupported-platform error instead of a
  misleading architecture mapping.

### Migration

- No store migration is required.

## [v0.12.0] - 2026-03-24

### Added

- Claude session sidebar — a new panel in view mode showing live session
  metadata: current tool activity, pending input detail, active prompt
  context, task todos (expanded inline), and plan progress. Toggle
  visibility with a keybind. Task data is sourced from the Claude task
  store when available, with transcript fallback.
- Latest prompt dialog now shows a scrollable list of all Claude session
  prompts with timestamps. Navigate with `j`/`k`, copy the selected
  prompt to clipboard with `y` (uses `wl-copy` with `xclip` fallback).
  Each entry shows a colored timestamp and the first line of the prompt,
  truncated with an ellipsis when needed.

### Changed

- Markdown viewer and picker UX improvements.

### Migration

- No store migration is required.

## [v0.11.1] - 2026-03-19

### Changed

- Improved TSX syntax highlighting in the diff viewer.

### Fixed

- Restored sessions now resize correctly to the current pane dimensions
  on attach, and the session picker no longer wraps unexpectedly on
  narrow terminals.
- `amf upgrade` now downloads the full `.tar.gz` bundle and extracts
  all bundled files (`amf`, `tmux`, `tmux-real`, libs) into the install
  directory, so the bundled tmux binary is also updated on upgrade.
- Install and upgrade scripts remove the existing `/opt/amf` directory
  before moving the new bundle into place, preventing the old binary
  from being left behind when `/opt/amf` already exists.

## [v0.11.0] - 2026-03-17

### Added

- Per-session token usage tracking — Claude, Codex, and Opencode agent
  sessions now show a live cost line in the dashboard:
  `2.3M in · 4.8k out · 304.8k eff · $0.91`. Pricing defaults to
  Claude Sonnet 4.x rates and is configurable via `token_pricing` in
  `config.json`. Set `show_cost: false` to hide the dollar column.
- Steering prompt editor — edit the feature's steering prompt directly
  from the dashboard without leaving the TUI. Accessible via the
  feature creation flow and a new view-mode shortcut.
- Gruvbox Dark and Gruvbox Light UI themes, plus a matching
  `amf-gruvbox.json` Opencode theme with full syntax, markdown, and
  diff highlighting.
- Live theme preview in the theme picker — scrolling through themes
  applies them immediately; `Esc` reverts to the original and `Enter`
  confirms. Press `t` inside the picker to toggle transparent
  background on the fly.
- Latest prompt injection — press `Tab` or `Enter` in the latest
  prompt dialog (leader `l`) to send the saved prompt directly into
  the active session without leaving view mode.

### Changed

- Memo sessions removed — the `m` keybind, `has_notes` field, and
  all related UI and automation API surface have been dropped. Existing
  features with notes are unaffected at the data level, but the session
  type will no longer appear in pickers.
- Session picker no longer spawns a `code --version` subprocess on
  every open; VSCode availability is cached at startup. Config is also
  read from the already-loaded extension instead of hitting disk again.

### Fixed

- Thinking/tool hook scripts (`thinking-start.sh`, `tool-start.sh`,
  etc.) now use `$AMF_SESSION` (the tmux session name) as the IPC
  key instead of the Claude hook UUID. This fixes the thinking
  throbber never appearing in IPC mode.
- Bundled `ld-linux` is used when invoking the bundled tmux on
  systems where the host glibc version is too old, preventing
  "version not found" errors on older Linux distros.
- UI hangups caused by blocking file I/O in the usage refresh path
  are eliminated.

### Migration

- No store migration is required.
- If you relied on Memo sessions, those session entries will no longer
  start or appear in pickers. Remove them from saved features if
  desired.
- If you have custom `token_pricing` needs, add a `token_pricing`
  block to `~/.config/amf/config.json` (see configuration docs).

## [v0.10.1] - 2026-03-13

### Fixed

- `custom-diff-review.sh` now resolves `NOTIFY_DIR` from the git
  repository root rather than the current working directory. This
  fixes missed notifications when Claude operates in a subdirectory
  of the repo.

## [v0.10.0] - 2026-03-12

### Added

- AMF release bundles now include a statically-linked `tmux` binary.
  When launched outside an existing tmux session, AMF uses the bundled
  binary on a private socket so that tmux does not need to be installed
  separately.
- `AMF_TMUX_BIN` and `AMF_TMUX_SOCKET` environment variables let you
  override the tmux binary and socket path at runtime.

### Changed

- Default branch name changed from `master` to `main` throughout —
  diff review scripts, PR helpers, and worktree operations now default
  to `main` as the base branch.

### Migration

- No store migration is required.
- If you have existing scripts that relied on `master` as the default
  base branch, update them to use `main` (or set the branch explicitly).

## [v0.9.0] - 2026-03-12

### Added

- On-demand tree-sitter parser management — a language picker lets you
  install and select syntax highlighting grammars at runtime without
  restarting, accessible from the diff viewer and diff review prompt.
- Scroll support in the diff review prompt pane (j/k, g/G, mouse wheel).
- Opencode change-tracker plugin (`.opencode/plugins/change-tracker.js`)
  that watches file writes, emits AMF notifications, and wires into the
  diff review approval flow for Opencode sessions.

### Fixed

- Diff review flow for Opencode sessions now correctly triggers the
  change-reason prompt and handles accept/reject signalling.
- Diff review patch scroll state is now shared consistently between the
  diff viewer and diff review prompt.

### Migration

- No store migration is required.
- To use Opencode diff review, the
  `.opencode/plugins/change-tracker.js` plugin must be present in your
  repo (included automatically for new features).

## [v0.8.0] - 2026-03-11

### Added

- Built-in in-app diff viewer for view mode with a file list, patch pane,
  unified and side-by-side layouts, refresh support, and keyboard
  navigation.
- Tree-sitter syntax highlighting for the diff viewer, plus contextual
  line highlighting and clearer diff gutters.
- In-app markdown file picker and viewer for `.claude/*.md` files and
  top-level `*.md` files while viewing a feature.
- Repo-root markdown discovery for worktree features, so shared files
  like `PLAN.md` can be opened without leaving the current session.
- Vibeless-mode Codex diff review automation that watches file writes,
  opens the change-reason prompt, and reverts rejected changes.

### Changed

- Diff review and markdown-reading workflows now stay inside AMF instead
  of requiring an external tool or a separate terminal flow.
- Markdown picker labels now distinguish worktree files from repo-root
  files when both scopes are available.

### Migration

- No store migration is required.
- If you use Codex vibeless-mode diff review, install `inotifywait`
  (usually provided by `inotify-tools`) so the watcher can run.

## [v0.7.0] - 2026-03-09

### Added

- Full automation system for creating projects, features, and batch features via CLI and IPC
  - `amf automation create-project` for programmatic project creation
  - `amf automation create-feature` for programmatic feature creation
  - `amf automation create-batch-features` for parallel multi-feature creation
  - JSON-based request/response interface with timeout and dry-run support
- Steering Coach startup prompt overlay for coaching new features
- Plan mode for collaborative planning sessions with shared PLAN.md
- Show pending worktree scripts in project list with visual indicators
- Project preferred agents configuration per workspace
- Worktree session configuration dialog for customizing sessions
- Mouse wheel support for pane scrolling in view mode
- Codex session restore functionality
- Release session now displays current version before prompting for new version

### Changed

- Release session moved from global to local repo configuration
- Better error handling and status messaging throughout the application
- Improved review mode selection and behavior
- Enhanced Codex thinking detection for repo-root features
- Fixed Codex latest prompt lookup
- Worktree script visibility improved with blocking operations

### Fixed

- Review mode selection now correctly handles feature states
- Codex thinking detection properly works for features using repo root directly
- Session restore functionality works across different agent types

### Migration

- No manual migration required, but review the new automation interface if integrating AMF into CI/CD workflows

## [v0.6.1] - 2026-03-06

### Fixed

- Fixed extension reload path handling for workspace-local
  `.amf/config.json`.
- Repaired related test fixtures around extension loading.

### Migration

- No manual migration required.

## [v0.6.0] - 2026-03-06

### Added

- Full AMF UI theming with built-in `default`, `amf`, `dracula`,
  `nord`, and Catppuccin variants.
- A dashboard theme picker and `theme` / `transparent_background`
  config support.
- `allowed_agents` config so each workspace can restrict AMF to a
  subset of Claude, Codex, and Opencode.
- Session bookmarks with `H`, `M`, and `1`-`9` quick
  jumps.
- Ready-state tracking for features.
- Configurable leader timeout via `leader_timeout_seconds`.
- Codex usage bars in the status area plus extra usage debug logging.

### Changed

- Leader mode now opens clearer popup menus in view mode.
- Codex notifications prefer IPC delivery and merge with local
  worktree configuration.
- Debug log rendering wraps long lines instead of clipping them.

### Migration

- Optional: add `theme`, `transparent_background`,
  `leader_timeout_seconds`, or `extension.allowed_agents` to
  `~/.config/amf/config.json` or a repo-local `.amf/config.json`.
- No store migration is needed. `projects.json` is auto-migrated on
  load.

## [v0.5.0] - 2026-03-06

### Added

- Initial Codex agent and session support.
- IPC-based input notifications with file-based fallback when the AMF
  socket is unavailable.
- Improved input request handling for Codex worktrees.

### Changed

- Session picker UX was tightened up around mixed agent/session types.

### Migration

- Install the `codex` CLI before creating Codex-backed features.
- No manual migration is required for existing Claude or Opencode
  features.

## [v0.4.1] - 2026-03-05

### Added

- `amf upgrade` to replace the installed binary with the latest GitHub
  release.
- `amf -V` / `amf --version` for quick version checks.

### Migration

- No manual migration required.

## [v0.4.0] - 2026-03-04

### Added

- Batch feature creation for spinning up numbered worktrees in one
  flow.
- Feature nicknames shown in the dashboard.
- Per-feature/session workdir handling for local extension config.

### Changed

- Forking preserves uncommitted changes when creating the new feature.
- Claude thinking detection became mtime-based for lower overhead and
  better responsiveness.

### Migration

- Repo-local `.amf/config.json` files are now respected alongside the
  global config and merged on top of it.
- Existing saved state is auto-migrated to include summary and
  nickname fields.

## [v0.3.0] - 2026-03-04

### Added

- Custom session `autolaunch`, `on_stop`, status text, and `pre_check`
  support.
- Session forking with transcript context export.
- Theme picker dialog in the dashboard.
- Auto-generated session summaries.
- Mouse text selection and clipboard copy in the embedded pane.
- Debug log overlay with file logging.
- Claude session resume picker on `S`.

### Changed

- Removed the old "switch directly to tmux" workflow in favor of the
  embedded view.
- Final review no longer kicks you out of viewing mode when there are
  no changes.

### Migration

- If you use custom sessions, you can now optionally add `autolaunch`
  and `pre_check` fields to their config entries.
- No manual store migration is required.

## [v0.2.0] - 2026-03-02

### Added

- Built-in AMF themes, transparent background support, and bundled
  Opencode themes for the embedded pane.
- VSCode session support launched through the `code` CLI.
- Saved latest Claude prompt overlay.
- Custom session status relays and `on_stop` cleanup hooks.

### Migration

- Optional: set `theme`, `transparent_background`, or
  `opencode_theme` in `~/.config/amf/config.json`.
- If you use VSCode sessions, make sure the `code` CLI is installed.

## [v0.1.1] - 2026-03-01

### Added

- Initial public release of the multi-project, multi-feature dashboard.
- Embedded tmux view for Claude and Opencode sessions.
- Git worktree orchestration, vibe modes, notifications, search, and
  session management.

### Migration

- First tagged release. No migration required.
