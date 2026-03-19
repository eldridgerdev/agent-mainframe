# Changelog

All notable changes to AMF are documented in this file.

This changelog follows a Keep a Changelog style layout. Use
`## [Unreleased]` for pending work, then add a dated release section
when cutting a version. Major and minor releases are expected to
document user-facing changes and any migration notes here before they
are tagged.

## [Unreleased]

_No unreleased changes yet._

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
