# Agent Mainframe (amf)

Run many AI coding agents in parallel — each on its own branch,
each in its own terminal — without losing track of any of them.

`amf` is a terminal dashboard for managing concurrent
[Claude Code](https://docs.anthropic.com/en/docs/claude-code),
[Codex](https://github.com/openai/codex),
[Opencode](https://opencode.ai), and Pi agent sessions. Each feature
gets its own tmux session and git worktree so agents work
simultaneously without conflicts. You watch them all from one place,
jump into whichever needs attention, and get notified the moment one
is waiting for input.

<img width="1896" height="1030" alt="image" src="https://github.com/user-attachments/assets/d8160bc6-49ea-4b2b-839a-7ec056897ffc" />

## Features

- **Multi-agent workspace manager** — run Claude Code, Codex,
  Opencode, and Pi features side by side; a first-run wizard picks
  which harnesses are enabled.
- **Project / feature tree** — organize work by repo, branch,
  nickname, ready state, timestamps, and one-line summaries.
- **Flexible session model** — each feature can host agent sessions,
  terminals, nvim, VSCode, and custom session types.
- **Embedded tmux view** — watch panes directly in the TUI with ANSI
  rendering, mouse text selection, and clipboard copy.
- **Worktree automation** — first feature can reuse the repo, later
  features get worktrees automatically; batch-create and fork features
  when you need parallel branches quickly.
- **Final review workflow** — review a feature's diff file by file
  with line and multi-line comments and concrete suggested changes;
  optionally let Claude draft a first pass of comments, and post the
  result to the branch's GitHub PR.
- **PR Triage** — triage GitHub PR review comments in a
  dedicated pane: reply, resolve threads, and inject scoped fix
  prompts into an agent session one at a time or in batches.
- **Per-project TODO lists** — a native full-screen checklist per
  project with a scratchpad note; spawn an agent for a TODO straight
  from the list.
- **Usage and cost meters** — live token usage and dollar cost per
  agent session, with feature-level totals on the dashboard.
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
- **On-demand syntax parsers** — tree-sitter grammars are installed
  only when you choose them from the dashboard picker.
- **Non-git projects** — projects do not require git; worktree-only
  features are simply disabled for those repos.

## Prerequisites

### Required (build from source only)

- **tmux** — pre-built release bundles include a statically-linked
  `tmux` binary, so no system install is needed when using a release.
  When building from source, `tmux` must be available in `PATH`. AMF
  auto-detects a bundled `tmux` binary placed next to `amf`. By
  default, AMF uses tmux control mode on a dedicated managed socket for
  responsive embedded-session input and rendering;
  `AMF_TMUX_BIN` and `AMF_TMUX_SOCKET` can override these at runtime.

### Agent (choose one or more)

- **Claude CLI** — required for Claude Code sessions
  ([Claude Code docs](https://docs.anthropic.com/en/docs/claude-code))
- **Codex CLI** — required for Codex sessions
  ([Codex repo](https://github.com/openai/codex))
- **Opencode** — optional alternative agent
  ([opencode.ai](https://opencode.ai))
- **Pi** — optional; required only for Pi sessions (the `pi` CLI must
  be available in `PATH`)

### Git (optional)

- **git** — required only for git projects with worktree features
  (non-git projects are supported without git)

### Optional

- **GPU-accelerated terminal** (Ghostty, Wezterm, Kitty, Alacritty) —
  highly recommended for smooth ANSI rendering in the embedded view
- **C compiler (`cc`) and git** — needed only when you install
  on-demand
  [Tree-sitter](https://tree-sitter.github.io/tree-sitter/) syntax
  parsers from the syntax picker. `cc` is the generic command name many
  Unix-like systems use for a C compiler; it is often provided by
  [Clang](https://clang.llvm.org/get_started.html) or
  [GCC](https://gcc.gnu.org/install/). Tree-sitter compiles parser code
  locally, and the existing git install is used to fetch parser
  repositories. On Linux, install your distribution's C build tools
  package such as `build-essential`, `base-devel`, or
  `Development Tools`; on macOS, install Xcode Command Line Tools with
  `xcode-select --install`. See the
  [Tree-sitter parser guide](https://tree-sitter.github.io/tree-sitter/creating-parsers/1-getting-started.html)
  and [Git installation guide](https://git-scm.com/book/en/v2/Getting-Started-Installing-Git)
  for more detail.
- **Nerd Font** — a
  [Nerd Font](https://www.nerdfonts.com/) is recommended for icon
  rendering. The app defaults to `nerd_font: true`; if your terminal
  font does not include Nerd Font glyphs, set `nerd_font: false` in
  `~/.config/amf/config.json` to use ASCII fallbacks instead.
- **VSCode CLI** — install `code` in `PATH` if you want VSCode
  sessions from the session picker.

## Installation

### Pre-built binaries (recommended)

Download the latest release bundle from the
[GitHub Releases page](https://github.com/eldridgerdev/agent-mainframe/releases).

| Platform | File |
| --- | --- |
| Linux x86_64 (musl) | `amf-x86_64-unknown-linux-musl.tar.gz` |
| Linux x86_64 (gnu) | `amf-x86_64-unknown-linux-gnu.tar.gz` |
| Linux aarch64 | `amf-aarch64-unknown-linux-gnu.tar.gz` |
| macOS (Apple Silicon) | `amf-aarch64-apple-darwin.tar.gz` |

Quick install:

Linux x86_64 (most portable):

```bash
curl -L https://github.com/eldridgerdev/agent-mainframe/releases/latest/download/amf-x86_64-unknown-linux-musl.tar.gz -o amf.tar.gz
tar -xzf amf.tar.gz
sudo rm -rf /opt/amf && sudo mv amf-x86_64-unknown-linux-musl /opt/amf
sudo ln -sf /opt/amf/amf /usr/local/bin/amf
```

macOS (Apple Silicon):

```bash
curl -L https://github.com/eldridgerdev/agent-mainframe/releases/latest/download/amf-aarch64-apple-darwin.tar.gz -o amf.tar.gz
tar -xzf amf.tar.gz
sudo rm -rf /opt/amf && sudo mv amf-aarch64-apple-darwin /opt/amf
sudo ln -sf /opt/amf/amf /usr/local/bin/amf
```

Linux x86_64 (gnu):

```bash
curl -L https://github.com/eldridgerdev/agent-mainframe/releases/latest/download/amf-x86_64-unknown-linux-gnu.tar.gz -o amf.tar.gz
tar -xzf amf.tar.gz
sudo rm -rf /opt/amf && sudo mv amf-x86_64-unknown-linux-gnu /opt/amf
sudo ln -sf /opt/amf/amf /usr/local/bin/amf
```

Linux aarch64:

```bash
curl -L https://github.com/eldridgerdev/agent-mainframe/releases/latest/download/amf-aarch64-unknown-linux-gnu.tar.gz -o amf.tar.gz
tar -xzf amf.tar.gz
sudo rm -rf /opt/amf && sudo mv amf-aarch64-unknown-linux-gnu /opt/amf
sudo ln -sf /opt/amf/amf /usr/local/bin/amf
```

### Build from source

This project requires Rust 1.85+ (2024 edition). The following
installs the toolchain (skip the first two lines if you already have
rustup), then builds and installs the `amf` binary to `~/.cargo/bin/`:

```bash
# Install the Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Clone, build, and install amf
git clone https://github.com/eldridgerdev/agent-mainframe
cd agent-mainframe
cargo install --path .
```

To build without installing to `~/.cargo/bin/`, produce a release
binary at `target/release/amf` instead:

```bash
cargo build --release
./target/release/amf
```

### Upgrade and Release Notes

Upgrade an existing install and confirm the new version:

```bash
amf upgrade   # fetch and install the latest release over HTTPS
amf -V        # print the installed version
```

> Troubleshooting: if `amf upgrade` fails on a very old install
> (pre-v0.12 TLS or `404` errors), reinstall once from the latest
> release bundle; upgrades work normally from then on.

User-facing release notes and migration guidance live in
[`CHANGELOG.md`](CHANGELOG.md).

## Docker

For a container that installs AMF in an environment without the system
`tmux` package preinstalled, see
[`docs/docker-no-tmux.md`](docs/docker-no-tmux.md).

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

1. Launch the dashboard:

   ```bash
   amf
   ```

   AMF can be started directly from a normal shell. Running inside an
   existing tmux client is still supported, but no longer required.

   On first run, AMF opens the **harness setup wizard** — a checklist
   of the supported agent harnesses (Claude, Codex, Opencode, Pi).
   Toggle the ones you use with `Enter`/`Space` (AMF verifies each CLI
   is actually installed) and press `c` to confirm. Only enabled
   harnesses appear in feature creation and the session picker; press
   `A` on the dashboard at any time to change the selection.

2. Press `N` to create a new project. Enter a name and the path to a
   git repository (or press `Ctrl+B` to browse for a directory).

3. Press `n` to add a feature. Enter a branch name, choose your agent
   (Claude, Codex, Opencode, or Pi), and pick a vibe mode. A git worktree
   is created automatically when needed, and features auto-start on
   creation. With plan mode enabled, AMF first runs a guided interview and
   lets you review or edit the proposed `.claude/plan.md`; the agent starts
   only after you accept the plan. Codex supports `Vibe` and `SuperVibe`;
   `Vibeless` is only available for agents with diff-review hook support.

<img width="1896" height="1030" alt="image" src="https://github.com/user-attachments/assets/328be46c-b8db-4150-9955-436377c03295" />


4. Press `s` to add more sessions to a running feature. The picker can
   launch agent sessions, terminals, nvim, VSCode, and custom session
   types from your config.

  <img width="1896" height="1030" alt="image" src="https://github.com/user-attachments/assets/2e493755-af11-4a14-ae8d-86353c1c0a41" />


5. Press `Enter` on a session to view the embedded tmux output.

6. Use `Ctrl+Space` then a key for leader commands while in view mode.

## Keybindings

### Dashboard (Normal Mode)

| Key | Action |
| --- | --- |
| `j` / `k` / `↑` / `↓` | Navigate project tree |
| `h` | Collapse project / go to parent |
| `l` | Expand project / feature |
| `Enter` | Toggle expand, view a running session, or recover a stopped agent session |
| `N` | Create new project |
| `n` | Create new feature |
| `B` | Batch-create features for a workspace |
| `O` | Open the `~/.config/amf` settings project |
| `A` | Manage agent harnesses (the first-run wizard) |
| `G` | Open PR Triage |
| `W` | Open AI Review (AMF's own review of this feature's PR diff) |
| `s` | Open session picker / add a session |
| `S` | Resume a saved Claude, Opencode, or Codex session |
| `r` | Rename selected feature or session |
| `d` | Delete selected project, feature, or session |
| `c` | Start selected feature |
| `x` | Stop selected feature or remove selected session |
| `u` | Configure preferred harness (project) or session settings (feature/session) |
| `V` | Check pending diff review |
| `F` | Fork the selected feature into a new worktree |
| `f` | Filter by session type |
| `y` | Toggle ready state for the selected feature |
| `Z` | Generate a one-line summary for the selected feature |
| `T` | Open the theme picker |
| `p` | Open the syntax parser picker |
| `L` | Open the prompt library |
| `i` | Input requests picker |
| `/` | Search and jump |
| `D` | Open the debug log overlay |
| `R` | Refresh statuses |
| `Ctrl+Space` `c` | Open the config wizard |
| `?` | Toggle help |
| `q` / `Esc` | Quit |

### Plan Interview

When plan mode is enabled during feature creation, AMF collects a required
feature brief followed by optional built-in discovery questions before the
agent launches. After those questions, AMF offers two distinct AI actions:

- **Ask AI follow-up questions (`a`)** generates another round of questions
  tailored to the answers and repository. You answer those questions before
  AMF drafts the plan.
- **Draft the plan now (`Ctrl+F`)** stops the interview immediately, skips
  every remaining built-in and AI follow-up question, and drafts the plan from
  the answers already collected.

Both actions use agent tokens. Pressing `Enter` or `Ctrl+S` at the optional AI
prompt uses no agent tokens and opens a raw interview plan for review instead.
An unavailable or invalid AI draft also falls back to that raw plan. Every
completed flow stops at a rendered review gate; AMF does not write the plan or
launch the feature until you explicitly accept it.

Press `P` on a feature (or run the `plan-interview` command) to run the
interview again later. A re-run starts from the answers behind the plan you
last accepted: each question arrives pre-filled, `Enter` keeps that answer,
typing changes it, and `Ctrl+R` restores it if you change your mind. A
multiple-choice answer is pre-filled only while it is still one of the
question's options, so editing `plan_questions` between runs drops the stale
choice instead of carrying it into the new plan. The
previous run's AI follow-up questions are asked again with their answers, but
adaptive rounds are not: a re-run gets its own opt-in and its own round budget.

| Key | Action |
| --- | --- |
| `Enter` | Save the current answer or selection; at the optional AI prompt, skip AI and review the raw interview plan |
| `a` | At the optional AI prompt, use AI to generate more interview questions; answer them before AMF drafts the plan |
| `Alt+Enter` | Insert a newline in a free-text answer |
| `j` / `k` / `↑` / `↓` | Navigate select-option answers |
| `Ctrl+B` | Return to the previous question |
| `Ctrl+S` | Skip an optional question; at the optional AI prompt, skip AI and review the raw interview plan |
| `Ctrl+R` | On a re-run, restore the previous interview's answer to this question |
| `Ctrl+F` | Stop asking questions and use AI to draft the plan now from the answers already collected |
| `Esc` | Cancel, then choose whether to launch without a plan or cancel the feature |

At the plan review gate:

| Key | Action |
| --- | --- |
| `j` / `k` / `PgUp` / `PgDn` | Scroll the rendered plan |
| `e` | Edit the raw plan markdown |
| `r` | Regenerate the plan (uses agent tokens when a headless harness is available) |
| `Enter` | Accept the reviewed plan, write it, and launch the feature |
| `Ctrl+S` | Save a raw-markdown edit and return to the rendered preview |
| `Esc` | Discard a raw-markdown edit, or open abort confirmation from the preview |

### Viewing Mode (Embedded tmux)

All keys are forwarded to the tmux session except:

| Key | Action |
| --- | --- |
| `Ctrl+Q` | Exit view, return to dashboard |
| `Ctrl+Space` | Activate leader key (default 5s window, configurable) |
| any text key | Open the compose box (agent sessions, when compose input is on) |

### Leader Commands (after Ctrl+Space)

| Key | Action |
| --- | --- |
| `q` | Exit view |
| `t` / `T` | Cycle between sessions |
| `w` | Open session switcher |
| `h` | Open bookmark picker popup |
| `/` | Command palette |
| `a` | Command palette focused on AMF local actions |
| `i` | Input requests picker |
| `e` | Toggle compose/direct input (agent sessions) |
| `s` | Steering coach (experimental) |
| `d` | Choose all current changes or one commit to view |
| `m` | Markdown file picker/viewer |
| `b` | Show/hide the sidebar |
| `v` | Expand/collapse todos in the sidebar |
| `N` | Quick-capture a TODO for this project |
| `g` | Generate a session summary |
| `r` | Refresh statuses |
| `R` | Refresh pane sizing |
| `V` | Check pending diff review |
| `x` | Stop session and exit view |
| `f` | Trigger final review |
| `l` | Show the latest saved prompt |
| `p` | Open the prompt library (inject a saved prompt) — same as `Ctrl+P` in the compose box |
| `o` / `S` | Toggle pane scroll mode |
| `A` | Manage agent harnesses |
| `D` | Open debug log |
| `H` / `M` | Bookmark / unbookmark current session |
| `1`-`9` | Jump to bookmark slot |
| `?` | Show help |

> **Next / previous feature** navigation has no default key. Bind the
> `next_feature` and `prev_feature` actions in your config (or via the config
> wizard) to enable them as leader commands; they only appear in the leader
> menu once bound.

### Command Palette

The leader command palette (`Ctrl+Space` then `/`) now mixes two kinds of entries:

- slash commands discovered from global or project `.claude/commands`
- AMF-owned local actions such as debug/dev/test helpers

Inside the palette:

- `Enter` executes the selected entry
- `D` jumps between the AMF local sections
- local actions are rendered with an `amf` badge instead of a `/` prefix

Use `Ctrl+Space` then `a` to open the palette already focused on AMF local actions.
From the debug log overlay, press `/` to jump straight into that local-actions view.

## How It Works

### Data Model

```text
ProjectStore (version: 5, session_bookmarks)
  └─ Project (name, repo path, is_git)
       └─ Feature (branch, nickname?, workdir, tmux session,
                   status, ready, mode, review, agent,
                   summary?)
            └─ FeatureSession (kind: Claude|Opencode|Codex|Pi|
                               Terminal|Nvim|VSCode|Custom|Todos,
                               label, tmux window)
```

State is persisted in the SQLite database at `~/.config/amf/amf.db`
for every checkout. AMF resolves this single global store regardless of
the directory you launch it from.

### Tmux Sessions

Each feature gets a tmux session named `amf-<branch>`. Features start
with an agent session plus a terminal session.

Use `s` to open the session picker and add more sessions at any time:

| Picker entry | Session type |
| --- | --- |
| `Claude` / `Opencode` / `Codex` / `Pi` | Another agent session in the same feature |
| `Terminal` | Plain shell terminal |
| `Nvim` | Neovim in the feature's working directory |
| `VSCode` | Open the feature workdir in VSCode via `code` |
| `TODOs` | The project's TODO list (native view, no tmux pane) |
| Custom entries | Commands defined in `extension.custom_sessions` |

Sessions can be renamed with `r` when a feature or session item is
selected. `S` can pick a saved Claude, Opencode, or Codex session to
resume for the selected feature.

Press `Enter` to enter the embedded view, which streams the tmux pane
output live through a vt100 parser and renders it with full ANSI color:

If an agent session's tmux process disappeared behind your back — a
crash, a reboot, an external `tmux kill-server` — and AMF still holds
its saved harness ID, `Enter` opens a recovery dialog: resume the saved
session, start a clear one, pick a different saved session (the `S`
picker), or cancel. Everything else keeps its usual one-keypress start:
a feature you stopped yourself with `x`, a feature that was never
started, terminal panes, and harnesses without resume support.

This view includes a custom header and amf commands accesible with a leader key (ctrl+space default)

<img width="1263" height="376" alt="image" src="https://github.com/user-attachments/assets/2aa2998e-2bce-4e06-99d6-f5400c2d1bc1" />
<img width="1896" height="1030" alt="image" src="https://github.com/user-attachments/assets/83a84c62-cae1-4893-91d5-d26c8b6ab797" />


#### Session Picker

While viewing a feature, press `leader w` to open the session picker — a
popup listing all sessions for the current feature.

<img width="1896" height="1030" alt="image" src="https://github.com/user-attachments/assets/19387489-18f4-45f0-8391-fb34ece257d8" />

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
| **SuperVibe** | Skips permission prompts. Claude uses `--dangerously-skip-permissions`; Codex uses `--sandbox danger-full-access --ask-for-approval never`. Shows a confirmation warning before creation. |

### Diff-Review Hook

In Vibeless mode, `amf` installs a Claude Code hook in the feature's
`.claude/settings.local.json`. The hook intercepts every `Edit`, `Write`,
and `MultiEdit` tool call before it executes and shows you the diff.
Codex worktrees do not support this hook path, so Codex features must
use `Vibe` or `SuperVibe` instead. By default this uses the AMF in-app
diff viewer; set `"diff_review_viewer": "nvim"` in
`~/.config/amf/config.json` if you want the legacy tmux/neovim popup:

<img width="1896" height="1030" alt="image" src="https://github.com/user-attachments/assets/f41435e0-b607-425f-aa5e-bfaa3f944c08" />

The hook is written to the worktree's local `.claude/settings.local.json`
only — your global Claude Code settings are never modified.

### Notifications

When an agent session needs user input, AMF prefers push-based IPC
notifications and falls back to file polling if the socket is not
available. Press `<leader> i` to open the picker and jump to the session that
needs attention:

<img width="888" height="23" alt="image" src="https://github.com/user-attachments/assets/5609d2d8-7eab-4423-8750-d8d542a095fa" />
<img width="1896" height="1030" alt="image" src="https://github.com/user-attachments/assets/ea52a5cd-a0cc-432f-bdb2-3c97be99d136" />


Claude hooks are configured in the feature's local
`.claude/settings.local.json`, Codex notifications are written into the
worktree's `.codex/config.toml`, and Opencode plugins are refreshed
into `.opencode/plugins/` automatically. Diff review is only available
through the hook-based Claude and Opencode paths.

### Final Review

Press `Ctrl+Space` then `f` while viewing a session to start a final
review — a file-by-file pass over the feature's diff before you ship
it:

- Approve, reject, or skip each file. With the line cursor active
  (`c`) you can attach a comment to a line or to a `v`-selected range,
  and `]` / `[` jump between hunks.
- Press `S` on a line to write a **suggested change** — a concrete
  replacement, recorded as a fenced `suggestion` block the agent can
  apply verbatim (and posted as a one-click GitHub suggestion when PR
  posting is on).
- Press `A` to have Claude draft a first pass of line comments for the
  current file; accept (`a`), dismiss (`d`), or edit (`Enter`) each
  draft. Dismissed drafts never reach the feedback.
- Finishing writes `.claude/final-review-feedback.md` and hands the
  feedback to the feature's agent. Press `t` to send fixes to a
  dedicated review session (you pick its harness) instead of the live
  one.
- With `final_review_post_to_pr` enabled, finishing also posts the
  feedback to the branch's GitHub PR as a review (needs an
  authenticated `gh` CLI).
- Re-reviewing the same feature marks files changed since your last
  pass with `Δ` and narrows the file list to just those files.

### PR Triage

Press `G` on a feature to triage its GitHub pull-request comments
inside AMF (requires an authenticated `gh` CLI). If the branch has no
detectable PR, a picker lists the repo's pull requests.

In the PR Triage pane:

- `j`/`k` navigate comments; bodies render as Markdown and diff hunks
  are colored and syntax-highlighted (press `i` to install a missing
  language parser without leaving the pane).
- `f` injects a scoped fix prompt for the selected comment into an
  agent session — the first fix/batch of a visit asks which session
  fixes should target: the feature's existing live session, a
  dedicated triage session on a chosen harness, or **New feature…**.
  Mark several comments with `Space`, then `B` opens one combined,
  editable prompt for all of them.
- **New feature…** creates an isolated, worktree-backed AMF feature
  just for this PR's triage, with its own harness and vibe mode chosen
  in one compact form (or from a feature preset) — so you can build a
  PR in SuperVibe and apply review fixes under Vibeless supervision,
  without the triage agent writing hooks or permissions into the
  source worktree. It's seeded from the PR head onto a companion
  branch (git can't check the PR's own branch out twice) and is reused
  for every fix in that PR, across restarts.
- `I` lands the triage feature's commits on the PR: push the companion
  branch onto the PR branch, or cherry-pick into the source worktree.
  Both are explicit and non-destructive — the push is never forced,
  and the cherry-pick refuses to run while the source worktree is
  dirty.
- `R` opens a reply picker: "Done" replies "Done in `<sha>`", seeded
  from the most recent commit that touched the comment's file/line
  (falling back to your latest commit, flagged "latest commit", when
  no matching history is found); "not needed" replies why a fix isn't
  needed.
- `m` opens a "Mark" picker: mark the comment `Done` or `Skip`
  locally, or resolve/reopen its GitHub thread (the one option here
  that writes to GitHub — labeled as such). Triage state is stored in
  SQLite and survives restarts, re-opens, and new pushes.
- `M` adds the comment as a recurring finding to a **review-memory**
  doc, so the next AI review checks for it instead of rediscovering
  it. There are two: this repo's committed `.amf/review-memory.md`
  (the default) and a cross-project `~/.config/amf/review-memory.md`
  for lessons that follow you between repos — `g` picks which, and the
  dialog names the exact file before you commit to it. In the PR
  picker, `b` bootstraps a doc from recent merged PRs (`g` picks the
  doc there too) and `c` compacts the project one, merging duplicates
  and pruning stale rules behind a review-and-confirm step.
- `A` opens the AI Review pane for this same PR (see below).

### AI Review

Press `W` on a feature (or `A` from inside PR Triage, or `W` in the PR
picker, or leader `W` from inside an agent session) to have AMF
generate its own review of a PR's diff — a separate workflow from PR
Triage, since it's reviewing the diff itself rather than triaging what
other reviewers already said.

- `A` fetches the diff and runs one headless agent pass (the first run
  asks which harness/model to use); findings are cached by the PR's
  head SHA, so reopening the pane after a same-SHA visit is instant and
  spends no tokens. Both review-memory docs feed the pass as context —
  this repo's first, then your cross-project one, with anything the
  repo doc already says dropped from the second so it isn't sent twice.
- `j`/`k` navigate findings; `s` skips a finding (excluding it from a
  post without discarding it) and `e` edits its body.
- `W` posts every kept finding to GitHub as a single review (an
  editable summary plus inline comments where the finding's line
  matched the diff). Once posted, follow up on a finding — mark done,
  reply, resolve, inject a fix — back in PR Triage, where a refresh
  picks it up as an ordinary fetched comment.

### Per-Project TODO Lists

Add a `TODOs` session from the session picker (`s`) to keep one
running checklist per project, shared across all its features. It
opens as a native full-screen list (no tmux pane) with a free-form
scratchpad note at the top.

- `a` add, `e` edit the title, `o` edit longer notes, `Space` toggle
  done, `p` cycle priority, `J`/`K` reorder, `d` delete, `b` edit the
  scratchpad note.
- Press `g` (or `Enter`) on an item to spawn an agent for it: AMF
  opens a session in the list's feature and pre-fills the composer
  from the item's title and notes. Pressing `g` again on a launched
  item jumps back to the same session.
- Quick-capture from anywhere: while viewing any session,
  `Ctrl+Space` then `N` appends a TODO to the project's list.

### Usage and Cost Meters

Agent sessions show live token usage and estimated dollar cost on the
dashboard — on each agent session row and its sidebar, plus a
feature-level total on the feature row. Costs are computed from the
`token_pricing` section of `~/.config/amf/config.json` (defaults to
Claude Sonnet pricing); set `"show_cost": false` there to hide the
dollar column. Terminal, editor, custom, and Pi sessions are excluded
from feature totals.

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
- Pi is supported as a fourth harness for dedicated agent sessions,
  with the same embedded view and composer. Pi sessions launch the
  plain `pi` CLI: vibe-mode permission flags, diff-review hooks, and
  usage meters do not apply to Pi.

## Configuration

The config file lives at `~/.config/amf/config.json` and is created
automatically with defaults on first run.

### Top-level options

| Key | Type | Default | Description |
| --- | --- | --- | --- |
| `nerd_font` | bool | `true` | Enable Nerd Font icons. Set to `false` to use ASCII fallbacks. |
| `leader_timeout_seconds` | number | `5` | Leader chord timeout in viewing mode. |
| `input_request_wait_seconds` | number | `1.5` | Startup grace period before AMF refreshes background input-request state while viewing a session. Increase it if startup refreshes interfere with the embedded pane; decrease it for faster pending-input updates. |
| `tmux_control_mode` | bool | `true` | Use tmux control mode with AMF's dedicated managed tmux socket for responsive embedded input/rendering. Set to `false` to use the legacy ambient tmux socket and direct `tmux send-keys` fallback path. |
| `max_agent_autostart_sessions` | number | `1` | Maximum saved agent harness panes AMF auto-launches when starting a stopped feature. Extra saved agent panes are recreated as tmux windows but left at the shell prompt. Set to `0` for unlimited auto-start. |
| `diff_review_viewer` | string | `"amf"` | Vibeless Claude diff-review UI. Only the in-app reviewer is supported; the value is retained for compatibility (the legacy `"nvim"`/`"legacy"`/`"custom"` values are accepted and map to the in-app reviewer). |
| `final_review_submit_prompt` | bool | `true` | When finishing a final review with feedback, auto-submit the "address the feedback" prompt to the feature's agent (paste + Enter). Set to `false` to paste the prompt without sending Enter, so you can eyeball or edit it before submitting. |
| `final_review_post_to_pr` | bool | `false` | When finishing a final review, also post the feedback to the branch's GitHub PR as a review (line comments inline, rejections and general feedback in the summary). Best-effort; requires an authenticated `gh` CLI. |
| `view_auto_refresh` | bool | `false` | Periodically repair embedded tmux views while idle. Can hide agent rendering glitches, but costs extra refresh work; manual refresh and event-driven updates always work. |
| `token_pricing` | object | Claude Sonnet pricing | Per-million-token prices used by the usage/cost meters (`input_per_mtok`, `output_per_mtok`, `reasoning_per_mtok`, `cache_read_per_mtok`, `cache_write_per_mtok`), plus `show_cost` (default `true`) to show or hide the dollar column. |
| `remote_control_default` | bool | `false` | Default state of the Remote Control toggle for new Claude features (still subject to provider availability). |
| `theme` | string | `"default"` | AMF UI theme: `default`, `amf`, `dracula`, `nord`, Gruvbox, Gruvbox Material, or Catppuccin variants. |
| `transparent_background` | bool | `false` | Render the AMF background with terminal transparency. |
| `opencode_theme` | string? | `"catppuccin-frappe"` | Theme name written to global Opencode config. |
| `extension` | object | `{}` | Global extension settings merged with repo-local `.amf/config.json`. |

To customize input-request startup waiting, edit
`~/.config/amf/config.json`:

```json
{
  "input_request_wait_seconds": 1.5
}
```

### `extension` — customizations

The `extension` block can be set globally in
`~/.config/amf/config.json` or per-project in `.amf/config.json` at
the repo root. Project-level settings are merged on top of global ones.
Press `Ctrl+Space`, then `c`, from a feature view to edit the supported
extension settings with AMF's config wizard, including plan questions.

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
| `icon_nerd` | string? | Nerd Font glyph shown when `nerd_font` is enabled. The config wizard includes a visual chooser; bundled names such as `nf-md-server` are also accepted. |
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

<img width="521" height="108" alt="image" src="https://github.com/user-attachments/assets/06a40ecc-a7f5-4eb4-a3ce-afeb520abeae" />


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
`search`, `refresh`, `filter`, `syntax_picker`, `pr_review`,
`session_config`, `fork_feature`, `mark_ready`.

Two more actions have no default key and are active only once you
bind them: `next_feature` and `prev_feature` (leader-mode next /
previous feature navigation).

#### `feature_presets`

Presets appear as quick-select options when creating a new feature,
pre-filling the vibe mode, agent, and other settings.

```json
"feature_presets": [
  {
    "name": "Quick fix",
    "branch_prefix": "fix/",
    "mode": "vibe",
    "agent": "claude",
    "review": false,
    "plan_mode": false,
    "enable_chrome": false,
    "remote_control": false
  }
]
```

| Key | Type | Description |
| --- | --- | --- |
| `name` | string | Preset label shown during feature creation. |
| `branch_prefix` | string? | Prepended to the branch name automatically. |
| `mode` | string | Vibe mode: `"vibeless"`, `"vibe"`, or `"supervibe"`. |
| `agent` | string | Agent to use: `"claude"`, `"codex"`, `"opencode"`, or `"pi"`. |
| `review` | bool | Whether to enable the diff-review hook. |
| `plan_mode` | bool | Run the guided plan interview before starting the agent. |
| `enable_chrome` | bool | Enable browser/Chrome integration. |
| `remote_control` | bool | Enable Remote Control for the feature (Claude only, subject to availability). |

#### `allowed_agents`

Restrict which agents may be used in a workspace. This can be set
globally or in a repo-local `.amf/config.json`.

```json
"allowed_agents": ["claude", "codex"]
```

Valid values are `"claude"`, `"codex"`, `"opencode"`, and `"pi"`
(lowercase).

An empty array means "allow all agents".

#### `plan_questions`

Add questions to the guided interview that runs before a plan-mode
feature starts. Questions configured globally are followed by
project questions; a project question whose trimmed `id` matches a
global question replaces it. Set `skip_builtin_questions` to `true`
to use only configured questions.

```json
"plan_questions": [
  {
    "id": "ui-surface",
    "text": "Where should this feature appear?",
    "options": ["Dashboard", "Session view", "Both"],
    "optional": true
  },
  {
    "id": "constraints",
    "text": "What constraints must the implementation preserve?",
    "optional": false
  }
],
"skip_builtin_questions": false
```

| Key | Type | Description |
| --- | --- | --- |
| `id` | string | Stable merge key. IDs must be non-empty; project entries override global entries with the same trimmed ID. |
| `text` | string | Question shown in the plan interview. |
| `options` | string[] | Optional choices. Omit or leave empty for a free-text answer. |
| `optional` | bool | Whether the user may skip the question. Defaults to `true`. |

For the global file, place these keys inside `extension`. In a
project's `.amf/config.json`, place them at the top level. The config
wizard's **Plan Questions** category supports both scopes; press `b`
in its question list to include or exclude AMF's built-in question
bank.

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
- `gruvbox-dark`
- `gruvbox-light`
- `gruvbox-material-dark-hard`
- `gruvbox-material-dark-medium`
- `gruvbox-material-dark-soft`
- `gruvbox-material-light-hard`
- `gruvbox-material-light-medium`
- `gruvbox-material-light-soft`
- `gruvbox-material-mix-dark-hard`
- `gruvbox-material-mix-dark-medium`
- `gruvbox-material-mix-dark-soft`
- `gruvbox-material-mix-light-hard`
- `gruvbox-material-mix-light-medium`
- `gruvbox-material-mix-light-soft`
- `gruvbox-material-original-dark-hard`
- `gruvbox-material-original-dark-medium`
- `gruvbox-material-original-dark-soft`
- `gruvbox-material-original-light-hard`
- `gruvbox-material-original-light-medium`
- `gruvbox-material-original-light-soft`

### Syntax Parsers

AMF can install tree-sitter parsers on demand instead of shipping every
language parser in the binary.

The curated picker currently includes Bash, C, C++, CSS, Go, HTML, Java,
JavaScript, JSON, Markdown, Python, Rust, TOML, TSX, TypeScript, and YAML.

1. Press `p` in the dashboard to open the syntax parser picker.
2. Use `Enter` or `i` to install or reinstall the selected parser.
3. Use `x` to remove a parser you no longer need.

Installed parsers are stored under `~/.config/amf/tree-sitter/`.

### Prompt Library

Save reusable prompts once and inject them into a session on demand
instead of retyping them.

1. Open the library with `leader+p` while viewing a session, or press
   `L` from the dashboard to manage entries.
2. Press `Enter` to inject the selected prompt. When compose is on it
   seeds the compose box so you can review and edit before sending; when
   compose is off it pastes into the agent window **without** sending.
3. Manage entries from the picker: `n` new, `e` edit, `d` delete, `y`
   duplicate, `/` search.
4. Export a saved prompt to declarative config with `x`, then choose
   `g` for the global config (`~/.config/amf/config.json`) or `p` for
   the project config (`{repo}/.amf/config.json`). Exported templates
   are version-controllable and shareable; an entry with the same name
   is replaced in place.
5. While composing, press `Ctrl+P` to open the library and inject a
   saved prompt into the box (mirrors `leader+p`), or `Ctrl+S` to save
   the current compose buffer as a new template.

Templates are stored in the AMF SQLite database
(`~/.config/amf/amf.db`). Templates can carry fill-in
`{{placeholder}}` slots — AMF prompts for the values on injection —
and exported declarative templates can be version-controlled and
shared with a team.

### Bundled Opencode Themes

AMF also injects Opencode themes into every worktree so embedded pane
rendering stays readable and consistent.

### Available Themes

- **amf** - Nord-based theme with transparent background
- **amf-tokyonight** - Tokyo Night with transparent background
- **amf-catppuccin** - Catppuccin Mocha with transparent background
- **amf-gruvbox** - Gruvbox with transparent background

### Using the Themes

When you start an Opencode feature, these themes are automatically
added to `.opencode/themes/` in the worktree. You can then:

1. Use the `/theme` command in opencode to select a theme
2. Edit `.opencode/tui.json` in the worktree to set your preferred theme

The themes are embedded in the AMF binary, so they're always available without any external dependencies.

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

For a module-by-module map of `src/`, see
[`CLAUDE.md`](CLAUDE.md). The "Architecture Notes" and key design
patterns below cover the parts worth reading before the source.

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
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

CI enforces a warning-free `clippy` and clean formatting, so run both
before pushing. Manual TUI testing is still important for pane
rendering, tmux integration, and hook flows.

1. Fork the repo and create a feature branch.
2. Make your changes. Run `cargo test`, `cargo check`,
   `cargo clippy --all-targets -- -D warnings`, and
   `cargo fmt --check` before submitting.
3. Open a pull request with a short description of what changed and
   why.

The project uses Rust 2024 edition (rustc 1.85+).

## License

AMF is released under the [MIT License](LICENSE).

---

*Last updated: 2026-07-08*
