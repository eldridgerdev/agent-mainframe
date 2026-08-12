# Scripts

Scripts in this directory (not `scripts/dev/`) are runtime hook
scripts embedded into the `amf` binary via `include_str!`
(`src/app/setup.rs`) and written out to a feature's
`.claude/settings.local.json` / `.opencode/` hooks at runtime. Users
never run these directly. Generated Claude shell helpers are staged under
`~/.amf/hooks` so their executable paths remain safe on macOS, whose standard
config directory contains the space-bearing `Library/Application Support`.

- `notify.sh`, `clear-notify.sh`, `save-prompt.sh`,
  `thinking-start.sh`, `thinking-stop.sh`, `tool-start.sh`,
  `tool-stop.sh` — Claude Code hook scripts
- `codex-notify.sh`, `codex-diff-review.sh` — Codex hook scripts
- `set-session-status.sh` — custom-session status reporting, called
  from a user's own hook script

See [`scripts/dev/`](dev/) for release tooling and manual test
helpers used when developing AMF itself.
