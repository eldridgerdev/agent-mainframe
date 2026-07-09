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
