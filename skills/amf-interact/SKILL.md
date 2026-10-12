---
name: amf-interact
description: Inspect, drive or troubleshoot AMF's GUI or TUI with compact JSON, batched actions and local waits. Use for AMF interface workflows, including when AMF manages another project. Ordinary work on that project's own app uses its own tools.
---

Choose the tool that matches the requested AMF task:

| Task | Tool |
| --- | --- |
| Configure this project's AMF sessions, hooks, presets or prompts | Available `amf:add-*` / `amf:configure` skills, or edit `amf.json` |
| Create projects/features in the running TUI | `amf automation …`; inspect `--help` and use `--dry-run` first |
| Exercise AMF terminal input or inspect its rendered text | Bundled TUI driver; read [references/tui.md](references/tui.md) |
| Exercise AMF desktop controls and the native backend together | Bundled GUI driver; read [references/gui.md](references/gui.md) |
| Validate a focused AMF code change | Existing component/core tests; add native interaction when the interface round trip matters |
| Obtain requested visual proof | Use the screenshot tooling available in the AMF source checkout; these drivers return text |

Resolve bundled paths **relative to this SKILL.md**, not the project's working
directory. In injected copies, scripts live in `scripts/` beside this file.
In AMF's own source checkout, the same maintained scripts live at
`../../scripts/dev/` relative to this file; the operating guides handle both.
AMF injects the scripts and references together, so the TUI driver
works with an installed `amf`, Python 3 and tmux, without an AMF source checkout.
The GUI driver also needs a matching Linux/WSLg debug GUI, AMF frontend source
and dependencies, an X11 display, and Python `websocket-client`.

Driver `start` creates a disposable private AMF instance. It does not connect to
the dashboard managing your current project. Use a temporary fixture repository
for tests; operations on real repositories can still edit files. Paid harnesses
are disabled by default. Live `amf automation` instead changes the running TUI
and can start real agents; follow the user's requested scope.

Reuse the returned session, scope observations, batch known actions with local
waits, and request only the final observation. Stop in a `finally` block or shell
trap. After a batch error, inspect current state before continuing: earlier
actions remain applied. Text observations do not validate fonts, colors or
browser input routing. Capture screenshots only when explicitly requested.

If a prerequisite is missing, report the specific missing capability and use an
available route from the table. A TUI result does not establish GUI behavior.
