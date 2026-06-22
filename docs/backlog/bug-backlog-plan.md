# Bug backlog

- **Status:** Backlog
- **Owner:** unassigned
- **Relates to:** dashboard key handling (`src/handlers/normal.rs`)

A running list of known bugs that are not yet scheduled for a fix. Unlike
the feature plans in this directory, this is a single shared doc — add a
new `## ` section per bug. The filename ends in `-plan` only so AMF's
in-app Markdown viewer (which filters to paths containing "plan") picks
it up.

For each bug record: how to reproduce, expected vs. actual behaviour, the
relevant code, and any leads on the cause. Move a bug out of this doc (or
strike it through with the fixing commit/PR) once resolved.

## ~~`A` does not open harness setup from the dashboard~~ (Fixed)

- **Status:** Fixed (2026-06-21)
- **Reported:** 2026-06-21
- **Root cause:** The `KeyCode::Char('A') => app.open_harness_setup(false)`
  arm was inside `handle_normal_leader_key` (the Ctrl+Space chord handler),
  not `handle_normal_key`. A bare `A` on the dashboard fell through to
  `_ => {}`; only `Ctrl+Space` then `A` opened the picker. The remap-block
  lead was a red herring — with default (empty) keybindings the remap is a
  no-op.
- **Fix:** Moved the arm into `handle_normal_key` alongside the other bare
  capital actions, added `A` to the dashboard help overlay, and added a
  regression test (`bare_uppercase_a_opens_harness_setup_from_dashboard`).

### Repro

1. Launch AMF and land on the dashboard.
2. Press `A` (capital A), expecting the harness setup picker (the
   first-run multi-select where you enable Claude / Opencode / Codex /
   Pi) to open.

### Expected

The harness setup picker opens so the available harnesses can be changed
after first run.

### Actual

Nothing happens — the picker does not open in dashboard view.

### Leads / where to look

- The key is dispatched at `src/handlers/normal.rs:361`:
  `KeyCode::Char('A') => app.open_harness_setup(false)`.
- `open_harness_setup()` (`src/app/mod.rs:2591`) is unconditional, so the
  failure is likely upstream of the dispatch.
- This arm only runs in `AppMode::Normal` via `handle_normal_key`. If the
  reporter was in an embedded pane / Viewing mode, `A` is forwarded to
  tmux instead of opening the picker — confirm which mode "dashboard
  view" means.
- Check the keybinding remap block at `src/handlers/normal.rs:38-52`: a
  remap whose target resolves through `default_key_for_action` could
  rewrite `A` to a different canonical char before the match.
- `A` is not advertised in the help overlay, so discoverability is also
  a gap regardless of the dispatch bug.

### Notes

Other uppercase arms in the same handler (`N`, `B`, `O`, `S`) are
reported to work, which points at something specific to the `A` path or
to the mode the user was actually in rather than a blanket
uppercase-key problem.
