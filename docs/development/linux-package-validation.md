# Linux package pair validation

The reproducible runner exercises **released [v0.48.0](https://github.com/eldridgerdev/agent-mainframe/releases/tag/v0.48.0)
and [v0.49.0](https://github.com/eldridgerdev/agent-mainframe/releases/tag/v0.49.0) payloads**,
not binaries built from the current PR. The default GUI uses the x86_64 GNU `.deb`;
the default TUI uses the x86_64 musl tarball, including its bundled tmux.
Release asset SHA-256 values are pinned from GitHub's release asset metadata.
Downloads come from `eldridgerdev/agent-mainframe` and fail if their digest
does not match. The runner does not build a release or start a paid harness.

Run on x86_64 Linux with Python 3 including `tarfile.data_filter`, `fakeroot`,
`dpkg`, `gh` for downloads, GTK 3/WebKitGTK 4.1,
Python `websocket`, and an X11 display. The capture wrapper also needs Python
GI/Gdk/Xlib. It uses the existing display locally; when no display is available,
it needs `xvfb-run`, `xauth` and `dbus-run-session`. Only the isolated CI branch
of the wrapper installs prerequisites. Local runs never install system packages.

```sh
# Exercise actual packaged GUIs and TUI startup, without saving images.
python3 scripts/dev/screenshot/validate-linux-package-pair.py /tmp/package-pair

# When native visual proof is explicitly requested:
bash scripts/dev/screenshot/validate-linux-package-pair.sh /tmp/package-pair-proof

# Optional pre-downloaded assets, under v0.48.0/ and v0.49.0/:
AMF_PACKAGE_ASSETS_DIR=/tmp/release-assets \
  bash scripts/dev/screenshot/validate-linux-package-pair.sh /tmp/package-pair-proof

# Check the GNU TUI instead on a host with the required glibc:
python3 scripts/dev/screenshot/validate-linux-package-pair.py \
  /tmp/package-pair-gnu --tui-target gnu

# AppImage extract-and-run: no FUSE mount or system installation.
AMF_GUI_PACKAGE=appimage AMF_PACKAGE_ASSETS_DIR=/tmp/release-assets \
  bash scripts/dev/screenshot/validate-linux-package-pair.sh /tmp/appimage-pair-proof

python3 scripts/dev/screenshot/test_linux_package_pair.py
```

Each upgrade order owns a temporary installation root, XDG
configuration/state/data/cache and private tmux socket, plus a private dpkg
database for the `.deb` run (an AppImage run creates none). `HOME` stays unchanged.
The `.deb` is installed/configured with `fakeroot dpkg --root=...`; all dpkg logs
also go to the output directory. The TUI archive goes into that root's private
`opt/amf`, with directory replacement preserving the old mapped tmux binary.
The runner checks GUI package name/version/architecture/dependencies and the
installed dpkg status, and runs the TUI's real `--version` and dashboard.

With `--gui-package appimage`, it instead verifies the official AppImage digest,
atomically replaces an executable in the private root, and launches the actual
AppImage runtime with `--appimage-extract-and-run`. Extraction uses a private
`TMPDIR` and does not require FUSE. Screenshots accept only windows in the live,
unreaped AppImage launcher's private process group, covering its child GUI;
foreign groups and stale launcher identities are rejected.

Five checkpoints cover the original pair, a GUI-only upgrade, the completed
pair, a TUI-only upgrade in a separate fixture, and completion in the opposite
order. At each checkpoint:

- Installing one interface leaves the other interface's binary hash unchanged.
- Both interfaces reopen the same project, feature and session records.
- The original tmux session, pane, pane process and server IDs remain identical,
  including after the GUI closes. The native GUI reattaches and its real
  `terminal_input` command reaches the same unpaid raw receiver.
- SQLite integrity remains valid. The released additive migrations advance
  schema 43 to 45, and the older released interface can reopen that fixture
  after the newer interface migrates it.

The unpaid receiver starts and reports ready before either GUI opens, creating
its tmux server independently of AppImage's environment. Every identity requires
nonempty numeric session, pane and process IDs: `tmux display-message` succeeding
with empty target fields does not count as a live session.

`package-validation.json` records versions, package kind, binary hashes, schema versions,
tmux version, process identities and received input. With `--capture`, five
native PNGs and `capture-notes.jsonl` accompany it. Windows are selected by the
owned GUI PID; cleanup stops only owned child groups and the private tmux server.
A group is reaped only after every member has exited, escalating to `SIGKILL`,
so an AppImage GUI cannot outlive its launcher into the next step.
All temporary installation roots are removed, including on assertion failure.
The screenshot scenario is
[`gui-linux-package-pair.txt`](../../scripts/dev/screenshot/scenarios/gui-linux-package-pair.txt),
or [`gui-linux-appimage-pair.txt`](../../scripts/dev/screenshot/scenarios/gui-linux-appimage-pair.txt).

Local validation on Ubuntu 22.04/WSLg passed both upgrade orders with the musl
TUI and Debian GUI packages, and separately with the released AppImages using
extract-and-run in both upgrade orders. The official v0.48.0 GNU TUI **failed to start**
there because it requires `GLIBC_2.39`; the runner does not inject a loader or
silently fall back to a different asset. Use the musl TUI on that host, or test
the GNU artifact on a compatible host. The GUI `.deb` itself launched there.

A separate fresh-server probe found a released AppImage packaging failure on
Ubuntu 22.04: when the GUI starts the private tmux server first, that server
inherits AppRun's `PYTHONHOME` and `PYTHONPATH` pointing into the extracted image.
`/usr/bin/python3 --version` succeeds, but a new Python process importing `json`
exits with `ModuleNotFoundError: No module named 'encodings'`. This reproduces
with both v0.48.0 and v0.49.0. The existing-session proof creates its receiver
before the GUI opens and **does not validate new Python process startup under
an AppImage-created server**. No AppRun environment or released payload is
modified to conceal that failure; fixing the runtime boundary is separate work.

This is a **rootless payload and compatibility test**, not an apt installation:
`--force-depends` is deliberate because the private dpkg database does not own
the host GTK/WebKit libraries. Dependency resolution, desktop-menu integration,
system uninstall/upgrade, AppImage FUSE mounts/desktop integration, other architectures, and macOS/Finder,
signing and notarization remain unvalidated. Compatibility here covers these
two released versions and this additive schema fixture, not arbitrary version
mixes, downgrades or concurrent mutation of the same feature. No current-source
GUI capability is implied by these older release screenshots.
