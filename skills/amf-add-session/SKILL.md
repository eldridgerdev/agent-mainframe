---
name: amf:add-session
description: >
  Add a custom tmux session to this AMF workspace. Use when the
  user wants to add a dev server, build watcher, docker compose,
  test runner, or any other persistent background process that
  should appear in the AMF session picker alongside the agent
  and terminal sessions.
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
| `description` | no | Subtitle shown below the name |
| `command` | no | Shell command to run when session starts |
| `window_name` | no | tmux window name (defaults to `name`) |
| `working_dir` | no | Path relative to workdir; `null` = workdir root |
| `icon` | no | Emoji icon |
| `icon_nerd` | no | Nerd font icon (used if terminal supports it) |
| `on_stop` | no | Command run when the session is stopped from AMF |
| `autolaunch` | no | Start automatically when the feature starts |
| `pre_check` | no | Skip launch silently if this command exits non-zero |

## Config path (read the legacy file too)

Project config lives at `amf.json` in the repo root. Older checkouts
keep it at `.amf/config.json`, and the block above falls back to that
file when `amf.json` is absent — so what you see is the config that is
actually in effect, not an empty `{}`.

Always **write** to `amf.json`. If the config you read came from the
legacy path, carry every existing key over into `amf.json` and then
delete the legacy file:

```bash
test -f amf.json && rm .amf/config.json
```

AMF prefers `amf.json` whenever it exists, so leaving both behind would
let a stale `.amf/config.json` sit there looking authoritative while
changing nothing.

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
