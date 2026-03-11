# Agent Mainframe (amf)

Run many AI coding agents in parallel — each on its own branch,
each in its own terminal — without losing track of any of them.

`amf` is a terminal dashboard for managing concurrent
[Claude Code](https://docs.anthropic.com/en/docs/claude-code),
[Codex](https://github.com/openai/codex), and
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

- **Multi-agent workspace manager** — run Claude Code, Codex, and
  Opencode features side by side.
- **Project / feature tree** — organize work by repo, branch,
  nickname, ready state, timestamps, and one-line summaries.
- **Flexible session model** — each feature can host agent sessions,
  terminals, nvim, VSCode, and custom session types.
- **Embedded tmux view** — watch panes directly in the TUI with ANSI
  rendering, mouse text selection, and clipboard copy.
- **Worktree automation** — first feature can reuse the repo, later
  features get worktrees automatically; batch-create and fork features
  when you need parallel branches quickly.
- **Local hooks and notifications** — input requests are pushed over
  IPC when possible, with file-based fallback, and hooks stay local to
  the worktree.
- **Automation entrypoints** — external agents can call structured
  AMF actions like batch feature creation and get JSON replies instead
  of scripting the TUI.
- **Leader workflow** — `Ctrl+Space` opens session controls, final
  review, bookmarks, latest prompt, scroll mode, and debug tools.
- **Workspace-level customization** — merge global
  `~/.config/amf/config.json` with repo-local `.amf/config.json` for
  presets, lifecycle hooks, custom sessions, agent restrictions, and
  key remaps.
- **Theme system** — built-in AMF UI themes plus bundled Opencode
  themes that are injected into every worktree.
- **Non-git projects** — projects do not require git; worktree-only
  features are simply disabled for those repos.

## Prerequisites

### Required

- **tmux** — must be installed and in `PATH`
  ([installation guide](https://github.com/tmux/tmux/wiki/Installing))

### Agent (choose one or more)

- **Claude CLI** — required for Claude Code sessions
  ([Claude Code docs](https://docs.anthropic.com/en/docs/claude-code))
- **Codex CLI** — required for Codex sessions
  ([Codex repo](https://github.com/openai/codex))
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
- **VSCode CLI** — install `code` in `PATH` if you want VSCode
  sessions from the session picker.

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

### Upgrade and Release Notes

Upgrade an existing install:

```bash
amf upgrade
```

Check the installed version:

```bash
amf -V
```

User-facing release notes and migration guidance live in
[`CHANGELOG.md`](CHANGELOG.md).

## Automation

AMF exposes machine-friendly automation commands for external agents:

```bash
amf automation create-project --file docs/automation/create-project.example.json
amf automation create-feature --file docs/automation/create-feature.example.json
amf automation create-batch-features --file docs/automation/create-batch-features.example.json
```

Create-project and batch-feature templates, examples, and the JSON response format live in
[`docs/automation/README.md`](docs/automation/README.md).

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
   (Claude, Codex, or Opencode), and pick a vibe mode. A git worktree
   is created automatically when needed, and features auto-start on
   creation. Codex supports `Vibe` and `SuperVibe`; `Vibeless` is only
   available for agents with diff-review hook support.

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

5. Press `s` to add more sessions to a running feature. The picker can
   launch agent sessions, terminals, nvim, VSCode, and custom session
   types from your config.

6. Press `Enter` on a session to view the embedded tmux output.

7. Use `Ctrl+Space` then a key for leader commands while in view mode.

## Keybindings

### Dashboard (Normal Mode)

| Key | Action |
| --- | --- |
| `j` / `k` / `↑` / `↓` | Navigate project tree |
| `h` | Collapse project / go to parent |
| `l` | Expand project / feature |
| `Enter` | Toggle expand or view selected session |
| `N` | Create new project |
| `n` | Create new feature |
| `B` | Batch-create features for a workspace |
| `O` | Open the `~/.config/amf` settings project |
| `s` | Open session picker / add a session |
| `S` | Resume a Claude or Opencode session |
| `r` | Rename selected feature or session |
| `d` | Delete selected project, feature, or session |
| `c` | Start selected feature |
| `x` | Stop selected feature or remove selected session |
| `F` | Fork the selected feature into a new worktree |
| `f` | Filter by session type |
| `m` | Create or open `.claude/notes.md` as a Memo session |
| `y` | Toggle ready state for the selected feature |
| `Z` | Generate a one-line summary for the selected feature |
| `T` | Open the theme picker |
| `i` | Input requests picker |
| `/` | Search and jump |
| `D` | Open the debug log overlay |
| `R` | Refresh statuses |
| `?` | Toggle help |
| `q` / `Esc` | Quit |

### Viewing Mode (Embedded tmux)

All keys are forwarded to the tmux session except:

| Key | Action |
| --- | --- |
| `Ctrl+Q` | Exit view, return to dashboard |
| `Ctrl+Space` | Activate leader key (default 5s window, configurable) |

### Leader Commands (after Ctrl+Space)

| Key | Action |
| --- | --- |
| `q` | Exit view |
| `t` / `T` | Cycle between sessions |
| `w` | Open session switcher |
| `h` | Open bookmark picker popup |
| `n` | Next feature (same project) |
| `p` | Previous feature (same project) |
| `/` | Command palette |
| `i` | Input requests picker |
| `r` | Refresh statuses |
| `x` | Stop session and exit view |
| `f` | Trigger final review |
| `l` | Show the latest saved prompt |
| `o` / `S` | Toggle pane scroll mode |
| `D` | Open debug log |
| `H` / `M` | Bookmark / unbookmark current session |
| `1`-`9` | Jump to bookmark slot |
| `?` | Show help |

## How It Works

### Data Model

```text
ProjectStore (version: 4, session_bookmarks)
  └─ Project (name, repo path, is_git)
       └─ Feature (branch, nickname?, workdir, tmux session,
                   status, ready, mode, review, agent,
                   summary?)
            └─ FeatureSession (kind: Claude|Opencode|Codex|
                               Terminal|Nvim|VSCode|Custom,
                               label, tmux window)
```

State is persisted as JSON at `~/.config/amf/projects.json`.

### Tmux Sessions

Each feature gets a tmux session named `amf-<branch>`. Features start
with an agent session plus a terminal session. If notes are enabled, a
Memo nvim session can be added automatically too.

Use `s` to open the session picker and add more sessions at any time:

| Picker entry | Session type |
| --- | --- |
| `Claude` / `Opencode` / `Codex` | Another agent session in the same feature |
| `Terminal` | Plain shell terminal |
| `Nvim` | Neovim in the feature's working directory |
| `VSCode` | Open the feature workdir in VSCode via `code` |
| Custom entries | Commands defined in `extension.custom_sessions` |

Sessions can be renamed with `r` when a feature or session item is
selected. `S` can resume Claude or Opencode sessions for the selected
feature.

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

### Git Worktrees, Forks, and Batch Creation

The first feature in a project uses the repo directory directly.
Additional features get worktrees under `.worktrees/<branch>` so
multiple agents can work on the same repo simultaneously without
conflicts.

- `F` forks the selected feature into a new worktree, preserves
  uncommitted changes, and can export transcript context into
  `.claude/context.md`.
- `B` batch-creates numbered features for a repo when you want a set
  of parallel branches immediately.

### Vibe Modes

Each feature is created with one of three vibe modes that control how
the agent handles permissions:

| Mode | Behavior |
| --- | --- |
| **Vibeless** | Diff-review hook gates all Edit/Write operations. You review each change before it's applied. Available for Claude Code and Opencode. Codex does not support Vibeless diff review. |
| **Vibe** | Auto-accepts edits (`--permission-mode acceptEdits`). No diff-review hook. |
| **SuperVibe** | Skips all permission checks (`--dangerously-skip-permissions`). Shows a confirmation warning before creation. |

### Diff-Review Hook

In Vibeless mode, `amf` installs a Claude Code hook in the feature's
`.claude/settings.local.json`. The hook intercepts every `Edit`, `Write`,
and `MultiEdit` tool call before it executes and shows you the diff.
Codex worktrees do not support this hook path, so Codex features must
use `Vibe` or `SuperVibe` instead. By default this uses the AMF in-app
diff viewer; set `"diff_review_viewer": "nvim"` in
`~/.config/amf/config.json` if you want the legacy tmux/neovim popup:

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

The hook is written to the worktree's local `.claude/settings.local.json`
only — your global Claude Code settings are never modified.

### Notifications

When an agent session needs user input, AMF prefers push-based IPC
notifications and falls back to file polling if the socket is not
available. Press `i` to open the picker and jump to the session that
needs attention:

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

Claude hooks are configured in the feature's local
`.claude/settings.local.json`, Codex notifications are written into the
worktree's `.codex/config.toml`, and Opencode plugins are refreshed
into `.opencode/plugins/` automatically. Diff review is only available
through the hook-based Claude and Opencode paths.

### Agent Support

- [Claude Code](https://docs.anthropic.com/en/docs/claude-code)
  supports diff-review hooks, latest-prompt capture, and session
  resume.
- [Codex](https://github.com/openai/codex) supports dedicated feature
  sessions, notifications, and usage meters in the status bar. Codex
  does not support Vibeless diff review, so Codex features must run in
  `Vibe` or `SuperVibe`.
- [Opencode](https://opencode.ai) is supported as a first-class
  alternative agent, including injected AMF-friendly themes and local
  plugins.

## Configuration

The config file lives at `~/.config/amf/config.json` and is created
automatically with defaults on first run.

### Top-level options

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `nerd_font` | bool | `true` | Enable Nerd Font icons. Set to `false` to use ASCII fallbacks. |
| `leader_timeout_seconds` | number | `5` | Leader chord timeout in viewing mode. |
| `diff_review_viewer` | string | `"amf"` | Vibeless Claude diff-review UI: `"amf"` uses the in-app reviewer and `"nvim"` uses the legacy tmux/neovim popup. Older `"custom"` and `"legacy"` values are still accepted. |
| `theme` | string | `"default"` | AMF UI theme: `default`, `amf`, `dracula`, `nord`, or one of the Catppuccin variants. |
| `transparent_background` | bool | `false` | Render the AMF background with terminal transparency. |
| `opencode_theme` | string? | `"catppuccin-frappe"` | Theme name written to global Opencode config. |
| `zai` | object? | `null` | Optional ZAI usage limits for the status bar. |
| `extension` | object | `{}` | Global extension settings merged with repo-local `.amf/config.json`. |

### `zai` — token usage limits (optional)

Controls whether ZAI usage is shown in the status bar. Set to `null`
or omit the key entirely to disable it.

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
| `description` | string? | Secondary text shown in the session picker. |
| `command` | string? | Shell command to run when the session starts. |
| `icon` | string? | ASCII icon shown for the custom session. |
| `icon_nerd` | string? | Nerd Font icon shown when `nerd_font` is enabled. |
| `on_stop` | string? | Shell command to run when the session is stopped or removed. Runs via `sh -c` in the feature's workdir with `AMF_SESSION_ID` and `AMF_STATUS_DIR` set. Fire-and-forget (non-blocking). |
| `autolaunch` | bool? | Start this session automatically when a feature starts. |
| `pre_check` | string? | Command to run before launch; non-zero exit blocks session startup and shows the command output. |
| `window_name` | string? | tmux window name (defaults to a slug of `name`). |
| `working_dir` | path? | Working directory relative to the feature's workdir. |

`command` and `pre_check` are run via `bash -c`, so shell features
work consistently across `bash`, `zsh`, and `fish` login
environments.

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

Shell scripts executed automatically on feature lifecycle events. Each
hook can be either a plain script path or an object with `script` plus
an interactive `prompt`. The script receives the feature's working
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

Prompt-enabled form:

```json
"on_worktree_created": {
  "script": "/path/to/init-worktree.sh",
  "prompt": {
    "title": "Run workspace bootstrap?",
    "options": ["yes", "no"]
  }
}
```

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
`start_session`, `stop_session`, `delete`, `sessions`, `help`,
`search`, `refresh`, `filter`, `fork_feature`, `mark_ready`.

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
| `agent` | string | Agent to use: `"Claude"`, `"Codex"`, or `"Opencode"`. |
| `review` | bool | Whether to enable the diff-review hook. |
| `enable_chrome` | bool | Enable browser/Chrome integration. |
| `enable_notes` | bool | Enable session notes. |

#### `allowed_agents`

Restrict which agents may be used in a workspace. This can be set
globally or in a repo-local `.amf/config.json`.

```json
"allowed_agents": ["Claude", "Codex"]
```

An empty array means "allow all agents".

## Themes

### AMF UI Themes

AMF has a full built-in theme system for the dashboard and embedded
view. You can:

1. Press `T` in the dashboard to open the theme picker.
2. Set a default in `~/.config/amf/config.json`:

   ```json
   {
     "theme": "catppuccin-frappe",
     "transparent_background": true
   }
   ```

Available UI themes:

- `default`
- `amf`
- `dracula`
- `nord`
- `catppuccin-latte`
- `catppuccin-frappe`
- `catppuccin-macchiato`
- `catppuccin-mocha`

### Bundled Opencode Themes

AMF also injects Opencode themes into every worktree so embedded pane
rendering stays readable and consistent.

### Available Themes

- **amf** - Nord-based theme with transparent background
- **amf-tokyonight** - Tokyo Night with transparent background
- **amf-catppuccin** - Catppuccin Mocha with transparent background

### Using the Themes

When you start an Opencode feature, these themes are automatically
added to `.opencode/themes/` in the worktree. You can then:

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
├── codex.rs           # Codex CLI launcher
├── ipc.rs             # local IPC server/client for notifications
├── summary.rs         # feature summary generation
├── theme.rs           # AMF theme system + Opencode theme injection
├── upgrade.rs         # self-upgrade command
├── app/
│   ├── mod.rs         # App struct, config types, new/save
│   ├── state.rs       # AppMode, Selection, dialog states
│   ├── navigation.rs  # tree navigation, selection getters
│   ├── sync.rs        # status polling, thinking detection
│   ├── project_ops.rs # project CRUD, path browsing
│   ├── feature_ops.rs # feature create/start/stop/delete
│   ├── harpoon.rs     # session bookmarks
│   ├── session_ops.rs # session picker, add/remove sessions
│   ├── view.rs        # embedded tmux view, leader key
│   ├── switcher.rs    # in-view session switcher
│   ├── notifications.rs # notification scanning
│   ├── hooks.rs       # lifecycle hook execution
│   ├── opencode.rs    # opencode session management
│   ├── claude_session_picker.rs # Claude resume picker
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
├── usage.rs           # token usage tracking (Claude / Codex / ZAI)
├── traits.rs          # shared traits (TmuxOps, WorktreeOps)
├── handlers/
│   ├── mod.rs         # top-level key dispatch
│   ├── normal.rs      # dashboard normal mode
│   ├── view.rs        # embedded tmux view mode
│   ├── dialog.rs      # project/delete/rename handlers
│   ├── batch_creation.rs # batch feature creation
│   ├── feature_creation.rs # feature creation wizard
│   ├── browse.rs      # path browser
│   ├── fork.rs        # feature forking flow
│   ├── hooks.rs       # hook/delete-progress handlers
│   ├── picker.rs      # notification/session/command pickers
│   ├── search.rs      # search mode
│   ├── diff_review.rs  # diff review prompt
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
        ├── batch_creation.rs # batch feature dialog
        ├── project.rs # create/delete project dialogs
        ├── feature.rs # feature creation, forking, supervibe confirm
        ├── session.rs # rename session
        ├── help.rs    # keybindings help
        ├── debug.rs   # debug log overlay
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
- Input notifications are delivered over a local IPC socket when
  possible, with file-based fallback if the socket cannot be started
- The embedded tmux view captures ANSI output from tmux and renders it
  through a vt100 parser into ratatui spans
- The embedded tmux view renders agent output directly in the TUI
  without leaving the dashboard
- Hook files are written to the worktree's local `.claude/settings.local.json`
  only — global settings are never modified. On startup,
  `cleanup_global_hooks()` removes any previously-injected entries.

### Contributing

Contributions are welcome. The main verification loop is:

```bash
cargo test
cargo check
cargo clippy -- -W clippy::all
```

Manual TUI testing is still important for pane rendering, tmux
integration, and hook flows.

1. Fork the repo and create a feature branch.
2. Make your changes. Run `cargo test`, `cargo check`, and
   `cargo clippy -- -W clippy::all` before submitting.
3. Open a pull request with a short description of what changed and
   why.

The project uses Rust 2024 edition (rustc 1.85+).

---

*Last updated: 2026-03-07*
