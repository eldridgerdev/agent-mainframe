# Agent Mainframe (AMF)

Run multiple AI coding agents in parallel—each on its own branch and in its
own terminal—without losing track of them.

AMF is a terminal dashboard for managing
[Claude Code](https://docs.anthropic.com/en/docs/claude-code),
[Codex](https://github.com/openai/codex),
[OpenCode](https://opencode.ai), and Pi sessions. It organizes work by project
and feature, creates isolated git worktrees when needed, and shows which
agents are asking a question and which have finished work waiting to be
reviewed.

<img width="1896" height="1030" alt="AMF dashboard showing several agent sessions" src="https://github.com/user-attachments/assets/d8160bc6-49ea-4b2b-839a-7ec056897ffc" />

## What AMF does

- Runs several coding-agent sessions side by side from one dashboard.
- Keeps concurrent features isolated with git branches and worktrees.
- Embeds agent terminals, shells, Neovim, VS Code, and custom sessions.
- Surfaces which agents need attention, session status, token usage, and
  estimated cost.
- Supports guided planning, supervised edits, final diff review, and GitHub PR
  review workflows.
- Explains code you didn't write: browse a repository read-only and ask an
  agent about any file, hunk, or line range, with the answers kept per project.
- Provides reusable prompts, scoped TODO lists, themes, lifecycle hooks, and
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
  triage, AI review, posting final-review feedback, and the dashboard's
  open/merged/closed PR badge.
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

Multiple-choice questions in the interview also take your own answer: press `e`
to type into the "Your own answer" box under the options. You can answer with
your own text alone or add it alongside a picked option; `Enter` in the box
returns to the options without submitting, and `Enter` on the option list
submits.

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
| `i` | Show agents needing attention: questions first, then finished work |
| `I` | On a TODOs session row: start an agent on the next TODO in priority order, across the lists currently showing |
| `z` | Show dormant features: idle and unattended |
| `G` | Open GitHub PR triage |
| `W` | Run AMF's AI review of a PR diff |
| `K` | Open Learning Mode: read the code and ask about it |
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
| `Ctrl+Space`, then `i` | Jump to an agent needing attention |
| `Ctrl+Space`, then `f` | Start final diff review |
| `Ctrl+Space`, then `p` | Open the prompt library |
| `Ctrl+Space`, then `N` | Add a TODO to this worktree's list |
| `Ctrl+Space`, then `?` | Show all leader commands |

## User workflows

### Understand a codebase you didn't write

Select a feature and press `K` to open Learning Mode. It is a read-only reader
for the project's code with an agent attached: browse the files, point at the
part you don't understand, and ask about it. **Nothing in Learning Mode changes
your files.** The only key that can lead to an edit is `S`, which hands the
answer to an ordinary agent session and says so.

If you don't know where to start, you don't have to. On a project you have not
asked anything about yet, the file list opens on a pinned **Start here** group:
a ready-made question asking for a tour of the whole project, with the cursor
already on it, followed by whichever of the README, the contribution guide, the
entry point, and the manifest that project actually has. Press `t` at any time
for starter questions ("Explain this line by line", "What would break if I
deleted this?") that load into the prompt so you can edit them before asking.

Below that group the project's files are a **folder tree**, so the list shows
you the layout rather than a wall of paths. Folders open at the top level with
the way down to the Start here files already unfolded, and each closed folder
says how many files are inside it, so you can skip whole areas you have no
business in yet.

| Key | Moves you |
| --- | --- |
| `l` | Open the folder under the cursor, or step into an open one |
| `h` | Close the folder, or step out to the one above |
| `Z` | Open every folder in the project, or fold them all back |
| `Enter` | The same, on a folder — and opens the file, on a file |

Resting on a folder changes nothing about the question you have lined up;
folders are only for finding files.

Point at what you want to ask about with `f` (the whole file), `v` (start a line
range), `P` (the whole project), or `x` (the change under the cursor, in
branch-changes scope). Then ask one of two ways:

| Key | Asks for |
| --- | --- |
| `e` | *Explain this to me* — a teaching answer; no change is proposed |
| `c` | *Ask for a change* — a concrete proposal you can act on later |

Asking never blocks: keep browsing and asking while earlier answers arrive, and
the header counts what is still generating. Answers are written for a
**newcomer** by default — terms defined on first use, ending with what to read
next — and `L` switches to **familiar** for denser answers. `s` switches between
all files in the project and only the files changed on this branch. `m` picks
which agent answers, and is already set to one that works.

Once an answer is on screen, five keys act on it:

| Key | Action |
| --- | --- |
| `F` | Ask a follow-up; the agent keeps the question and answer you just read |
| `D` | Ask again with the repository readable — slower, but it checks (except on Codex, see below) |
| `i` | Re-file the entry as the other kind; the answer text is left alone |
| `a` | Keep the answer as a to-do on this feature's TODO list |
| `S` | Hand it to a live agent session, with the prompt filled in and unsent |

`D` is worth knowing about: an ordinary answer only sees the code on screen, so
it can name files or line numbers that don't exist. `D` re-asks the same
question with the repository open and keeps both answers so you can read them
against each other.

Codex is the exception: it has no way to answer without reading the repository,
so every Codex answer already read it — the row says *read the repo* from the
start, and `D` tells you there is nothing deeper to ask for and points you at
`F` instead. If you want the on-screen-only answer and the repository-checked
answer side by side, ask with an agent that offers both (`m`).

Questions and answers are kept per project, so reopening `K` brings back what
you asked before, with follow-ups still under the question they continue. A
question remembers the lines it was about, and the code moves on — so on the way
in AMF checks each one against the project as it is now. An entry whose code has
shifted is marked **moved** and the answer tells you where it went; one whose
code is gone is marked **anchor lost**. Either way the question and the answer
are kept exactly as they were: the point of the mark is that a stale line number
should not read as an answer that was always wrong. Press `?` inside Learning
Mode for the full key list; it opens by itself the first time you enter a
project.

### Review changes before shipping

From an embedded session, press `Ctrl+Space`, then `f`. AMF presents the
feature's diff file by file and lets you attach line comments or suggested
changes. When you finish, AMF writes the feedback and hands it to an agent.
With an authenticated `gh` CLI, AMF can also post the feedback to the branch's
pull request.

### Work through pull-request feedback

With an authenticated `gh` and a GitHub remote, a feature whose branch has a
pull request shows a `[PR #N · M open]` badge on its dashboard row and in an
embedded session's header, refreshed every few minutes in the background.
Once that PR is merged or closed without merging, the badge switches to
`[PR #N merged]` / `[PR #N closed]` instead of disappearing, so a finished
feature stays visually distinct from one that never had a PR. A branch
without `gh`, without a GitHub remote, or without any PR shows no badge at
all.

Select a feature and press `G` to open PR Triage. You can inspect review
threads, send an individual or batched fix prompt to an agent, reply, and mark
threads done. Press `W` to have AMF run its own review of the PR diff. GitHub
actions are presented for confirmation before AMF writes to the pull request.
When `f` or `B` targets a dedicated session, AMF lets you name it so multiple
PR Triage agents can run simultaneously; leave the name blank to reuse the
default `PR Triage` session.

A comment that asks a question rather than requesting a change can be
investigated instead of fixed: press `v` to run a strictly read-only headless
pass on the selected comment — it inspects the repo but changes nothing — and
the overlay blocks until it returns. Its answer persists per PR and reopens in
the detail panel. `f` and `B` still fix and batch a comment normally whether or
not it has an investigation — and when it does, the investigation's findings are
appended to the fix prompt as a starting point. Press `a` on a finished
investigation to act on it:
post an editable reply, ask a follow-up (re-runs read-only with the prior answer
as context), dismiss it, or keep it as a TODO.
An unchanged AI-drafted reply discloses the harness, best-effort model,
estimated tokens, and estimated cost of the session that wrote it. AMF's own
AI review (`W`) carries the same attribution: once a run finishes, the pane
shows which harness and model produced it and the run's token usage and
estimated cost, and that line is included above the `— AI review via AMF`
marker on the posted summary and every inline comment. A harness that reports
no usage degrades to model-only attribution rather than showing a fake `$0.00`.

To seed review memory from earlier reviews, open the PR picker and press `b`.
If `G` opened the feature's pull request directly, press `g` from PR Triage to
return to the picker first. Choose a lookback of 20, 50, 100, or all recent
closed and merged pull requests with `j`/`k`, then press `Enter`. The default is
50. Press `g` in the lookback dialog to switch between the project's
`.amf/review-memory.md` and the cross-project memory at
`~/.config/amf/review-memory.md`; project memory is the default. This requires
authenticated `gh` and an available Claude CLI. Fetching the review history
does not use agent tokens; AMF then makes one agent pass to distill recurring
findings and appends only new ones. You can press `Esc` while it runs to return
to the picker without cancelling the background job.

### Reuse prompts and track TODOs

Press `L` to manage reusable prompt templates, or open them from a session with
`Ctrl+Space`, then `p`. Templates may include `{{placeholder}}` fields that AMF
asks you to fill before injection.

Add a `TODOs` session with `s` to open a checklist — one per feature. The
editor shows up to three lists side by side:

| List | What it holds |
| --- | --- |
| **Worktree** | Work belonging to this feature's own checkout. Features on the repo root have no worktree list. |
| **Project** | Work belonging to the project as a whole, whichever checkout you are in. |
| **Global** | Work belonging to no project at all, shared across every repo AMF knows about. |

All three scopes start visible. Press `p` to hide or show the project list and
`g` to hide or show the global list independently; the worktree list is always
visible when the feature has one. Hidden scopes stay represented by compact
labeled placeholders. The choice is shared by every TODO view for the rest of
the current AMF run and resets the next time AMF starts. `Tab` / `Shift+Tab`
move between visible lists; each list keeps its own cursor, scroll, and
scratchpad while hidden. `M` moves the selected TODO to another visible list
and `C` copies it — a move carries whatever was already started for the item,
while a copy lands as fresh, unstarted work.

From any session, press `Ctrl+Space`, then `N` to capture a TODO without
leaving your current work. It lands in that feature's worktree list (the
project's if the feature sits on the repo root), and the capture box names the
list it is writing to.

Existing TODO lists are unchanged by the upgrade: they stay project-scoped,
keep their host feature and their links, and show up in the project pane. New
worktree lists start empty.

Press `Enter` on a TODO to start work on it. AMF asks how:

| Choice | What happens |
| --- | --- |
| **Start an agent on this TODO** | Opens a session with the TODO in the composer, unsent — in this feature for a worktree TODO, or in a feature you pick for a project or global one. |
| **Start an agent in a new feature** | Opens the ordinary create-feature wizard, pre-filled with a branch name from the TODO title and plan mode off; once the feature exists, the TODO is linked to it and its agent is seeded with the TODO, unsent. Needs a git repository. |
| **Plan this TODO first** | Runs the guided plan interview, with the TODO's title, notes, and the list scratchpad already filled in as the feature brief — editable before the first question. |

If you choose to plan it, AMF then asks where the plan should land: **here**, in
the feature the list lives in, or in a **new feature and worktree** created for
it. Picking a new feature opens the ordinary create-feature wizard, pre-filled
from the TODO, so the branch, agent, and permission mode are still yours to
change; the worktree is created before the interview starts, so the plan is
written into a real checkout.

Either way the accepted plan is saved and an agent is started on it. A plan that
lands in an existing feature is written to `AMF_PLAN.todo-<name>.md` rather than
`AMF_PLAN.md`, so it sits beside that feature's own plan instead of replacing
it, and the agent is told which file is its own.

A TODO planned into a new feature stays open on the list, marked as linked, and
`Enter` afterwards jumps to that feature rather than asking again. If the
feature is later deleted the TODO survives, the link is dropped, and `Enter`
offers the choice again. An interrupted interview is saved as a draft and
offered back the next time you press `Enter` on that TODO.

Press `I` to work the list rather than a particular item: AMF takes the
highest-priority TODO nobody has started, opens an agent on it, and marks the
item in progress (`[~]`) so the next `I` moves on. It considers whichever
lists are currently visible. Hidden project and global scopes are excluded;
at equal priority AMF prefers the narrower visible scope: worktree, then
project, then global. It works on the list itself and on the `TODOs` row on the
dashboard, and the composer is seeded but unsent, exactly as `Enter` leaves it.

A worktree TODO is worked in the feature that owns the checkout. A project or
global TODO belongs to no one checkout, so `Enter` and `I` ask which feature
should work it; the feature you pick supplies the agent and permission
mode exactly as a worktree TODO's own feature would. Press `i` on a TODO to
set or clear that in-progress mark by hand — useful when you abandoned a session
without closing it. When every remaining TODO is already underway, AMF offers
the next one anyway and asks whether to go to the work already started, start a
second agent, skip it, or cancel.

Deleting a feature deletes its worktree list along with the checkout, so if
that list still has unfinished items AMF asks first: move them to the project
list, move them to the global list, delete them with the worktree, or cancel
the deletion. Nothing is killed or removed until you answer.

### See which agents need you

A stopped agent has stopped for a reason, and "waiting for input" does not say
which. AMF reads its harnesses' lifecycle hooks and marks each stopped session
as one of:

| State | Meaning |
| --- | --- |
| **Question** | The agent asked something, or wants permission, and cannot continue without an answer. |
| **Completed** | The agent finished its turn; the work is waiting to be looked at. |
| **Waiting** | The session stopped, but its harness could not say why. |

The state shows on the feature's dashboard row and in the header count. Press
`i` for the full list, questions first and oldest first within each group;
`Enter` opens the session, `x` dismisses the row.

AMF does not capture the agent's message — open the session to read the
question or the summary.

Fidelity depends on the harness. Claude Code and OpenCode report all three
states. Codex fires one hook when a turn ends and cannot say whether it
finished or is asking, so its sessions show as **Waiting** either way. Pi has
no hook mechanism, so its sessions carry no state at all. States are not saved: after an AMF restart, sessions
show as ordinary active until their next event.

### Keep the machine from filling up

Agents and the editors they sit alongside are the bulk of what AMF puts on your
machine, and nothing warns you before the last gigabyte goes.

Before starting an agent, AMF checks how many are already running — across every
project, plus any headless review or plan run in flight — and how much memory is
left. If either is past its threshold, one dialog says which, and `y` starts it
anyway. It never refuses. Terminals, editors, and TODO sessions are not counted
and never raise it.

Creating a feature never raises that dialog: a batch create would queue one per
feature. The feature is created and left stopped instead, with a toast saying
why; `c` starts it.

Stopping a feature also closes the editor AMF opened for it, and the language
servers under it — usually the largest thing the feature was holding. AMF only
closes a window it opened itself and can still identify; anything else it
reports as left alone. Under WSL, where `code` hands off to the Windows side,
there is no local window for AMF to own, so it always reports a skip.

Press `z` to list dormant features — idle *and* unattended — with per-row stop,
close-editor, delete, and open.

## Configuration

AMF creates its global configuration at `~/.config/amf/config.json`. A project
can add `amf.json` at its repository root; project settings override
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

These settings govern the resource guards described above:

```json
{
  "max_concurrent_agents": 4,
  "low_memory_warn_mb": 1536,
  "kill_editor_on_stop": true,
  "dormant_idle_minutes": 60,
  "dormant_last_accessed_hours": 4
}
```

`0` disables either warning. The defaults come from measured idle memory: a
Claude harness settles near 380 MiB and Codex near 220 MiB, while a single
language server runs to roughly 1.8 GiB — which is why editors are reclaimed
when a feature stops but never counted as agents. A feature is dormant only
when both of its thresholds are past.

This setting governs how long a needs-attention state stays on the dashboard:

```json
{
  "waiting_stale_minutes": 30
}
```

A question nobody has answered for this long stops being news, so AMF drops it
back to plain idle and the session leaves the `i` list. `0` keeps states up
until the agent produces output again. Ageing out does not stop or change the
session, and a waiting session still counts toward `max_concurrent_agents` and
still qualifies as dormant.

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

- Run `amf doctor` for a read-only report on what AMF is putting on the
  machine: agents against your limit, editor windows open alongside them,
  memory and swap, `amf-*` tmux sessions and worktrees with no matching
  feature, editors still running for features you stopped, and any project
  still keeping config at the legacy `.amf/config.json` path. `--json` emits
  the same findings for scripting. It changes nothing and always exits `0`.
- Press `D` on the dashboard to view AMF's debug log. The same log is available
  at `~/.local/state/amf/debug.log`.
- If icons render incorrectly, set `"nerd_font": false` in the global config.
- If an agent does not appear during feature creation, press `A` and confirm
  that its CLI is installed and enabled.
- If an old AMF installation cannot run `amf upgrade`, install the latest
  release bundle once and then use the built-in upgrader for future releases.

## License

AMF is released under the [MIT License](LICENSE).
