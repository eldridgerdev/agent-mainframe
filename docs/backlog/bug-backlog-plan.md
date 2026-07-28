# Bug backlog

- **Status:** Backlog
- **Owner:** unassigned
- **Relates to:** dashboard key handling (`src/handlers/normal.rs`)

A running list of known bugs that are not yet scheduled for a fix. Unlike
the feature plans in this directory, this is a single shared doc — add a
new `## ` section per bug. The filename ends in `-plan` only so AMF's
in-app Markdown viewer (which filters to paths containing "plan") picks
it up.

For each bug record: how to reproduce, expected vs. actual behaviour, the
relevant code, and any leads on the cause. Move a bug out of this doc (or
strike it through with the fixing commit/PR) once resolved.

## ~~Stopped agent sessions open an empty shell after tmux disappears~~ (Fixed)

- **Status:** Fixed (2026-07-28)
- **Reported:** 2026-07-28
- **Relates to:** dashboard session opening and resume handling
- **Root cause:** `Enter` recreated a stopped feature through the ordinary
  bulk-start path, which could leave the selected saved agent window at a
  shell prompt when the restart limit skipped it. `S` used a separate
  harness-specific path, so the two entry points did not share the same
  missing-tmux decision or selected-session validation.
- **Fix:** `Enter` and `S` now share a stopped-agent recovery dialog with
  resume, clear-start, and cancel actions. Recovery recreates the saved tmux
  layout, always launches the selected agent pane, passes the persisted
  harness ID only for resume, and reports setup or launch failures in AMF.

### Repro

1. Start a feature with a Claude, Codex, Opencode, or Pi session.
2. Stop its tmux session externally, or restart the computer.
3. Relaunch AMF, select the saved agent session, and press `Enter` or `S`.

### Expected

AMF offers to resume the previous harness session or start a clear one, then
opens the selected agent pane.

### Actual

The selected pane could open as an empty shell, or the independent resume path
could fail against missing tmux state.

## PR Triage fix agents sometimes target the wrong lines

- **Status:** Backlog
- **Reported:** 2026-07-20
- **Relates to:** PR Triage fix-prompt assembly
  (`src/app/pr_review.rs::PrComment::fix_prompt`, `fix_prompt_body`,
  `combined_fix_prompt`)
- **Lead:** The prompt currently gives the agent a `file:line` pointer, the
  review comment, and its GitHub diff hunk, but the agent can still interpret
  that context against the wrong location in the current file. The agent has
  described the failure as having "mis-anchored" the comment. Capture a real
  failing prompt and compare its stored `line`, `side`, `outdated`, and
  `diff_hunk` fields with the current worktree before deciding whether the fix
  belongs in comment normalization, hunk construction, or stronger prompt
  guidance.

### Suggested changes and checks

1. Never truncate, window, or otherwise re-slice GitHub's `diff_hunk` before
   handing it to the agent. In particular, audit `PrComment::prompt_hunk`,
   which currently reduces longer line-anchored hunks to three context lines
   on either side of the computed anchor.
2. Prefer GitHub's current `line` whenever it is present; use `original_line`
   only as the fallback for an outdated comment. Preserve and test the current
   `c.line.or(c.original_line)` precedence during any refactor.
3. Add a cheap consistency check before dispatch: parse the
   `@@ -old_start,old_count +new_start,new_count @@` header and verify that the
   chosen line falls within the old or new range selected by `side`. If the
   anchor cannot be mapped, refresh or surface the mismatch instead of quietly
   constructing a possibly incorrect excerpt.
4. Audit freshness at the same boundary. Any line mapping or last-`N` hunk
   slicing must not combine a current line number with a cached or stale hunk.
   Verify that the line, side, hunk header, hunk body, and PR head SHA all come
   from the same fresh fetch; specifically check whether an `@@ ... @@` excerpt
   is being cut from the tail of an older cached hunk rather than the freshly
   fetched comment hunk.

