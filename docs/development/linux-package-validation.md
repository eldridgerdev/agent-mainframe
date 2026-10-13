# Linux package pair validation

The reproducible runner exercises **released [v0.48.0](https://github.com/eldridgerdev/agent-mainframe/releases/tag/v0.48.0)
and [v0.49.0](https://github.com/eldridgerdev/agent-mainframe/releases/tag/v0.49.0) payloads**,
not binaries built from the current PR. The GUI uses the x86_64 GNU `.deb`;
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

python3 scripts/dev/screenshot/test_linux_package_pair.py
```

Each upgrade order owns a temporary installation root, dpkg database, XDG
configuration/state/data/cache and private tmux socket. `HOME` stays unchanged.
The `.deb` is installed/configured with `fakeroot dpkg --root=...`; all dpkg logs
also go to the output directory. The TUI archive goes into that root's private
`opt/amf`, with directory replacement preserving the old mapped tmux binary.
The runner checks GUI package name/version/architecture/dependencies and the
installed dpkg status, and runs the TUI's real `--version` and dashboard.

Five checkpoints cover the original pair, a GUI-only upgrade, the completed
pair, a TUI-only upgrade in a separate fixture, and completion in the opposite
order. At each checkpoint:

- Installing one interface leaves the other interface's binary hash unchanged.
- Both interfaces reopen the same project, feature and session records. The
  TUI dashboard is started before the GUI opens and again after it closes.
- The original tmux session, pane, pane process and server IDs remain identical,
  including after the GUI closes. The native GUI reattaches and its real
  `terminal_input` command reaches the same unpaid raw receiver.
- SQLite integrity remains valid. The released additive migrations advance
  schema 43 to 45, and the older released interface can reopen that fixture
  after the newer interface migrates it: the v0.48.0 TUI after the v0.49.0 GUI
  in one order, the v0.48.0 GUI after the v0.49.0 TUI in the other.

`package-validation.json` records versions, binary hashes, schema versions,
the live tmux server's version beside the installed client's, process
identities and received input. With `--capture`, five
native PNGs and `capture-notes.jsonl` accompany it. Windows are selected by the
owned GUI PID; cleanup stops only owned child groups and the private tmux server.
All temporary installation roots are removed, including on assertion failure.
The screenshot scenario is
[`gui-linux-package-pair.txt`](../../scripts/dev/screenshot/scenarios/gui-linux-package-pair.txt).

Local validation on Ubuntu 22.04/WSLg passed both upgrade orders with the musl
TUI and Debian GUI packages. The official v0.48.0 GNU TUI **failed to start**
there because it requires `GLIBC_2.39`; the runner does not inject a loader or
silently fall back to a different asset. Use the musl TUI on that host, or test
the GNU artifact on a compatible host. The GUI `.deb` itself launched there.

This is a **rootless payload and compatibility test**, not an apt installation:
`--force-depends` is deliberate because the private dpkg database does not own
the host GTK/WebKit libraries. Dependency resolution, desktop-menu integration,
system uninstall/upgrade, AppImage/FUSE, other architectures, and macOS/Finder,
signing and notarization remain unvalidated. Compatibility here covers these
two released versions and this additive schema fixture, not arbitrary version
mixes, downgrades or concurrent mutation of the same feature. No current-source
GUI capability is implied by these older release screenshots.
