# Remote Control — server mode

- **Status:** Backlog
- **Owner:** unassigned
- **Relates to:** interactive Remote Control, shipped in v0.24.0 (see
  `REMOTE_CONTROL_PLAN.md`); `claude remote-control` CLI

## Why / problem

Interactive Remote Control (shipped) runs **one local Claude process per
AMF feature** (`claude --remote-control "<name>"`) and mirrors it to
claude.ai / the Claude mobile app. It lets you **drive a session you
already started locally** from your phone or a browser.

It does **not** let you **start new work remotely**. To kick off a fresh
task you still have to be at your machine to create the feature in AMF.

Server mode (`claude remote-control`) is the piece that would let you
spin up brand-new sessions on demand from web/mobile — useful when you
are away from your machine and want to launch parallel, isolated tasks
rather than continue an existing one.

## What server mode actually is (verified against the CLI)

`claude remote-control [options]` runs as a **persistent server** in a
directory and hosts **multiple concurrent sessions from one process**.
Relevant flags (`claude remote-control --help`):

- `--spawn <same-dir|worktree|session>` (default `same-dir`) — how each
  on-demand session is placed. `worktree` gives each session its own git
  worktree; `session` is the classic single-session mode that exits when
  the session ends (≈ what interactive mode already does). Press `w` at
  runtime to toggle same-dir/worktree.
- `--capacity <N>` (default 32) — max concurrent sessions.
- `--[no-]create-session-in-dir` — pre-create a session in cwd; in
  worktree mode that one stays in cwd while on-demand sessions get
  isolated worktrees.
- `--permission-mode`, `--name`,
  `--remote-control-session-name-prefix` — as for interactive mode.
- Notes from `--help`: must be logged in with a subscription; run
  `claude` once in the dir first to accept workspace trust; **worktree
  mode requires a git repository _or_ `WorktreeCreate`/`WorktreeRemove`
  hooks.**

The distinguishing capability vs. the shipped integration: you can
**start new sessions on demand from the phone/web**, not just drive
sessions you created locally, and each can be isolated in its own
worktree.

## Proposed design

### The core architectural tension

AMF's model: **AMF** creates the feature, creates the worktree (under
`.worktrees/`), and owns one tmux session/window per agent, which it
embeds and drives. Server mode **inverts** this: **Claude** spawns
sessions and worktrees on demand, outside AMF's creation flow. A naive
integration (run the server, then discover what it created) means AMF is
permanently racing to reconcile state it didn't author.

### The integration lever: worktree hooks

The `--help` note is the key. Worktree mode accepts
`WorktreeCreate`/`WorktreeRemove` hooks. That lets AMF **provision the
worktrees itself** instead of discovering them after the fact:

- Launch the server with those hooks pointed at a small AMF-provided
  command (e.g. `amf remote-worktree create|remove`, or scripts written
  into the workspace like the existing notification hooks).
- `WorktreeCreate`: AMF creates the worktree under its own
  `.worktrees/<name>` via `WorktreeManager`, registers a new `Feature`
  in the project (origin = `RemoteServer`), and returns the path to
  Claude. AMF stays the source of truth for worktrees and features.
- `WorktreeRemove`: AMF removes the worktree and marks/removes the
  corresponding feature.

This keeps AMF in the loop at creation time rather than reconciling
blind, and reuses the existing worktree + feature machinery.

### The hard limitation (and how it reshapes the design)

A server hosting N sessions is **one process in one tmux window**. AMF's
whole view UX is embedding a tmux pane per agent session. Server-spawned
sessions do **not** each get their own pane, so AMF cannot embed/drive
them individually the way it does features today. The live conversation
for those sessions happens on claude.ai / web / mobile.

So AMF's role for server mode is **not** "drive the agent." It is:

1. **Run + surface the server** as a project-scoped background service.
2. **Provision + track the worktrees** the server spawns (via hooks), so
   the code those remote sessions produce lands in AMF-tracked worktrees
   you can **diff, review, and clean up** locally.

