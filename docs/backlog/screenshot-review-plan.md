# Screenshot & video review harness

- **Status:** All epics shipped
- **Owner:** unassigned
- **Relates to:** ANSI pane capture (`src/tmux.rs:1704`
  `capture_pane_ansi`); XDG config/state resolution
  (`src/project.rs:1217` `amf_config_dir`, `src/ipc.rs:13`
  `socket_path`, `src/debug.rs:47`); automation IPC
  (`src/automation.rs`, `docs/automation/README.md`); skills
  (`.claude/skills/`, `skills/`); no-tmux Docker harness
  (`docker/no-tmux/`)

## Why / problem

AMF is a ratatui TUI, so a change can't be "seen" the way a web page can.
Today, verifying an AMF UI change means the human launches `amf` and
looks. We want to ask an agent (Claude, Codex, …) to build something and
then — **only when explicitly asked** — have the agent run AMF itself and
return **screenshots** (later GIF/video) proving the feature works, with
zero risk to the human's real projects or running dashboard.

The hard pieces already exist in-repo: `tmux capture-pane -e -p` yields
ANSI-colored pane text, and config/socket/log paths all resolve through
the `dirs` crate, which honors `XDG_CONFIG_HOME` / `XDG_STATE_HOME`. That
means an **isolated scratch AMF instance needs no Rust changes** — env
vars alone give it a private DB, IPC socket, and logs. The only genuinely
new code is a renderer that turns captured ANSI frames into PNGs.

## Scope decisions (settled)

- **Instance target:** a fresh, throwaway AMF instance (own scratch
  `XDG_CONFIG_HOME` + `XDG_STATE_HOME` + tmux session), never the human's
  real running session.
- **Rendering fidelity:** ship the **lightweight Python + Pillow** renderer
  now (Pillow 10.3 + `DejaVuSansMono.ttf` already present, no new system
  deps); keep a **high-fidelity `vhs`** path documented as a future
  nice-to-have (needs `ttyd` + `ffmpeg` + headless browser, none present).
- **Default artifacts:** **screenshots (PNG)** by default; **GIF only on
  request** (Pillow assembles GIFs natively — no ffmpeg needed).

## Design summary

New, self-contained tooling under `scripts/dev/screenshot/`, driven by a
new skill. No changes to existing Rust for the lightweight path.

1. **Isolation via env.** Scratch root
   `${AMF_SHOT_DIR:-/tmp/amf-shots}/<ts>/` with `config/`, `state/`, and an
   optional throwaway git repo. Export `XDG_CONFIG_HOME` / `XDG_STATE_HOME`
   at it; **keep the real `HOME`** so `claude` auth + git identity work.
   Distinct tmux session name (`amf-shot-<ts>`) + distinct XDG dirs ⇒ the
   real `~/.config/amf/amf.db`, IPC socket, and any live `amf` are
   untouched.
2. **Harness.** `tmux new-session -d -s amf-shot-<ts> -x 120 -y 40` (fixed
   geometry ⇒ reproducible images), send the isolated `amf` invocation,
   poll `capture-pane -p` until a known dashboard string appears
   (wait-for-ready, not a blind sleep).
3. **Seeding (optional).** `amf automation create-project|create-feature`
   against the scratch socket to populate demo state before capture.
4. **Scenario → shots.** A newline-delimited scenario of `send-keys` steps;
   after each labeled step, `capture-pane -e -p` → `render_ansi.py` →
   numbered PNG.
5. **Renderer.** `render_ansi.py` walks the captured grid cell-by-cell,
   maintaining SGR state (16/256/truecolor fg+bg, bold, reverse,
   underline). Because `capture-pane` emits an already-laid-out grid, a
   small SGR state machine suffices — **no full terminal emulator needed**.
   v1 limitation: double-width CJK/emoji drawn single-width.

## Epics, priorities & parallelization

Priorities: **P0** = minimum to demo value; **P1** = makes it usable by the
agent unattended; **P2** = future nice-to-haves.

Dependency shorthand: an epic lists what it **needs** before starting.
Epics with no unmet needs can run **in parallel**.

| Epic | Priority | Needs | Can start immediately? |
|------|----------|-------|------------------------|
| A. ANSI→PNG renderer | P0 | — | ✅ yes (parallel) |
| B. Isolated harness + driver | P0 | — | ✅ yes (parallel) |
| C. Scenario format + examples | P1 | A, B | after A+B land |
| D. Seeding via automation IPC | P1 | B | after B lands |
| E. GIF assembly | P2 | A, B | after A+B land |
| F. Screenshot skill | P1 | B, C | after B+C land |
| G. Docs + high-fidelity `vhs` path | P2 | B | after B lands |

**Parallel track 1:** Epic A (renderer) — pure Python, testable against a
captured sample dump with no AMF involvement.
**Parallel track 2:** Epic B (harness/driver) — shell + tmux, testable by
launching isolated AMF and dumping raw captures, no renderer needed.
A and B are the two independent P0 tracks; everything else layers on top.

## Progress

### Epic A — ANSI→PNG renderer (P0, independent)
- [x] `scripts/dev/screenshot/render_ansi.py`: read `capture-pane -e -p`
      dump from stdin/file.
- [x] SGR state machine: 16-color, 256-color, truecolor fg+bg; bold,
      reverse, underline.
- [x] Render cell grid onto monospace canvas (`DejaVuSansMono.ttf`), dark
      background; write PNG.
- [x] Flags: `--out`, `--cols/--rows`, `--font-size`, `--theme dark|light`.
- [x] Document v1 double-width-cell limitation in-file.
- [x] Verify against a hand-captured sample: colors + layout faithful.

