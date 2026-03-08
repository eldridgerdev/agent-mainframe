# AGENTS.md

This file provides guidance to AI coding agents
(Claude Code, OpenCode, etc.) when working with
this repository.

## Build and Run

```bash
cargo build            # debug build
cargo run              # run the TUI (binary name: amf)
cargo build --release  # release build
cargo check            # type-check without building
cargo clippy           # lint
```

The binary is named `amf` (agent-mainframe). The package
name in Cargo.toml is `agent-mainframe`. There are no tests
yet.

## Runtime Requirements

- **tmux** must be installed and in PATH (checked at startup)
- **claude** CLI (Claude Code) is launched inside tmux
  sessions

## Architecture

Rust TUI application that manages multiple concurrent Claude
Code agent sessions, each running in its own tmux session.
Built with ratatui 0.29 / crossterm 0.28 / vt100 0.15.
Uses Rust 2024 edition.

### Data Model (project.rs)

```text
ProjectStore (version: u32, projects: Vec<Project>)
  └─ Project (id, name, repo: PathBuf, collapsed, features,
             created_at)
       └─ Feature (id, name, branch, workdir: PathBuf,
                   is_worktree, tmux_session, claude_session_id,
                   status: ProjectStatus, created_at,
                   last_accessed)

ProjectStatus: Active | Idle | Stopped
```

State persisted as JSON at
`~/.config/amf/projects.json`.
Tmux sessions are prefixed `amf-` (e.g., `amf-mybranch`).

### App State & Modes (app/)

The `app/` directory is split into focused submodules:

```text
app/
├── mod.rs           # App struct, AppConfig, ZaiPlanConfig,
│                    # new(), save(), re-exports
├── state.rs         # AppMode, Selection, ViewState,
│                    # CreateProjectState, etc.
├── navigation.rs    # visible_items(), select_next/prev(),
│                    # selected_project/feature/session()
├── sync.rs          # sync_statuses(), thinking status
├── project_ops.rs   # toggle_collapse(), create/delete project,
│                    # browse path
├── feature_ops.rs   # create/start/stop/delete feature
├── session_ops.rs   # session picker, add/remove sessions
├── view.rs          # enter/exit view, leader key, scroll,
│                    # view navigation
├── switcher.rs      # session switcher
├── notifications.rs # scan_notifications(), handle select
├── hooks.rs         # lifecycle hooks
├── opencode.rs      # opencode session management
├── search.rs        # search and jump
├── commands.rs      # command picker
├── rename.rs        # session renaming
├── review.rs        # trigger_final_review()
├── setup.rs         # ensure_notification_hooks(),
│                    # ensure_notify_scripts(), load_config()
├── util.rs          # shorten_path(), slugify(),
│                    # detect_repo_path(), detect_branch()
└── tests.rs         # all #[cfg(test)] tests
```

Key App methods (spread across submodules):

- `new(store_path) -> Result<Self>`
- `save() -> Result<()>`
- `visible_items() -> Vec<VisibleItem>` - flattened tree
- `select_next/prev()` - wrapping navigation
- `sync_statuses()` - polls tmux sessions
- `selected_project() -> Option<&Project>`
- `selected_feature() -> Option<(&Project, &Feature)>`
- `toggle_collapse()`
- Project CRUD: `start_create_project()`,
  `create_project()`, `delete_project()`
- Feature CRUD: `start_create_feature()`,
  `create_feature()`, `start_feature()`,
  `stop_feature()`, `delete_feature()`
- View: `enter_view()`, `exit_view()`,
  `view_next/prev_feature()`, `switch_to_selected()`,
  `open_terminal()`
- Leader: `activate_leader()`, `deactivate_leader()`,
  `leader_timed_out()`

### Event Loop & Key Handling (main.rs)

`run_loop()` drives the event loop with 50ms poll in
Viewing mode, 250ms otherwise. Status sync every 5s.

Key dispatch per mode:

- `handle_normal_key()` - j/k nav, N/n create, Enter
  view/collapse, c start, x stop, s switch, d delete,
  h help, r refresh, q quit
- `handle_view_key()` - Ctrl+Q exit, Ctrl+Space leader,
  else forward to tmux via `crossterm_key_to_tmux()`
- `handle_leader_key()` - q/t/s/n/p/r/x/h after
  Ctrl+Space
- `handle_create_project_key()` - Enter/Tab/Backspace/Char
- `handle_create_feature_key()` - Enter/Backspace/Char
- `handle_delete_*_key()` - y confirm, n/Esc cancel
- `handle_help_key()` - Esc/q/h close

### External Tool Managers

**TmuxManager** (tmux.rs) - all static methods:

- `check_available()`, `session_exists(session)`
- `create_session(session, workdir)` - creates `claude` +
  `terminal` windows
- `launch_claude(session, resume_session_id)`
- `is_inside_tmux()`, `current_session()`
- `switch_client(session)`, `attach_session(session)`
- `kill_session(session)`, `list_sessions()` (filters
  `amf-*`)
- `capture_pane(session, window)`,
  `capture_pane_ansi(session, window)`
- `resize_pane(session, window, cols, rows)`
- `send_literal(session, window, text)`,
  `send_key_name(session, window, key_name)`,
  `send_keys(session, window)`

