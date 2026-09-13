+++
title = "Installation"
description = "Install AMF from a release bundle or build it from source."
weight = 10
+++

## Requirements

Install at least one supported agent CLI and sign in to it before using AMF:

- [Claude Code](https://docs.anthropic.com/en/docs/claude-code)
- [Codex](https://github.com/openai/codex)
- [OpenCode](https://opencode.ai)
- Pi (`pi` must be available in `PATH`)

Release bundles include `tmux`. A source installation requires `tmux` in
`PATH`. Git is required for branch and worktree features, but AMF can manage
non-git directories without it.

Optional tools:

- An authenticated [GitHub CLI](https://cli.github.com/) (`gh`) enables PR
  triage, AI review, posting final-review feedback, and the dashboard's
  open/merged/closed PR badge.
- A [Nerd Font](https://www.nerdfonts.com/) provides the best icon rendering.
- The `code` command enables VS Code sessions.

## Release bundle (recommended)

Download the archive for your platform from
[GitHub Releases](https://github.com/eldridgerdev/agent-mainframe/releases),
extract it, and place `amf` somewhere in your `PATH`.

| Platform | Archive |
| --- | --- |
| Linux x86_64, most portable | `amf-x86_64-unknown-linux-musl.tar.gz` |
| Linux x86_64, glibc | `amf-x86_64-unknown-linux-gnu.tar.gz` |
| Linux aarch64 | `amf-aarch64-unknown-linux-gnu.tar.gz` |
| macOS, Apple Silicon | `amf-aarch64-apple-darwin.tar.gz` |

For example, on Linux x86_64:

```bash
curl -L https://github.com/eldridgerdev/agent-mainframe/releases/latest/download/amf-x86_64-unknown-linux-musl.tar.gz -o amf.tar.gz
tar -xzf amf.tar.gz
sudo mv amf-x86_64-unknown-linux-musl /opt/amf
sudo ln -s /opt/amf/amf /usr/local/bin/amf
```

On macOS with Apple Silicon:

```bash
curl -L https://github.com/eldridgerdev/agent-mainframe/releases/latest/download/amf-aarch64-apple-darwin.tar.gz -o amf.tar.gz
tar -xzf amf.tar.gz
sudo install -m 755 amf-aarch64-apple-darwin/amf /usr/local/bin/amf
```

## Build from source

Building uses the current stable Rust toolchain, a C compiler, and `tmux`:

```bash
git clone https://github.com/eldridgerdev/agent-mainframe
cd agent-mainframe
cargo install --path . --locked
```

## Upgrade

```bash
amf upgrade
amf -V
```

See the project's [CHANGELOG](https://github.com/eldridgerdev/agent-mainframe/blob/main/CHANGELOG.md)
for release notes and migration guidance.
