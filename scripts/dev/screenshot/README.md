# AMF screenshot harness

Tools for getting reviewable proof-of-work (PNG screenshots, optionally a
GIF) out of a throwaway `amf` instance, without touching a real
`~/.config/amf/amf.db` or any real `amf-*` tmux session. Background and
design rationale: `docs/backlog/screenshot-review-plan.md`.

Three scripts:

- `amf-capture.sh` — the driver. Launches an isolated `amf`, drives it
  through a scenario, dumps raw ANSI captures.
- `render_ansi.py` — turns one `tmux capture-pane -e -p` dump into a PNG.
- `assemble_gif.py` — stitches PNG frames into an animated GIF (used
  internally by `amf-capture.sh --gif`; can also be run standalone).

## Usage

Basic smoke test, no flags — builds `amf` if needed, launches it in a
scratch tmux session, presses `j` once, writes two `.ansi` captures, then
tears everything down:

```bash
./scripts/dev/screenshot/amf-capture.sh
```

Output looks like:

```
scratch root: /tmp/amf-shots/20260719-121610-294766
tmux session: amf-shot-20260719-121610-294766 (120x40)
dashboard ready
shot: /tmp/amf-shots/20260719-121610-294766/shots/001-dashboard-ready.ansi
shot: /tmp/amf-shots/20260719-121610-294766/shots/002-after-j.ansi
shots written to: /tmp/amf-shots/20260719-121610-294766/shots
```

On exit, the scratch root is deleted and the scratch tmux session is
killed — nothing is left behind unless `--keep` is passed.

Each flag:

- `--scenario <file>` — drive a real workflow instead of the built-in
  two-shot smoke test. See "Scenario format" below.

  ```bash
  ./scripts/dev/screenshot/amf-capture.sh \
      --scenario scripts/dev/screenshot/scenarios/dashboard-tour.txt --keep
  ```

- `--seed <file>` — apply an automation JSON payload (same shape as
  `docs/automation/*.template.json`) against the scratch instance right
  after the dashboard becomes ready, before any `--scenario` steps run.
  The action is inferred from the payload's keys: a top-level `path` key
  means `create-project`, a top-level `branch` key means `create-feature`.

  ```bash
  ./scripts/dev/screenshot/amf-capture.sh \
      --seed scripts/dev/screenshot/scenarios/seed-project.json --keep
  ```

- `--seed-feature <file>` — a second automation payload, always applied as
  `create-feature`, right after `--seed`. Most scenarios that need a
  feature to already exist (a project alone has nothing in it) want both
  flags together, `project_name` matching between the two files:

  ```bash
  ./scripts/dev/screenshot/amf-capture.sh \
      --seed scripts/dev/screenshot/scenarios/seed-project.json \
      --seed-feature scripts/dev/screenshot/scenarios/seed-feature.json --keep
  ```

- `--gif [path]` — after all `shot:` steps run, render every numbered
  `.ansi` capture to a PNG and assemble them into an animated GIF. Off by
  default (no ffmpeg needed either way — see "Rendering"). Path defaults
  to `<out-dir>/capture.gif`.

  ```bash
  ./scripts/dev/screenshot/amf-capture.sh \
      --scenario scripts/dev/screenshot/scenarios/dashboard-tour.txt --gif
  ```

- `--keep` — keep the scratch root (`config/`, `state/`, `shots/`) on
  exit instead of deleting it. The scratch tmux session is still killed
  either way; only the on-disk scratch root is affected.

- `--geometry <WxH>` — tmux session geometry, must match `^[0-9]+x[0-9]+$`.
  Default `120x40`. Fixed geometry keeps screenshots reproducible.

  ```bash
  ./scripts/dev/screenshot/amf-capture.sh --geometry 160x50 --keep
  ```

- `--out-dir <dir>` — where numbered `.ansi` captures land. Defaults to
  `<scratch-root>/shots`, which is deleted with the rest of the scratch
  root unless `--keep` is also passed — pass `--out-dir` explicitly if you
  want captures to survive independent of `--keep`.

  ```bash
  ./scripts/dev/screenshot/amf-capture.sh --out-dir /tmp/my-shots
  ```

