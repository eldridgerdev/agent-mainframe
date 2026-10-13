# AMF screenshot harness

Tools for getting reviewable proof-of-work (PNG screenshots, optionally a
GIF) out of a throwaway `amf` instance, without touching a real
`~/.config/amf/amf.db` or any real `amf-*` tmux session. Background and
design rationale: `docs/backlog/screenshot-review-plan.md`.

The scripts:

- `amf-capture.sh` — the driver. Launches an isolated `amf`, drives it
  through a scenario, dumps raw ANSI captures.
- `render-captures.sh` — turns `tmux capture-pane -e -p` dumps into PNGs.
  It uses `render_xterm.cjs` (xterm.js in headless Chromium) and falls back
  to `render_ansi.py` (Pillow) where Node or a browser is missing. See
  "Rendering".
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
tmux socket: /tmp/amf-shots/20260719-121610-294766/tmux.sock
tmux session: amf-shot-20260719-121610-294766 (120x40)
dashboard ready
shot: /tmp/amf-shots/20260719-121610-294766/shots/001-dashboard-ready.ansi
shot: /tmp/amf-shots/20260719-121610-294766/shots/002-after-j.ansi
shots written to: /tmp/amf-shots/20260719-121610-294766/shots
```

On exit, the owned tmux server and scratch root are removed, including any
feature sessions the scenario started. Existing or concurrently created
sessions on another server are untouched. `--keep` closes only the capture
pane and retains the scratch files and any spawned sessions on the private
socket printed at startup.

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
  exit instead of deleting it. The capture pane is still closed; any spawned
  sessions remain on the private socket for debugging. Stop that server with
  `tmux -S <printed-socket> kill-server` when finished.

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

## Native GUI diff proof

The native diff scenario captures the actual Tauri application through its Rust
commands, using an isolated Git checkout and SQLite database. It starts no agent
sessions. The six frames cover the stopped-feature entry, current changes,
unified and side-by-side layouts, whole-file context, one commit and a binary
notice; assertions run before each frame is saved.

```bash
scripts/dev/screenshot/amf-capture.sh \
  --scenario scripts/dev/screenshot/scenarios/gui-standalone-diffs.txt \
  --out-dir /tmp/amf-gui-diff-proof
```

Local captures need the Linux GUI build dependencies, an X11 display (or Xvfb),
and system Python's GTK introspection, `websocket` and `Xlib` modules. The helper
installs these in the capture-only CI job, builds the frontend and GUI, then
cleans up its own GUI and Vite process groups. `HOME` stays unchanged; both AMF
configuration/state directories are private. Port 1420 must be free. The TUI
setup frame remains internal, and only native PNG frames reach the gallery.
Publish this scenario with the same `publish-pages.sh --strict` path as TUI
proof; no extra seed files are needed.

The manual Final Review scenario reuses the same isolated native GUI fixture.
Its seven frames cover the stopped-feature entry, notes and saved line threads,
verdicts and whole-file comments, split layout, unsaved-draft protection,
pause/reopen, changed-patch rejection and approval invalidation after refresh.
It checks both the visible UI and saved review progress without starting an agent.

```bash
scripts/dev/screenshot/amf-capture.sh \
  --scenario scripts/dev/screenshot/scenarios/gui-manual-review.txt \
  --out-dir /tmp/amf-gui-review-proof
```

The Final Review AI scenario uses offline Claude/Codex executables in a private
Git checkout. Eight asserted native frames cover the controls, resolved prompt
preview, walkthrough and overview, co-review drafts, local multiline question,
repository-aware answer, accepted finding persistence and changed-patch refusal.
It checks the saved progress and invocation log through real Rust IPC; opening,
editing, cancelling and reopening must not start additional calls. No paid
harness or terminal agent is launched.

If Claude's native versions exist under HOME, this scenario also requires
`bubblewrap` (`bwrap`). The GUI's mount namespace masks those versions so
discovery selects the private fixture executables. A clean CI HOME uses the
private PATH directly, without creating a namespace. HOME and installed files
stay unchanged. Its GUI uses
software rendering; the WebKit process sandbox is disabled only for this
isolated offline capture. The helper owns its GUI/Vite process groups and a
private tmux socket.

```bash
scripts/dev/screenshot/amf-capture.sh \
  --scenario scripts/dev/screenshot/scenarios/gui-review-ai.txt \
  --out-dir /tmp/amf-gui-review-ai-proof
