---
name: amf:add-preset
description: >
  Add a feature preset to this AMF workspace. Use when the user
  wants a named template that pre-configures agent harness, mode,
  branch prefix, plan mode, review, and Chrome settings for new
  features — so they can create a feature with a single selection
  instead of filling out every field each time.
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
  "mode": "normal",
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
| `mode` | no | `"normal"`, `"vibeless"` | `vibeless` = diff review on every edit |
| `agent` | no | `"claude"`, `"opencode"`, `"codex"` | Agent harness to use |
| `review` | no | `true`, `false` | Enable final review pass when feature finishes |
| `plan_mode` | no | `true`, `false` | Start with a shared PLAN.md task list |
| `enable_chrome` | no | `true`, `false` | Allow agent to control a browser |

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