- `--amf-bin <path>` — path to the `amf` binary. Defaults to
  `target/debug/amf` relative to the repo root; if that doesn't exist yet
  the driver builds it with `cargo build -j 2` (capped at `-j 2` — see
  the WSL2 OOM note if you're on a memory-constrained box) before
  launching.

  ```bash
  ./scripts/dev/screenshot/amf-capture.sh --amf-bin target/release/amf
  ```

Also respected: the `AMF_SHOT_DIR` env var overrides the scratch root
parent (default `/tmp/amf-shots`).

## Capture contract

The driver and the two Python helpers have deliberately separate output
contracts:

- `amf-capture.sh` always captures terminal state as numbered
  `<NNN>-<label>.ansi` files using `tmux capture-pane -e -p`. It also writes a
  same-named `.txt` file from `capture-pane -p`; the text twin is escape-free
  and is intended for cheap content assertions. The default smoke test writes
  two shots, while a scenario writes one shot for each `shot:` step.
- Ordinary capture runs do **not** render PNGs. To render one capture, invoke
  `render_ansi.py` separately and choose the output path:

  ```bash
  python3 scripts/dev/screenshot/render_ansi.py \
      /tmp/amf-shots/<run>/shots/001-dashboard-ready.ansi \
      --out /tmp/amf-shots/<run>/dashboard-ready.png \
      --cols 120 --rows 40
  ```

- `amf-capture.sh --gif [path]` is the convenience path: after all shots are
  captured, it renders every numbered `.ansi` file to a same-directory `.png`
  using the requested `--geometry`, then calls `assemble_gif.py` to produce
  `capture.gif` (or the optional path). Thus `--gif` produces both the PNG
  frames and the GIF; it is the only driver flag that performs PNG rendering.
- `assemble_gif.py` is also usable directly, but expects one or more existing
  PNG paths in display order. It writes a looping GIF with Pillow's native
  `save_all=True`/`append_images=...` behavior and defaults to 800 ms per frame;
  `--duration-ms` overrides that delay.

The normal deliverable is therefore a directory of `.ansi`/`.txt` captures
unless PNG rendering was requested explicitly. A run's scratch root and its
default output directory are removed on exit; pass `--keep` or use an explicit
`--out-dir` when captures must survive teardown. The driver always tears down
its isolated tmux session.

## Publish private browser-viewable evidence to a PR

For agent-driven publication, use the repository publisher after the branch and
scenario have been pushed:

```bash
scripts/dev/screenshot/publish-pages.sh \
    --pr <number> \
    --scenario scripts/dev/screenshot/scenarios/<scenario>.txt \
    --ref <pushed-branch>
```

Add `--gif` when an animation is wanted and `--geometry 160x44` when the
scenario needs another fixed size. The invoking agent needs an authenticated
`gh` CLI session with permission to dispatch Actions and edit the PR. The
scenario must exist on the pushed ref; screenshots never need to be committed.

The workflow must already be present on the repository's default branch before
the command can dispatch it; the capture itself may target any pushed ref.

The command dispatches `.github/workflows/amf-screenshot-artifact.yml`. Its
unprivileged capture job builds AMF on an isolated runner, runs
`amf-capture.sh`, and renders PNGs. A separate `screenshot-pages` deployment
job does not check out or execute the requested ref; it receives only rendered
images via a 14-day internal artifact, creates a script-free gallery with a
restrictive Content Security Policy, and deploys it to the dedicated
Cloudflare Pages preview branch `pr-<number>`.

- numbered `.ansi` captures and escape-free `.txt` assertion twins;
- rendered PNG frames and, when requested, `capture.gif`;
- `capture-metadata.json`; and
- a self-contained `gallery.html` with images embedded in the HTML.

After success, the publisher replaces only the PR body region between
`<!-- amf:screenshots:start -->` and `<!-- amf:screenshots:end -->` with the
stable private Pages URL. In the Pages project, enable **Settings > General >
Enable access policy** before publishing evidence; raw ANSI/text captures remain
in the internal artifact. Repeated runs update the same branch alias while
preserving all other PR body content.

Capture, authentication, workflow, artifact, and PR-body failures print an
actionable `warning:` and return success by default so a larger PR workflow can
continue. Pass `--strict` when the caller needs a nonzero exit status instead.

The runner installs the Codex CLI so a fresh scratch instance can pass AMF's
harness setup for UI-only scenarios. Scenarios that launch an agent still need
the corresponding provider credentials and harness-specific setup available
to the workflow.

## Scenario format

A scenario file is newline-delimited steps. Each line is a `|`-separated
list of one or more of:

- `key:<name>` — `tmux send-keys` with a key name (e.g. `key:Enter`,
  `key:j`, `key:Escape`, `key:?`).
- `text:<text>` — `tmux send-keys -l` with literal text (special
  characters are not interpreted as key names).
- `wait:<ms>` — sleep this many milliseconds before the next step.
- `shot:<label>` — `capture-pane -e -p` the current pane to
  `NNN-<label>.ansi` in the output dir (`NNN` is a zero-padded, per-run
  step counter, not tied to the line number). Each shot also writes an
  escape-free `NNN-<label>.txt` twin (`capture-pane -p`) — the cheap
  artifact for greppable content checks, so nothing has to parse or
  read the ANSI dump just to verify text.
- `run:<cmd>` — `eval` an arbitrary shell command. The escape hatch for
  anything the grammar above can't express: a second automation call
  (this shell has `AMF_BIN` and the scratch instance's XDG vars
  exported, so it talks to the same running instance), seeding
  working-tree content a JSON payload can't carry, or **mouse input**,
  which has no step of its own. Send the SGR bytes a real terminal
  would, straight into the pane, and crossterm parses them into the
  same event a physical mouse produces:

  ```
  # wheel down at column 40, row 20  ->  ESC [ < 65 ; 40 ; 20 M
  run:tmux send-keys -t "$SESSION" -H 1b 5b 3c 36 35 3b 34 30 3b 32 30 4d
  ```

  Button 64 is wheel up, 0 is a left press, 3 a release.
  `scenarios/plan-review-mouse-scroll.txt` is a worked example.

Blank lines and lines starting with `#` are skipped, so comments can
explain what a step does or which real keybinding it exercises. Multiple
`|`-delimited parts run in order on one line, e.g.:

```
key:N|wait:200|text:demo-project|wait:200|shot:create-project-name
```

Two ready-to-run templates live in `scenarios/`:

- `dashboard-tour.txt` — pokes at read-only dashboard surfaces (help
  overlay, search) without creating or mutating any project/feature
  state.
- `create-project-flow.txt` — walks the `N` create-project wizard
  (Name → Path → Agent) with typed input, then cancels with `Esc` instead
  of submitting, so it's safe to run with no real repo path handy.

And two seed payloads for use with `--seed`, mirroring the
`docs/automation/*.template.json` shape:

- `seed-project.json` — a `create-project` payload (has a `path` key).
- `seed-feature.json` — a `create-feature` payload (has a `branch` key).

Copy any of these as a starting point for a new scenario or seed file.

## Isolation model

The driver is safe to run against a real, in-use AMF setup because it
never touches the paths or process your real `amf` uses:

- **Scratch XDG dirs.** `XDG_CONFIG_HOME` and `XDG_STATE_HOME` are
  exported to fresh directories under `${AMF_SHOT_DIR:-/tmp/amf-shots}/<timestamp>-<pid>/`.
  AMF resolves its config dir, state dir, DB, and IPC socket through the
  `dirs` crate under these vars, so the scratch instance gets its own
  `amf.db`, socket, and log — completely separate from
  `~/.config/amf/amf.db`.
- **Real `HOME` preserved.** Only `XDG_CONFIG_HOME`/`XDG_STATE_HOME` are
  overridden; `HOME` itself is left alone, so `claude` auth and git
  identity still work inside the scratch instance if a scenario actually
  launches an agent session.
- **Dedicated tmux session.** The scratch `amf` runs in its own session
  named `amf-shot-<timestamp>-<pid>`, distinct from any real `amf-*`
  session, and is killed on exit regardless of `--keep`.
- **`-e` on `tmux new-session` is load-bearing, not decorative.** If a
  tmux server is already running (e.g. because you have your own
  `amf-*` sessions open), `new-session` attaches to that *existing*
  server, and the new session's process environment is **not**
  refreshed from the driver's just-exported vars — it inherits whatever
  environment the server was originally started with. Without passing
  `-e XDG_CONFIG_HOME=... -e XDG_STATE_HOME=...` explicitly on the
  `tmux new-session` command, the scratch `amf` would silently launch
  against your real `~/.config/amf` instead of the scratch one.
- **The XDG amf subdir must exist before `amf` starts.** The driver runs
  `mkdir -p "$CONFIG_DIR/amf" "$STATE_DIR/amf"` before launching, not
  just the scratch root. AMF's `amf_config_dir_with()` falls back to the
  legacy `~/.config/amf` path whenever the real `HOME` already has one
  *and* the XDG-resolved dir doesn't exist yet — and since this harness
  deliberately keeps the real `HOME`, skipping this pre-create would mean
  the scratch instance silently opens the user's real database.
- **Teardown.** On exit (`trap cleanup EXIT`), the scratch tmux session
  is always killed; the scratch root directory is deleted unless
  `--keep` was passed.

One caveat worth knowing: a truly fresh scratch config shows AMF's
one-time "Configure Agent Harnesses" onboarding dialog before the
dashboard. The driver detects this and drives it automatically (selects
the first harness entry, waits for its availability check, confirms with
`c`) before running your scenario, so scenarios don't need to account for
first-run onboarding themselves.

## Rendering

`render_ansi.py` turns a single `tmux capture-pane -e -p` dump into a
PNG. It's a small SGR state machine, not a full terminal emulator —
`capture-pane` already emits a laid-out grid with no cursor-movement
escapes, so the renderer just walks the text tracking current SGR state
(16/256/truecolor fg+bg, bold, reverse, underline) per cell and
rasterizes onto a monospace canvas (`DejaVuSansMono.ttf`).

```bash
python3 scripts/dev/screenshot/render_ansi.py \
    scripts/dev/screenshot/sample-dump.ansi --out /tmp/dashboard.png
```

Flags:

- `--out <path>` — output PNG path (default `screenshot.png`).
- `--cols` / `--rows` — override the inferred grid size. Without these,
  the renderer infers grid dimensions from the highest row/column seen in
  that specific frame, which can vary frame-to-frame (e.g. a dialog with
  fewer visible rows) and produce mismatched canvas sizes — pass both
  explicitly (matching the `--geometry` used to capture) whenever you
  need same-size frames, as `--gif` below does.
- `--font-size <n>` — font size in points (default 14).
- `--theme dark|light` — background/default-foreground theme (default
  `dark`).

Known v1 limitation, documented in the file itself: double-width
CJK/emoji cells are drawn single-width (one codepoint per cell), so wide
glyphs will visually overlap or clip.

`amf-capture.sh --gif` automates render+assemble for an entire scenario
run: every `NNN-<label>.ansi` capture in the output dir is rendered to a
same-size PNG (explicit `--cols`/`--rows` taken from `--geometry`, for
exactly the mismatched-size reason above), then `assemble_gif.py` stitches
them into one animated GIF via Pillow's native `save_all=True,
append_images=...` — no ffmpeg dependency. Frame duration is fixed at
`assemble_gif.py`'s default (800ms/frame) when invoked this way; run
`assemble_gif.py` directly with `--duration-ms` for a different pace.

## Future: high-fidelity via `vhs`

Not built — documented here as a future option per the design doc's
scope decision to ship the lightweight Pillow renderer now and defer a
high-fidelity path.

[`vhs`](https://github.com/charmbracelet/vhs) is a tool that drives a
real terminal (via `ttyd`) inside a headless browser and records actual
video frames, rather than reconstructing a grid from `capture-pane`
text. The tradeoff versus the current renderer: real terminal
rendering (accurate double-width glyphs, ligatures, true cursor
blink/animation) at the cost of a much heavier dependency chain and
slower capture.

A `vhs` path would use a `.tape` script (vhs's own DSL: `Type`, `Enter`,
`Sleep`, `Screenshot`, etc.) instead of this harness's scenario grammar,
and would need, none of which are installed in this environment:

- `ttyd` — serves a terminal over a local HTTP/WebSocket connection for
  `vhs` to drive.
- `ffmpeg` — `vhs` uses it to encode captured frames into GIF/MP4/WebM.
- A headless browser (`vhs` bundles a Chromium via `go-rod`/
  `chromedp`-style automation) to actually render the served terminal
  and screenshot it.

If double-width glyph fidelity or true video output ever becomes a hard
requirement, revisit this path — but the lightweight ANSI-grid renderer
covers AMF's actual dashboard UI (no wide glyphs in the chrome itself)
today.
