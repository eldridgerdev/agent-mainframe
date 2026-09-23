# AMF GUI development preview

This is the Tauri desktop interface for AMF. The `amf` terminal interface
remains available and uses the same project database and tmux sessions.

## Install a release build

Each AMF release on GitHub includes Linux x86_64 GUI builds next to the `amf`
downloads. They carry the same version number as `amf`.

- **Debian/Ubuntu:** download `amf-gui-x86_64-unknown-linux-gnu.deb` and
  install it with `sudo apt install ./amf-gui-x86_64-unknown-linux-gnu.deb`,
  which also installs `tmux`. Launch it from your applications menu or run
  `amf-gui`.
- **Other distributions:** download `amf-gui-x86_64-unknown-linux-gnu.AppImage`,
  run `chmod +x` on it, and run it. Install `tmux` yourself.

Either way, the harness CLIs you use (`claude`, `codex`, `opencode`, `pi`) must
be on `PATH`. On Windows, install the Linux build inside WSL2 and it opens
through WSLg. macOS builds are not published yet; build from source instead.

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

The release workflow (`.github/workflows/release.yml`) builds the Linux `.deb`
and AppImage from each version tag. If the GUI build fails, the `amf` release
is still published. macOS builds, code signing, the macOS Finder launch
environment and in-app updates remain packaging work.
