+++
title = "Quick Start"
description = "Get a first project and feature running."
weight = 20
+++

1. Start AMF from a normal shell or an existing tmux session:

   ```bash
   amf
   ```

2. On first launch, choose the agent CLIs you want AMF to use. AMF checks that
   each selected CLI is installed. You can reopen this setup later with `A`.

3. Press `N` to add a project, then enter its name and directory (type the
   full path, or press `Ctrl+B` to browse; `Ctrl+U` clears a field). The
   directory may be a git repository or an ordinary folder. A plain folder gets a warning and a `Ctrl+G` shortcut to run `git init`, and shows a `[no git]` tag; worktrees, diffs, issues and PRs need git.

4. Press `n` to create a feature. The form has one row per choice: name,
   worktree, agent, permission mode (Vibeless asks before every edit or
   command, Vibe edits without asking, SuperVibe never asks), the guided plan
   interview, and the session name. `Tab` / `Shift+Tab` or `Enter` move
   between rows, `Space` toggles a checkbox, and `Ctrl+S` creates from any
   row. The new feature's agent session opens when it starts.

5. Select the agent session and press `Enter` to work in its embedded
   terminal. Press `Ctrl+Q` to return to the dashboard.

6. Press `s` on a feature to add another agent, terminal, editor, TODO list,
   or custom session.

<figure>
  <img src="/images/dashboard.png" alt="AMF dashboard showing a project with four concurrent Claude agent sessions, each on its own git worktree">
  <figcaption>A project with several features running side by side, each in its own worktree.</figcaption>
</figure>

Press `?` at any time on the dashboard to see the complete, current
keybinding reference.
