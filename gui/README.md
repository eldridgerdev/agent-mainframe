# AMF GUI development preview

This is the Tauri desktop interface for AMF. The `amf` terminal interface
remains available and uses the same project database and tmux sessions.

## Run from source

Install the current stable Rust toolchain, Node.js 22.12 or later, a C compiler,
`tmux`, and the harness CLIs you intend to use. On Linux, install the system
libraries in [Tauri's Linux prerequisites](https://v2.tauri.app/start/prerequisites/).
Run the app from a shell with `tmux` and the harness CLIs on `PATH`:

```sh
cd gui
npm ci
npm run tauri dev
```

For a standalone release-mode binary without an installer, run
`npm run tauri -- build --no-bundle --ci`. The executable is
`target/release/amf-gui` at the repository root. This build still needs
`tmux` and the harness CLIs available at runtime.

On Windows, use a Linux build inside WSL2 with WSLg. Native Windows installers
are outside this preview's platform scope.

## Current coverage

The [workflow inventory](../docs/backlog/amf-gui-workflow-inventory.md) labels
each available, limited, and planned GUI workflow. The GUI currently supports
project and feature creation, session terminals, TODO lists and agent starts,
and Full and Quick Plan interviews. Continue to use `amf` for workflows
marked Planned.

Both interfaces read the existing `~/.config/amf/amf.db`. The GUI checks for
external workspace and TODO changes every two seconds. Each feature page has
one tab per session; leaving a session's tab detaches the GUI's view and leaves
the tmux agent session running, while Stop ends the feature session. Agent
starts that hit AMF's resource warning ask for explicit approval.

## Checks

```sh
cd gui
npm test
npm run build
cd ..
cargo test --workspace --locked
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Installers, signing, macOS Finder launch environment, and independently
versioned release updates remain packaging work. The repository's existing
release workflow publishes the TUI only.
