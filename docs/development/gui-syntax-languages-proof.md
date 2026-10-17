# Native syntax language manager proof

The desktop workspace **Syntax languages** action lists the same sixteen
languages as the TUI. Each row distinguishes an absent parser, a loadable
installed parser, and a partial or unloadable install that needs repair.
Install, Repair and Uninstall require explicit confirmation. Refresh reads the
current files and resets the runtime highlighting cache; it changes no files.
The existing per-file installer and this manager share one background worker.
Closing the manager leaves that worker running; reopening observes its progress.

Actions bind the listed language state, source checkout identity, library bytes
and query bytes. A changed revision refuses the request before mutation. The
coordinator excludes overlapping operations within the desktop process, and
operation IDs reject late progress/completion replies. Another process can
still edit files after validation; this is not a filesystem transaction. Failed
operations report an error and refresh actual state rather than claiming rollback.
Output is bounded. Diff, Final Review and Learning stay mounted behind the
manager, retaining selections and unsent drafts.

## Reproduce

From a matching source/debug build with the native GUI prerequisites installed:

```sh
scripts/dev/screenshot/amf-capture.sh \
  --scenario scripts/dev/screenshot/scenarios/gui-syntax-manager.txt \
  --out-dir /path/to/private-proof
```

The scenario uses real WebKit controls through the maintained AMF GUI driver and
real Tauri IPC. It owns a temporary Git repository/worktree, SQLite database,
XDG directories and tmux socket. HOME stays unchanged, with a bubblewrap mount
for the private `~/.amf` runtime. Digest guards verify real user harness settings
and runtime inventory remain unchanged. No paid answering or editing harness runs.
The runner refuses missing namespace isolation; the shared opt-in helper only
adjusts an ephemeral GitHub-hosted Linux runner's namespace prerequisite when
needed and restores its own adjustment afterward.

Preparation fetches the official `tree-sitter/tree-sitter-json` commit
`ee35a6ebefcef0c5c416c0d1ccec7370cfca5a24`, the commit pinned by AMF's shared
package specification. A run-owned Git config rewrites this exact repository
URL to a private local mirror. GUI operations then use AMF's unmodified clone,
checkout, real C compiler, shared-library build and parser-load operations.
A fixture-only compiler wrapper waits on a bounded gate to make busy state and
close/reopen deterministic, then delegates to the actual compiler. No global
Git configuration is written. `grammar-provenance.json` records this boundary.

Eleven asserted frames cover the full catalog, cancelled install, a stale
uninstall after external library replacement, busy repair and close/reopen,
loadable installation, highlighted Learning content, discovery of a broken
external replacement, repair, uninstall and an unchanged unsent Learning
question. Every frame checks the original feature/session rows and pane PIDs.
`assertions.json`, `isolation.json` and the capture notes accompany the rendered
frames; only the PNG gallery is published for reviewers.
