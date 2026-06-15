# Changelog

All notable changes to AMF are documented in this file.

This changelog follows a Keep a Changelog style layout. Use
`## [Unreleased]` for pending work, then add a dated release section
when cutting a version. Major and minor releases are expected to
document user-facing changes and any migration notes here before they
are tagged.

## [Unreleased]

### Added

- Vim mode in the compose and steering-prompt inputs now supports undo
  and redo: press `u` in normal mode to undo and `Ctrl+R` to redo.
  Everything typed during a single insert session is undone in one step.

### Changed

- Review mode, final review, plan mode, and the steering coach now show
  an experimental label in the UI, so users can tell these workflows are
  still being refined before they opt into them.

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
- Harpoon-style session bookmarks with `H`, `M`, and `1`-`9` quick
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
