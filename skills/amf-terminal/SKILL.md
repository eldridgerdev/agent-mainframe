---
name: amf-terminal
description: Open an AMF Terminal Session in this worktree instead of a tmux pane. Use whenever you would otherwise split a tmux pane or window, or ask the user to run a dev server, test, build, REPL or any command in a terminal to see or try something.
---

You run inside AMF. When you need a terminal for the user to see or try
something, **do not run `tmux split-window`/`new-window` and do not ask whether
to open a tmux pane.** Create an AMF Terminal Session in this same worktree.
It appears in the feature's session list, inherits this working directory, and
is cleaned up with the feature.

```bash
cat <<'JSON' | amf automation create-session
{"kind":"terminal","label":"dev server","reuse":true,
 "notes":"Run `npm run dev`, then open http://localhost:3000 and try the login form."}
JSON
```

With no target in the request, AMF uses `$AMF_FEATURE_SESSION_ID`, which is
set in your environment, so you never need the project or feature name. Use
`$AMF_BIN` if `amf` is not on `PATH`.

- `label`: short and specific (`dev server`, `tests`, `repl`). The user sees it.
- `command` + `run`: paste the command and press Enter. Omit `run` to leave
  the command typed for the user to review and run themselves. Omit `command`
  for an empty shell.
- `notes`: markdown the user can read beside the terminal. **Whenever you ask
  the user to run something in the terminal, put what they need in `notes`**:
  the exact commands in code blocks, what success looks like, and anything to
  report back. It appears in the sidebar's Terminal Notes section (`leader u`
  opens it), so they need not scroll back through the conversation. Keep it
  short. With `reuse`, new notes replace that terminal's old ones.
- `reuse: true`: if a live terminal with that label exists, use it instead of
  opening another. Always set it, so a restarted server reuses its terminal.
- `--dry-run` validates without creating anything.

## Working with the terminal afterwards

These take the same optional target and select the terminal by `label` (or
`session_id` from the create reply).

```bash
amf automation list-sessions
echo '{"label":"dev server","lines":80}' | amf automation capture-session
echo '{"label":"dev server","key":"C-c"}' | amf automation send-session-input
echo '{"label":"dev server","text":"npm run dev","run":true}' | amf automation send-session-input
echo '{"label":"dev server"}' | amf automation remove-session
```

- **Verify with `capture-session`.** After starting a server or long job, read
  its output to confirm it came up or to read the error. Do not ask the user to
  paste logs you can read yourself.
- **Restart** with `key: "C-c"` then a new `text` + `run`.
- **Clean up** terminals you created once they are no longer useful
  (`remove-session`). Never remove one the user opened.
- Input and removal work on any non-agent session (terminal, custom, nvim,
  VS Code), never on agent sessions.
- For a quick one-shot command whose output only you need (a build, a test
  run), just run it in your own shell. Use a Terminal Session for things the
  user should watch or interact with: servers, TUIs, REPLs, manual testing.

If `amf automation` fails (no running AMF, no such session), say what failed
and ask the user how to proceed. Do not fall back to tmux silently.
