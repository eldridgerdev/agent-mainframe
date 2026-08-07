# Agent Mainframe (AMF)

Run multiple AI coding agents in parallel—each on its own branch and in its
own terminal—without losing track of them.

AMF is a terminal dashboard for managing
[Claude Code](https://docs.anthropic.com/en/docs/claude-code),
[Codex](https://github.com/openai/codex),
[OpenCode](https://opencode.ai), and Pi sessions. It organizes work by project
and feature, creates isolated git worktrees when needed, and shows when an
agent is waiting for input.

<img width="1896" height="1030" alt="AMF dashboard showing several agent sessions" src="https://github.com/user-attachments/assets/d8160bc6-49ea-4b2b-839a-7ec056897ffc" />

## What AMF does

- Runs several coding-agent sessions side by side from one dashboard.
- Keeps concurrent features isolated with git branches and worktrees.
- Embeds agent terminals, shells, Neovim, VS Code, and custom sessions.
- Surfaces input requests, session status, token usage, and estimated cost.
- Supports guided planning, supervised edits, final diff review, and GitHub PR
  review workflows.
- Provides reusable prompts, project TODO lists, themes, lifecycle hooks, and
  workspace presets.
- Supports ordinary directories as well as git repositories.

## Requirements

Install at least one supported agent CLI and sign in to it before using AMF:

- [Claude Code](https://docs.anthropic.com/en/docs/claude-code)
- [Codex](https://github.com/openai/codex)
- [OpenCode](https://opencode.ai)
- Pi (`pi` must be available in `PATH`)

Release bundles include `tmux`. A source installation requires `tmux` in
`PATH`. Git is required for branch and worktree features, but AMF can manage
non-git directories without it.

Optional tools:

- An authenticated [GitHub CLI](https://cli.github.com/) (`gh`) enables PR
  triage, AI review, and posting final-review feedback.
- A [Nerd Font](https://www.nerdfonts.com/) provides the best icon rendering.
- The `code` command enables VS Code sessions.
- A C compiler and git are required only for installing optional tree-sitter
  syntax parsers.

## Installation

### Release bundle (recommended)

Download the archive for your platform from
[GitHub Releases](https://github.com/eldridgerdev/agent-mainframe/releases),
extract it, and place `amf` somewhere in your `PATH`.

| Platform | Archive |
| --- | --- |
| Linux x86_64, most portable | `amf-x86_64-unknown-linux-musl.tar.gz` |
| Linux x86_64, glibc | `amf-x86_64-unknown-linux-gnu.tar.gz` |
| Linux aarch64 | `amf-aarch64-unknown-linux-gnu.tar.gz` |
| macOS, Apple Silicon | `amf-aarch64-apple-darwin.tar.gz` |

For example, on Linux x86_64:

```bash
curl -L https://github.com/eldridgerdev/agent-mainframe/releases/latest/download/amf-x86_64-unknown-linux-musl.tar.gz -o amf.tar.gz
tar -xzf amf.tar.gz
sudo mv amf-x86_64-unknown-linux-musl /opt/amf
sudo ln -s /opt/amf/amf /usr/local/bin/amf
```

### Build from source

Building requires Rust 1.85 or newer and `tmux`:

```bash
git clone https://github.com/eldridgerdev/agent-mainframe
cd agent-mainframe
cargo install --path .
```

For a container installation, see the
[Docker guide](docs/docker-no-tmux.md).

### Upgrade

```bash
amf upgrade
amf -V
```

See [CHANGELOG.md](CHANGELOG.md) for release notes and migration guidance.

## Quick start

1. Start AMF from a normal shell or an existing tmux session:

   ```bash
   amf
   ```

2. On first launch, choose the agent CLIs you want AMF to use. AMF checks that
   each selected CLI is installed. You can reopen this setup later with `A`.

3. Press `N` to add a project, then enter its name and directory. The directory
   may be a git repository or an ordinary folder.

4. Press `n` to create a feature. Choose a branch name, agent, permission mode,
   and whether to use the guided plan interview. The feature starts when setup
   is complete.

5. Select the agent session and press `Enter` to work in its embedded terminal.
   Press `Ctrl+Q` to return to the dashboard.

6. Press `s` on a feature to add another agent, terminal, editor, TODO list, or
   custom session.

Press `?` at any time on the dashboard to see the complete, current keybinding
reference.

## Core concepts

### Projects, features, and sessions

- A **project** points to a directory that you want to work in.
- A **feature** represents one task or branch within that project.
- A **session** is an agent, terminal, editor, TODO list, or custom command
  attached to a feature.

For git projects, the first feature can use the repository directory directly.
Additional features use worktrees under `.worktrees/`, allowing their agents to
work concurrently without changing one another's files. Press `F` to fork a
feature or `B` to create several features at once.

### Permission modes

AMF asks you to choose how much autonomy an agent receives for each feature:

| Mode | Behavior |
| --- | --- |
| **Vibeless** | Shows supported file edits for approval before they are applied. Available for Claude Code and OpenCode. |
| **Vibe** | Allows edits without AMF's per-edit review gate while retaining the agent's normal permission controls. |
| **SuperVibe** | Skips agent permission prompts. AMF shows a warning before enabling it. |

Codex and Pi do not support AMF's Vibeless edit-review hooks. Pi also does not
receive AMF permission-mode flags.

### Plan mode

Plan mode interviews you about a feature before launching its agent. You can
review and edit the resulting plan, ask an agent to improve it, or cancel it.
AMF does not save the plan or launch the feature until you accept it. Press `P`
on an existing feature to run the interview again.

## Essential controls

### Dashboard

| Key | Action |
| --- | --- |
| `j` / `k` or arrow keys | Move through projects, features, and sessions |
| `h` / `l` | Collapse or expand the selected item |
| `Enter` | Open the selected session or expand/collapse an item |
| `N` / `n` | Create a project / feature |
| `s` | Add a session to a feature |
| `c` / `x` | Start / stop the selected feature or session |
| `r` / `d` | Rename / delete the selected item |
| `/` | Search and jump |
| `i` | Show agents waiting for input |
| `G` | Open GitHub PR triage |
| `W` | Run AMF's AI review of a PR diff |
| `L` | Open the prompt library |
| `T` | Choose a theme |
| `A` | Manage installed agent harnesses |
| `?` | Show all keybindings |
| `q` / `Esc` | Quit |

### Embedded session

Most keys go directly to the active session. These controls belong to AMF:

| Key | Action |
| --- | --- |
| `Ctrl+Q` | Return to the dashboard |
| `Ctrl+Space` | Open the leader-command menu |
| `Ctrl+Space`, then `w` | Switch sessions |
| `Ctrl+Space`, then `i` | Jump to an agent waiting for input |
| `Ctrl+Space`, then `f` | Start final diff review |
| `Ctrl+Space`, then `p` | Open the prompt library |
| `Ctrl+Space`, then `N` | Add a project TODO |
| `Ctrl+Space`, then `?` | Show all leader commands |

## User workflows

### Review changes before shipping

From an embedded session, press `Ctrl+Space`, then `f`. AMF presents the
feature's diff file by file and lets you attach line comments or suggested
changes. When you finish, AMF writes the feedback and hands it to an agent.
With an authenticated `gh` CLI, AMF can also post the feedback to the branch's
pull request.

### Work through pull-request feedback

Select a feature and press `G` to open PR Triage. You can inspect review
threads, send an individual or batched fix prompt to an agent, reply, and mark
threads done. Press `W` to have AMF run its own review of the PR diff. GitHub
actions are presented for confirmation before AMF writes to the pull request.

### Reuse prompts and track TODOs

Press `L` to manage reusable prompt templates, or open them from a session with
`Ctrl+Space`, then `p`. Templates may include `{{placeholder}}` fields that AMF
asks you to fill before injection.

Add a `TODOs` session with `s` to maintain a project-wide checklist. From any
session, press `Ctrl+Space`, then `N` to capture a TODO without leaving your
current work.

## Configuration

AMF creates its global configuration at `~/.config/amf/config.json`. A project
can add `.amf/config.json` at its repository root; project settings override
matching global settings.

From an embedded session, press `Ctrl+Space`, then `c` to open the configuration
wizard. It covers the common customizations, including:

- feature presets;
- custom session commands;
- lifecycle hooks;
- workspace-specific agent restrictions;
- plan interview questions; and
- key remapping.

Use `T` on the dashboard to choose an AMF theme. If your terminal does not have
a Nerd Font, enable ASCII fallbacks:

```json
{
  "nerd_font": false
}
```

AMF stores its projects, features, saved prompts, and other application state
in `~/.config/amf/amf.db`.

### Built-in customization skills

AMF injects a small set of skills into Claude Code, Codex, and OpenCode feature
workspaces so the agent can safely customize the project-local AMF config for
you. Ask the agent to use `amf:configure` to explain the current setup,
`amf:add-session` to add a dev server or other persistent command,
`amf:add-hook` to automate feature lifecycle events, `amf:add-preset` to create
a reusable feature setup, or `amf:add-prompt` to add a shared prompt template.
You can invoke a skill by name or simply describe the customization you want.

## Automation

A running AMF instance accepts structured commands for creating projects and
features without scripting the TUI:

```bash
amf automation create-project --file request.json
amf automation create-feature --file request.json
amf automation create-batch-features --file request.json
```

See the [automation guide](docs/automation/README.md) for request templates,
examples, dry runs, and response formats.

## Troubleshooting

- Press `D` on the dashboard to view AMF's debug log. The same log is available
  at `~/.local/state/amf/debug.log`.
- If icons render incorrectly, set `"nerd_font": false` in the global config.
- If an agent does not appear during feature creation, press `A` and confirm
  that its CLI is installed and enabled.
- If an old AMF installation cannot run `amf upgrade`, install the latest
  release bundle once and then use the built-in upgrader for future releases.

## License

AMF is released under the [MIT License](LICENSE).
