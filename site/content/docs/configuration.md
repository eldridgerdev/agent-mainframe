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

See [Project Config Reference](@/docs/project-config.md) for every key
`amf.json` can hold and how project settings merge with global ones.

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

## Issue trackers in plan interviews

Plan interviews can read tickets from an issue tracker such as Asana, Linear,
or Jira through the same MCP tools Claude Code uses. When the brief links a
ticket, the interview fetches it and treats its contents as part of the
brief, so you don't have to paste it in. This works only when the interview
runs on Claude. Other harnesses ignore the setting and log that they did.

This is a **global-only** setting in `~/.config/amf/config.json`. A
repository's `amf.json` can't turn it on, because MCP servers run programs.

**claude.ai connectors** (tools named `mcp__claude_ai_<Name>__…`) and
servers you added with `claude mcp add` load automatically. You only list
the tools the interview may call:

```json
{
  "plan_interview_mcp": {
    "allowed_tools": [
      "mcp__claude_ai_Asana__get_task",
      "mcp__claude_ai_Asana2__get_task"
    ]
  }
}
```

`allowed_tools` takes **exact** tool names in the form
`mcp__<server>__<tool>`. Wildcards such as `mcp__claude_ai_Asana__*` are
rejected, because most tracker tools can also create and edit tickets. List
only tools that read. The interview sees every connected tool, but anything
not on this list is denied.

The tool names above are placeholders. To print the real ones, run this
command, which makes one small model call:

```sh
echo hi | MCP_CONNECTION_NONBLOCKING=false claude -p --output-format stream-json \
  --verbose --setting-sources "" --tools Read --model haiku \
  | grep -o '"mcp__claude_ai_Asana[^"]*"' | sort -u
```

If you have more than one connector for the same tracker (for example two
Asana workspaces, `Asana` and `Asana2`), allow the read tools from each one
so tickets from either workspace can be read.

**A server that isn't in Claude Code yet** can be supplied as a file with
`config`, which takes an absolute or `~/` path to a file in the
`{"mcpServers": {...}}` format of `.mcp.json`:

```json
{
  "plan_interview_mcp": {
    "config": "~/.config/amf/plan-mcp.json",
    "allowed_tools": ["mcp__linear__get_issue"]
  }
}
```

```json
{
  "mcpServers": {
    "linear": {
      "type": "http",
      "url": "<the server's MCP URL>"
    }
  }
}
```

The server key (`linear` here) is the middle part of each tool name.

With this configured, the adaptive rounds, the synthesis pass, the Expert
plan review, and its follow-up run with read-only repository tools plus the
listed MCP tools. Directed revisions and isolated investigations don't use
them. The interview still can't edit files or run shell commands, and the
repository's own Claude settings, hooks, and `.mcp.json` servers are not
loaded. AMF passes `--setting-sources ""` for that, plus
`MCP_CONNECTION_NONBLOCKING=false` so a run waits for claude.ai connectors
instead of sometimes starting without them. If a tool name or the config
file is invalid, the interview runs without MCP and shows why.

To test a setup outside AMF, this runs the same command the interview uses:

```sh
MCP_CONNECTION_NONBLOCKING=false claude -p --setting-sources "" \
  --tools Read,Glob,Grep --permission-mode dontAsk \
  --allowedTools mcp__claude_ai_Asana__get_task \
  "Fetch Asana task 1201234567890 and summarize it"
```

## Built-in customization skills

AMF injects a small set of skills into Claude Code, Codex, and OpenCode
feature workspaces so the agent can safely customize the project-local AMF
config for you: `amf:configure` explains the current setup, `amf:add-session`
adds a dev server or other persistent command, `amf:add-hook` automates
feature lifecycle events, `amf:add-preset` creates a reusable feature setup,
and `amf:add-prompt` adds a shared prompt template. You can invoke a skill by
name or simply describe the customization you want.
