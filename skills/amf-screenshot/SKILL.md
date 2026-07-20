---
name: amf:screenshot
description: >
  Capture screenshots (PNG) or a GIF of AMF's own TUI running in an
  isolated, throwaway instance, as visual proof a feature/UI change
  works, then publish them as a small viewable Artifact gallery page
  (a terminal often won't render PNGs/GIFs inline, so the raw files
  alone aren't a usable deliverable). Use only when the user explicitly
  asks for visual proof ("show me a screenshot of X", "prove the
  dashboard renders Y") — not automatically after every UI change.
allowed-tools: Bash(scripts/dev/screenshot/*) Bash(python3 *) Bash(mkdir *) Bash(cat *) Bash(ls *) Write Read Skill Artifact
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
- `gh` still authenticates as the real user (`GH_CONFIG_DIR` is pinned
  to the real config before `XDG_CONFIG_HOME` is overridden) — needed
  for any scenario that opens PR Triage or the PR picker. Same idea as
  leaving `HOME` untouched for `claude` auth / git identity.
- Fixed geometry, `120x40` by default (`--geometry WxH` to override) —
  reproducible pane layout across runs.
- Teardown kills the scratch tmux session **and any other tmux session
  AMF itself spawned during the run** (e.g. starting a feature creates
  its own top-level `amf-<project>-<feature>` session, outside
  `amf-shot-*`) — found by diffing tmux's session list against a
  pre-run snapshot, not by name pattern, so it catches whatever the
  scenario/seed named things. The scratch root is also deleted on
  exit. Pass `--keep` to preserve both the scratch root and any
  spawned sessions for debugging.

**Not sandboxed:** reading PR comments (opening PR Triage / the PR
picker) is a real, read-only `gh` call against GitHub — safe by
default. But if a scenario also *confirms* a fix-target pick, posts a
reply, or resolves a thread, that's a real write against the real
repo — teardown cleans up the tmux session either way (see above), but
it can't undo a posted GitHub comment or a resolved thread. Drive up
to the interesting frame, `shot:`, then `key:Escape` out rather than
confirming, unless the scenario is deliberately meant to exercise a
write.

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

This produces, per `shot:` step, a numbered `.ansi` dump and a
plain-text `.txt` twin (same capture, no escape codes) in `--out-dir`
(default `<scratch-root>/shots`).

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

## Step 4: verify cheaply, then return the result

Verify content against the `.txt` twins, not the images: grep each
one for the strings the frame should show (a dialog title, the typed
text, a status line). The `.txt` files are small and escape-free —
never read the `.ansi` files, whose escape codes waste tokens.

Only after the text checks pass, Read **one or two representative
PNGs** as images to confirm layout/colors look right — not every
frame.

## Step 5: publish an Artifact gallery (the deliverable)

Raw file paths are not the deliverable — most terminal environments
don't render PNGs/GIFs inline for the user, so finish by publishing a
small, self-contained HTML page with the shots embedded. Do this every
time this skill runs, not just when asked.

1. **Load the `artifact-design` skill** before writing the HTML (the
   `Artifact` tool requires it). Treat this as a utilitarian proof
   page, not a landing page: a short title naming the feature, one line
   of context (branch, PR number, what was seeded — whatever grounds
   the shots), then one `<figure>` per shot in story order, each with a
   caption that says what's notable in that frame. No hero, no
   flourish — a plain terminal-window chrome around each image (a
   title-bar strip is enough) suits the subject better than decoration.
2. **Base64-encode each PNG/GIF** (`python3 -c "import base64; ..."`),
   writing the encoded string to a scratch `.b64` file — don't paste it
   into the HTML through Edit/Write. A multi-shot gallery is tens to
   hundreds of KB of base64; pushing that through the conversation
   burns context for no benefit.
3. **Write the HTML with placeholder tokens** (`IMG_1`, `IMG_2`, …) in
   the `<img src="data:image/png;base64,IMG_1">` slots (or
   `image/gif` for a `--gif` run), then substitute the real base64 in
   directly on disk with a small Python `str.replace` script — the
   encoded data itself never needs to pass through the model.
4. **Publish with the `Artifact` tool** (`file_path` pointing at the
   HTML, a `favicon` emoji fitting the feature, a one-line
   `description`). If re-running this skill again for the *same*
   feature/PR in the same conversation, reuse the same `file_path` so
   republishing updates the existing URL instead of minting a new one.
5. **Return the artifact URL as the primary deliverable.** Mention the
   on-disk PNG/GIF paths too (useful if the user wants the raw files),
   but the URL is what answers "show me."
