# Agent Mainframe (amf)

Run many AI coding agents in parallel — each on its own branch,
each in its own terminal — without losing track of any of them.

`amf` is a terminal dashboard for managing concurrent
[Claude Code](https://docs.anthropic.com/en/docs/claude-code) and
[Opencode](https://opencode.ai) agent sessions. Each feature gets its
own tmux session and git worktree so agents work simultaneously without
conflicts. You watch them all from one place, jump into whichever needs
attention, and get notified the moment one is waiting for input.

NOTE: I'll add real screenshots eventually
```text
┌─ Agent Mainframe ─────────────────── ~/code ───────── ? help ─┐
└────────────────────────────────────────────────────────────────┘
┌ Projects ──────────────────────────────────────────────────────┐
│ v my-app  ~/code/my-app                                        │
│   ├─ ●  v  main          [vibeless]  2h ago   [2]             │
│   │       ├─ * claude                                          │
│   │       └─ > terminal                                        │
│   └─ ⠿  v  auth-rework   [vibe]      just now [1]             │
│              └─ * claude                                       │
│ v api-service  ~/code/api                                      │
│   └─ ■     cache-layer   [supervibe] Jan 10   [0]             │
└────────────────────────────────────────────────────────────────┘
┌────────────────────────────────────────────────────────────────┐
│ auth-rework [auth-rework]  ~/code/my-app/.worktrees/auth-rew…  │
│  n feature  Enter expand  c start  x stop  t +term  q quit     │
│       [Claude] 5h ┃┃┃░░░░░░░░░░░░ 20%  7d ┃░░░░░░░░░ 8%      │
└────────────────────────────────────────────────────────────────┘
```

## Features

- **Project / Feature hierarchy** — organize work by project and
  feature branch
- **Multi-session support** — each feature can have multiple Claude,
  terminal, and nvim sessions, all in the same tmux session
- **Vibe modes** — choose Vibeless (diff-review gate), Vibe
  (auto-accept edits), or SuperVibe (skip all permissions) per feature
- **Embedded tmux view** — watch agent output directly inside the TUI
  with full ANSI rendering
- **Git worktree integration** — each feature automatically gets its
  own worktree so agents work in parallel without conflicts
- **Notification system** — get alerted when an agent needs input;
  jump straight to the right session
- **Leader key chords** — vim-style `Ctrl+Space` leader key for quick
  actions while viewing a session
- **File browser** — browse and select project paths with an
  interactive explorer (`Ctrl+B`)
- **Opencode support** — use Opencode as an alternative agent
  alongside or instead of Claude Code
- **Non-git projects** — projects don't require a git repository
  (worktree features are disabled for those)

## Prerequisites

### Required

- **tmux** — must be installed and in `PATH`
  ([installation guide](https://github.com/tmux/tmux/wiki/Installing))

### Agent (choose one or both)

- **Claude CLI** — required for Claude Code sessions
  ([Claude Code docs](https://docs.anthropic.com/en/docs/claude-code))
- **Opencode** — optional alternative agent
  ([opencode.ai](https://opencode.ai))

### Git (optional)

- **git** — required only for git projects with worktree features
  (non-git projects are supported without git)

### Optional

- **GPU-accelerated terminal** (Ghostty, Wezterm, Kitty, Alacritty) —
  highly recommended for smooth ANSI rendering in the embedded view
- **Nerd Font** — a
  [Nerd Font](https://www.nerdfonts.com/) is recommended for icon
  rendering. The app defaults to `nerd_font: true`; if your terminal
  font does not include Nerd Font glyphs, set `nerd_font: false` in
  `~/.config/amf/config.json` to use ASCII fallbacks instead.

## Installation

### Pre-built binaries (recommended)

Download the latest binary from the
[GitHub Releases page](https://github.com/eldridgerdev/agent-mainframe/releases).

| Platform | File |
| --- | --- |
| Linux x86_64 (musl) | `amf-x86_64-unknown-linux-musl` |
| Linux x86_64 (gnu) | `amf-x86_64-unknown-linux-gnu` |
| Linux aarch64 | `amf-aarch64-unknown-linux-gnu` |
| macOS (Apple Silicon) | `amf-aarch64-apple-darwin` |

Quick install:

Linux x86_64 (most portable):

```bash
curl -L https://github.com/eldridgerdev/agent-mainframe/releases/latest/download/amf-x86_64-unknown-linux-musl -o amf
chmod +x amf
sudo mv amf /usr/local/bin/
```

macOS (Apple Silicon):

```bash
curl -L https://github.com/eldridgerdev/agent-mainframe/releases/latest/download/amf-aarch64-apple-darwin -o amf
chmod +x amf
sudo mv amf /usr/local/bin/
```

Linux x86_64 (gnu):

```bash
curl -L https://github.com/eldridgerdev/agent-mainframe/releases/latest/download/amf-x86_64-unknown-linux-gnu -o amf
chmod +x amf
sudo mv amf /usr/local/bin/
```

Linux aarch64:

```bash
curl -L https://github.com/eldridgerdev/agent-mainframe/releases/latest/download/amf-aarch64-unknown-linux-gnu -o amf
chmod +x amf
sudo mv amf /usr/local/bin/
```

### Build from source

This project requires Rust 1.85+ (2024 edition). Install via rustup:

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

## Quick Start

1. Create or attach to a tmux session:

   ```bash
   tmux new -s main    # new session
   tmux attach         # or attach to existing
   ```

2. Launch the dashboard:

   ```bash
   amf
   ```

3. Press `N` to create a new project. Enter a name and the path to a
   git repository (or press `Ctrl+B` to browse for a directory).

4. Press `n` to add a feature. Enter a branch name, choose your agent
   (Claude or Opencode), and pick a vibe mode. A git worktree is
   created automatically (the first feature reuses the repo directory).
   Features auto-start on creation.

   ```text
            ┌─ New Feature ─────────────────────────────────┐
            │                                               │
            │  Branch: auth-rework_                         │
            │                                               │
            │  Agent:  [● Claude] [  Opencode]              │
            │                                               │
            │  Mode:                                        │
            │  ● Vibeless  diff-review gate on all edits    │
            │  ○ Vibe      auto-accept edits                │
            │  ○ SuperVibe skip all permission checks       │
            │                                               │
            │  Review hook: [✓]                             │
            │                                               │
            │              Enter confirm   Esc cancel       │
            └───────────────────────────────────────────────┘
   ```

5. Press `Enter` to view the embedded tmux output.

6. Use `Ctrl+Space` then a key for leader commands while in view mode.

## Keybindings

### Dashboard (Normal Mode)

| Key | Action |
| --- | --- |
| `j` / `k` / `↑` / `↓` | Navigate project tree |
| `h` | Collapse project / go to parent |
| `l` | Expand project / view feature |
| `Enter` | Toggle expand / view feature |
| `N` | Create new project |
| `n` | Create new feature |
| `a` | Add Claude session to feature |
| `t` | Add terminal session to feature |
| `v` | Add nvim session to feature |
| `r` | Rename session (when session selected) |
| `R` | Refresh statuses |
| `d` | Delete project or feature |
| `c` | Start feature session |
| `x` | Stop feature session |
| `f` | Filter by session type |
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
| `t` / `T` | Cycle between sessions (claude, terminal, nvim) |
| `w` | Open session switcher |
| `n` | Next feature (same project) |
| `p` | Previous feature (same project) |
| `/` | Command palette |
| `i` | Input requests picker |
| `r` | Refresh statuses |
| `x` | Stop session and exit view |
| `?` | Show help |

## How It Works

### Data Model

```text
ProjectStore
  └─ Project (name, repo path, is_git)
       └─ Feature (branch, workdir, tmux session, status,
                   mode: VibeMode, agent: Claude|Opencode)
            └─ FeatureSession (kind: Claude|Terminal|Nvim|Custom,
                               label, tmux window)
```

State is persisted as JSON at `~/.config/amf/projects.json`.

### Tmux Sessions

Each feature gets a tmux session named `amf-<branch>`. Features start
with a Claude (or Opencode) session and a terminal session. You can
add more sessions at any time:

| Key | Session type |
| --- | --- |
| `a` | Claude Code session |
| `t` | Plain shell terminal |
| `v` | Neovim in the feature's working directory |

Sessions can be renamed with `r` when a session item is selected.

Press `Enter` to enter the embedded view, which streams the tmux pane
output live through a vt100 parser and renders it with full ANSI color:

```text
  my-app /auth-rework /claude  [vibe] | Ctrl+Space command palette

  ╭──────────────────────────────────────────────────────────────╮
  │ > Implement JWT auth with refresh token rotation             │
  ╰──────────────────────────────────────────────────────────────╯

  ● Reading src/auth/mod.rs...
  ● Reading src/middleware/auth.rs...

  Here's my plan:
   1. Replace session tokens with signed JWTs
   2. Add refresh token rotation on each use
   3. Update the auth middleware to validate claims

  Shall I proceed? [Y/n] _
```

#### Session Picker

While viewing a feature, press `w` to open the session picker — a
popup listing all sessions for the current feature. Use `j`/`k` to
navigate, `Enter` to switch to a session, `r` to rename, `Esc` to
dismiss.

### Git Worktrees

The first feature in a project uses the repo directory directly.
Additional features get worktrees under `.worktrees/<branch>` so
multiple agents can work on the same repo simultaneously without
conflicts.

### Vibe Modes

Each feature is created with one of three vibe modes that control how
the agent handles permissions:

| Mode | Behavior |
| --- | --- |
| **Vibeless** | Diff-review hook gates all Edit/Write operations. You review each change before it's applied. |
| **Vibe** | Auto-accepts edits (`--permission-mode acceptEdits`). No diff-review hook. |
| **SuperVibe** | Skips all permission checks (`--dangerously-skip-permissions`). Shows a confirmation warning before creation. |

### Diff-Review Hook

In Vibeless mode, `amf` installs a Claude Code hook in the feature's
`.claude/settings.json`. The hook intercepts every `Edit`, `Write`,
and `MultiEdit` tool call before it executes and shows you the diff:

```text
  ╭─ Diff Review ──────────────────────────────────────────────╮
  │                                                            │
  │  src/auth/mod.rs                                           │
  │                                                            │
  │  - fn verify_token(token: &str) -> bool {                  │
  │  -     session_store.contains(token)                       │
  │  + fn verify_token(token: &str) -> Result<Claims> {        │
  │  +     jwt::decode(token, &KEYS.decoding)                  │
  │  }                                                         │
  │                                                            │
  │  Enter accept   r reject   Esc skip                        │
  ╰────────────────────────────────────────────────────────────╯
```

Press `Enter` to accept the change, `r` to reject it (the agent is
told the edit was refused), or `Esc` to skip review and allow it.

The hook is written to the worktree's local `.claude/settings.json`
only — your global Claude Code settings are never modified.

### Notifications

When a Claude Code session needs user input, a notification hook writes
a JSON file to `~/.config/amf/notifications/`. The dashboard polls
this directory and shows a badge. Press `i` to open the picker and
jump to the session that needs attention:

```text
       ┌─ Input Requests ──────────────────────────────┐
       │                                               │
       │ ▶ my-app / auth-rework                        │
       │     claude is waiting for input               │
       │                                               │
       │ ▶ my-app / main                               │
       │     diff review pending                       │
       │                                               │
       │   j/k navigate   Enter jump   Esc close       │
       └───────────────────────────────────────────────┘
```

Notification hooks are configured automatically in each feature's
`.claude/settings.json` when the session starts.

### Opencode Support

[Opencode](https://opencode.ai) is supported as an alternative to
Claude Code. When creating a feature, choose **Opencode** as the
agent. `amf` launches it in the same tmux session structure and
monitors it the same way.

## Configuration

The config file lives at `~/.config/amf/config.json` and is created
automatically with defaults on first run.

### Top-level options

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `nerd_font` | bool | `true` | Enable Nerd Font icons. Set to `false` to use ASCII fallbacks. |
| `opencode_theme` | string | `"catppuccin-frappe"` | Theme name passed to Opencode. |
| `zai` | object? | `null` | ZAI usage tracking. `null` or omitted disables ZAI in the status bar. |

### `zai` — token usage limits (optional)

_Not currently working. You can put some numbersin the token limits and guess if you want._
Controls whether ZAI usage is shown in the status bar. Set to `null`
or omit the key entirely to disable — the status bar will only show
Claude usage.

```json
"zai": null
```

To enable ZAI, set `plan` to one of the presets or override individual
limits manually:

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
`~/.config/amf/config.json` or per-project in `.amf/config.json` at
the repo root. Project-level settings are merged on top of global ones.

#### `custom_sessions`

Add extra session types that appear alongside Claude, Terminal, and
Nvim when creating sessions.

```json
"custom_sessions": [
  {
    "name": "Docs",
    "command": "npm run docs:dev",
    "on_stop": "pkill -f 'docs:dev'",
    "window_name": "docs",
    "working_dir": "packages/docs"
  }
]
```

| Key | Type | Description |
| --- | --- | --- |
| `name` | string | Display name shown in the session list. |
| `command` | string? | Shell command to run when the session starts. |
| `on_stop` | string? | Shell command to run when the session is stopped or removed. Runs via `sh -c` in the feature's workdir with `AMF_SESSION_ID` and `AMF_STATUS_DIR` set. Fire-and-forget (non-blocking). |
| `window_name` | string? | tmux window name (defaults to a slug of `name`). |
| `working_dir` | path? | Working directory relative to the feature's workdir. |

##### Session Status Text

Custom sessions can relay runtime information back to the
dashboard. When a custom session starts, two environment
variables are exported into its tmux window:

| Variable | Description |
| --- | --- |
| `AMF_SESSION_ID` | The session's unique ID. |
| `AMF_STATUS_DIR` | Path to the status directory (`{workdir}/.amf/session-status/`). |

Write a status file to display text under the session
entry in the project tree:

```bash
echo "API :3000 | DB :5432" \
  > "$AMF_STATUS_DIR/$AMF_SESSION_ID.txt"
```

AMF polls the status file every 5 seconds and displays the
first line in the dashboard. The text appears in a muted
style below the session name:

```text
  │   ├─ $ Dev Servers
  │   │   API :3000 | DB :5432
  │   └─ > terminal
```

To clear the status, delete the file or write an empty
string. The `.amf/` directory is local to the worktree and
should be added to `.gitignore`.

#### `lifecycle_hooks`

Shell scripts executed automatically on feature lifecycle events. The
script receives the feature's working directory as the first argument.

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

Remap dashboard normal-mode keys. The key is the action name and the
value is the replacement character.

```json
"keybindings": {
  "create_feature": "f",
  "delete": "D"
}
```

Available actions: `quit`, `create_project`, `create_feature`,
`start_session`, `stop_session`, `delete`, `help`,
`search`, `refresh`, `filter`.

#### `feature_presets`

Presets appear as quick-select options when creating a new feature,
pre-filling the vibe mode, agent, and other settings.

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

## Built-in OpenCode Themes

AMF includes custom transparent-background themes for opencode that are automatically injected into every worktree. These themes are designed to work well when viewing opencode inside AMF's embedded tmux view.

NOTE: Normal opencode themes don't look right in the embedded tmux view so I have to extend and modify them. I will port other more themes to amf-themes as I go

### Available Themes

- **amf** - Nord-based theme with transparent background
- **amf-tokyonight** - Tokyo Night with transparent background
- **amf-catppuccin** - Catppuccin Mocha with transparent background

### Using the Themes

When you start a feature in AMF, these themes are automatically added to `.opencode/themes/` in your worktree. You can then:

1. Use the `/theme` command in opencode to select a theme
2. Edit `.opencode/tui.json` in the worktree to set your preferred theme

The themes are embedded in the AMF binary, so they're always available without any external dependencies.

Or build without installing:

```bash
cargo build --release
# binary at target/release/amf
```


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
├── main.rs            # entry point, event loop
├── app/
│   ├── mod.rs         # App struct, config types, new/save
│   ├── state.rs       # AppMode, Selection, dialog states
│   ├── navigation.rs  # tree navigation, selection getters
│   ├── sync.rs        # status polling, thinking detection
│   ├── project_ops.rs # project CRUD, path browsing
│   ├── feature_ops.rs # feature create/start/stop/delete
│   ├── session_ops.rs # session picker, add/remove sessions
│   ├── view.rs        # embedded tmux view, leader key
│   ├── switcher.rs    # session switcher
│   ├── notifications.rs # notification scanning
│   ├── hooks.rs       # lifecycle hook execution
│   ├── opencode.rs    # opencode session management
│   ├── search.rs      # search and jump
│   ├── commands.rs    # command picker
│   ├── rename.rs      # session renaming
│   ├── review.rs      # final review trigger
│   ├── setup.rs       # notification hooks, config loading
│   ├── util.rs        # path/string helpers
│   └── tests.rs       # unit tests
├── project.rs         # ProjectStore / Project / Feature models,
│                      # JSON persistence
├── extension.rs       # extension config (presets, hooks,
│                      # sessions)
├── tmux.rs            # TmuxManager — all tmux interaction
├── worktree.rs        # WorktreeManager — git worktree ops
├── claude.rs          # ClaudeLauncher — claude CLI wrapper
├── usage.rs           # token usage tracking (Claude / ZAI)
├── traits.rs          # shared traits (TmuxOps, WorktreeOps)
├── handlers/
│   ├── mod.rs         # top-level key dispatch
│   ├── normal.rs      # dashboard normal mode
│   ├── view.rs        # embedded tmux view mode
│   ├── dialog.rs      # project/delete/rename handlers
│   ├── feature_creation.rs # feature creation wizard
│   ├── browse.rs      # path browser
│   ├── hooks.rs       # hook/delete-progress handlers
│   ├── picker.rs      # notification/session/command pickers
│   ├── search.rs      # search mode
│   ├── change_reason.rs # diff review prompt
│   ├── input.rs       # paste handling
│   └── mouse.rs       # mouse events
└── ui/
    ├── mod.rs         # top-level draw dispatch
    ├── dashboard.rs   # layout, ANSI rendering
    ├── list.rs        # project tree rendering
    ├── header.rs      # header bar
    ├── status.rs      # status bar + usage meters
    ├── pane.rs        # embedded tmux ANSI view
    ├── picker.rs      # picker overlays
    └── dialogs/
        ├── mod.rs     # re-exports
        ├── project.rs # create/delete project dialogs
        ├── feature.rs # feature creation, supervibe confirm
        ├── session.rs # rename session
        ├── help.rs    # keybindings help
        ├── browse.rs  # path browser dialog
        ├── search.rs  # search dialog
        └── hooks.rs   # hook/review dialogs
```

### Key Dependencies

- [ratatui](https://ratatui.rs/) 0.29 — terminal UI framework
- [crossterm](https://github.com/crossterm-rs/crossterm) 0.28 —
  terminal input/output
- [vt100](https://github.com/doy/vt100-rust) 0.15 — ANSI escape
  sequence parsing for embedded tmux rendering
- [clap](https://github.com/clap-rs/clap) 4 — CLI argument parsing
- [serde](https://serde.rs/) / serde_json — JSON serialization
- [chrono](https://github.com/chronotope/chrono) 0.4 — timestamps
- [ratatui-explorer](https://github.com/tatounee/ratatui-explorer)
  0.2 — file browser widget for path selection

### Architecture Notes

- All external tool interaction (tmux, git, claude) goes through
  `std::process::Command` in dedicated manager structs
- The event loop polls at 50ms in viewing mode (for smooth tmux
  rendering) and 250ms otherwise
- Status sync polls tmux every 5 seconds to reconcile feature statuses
  with actual session state
- The embedded tmux view captures ANSI output from tmux and renders it
  through a vt100 parser into ratatui spans
- The embedded tmux view renders agent output directly in the TUI
  without leaving the dashboard
- Hook files are written to the worktree's local `.claude/settings.json`
  only — global settings are never modified. On startup,
  `cleanup_global_hooks()` removes any previously-injected entries.

### Contributing

Contributions are welcome. There are no tests yet — the main way to
verify changes is to run `cargo check && cargo clippy` and then
exercise the TUI manually.

1. Fork the repo and create a feature branch.
2. Make your changes. Run `cargo check && cargo clippy -- -W
   clippy::all` and fix any warnings before submitting.
3. Open a pull request with a short description of what changed and
   why.

The project uses Rust 2024 edition (rustc 1.85+).

---

*Last updated: 2026-03-01*
