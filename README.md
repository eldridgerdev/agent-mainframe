# Agent Mainframe (amf)

A terminal UI for managing multiple concurrent
[Claude Code](https://docs.anthropic.com/en/docs/claude-code)
agent sessions. Each session runs in its own tmux session,
and the dashboard lets you create, monitor, switch between,
and interact with them all from one place.

## Features

- **Project / Feature hierarchy** - organize work by
  project and feature branch
- **Embedded tmux view** - watch Claude Code output
  directly inside the TUI with full ANSI rendering
- **Git worktree integration** - each feature automatically
  gets its own worktree so agents work in parallel without
  conflicts
- **Notification system** - get alerted when an agent needs
  input, jump straight to the right session
- **Leader key chords** - vim-style Ctrl+Space leader key
  for quick actions while viewing a session

## Prerequisites

- **Rust** (edition 2024, requires rustc 1.85+)
- **tmux** - must be installed and in `PATH`
- **claude** CLI - the
  [Claude Code](https://docs.anthropic.com/en/docs/claude-code)
  CLI must be installed and authenticated
- **git** - used for worktree management

## Installation

```bash
git clone <repo-url>
cd claude_super_vibeless
cargo install --path .
```

This installs the `amf` binary to `~/.cargo/bin/`.

Or build without installing:

```bash
cargo build --release
# binary at target/release/amf
```

## Quick Start

1. Launch the dashboard:

   ```bash
   amf
   ```

2. Press `N` to create a new project. Enter a name and
   the path to a git repository.

3. Press `n` to add a feature. Enter a branch name - a
   git worktree is created automatically (the first
   feature reuses the repo directory).

4. Press `c` to start a Claude Code session for the
   selected feature, or press `Enter` to view its
   embedded tmux output.

5. Use `Ctrl+Space` then a key for leader commands while
   viewing a session.

## Keybindings

### Dashboard (Normal Mode)

| Key | Action |
| --- | --- |
| `j` / `k` / `Up` / `Down` | Navigate project tree |
| `Enter` | View feature (embedded tmux) / expand project |
| `s` | Switch to feature (tmux attach) |
| `t` | Open terminal window |
| `N` | Create new project |
| `n` | Create new feature |
| `d` | Delete project or feature |
| `c` | Start feature session |
| `x` | Stop feature session |
| `i` | Input requests picker |
| `r` | Refresh statuses |
| `h` | Toggle help |
| `q` / `Esc` | Quit |

### Viewing Mode (Embedded tmux)

All keys are forwarded to the tmux session except:

| Key | Action |
| --- | --- |
| `Ctrl+Q` | Exit view, return to dashboard |
| `Ctrl+Space` | Activate leader key (2s window) |

### Leader Commands (after Ctrl+Space)

| Key | Action |
| --- | --- |
| `q` | Exit view |
| `t` | Toggle claude / terminal window |
| `s` | Switch/attach to tmux session directly |
| `n` | Next feature (same project) |
| `p` | Previous feature (same project) |
| `i` | Input requests picker |
| `r` | Refresh statuses |
| `x` | Stop session and exit view |
| `h` | Show help |

## How It Works

### Data Model

```text
ProjectStore
  └─ Project (name, repo path)
       └─ Feature (branch, workdir, tmux session, status)
```

State is persisted as JSON at
`~/.config/claude-super-vibeless/projects.json`.

### Tmux Sessions

Each feature gets a tmux session named `amf-<branch>` with
two windows:

- **claude** - runs `claude` CLI interactively
- **terminal** - a plain shell in the feature's working
  directory

### Git Worktrees

The first feature in a project uses the repo directory
directly. Additional features get worktrees under
`.worktrees/<branch>` so multiple agents can work on the
same repo simultaneously without conflicts.

### Notifications

When a Claude Code session needs user input, a notification
hook writes a JSON file to
`~/.config/claude-super-vibeless/notifications/`. The
dashboard polls this directory and shows a badge. Press `i`
to open the picker and jump to the session that needs
attention.

The notification hooks are configured automatically in each
feature's `.claude/settings.json` when the session starts.

## Development

### Build Commands

```bash
cargo build            # debug build
cargo run              # run the TUI
cargo build --release  # release build
cargo check            # type-check without full build
cargo clippy           # lint
```

### Project Structure

```text
src/
├── main.rs        # entry point, event loop, key handling
├── app.rs         # App state, modes, CRUD operations,
│                  # notification hooks
├── project.rs     # ProjectStore / Project / Feature models,
│                  # JSON persistence
├── tmux.rs        # TmuxManager - all tmux interaction
├── worktree.rs    # WorktreeManager - git worktree ops
├── claude.rs      # ClaudeLauncher - claude CLI wrapper
└── ui/
    └── dashboard.rs  # ratatui rendering, help overlay,
                      # ANSI-to-TUI conversion
```

### Key Dependencies

- [ratatui](https://ratatui.rs/) 0.29 - terminal UI
  framework
- [crossterm](https://github.com/crossterm-rs/crossterm)
  0.28 - terminal input/output
- [vt100](https://github.com/doy/vt100-rust) 0.15 - ANSI
  escape sequence parsing for embedded tmux rendering
- [clap](https://github.com/clap-rs/clap) 4 - CLI argument
  parsing
- [serde](https://serde.rs/) / serde_json - JSON
  serialization
- [chrono](https://github.com/chronotope/chrono) 0.4 -
  timestamps

### Architecture Notes

- All external tool interaction (tmux, git, claude) goes
  through `std::process::Command` in dedicated manager
  structs
- The event loop polls at 50ms in viewing mode (for smooth
  tmux rendering) and 250ms otherwise
- Status sync polls tmux every 5 seconds to reconcile
  feature statuses with actual session state
- The embedded tmux view captures ANSI output from tmux and
  renders it through a vt100 parser into ratatui spans
- When running inside tmux, switching uses
  `switch-client`; outside tmux, the TUI exits and
  attaches directly