### Repro

1. Open PR Triage and select an inline review comment.
2. Send the comment to an agent with `f` (or include it in a batch with `B`).
3. Let the agent make the requested fix and inspect the resulting edit.

### Expected

The agent applies the fix to the exact line or code block the review comment
is anchored to.

### Actual

Intermittently, the agent applies or explains the fix against different lines
than the comment refers to and reports that it mis-anchored the comment.

## AI Review pane doesn't refresh PR Triage after posting

- **Status:** Backlog
- **Reported:** 2026-07-20
- **Relates to:** AI Review posting (`src/app/ai_review.rs::ai_review_post`), PR
  Triage refresh (`src/app/pr_review.rs::refresh_pr_review`,
  `start_pr_review_fetch`, `pr_review_cache`)
- **Root cause (by design, currently):** `ai_review_post` marks the kept
  findings `published` and re-caches the AI Review pane's own state, but it
  never touches PR Triage's cached comments (`pr_review_cache`) or triggers
  a re-fetch. `refresh_pr_review` only runs while `self.mode` is already
  `AppMode::PrReview`, so nothing re-fetches on the AI Review pane's behalf.
  The success toast even says so explicitly: "follow up in PR Triage after
  a refresh." Returning to PR Triage (`Esc`) shows the same stale cached
  comment list until the user manually triggers a refresh.
- **Ask:** After a successful `W` post from the AI Review pane, kick off the
  same background comment re-fetch `refresh_pr_review`/
  `start_pr_review_fetch` do, keyed by the workdir + PR number, so that by
  the time the user returns to (or next opens) PR Triage for that PR, the
  newly posted review comments are already there — no manual refresh
  keypress required.

### Repro

1. Open a feature's PR, press `A` to generate an AI review.
2. Keep one or more findings, press `W` to post them as a GitHub review.
3. Return to PR Triage (`Esc`) for the same PR without pressing refresh.

### Expected

The newly posted AI review comments show up in PR Triage's comment list
right away.

### Actual

PR Triage still shows its last-cached comment list; the new comments only
appear after an explicit manual refresh.

## ~~Agent launch commands are cut off on macOS~~ (Fixed)

- **Status:** Fixed (2026-07-09)
- **Reported:** 2026-07-09
- **Relates to:** agent harness startup (`src/tmux.rs`)
- **Root cause:** AMF typed the full environment-prefixed launch command into
  the tmux pane. On macOS that long input could be truncated before
  `AMF_FEATURE_SESSION_ID` and the agent binary were reached, leaving the pane
  at a shell prompt with only part of the launch command visible.
- **Fix:** Write launch commands to a short-lived shell script and send only
  `sh <script>` through tmux. The same path is used for Claude, Codex,
  opencode, Pi, custom session commands, and the Codex diff-review watcher.

### Repro

1. On macOS, create or start an agent session in AMF.
2. Watch the pane while the harness starts.

### Expected

The agent harness starts after AMF creates the tmux window.

### Actual

The shell input stops partway through the environment setup, around
`AMF_TMUX_WINDOW='claude' AMF_FEAT...`, and Claude never starts.

## ~~Claude hook commands fail under macOS Application Support~~ (Fixed)

- **Status:** Fixed (2026-07-13)
- **Reported:** 2026-07-13
- **Relates to:** Claude hook setup (`src/app/setup.rs`)
- **Root cause:** AMF wrote Claude hook commands as bare script paths. On
  fresh macOS installs, the AMF config directory can live under
  `~/Library/Application Support/amf`, and Claude executes hook commands
  through `/bin/sh`. The unquoted space after `Application` made the shell
  split the path before the helper script could run. Older unquoted hook
  entries also stayed in existing `.claude/settings.local.json` files.
