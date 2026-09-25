# AMF GUI development preview

This is the Tauri desktop interface for AMF. The `amf` terminal interface
remains available and uses the same project database and tmux sessions.

## Install a release build

Each AMF release on GitHub includes GUI builds for x86_64 and aarch64 Linux,
and for Apple Silicon Macs, next to the `amf` downloads. They carry the same version number as `amf`.
Pick the file for your machine's architecture: `x86_64-unknown-linux-gnu`
for Intel and AMD, `aarch64-unknown-linux-gnu` for ARM64.

- **Debian/Ubuntu:** download the `.deb`, for example
  `amf-gui-x86_64-unknown-linux-gnu.deb`, and install it with
  `sudo apt install ./amf-gui-x86_64-unknown-linux-gnu.deb`, which also
  installs `tmux`. Launch it from your applications menu or run `amf-gui`.
- **Other distributions:** download the `.AppImage` for your architecture, run
  `chmod +x` on it, and run it. Install `tmux` yourself.

Either way, the harness CLIs you use (`claude`, `codex`, `opencode`, `pi`) must
be on `PATH`. For a Mac, follow [macOS](#macos) below; on Windows, follow
[Windows (WSL2)](#windows-wsl2).

## macOS

Releases include `amf-gui-aarch64-apple-darwin.dmg` for Apple Silicon Macs
(M1 and later). There is no Intel Mac build; build from source on an Intel Mac.

The maintainer doesn't currently have a Mac available to set up and test Apple
Developer ID signing and notarization. CI builds the macOS releases with an
ad-hoc signature, so downloaded builds can show a **"developer cannot be
verified"** warning. Developer ID signing and notarization remain pending.

1. **Install `tmux`,** for example with `brew install tmux`, and install the
   agent CLIs you use.
2. **Open the `.dmg` and drag AMF GUI into Applications.**
3. **Approve it the first time.** The app is not signed with an Apple
   Developer ID yet, so macOS blocks the first launch with a warning that it
   can't verify the app. Close the warning, open **System Settings → Privacy &
   Security**, scroll down and click **Open Anyway** next to AMF GUI, then
   confirm. macOS remembers this, so later launches open normally.

   On a company-managed Mac, **Open Anyway** may be unavailable. See
   [Apple's explanation of these warnings](https://support.apple.com/en-us/102445)
   and the local build option below.

Apps opened from Finder, the Dock or Spotlight don't inherit your shell's
`PATH`, so at startup the GUI asks your login shell (`$SHELL`, usually zsh)
for its `PATH`. As long as `tmux` and your agent CLIs work in a new terminal
window, the GUI will find them. If your shell's startup files take longer than
three seconds, the GUI gives up and keeps the system `PATH`; launch it from a
terminal with `open -a "AMF GUI"` instead.

### Build locally without a Developer ID

You can build and run AMF on your own Mac without a Developer ID certificate or
a paid Apple Developer account. A local build normally avoids the downloaded-app
quarantine involved in that warning. Standard Git clients don't normally
quarantine their checkouts; see
[Apple's developer guidance](https://developer.apple.com/forums/thread/773965).
Company security policies can still restrict locally built programs; a source
build does not guarantee approval on a managed Mac.

Install the current stable Rust toolchain, Node.js **22.12 or later**, `tmux`,
and the agent CLIs you use. For the compiler and macOS SDK, Xcode Command Line
Tools are sufficient; the full Xcode app is not required for this desktop build.
See [Tauri's macOS prerequisites](https://v2.tauri.app/start/prerequisites/#macos).
If the command line tools aren't installed, run:

```sh
xcode-select --install
```

Clone AMF with Git onto the Mac. From the repository root, build and launch a
standalone executable:

```sh
cd gui
npm ci
npm run tauri -- build --no-bundle --ci
../target/release/amf-gui
```

The executable is `target/release/amf-gui` at the repository root. Run it again
from a terminal whenever you want to open the GUI; `tmux` and your agent CLIs
must remain on `PATH`.

For development with automatic rebuilding, run this from `gui/` after
`npm ci` instead:

```sh
npm run tauri dev
```

## Windows (WSL2)

There is no native Windows build. AMF runs every agent session in tmux, and
tmux doesn't run on Windows, so the GUI runs as a Linux app inside WSL2 and
opens as a normal window through WSLg.

You need Windows 11, or Windows 10 version 21H2 or later with the Microsoft
Store version of WSL.

1. **Install WSL2 with Ubuntu.** In PowerShell as administrator, run:

   ```powershell
   wsl --install -d Ubuntu
   ```

   Restart when asked, then open **Ubuntu** from the Start menu and create your
   Linux user. If WSL was already installed, run `wsl --update` instead to get
   a version with WSLg.

2. **Download the GUI inside Ubuntu.** Use the Ubuntu terminal, not Windows,
   so the `.deb` ends up in the Linux file system. On an ARM PC, use
   `aarch64-unknown-linux-gnu` in place of `x86_64-unknown-linux-gnu`.

   ```sh
   curl -LO https://github.com/eldridgerdev/agent-mainframe/releases/latest/download/amf-gui-x86_64-unknown-linux-gnu.deb
   ```

3. **Install it.** This also installs `tmux` and the GUI's runtime libraries:

   ```sh
   sudo apt update
   sudo apt install ./amf-gui-x86_64-unknown-linux-gnu.deb
   ```

4. **Install your agent CLIs inside Ubuntu.** Install `claude`, `codex`,
   `opencode` or `pi` in Ubuntu the same way as on Linux. A CLI installed on the
   Windows side isn't visible to the GUI.

5. **Keep your repositories in the Linux file system,** for example under
   `~/code`, rather than under `/mnt/c`. Git and agents are much slower across
   the Windows boundary, and worktrees are created next to the repository.

6. **Start the GUI.** Run `amf-gui` in the Ubuntu terminal. WSLg usually also
   adds an **AMF GUI** entry to the Windows Start menu, marked with your distro's
   name.

The GUI and `amf` share one database inside WSL (`~/.config/amf/amf.db`), so if
you also use `amf` there, both see the same projects and sessions. The GUI's
terminal includes the icon fonts agent status lines use, so you don't need a
Nerd Font installed in WSL.

## Run from source

Install the current stable Rust toolchain, Node.js 22.12 or later, a C compiler,
`tmux`, and the harness CLIs you intend to use. On Linux, install the system
libraries in [Tauri's Linux prerequisites](https://v2.tauri.app/start/prerequisites/).
On macOS, see [Build locally without a Developer ID](#build-locally-without-a-developer-id)
for prerequisites and a standalone build that doesn't need a signing account.
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

On Windows, build from source inside WSL2 the same way, after step 1 of
[Windows (WSL2)](#windows-wsl2).

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

The release workflow (`.github/workflows/release.yml`) builds the `.deb` and
AppImage for x86_64 and aarch64 Linux and the `.dmg` for Apple Silicon from
each version tag. If a GUI build fails, the `amf` release is still published.
The macOS build is ad-hoc signed, not signed with a Developer ID or notarized.
Developer ID signing, notarization, Intel Mac builds and in-app updates remain
packaging work.
