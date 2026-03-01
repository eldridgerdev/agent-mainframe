# Agent Mainframe (amf)

A terminal UI for managing multiple concurrent
[Claude Code](https://docs.anthropic.com/en/docs/claude-code)
agent sessions. Each session runs in its own tmux session,
and the dashboard lets you create, monitor, switch between,
and interact with them all from one place.

NOTE: Opencode is supported but a little buggy (mostly visual)

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

### Required

- **Rust** (edition 2024, requires rustc 1.85+)

- **tmux** - must be installed and in `PATH`
  ([installation instructions](https://github.com/tmux/tmux/wiki/Installing))

- **claude** CLI - the
  [Claude Code](https://docs.anthropic.com/en/docs/claude-code)
  CLI must be installed and authenticated

- **git** - used for worktree management

### Optional

- **GPU accelerated terminal** (Ghostty, Wezterm, Kitty,
  Alacritty) - highly recommended for smooth ANSI rendering

- **Nerd Font** - a
  [Nerd Font](https://www.nerdfonts.com/) is recommended
  for icon rendering. The app defaults to `nerd_font: true`;
  if your terminal font does not include Nerd Font glyphs,
  set `nerd_font: false` in `~/.config/amf/config.json` to
  use ASCII fallbacks instead.

## Installation

### Pre-built binaries (recommended)

Download the latest binary from the
[GitHub Releases page](https://github.com/eldridgerdev/agent-mainframe/releases).

| Platform              | File                              |
| --------------------- | --------------------------------- |
| Linux x86_64 (musl)   | `amf-x86_64-unknown-linux-musl`  |
| Linux x86_64 (gnu)    | `amf-x86_64-unknown-linux-gnu`   |
| Linux aarch64         | `amf-aarch64-unknown-linux-gnu`  |
| macOS (Apple Silicon) | `amf-aarch64-apple-darwin`       |

Quick install (Linux x86_64 musl — most portable):

```bash
curl -L \
  https://github.com/eldridgerdev/agent-mainframe/releases/latest/download/amf-x86_64-unknown-linux-musl \
  -o amf
chmod +x amf
sudo mv amf /usr/local/bin/
```

### Build from source

This project requires Rust 1.85+ (2024 edition). Install using rustup:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

After installation, restart your shell or run:

```bash
source ~/.cargo/env
```

Then clone and install:

```bash
git clone https://github.com/eldridgerdev/agent-mainframe
cd agent-mainframe
cargo install --path .
```

This installs the `amf` binary to `~/.cargo/bin/`.

Or build without installing:

```bash
cargo build --release
# binary at target/release/amf
```

## Quick Start
0. Create or attach to a TMUX session (tmux new / tmux attach)
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
NOTE: hotkeys are a bit of a mess, I put 0 thought into them and just went with AI suggestions.
I plan on cleaning them up and making them configurable

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
`~/.config/amf/projects.json`.

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

Sessions can be renamed with `r` when a session is
selected on the dashboard.

#### Session Picker

While viewing a feature (`Enter`), press `w` to open the
session picker — a popup listing all sessions for the
current feature. Use `j`/`k` to navigate, `Enter` to
switch to a session, `r` to rename the selected session,
and `Esc` to dismiss.

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
`~/.config/amf/notifications/`. The
dashboard polls this directory and shows a badge. Press `i`
to open the picker and jump to the session that needs
attention.

The notification hooks are configured automatically in each
feature's `.claude/settings.json` when the session starts.

## Configuration

The config file lives at `~/.config/amf/config.json` and
is created automatically with defaults on first run.

### Top-level options

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `nerd_font` | bool | `true` | Enable Nerd Font icons. Set to `false` to use ASCII fallbacks. |
| `opencode_theme` | string | `"catppuccin-frappe"` | Theme name passed to Opencode. |
| `zai` | object? | `null` | ZAI usage tracking. `null` or omitted disables ZAI in the status bar. |

### `zai` — token usage limits (optional)

Controls whether ZAI usage is shown in the status bar.
Set to `null` or omit the key entirely to disable ZAI
tracking — the status bar will only show Claude usage and
will not rotate between models.

```json
"zai": null
```

To enable ZAI, set `plan` to one of the presets or
override individual limits manually:

```json
"zai": {
  "plan": "coding-plan"
}
```

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `plan` | string | `"free"` | Preset plan: `"free"`, `"coding-plan"`, or `"unlimited"`. |
| `monthly_token_limit` | number? | (from plan) | Override the monthly token limit. |
| `weekly_token_limit` | number? | (from plan) | Override the weekly token limit. |
| `five_hour_token_limit` | number? | (from plan) | Override the rolling 5-hour token limit. |

### `extension` — customizations

The `extension` block can be set globally in
`~/.config/amf/config.json` or per-project in
`.amf/config.json` at the repo root. Project-level
settings are merged on top of global ones.

#### `custom_sessions`

Add extra session types that appear alongside Claude,
Terminal, and Nvim when creating sessions.

```json
"custom_sessions": [
  {
    "name": "Docs",
    "command": "npm run docs:dev",
    "window_name": "docs",
    "working_dir": "packages/docs"
  }
]
```

| Key | Type | Description |
| --- | --- | --- |
| `name` | string | Display name shown in the session list. |
| `command` | string? | Shell command to run when the session starts. |
| `window_name` | string? | tmux window name (defaults to a slug of `name`). |
| `working_dir` | path? | Working directory relative to the feature's workdir. |

#### `lifecycle_hooks`

Shell scripts executed automatically on feature lifecycle
events. The script receives the feature's working
directory as the first argument.

```json
"lifecycle_hooks": {
  "on_start": "/path/to/setup.sh",
  "on_stop": "/path/to/teardown.sh",
  "on_worktree_created": "/path/to/init-worktree.sh"
}
```

| Key | Description |
| --- | --- |
| `on_start` | Runs when a feature session is started. |
| `on_stop` | Runs when a feature session is stopped. |
| `on_worktree_created` | Runs once after a new worktree is created for a feature. |

#### `keybindings`

Remap dashboard normal-mode keys. The key is the action
name and the value is the replacement character.

```json
"keybindings": {
  "create_feature": "f",
  "delete": "D"
}
```

Available actions: `quit`, `create_project`,
`create_feature`, `start_session`, `stop_session`,
`delete`, `sessions`, `help`, `search`, `refresh`,
`filter`.

#### `feature_presets`

Presets appear as quick-select options when creating a
new feature, pre-filling the vibe mode, agent, and other
settings.

```json
"feature_presets": [
  {
    "name": "Quick fix",
    "branch_prefix": "fix/",
    "mode": "Vibe",
    "agent": "Claude",
    "review": false,
    "enable_chrome": false,
    "enable_notes": false
  }
]
```

| Key | Type | Description |
| --- | --- | --- |
| `name` | string | Preset label shown during feature creation. |
| `branch_prefix` | string? | Prepended to the branch name automatically. |
| `mode` | string | Vibe mode: `"Vibeless"`, `"Vibe"`, or `"SuperVibe"`. |
| `agent` | string | Agent to use: `"Claude"` or `"Opencode"`. |
| `review` | bool | Whether to enable the diff-review hook. |
| `enable_chrome` | bool | Enable browser/Chrome integration. |
| `enable_notes` | bool | Enable session notes. |

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
