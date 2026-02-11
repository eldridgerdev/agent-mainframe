# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code)
when working with code in this repository.

## Build and Run

```bash
cargo build            # debug build
cargo run              # run the TUI (binary name: csv)
cargo build --release  # release build
```

The binary is named `csv` (claude-super-vibeless). There are
no tests yet.

## Runtime Requirements

- **tmux** must be installed and in PATH (checked at startup)
- **claude** CLI (Claude Code) is launched inside tmux
  sessions

## Architecture

This is a Rust TUI application that manages multiple
concurrent Claude Code agent sessions, each running in its
own tmux session. Built with ratatui/crossterm for the
terminal UI.

### Core Flow

The app creates a tmux session per project with two windows:
`claude` (runs `claude` CLI) and `terminal` (plain shell).
Tmux sessions are prefixed `csv-` (e.g., `csv-myproject`).
Project state is persisted as JSON at
`~/.config/claude-super-vibeless/projects.json`.

### Module Responsibilities

- **`main.rs`** - Terminal setup, event loop, all key
  handling dispatched by `AppMode` (Normal, Creating,
  Deleting)
- **`app.rs`** - `App` struct holds all application state
  (`ProjectStore`, selection index, mode, messages).
  Contains business logic for create/delete/switch/stop
  operations
- **`project.rs`** - `Project` and `ProjectStore` data
  models with JSON serialization. `ProjectStatus` enum
  (Active/Idle/Stopped) tracks session state
- **`tmux.rs`** - `TmuxManager` wraps all tmux subprocess
  calls (create/kill sessions, send keys, switch client,
  capture pane output)
- **`worktree.rs`** - `WorktreeManager` wraps git worktree
  commands. When a second project targets the same repo with
  a different branch, a worktree is created under
  `.worktrees/`
- **`claude.rs`** - `ClaudeLauncher` wraps the `claude` CLI
  for headless execution (text/JSON output modes)
- **`ui/dashboard.rs`** - All ratatui rendering: header,
  project list, status bar, create dialog overlay, delete
  confirmation overlay

### Key Design Patterns

- All external tool interaction (tmux, git, claude) goes
  through `std::process::Command` in dedicated manager
  structs
- Status sync polls tmux every 5 seconds to reconcile
  `ProjectStatus` with actual session state
- When running inside tmux, switching uses
  `switch-client`; outside tmux, the TUI exits and attaches
  via `should_switch` field
- Uses Rust 2024 edition
