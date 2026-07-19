---
name: amf:screenshot
description: >
  Capture screenshots (PNG) or a GIF of AMF's own TUI running in an
  isolated, throwaway instance, as visual proof a feature/UI change
  works. Use only when the user explicitly asks for visual proof
  ("show me a screenshot of X", "prove the dashboard renders Y") —
  not automatically after every UI change.
allowed-tools: Bash(scripts/dev/screenshot/*) Bash(python3 *) Bash(mkdir *) Bash(cat *) Bash(ls *) Write Read
---

## When to use

Only when the user explicitly asks for visual proof that an AMF
feature or UI change works. Do not run this unprompted after routine
UI edits — it's for on-demand review, not a build step.

## Isolation guarantees

`scripts/dev/screenshot/amf-capture.sh` launches a **throwaway
scratch AMF instance**, never the user's real one:

- Its own `XDG_CONFIG_HOME` / `XDG_STATE_HOME` under
  `${AMF_SHOT_DIR:-/tmp/amf-shots}/<timestamp>/` — the real
  `~/.config/amf/amf.db` and any real running `amf` session are never
  touched.
- A dedicated tmux session (`amf-shot-<timestamp>`), separate from any
  `amf-*` session the user already has.
- Fixed geometry, `120x40` by default (`--geometry WxH` to override) —
  reproducible pane layout across runs.
- Teardown kills the scratch tmux session and deletes the scratch root
  on exit; pass `--keep` to preserve it for debugging.

## Step 1: author a scenario for the feature you just built

Look at what you actually changed, then write a scenario file using
the driver's grammar — don't reuse a generic scenario blindly. Read
`scripts/dev/screenshot/scenarios/dashboard-tour.txt` and
`create-project-flow.txt` for the grammar in practice before writing
one.

Grammar (one step per line, `|`-separated, blank lines and `#`
comments ignored):

- `key:<name>` — `tmux send-keys` key name (`key:Enter`, `key:j`,
  `key:Escape`, `key:?`)
- `text:<literal>` — literal typed text (`text:my-feature-name`)
- `wait:<ms>` — sleep, use after keys that trigger a redraw or async
  work (harness checks, status sync)
- `shot:<label>` — capture the pane now, written as
  `NNN-<label>.ansi`

Put a `shot:` at every point worth showing (before the change, mid
interaction, after the change lands) rather than just first/last.
Author the file under `scripts/dev/screenshot/scenarios/` if it's
worth keeping as a reusable example, otherwise a scratch path (e.g.
your scratchpad dir) is fine for a one-off.

## Step 2: run the driver

```bash
scripts/dev/screenshot/amf-capture.sh --scenario <your-scenario> --out-dir <dir>
```

Relevant flags:

- `--seed <automation-json>` — pre-populate demo state (a project
  and/or feature) via the automation IPC before the scenario runs, if
  the feature you're proving needs existing projects/features to be
  visible. See `scripts/dev/screenshot/scenarios/seed-project.json`
  and `seed-feature.json` for the payload shape (`docs/automation/`
  has the full schema); action is inferred from the payload's keys
  (`path` → create-project, `branch` → create-feature).
- `--gif` — only pass this if the user asked for a GIF/video, not for
  a plain "show me a screenshot" request. Screenshots (PNG) are the
  default deliverable.
- `--keep` — preserve the scratch root instead of deleting it on exit
  (useful while iterating on a scenario).

This produces numbered `.ansi` dumps (one per `shot:` step) in
`--out-dir` (default `<scratch-root>/shots`).

## Step 3: render PNGs

**`amf-capture.sh` does not render PNGs unless `--gif` is passed** —
without `--gif` you get raw `.ansi` dumps only, and must render each
one yourself:

```bash
python3 scripts/dev/screenshot/render_ansi.py <dump>.ansi --out <dump>.png --cols 120 --rows 40
```

Pass `--cols`/`--rows` matching the `--geometry` used for the capture
(120x40 by default) so every frame renders at the same size. Repeat
per `.ansi` file. If `--gif` was passed instead, the driver already
renders and assembles the GIF — nothing further to do.

## Step 4: verify and return the result

Read the generated PNG(s) (or the GIF) yourself to confirm they show
what you intended before handing them to the user. Return the
absolute file paths as the deliverable proof — that's the artifact the
user is asking for, not a description of what should be visible.
