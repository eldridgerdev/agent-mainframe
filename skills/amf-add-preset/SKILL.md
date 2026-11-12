---
name: amf:add-preset
description: >
  Add a feature preset to this AMF workspace: a named template that
  pre-fills harness, mode, branch prefix, planning, review and Chrome
  settings when creating a feature.
allowed-tools: Bash(cat *) Bash(mkdir *) Edit(amf.json) Bash(test *) Bash(rm .amf/config.json)
argument-hint: "[preset name]"
---

## Current config

!`cat amf.json 2>/dev/null || cat .amf/config.json 2>/dev/null || echo "{}"`

## Task

Add or update an entry in the `feature_presets` array in
`amf.json`. Create the file if it doesn't exist.

## FeaturePreset schema

```json
{
  "name": "Quick Fix",
  "branch_prefix": "fix/",
  "mode": "vibeless",
  "agent": "claude",
  "review": true,
  "plan_mode": false,
  "enable_chrome": false
}
```

| Field | Required | Valid values | Notes |
|---|---|---|---|
| `name` | yes | any string | Label shown in the preset picker |
| `branch_prefix` | no | any string | Prepended to the branch name the user types |
| `mode` | no | `"vibeless"`, `"vibe"`, `"supervibe"` | Default `vibeless` (diff review on every edit); `supervibe` skips permission prompts |
| `agent` | no | `"claude"`, `"opencode"`, `"codex"`, `"pi"` | Agent harness to use |
| `review` | no | `true`, `false` | Review Mode: the agent keeps a short note per changed file |
| `plan_mode` | no | `true`, `false` | Run the plan interview first; the plan is saved as `AMF_PLAN.md` |
| `quick_plan` | no | `true`, `false` | With `plan_mode`, use the shorter Quick Plan triage |
| `enable_chrome` | no | `true`, `false` | Allow agent to control a browser |
| `remote_control` | no | `true`, `false` | Start the session with remote control enabled, where allowed |

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

Project presets appear first in the picker; global presets are
appended unless the same name exists in the project config.
If the user hasn't specified scope, default to project scope.

## allowed_agents interaction

If `allowed_agents` is set in config, only presets whose `agent`
is in that list will be shown. Make sure the preset's agent is
allowed, or omit `allowed_agents` to allow all.

## Steps

1. Read the current config (shown above — it may have come
   from the legacy `.amf/config.json`)
2. Add the new preset to `feature_presets` (or create the array)
3. Write the updated JSON back to `amf.json` — preserve existing
   presets and other config — then delete `.amf/config.json` if the
   config came from there
4. Tell the user the preset will appear when creating a new
   feature in AMF (press `N` on the project)
