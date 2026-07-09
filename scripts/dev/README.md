# Dev scripts

Tooling for developing and releasing AMF itself — not shipped to end
users, not embedded in the binary.

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
