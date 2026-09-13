+++
title = "Keybindings"
description = "The essential dashboard and embedded-session controls."
weight = 35
+++

This covers the controls you need to get around AMF day to day. Feature-specific
keys — Learning Mode, PR Triage, TODOs — are documented on their own pages.
Press `?` at any time on the dashboard, or `Ctrl+Space` then `?` in a session,
for the complete, current keybinding reference built into that version of AMF.

## Dashboard

| Key | Action |
| --- | --- |
| `j` / `k` or arrow keys | Move through projects, features, and sessions |
| `h` / `l` | Collapse or expand the selected item |
| `Enter` | Open the selected session or expand/collapse an item |
| `N` / `n` | Create a project / feature |
| `s` | Add a session to a feature |
| `c` | Start the selected feature |
| `x` | Stop a feature, or remove the selected session |
| `r` | Rename a feature or session |
| `d` | Delete a project, feature, or session |
| `/` | Search and jump |
| `i` | Show agents needing attention: questions first, then finished work |
| `I` | On a TODOs session row: start an agent on the next TODO in priority order, across the lists currently showing |
| `z` | Show dormant features: idle and unattended |
| `G` | Open GitHub PR triage |
| `W` | Run AMF's AI review of a PR diff |
| `K` | Open Learning Mode: read the code and ask about it |
| `L` | Open the prompt library |
| `E` | Edit headless AI prompt templates (overrides) |
| `T` | Choose a theme |
| `A` | Manage installed agent harnesses |
| `?` | Show all keybindings |
| `q` / `Esc` | Quit |

Search (`/`) accepts every letter, including `j` and `k`. Use arrow keys or
`Tab` / `Shift+Tab` to select a result, `Enter` to jump, and `Esc` to cancel.

## Embedded session

Most keys go directly to the active session. These controls belong to AMF:

| Key | Action |
| --- | --- |
| `Ctrl+Q` | Return to the dashboard |
| `Ctrl+Space` | Open the leader-command menu |
| `Ctrl+Space`, then `w` | Switch sessions |
| `Ctrl+Space`, then `i` | Jump to an agent needing attention |
| `Ctrl+Space`, then `f` | Start final diff review |
| `Ctrl+Space`, then `p` | Open the prompt library |
| `Ctrl+Space`, then `E` | Edit headless AI prompt templates (overrides) |
| `Ctrl+Space`, then `N` | Add a TODO to this worktree's list |
| `Ctrl+Space`, then `?` | Show all leader commands |

## Where the rest live

- [Understanding a Codebase](@/docs/learning-mode.md) — the file-tree, ask,
  and act-on-answer keys inside Learning Mode.
- [Reviewing Changes and PR Feedback](@/docs/review-and-pr-feedback.md) —
  PR Triage's investigate/fix/reply keys.
- [Prompts and TODOs](@/docs/prompts-and-todos.md) — the TODO list's own
  scope-toggle, move/copy, and work-the-queue keys.