- **Fix:** Quote AMF-managed Claude hook command paths, force one more hook
  refresh, and treat older unquoted `Library/Application Support/amf` helper
  entries as managed so AMF removes and replaces them automatically. AMF also
  checks stored Claude feature workdirs during startup and immediately repairs
  stale unquoted local hook settings, even if the global hook refresh stamp is
  already current.

### Repro

1. On a fresh macOS install that stores AMF config under
   `~/Library/Application Support/amf`, create or start a Claude feature.
2. Submit a prompt or stop the session so Claude runs AMF's local hooks.

### Expected

Claude hooks run without shell path errors.

### Actual

Claude reports a non-blocking hook error like
`/bin/sh: /Users/.../Library/Application: no such file or directory`.

## ~~Stale Claude hooks accumulate under a custom XDG_CONFIG_HOME~~ (Fixed)

- **Status:** Fixed (2026-07-20)
- **Reported:** 2026-07-20
- **Relates to:** Claude hook setup (`src/app/setup.rs`)
- **Root cause:** The managed-hook heuristic only recognized AMF helper
  scripts under a path ending in `/.config/amf` (the dotted XDG default) or
  `/Library/Application Support/amf` (macOS). `XDG_CONFIG_HOME` can point
  anywhere per the XDG spec — it isn't required to end in `config` at all
  (e.g. `XDG_CONFIG_HOME=/tmp/foo` resolves the AMF config dir to
  `/tmp/foo/amf`), and tools like the screenshot dev tool use an isolated
  `<tmp>/config` root, producing hook paths like `.../config/amf/<script>.sh`.
  Neither shape matched the hardcoded patterns, so `ensure_notification_hooks`
  never recognized these as AMF-managed, never removed them on a later run
  with a different config root, and every such run left a new stale hook
  block behind in whatever `.claude/settings.local.json` it touched.
