---
name: amf:add-hook
description: >
  Add an AMF lifecycle hook: a script run when a feature starts
  (on_start), stops (on_stop), or gets a new git worktree
  (on_worktree_created), optionally after the user picks an option.
allowed-tools: Bash(cat *) Bash(mkdir *) Edit(amf.json) Bash(test *) Bash(rm .amf/config.json)
argument-hint: "[on_start | on_stop | on_worktree_created] [script path]"
---

## Current config

!`cat amf.json 2>/dev/null || cat .amf/config.json 2>/dev/null || echo "{}"`

## Task

Add or update a hook in `lifecycle_hooks` in `amf.json`.
Create the file if it doesn't exist.

## Hook events

| Event | When it fires |
|---|---|
| `on_start` | Feature session is started or resumed in AMF |
| `on_stop` | Feature session is stopped from AMF |
| `on_worktree_created` | A new git worktree is created for this project |

## Simple hook (plain script path)

```json
{
  "lifecycle_hooks": {
    "on_start": "~/scripts/setup.sh",
    "on_stop": "~/scripts/teardown.sh"
  }
}
```

The `~/` prefix is expanded by AMF. Scripts run with the feature
workdir as the working directory.

## Prompt hook (user picks an option before the script runs)

```json
{
  "lifecycle_hooks": {
    "on_worktree_created": {
      "script": "~/scripts/init-worktree.sh",
      "prompt": {
        "title": "Which environment?",
        "options": ["staging", "production"]
      }
    }
  }
}
```

The user's choice is passed to the script as the environment
variable `$AMF_HOOK_CHOICE`. Only `on_worktree_created` shows
the prompt in the AMF UI before running; `on_start` and
`on_stop` run silently.

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

Project hooks take priority over global hooks for the same event.
If the user hasn't specified scope, default to project scope.

## Steps

1. Read the current config (shown above — it may have come
   from the legacy `.amf/config.json`)
2. Add or replace the relevant hook event in `lifecycle_hooks`
3. Write the updated JSON back to `amf.json` — preserve other
   hooks and sessions — then delete `.amf/config.json` if the
   config came from there
4. Tell the user which event will trigger the script and how to
   test it (e.g. stop and restart the feature from AMF)
