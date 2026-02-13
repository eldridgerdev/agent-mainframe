# Agent Mainframe (amf)

A terminal UI for managing multiple concurrent
[Claude Code](https://docs.anthropic.com/en/docs/claude-code)
agent sessions. Each session runs in its own tmux session,
and the dashboard lets you create, monitor, switch between,
and interact with them all from one place.

## Features

- **Project / Feature hierarchy** - organize work by
  project and feature branch
- **Multi-session support** - each feature can have
  multiple Claude, terminal, and nvim sessions
- **Vibe modes** - choose Vibeless (diff-review gate),
  Vibe (auto-accept edits), or SuperVibe (skip all
  permissions) per feature
- **Embedded tmux view** - watch Claude Code output
  directly inside the TUI with full ANSI rendering
- **Git worktree integration** - each feature automatically
  gets its own worktree so agents work in parallel without
  conflicts
- **Notification system** - get alerted when an agent needs
  input, jump straight to the right session
- **Leader key chords** - vim-style Ctrl+Space leader key
  for quick actions while viewing a session
- **File browser** - browse and select project paths with
  an interactive file explorer (Ctrl+B)
- **Non-git projects** - projects don't require a git
  repository (worktree features are disabled)

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
   the path to a git repository (or press `Ctrl+B` to
   browse for a directory).

3. Press `n` to add a feature. Enter a branch name and
   select a vibe mode. A git worktree is created
   automatically (the first feature reuses the repo
   directory). Features auto-start on creation.

4. Press `Enter` to view the embedded tmux output, or
   `s` to switch directly into the tmux session.

5. Use `Ctrl+Space` then a key for leader commands while
   viewing a session.

## Keybindings

### Dashboard (Normal Mode)

| Key | Action |
| --- | --- |
| `j` / `k` / `Up` / `Down` | Navigate project tree |
| `h` | Collapse project / go to parent |
| `l` | Expand project / view feature |
| `Enter` | Toggle expand / view feature |
| `s` | Switch to feature (tmux attach) |
| `N` | Create new project |
| `n` | Create new feature |
| `a` | Add Claude session to feature |
| `t` | Add terminal session to feature |
| `v` | Add nvim session to feature |
| `r` | Rename session (on session) / refresh |
| `R` | Refresh statuses |
| `d` | Delete project or feature |
| `c` | Start feature session |
| `x` | Stop feature session |
| `i` | Input requests picker |
| `?` | Toggle help |
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
  └─ Project (name, repo path, is_git)
       └─ Feature (branch, workdir, tmux session, status,
                   mode: VibeMode)
            └─ FeatureSession (kind: Claude|Terminal|Nvim,
                               label, tmux window)
```

State is persisted as JSON at
`~/.config/claude-super-vibeless/projects.json`.

### Tmux Sessions

Each feature gets a tmux session named `amf-<branch>`.
Features start with a Claude session and a terminal
session, and you can add more sessions at any time:

- **Claude** (`a`) - runs `claude` CLI interactively,
  with CLI flags determined by the feature's vibe mode
- **Terminal** (`t`) - a plain shell in the feature's
  working directory
- **Nvim** (`v`) - opens neovim in the feature's working
  directory

Sessions can be renamed with `r` when selected.

### Git Worktrees

The first feature in a project uses the repo directory
directly. Additional features get worktrees under
`.worktrees/<branch>` so multiple agents can work on the
same repo simultaneously without conflicts.

### Vibe Modes

Each feature is created with one of three vibe modes that
control how Claude Code handles permissions:

| Mode | Behavior |
| --- | --- |
| **Vibeless** | Diff-review hook gates all Edit/Write operations. You review each change before it's applied. |
| **Vibe** | Auto-accepts edits (`--permission-mode acceptEdits`). No diff-review hook. |
| **SuperVibe** | Skips all permission checks (`--dangerously-skip-permissions`). Shows a confirmation warning before creation. |

The vibe mode is selected during feature creation and
determines both the CLI flags passed to `claude` and
whether the diff-review hook is installed in the feature's
`.claude/settings.json`.

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
- [ratatui-explorer](https://github.com/tatounee/ratatui-explorer)
  0.2 - file browser widget for path selection

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

---

*Last updated: 2026-02-13*
