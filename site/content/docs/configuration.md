+++
title = "Configuration"
description = "Global and per-project config, resource guards, and theming."
weight = 90
+++

AMF creates its global configuration at `~/.config/amf/config.json`. A
project can add `amf.json` at its repository root; project settings override
matching global settings.

From an embedded session, press `Ctrl+Space`, then `c` to open the
configuration wizard. It covers the common customizations, including:

- feature presets;
- custom session commands;
- lifecycle hooks;
- workspace-specific agent restrictions;
- plan interview questions; and
- key remapping.

Use `T` on the dashboard to choose an AMF theme. If your terminal does not
have a Nerd Font, enable ASCII fallbacks:

```json
{
  "nerd_font": false
}
```

AMF stores its projects, features, saved prompts, and other application
state in `~/.config/amf/amf.db`.

## Resource guards

These settings govern AMF's agent-count and memory guards:

```json
{
  "max_concurrent_agents": 4,
  "low_memory_warn_mb": 1536,
  "kill_editor_on_stop": true,
  "dormant_idle_minutes": 60,
  "dormant_last_accessed_hours": 4
}
```

`0` disables either warning. A feature is dormant only when both of its
thresholds are past.

This setting governs how long a needs-attention state stays on the
dashboard:

```json
{
  "waiting_stale_minutes": 30
}
```

A question nobody has answered for this long stops being news, so AMF drops
it back to plain idle. `0` keeps states up until the agent produces output
again.

## Built-in customization skills

AMF injects a small set of skills into Claude Code, Codex, and OpenCode
feature workspaces so the agent can safely customize the project-local AMF
config for you: `amf:configure` explains the current setup, `amf:add-session`
adds a dev server or other persistent command, `amf:add-hook` automates
feature lifecycle events, `amf:add-preset` creates a reusable feature setup,
and `amf:add-prompt` adds a shared prompt template. You can invoke a skill by
name or simply describe the customization you want.
