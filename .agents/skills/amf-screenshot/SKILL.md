---
name: amf-screenshot
description: Capture PNG screenshots or an optional GIF of AMF's TUI in an isolated throwaway instance and return viewable visual proof. Use only when the user explicitly asks Codex to show, capture, or prove an AMF UI behavior with screenshots, images, a GIF, or visual evidence; do not invoke automatically after routine UI changes.
---

# Capture AMF screenshots

Use the repository's screenshot harness to exercise the changed UI without touching the user's real AMF database or tmux sessions.

## Preserve isolation

Run `scripts/dev/screenshot/amf-capture.sh`. It creates private `XDG_CONFIG_HOME` and `XDG_STATE_HOME` directories under `${AMF_SHOT_DIR:-/tmp/amf-shots}`, launches a dedicated fixed-size tmux session, and removes the scratch instance afterward. It also detects and removes any tmux sessions spawned by the scenario.

Keep these constraints:

- Leave `HOME` unchanged so agent authentication and git identity remain available.
- Use the default `120x40` geometry unless the UI needs another size.
- Use `--keep` only while debugging; use an explicit persistent `--out-dir` for deliverables.
- Treat GitHub reads from PR Triage as real read-only operations. Do not confirm actions that reply, resolve, or otherwise write to GitHub unless the user explicitly requested that write.

## Author the scenario

Inspect the UI change and read these examples before authoring a scenario:

- `scripts/dev/screenshot/scenarios/dashboard-tour.txt`
- `scripts/dev/screenshot/scenarios/create-project-flow.txt`

Write one step per line. Separate multiple steps on a line with `|`:

- `key:<name>` sends a tmux key such as `Enter`, `j`, or `Escape`.
- `text:<literal>` types literal text.
- `wait:<ms>` waits for a redraw or asynchronous operation.
- `shot:<label>` captures numbered `.ansi` and plain-text `.txt` files.

Add a shot at each frame that materially demonstrates the requested behavior. Prefer a scratch scenario file unless it is broadly reusable.

When the scenario needs existing data, pass `--seed` and optionally `--seed-feature`. Use the payload shapes in `scripts/dev/screenshot/scenarios/seed-project.json`, `seed-feature.json`, and `docs/automation/`.

## Capture and render

Run the scenario with a persistent output directory:

```bash
scripts/dev/screenshot/amf-capture.sh \
  --scenario <scenario-file> \
  --out-dir <output-directory>
```

For normal screenshot requests, render each captured ANSI file:

```bash
python3 scripts/dev/screenshot/render_ansi.py <capture>.ansi \
  --out <capture>.png --cols 120 --rows 40
```

Match `--cols` and `--rows` to the capture geometry. Pass `--gif` to the driver only when the user asks for a GIF or animation; the driver then renders the frames and assembles the GIF itself.

## Verify and deliver

Search the escape-free `.txt` twins for the expected dialog titles, labels, entered text, and status messages. Do not read `.ansi` files into context. Then use the local image viewer on one or two representative PNGs to check layout, clipping, and color.

Return the visual files as the primary result:

- Embed representative PNGs in the final response with Markdown image syntax using absolute local paths so the Codex client can display them.
- Link every PNG or GIF with an absolute local file link, in story order, and give each one a short caption describing what it proves.
- Mention the scenario and output directory so the capture is reproducible.

Do not claim the UI is proven when the text assertions or representative visual inspection fail. Fix the scenario or implementation and rerun the capture first.