- **Fix:** Generalized `is_amf_claude_hook_command` to match any parent
  directory literally named `amf` (the invariant `amf_config_dir()` always
  produces, regardless of what config root it's nested under), instead of
  hardcoding specific parent path shapes. Stale entries from any custom
  `XDG_CONFIG_HOME` — dotted, non-dotted, or unrelated to the word `config`
  entirely — are now recognized and replaced on the next hook refresh.

### Repro

1. Run AMF once with `XDG_CONFIG_HOME` pointed at `/tmp/some-dir/config`
   (as the screenshot dev tool does) against a feature's workdir.
2. Run AMF again normally (real `~/.config/amf`) against the same workdir.
3. Inspect `.claude/settings.local.json` in that workdir.

### Expected

Only one set of AMF-managed hook commands remains, pointing at the current
config directory.

### Actual

Both the stale temp-path hook commands and the current ones are present.
Claude reports non-blocking `UserPromptSubmit`/`PreToolUse`/etc. hook
errors like `/bin/sh: 1: /tmp/.../config/amf/thinking-start.sh: not found`
for every leftover temp-path entry.

## ~~macOS build fails in tmux PTY setup~~ (Fixed)

- **Status:** Fixed (2026-07-09)
- **Reported:** 2026-07-09
- **Relates to:** tmux PTY control clients (`src/tmux.rs`)
- **Root cause:** macOS exposes libc signatures where `openpty` expects
  mutable `termios` / `winsize` pointers, and `ioctl` expects the
  controlling-terminal request as `c_ulong`. AMF passed immutable pointers to
  `openpty` and passed `TIOCSCTTY` without the platform-width cast.
- **Fix:** Pass mutable `termios` and `winsize` values into `openpty`, and
  cast each `TIOCSCTTY` request to `libc::c_ulong`.

### Repro

1. Build AMF on macOS with `cargo build`.

### Expected

The project builds successfully.

### Actual

Compilation fails in `src/tmux.rs` with libc type errors around `openpty` and
`ioctl`.

## ~~New-feature wizard's existing-worktree picker doesn't scroll~~ (Fixed)

- **Status:** Fixed (2026-07-09)
- **Reported:** 2026-07-09
- **Relates to:** feature creation wizard (`src/ui/dialogs/feature.rs`)
- **Root cause:** `draw_create_feature_worktree_picker` rendered the
  worktree list with a plain `frame.render_widget(list, chunks[1])`,
  never handing ratatui a `ListState` with the selection set. Without a
  stateful render, `List` has no way to know which item is selected and
  never scrolls the viewport to keep it visible, so once a project had
  more worktrees than fit in the dialog, navigating past the visible rows
  just scrolled the selection off-screen with nothing to bring it back.
- **Fix:** Switched to `ListState` + `render_stateful_widget`, matching
  the pattern already used by `draw_markdown_file_picker` in
  `src/ui/picker.rs` — compute the selected item's position within the
  filtered/visible list and pass it to `list_state.select(...)`, letting
  ratatui auto-scroll.

### Repro

1. Have a project with enough worktrees that the list overflows the
   "Select Worktree" dialog.
2. Start a new feature "from existing worktree" and navigate down past
   the last visible row.

### Expected

The list scrolls so the selected worktree stays visible.

### Actual

The selection moved off-screen with no way to see it or the remaining
options.

## ~~Terminal sessions insert extra blank lines on macOS~~ (Fixed)

- **Status:** Fixed (2026-07-02)
- **Reported:** 2026-07-02
- **Relates to:** embedded terminal input (`src/handlers/view.rs`)
- **Root cause:** AMF treated every Enter repeat event as a real submit and
  could also forward raw carriage-return/newline character events as literal
  terminal input. On macOS terminals this could show up as several blank lines
  from a single input action.
- **Fix:** Ignore repeated Enter events in view-mode tmux key translation and
  drop raw `\r` / `\n` character events instead of forwarding them as literal
  input. Plain Enter still forwards exactly one tmux `Enter` key.

### Repro

1. On macOS, open a plain terminal session inside AMF.
2. Press Enter or submit input in the embedded terminal.

### Expected

The terminal receives one Enter and advances once.

### Actual

The embedded terminal sometimes advances by several lines, making the pane look
awkward and jumpy.

## ~~New agent sessions show their launch command before the harness opens~~ (Fixed)

- **Status:** Fixed (2026-07-02)
- **Reported:** 2026-07-02
- **Root cause:** AMF created the tmux pane and immediately rendered captured
  pane content. For newly launched agent sessions, the first captured frame can
  still be the shell echo of AMF's long harness launch command and environment.
  The first loading-mask attempt only covered brand-new feature tmux sessions,
  but adding `Codex 2` / another agent from the session picker creates a new
  tmux window inside an already-running feature tmux session. A content-only
  fallback then over-corrected because the launch echo can remain in captured
  scrollback after the harness is already running.
- **Fix:** Track a transient startup mask on the specific `ViewState` created
  for a new agent pane. Brand-new feature sessions and new agent windows from
  the session picker set that mask explicitly. The mask clears when fresh pane
  content no longer looks like AMF's launch echo, with a timeout fallback so a
  failed launch becomes visible. Follow-up coverage applies the same mask to
  resumed Claude/Codex/opencode sessions launched from `S`, and includes Pi in
  the feature-row default agent lookup so Pi gets the same loading screen when
  opened from a feature row.

### Repro

1. From an existing feature, add a new agent session such as `Codex 2`.
2. AMF switches into the new session view while the harness is launching.

### Expected

AMF shows a short loading screen until the agent harness is visible.

### Actual

The embedded pane briefly shows the full tmux launch command and exported AMF
environment before the harness takes over.

## ~~PR review: triage / reply state is lost on return~~ (Fixed)

- **Status:** Fixed (2026-06-30, PR #373, `c41f465`)
- **Reported:** 2026-06-29
- **Relates to:** PR comment review
  ([pr-comment-review-plan.md](pr-comment-review-plan.md), Epic D bug item)
- **Root cause:** Every mutation path did persist immediately, and
  `apply_persisted_triage` did run on both load paths — but
  `pr_comment_triage` was keyed by `PR# + comment id + head_sha`. The fix
  session's push moved the PR head, so returning via `G` re-resolved to a
  new SHA and the overlay looked up rows under a SHA that no longer
  matched, silently dropping every mark.
- **Fix:** Migration 010 re-keys the table on `PR# + comment id`
  (collapsing per-SHA duplicates to the newest row); `load()` drops the
  SHA filter. `head_sha` is kept only as an informational record. DB-layer
  SHA-change survival and migration tests cover it.

Epic B claims triage is authoritative in the `pr_comment_triage` table and
that `apply_persisted_triage` overlays it onto every reload — but in real
use, none of it survives a round-trip through a fix session.

### Repro

1. Open a PR in the review pane (`G`).
2. Mark a comment done (`m`), and/or post a reply.
3. Start a fix on another comment (`f`), switching into the dedicated
   `"PR Review"` session.
4. Return to the review pane.

### Expected

The done mark / reply / skip state is still there — persisted in SQLite
and re-overlaid on the reloaded review.

### Actual

The review comes back with none of it saved; the earlier triage and reply
state are gone.

### Leads / where to look

- State is likely mutated in the in-memory `PrReviewState` but **not
  flushed to SQLite before the pane is left** to switch into the fix
  session. The `f`-marks-`Fixing` path is said to persist before leaving;
  the `m` / `s` / reply paths may only update memory. → `src/app/pr_review.rs`,
  `src/handlers/pr_review.rs`.
- The return path may **re-fetch without re-overlaying** persisted triage
  — confirm `apply_persisted_triage` runs on the cache-hit, background-fetch,
  **and** return-from-session paths.
- Verify both sides read/write the triage row under the **same**
  `PR# + comment_id + head_sha` key (a head-SHA mismatch would silently
  orphan the saved rows). → `src/db/pr_comment_triage.rs`.
- Fix direction: persist every triage/reply mutation **immediately**
  (not on pane exit).

## ~~`A` does not open harness setup from the dashboard~~ (Fixed)

- **Status:** Fixed (2026-06-21)
- **Reported:** 2026-06-21
- **Root cause:** The `KeyCode::Char('A') => app.open_harness_setup(false)`
  arm was inside `handle_normal_leader_key` (the Ctrl+Space chord handler),
  not `handle_normal_key`. A bare `A` on the dashboard fell through to
  `_ => {}`; only `Ctrl+Space` then `A` opened the picker. The remap-block
  lead was a red herring — with default (empty) keybindings the remap is a
  no-op.
- **Fix:** Moved the arm into `handle_normal_key` alongside the other bare
  capital actions, added `A` to the dashboard help overlay, and added a
  regression test (`bare_uppercase_a_opens_harness_setup_from_dashboard`).

### Repro

1. Launch AMF and land on the dashboard.
2. Press `A` (capital A), expecting the harness setup picker (the
   first-run multi-select where you enable Claude / Opencode / Codex /
   Pi) to open.

### Expected

The harness setup picker opens so the available harnesses can be changed
after first run.

### Actual

Nothing happens — the picker does not open in dashboard view.

### Leads / where to look

- The key is dispatched at `src/handlers/normal.rs:361`:
  `KeyCode::Char('A') => app.open_harness_setup(false)`.
- `open_harness_setup()` (`src/app/mod.rs:2591`) is unconditional, so the
  failure is likely upstream of the dispatch.
- This arm only runs in `AppMode::Normal` via `handle_normal_key`. If the
  reporter was in an embedded pane / Viewing mode, `A` is forwarded to
  tmux instead of opening the picker — confirm which mode "dashboard
  view" means.
- Check the keybinding remap block at `src/handlers/normal.rs:38-52`: a
  remap whose target resolves through `default_key_for_action` could
  rewrite `A` to a different canonical char before the match.
- `A` is not advertised in the help overlay, so discoverability is also
  a gap regardless of the dispatch bug.

### Notes

Other uppercase arms in the same handler (`N`, `B`, `O`, `S`) are
reported to work, which points at something specific to the `A` path or
to the mode the user was actually in rather than a blanket
uppercase-key problem.

## ~~Claude sidebar does not show the current todo list~~ (Fixed)

- **Status:** Fixed (2026-06-24)
- **Reported:** 2026-06-24
- **Root cause:** AMF only looked for Claude tasks under
  `~/.claude/tasks/<session_id>` or reconstructed them from transcript
  `TaskCreate` / `TaskUpdate` events. Claude can persist the visible
  checklist under a separate task-store ID, leaving the sidebar with no
  `Todos` section even while Claude shows tasks in the pane.
- **Fix:** Keep the exact session task directory as the first choice, then
  fall back to the newest task-store directory that contains readable task
  JSON. Empty newer task directories are skipped. Added regression tests for
  both the fallback and exact-session precedence.
- **Follow-up fix (2026-07-11):** Removed the newest global task-store
  fallback after it proved too broad: a newer checklist from another Claude
  session could appear in the current session's sidebar. AMF now only reads
  the exact task-store directory for the selected session, then falls back to
  that session's transcript.

### Repro

1. Open a Claude feature that has an active checklist in the Claude pane.
2. Keep the AMF sidebar visible.
3. Observe the sidebar sections.

### Expected

The sidebar includes a `Todos` section with the current checklist progress.

### Actual

The sidebar shows `Status`, `Work`, and `Prompt`, but no `Todos` section.

## ~~Claude composer opens over a blank pane~~ (Fixed)

- **Status:** Fixed (2026-06-25)
- **Reported:** 2026-06-25
- **Root cause:** Opening an agent session with composer input enabled moved AMF
  directly into `AppMode::Compose`. The main loop only initialized pane sizing
  for `AppMode::Viewing`, so the snapshot worker had zero pane dimensions and
  did not capture the Claude pane behind the composer.
- **Fix:** Treat `AppMode::Compose` as a live view for pane sizing and Claude
  pane drift checks, so the tmux pane is resized and captured before the
  composer overlay is drawn.

### Repro

1. Open a Claude Code agent session with composer input enabled.
2. Let AMF auto-open the composer on entry.
3. Look at the pane behind the composer.

### Expected

Claude Code's current pane content is visible behind the composer immediately.

### Actual

The composer opens, but the pane behind it is blank until the composer is
closed or input is sent.

## ~~Composer paste (Ctrl+V) does nothing on macOS~~ (Fixed)

- **Status:** Fixed (2026-07-16)
- **Reported:** 2026-07-16
- **Relates to:** clipboard helpers (`src/app/util.rs`)
- **Root cause:** `read_clipboard`, `copy_image_to_clipboard`, and
  `copy_to_clipboard` only handled WSL (`clip.exe`/`powershell.exe`), Wayland
  (`wl-paste`/`wl-copy`), and X11 (`xclip`). There was no macOS branch at all,
  so every clipboard call on macOS fell through to
  `Err("No clipboard utility found")`. Typing directly into Claude Code
  (bypassing AMF's composer) still worked because that path never goes
  through these helpers.
- **Fix:** Add macOS clipboard helpers using `pbcopy`/`pbpaste` for text and
  `osascript` (via a temp PNG file) for images, wired in ahead of the
  Wayland/X11 fallbacks the same way the WSL helpers are.

### Repro

1. On macOS, open an agent session in AMF with the composer enabled.
2. Copy an image or text, then press Ctrl+V in the composer.

### Expected

The clipboard content is pasted into the composer (text) or attached as an
image placeholder.

### Actual

Nothing happens; pasting only works if the composer is bypassed and the
paste goes directly to Claude Code's own input handling.
