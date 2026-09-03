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
- `note:<text>` explains what the immediately following `shot:` proves; use a
  complete reviewer-facing sentence.
- `shot:<label>` captures numbered `.ansi` and plain-text `.txt` files.

Add a shot at each frame that materially demonstrates the requested behavior.
Put a `note:` immediately before every published shot—state the visible state
and why it proves the flow. Prefer a scratch scenario file unless it is broadly
reusable.

When the scenario needs existing data, pass `--seed` and optionally `--seed-feature`. Use the payload shapes in `scripts/dev/screenshot/scenarios/seed-project.json`, `seed-feature.json`, and `docs/automation/`.

For a screenshot of an already-completed AI Review, use
`scenarios/ai-review-completed-fixture.txt`. It uses AMF's deterministic
`seed-ai-review` fixture; CI must never start a live `A` review or depend on a
logged-in Claude/Codex harness for visual proof.

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

## Publish and deliver

Search the escape-free `.txt` twins for the expected dialog titles, labels, entered text, and status messages. Do not read `.ansi` files into context. Then use the local image viewer on one or two representative PNGs to check layout, clipping, and color.

When the branch and scenario are pushed and the user wants the proof attached to
an **open** PR, run the repository's agent-driven publisher. The deploy step
runs locally and needs `CLOUDFLARE_ACCOUNT_ID` set (it is, in the owner's
shell), `wrangler` (or `npx`) available, and a `CLOUDFLARE_API_TOKEN` in the
environment.

**Do not run `wrangler login` — it is unreliable here, and you must not mint a
token yourself.** The token is a Pages-scoped API token the repository owner
keeps in `~/.secrets/cf-amf-pages.env`; the `amf-publish-screenshots` shell
function sources that file and `exec`s `publish-pages.sh`. Invoke it through the
wrapper, or source the file first, so the run is non-interactive:

```bash
amf-publish-screenshots \
  --pr <number> \
  --scenario scripts/dev/screenshot/scenarios/<scenario>.txt \
  --summary "One sentence explaining the complete flow under review" \
  --ref <pushed-branch> \
  --strict

# equivalently, from a non-login shell:
( set -a; . ~/.secrets/cf-amf-pages.env; set +a
  scripts/dev/screenshot/publish-pages.sh --pr <number> \
    --scenario scripts/dev/screenshot/scenarios/<scenario>.txt \
    --summary "..." --ref <pushed-branch> --strict )
```

If `publish-pages.sh` still reports missing Cloudflare auth after that (the
secrets file is absent or the token is unset), surface the warning and stop.

Add `--gif` only when the user asks for animation. The command dispatches the
isolated **capture-only** workflow, then locally downloads the rendered frames,
builds a script-free CSP-locked gallery, deploys it to Cloudflare Pages with
`wrangler`, and replaces only the PR body section between
`<!-- amf:screenshots:start -->` and `<!-- amf:screenshots:end -->`. The
Pages gallery is restricted by Cloudflare Access; raw ANSI/text captures remain
in a 14-day internal artifact and screenshots are never committed to the branch.
The gallery index displays the flow summary first, followed by ordered frames
with their `note:` explanation under **What this proves**.

Publication is a real external write and must be explicitly requested. The
publisher accepts only the `eldridgerdev` GitHub identity; it rejects another
`gh` login before dispatching, and serializes capture dispatches. There is no
`screenshot-pages` environment or per-run approval: the Cloudflare token stays
on the local machine and never enters CI, and the deploy step only handles
rendered images, never the captured ref's code. If capture, deploy, or the PR
update does not finish, report the actionable warning and do not claim that the
PR has visual proof.

Return the publication result as the primary result:

- Link the PR and private Cloudflare Pages gallery.
- Report any warning the command printed; do not expose artifact URLs or
  raw ANSI/text captures in the PR.
- If no PR publication was requested, link every local PNG or GIF in story order and give each one a short caption describing what it proves.

Do not claim the UI is proven when the text assertions or representative visual inspection fail. Fix the scenario or implementation and rerun the capture first.
