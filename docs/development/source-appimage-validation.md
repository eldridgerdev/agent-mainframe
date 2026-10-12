# Source-built debug AppImage validation

This local probe exercises an actual, unmodified debug AppImage through AppRun.
It validates the fresh-server Python environment fix independently of the hosted
`gui-appimage-python.txt` scenario, which injects released AppRun variables into
a source-built GUI child.

Use a Linux desktop with the normal Tauri build prerequisites, `tmux`, host
`/usr/bin/python3`, `nm`, `rg`, and the native screenshot runner's Python/WebKit
dependencies. Install frontend dependencies first. Keep `HOME` unchanged and
leave `TAURI_CONFIG` unset. Reserve the display and an exclusively owned Cargo
target; the build helper cleans **only** the `agent-mainframe` and `amf-gui`
packages in that target, retaining dependency caches.

```bash
CARGO_TARGET_DIR=/absolute/path/to/exclusive/target \
  bash scripts/dev/screenshot/build-debug-appimage.sh /tmp/amf-debug-artifact
/usr/bin/python3 scripts/dev/screenshot/capture-gui-source-appimage.py \
  /tmp/amf-debug-artifact/amf-gui-debug-<short-source-ref>.AppImage \
  /tmp/amf-debug-proof
```

The build uses `npm run tauri -- build --ci --debug --bundles appimage -- --locked`
with a private `TMPDIR` and extract-and-run packaging tools. It records the source
ref and artifact SHA-256 in `build-report.json`, and checks the actual packaged
debug executable for the Python-environment fix symbol. Rebuilding both product
packages prevents a shared target from reusing another worktree's stale core
library. An initial local artifact was rejected for exactly that stale linkage;
it is not evidence for the recorded source ref.

The probe preserves `HOME`, creates private XDG directories, a private extraction
directory and tmux socket, and launches the AppImage before starting receivers.
It supplies a host Python test-module path, but never injects or removes AppRun
variables and never patches the payload. It verifies that AppRun's bundle Python
redirects exist in the actual GUI process, that the GUI-created server removes
those redirects while preserving the host search path, and that two host Python
receivers import both `json` and the host module. Nonempty receiver identities,
empty received-prompt arrays, embedded `tauri://localhost` (or Tauri's HTTP
custom-protocol equivalent) assets, an unchanged artifact hash and owned-process
cleanup are required. Only the explicitly allowlisted Python/AppRun environment
fields are recorded in `source-appimage-proof.json`. Two native PNGs show the
live receivers; the runner sends no prompt and starts no paid harness.

Validated locally on 2026-10-12:

- Product source: `b6842320f716273d34351ac4360b21159f4eb713`.
- Debug AppImage SHA-256:
  `e032ae18625da2e2e1fc8491509f3b2b2e790ee4d4b96b840d08ad3ebe673e6a`.
- Full probe passed with frontend URL `tauri://localhost`; both receivers
  imported successfully and all owned GUI/tmux processes closed.

This establishes debug AppImage extract-and-run behavior for that source and
artifact. It does not establish release-artifact behavior, FUSE integration,
desktop installation, system dependency resolution, macOS signing, or package
upgrade continuity. The released-package continuity probe remains separate.
