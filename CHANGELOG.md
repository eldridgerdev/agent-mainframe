# Changelog

All notable changes to AMF are documented in this file.

This changelog follows a Keep a Changelog style layout. Use
`## [Unreleased]` for pending work, then add a dated release section
when cutting a version. Major and minor releases are expected to
document user-facing changes and any migration notes here before they
are tagged.

## [Unreleased]

_No unreleased changes yet._

## [v0.17.2] - 2026-04-21

### Fixed

- Embedded tmux view updates now reseed from tmux when control-mode output
  arrives, which prevents stale whitespace from lingering until the next
  manual input or view refresh.

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