That is a genuinely different value proposition from the rest of AMF
(provision + review, not embed + drive), which is the main reason it is
deferred rather than treated as a natural extension.

### Concrete shape

- **Data model** (`project.rs`):
  - `Project.remote_server: Option<RemoteServerConfig>` —
    `{ spawn_mode, capacity, permission_mode, tmux_session, status }`.
    The server is project-scoped, not a per-feature Claude session.
  - `Feature` gains an origin marker (e.g. `source: FeatureSource` with
    `Manual` | `RemoteServer`) so AMF treats server-spawned features'
    lifecycle differently — it owns their worktree + review but not their
    conversation.
- **Server lifecycle** (new `app/remote_server.rs`):
  - Start/stop the server in a dedicated tmux window running
    `claude remote-control --spawn worktree --capacity N
    --permission-mode <mode>` with the worktree hooks configured.
  - Reuse `ClaudeLauncher::remote_control_block_reason()` to guard
    (z.ai / provider / version) before offering it — and verify a
    server-mode minimum version separately.
- **Hook contract** (`app/setup.rs`, alongside existing hook writers):
  - Write `WorktreeCreate`/`WorktreeRemove` scripts into the workspace
    (never global config — same rule as notification hooks) that call
    back into AMF over the existing IPC / marker-file path so the running
    AMF process registers/removes the feature. **Exact input/output
    contract must be confirmed against the docs before building.**
- **UI surfacing**:
  - A project-scoped "Remote Server" row: running/stopped, URL, QR
    (server mode is exactly where a QR overlay pays off, since the server
    prints a stable URL), capacity, and active-session count if parseable
    from server output.
  - Server-spawned features marked `[remote-spawned]` in the list, with
    their worktree available for the diff/review flow; conversation
    driving is delegated to web/mobile (no embedded pane).
- **Reconciliation** (`app/sync.rs`):
  - Hooks are the primary path. On sync, if the server process is gone,
    mark the server stopped and its spawned features idle/stopped, and
    sweep for orphaned `.worktrees/` entries the `WorktreeRemove` hook
    may have missed.

## Progress

Nothing started — this is a design only. Implementation items:

- [ ] `RemoteServerConfig` + `Project.remote_server` data model
- [ ] `Feature` origin marker (`Manual` | `RemoteServer`)
- [ ] Server lifecycle module (`app/remote_server.rs`): start/stop in a
  dedicated tmux window, guarded by `remote_control_block_reason()`
- [ ] `WorktreeCreate`/`WorktreeRemove` hook scripts written into the
  workspace + AMF-side register/remove callback
- [ ] UI: project-scoped "Remote Server" row (status, URL, QR, capacity)
- [ ] UI: `[remote-spawned]` marker on server-originated features
- [ ] Reconciliation in `app/sync.rs` (server-gone handling + orphan
  worktree sweep)
- [ ] QR overlay (shared with the deferred Phase 3 stretch)
- [ ] Tests + manual verification against a real claude.ai login

## Open questions

- Exact `WorktreeCreate`/`WorktreeRemove` hook I/O contract (how the path
  is returned, env/args provided, failure semantics).
- Whether the server exposes per-session identity (id / name / URL) in a
  machine-parseable way, so AMF can map sessions → worktrees for display.
- Whether an individual server session can be attached/embedded at all,
  or is strictly web/mobile-driven (assumed the latter above).
- Minimum Claude Code version that supports server mode + the hooks.

## Reasoning / when to build

Build only if **"spin up parallel, isolated tasks from my phone, then
review them in AMF later"** is a workflow that is actually wanted. For
"continue / drive work I already started," the shipped interactive
integration already covers it, and AMF's own multi-feature model already
provides parallelism (many features, each its own independently
remote-controllable Claude).

A QR overlay (deferred from the interactive work) becomes worthwhile
here too: in interactive mode tmux `capture-pane` usually can't recover
the session URL (OSC 8 hyperlink targets are dropped), but the server
prints a stable URL as plain text, so a scannable QR would have
something reliable to encode.
