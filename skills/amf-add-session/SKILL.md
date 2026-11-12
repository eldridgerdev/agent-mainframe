---
name: amf:add-session
description: >
  Add a custom tmux session to this AMF workspace: a dev server,
  watcher, docker compose, test runner or agent launcher that appears
  in the AMF session picker.
allowed-tools: Bash(cat *) Bash(mkdir *) Edit(amf.json) Bash(test *) Bash(rm .amf/config.json)
argument-hint: "[session name and command]"
---

## Current config

!`cat amf.json 2>/dev/null || cat .amf/config.json 2>/dev/null || echo "{}"`

## Task

Add or update an entry in the `custom_sessions` array in
`amf.json`. Create the file if it doesn't exist.

## CustomSessionConfig schema

```json
{
  "name": "dev",
  "description": "Vite dev server",
  "command": "npm run dev",
  "window_name": "dev",
  "working_dir": null,
  "icon": "🚀",
  "icon_nerd": "",
  "on_stop": "pkill -f 'npm run dev'",
  "autolaunch": false,
  "pre_check": "which npm"
}
```

| Field | Required | Notes |
|---|---|---|
| `name` | yes | Label shown in AMF session picker |
| `agent` | no | `codex`, `claude`, `opencode`, or `pi` when the command hosts an agent, including wrapper scripts; enables its sidebar, tracking and agent actions |
| `description` | no | Subtitle shown below the name |
| `command` | no | Shell command to run when session starts |
| `initial_prompt` | no | Nonempty text for AMF's editable, unsent composer when a new session is created; requires `agent`. Restarts do not seed it again. |
| `window_name` | no | tmux window name (defaults to `name`) |
| `working_dir` | no | Path relative to workdir; `null` = workdir root |
| `icon` | no | Emoji icon |
| `icon_nerd` | no | Nerd font icon (used if terminal supports it) |
| `on_stop` | no | Command run when the session is stopped from AMF |
| `autolaunch` | no | Start automatically when the feature starts |
| `pre_check` | no | Skip launch silently if this command exits non-zero |

For an agent launcher, always set `agent` to the harness it runs. For example:

```json
{ "name": "orchestrator", "agent": "codex", "command": "python3 scripts/launch_orchestrator.py" }
```

AMF supplies its agent environment and launch arguments to the harness through
PATH, so wrappers should invoke `codex` (or the declared harness) by name and
preserve the inherited environment. Keep custom model flags in the wrapper.
Use `initial_prompt` to prefill AMF's composer; do not paste that prompt into
the harness from the wrapper. For example:

```json
{ "name": "reviewer", "agent": "claude", "command": "python3 scripts/launch_reviewer.py", "initial_prompt": "Review the changes and suggest improvements." }
```

`amf automation create-session` also accepts `initial_prompt` for custom agent
sessions. A request prompt overrides the configured default; `submit_prompt: false`
(the default) seeds AMF's composer, and `true` pastes into the ready harness and
submits it. Do not launch the harness by an absolute path or
replace PATH. Existing generic sessions with an unambiguous matching saved
command are recognized on config reload/startup without restarting the process;
launch-time settings take effect on its next launch. Omit `agent` for ordinary
servers, shells and tools. Transcript selection uses the chosen harness's native
resume command; ordinary restarts keep the configured launcher.

## Config path

Always write `amf.json` in the repo root. If the config shown above came from
the legacy `.amf/config.json`, carry every existing key over, then delete the
legacy file so it cannot be mistaken for the live one:

```bash
test -f amf.json && rm .amf/config.json
```

## Scope

- **Project** (this repo only): `amf.json` — edit this file
- **Global** (all projects): `~/.config/amf/config.json` under
  the `"extension"` key

Project sessions take priority over global ones with the same name.
If the user hasn't specified scope, default to project scope.

## Steps

1. Read the current config (shown above — it may have come
   from the legacy `.amf/config.json`)
2. Add the new session to `custom_sessions` (or create the array)
3. Write the updated JSON back to `amf.json` — preserve any
   existing entries — then delete `.amf/config.json` if the config
   came from there
4. Tell the user the session will appear in the AMF picker after
   pressing `s` on the feature, or automatically on next start
   if `autolaunch` is true
