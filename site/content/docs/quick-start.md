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

3. Press `N` to add a project, then enter its name and directory. The
   directory may be a git repository or an ordinary folder.

4. Press `n` to create a feature. Choose a branch name, agent, permission
   mode, and whether to use the guided plan interview. The feature starts
   when setup is complete.

5. Select the agent session and press `Enter` to work in its embedded
   terminal. Press `Ctrl+Q` to return to the dashboard.

6. Press `s` on a feature to add another agent, terminal, editor, TODO list,
   or custom session.

Press `?` at any time on the dashboard to see the complete, current
keybinding reference.