```

## Capture contract

The driver and the two Python helpers have deliberately separate output
contracts:

- `amf-capture.sh` always captures terminal state as numbered
  `<NNN>-<label>.ansi` files using `tmux capture-pane -e -p`. It also writes a
  same-named `.txt` file from `capture-pane -p`; the text twin is escape-free
  and is intended for cheap content assertions. The default smoke test writes
  two shots, while a scenario writes one shot for each `shot:` step.
- Ordinary capture runs do **not** render PNGs. To render captures, run
  `render-captures.sh` on them; each PNG is written beside its `.ansi`:

  ```bash
  scripts/dev/screenshot/render-captures.sh --cols 120 --rows 40 \
      /tmp/amf-shots/<run>/shots/*.ansi
  ```

- `amf-capture.sh --gif [path]` is the convenience path: after all shots are
  captured, it runs `render-captures.sh` over every numbered `.ansi` file
  using the requested `--geometry`, then calls `assemble_gif.py` to produce
  `capture.gif` (or the optional path). Thus `--gif` produces both the PNG
  frames and the GIF; it is the only driver flag that performs PNG rendering.
  Those frames are rendered at `--gif-scale` (default `1`), not at
  `render-captures.sh`'s default of `2`.
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
    --summary "One sentence explaining the complete flow under review" \
    --ref <pushed-branch>
```

Add `--gif` when an animation is wanted and `--geometry 160x44` when the
scenario needs another fixed size. The invoking agent needs an authenticated
`gh` CLI session with permission to dispatch Actions and edit the PR, and — for
the deploy step, which now runs locally — `CLOUDFLARE_API_TOKEN` in the
environment, `CLOUDFLARE_ACCOUNT_ID` set, and `wrangler` on `PATH` (or `npx`).
The owner keeps the token in `~/.secrets/cf-amf-pages.env`; the
`amf-publish-screenshots` shell wrapper sources that file and calls
`publish-pages.sh`, so prefer the wrapper (or `source` the file yourself).
`wrangler login` also satisfies the check but is unreliable here — use the
token. The scenario must exist on the pushed ref; screenshots never need to be
committed.

The workflow must already be present on the repository's default branch before
the command can dispatch it; the capture itself may target any pushed ref.

The command dispatches `.github/workflows/amf-screenshot-artifact.yml`, which
is **capture-only**: one unprivileged job builds AMF on an isolated runner,
runs `amf-capture.sh`, renders PNGs, and uploads them as a 14-day internal
artifact. It holds no deployment credentials and there is no environment gate.

`publish-pages.sh` then waits for that run, downloads its artifact with
`gh run download`, runs `build_static_gallery.py` to produce a script-free
`index.html` (images inlined as `data:` URIs) plus a `_headers` file with a
`default-src 'none'` Content-Security-Policy, and deploys that directory to the
Cloudflare Pages preview branch `pr-<number>` with `wrangler pages deploy` —
all on the local machine. The Pages credentials never enter CI. Safety rests on
the split itself: the deploy step only ever handles rendered images, never the
requested ref's code.

The internal capture artifact contains:

- numbered `.ansi` captures and escape-free `.txt` assertion twins;
- rendered PNG frames and, when requested, `capture.gif`;
- `capture-metadata.json`; and
- a self-contained `gallery.html` with images embedded in the HTML.

After the deploy, the publisher replaces only the PR body region between
`<!-- amf:screenshots:start -->` and `<!-- amf:screenshots:end -->` with the
stable private Pages URL. In the Pages project, enable **Settings > General >
Enable access policy** before publishing evidence; raw ANSI/text captures remain
in the internal artifact. Repeated runs update the same branch alias while
preserving all other PR body content.

To make the index useful without downloading anything, add a `note:` step
immediately before every `shot:` in the scenario. The note is a concise,
reviewer-facing sentence describing the visible state and what it proves. The
required `--summary` describes the overall flow. Pages renders the summary and
an ordered walkthrough with each note under **What this proves**.

Capture, authentication, workflow, artifact, download, gallery-build, deploy,
and PR-body failures print an actionable `warning:` and return success by
default so a larger PR workflow can continue. Pass `--strict` when the caller
needs a nonzero exit status instead.

### Deployment safeguards

Only the GitHub account `eldridgerdev` may execute the capture workflow (the
`if: github.actor == 'eldridgerdev'` gate on the job), and all dispatches share
one concurrency group so a burst just queues one Rust build at a time. Agents
must use that account's authenticated `gh` session.

The Cloudflare Pages deploy runs on the operator's machine, from
`publish-pages.sh`. Provide a Pages-scoped `CLOUDFLARE_API_TOKEN` — the owner
keeps it in `~/.secrets/cf-amf-pages.env`, and the `amf-publish-screenshots`
shell wrapper sources that file before invoking the script — and set
`CLOUDFLARE_ACCOUNT_ID` (an identifier, not a secret). `wrangler login` (OAuth,
nothing stored by us) also passes the check, but it is unreliable on this box;
prefer the token. There is no `screenshot-pages` GitHub environment and no
per-run approval: no Cloudflare credential is ever stored in GitHub, and the
deploy step only ever sees rendered images downloaded from the capture
artifact — it never checks out or executes the captured ref. If auth or
`wrangler` is missing, the script warns and stops before dispatching a capture
it could not publish.

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
- `note:<text>` — a reviewer-facing sentence explaining what the next `shot:`
  proves; it is shown in the private Pages gallery, not sent as terminal input.
  A `note:` is a *claim*, not a check — pair it with `expect:`/`expect_not:`
  below so the claim is actually verified, not just narrated.
- `expect:<text>` — a literal substring that **must** appear in the next
  `shot:`'s captured pane (checked against the escape-free `.txt` twin).
  Stack several `expect:` lines before one `shot:` to require all of them.
  If the substring is missing, `shot()` prints the assertion, the offending
  shot, and the actual pane content to stderr, and the whole run exits
  nonzero **immediately** — no further steps run, and nothing gets published.
  This is what turned the September 2026 "every screenshot was actually the
  syntax-parser picker, not the feature under test" incident into a failed
  CI run instead of a silently-wrong published gallery: the scenario assumed
  a seeded project/feature that a bare `amf-publish-screenshots` call (no
  `--seed`/`--seed-feature`) never provided, so the intended key sequence
  drove the dashboard's default keybindings instead of the flow the scenario
  claimed to show.
- `expect_not:<text>` — the inverse: the next `shot:` must **not** contain
  this substring. Same stacking, same fail-fast, all-or-nothing behavior as
  `expect:`. Useful for asserting a fallback/error screen was *not* what
  actually got captured.
- `shot:<label>` — `capture-pane -e -p` the current pane to
  `NNN-<label>.ansi` in the output dir (`NNN` is a zero-padded, per-run
  step counter, not tied to the line number). Each shot also writes an
  escape-free `NNN-<label>.txt` twin (`capture-pane -p`) — the cheap
  artifact for greppable content checks, so nothing has to parse or
  read the ANSI dump just to verify text. Immediately after capturing,
  `shot()` checks every `expect:`/`expect_not:` staged since the last
  `shot:` and clears them.

**Every shot that is meant to prove something needs at least one
`expect:`/`expect_not:` immediately before it** — not just a `note:`. A
`note:` only documents what a human reader should see; it is never checked
against what the pane actually contains. `expect:`/`expect_not:` is the
mechanism that actually verifies it, and a scenario ending with a staged
`expect:`/`expect_not:` that no later `shot:` consumes is itself a hard
error (a dead assertion that never ran proves nothing). A purely
navigational shot with nothing specific to assert (e.g. "press Escape,
capture the resulting dashboard" with no particular claim about its
content) can skip it, but a shot the note describes as showing a specific
title, label, or state must be backed by an assertion that would actually
fail if that state were wrong.
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

  `scenarios/ai-review-completed-fixture.txt` is the approved exception for a
  completed AI Review visual: it calls `seed-ai-review-fixture.sh`, which
  writes deterministic cache data and opens the pane without invoking an
  agent, spending tokens, or calling GitHub. Do not replace it with a live
  `A` review in CI.

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
- `ai-review-completed-fixture.txt` — shows a completed AI review and its
  post-confirmation disclosure using the deterministic fixture helper.

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
- **Dedicated tmux server.** Every run creates a private socket under its
  scratch root and starts tmux without the operator's tmux configuration.
  Unix socket paths are short (about 104 bytes on macOS, 108 on Linux), so
  when `AMF_SHOT_DIR` makes `<scratch-root>/tmux.sock` too long the socket
  alone moves to a private `/tmp/amf-shot-tmux.*` directory, removed with the
  server. The `tmux socket:` line printed at startup is always the real one.
- **One `tmux` for everything the run starts.** The driver writes a wrapper
  to `<scratch-root>/tmux-bin/tmux` that adds the owned socket, puts it first
  on `PATH` for `run:` steps, and clears any inherited `TMUX`. Plain `tmux`
  therefore reaches the owned server from Bash, `sh` and Python
  `subprocess` helpers alike. The spawned TUI and automation get the same
  wrapper as `AMF_TMUX_BIN`, with `AMF_TMUX_SOCKET` and
  `AMF_TMUX_DEDICATED_SOCKET` set, overriding an ambient tmux identity. The
  wrapper sits in a directory of its own because AMF prepends that
  directory to each agent window's `PATH`; the real tmux's directory there
  would let a real `codex`/`gh`/`git` beat a `shim:` stand-in.
- **Explicit servers stay explicit.** A helper that passes `-S <path>` or
  `-L <name>` gets exactly that server, and owns its cleanup: the driver
  neither redirects it to the owned server nor stops it. Native fixtures use
  this for their own private sockets. No shared server is enumerated.
- **Explicit pane environment.** `-e` on `tmux new-session` and the final
  `exec env` pin the scratch paths and tmux identity even if the pane shell's
  startup files rebuild its environment.
- **The XDG amf subdir must exist before `amf` starts.** The driver runs
  `mkdir -p "$CONFIG_DIR/amf" "$STATE_DIR/amf"` before launching, not
  just the scratch root. AMF's `amf_config_dir_with()` falls back to the
  legacy `~/.config/amf` path whenever the real `HOME` already has one
  *and* the XDG-resolved dir doesn't exist yet — and since this harness
  deliberately keeps the real `HOME`, skipping this pre-create would mean
  the scratch instance silently opens the user's real database.
- **Teardown.** On exit (`trap cleanup EXIT`), the capture pane is always
  closed. Unless `--keep` was passed, only the run-owned server is killed and
  its scratch directory removed. Session names or a before/after snapshot
  never determine ownership on a shared server.

The real-tmux regression suite runs against test-owned private servers and a
controlled default-command shim, never the operator's default server:

```bash
python3 -m unittest discover -s scripts/dev/screenshot -p 'test_capture_isolation.py' -v
```

It checks success, failed screenshot assertions, `--keep`, inherited Bash
functions, explicit `-S` and `-L` overrides, plain `tmux` from Bash, `sh` and
Python helpers, a scratch root too long for a socket path, the
application/automation identity surviving a shell startup file that re-exports
the ambient one, and survival of both existing and concurrently created
unrelated sessions. CI runs it on Linux and macOS after `cargo test`.

One caveat worth knowing: a truly fresh scratch config shows AMF's
one-time "Configure Agent Harnesses" onboarding dialog before the
dashboard. The driver detects this and drives it automatically (selects
the first harness entry, waits for its availability check, confirms with
`c`) before running your scenario, so scenarios don't need to account for
first-run onboarding themselves.

## Rendering

`capture-pane` already emits a laid-out grid with no cursor-movement
escapes, so rendering is only a matter of drawing that grid well.
`render-captures.sh` does it with one of two renderers and writes
`<capture>.png` beside each `<capture>.ansi`:

```bash
scripts/dev/screenshot/render-captures.sh --cols 120 --rows 40 <dir>/*.ansi
```

- `--cols` / `--rows` — the grid size; match the `--geometry` used to
  capture (default `120x40`). Every frame of a run is then the same size,
  which a GIF requires.
- `--scale <n>` — pixel density (default `2`, sharp on a HiDPI display;
  `1` gives the smallest files).
- `--renderer auto|xterm|pillow` — `auto` (the default) uses xterm.js and
  falls back to Pillow with a warning; `xterm` fails instead of falling
  back; `pillow` skips xterm.js. One renderer always draws the whole set,
  so a run never mixes the two. In a GitHub Actions run the fallback is
  also raised as a `::warning::` annotation, since nothing in the frames or
  the gallery says which renderer drew them.

### xterm.js (default)

`render_xterm.cjs` writes each dump into a real terminal emulator — the
`@xterm/xterm` the desktop app uses — running in headless Chromium, and
screenshots the result. What an emulator handles is therefore handled:
double-width characters, dim, italic and struck-through text, styled
underlines, colon-form colour sequences, and box and block characters
drawn as geometry so borders are continuous lines. (An emoji still needs an
emoji font on the machine; CI installs only DejaVu.)

- **Setup.** `node scripts/dev/screenshot/render_xterm.cjs --install`
  fetches the pinned `playwright-core`, `@xterm/xterm` and
  `@xterm/addon-webgl` into `${XDG_CACHE_HOME:-~/.cache}/amf/screenshot-xterm`
  and downloads a headless Chromium. Nothing is downloaded by `auto`: until
  `--install` has run it falls back to Pillow straight away. `--renderer
  xterm`, or running `render_xterm.cjs` directly without `--no-install`,
  fetches missing packages on first use but never the browser.
  `AMF_PWA_BROWSER=<path>` uses an installed Chromium instead.
- **Palette.** The 16 ANSI colours come from `DARK_TERMINAL_THEME` in
  `gui/src/terminalTheme.ts`, read at render time, with the same
  `minimumContrastRatio` the desktop app applies. A screenshot therefore
  matches the app's own terminal pane, and dim text is lifted to a readable
  contrast exactly as it is there.
- **Font.** DejaVu Sans Mono, the one font CI and a workstation share.
  Where it is not installed the browser substitutes its default monospace
  font; `render_xterm.cjs` warns, because the frames then differ in cell
  size and pixel dimensions from CI's, but still renders (Pillow without
  DejaVu would draw a bitmap font, which is worse).
- **WebGL is required.** xterm's default DOM renderer takes box characters
  from the font and leaves seams between cells; the WebGL renderer draws
  them. `render_xterm.cjs` fails rather than degrade to the DOM renderer,
  and also fails, without writing the frame, if the WebGL context is lost
  part-way through a run.
- **Density comes from the font size.** At a device scale factor of 2 the
  WebGL canvas drew a zoomed quarter of the screen in the headless shell, so
  `--scale` multiplies the font size at a device scale of 1 instead.

Run directly, it takes the same `--cols`/`--rows`/`--scale` plus
`--out-dir <dir>` to write the PNGs somewhere other than beside the inputs,
and `--no-install` to fail instead of fetching missing packages.

### Pillow (fallback)

`render_ansi.py` needs only Pillow and `DejaVuSansMono.ttf`. It is a small
SGR state machine, not a terminal emulator: it walks the dump tracking
16/256/truecolor fg+bg, bold, reverse and underline per cell and rasterizes
the grid. Box-drawing and block-element characters are drawn as geometry
(as terminal emulators do), because a font's glyph is sized to its own
fractional advance and stops short of the next cell once that is rounded to
whole pixels.

```bash
python3 scripts/dev/screenshot/render_ansi.py \
    scripts/dev/screenshot/sample-dump.ansi --out /tmp/dashboard.png
```

It renders one dump per call and takes `--out <path>`, `--cols`/`--rows`
(inferred from that frame's content when omitted, which varies frame to
frame), `--font-size <n>` (default 14), `--scale <n>` (default 2) and
`--theme dark|light`.

What it does not do, and xterm.js does: double-width CJK/emoji cells are
drawn single-width, dim and italic are ignored, and its palette is the
classic VGA one rather than the desktop app's.

`python3 scripts/dev/screenshot/test_render_ansi.py` checks that borders
render as unbroken lines and that every box character reaches the cell
edges it should.

### GIFs

`amf-capture.sh --gif` runs `render-captures.sh` over every
`NNN-<label>.ansi` in the output dir, then `assemble_gif.py` stitches the
PNGs into one animated GIF via Pillow's native `save_all=True,
append_images=...` — no ffmpeg dependency. The frames are rendered at
`--scale 1` (960x640 at 120x40 with xterm.js, a quarter of the pixels
of the default `2`) to keep the GIF small; `--gif-scale
<n>` raises that, and the PNG frames left beside the GIF are the same
size. Frame duration is fixed at
`assemble_gif.py`'s default (800ms/frame) when invoked this way; run
`assemble_gif.py` directly with `--duration-ms` for a different pace.

### Why not `vhs` or a native terminal

[`vhs`](https://github.com/charmbracelet/vhs) is xterm.js in a headless
browser too, behind `ttyd`, `ffmpeg` and its own `.tape` DSL. Rendering the
`capture-pane` dump with xterm.js directly gives the same picture while
keeping this harness's scenario grammar and its `expect:` assertions.
Screenshotting a native terminal (kitty, wezterm) would need a virtual
display and software GL on every runner for no gain in fidelity. Revisit
only if true video — cursor blink, animation between frames — is needed.

### Native TODO and planning proof

`scenarios/gui-planning.txt` exercises the desktop through real Tauri IPC with
private Git worktrees, SQLite and tmux. Its terminal and synthesis CLIs are
local offline stand-ins that refuse checkouts without the fixture marker; it
never invokes a paid harness. The scenario asserts scoped TODO reorder/move/copy,
ordinary and TODO hook choices, form and resource-warning cancellation, an unsent
composer handoff, and full/quick/TODO planning cancellation. It verifies that
cancelled interviews save no plan, create no feature or agent and do not claim
the TODO, while preserving their already-prepared checkouts. It also checks
synthesis pre-call cancellation/retry and unsaved markdown surviving minimize.

The native frames are paired with SQLite, filesystem and receiver assertions;
the latter distinguish an editable draft from a message sent to a harness.
Local runs may set `AMF_GUI_CAPTURE_PORT` and `AMF_GUI_CAPTURE_BINARY` to reuse a
matching desktop build. The binary's compiled dev URL must use the same port.
The ordinary capture workflow builds the requested ref and defaults to port 1420.

### Native Learning proof

`scenarios/gui-learning.txt` exercises the desktop reader and question engine
through real Tauri IPC against private SQLite, Git worktrees and tmux. The
offline Codex fixture refuses non-fixture checkouts and headless commands
without a read-only sandbox. It records requests and receives terminal input
locally; no paid harness runs.

Eleven frames cover repository and diff reading, line/hunk selection, a starter
question and unsent-close protection, a saved answer reloaded after reopening,
a threaded follow-up, editable keep-as-TODO and its save, the resource warning,
an editable unsent handoff and the offer to return to it. SQLite, receiver and
page assertions between frames verify the answer/TODO/session links, a
follow-up relabelled as a change request, unchanged state after cancelling the
TODO draft or the resource warning, and that returning to the linked session
adds no session, sends nothing and leaves the edited draft exactly as it was.
The proof does not validate paid execution, other harnesses, deep dives, drift
notices, stopped-session resume or macOS desktop interactions.

`AMF_GUI_CAPTURE_PORT` and `AMF_GUI_CAPTURE_BINARY` apply as described above.
