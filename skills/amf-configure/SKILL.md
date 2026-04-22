---
name: amf:configure
description: >
  Show and explain the current AMF workspace configuration.
  Use when the user asks what AMF settings are available,
  wants to understand what is currently configured, or needs
  to know the difference between project and global config scope.
  Routes to amf:add-session, amf:add-hook, or amf:add-preset for
  targeted changes.
allowed-tools: Bash(cat *) Bash(test *)
disable-model-invocation: true
---

## Current project config (.amf/config.json)

!`cat .amf/config.json 2>/dev/null || echo "(none — file does not exist yet)"`

## Current global config (~/.config/amf/config.json)

!`cat ~/.config/amf/config.json 2>/dev/null | python3 -c "import sys,json; d=json.load(sys.stdin); print(json.dumps(d.get('extension',{}), indent=2))" 2>/dev/null || echo "(none)"`

## Config scope

| Scope | File | Key |
|---|---|---|
| This project only | `.amf/config.json` | top-level |
| All projects | `~/.config/amf/config.json` | under `"extension"` |

Project config wins over global for the same key/name.

## What you can configure

### Custom sessions — `custom_sessions`

Persistent tmux windows that appear in the AMF session picker
alongside the agent and terminal. Examples: dev server, docker
compose, test watcher, database shell.

Use `/amf:add-session` to add one.

### Lifecycle hooks — `lifecycle_hooks`

Scripts that run automatically at key moments:

- `on_start` — when a feature is started or resumed
- `on_stop` — when a feature is stopped
- `on_worktree_created` — when a new git worktree is created
  (can prompt user to choose an option first)

Use `/amf:add-hook` to add one.

### Feature presets — `feature_presets`

Named templates that pre-fill agent, mode, branch prefix, plan
mode, review, and Chrome settings when creating a new feature.

Use `/amf:add-preset` to add one.

### Keybindings — `keybindings`

Override default AMF key mappings per action. Project overrides
global per-key. Example: `{ "quit": "Q" }`.

### Allowed agents — `allowed_agents`

Restrict which agent harnesses are available for this project.
Values: `"claude"`, `"opencode"`, `"codex"`. Omit to allow all.

Example: `{ "allowed_agents": ["claude", "opencode"] }`

## What would you like to do?

Based on what the user is asking for, invoke the appropriate skill:

- Adding a session, server, or background process →
  `/amf:add-session`
- Setting up a startup, shutdown, or worktree script →
  `/amf:add-hook`
- Creating a feature template or preset → `/amf:add-preset`
- Anything else — explain using the schema above and edit
  `.amf/config.json` directly
