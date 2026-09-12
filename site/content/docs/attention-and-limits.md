+++
title = "Attention and Resource Limits"
description = "Which agents need you, and keeping the machine from filling up."
weight = 80
+++

## See which agents need you

A stopped agent has stopped for a reason, and "waiting for input" does not
say which. AMF reads its harnesses' lifecycle hooks and marks each stopped
session as one of:

| State | Meaning |
| --- | --- |
| **Question** | The agent asked something, or wants permission, and cannot continue without an answer. |
| **Completed** | The agent finished its turn; the work is waiting to be looked at. |
| **Waiting** | The session stopped, but its harness could not say why. |

The state shows on the feature's dashboard row and in the header count.
Press `i` for the full list, questions first and oldest first within each
group; `Enter` opens the session, `x` dismisses the row.

Fidelity depends on the harness. Claude Code and OpenCode report all three
states. Codex fires one hook when a turn ends and cannot say whether it
finished or is asking, so its sessions show as **Waiting** either way. Pi
has no hook mechanism, so its sessions carry no state at all.

## Keep the machine from filling up

Agents and the editors they sit alongside are the bulk of what AMF puts on
your machine, and nothing warns you before the last gigabyte goes.

Before starting an agent, AMF checks how many are already running — across
every project, plus any headless review or plan run in flight — and how
much memory is left. If either is past its threshold, one dialog says
which, and `y` starts it anyway. **It never refuses.** Terminals, editors,
and TODO sessions are not counted and never raise it.

Creating a feature never raises that dialog: a batch create would queue one
per feature. The feature is created and left stopped instead, with a toast
saying why; `c` starts it.

Stopping a feature also closes the editor AMF opened for it, and the
language servers under it — usually the largest thing the feature was
holding. AMF only closes a window it opened itself and can still identify;
anything else it reports as left alone.

Press `z` to list dormant features — idle *and* unattended — with per-row
stop, close-editor, delete, and open.

See [Configuration](@/docs/configuration.md#resource-guards) for the
`max_concurrent_agents`, `low_memory_warn_mb`, and dormancy threshold
settings that drive these checks.