**WorktreeManager** (worktree.rs) - all static methods:

- `repo_root(path) -> Result<PathBuf>`
- `is_worktree(path) -> bool`
- `create(repo, name, branch) -> Result<PathBuf>` -
  creates under `.worktrees/`, handles existing vs new
  branch
- `remove(repo, worktree_path)`
- `list(repo) -> Result<Vec<WorktreeInfo>>`
- `current_branch(path) -> Result<Option<String>>`

**ClaudeLauncher** (claude.rs):

- `check_available()`
- `launch_interactive(session, resume_id)`
- `run_headless(workdir, prompt) -> Result<String>`
- `run_headless_json(workdir, prompt) -> Result<String>`

### UI Rendering (ui/)

`draw(frame, app)` in `ui/dashboard.rs` dispatches to:

- `draw_pane_view()` - full-screen embedded tmux with ANSI
  rendering via vt100 parser
- `draw_header()`, `draw_project_list()`,
  `draw_status_bar()`
- Dialog overlays in `ui/dialogs/`:
   - `project.rs` - create/delete project dialogs
   - `feature.rs` - create/delete feature, supervibe
     confirm, deleting feature progress
   - `session.rs` - rename session dialog
   - `help.rs` - keybindings help overlay
   - `browse.rs` - path browser dialog
   - `search.rs` - search dialog
   - `hooks.rs` - change reason, running hook, hook
     prompt dialogs
- `centered_rect(percent_x, percent_y, area) -> Rect`
- `ansi_to_ratatui_text(raw, cols, rows) -> Vec<Line>`

### Key Handlers (handlers/)

Key dispatch is split across focused modules:

- `handlers/normal.rs` - dashboard normal mode
- `handlers/view.rs` - embedded tmux view mode
- `handlers/dialog.rs` - project creation, help, delete
  confirms, rename
- `handlers/feature_creation.rs` - multi-step feature
  creation wizard
- `handlers/browse.rs` - path browser key handling
- `handlers/hooks.rs` - running hook, deleting feature,
  hook prompt handlers
- `handlers/picker.rs` - notification, session, command,
  opencode pickers
- `handlers/search.rs` - search mode
- `handlers/change_reason.rs` - diff review prompt
- `handlers/mouse.rs` - mouse event handling

### Debug Logging

**NEVER use `println!` or `eprintln!` in TUI code** - it corrupts
the terminal display. Use the built-in debug log instead.

To view the debug log at runtime, press `D` from the dashboard.

**Log file location:** `~/.local/state/amf/debug.log`

You can tail this file in a separate terminal:
```bash
tail -f ~/.local/state/amf/debug.log
```

**Usage in code:**

```rust
// From anywhere with access to `app`:
app.log_debug("context", format!("value: {}", value));
app.log_info("context", "operation completed".to_string());
app.log_warn("context", "something unexpected".to_string());
app.log_error("context", format!("failed: {}", err));
```

**Log levels** (color-coded in UI):
- `DEBUG` (gray) - detailed tracing
- `INFO` (green) - normal operations
- `WARN` (yellow) - unexpected but handled
- `ERROR` (red) - failures

**Context strings** should be short identifiers like:
- `"amf"` - app lifecycle
- `"sync"` - status sync operations
- `"tmux"` - tmux interactions
- `"worktree"` - git worktree operations
- `"hooks"` - lifecycle hooks

Errors from `show_error()` are automatically logged to the
debug log with level ERROR.

### Key Design Patterns

- All external tool interaction (tmux, git, claude) goes
  through `std::process::Command` in dedicated manager
  structs
- Status sync polls tmux every 5 seconds to reconcile
  `ProjectStatus` with actual session state
- When running inside tmux, switching uses
  `switch-client`; outside tmux, the TUI exits and
  attaches via `should_switch` field
- First feature per project uses repo dir directly;
  subsequent features get git worktrees under
  `.worktrees/`
- ViewState embeds tmux pane content by capturing ANSI
  output and rendering through vt100 parser
- Leader key (Ctrl+Space) activates a 2-second chord
  window for view-mode commands
- Feature creation has exactly three vibe modes:
  `Vibeless`, `Vibe`, and `SuperVibe`. `Review` is not a
  vibe mode. It is a separate boolean/checkbox that can be
  enabled alongside any vibe mode. Do not re-add `Review`
  to `VibeMode`, mode pickers, or presets.
- **Never modify `~/.claude/settings.json` (global) or
  `~/.config/opencode/` (global opencode config) to inject
  hooks or settings.** Instead, write to the worktree's
  local `.claude/settings.json` (or `.opencode/` equivalent)
  via `ensure_notification_hooks()`. For non-worktree
  features (first feature that uses the repo dir directly),
  write to `{repo}/.claude/settings.json`. On startup,
  `cleanup_global_hooks()` actively removes any
  previously-injected global entries.

### Dependencies (Cargo.toml)

- ratatui 0.29, crossterm 0.28, vt100 0.15
- clap 4 (derive), serde 1, serde_json 1
- uuid 1 (v4), dirs 6, anyhow 1, chrono 0.4 (serde)