### Epic B — isolated harness + driver (P0, independent)
- [x] `scripts/dev/screenshot/amf-capture.sh`: create scratch root, export
      `XDG_CONFIG_HOME`/`XDG_STATE_HOME`, keep real `HOME`.
- [x] Launch AMF in `tmux new-session ... -x 120 -y 40`; wait-for-ready by
      polling `capture-pane -p` for a known string (bounded timeout).
- [x] Per-step `capture-pane -e -p` → numbered `.ansi` dump (PNG rendering
      wired up once Epic C's driver integration lands; renderer itself is
      done per Epic A).
- [x] Teardown: kill scratch tmux session; `--keep` to preserve scratch
      root; confirm real AMF/tmux untouched.
- [x] Flags: `--scenario` (minimal `key:`/`wait:`/`shot:` grammar),
      `--out-dir`, `--geometry WxH`, `--keep`.

### Epic C — scenario format + examples (P1; needs A, B)
- [x] Define step grammar: `key:`, `text:`, `wait:` (ms), `shot: <label>`,
      `# comment`.
- [x] Parser in the driver mapping steps to `tmux send-keys` + capture.
- [x] Ship `scenarios/dashboard-tour.txt` (+1 more) as copyable templates.

### Epic D — seeding via automation IPC (P1; needs B)
- [x] Driver `--seed <automation-json>` runs `amf automation
      create-project|create-feature` against the scratch socket.
- [x] Example seed payloads under `scenarios/` reusing
      `docs/automation/*.template.json` shape.
- [x] **Follow-up, from real use.** `--seed` only ever applies one
      payload, but the example `seed-project.json`/`seed-feature.json`
      pair (same `project_name`) clearly intends to compose — a project
      alone has nothing in it for most scenarios to show. Added a second
      `--seed-feature <file>`, always applied as `create-feature` right
      after `--seed`, so a project and its first feature can be seeded
      together in one scratch run. → `scripts/dev/screenshot/amf-capture.sh`,
      `scripts/dev/screenshot/README.md`.

### Epic E — GIF assembly (P2; needs A, B)
- [x] Driver `--gif`: collect rendered frames, Pillow `save_all=True,
      append_images=..., loop=0, duration=...` → animated GIF (no ffmpeg).
- [x] Off by default; verify a multi-step scenario yields a valid GIF.

### Epic F — screenshot skill (P1; needs B, C)
- [x] `.claude/skills/amf-screenshot/SKILL.md` (mirror to `skills/`):
      invoke only when the user asks for proof a feature works; author a
      scenario for the just-built feature; run the driver; return PNG paths
      to the user; build a GIF only on request.
- [x] Codex features receive a native `.agents/skills/amf-screenshot/SKILL.md`
      from `skills/codex/amf-screenshot/`. It uses the same isolated capture,
      scenario, rendering, and verification workflow, then returns direct
      image previews and local file links in place of Claude's Artifact tool.
- [x] Document isolation guarantees + fixed geometry in the skill.
- [x] **Follow-up, from real use.** Returning raw PNG/GIF paths wasn't a
      usable deliverable — many terminal environments don't render images
      inline, so the user had to ask separately for a viewable version. The
      skill's last step now always publishes a small, self-contained
      Artifact HTML gallery (one `<figure>` per shot, terminal-window
      chrome, images inlined as base64 so nothing external loads) via the
      `Artifact` tool, loading `artifact-design` first, and returns the
      published URL as the primary deliverable instead of file paths. →
      `skills/amf-screenshot/SKILL.md`.

### Epic G — docs + high-fidelity path (P2; needs B)
- [x] `scripts/dev/screenshot/README.md`: usage, scenario format,
      isolation model.
- [x] "Future: high-fidelity via `vhs`" section: `.tape` approach gated
      behind `ttyd` + `ffmpeg` + headless browser install (unimplemented).

## Verification

1. Run `render_ansi.py` on a captured sample dump → inspect PNG fidelity.
2. `amf-capture.sh --scenario scenarios/dashboard-tour.txt --out-dir
   /tmp/amf-shots/test` → numbered PNGs produced; confirm real
   `~/.config/amf/amf.db` and any live `amf` session untouched.
3. Re-run with `--gif` → animated GIF produced without ffmpeg.
4. Skill dry-run from both Claude and Codex sessions: ask the agent to "prove
   the dashboard loads"; confirm it invokes the skill, runs the driver, and
   returns viewable PNGs.

## Open questions

- Should scenarios live in-repo (versioned, reusable) or be authored
  ad-hoc per request by the agent? (Plan ships a couple of versioned
  examples; agent can author throwaway ones.)
- Do we ever need shots of **real agent output** (a live `claude` run), or
  only AMF's own UI surfaces? (Plan assumes UI surfaces; real-agent shots
  would need real `HOME`/auth and are slower/nondeterministic.)
- Future: a first-class `amf screenshot` subcommand that captures AMF's
  *own* embedded panes internally — worth it, or is the external driver
  enough? (Deferred; external driver ships value first.)

## Reasoning / when to build

Build when we want reviewable proof-of-work from agents without the human
launching AMF. The two P0 epics (A renderer, B harness) are independent
and small — a single afternoon each — and together deliver the core
"screenshot on demand" capability. C/D/F make it agent-usable unattended;
E/G are polish. The env-isolation insight (no Rust changes needed) is what
makes this cheap; revisit a Rust subcommand only if the external driver
proves too limited.
