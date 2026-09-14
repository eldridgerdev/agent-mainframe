+++
title = "Troubleshooting and Automation"
description = "amf doctor, the automation CLI, the debug log, and common fixes."
weight = 100
+++

## `amf doctor`

```bash
amf doctor
amf doctor --json
```

A read-only report on what AMF is putting on the machine: agents running
against your `max_concurrent_agents` limit, editor windows open alongside
them, memory and swap, `amf-*` tmux sessions and worktrees with no matching
feature, editors still running for features you've stopped, and any project
still keeping its config at the legacy `.amf/config.json` path. `--json`
emits the same findings for scripting. It changes nothing on the machine and
always exits `0`.

## Automation CLI

A running AMF instance accepts structured commands for creating projects and
features without scripting the TUI:

```bash
amf automation create-project --file request.json
amf automation create-feature --file request.json
amf automation create-batch-features --file request.json
```

Each command sends a request to the running dashboard over the same local
IPC system AMF's own lifecycle hooks use; AMF applies it inside the
dashboard process and prints a JSON response. Every command accepts
`--dry-run` to validate and preview without changing AMF state.

See the
[automation guide](https://github.com/eldridgerdev/agent-mainframe/blob/main/docs/automation/README.md)
for the full field reference, request templates, and examples.

## Debug log

Press `D` on the dashboard to view AMF's debug log from inside the TUI. The
same log is written to `~/.local/state/amf/debug.log`.

## Common issues

- **An old installation can't run `amf upgrade`.** Install the latest
  [release bundle](@/docs/installation.md#release-bundle-recommended) once,
  then use the built-in upgrader for future releases.
- **Icons render incorrectly.** Your terminal likely lacks a
  [Nerd Font](https://www.nerdfonts.com/); see
  [Configuration](@/docs/configuration.md) for the ASCII-fallback setting.
- **An agent doesn't appear during feature creation.** Press `A` and confirm
  that CLI is installed and enabled.
