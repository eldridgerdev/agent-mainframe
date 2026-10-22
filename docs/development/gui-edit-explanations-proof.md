# Supervised-edit explanations

**Explain this edit** reads a saved review note first, without starting an AI
call. An explicitly empty saved section also counts as a saved note; this fixes
the TUI's previous fallback to an AI call for such a section. Other review-note
consumers retain their existing nonempty-section behavior.

Without a saved note, the desktop shows a mandatory pre-call notice with the
exact rendered prompt, Claude model and reasoning level. Continue consumes
that notice once. A changed edit, checkout, session binding, prompt or model
requires a new notice. Claude must be installed and allowed by the project;
saved notes remain available without a harness. Malformed or unreadable project
configuration refuses new calls instead of falling back to defaults. Existing resource warnings
require explicit approval before starting the call.

The desktop call receives no repository tools, uses Claude safe mode and does
not persist a conversation. Its output is capped at 1 MiB, error output at
64 KiB. One 180-second budget covers bounded Claude version discovery and the
call; version probes cap each output stream at 16 KiB. Preview admission only
checks executable paths and starts no subprocess. Cancellation and limit failures terminate
only the owned process group; its concurrency lease remains held until reaping.
The desktop drains all pipes while the process runs. The TUI retains its
existing launcher, mandatory pre-call gate and keybindings.

An explanation does not approve, reject, skip or write the proposed edit.
Results are displayed separately from the agent's own reason. Rejection drafts,
layout, context and the original answer safety hold stay in place. Closing,
retargeting or answering acknowledges cancellation of the matching owned call;
a failed acknowledgement blocks departure and remains retryable. Changed or
answered requests discard late output. Requests delivered directly to a running
TUI still belong to that TUI; the desktop uses the hook's file fallback.

## Reproduce

Use matching source/debug binaries and the native desktop prerequisites:

```sh
scripts/dev/screenshot/amf-capture.sh \
  --scenario scripts/dev/screenshot/scenarios/gui-edit-explanations.txt \
  --out-dir /path/to/private-proof
```

The scenario drives actual native WebKit controls through the maintained GUI
driver and real Tauri IPC. It invokes the shipped Claude diff-review hook with
offline tool input, then leaves that hook waiting while exercising explanation
controls. A private executable receiver validates the restricted flags,
configured model/effort and approved prompt. This receiver replaces paid Claude
execution; it does not establish live Claude service behavior.

The fixture owns its Git repository/worktree, SQLite database, tmux socket and
XDG directories. HOME stays unchanged, with bubblewrap mounts for private AMF
runtime and existing home harness configuration directories. User configuration
and runtime inventories are checked before and after cleanup. It never answers
the real waiting hook, sends an interactive agent prompt or changes source.
The opt-in namespace helper fails closed and only adjusts its own prerequisite
on ephemeral GitHub-hosted Linux runners, restoring it at exit.

Ten asserted frames cover saved-note zero-call reuse, mandatory exact preview,
notice cancellation, successful restricted receiver output, running cancellation
and reaping, external prompt-change refusal, running feature-stop invalidation,
and the retained unsent-feedback close guard. Original session bindings and pane
PIDs are checked at every frame. The capture notes, `assertions.json` and
`isolation.json` describe the fixture boundary; reviewer galleries publish the
rendered PNGs.
