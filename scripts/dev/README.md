# Dev scripts

Tooling for developing and releasing AMF itself. The AI interaction drivers and
their shared helper are also embedded in AMF and injected with the
[`amf-interact` skill](../../skills/amf-interact/SKILL.md) into managed projects.
Other scripts here are development tooling.

- `release.sh` — bump the version, validate `CHANGELOG.md`, tag, and
  push a release
- `package-release-bundle.sh` — used by the release CI workflow to
  package a built binary into a release archive
- `package-no-tmux-test-bundle.sh`, `run-no-tmux-docker.sh` — build
  and run the no-tmux Docker test image (see
  [`docs/docker-no-tmux.md`](../../docs/docker-no-tmux.md))
- `generate-amf-themes.sh` / `.js` — regenerate the bundled OpenCode
  theme presets from upstream
- `test-thinking.sh` — manually pulse the "thinking" sentinel for a
  tmux session to test dashboard detection without a real agent
- `amf/pr-checks.sh`, `amf/pr-info.sh` — PR context helpers used by
  the `amf:pr-*` Claude Code skills
- `gui-driver/amf-gui-driver.py` — persistent, isolated native GUI interaction
  with compact JSON snapshots, batched actions and local waits; see the
  [AI GUI driver guide](../../docs/development/gui-ai-driver.md)
- `tui-driver/amf-tui-driver.py` — persistent, isolated tmux TUI interaction,
  changed-row observations, batched keys/paste, local waits and scoped automation;
  see the [AI TUI driver guide](../../docs/development/tui-ai-driver.md)
- `ai_driver.py` — shared JSON protocol, output budgets, isolation and cleanup
  used by both drivers

For focused and full tests, runner prerequisites, dependency audits and debug
logging, see [development checks](../../docs/development/checks.md).
Choose feature modules and test suites using the
[architecture guide](../../docs/development/architecture.md).
